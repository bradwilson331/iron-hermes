use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_cron::JobStore;

use crate::memory_tool::SharedMemoryManager;

/// D-09 / D-25 (Phase 25): per-tool prerequisite descriptor for setup-wizard discovery.
/// Plain-String type per cross-crate convention (Phase 22.4.2.2 → 23 D-12 → 24 D-17 → 25 D-25).
#[derive(Debug, Clone)]
pub struct Prerequisite {
    /// "env_var" | "config_field" (string union per D-25; downstream matches on kind at call site).
    pub kind: String,
    /// e.g. "FIRECRAWL_API_KEY" or "search.brave_api_key" (dotted-path config key).
    pub name: String,
    /// Human-readable description shown by the setup wizard (D-18).
    pub description: String,
    /// true = blocks is_available() when missing; false = optional / advisory only.
    pub required: bool,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn toolset(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;

    /// Default: walk prerequisites(), return true iff every required env_var prereq is satisfied.
    /// Tools may override for custom logic (e.g., "either KEY_A or KEY_B") but MUST also
    /// implement prerequisites() for setup-wizard discovery (D-09).
    fn is_available(&self) -> bool {
        self.prerequisites()
            .iter()
            .filter(|p| p.required)
            .all(|p| match p.kind.as_str() {
                "env_var" => std::env::var(&p.name).is_ok(),
                "config_field" => true, // checked at config load site, not at trait level
                _ => true,              // unknown kinds are non-blocking by design (D-25)
            })
    }

    /// Per-tool prerequisite list for setup-wizard discovery (D-09 / Phase 25).
    /// Default returns empty Vec (most tools have no external prerequisites).
    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![]
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    guardrails: Vec<Box<dyn ironhermes_hooks::GuardrailHook>>,
    error_detail: ironhermes_hooks::ErrorDetailLevel,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            guardrails: Vec::new(),
            error_detail: ironhermes_hooks::ErrorDetailLevel::Full,
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Register a tool dynamically (e.g., from MCP discovery). Per D-10.
    /// Functionally identical to register() -- the name distinction is semantic
    /// (dynamic = runtime MCP vs static = startup built-in).
    pub fn register_dynamic(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Remove all tools whose name starts with `{server_name}__`.
    /// Called on /reload-mcp to clear one server's tools before re-registering.
    /// Returns the number of tools removed.
    pub fn unregister_by_prefix(&mut self, server_name: &str) -> usize {
        let prefix = format!("{server_name}__");
        let before = self.tools.len();
        self.tools.retain(|name, _| !name.starts_with(&prefix));
        before - self.tools.len()
    }

    /// Add a guardrail hook that will be checked before every tool dispatch.
    /// Guardrails are checked in registration order.
    /// Per D-05: register BlocklistGuardrail first, custom trait hooks second.
    pub fn add_guardrail(&mut self, hook: Box<dyn ironhermes_hooks::GuardrailHook>) {
        self.guardrails.push(hook);
    }

    /// Set the error detail level for guardrail block messages.
    pub fn set_error_detail(&mut self, level: ironhermes_hooks::ErrorDetailLevel) {
        self.error_detail = level;
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn get_definitions(&self, enabled_tools: Option<&[String]>) -> Vec<ToolSchema> {
        self.tools
            .values()
            .filter(|t| t.is_available())
            .filter(|t| {
                enabled_tools
                    .map(|list| list.iter().any(|name| name == t.name()))
                    .unwrap_or(true)
            })
            .map(|t| t.schema())
            .collect()
    }

    pub async fn dispatch(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        self.dispatch_with_hook(name, args, None::<fn(&str, &str)>).await
    }

    /// Check all registered guardrails for the given tool call WITHOUT executing the tool.
    ///
    /// Returns the first non-Allow decision (Block wins immediately), or Allow if all
    /// guardrails pass. Warn decisions are returned as-is — the caller decides whether
    /// to log or surface them.
    ///
    /// Used by agent_loop.rs to implement D-05 ordering:
    ///   check_guardrails → (Block → ToolCompleted{false}) | (Allow|Warn → ToolCalled → execute_tool → ToolCompleted)
    pub fn check_guardrails(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> ironhermes_hooks::GuardrailDecision {
        let mut last_warn = None;
        for guardrail in &self.guardrails {
            match guardrail.check(name, args) {
                ironhermes_hooks::GuardrailDecision::Allow => {}
                ironhermes_hooks::GuardrailDecision::Warn { reason } => {
                    tracing::warn!(
                        tool = %name,
                        guardrail = %guardrail.name(),
                        reason = %reason,
                        "Guardrail warning (proceeding)"
                    );
                    last_warn = Some(ironhermes_hooks::GuardrailDecision::Warn { reason });
                    // Continue -- a later guardrail might Block
                }
                ironhermes_hooks::GuardrailDecision::Block { reason } => {
                    return ironhermes_hooks::GuardrailDecision::Block { reason };
                }
            }
        }
        last_warn.unwrap_or(ironhermes_hooks::GuardrailDecision::Allow)
    }

    /// Execute a tool by name with the given args, WITHOUT running guardrail checks.
    ///
    /// Callers MUST call `check_guardrails` first and only call this on Allow/Warn.
    /// This is the execution-only half of the D-05 split API.
    pub async fn execute_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;

        if !tool.is_available() {
            return Err(anyhow::anyhow!("Tool '{}' is not available", name));
        }

        tool.execute(args).await
    }

    /// Return the configured error detail level for guardrail block messages.
    /// Used by agent_loop.rs to format block errors with the same detail level
    /// as dispatch_with_hook (preserves LLM-visible error format, T-07.4-06).
    pub fn guardrail_error_detail(&self) -> &ironhermes_hooks::ErrorDetailLevel {
        &self.error_detail
    }

    /// Dispatch a tool call, optionally firing a hook after the guardrail chain permits
    /// but before the tool executes.
    ///
    /// The `post_guardrail_hook` closure is called with `(tool_name, args_str)` only when
    /// every guardrail returns Allow or Warn — never when a guardrail blocks. This ensures
    /// `ToolCalled` hook events are emitted only for permitted calls (HOOK-01 ordering fix).
    pub async fn dispatch_with_hook<F>(
        &self,
        name: &str,
        args: serde_json::Value,
        post_guardrail_hook: Option<F>,
    ) -> anyhow::Result<String>
    where
        F: FnOnce(&str, &str),
    {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;

        if !tool.is_available() {
            return Err(anyhow::anyhow!("Tool '{}' is not available", name));
        }

        // Guardrail intercept (HOOK-02): check all guardrails before dispatch.
        // Per D-05: config blocklist is registered first, trait hooks second.
        // T-06-04: args reference is the same one passed to tool.execute() — no copy-after-check gap.
        for guardrail in &self.guardrails {
            match guardrail.check(name, &args) {
                ironhermes_hooks::GuardrailDecision::Allow => {}
                ironhermes_hooks::GuardrailDecision::Warn { reason } => {
                    tracing::warn!(
                        tool = %name,
                        guardrail = %guardrail.name(),
                        reason = %reason,
                        "Guardrail warning (proceeding)"
                    );
                    // Continue to next guardrail — warn does not block
                }
                ironhermes_hooks::GuardrailDecision::Block { reason } => {
                    let error_msg = ironhermes_hooks::format_guardrail_error(
                        name,
                        &reason,
                        guardrail.name(),
                        &self.error_detail,
                    );
                    return Err(anyhow::anyhow!("{}", error_msg));
                }
            }
        }

        // All guardrails passed — fire the post-guardrail hook before execution.
        // This is where ToolCalled events should be emitted (after permit, before execute).
        let args_str = args.to_string();
        if let Some(hook) = post_guardrail_hook {
            hook(name, &args_str);
        }

        tool.execute(args).await
    }

    pub fn list_tools(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.tools.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    pub fn register_defaults(&mut self) {
        use crate::file_tools::{PatchFileTool, ReadFileTool, SearchFilesTool, WriteFileTool};
        use crate::terminal::TerminalTool;
        use crate::web_read::WebReadTool;
        use crate::web_search::WebSearchTool;

        self.register(Box::new(TerminalTool::new()));
        self.register(Box::new(ReadFileTool));
        self.register(Box::new(WriteFileTool));
        self.register(Box::new(PatchFileTool));
        self.register(Box::new(SearchFilesTool));
        self.register(Box::new(WebSearchTool));
        self.register(Box::new(WebReadTool));
    }

    /// Register the memory tool with a shared `MemoryManager` handle (Plan 20-02).
    ///
    /// The handle delegates writes through the manager so the optional mirror
    /// provider is kept in sync. Callers build the handle via
    /// `ironhermes_agent::memory::factory::build_memory_manager`.
    pub fn register_memory_tool(&mut self, manager: SharedMemoryManager) {
        use crate::memory_tool::MemoryTool;
        self.register(Box::new(MemoryTool::new(manager)));
    }

    /// Register the cronjob tool with a shared JobStore.
    /// Called separately from register_defaults() because it requires a JobStore instance.
    pub fn register_cronjob_tool(&mut self, store: Arc<Mutex<JobStore>>) {
        use crate::cronjob_tool::CronjobTool;
        self.register(Box::new(CronjobTool::new(store)));
    }

    /// Register the skills tool with a shared SkillRegistry and active_skills tracker.
    /// Called separately from register_defaults() because it requires a SkillRegistry instance.
    ///
    /// Phase 19 Plan 03: now also takes `credential_dir` (root for per-skill credentials,
    /// per D-10) and `skills_config` (per-skill config map reserved for Plan 04 injection).
    pub fn register_skills_tool(
        &mut self,
        registry: Arc<ironhermes_core::SkillRegistry>,
        active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
        credential_dir: std::path::PathBuf,
        skills_config: std::collections::HashMap<String, std::collections::HashMap<String, serde_yaml::Value>>,
    ) {
        use crate::skills_tool::SkillsTool;
        self.register(Box::new(SkillsTool::new(
            registry,
            active_skills,
            credential_dir,
            skills_config,
        )));
    }

    /// Register the delegate_task tool with a SubagentRunner, semaphore, and config.
    ///
    /// The `runner` implements the `SubagentRunner` trait (defined in delegate_task.rs)
    /// and is typically constructed in ironhermes-agent to wrap AgentLoop::run().
    pub fn register_delegate_task_tool(
        &mut self,
        runner: Arc<dyn crate::delegate_task::SubagentRunner>,
        semaphore: Arc<tokio::sync::Semaphore>,
        memory_manager: Option<SharedMemoryManager>,
        config: ironhermes_core::SubagentConfig,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
        progress_callback: Option<crate::delegate_task::SubagentProgressCallback>,
    ) {
        use crate::delegate_task::DelegateTaskTool;
        let mut tool = DelegateTaskTool::new(
            runner, semaphore, memory_manager, config, cancel_token,
        );
        if let Some(cb) = progress_callback {
            tool = tool.with_progress_callback(cb);
        }
        self.register(Box::new(tool));
    }

    /// Register the execute_code tool with a separate RPC dispatch registry.
    ///
    /// `rpc_registry` must contain ONLY D-07 safe tools (no terminal, no execute_code).
    /// This is built separately from the main registry to structurally prevent recursion
    /// and terminal access from sandboxed scripts.
    ///
    /// Called AFTER all other tools are registered but BEFORE wrapping in Arc.
    pub fn register_execute_code_tool(
        &mut self,
        rpc_registry: Arc<ToolRegistry>,
        config: ironhermes_core::ExecConfig,
    ) {
        use crate::execute_code::ExecuteCodeTool;
        self.register(Box::new(ExecuteCodeTool::new(rpc_registry, config, None)));
    }

    /// Phase 19 Plan 06 (D-05): register execute_code with shared access to the
    /// active-skills list so skill-declared env vars bypass the sandbox secret-strip.
    pub fn register_execute_code_tool_with_active_skills(
        &mut self,
        rpc_registry: Arc<ToolRegistry>,
        config: ironhermes_core::ExecConfig,
        active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
    ) {
        use crate::execute_code::ExecuteCodeTool;
        self.register(Box::new(ExecuteCodeTool::with_active_skills(
            rpc_registry,
            config,
            None,
            active_skills,
        )));
    }

    /// Phase 21.7-06 (D-29): register execute_code with BOTH active-skills
    /// bypass AND a shared `ProcessRegistry` handle for the `background=true`
    /// branch. Replaces the `_with_active_skills` registration at the three
    /// CLI + gateway call sites so INV-21.7-03 totals 3 new + 0 legacy after
    /// Plan 06 wiring lands. Foreground (sandbox) mode is unchanged.
    pub fn register_execute_code_tool_with_process_registry(
        &mut self,
        rpc_registry: Arc<ToolRegistry>,
        config: ironhermes_core::ExecConfig,
        active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
        process_registry: Arc<tokio::sync::RwLock<ironhermes_exec::process_registry::ProcessRegistry>>,
    ) {
        use crate::execute_code::ExecuteCodeTool;
        let tool = ExecuteCodeTool::with_active_skills(
            rpc_registry,
            config,
            None,
            active_skills,
        )
        .with_process_registry(process_registry);
        self.register(Box::new(tool));
    }

    /// Phase 21.7-06 (D-29): register a `TerminalTool` whose `background=true`
    /// branch is wired to the session-scoped `ProcessRegistry`. Foreground
    /// behaviour is unchanged. Called from the three CLI sites + gateway
    /// runner when background spawning is desired.
    pub fn register_terminal_tool_with_process_registry(
        &mut self,
        process_registry: Arc<tokio::sync::RwLock<ironhermes_exec::process_registry::ProcessRegistry>>,
    ) {
        use crate::terminal::TerminalTool;
        let tool = TerminalTool::new().with_process_registry(process_registry);
        self.register(Box::new(tool));
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::ToolSchema;
    use ironhermes_hooks::{BlocklistGuardrail, GuardrailDecision, GuardrailHook};
    use std::sync::{Mutex, OnceLock};

    // ---------------------------------------------------------------------------
    // env_lock: serialise tests that mutate environment variables.
    // Copied from crates/ironhermes-cli/tests/profile_isolation.rs pattern.
    // Phase 21.6 D: Rust 2024 edition requires unsafe for env var mutation.
    // ---------------------------------------------------------------------------

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    // ---------------------------------------------------------------------------
    // Mock tool for testing
    // ---------------------------------------------------------------------------

    struct MockTool {
        tool_name: &'static str,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            self.tool_name
        }
        fn toolset(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "mock tool for testing"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new(
                self.tool_name,
                self.description(),
                serde_json::json!({ "type": "object", "properties": {} }),
            )
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("mock result".to_string())
        }
    }

    // ---------------------------------------------------------------------------
    // Warn-only guardrail for testing
    // ---------------------------------------------------------------------------

    struct WarnGuardrail;

    impl GuardrailHook for WarnGuardrail {
        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> GuardrailDecision {
            GuardrailDecision::Warn {
                reason: "always warn".to_string(),
            }
        }
        fn name(&self) -> &str {
            "warn-always"
        }
    }

    fn make_registry_with_tool(tool_name: &'static str) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool { tool_name }));
        registry
    }

    // ---------------------------------------------------------------------------
    // Test tools for prerequisite tests
    // ---------------------------------------------------------------------------

    /// Tool with no prerequisites() override — uses the default empty Vec.
    struct NoPrereqTool;

    #[async_trait]
    impl Tool for NoPrereqTool {
        fn name(&self) -> &str { "no_prereq" }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "tool with no prerequisites" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("no_prereq", "tool with no prerequisites",
                serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        // prerequisites() intentionally NOT overridden — tests the default
    }

    /// Tool with one required env_var prerequisite.
    struct RequiredEnvPrereqTool;

    #[async_trait]
    impl Tool for RequiredEnvPrereqTool {
        fn name(&self) -> &str { "required_env_prereq" }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "tool with required env_var prerequisite" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("required_env_prereq", "tool with required env_var prerequisite",
                serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "env_var".to_string(),
                name: "TEST_PREREQ_25_01_PRESENT".to_string(),
                description: "Test prerequisite env var for Phase 25 Plan 01 unit tests.".to_string(),
                required: true,
            }]
        }
    }

    /// Tool with one optional (required:false) env_var prerequisite.
    struct OptionalEnvPrereqTool;

    #[async_trait]
    impl Tool for OptionalEnvPrereqTool {
        fn name(&self) -> &str { "optional_env_prereq" }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "tool with optional env_var prerequisite" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("optional_env_prereq", "tool with optional env_var prerequisite",
                serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "env_var".to_string(),
                name: "TEST_PREREQ_25_01_PRESENT".to_string(),
                description: "Optional test prerequisite env var.".to_string(),
                required: false,
            }]
        }
    }

    /// Tool with a config_field prerequisite (should never block is_available()).
    struct ConfigFieldPrereqTool;

    #[async_trait]
    impl Tool for ConfigFieldPrereqTool {
        fn name(&self) -> &str { "config_field_prereq" }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "tool with config_field prerequisite" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("config_field_prereq", "tool with config_field prerequisite",
                serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "config_field".to_string(),
                name: "search.brave_api_key".to_string(),
                description: "Config field prerequisite — checked at config load, not trait level.".to_string(),
                required: true,
            }]
        }
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 01: Prerequisite struct + default is_available() tests
    // ---------------------------------------------------------------------------

    /// Test 1: A struct implementing Tool with no prerequisites() override returns
    /// empty Vec from prerequisites().
    #[test]
    fn prerequisite_default_impl_returns_empty() {
        let tool = NoPrereqTool;
        let prereqs = tool.prerequisites();
        assert!(prereqs.is_empty(), "default prerequisites() must return empty Vec");
    }

    /// Test 2: A test Tool whose prerequisites() returns one required env_var prereq,
    /// when the env var IS set, has is_available() == true.
    #[test]
    fn is_available_default_walks_prerequisites_required_env_var_present() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::set_var("TEST_PREREQ_25_01_PRESENT", "1") };
        let tool = RequiredEnvPrereqTool;
        let available = tool.is_available();
        unsafe { std::env::remove_var("TEST_PREREQ_25_01_PRESENT") };
        assert!(available, "is_available() must be true when required env_var prereq is set");
    }

    /// Test 3: Same Tool when the env var is NOT set has is_available() == false.
    #[test]
    fn is_available_default_walks_prerequisites_required_env_var_absent() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("TEST_PREREQ_25_01_PRESENT") };
        let tool = RequiredEnvPrereqTool;
        let available = tool.is_available();
        assert!(!available, "is_available() must be false when required env_var prereq is absent");
    }

    /// Test 4: A test Tool with required:false for an unset env var has is_available() == true
    /// (optional prereqs do not block).
    #[test]
    fn is_available_default_walks_prerequisites_optional_env_var_absent() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("TEST_PREREQ_25_01_PRESENT") };
        let tool = OptionalEnvPrereqTool;
        let available = tool.is_available();
        assert!(available, "is_available() must be true when only optional prereqs are absent");
    }

    /// Test 5: A prerequisite with kind "config_field" (or any non-"env_var" kind) does NOT
    /// block is_available() — the default treats it as satisfied (config-level checks happen
    /// elsewhere per D-09).
    #[test]
    fn is_available_default_treats_unknown_kind_as_satisfied() {
        let tool = ConfigFieldPrereqTool;
        // config_field prereqs are required:true but still non-blocking at trait level
        let available = tool.is_available();
        assert!(available,
            "is_available() must be true for config_field prereqs (checked at config load, not here)");
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 01 Task 2: D-01 toolset name enumeration test
    // ---------------------------------------------------------------------------

    /// Verify that every built-in tool's toolset() return value matches the D-01
    /// six-name enumeration: {web, code, memory, agent, skills, session}.
    ///
    /// For unit-struct tools (no constructor complexity), instantiate directly and
    /// assert toolset(). For CronjobTool (requires Arc<Mutex<JobStore>>), use the
    /// source-text invariant approach (include_str!) per Phase 22.3-12 pattern —
    /// verifies the literal "agent" is in the toolset() impl block.
    #[test]
    fn toolset_names_match_d01_enumeration() {
        use crate::file_tools::{PatchFileTool, ReadFileTool, SearchFilesTool, WriteFileTool};
        use crate::terminal::TerminalTool;

        // Direct instantiation for tools with trivial constructors
        assert_eq!(TerminalTool::new().toolset(), "code",
            "TerminalTool must be in 'code' toolset per D-01");
        assert_eq!(ReadFileTool.toolset(), "code",
            "ReadFileTool must be in 'code' toolset per D-01");
        assert_eq!(WriteFileTool.toolset(), "code",
            "WriteFileTool must be in 'code' toolset per D-01");
        assert_eq!(PatchFileTool.toolset(), "code",
            "PatchFileTool must be in 'code' toolset per D-01");
        assert_eq!(SearchFilesTool.toolset(), "code",
            "SearchFilesTool must be in 'code' toolset per D-01");

        // Source-text invariant for CronjobTool (requires Arc<Mutex<JobStore>> constructor).
        // Verifies that the toolset() impl block returns "agent" per D-01 Open Question 1 resolution.
        let cronjob_src = include_str!("cronjob_tool.rs");
        // Find the toolset() impl block and verify "agent" literal is present
        // (and "cronjob" is NOT present as a toolset return value)
        let toolset_section: String = cronjob_src
            .lines()
            .skip_while(|l| !l.contains("fn toolset"))
            .take(5)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            toolset_section.contains("\"agent\""),
            "CronjobTool::toolset() must return \"agent\" per D-01; found:\n{toolset_section}"
        );
        assert!(
            !toolset_section.contains("\"cronjob\""),
            "CronjobTool::toolset() must NOT return \"cronjob\" (fixed by Plan 1); found:\n{toolset_section}"
        );
    }

    // ---------------------------------------------------------------------------
    // register_dynamic tests (D-10)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_register_dynamic_inserts_tool() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool { tool_name: "dyn_tool" }));
        assert!(registry.get("dyn_tool").is_some(), "dynamically registered tool must be retrievable by name");
    }

    #[test]
    fn test_register_dynamic_overwrites_existing() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool { tool_name: "my_tool" }));
        registry.register_dynamic(Box::new(MockTool { tool_name: "my_tool" }));
        // Should still be exactly one tool named "my_tool"
        let names = registry.list_tools();
        let count = names.iter().filter(|&&n| n == "my_tool").count();
        assert_eq!(count, 1, "register_dynamic must overwrite, not duplicate");
    }

    // ---------------------------------------------------------------------------
    // unregister_by_prefix tests (D-10)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_unregister_by_prefix_removes_matching_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool { tool_name: "server__tool_a" }));
        registry.register_dynamic(Box::new(MockTool { tool_name: "server__tool_b" }));
        registry.register_dynamic(Box::new(MockTool { tool_name: "other__tool_c" }));

        let removed = registry.unregister_by_prefix("server");
        assert_eq!(removed, 2, "must remove both 'server__' prefixed tools");
        assert!(registry.get("server__tool_a").is_none(), "server__tool_a must be removed");
        assert!(registry.get("server__tool_b").is_none(), "server__tool_b must be removed");
    }

    #[test]
    fn test_unregister_by_prefix_does_not_remove_other_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool { tool_name: "server__tool_a" }));
        registry.register_dynamic(Box::new(MockTool { tool_name: "other__tool_c" }));

        registry.unregister_by_prefix("server");
        assert!(registry.get("other__tool_c").is_some(), "other__tool_c must NOT be removed");
    }

    #[test]
    fn test_unregister_by_prefix_returns_count() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool { tool_name: "srv__a" }));
        registry.register_dynamic(Box::new(MockTool { tool_name: "srv__b" }));
        registry.register_dynamic(Box::new(MockTool { tool_name: "srv__c" }));

        let count = registry.unregister_by_prefix("srv");
        assert_eq!(count, 3, "unregister_by_prefix must return count of removed tools");
    }

    #[test]
    fn test_unregister_by_prefix_empty_registry_returns_zero() {
        let mut registry = ToolRegistry::new();
        let count = registry.unregister_by_prefix("server");
        assert_eq!(count, 0, "unregister_by_prefix on empty registry must return 0");
    }

    #[test]
    fn test_unregister_by_prefix_no_match_returns_zero() {
        let mut registry = ToolRegistry::new();
        registry.register_dynamic(Box::new(MockTool { tool_name: "other__tool" }));
        let count = registry.unregister_by_prefix("x");
        assert_eq!(count, 0, "unregister_by_prefix with no matching prefix must return 0");
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dispatch_with_no_guardrails_passes() {
        let registry = make_registry_with_tool("test_tool");
        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    #[tokio::test]
    async fn test_dispatch_blocked_by_guardrail() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "test_tool".to_string(),
        ])));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_err(), "expected Err (blocked), got Ok");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.to_lowercase().contains("blocked")
                || err_msg.contains("blocklist")
                || err_msg.contains("security policy"),
            "error should mention block: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_dispatch_allowed_by_guardrail() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "other_tool".to_string(),
        ])));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "expected Ok (allowed), got {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    #[tokio::test]
    async fn test_dispatch_warn_guardrail_proceeds() {
        let mut registry = make_registry_with_tool("test_tool");
        registry.add_guardrail(Box::new(WarnGuardrail));

        let result = registry
            .dispatch("test_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_ok(), "warn guardrail must not block: {result:?}");
        assert_eq!(result.unwrap(), "mock result");
    }

    #[tokio::test]
    async fn test_guardrail_error_detail_minimal() {
        let mut registry = make_registry_with_tool("secret_tool");
        registry.set_error_detail(ironhermes_hooks::ErrorDetailLevel::Minimal);
        registry.add_guardrail(Box::new(BlocklistGuardrail::new(vec![
            "secret_tool".to_string(),
        ])));

        let result = registry
            .dispatch("secret_tool", serde_json::Value::Null)
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert_eq!(
            err_msg, "Tool call blocked by security policy",
            "minimal detail must not leak tool name: {err_msg}"
        );
        assert!(
            !err_msg.contains("secret_tool"),
            "tool name must not appear in minimal error: {err_msg}"
        );
    }
}
