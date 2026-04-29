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

/// D-12 / D-14 (Phase 25): async handler for intercepted tools.
/// Resolves Open Question 3 — async because execute_tool_call() is async; spawn_blocking
/// for sync-StateStore stays inside the closure body (see session_search migration in Plan 3).
///
/// Security (T-25-04): library-internal use only — handler closures MUST be constructed
/// by the workspace (ironhermes-agent::AgentLoop::with_intercepts), NOT deserialized from
/// config or user input. The `with_intercepts(...)` builder in Plan 3 accepts only the five
/// known-safe handles (memory_manager, state_store, subagent_runner, todo_state, cron_router).
pub type InterceptHandler = std::sync::Arc<
    dyn Fn(serde_json::Value) -> futures::future::BoxFuture<'static, anyhow::Result<String>>
        + Send
        + Sync,
>;

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    guardrails: Vec<Box<dyn ironhermes_hooks::GuardrailHook>>,
    error_detail: ironhermes_hooks::ErrorDetailLevel,
    /// D-14 (Phase 25): intercepted tools stored separately from regular tools.
    /// get_definitions() returns schemas from BOTH maps; D-15 prevents name collisions.
    intercepts: HashMap<String, (ToolSchema, InterceptHandler)>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            guardrails: Vec::new(),
            error_detail: ironhermes_hooks::ErrorDetailLevel::Full,
            intercepts: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        assert!(
            !self.intercepts.contains_key(&name),
            "register: name '{}' already registered as an intercepted tool — schema duplication blocked at registry build (D-15)",
            name,
        );
        self.tools.insert(name, tool);
    }

    /// Register a tool dynamically (e.g., from MCP discovery). Per D-10.
    /// Functionally identical to register() -- the name distinction is semantic
    /// (dynamic = runtime MCP vs static = startup built-in).
    /// D-15: Also guards against intercept name collisions for MCP-discovered tools.
    pub fn register_dynamic(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        assert!(
            !self.intercepts.contains_key(&name),
            "register_dynamic: name '{}' already registered as an intercepted tool — schema duplication blocked at registry build (D-15)",
            name,
        );
        self.tools.insert(name, tool);
    }

    /// Register an intercepted tool by name, schema, and async handler (D-12 / D-14, Phase 25).
    ///
    /// Intercepted tools are NOT in the regular `tools` HashMap; they live in a separate
    /// `intercepts` map. `get_definitions()` returns schemas from BOTH maps so the LLM sees
    /// the full surface, but `dispatch_intercepts()` handles them before `dispatch()` is called.
    ///
    /// D-15 reciprocal guard: panics if `name` is already registered as a regular tool —
    /// schema duplication is structurally impossible.
    ///
    /// Security (T-25-04): library-internal use only — handler closures MUST be constructed
    /// by the workspace (ironhermes-agent::AgentLoop::with_intercepts), NOT deserialized from
    /// config or user input.
    pub fn register_intercepted(
        &mut self,
        name: &str,
        schema: ToolSchema,
        handler: InterceptHandler,
    ) {
        assert!(
            !self.tools.contains_key(name),
            "register_intercepted: name '{}' already registered as a regular tool — schema duplication blocked at registry build (D-15)",
            name,
        );
        self.intercepts.insert(name.to_string(), (schema, handler));
    }

    /// Dispatch a tool call to the intercepts map (D-12, Phase 25).
    ///
    /// Returns `Some(result)` when the tool is intercepted; `None` to fall through to
    /// the normal `dispatch()` path. The agent_loop call site is responsible for:
    /// ```rust,ignore
    /// if let Some(r) = registry.dispatch_intercepts(name, args.clone()).await {
    ///     return r;
    /// }
    /// registry.dispatch(name, args).await
    /// ```
    pub async fn dispatch_intercepts(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Option<anyhow::Result<String>> {
        let (_schema, handler) = self.intercepts.get(name)?;
        Some(handler(args).await)
    }

    /// Returns (tool_name, [unsatisfied required prereqs]) for every Tool whose
    /// required prereqs are missing. Used by Plan 5's preflight banner (D-17).
    /// Only checks `kind == "env_var"` at the trait level; `kind == "config_field"`
    /// is checked at config-load, not here (D-08 / D-09).
    pub fn list_unavailable(&self) -> Vec<(String, Vec<Prerequisite>)> {
        self.tools
            .values()
            .filter_map(|t| {
                let missing: Vec<_> = t
                    .prerequisites()
                    .into_iter()
                    .filter(|p| {
                        p.required
                            && match p.kind.as_str() {
                                "env_var" => std::env::var(&p.name).is_err(),
                                _ => false, // config_field handled at config-load layer
                            }
                    })
                    .collect();
                if missing.is_empty() { None } else { Some((t.name().to_string(), missing)) }
            })
            .collect()
    }

    /// Unique set of toolset() values across all currently-registered tools, sorted alphabetically.
    /// Per D-03: membership is read at runtime from the trait, no separate table.
    /// Only includes regular tools (from the `tools` map); intercepted tools have no Tool::toolset()
    /// method. Plan 4's `hermes toolset list` presents intercepted-only names through a separate path.
    pub fn list_toolsets(&self) -> Vec<String> {
        let mut s: std::collections::HashSet<String> =
            self.tools.values().map(|t| t.toolset().to_string()).collect();
        let mut v: Vec<String> = s.drain().collect();
        v.sort();
        v
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
        let mut schemas: Vec<ToolSchema> = self
            .tools
            .values()
            .filter(|t| t.is_available())
            .filter(|t| {
                enabled_tools
                    .map(|list| list.iter().any(|name| name == t.name()))
                    .unwrap_or(true)
            })
            .map(|t| t.schema())
            .collect();
        // Phase 25 D-14: union with intercept schemas. Same enabled_tools filter applies.
        // Toolset-level filter (D-23) added in Plan 3 once toolset_config exists.
        schemas.extend(
            self.intercepts
                .iter()
                .filter(|(name, _)| {
                    enabled_tools
                        .map(|list| list.iter().any(|n| n == name.as_str()))
                        .unwrap_or(true)
                })
                .map(|(_, (schema, _))| schema.clone()),
        );
        schemas
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
// Phase 25 D-13 / Open Question 2: greenfield todo_* schema constructors.
// These are free functions (not Tool impls) because the in-session state
// (Arc<Mutex<Vec<String>>>) is owned by AgentLoop, not a Tool struct.
// Plan 3 wires real handlers via AgentLoop::with_intercepts(). Plan 2 ships
// only the schema constructors — do NOT register them in ToolRegistry::new().
// ---------------------------------------------------------------------------

/// Phase 25 D-13 / Open Question 2 (Plan 2): minimal greenfield schema for the
/// intercepted `todo_write` tool. `items` replaces the current todo list.
/// In-session state lives in `Arc<Mutex<Vec<String>>>` owned by AgentLoop and
/// passed to `with_intercepts()` (D-16, wired in Plan 3).
pub fn todo_write_schema() -> ToolSchema {
    ToolSchema::new(
        "todo_write",
        "Write (replace) the current todo list for this session. Replaces the entire list with the provided items.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "New todo list items. Replaces the entire current list."
                }
            },
            "required": ["items"]
        }),
    )
}

/// Phase 25 D-13 / Open Question 2 (Plan 2): minimal greenfield schema for
/// `todo_read`. Returns the current list. No required parameters.
pub fn todo_read_schema() -> ToolSchema {
    ToolSchema::new(
        "todo_read",
        "Read the current todo list for this session.",
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    )
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
    // Phase 25 Plan 01 Task 3: web tool prerequisites() and is_available() tests
    // ---------------------------------------------------------------------------

    /// Test: WebSearchTool::prerequisites() returns exactly one Prerequisite with
    /// kind == "env_var", name == "FIRECRAWL_API_KEY", required == true.
    #[test]
    fn web_search_prerequisites_lists_firecrawl_required_true() {
        let tool = crate::web_search::WebSearchTool;
        let prereqs = tool.prerequisites();
        assert_eq!(prereqs.len(), 1, "WebSearchTool must have exactly one prerequisite");
        let p = &prereqs[0];
        assert_eq!(p.kind, "env_var", "WebSearchTool prereq kind must be 'env_var'");
        assert_eq!(p.name, "FIRECRAWL_API_KEY", "WebSearchTool prereq name must be FIRECRAWL_API_KEY");
        assert!(p.required, "WebSearchTool FIRECRAWL_API_KEY prereq must be required:true");
    }

    /// Test: WebReadTool::prerequisites() returns one Prerequisite with
    /// kind == "env_var", name == "FIRECRAWL_API_KEY", required == false.
    #[test]
    fn web_read_prerequisites_lists_firecrawl_required_false() {
        let tool = crate::web_read::WebReadTool;
        let prereqs = tool.prerequisites();
        assert_eq!(prereqs.len(), 1, "WebReadTool must have exactly one prerequisite");
        let p = &prereqs[0];
        assert_eq!(p.kind, "env_var", "WebReadTool prereq kind must be 'env_var'");
        assert_eq!(p.name, "FIRECRAWL_API_KEY", "WebReadTool prereq name must be FIRECRAWL_API_KEY");
        assert!(!p.required, "WebReadTool FIRECRAWL_API_KEY prereq must be required:false (plain-text fallback)");
    }

    /// Test: With FIRECRAWL_API_KEY unset, WebSearchTool::is_available() == false
    /// (kept manual override per D-09).
    #[test]
    fn web_search_is_available_remains_blocked_without_firecrawl() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("FIRECRAWL_API_KEY") };
        let tool = crate::web_search::WebSearchTool;
        let available = tool.is_available();
        assert!(!available,
            "WebSearchTool::is_available() must be false when FIRECRAWL_API_KEY is unset");
    }

    /// Test: With FIRECRAWL_API_KEY unset, WebReadTool::is_available() == true
    /// (required:false does not block; web_read has plain-text fallback per D-09).
    #[test]
    fn web_read_is_available_stays_true_without_firecrawl() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("FIRECRAWL_API_KEY") };
        let tool = crate::web_read::WebReadTool;
        let available = tool.is_available();
        assert!(available,
            "WebReadTool::is_available() must be true when FIRECRAWL_API_KEY is unset (optional prereq)");
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

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 02 Task 1: InterceptHandler + register_intercepted + dispatch_intercepts tests
    // ---------------------------------------------------------------------------

    fn test_handler(response: &'static str) -> InterceptHandler {
        std::sync::Arc::new(move |_args| {
            Box::pin(async move { Ok(response.to_string()) })
        })
    }

    fn test_intercept_schema(name: &str) -> ToolSchema {
        ToolSchema::new(
            name,
            "test intercept tool",
            serde_json::json!({ "type": "object", "properties": {} }),
        )
    }

    /// Test: register_intercepted inserts schema; get_definitions(None) includes it exactly once.
    #[test]
    fn register_intercepted_inserts_schema_and_handler() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted(
            "test_intercept",
            test_intercept_schema("test_intercept"),
            test_handler("hello"),
        );
        let schemas = registry.get_definitions(None);
        let count = schemas.iter().filter(|s| s.function.name == "test_intercept").count();
        assert_eq!(count, 1, "intercepted tool must appear exactly once in get_definitions(None)");
    }

    /// Test: register_intercepted panics when name already registered as a regular tool (D-15).
    #[test]
    #[should_panic(expected = "already registered as a regular tool")]
    fn register_intercepted_panics_on_duplicate_with_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool { tool_name: "dup_name" }));
        registry.register_intercepted("dup_name", test_intercept_schema("dup_name"), test_handler("x"));
    }

    /// Test: register() panics when name already registered as an intercepted tool (D-15 reciprocal).
    #[test]
    #[should_panic(expected = "already registered as an intercepted tool")]
    fn register_tools_panics_on_duplicate_with_intercepts() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted("dup_name", test_intercept_schema("dup_name"), test_handler("x"));
        registry.register(Box::new(MockTool { tool_name: "dup_name" }));
    }

    /// Test: dispatch_intercepts returns Some(Ok("hello")) for a known intercepted tool.
    #[tokio::test]
    async fn dispatch_intercepts_returns_some_for_known() {
        let mut registry = ToolRegistry::new();
        registry.register_intercepted("known", test_intercept_schema("known"), test_handler("hello"));
        let result = registry.dispatch_intercepts("known", serde_json::json!({})).await;
        assert!(result.is_some(), "dispatch_intercepts must return Some for a known intercepted name");
        let inner = result.unwrap();
        assert!(inner.is_ok(), "handler must return Ok");
        assert_eq!(inner.unwrap(), "hello");
    }

    /// Test: dispatch_intercepts returns None for an unknown name (caller falls through to dispatch()).
    #[tokio::test]
    async fn dispatch_intercepts_returns_none_for_unknown() {
        let registry = ToolRegistry::new();
        let result = registry.dispatch_intercepts("unknown", serde_json::json!({})).await;
        assert!(result.is_none(), "dispatch_intercepts must return None for an unregistered name");
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 02 Task 2: get_definitions intercept union + list_unavailable + list_toolsets
    // ---------------------------------------------------------------------------

    /// Test: get_definitions(None) includes schemas from both regular tools and intercepted tools.
    #[test]
    fn get_definitions_includes_intercept_schemas() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool { tool_name: "regular" }));
        registry.register_intercepted("intercept_a", test_intercept_schema("intercept_a"), test_handler("a"));
        registry.register_intercepted("intercept_b", test_intercept_schema("intercept_b"), test_handler("b"));
        let schemas = registry.get_definitions(None);
        let names: std::collections::HashSet<String> = schemas.iter().map(|s| s.function.name.clone()).collect();
        assert_eq!(names.len(), 3, "must have 3 schemas: {names:?}");
        assert!(names.contains("regular"), "missing 'regular'");
        assert!(names.contains("intercept_a"), "missing 'intercept_a'");
        assert!(names.contains("intercept_b"), "missing 'intercept_b'");
    }

    /// Test: enabled_tools filter applies to both regular tools and intercepted tools.
    #[test]
    fn get_definitions_with_enabled_tools_filter_includes_intercepts() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockTool { tool_name: "regular" }));
        registry.register_intercepted("intercept_a", test_intercept_schema("intercept_a"), test_handler("a"));
        registry.register_intercepted("intercept_b", test_intercept_schema("intercept_b"), test_handler("b"));
        let enabled = vec!["regular".to_string(), "intercept_a".to_string()];
        let schemas = registry.get_definitions(Some(&enabled));
        let names: std::collections::HashSet<String> = schemas.iter().map(|s| s.function.name.clone()).collect();
        assert_eq!(names.len(), 2, "must have 2 schemas after filter: {names:?}");
        assert!(names.contains("regular"), "missing 'regular'");
        assert!(names.contains("intercept_a"), "missing 'intercept_a'");
        assert!(!names.contains("intercept_b"), "'intercept_b' must be filtered out");
    }

    /// Tool whose is_available() returns false — used to test filtering.
    struct UnavailableTool;

    #[async_trait]
    impl Tool for UnavailableTool {
        fn name(&self) -> &str { "unavailable" }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "always unavailable" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("unavailable", "always unavailable",
                serde_json::json!({ "type": "object", "properties": {} }))
        }
        fn is_available(&self) -> bool { false }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("never called".to_string())
        }
    }

    /// Test: unavailable regular tools are filtered out; intercepted tools (no is_available) always appear.
    #[test]
    fn get_definitions_filters_unavailable_regular_tools_only() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(UnavailableTool));
        registry.register_intercepted("always_on", test_intercept_schema("always_on"), test_handler("ok"));
        let schemas = registry.get_definitions(None);
        let names: Vec<String> = schemas.iter().map(|s| s.function.name.clone()).collect();
        assert!(!names.contains(&"unavailable".to_string()),
            "unavailable regular tool must be filtered out; got: {names:?}");
        assert!(names.contains(&"always_on".to_string()),
            "intercepted tool must always appear in get_definitions; got: {names:?}");
        assert_eq!(names.len(), 1, "only intercepted tool should appear; got: {names:?}");
    }

    /// Tool B with a required env_var prerequisite for list_unavailable testing.
    struct MissingKeyTool;

    #[async_trait]
    impl Tool for MissingKeyTool {
        fn name(&self) -> &str { "test_b" }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "tool requiring MISSING_KEY_25_02" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("test_b", "tool requiring MISSING_KEY_25_02",
                serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
        fn prerequisites(&self) -> Vec<Prerequisite> {
            vec![Prerequisite {
                kind: "env_var".to_string(),
                name: "MISSING_KEY_25_02".to_string(),
                description: "Test key for Phase 25 Plan 02 list_unavailable test.".to_string(),
                required: true,
            }]
        }
    }

    /// Tool A with no prerequisites for list_unavailable testing.
    struct AlwaysAvailTool;

    #[async_trait]
    impl Tool for AlwaysAvailTool {
        fn name(&self) -> &str { "test_a" }
        fn toolset(&self) -> &str { "test" }
        fn description(&self) -> &str { "always available tool" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("test_a", "always available tool",
                serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    /// Test: list_unavailable() returns tools with missing required prerequisites.
    #[test]
    fn list_unavailable_returns_missing_required_prereqs() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: single-threaded test with env_lock held; Rust 2024 edition requires unsafe.
        unsafe { std::env::remove_var("MISSING_KEY_25_02") };
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(AlwaysAvailTool));
        registry.register(Box::new(MissingKeyTool));
        let unavailable = registry.list_unavailable();
        assert_eq!(unavailable.len(), 1,
            "exactly one tool must be unavailable when MISSING_KEY_25_02 is unset; got: {unavailable:?}");
        let (tool_name, missing) = &unavailable[0];
        assert_eq!(tool_name.as_str(), "test_b",
            "the unavailable tool must be 'test_b'; got: {tool_name}");
        assert_eq!(missing.len(), 1,
            "must have exactly one missing prereq; got: {missing:?}");

        // With env set, returns empty Vec
        unsafe { std::env::set_var("MISSING_KEY_25_02", "1") };
        let unavailable_after = registry.list_unavailable();
        unsafe { std::env::remove_var("MISSING_KEY_25_02") };
        assert!(unavailable_after.is_empty(),
            "list_unavailable must return empty when all prereqs satisfied; got: {unavailable_after:?}");
    }

    /// Tools with different toolsets for list_toolsets testing.
    struct WebTool1;
    struct CodeTool1;
    struct WebTool2;

    #[async_trait]
    impl Tool for WebTool1 {
        fn name(&self) -> &str { "web_tool_1" }
        fn toolset(&self) -> &str { "web" }
        fn description(&self) -> &str { "web tool 1" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("web_tool_1", "web tool 1", serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> { Ok("ok".to_string()) }
    }

    #[async_trait]
    impl Tool for CodeTool1 {
        fn name(&self) -> &str { "code_tool_1" }
        fn toolset(&self) -> &str { "code" }
        fn description(&self) -> &str { "code tool 1" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("code_tool_1", "code tool 1", serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> { Ok("ok".to_string()) }
    }

    #[async_trait]
    impl Tool for WebTool2 {
        fn name(&self) -> &str { "web_tool_2" }
        fn toolset(&self) -> &str { "web" }
        fn description(&self) -> &str { "web tool 2" }
        fn schema(&self) -> ToolSchema {
            ToolSchema::new("web_tool_2", "web tool 2", serde_json::json!({ "type": "object", "properties": {} }))
        }
        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> { Ok("ok".to_string()) }
    }

    /// Test: list_toolsets() returns unique, sorted toolset names from regular tools.
    #[test]
    fn list_toolsets_returns_unique_set() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(WebTool1));
        registry.register(Box::new(CodeTool1));
        registry.register(Box::new(WebTool2));
        let toolsets = registry.list_toolsets();
        assert_eq!(toolsets, vec!["code", "web"],
            "list_toolsets must return deduplicated, sorted toolset names; got: {toolsets:?}");
    }

    // ---------------------------------------------------------------------------
    // Phase 25 Plan 02 Task 3: todo_write_schema, todo_read_schema, D-26 Test 3
    // ---------------------------------------------------------------------------

    /// Test: todo_write_schema() returns a ToolSchema with name "todo_write" and
    /// a required "items" field of type array of strings.
    #[test]
    fn todo_write_schema_minimal_shape() {
        let schema = crate::registry::todo_write_schema();
        assert_eq!(schema.function.name, "todo_write",
            "todo_write_schema must have name 'todo_write'");
        let params = serde_json::to_value(&schema.function.parameters).unwrap();
        let props = &params["properties"];
        assert!(props.get("items").is_some(),
            "todo_write_schema must have 'items' in properties; got: {props}");
        let items_type = props["items"]["type"].as_str().unwrap_or("");
        assert_eq!(items_type, "array",
            "todo_write_schema 'items' must be of type 'array'; got: {items_type}");
        let item_item_type = props["items"]["items"]["type"].as_str().unwrap_or("");
        assert_eq!(item_item_type, "string",
            "todo_write_schema items.items.type must be 'string'; got: {item_item_type}");
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("items")),
            "todo_write_schema must have 'items' in required; got: {required:?}");
    }

    /// Test: todo_read_schema() returns a ToolSchema with name "todo_read"
    /// and empty/no-arg parameters.
    #[test]
    fn todo_read_schema_minimal_shape() {
        let schema = crate::registry::todo_read_schema();
        assert_eq!(schema.function.name, "todo_read",
            "todo_read_schema must have name 'todo_read'");
        let params = serde_json::to_value(&schema.function.parameters).unwrap();
        let props = params["properties"].as_object();
        assert!(
            props.map(|p| p.is_empty()).unwrap_or(true),
            "todo_read_schema must have empty properties; got: {params}"
        );
        // No required fields (or empty required array)
        let required = params.get("required").and_then(|r| r.as_array());
        assert!(
            required.map(|r| r.is_empty()).unwrap_or(true),
            "todo_read_schema must have no required fields; got: {params}"
        );
    }

    /// D-26 Test 3 (mandatory): intercepted_tool_no_schema_duplicate.
    /// Boot registry with all 6 intercepted tool names; assert each appears
    /// exactly once in get_definitions(None).
    #[tokio::test]
    async fn intercepted_tool_no_schema_duplicate() {
        let mut registry = ToolRegistry::new();
        let names = ["memory", "session_search", "delegate_task", "todo_write", "todo_read", "cronjob"];
        for name in names {
            registry.register_intercepted(
                name,
                ToolSchema::new(
                    name,
                    "stub intercepted tool for D-26 Test 3",
                    serde_json::json!({ "type": "object", "properties": {} }),
                ),
                std::sync::Arc::new(|_args| {
                    Box::pin(async move { Ok("stub".to_string()) })
                }),
            );
        }
        let schemas = registry.get_definitions(None);
        let names_returned: Vec<String> = schemas.iter().map(|s| s.function.name.clone()).collect();
        for name in names {
            let count = names_returned.iter().filter(|n| n.as_str() == name).count();
            assert_eq!(
                count, 1,
                "intercepted tool '{}' must appear exactly once in schema list, found {}; all: {:?}",
                name, count, names_returned
            );
        }
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
