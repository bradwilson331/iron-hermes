//! `delegate_task` tool — spawns a child agent with isolated context and restricted toolset.
//!
//! AGENT-01: Child agents get a filtered subset of parent tools
//! AGENT-02: Default safe tool list when no allowlist is specified
//! AGENT-04: Child terminal runs in isolated temp directory
//! AGENT-05: delegate_task is structurally excluded from child registry (no recursion)

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::{MemoryStore, SubagentConfig, ToolSchema};
use serde_json::json;
use tokio::sync::Semaphore;
use tracing::info;

use crate::registry::{Tool, ToolRegistry};

/// Default tools available to subagents when no explicit allowlist is provided (D-02).
const DEFAULT_SAFE_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "patch",
    "search_files",
    "web_search",
    "web_read",
    "memory",
];

/// Trait abstracting the child agent loop execution.
///
/// Defined here (in ironhermes-tools) to avoid a circular dependency:
/// ironhermes-agent depends on ironhermes-tools, so ironhermes-tools
/// cannot import AgentLoop directly. Instead, ironhermes-agent implements
/// this trait and passes it as `Arc<dyn SubagentRunner>`.
#[async_trait]
pub trait SubagentRunner: Send + Sync {
    /// Run a child agent with the given registry, system prompt, and max iterations.
    /// Returns the final text response (or None if the agent produced no text).
    async fn run_child(
        &self,
        registry: Arc<ToolRegistry>,
        system_prompt: String,
        max_iterations: usize,
    ) -> anyhow::Result<Option<String>>;
}

/// Tool that delegates a focused task to a child agent with restricted tools.
pub struct DelegateTaskTool {
    runner: Arc<dyn SubagentRunner>,
    semaphore: Arc<Semaphore>,
    memory_store: Option<Arc<Mutex<MemoryStore>>>,
    config: SubagentConfig,
}

impl DelegateTaskTool {
    pub fn new(
        runner: Arc<dyn SubagentRunner>,
        semaphore: Arc<Semaphore>,
        memory_store: Option<Arc<Mutex<MemoryStore>>>,
        config: SubagentConfig,
    ) -> Self {
        Self {
            runner,
            semaphore,
            memory_store,
            config,
        }
    }
}

/// Build a child ToolRegistry from an allowlist, applying isolation rules.
///
/// - `delegate_task` is silently stripped (AGENT-05: no recursion)
/// - `skills` is silently stripped (D-05: not available to subagents)
/// - `execute_code` is silently stripped (not available to subagents)
/// - `cronjob` is silently stripped (not available to subagents)
/// - Unknown tool names cause an immediate error (D-04: fail-early)
/// - `terminal` gets an isolated CWD (AGENT-04)
/// - `memory` gets read-only mode (D-12)
pub fn build_child_registry(
    allowed_tools: &[String],
    memory_store: Option<Arc<Mutex<MemoryStore>>>,
    child_cwd: &Path,
) -> anyhow::Result<ToolRegistry> {
    let mut registry = ToolRegistry::new();

    for tool_name in allowed_tools {
        match tool_name.as_str() {
            // Structurally excluded tools — silently skip
            "delegate_task" => { /* AGENT-05: no recursion */ }
            "skills" => { /* D-05: not available to subagents */ }
            "execute_code" => { /* not available to subagents */ }
            "cronjob" => { /* not available to subagents */ }

            // File tools
            "read_file" => registry.register(Box::new(crate::file_tools::ReadFileTool)),
            "write_file" => registry.register(Box::new(crate::file_tools::WriteFileTool)),
            "patch" => registry.register(Box::new(crate::file_tools::PatchFileTool)),
            "search_files" => registry.register(Box::new(crate::file_tools::SearchFilesTool)),

            // Web tools
            "web_search" => registry.register(Box::new(crate::web_search::WebSearchTool)),
            "web_read" => registry.register(Box::new(crate::web_read::WebReadTool)),

            // Memory — read-only in child context (D-12)
            "memory" => {
                if let Some(ref store) = memory_store {
                    registry.register(Box::new(
                        crate::memory_tool::MemoryTool::new_read_only(store.clone()),
                    ));
                } else {
                    tracing::warn!("memory tool requested but no MemoryStore available; skipping");
                }
            }

            // Terminal — isolated CWD (AGENT-04)
            "terminal" => {
                registry.register(Box::new(
                    crate::terminal::TerminalTool::with_cwd(child_cwd.to_path_buf()),
                ));
            }

            // Unknown tool — fail early (D-04)
            other => anyhow::bail!("Unknown tool in allowed_tools: {}", other),
        }
    }

    Ok(registry)
}

#[async_trait]
impl Tool for DelegateTaskTool {
    fn name(&self) -> &str {
        "delegate_task"
    }

    fn toolset(&self) -> &str {
        "agent"
    }

    fn description(&self) -> &str {
        "Delegate a focused task to a child agent with a restricted toolset. \
         The child agent runs in an isolated context with its own temp directory \
         and read-only memory access. Use this for parallelizable sub-tasks."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "delegate_task",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Clear description of the task for the child agent to complete."
                    },
                    "allowed_tools": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of tool names the child agent may use. Defaults to safe tools: read_file, write_file, patch, search_files, web_search, web_read, memory."
                    }
                },
                "required": ["task"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let task = args["task"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: task"))?;

        let allowed_tools: Vec<String> = if let Some(tools) = args.get("allowed_tools") {
            tools
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("allowed_tools must be an array"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| anyhow::anyhow!("allowed_tools entries must be strings"))
                        .map(|s| s.to_string())
                })
                .collect::<anyhow::Result<Vec<String>>>()?
        } else {
            DEFAULT_SAFE_TOOLS.iter().map(|s| s.to_string()).collect()
        };

        // Create isolated temp directory for child (D-10, D-13)
        let child_dir = tempfile::TempDir::new()?;

        // Acquire semaphore permit (D-14), measuring actual wait time (D-15)
        let acquire_start = std::time::Instant::now();
        let _permit = self.semaphore.acquire().await
            .map_err(|e| anyhow::anyhow!("Semaphore closed: {}", e))?;
        let wait_duration = acquire_start.elapsed();
        if wait_duration > std::time::Duration::from_millis(50) {
            info!(
                "Acquired subagent slot after waiting {}ms",
                wait_duration.as_millis()
            );
        }

        // Build child registry (D-01..D-05)
        let child_registry = build_child_registry(
            &allowed_tools,
            self.memory_store.clone(),
            child_dir.path(),
        )?;

        // Build system prompt
        let system_prompt = format!(
            "You are a focused assistant. Complete the following task:\n\n{}",
            task
        );

        // Run child agent with timeout (D-08)
        let result = tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs),
            self.runner.run_child(
                Arc::new(child_registry),
                system_prompt,
                self.config.max_iterations,
            ),
        )
        .await;

        // _permit drops here, releasing the semaphore slot
        // child_dir (TempDir) drops here, cleaning up the temp directory (D-13)

        let response = match result {
            Ok(Ok(final_response)) => {
                final_response.unwrap_or_else(|| "(no response)".to_string())
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(anyhow::anyhow!(
                    "Subagent timed out after {}s",
                    self.config.timeout_secs
                ));
            }
        };

        if wait_duration > std::time::Duration::from_millis(50) {
            Ok(format!("[Waited {}ms for subagent slot]\n{}", wait_duration.as_millis(), response))
        } else {
            Ok(response)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // build_child_registry tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_child_registry_with_specific_tools() {
        let registry = build_child_registry(
            &["read_file".to_string(), "write_file".to_string()],
            None,
            Path::new("/tmp"),
        )
        .unwrap();
        let tools = registry.list_tools();
        assert!(tools.contains(&"read_file"), "should have read_file");
        assert!(tools.contains(&"write_file"), "should have write_file");
        assert_eq!(tools.len(), 2, "should have exactly 2 tools");
    }

    #[test]
    fn test_build_child_registry_strips_delegate_task() {
        // AGENT-05: delegate_task must never appear in child registry
        let registry = build_child_registry(
            &[
                "read_file".to_string(),
                "delegate_task".to_string(),
            ],
            None,
            Path::new("/tmp"),
        )
        .unwrap();
        let tools = registry.list_tools();
        assert!(
            !tools.contains(&"delegate_task"),
            "delegate_task must be stripped from child registry (AGENT-05)"
        );
        assert_eq!(tools.len(), 1, "should have only read_file");
    }

    #[test]
    fn test_build_child_registry_strips_skills() {
        // D-05: skills tool not available to subagents
        let registry = build_child_registry(
            &["read_file".to_string(), "skills".to_string()],
            None,
            Path::new("/tmp"),
        )
        .unwrap();
        let tools = registry.list_tools();
        assert!(
            !tools.contains(&"skills"),
            "skills must be stripped from child registry (D-05)"
        );
    }

    #[test]
    fn test_build_child_registry_unknown_tool_errors() {
        // D-04: fail-early on unknown tools
        let result = build_child_registry(
            &["nonexistent_tool".to_string()],
            None,
            Path::new("/tmp"),
        );
        assert!(result.is_err(), "unknown tool should cause error");
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("Unknown tool") && err.contains("nonexistent_tool"),
            "error should name the unknown tool: {err}"
        );
    }

    #[test]
    fn test_build_child_registry_empty_uses_no_tools() {
        // Empty list = empty registry (caller passes DEFAULT_SAFE_TOOLS when no user override)
        let registry = build_child_registry(&[], None, Path::new("/tmp")).unwrap();
        assert!(registry.list_tools().is_empty());
    }

    #[test]
    fn test_build_child_registry_terminal_gets_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let registry = build_child_registry(
            &["terminal".to_string()],
            None,
            dir.path(),
        )
        .unwrap();
        let tools = registry.list_tools();
        assert!(tools.contains(&"terminal"), "should have terminal");
    }

    #[test]
    fn test_build_child_registry_memory_is_read_only() {
        let mem_dir = tempfile::tempdir().unwrap();
        let mut store = MemoryStore::new(mem_dir.path().join("memories"));
        store.load_from_disk().unwrap();
        let store = Arc::new(Mutex::new(store));

        let registry = build_child_registry(
            &["memory".to_string()],
            Some(store),
            Path::new("/tmp"),
        )
        .unwrap();
        let tools = registry.list_tools();
        assert!(tools.contains(&"memory"), "should have memory");
        // The actual read-only behavior is tested in memory_tool::tests
    }

    #[test]
    fn test_build_child_registry_strips_execute_code() {
        let registry = build_child_registry(
            &["read_file".to_string(), "execute_code".to_string()],
            None,
            Path::new("/tmp"),
        )
        .unwrap();
        let tools = registry.list_tools();
        assert!(
            !tools.contains(&"execute_code"),
            "execute_code must be stripped from child registry"
        );
    }

    // -----------------------------------------------------------------------
    // DelegateTaskTool metadata tests
    // -----------------------------------------------------------------------

    struct MockRunner;

    #[async_trait]
    impl SubagentRunner for MockRunner {
        async fn run_child(
            &self,
            _registry: Arc<ToolRegistry>,
            _system_prompt: String,
            _max_iterations: usize,
        ) -> anyhow::Result<Option<String>> {
            Ok(Some("mock child response".to_string()))
        }
    }

    fn make_delegate_tool() -> DelegateTaskTool {
        DelegateTaskTool::new(
            Arc::new(MockRunner),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig::default(),
        )
    }

    #[test]
    fn test_delegate_task_name() {
        let tool = make_delegate_tool();
        assert_eq!(tool.name(), "delegate_task");
    }

    #[test]
    fn test_delegate_task_schema_has_required_task() {
        let tool = make_delegate_tool();
        let schema = tool.schema();
        let params = &schema.function.parameters;
        let required = params["required"].as_array().unwrap();
        assert!(
            required.iter().any(|v| v.as_str() == Some("task")),
            "schema must have 'task' as required"
        );
    }

    #[test]
    fn test_delegate_task_schema_has_allowed_tools() {
        let tool = make_delegate_tool();
        let schema = tool.schema();
        let props = &schema.function.parameters["properties"];
        assert!(props.get("allowed_tools").is_some(), "schema should have allowed_tools");
        assert_eq!(
            props["allowed_tools"]["type"].as_str(),
            Some("array"),
            "allowed_tools should be an array"
        );
    }

    // -----------------------------------------------------------------------
    // DelegateTaskTool execution tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_delegate_task_execute_basic() {
        let tool = make_delegate_tool();
        let result = tool
            .execute(json!({
                "task": "Write a haiku about rust"
            }))
            .await
            .unwrap();
        assert_eq!(result, "mock child response");
    }

    #[tokio::test]
    async fn test_delegate_task_execute_with_allowed_tools() {
        let tool = make_delegate_tool();
        let result = tool
            .execute(json!({
                "task": "Read some files",
                "allowed_tools": ["read_file", "search_files"]
            }))
            .await
            .unwrap();
        assert_eq!(result, "mock child response");
    }

    #[tokio::test]
    async fn test_delegate_task_execute_unknown_tool_fails() {
        let tool = make_delegate_tool();
        let result = tool
            .execute(json!({
                "task": "Do something",
                "allowed_tools": ["bogus_tool"]
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_delegate_task_missing_task_param() {
        let tool = make_delegate_tool();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("task"));
    }

    #[tokio::test]
    async fn test_delegate_task_timeout() {
        struct SlowRunner;

        #[async_trait]
        impl SubagentRunner for SlowRunner {
            async fn run_child(
                &self,
                _registry: Arc<ToolRegistry>,
                _system_prompt: String,
                _max_iterations: usize,
            ) -> anyhow::Result<Option<String>> {
                tokio::time::sleep(Duration::from_secs(999)).await;
                Ok(Some("should not reach".to_string()))
            }
        }

        let tool = DelegateTaskTool::new(
            Arc::new(SlowRunner),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig {
                timeout_secs: 1,
                max_subagents: 3,
                max_iterations: 10,
            },
        );

        let result = tool
            .execute(json!({"task": "slow task"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn test_delegate_task_no_response() {
        struct NoResponseRunner;

        #[async_trait]
        impl SubagentRunner for NoResponseRunner {
            async fn run_child(
                &self,
                _registry: Arc<ToolRegistry>,
                _system_prompt: String,
                _max_iterations: usize,
            ) -> anyhow::Result<Option<String>> {
                Ok(None)
            }
        }

        let tool = DelegateTaskTool::new(
            Arc::new(NoResponseRunner),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig::default(),
        );

        let result = tool
            .execute(json!({"task": "silent task"}))
            .await
            .unwrap();
        assert_eq!(result, "(no response)");
    }

    #[test]
    fn test_delegate_task_in_full_registry() {
        // Integration test: verify delegate_task appears in a fully-populated registry
        let mut registry = ToolRegistry::new();
        registry.register_defaults();

        let semaphore = Arc::new(Semaphore::new(3));
        let config = SubagentConfig::default();
        let runner: Arc<dyn SubagentRunner> = Arc::new(MockRunner);

        registry.register_delegate_task_tool(runner, semaphore, None, config);

        let tools = registry.list_tools();
        assert!(
            tools.contains(&"delegate_task"),
            "delegate_task must be registered in full registry"
        );
        assert!(
            tools.contains(&"terminal"),
            "default tools must still be present"
        );
        assert!(
            tools.contains(&"read_file"),
            "default tools must still be present"
        );
    }

    #[test]
    fn test_no_recursive_delegation() {
        // AGENT-05 end-to-end: when delegate_task is in the parent registry,
        // the child registry built by build_child_registry never contains it.
        let mut parent_registry = ToolRegistry::new();
        parent_registry.register_defaults();
        let runner: Arc<dyn SubagentRunner> = Arc::new(MockRunner);
        parent_registry.register_delegate_task_tool(
            runner,
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig::default(),
        );
        // Parent has delegate_task
        assert!(parent_registry.list_tools().contains(&"delegate_task"));

        // Child built from DEFAULT_SAFE_TOOLS must NOT have delegate_task
        let child_tools: Vec<String> = DEFAULT_SAFE_TOOLS.iter().map(|s| s.to_string()).collect();
        let child_registry =
            build_child_registry(&child_tools, None, Path::new("/tmp")).unwrap();
        assert!(
            !child_registry.list_tools().contains(&"delegate_task"),
            "AGENT-05: child registry must never contain delegate_task"
        );

        // Even if explicitly requested, delegate_task is stripped
        let with_delegate: Vec<String> = vec![
            "read_file".to_string(),
            "delegate_task".to_string(),
        ];
        let child_registry2 =
            build_child_registry(&with_delegate, None, Path::new("/tmp")).unwrap();
        assert!(
            !child_registry2.list_tools().contains(&"delegate_task"),
            "AGENT-05: delegate_task must be stripped even when explicitly requested"
        );
    }

    // -----------------------------------------------------------------------
    // Toolset resolution tests (D-01)
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_toolset_terminal() {
        let tools = resolve_toolset_tools("terminal").unwrap();
        assert_eq!(tools, vec!["terminal"]);
    }

    #[test]
    fn test_resolve_toolset_file() {
        let tools = resolve_toolset_tools("file").unwrap();
        assert_eq!(tools, vec!["read_file", "write_file", "patch", "search_files"]);
    }

    #[test]
    fn test_resolve_toolset_web() {
        let tools = resolve_toolset_tools("web").unwrap();
        assert_eq!(tools, vec!["web_search", "web_read"]);
    }

    #[test]
    fn test_resolve_toolset_unknown_errors() {
        let result = resolve_toolset_tools("unknown");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown toolset group"));
    }

    #[test]
    fn test_resolve_toolsets_union() {
        let tools = resolve_toolsets(&["terminal".into(), "file".into()]).unwrap();
        assert!(tools.contains(&"terminal".to_string()));
        assert!(tools.contains(&"read_file".to_string()));
        assert!(tools.contains(&"write_file".to_string()));
        assert!(tools.contains(&"patch".to_string()));
        assert!(tools.contains(&"search_files".to_string()));
        assert_eq!(tools.len(), 5, "should be deduplicated union");
    }

    #[test]
    fn test_resolve_toolsets_deduplicates() {
        // Requesting "file" twice should not produce duplicates
        let tools = resolve_toolsets(&["file".into(), "file".into()]).unwrap();
        assert_eq!(tools.len(), 4, "should not have duplicate tools");
    }

    // -----------------------------------------------------------------------
    // Schema tests for toolsets and model override
    // -----------------------------------------------------------------------

    #[test]
    fn test_delegate_task_schema_has_toolsets() {
        let tool = make_delegate_tool();
        let schema = tool.schema();
        let props = &schema.function.parameters["properties"];
        assert!(props.get("toolsets").is_some(), "schema should have toolsets");
        assert_eq!(
            props["toolsets"]["type"].as_str(),
            Some("array"),
            "toolsets should be an array"
        );
    }

    // -----------------------------------------------------------------------
    // Structured summary prompt test (D-10)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_delegate_task_structured_summary_prompt() {
        struct PromptCapture;

        #[async_trait]
        impl SubagentRunner for PromptCapture {
            async fn run_child(
                &self,
                _registry: Arc<ToolRegistry>,
                system_prompt: String,
                _max_iterations: usize,
                _model_override: Option<&str>,
            ) -> anyhow::Result<Option<String>> {
                // Return the system prompt so we can inspect it
                Ok(Some(system_prompt))
            }
        }

        let tool = DelegateTaskTool::new(
            Arc::new(PromptCapture),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig::default(),
        );

        let result = tool.execute(json!({"task": "test task"})).await.unwrap();
        assert!(result.contains("Actions Taken"), "prompt should contain Actions Taken");
        assert!(result.contains("Files Modified"), "prompt should contain Files Modified");
        assert!(result.contains("Findings"), "prompt should contain Findings");
        assert!(result.contains("Issues Encountered"), "prompt should contain Issues Encountered");
    }

    // -----------------------------------------------------------------------
    // Model override test (D-23)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_delegate_task_passes_model_override() {
        struct ModelCapture;

        #[async_trait]
        impl SubagentRunner for ModelCapture {
            async fn run_child(
                &self,
                _registry: Arc<ToolRegistry>,
                _system_prompt: String,
                _max_iterations: usize,
                model_override: Option<&str>,
            ) -> anyhow::Result<Option<String>> {
                Ok(Some(format!("model={}", model_override.unwrap_or("none"))))
            }
        }

        let tool = DelegateTaskTool::new(
            Arc::new(ModelCapture),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig {
                model: Some("google/gemini-flash".to_string()),
                ..SubagentConfig::default()
            },
        );

        let result = tool.execute(json!({"task": "test"})).await.unwrap();
        assert_eq!(result, "model=google/gemini-flash");
    }

    // -----------------------------------------------------------------------
    // Toolsets param execution test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_delegate_task_execute_with_toolsets() {
        let tool = make_delegate_tool();
        let result = tool
            .execute(json!({
                "task": "Do file work",
                "toolsets": ["file"]
            }))
            .await
            .unwrap();
        assert_eq!(result, "mock child response");
    }

    #[tokio::test]
    async fn test_delegate_task_execute_toolsets_precedence() {
        // When toolsets is provided, it should be used (not allowed_tools or defaults)
        // This test verifies it doesn't error — the toolset resolves to valid tools
        let tool = make_delegate_tool();
        let result = tool
            .execute(json!({
                "task": "Do work",
                "toolsets": ["terminal"],
                "allowed_tools": ["bogus_tool"]  // would error if used
            }))
            .await;
        assert!(result.is_ok(), "toolsets should take precedence over allowed_tools");
    }

    #[test]
    fn test_default_safe_tools_contents() {
        assert!(DEFAULT_SAFE_TOOLS.contains(&"read_file"));
        assert!(DEFAULT_SAFE_TOOLS.contains(&"write_file"));
        assert!(DEFAULT_SAFE_TOOLS.contains(&"patch"));
        assert!(DEFAULT_SAFE_TOOLS.contains(&"search_files"));
        assert!(DEFAULT_SAFE_TOOLS.contains(&"web_search"));
        assert!(DEFAULT_SAFE_TOOLS.contains(&"web_read"));
        assert!(DEFAULT_SAFE_TOOLS.contains(&"memory"));
        // Must NOT contain dangerous tools
        assert!(!DEFAULT_SAFE_TOOLS.contains(&"delegate_task"));
        assert!(!DEFAULT_SAFE_TOOLS.contains(&"skills"));
        assert!(!DEFAULT_SAFE_TOOLS.contains(&"execute_code"));
        assert!(!DEFAULT_SAFE_TOOLS.contains(&"terminal"));
    }
}
