//! `delegate_task` tool — spawns a child agent with isolated context and restricted toolset.
//!
//! AGENT-01: Child agents get a filtered subset of parent tools
//! AGENT-02: Default safe tool list when no allowlist is specified
//! AGENT-04: Child terminal runs in isolated temp directory
//! AGENT-05: delegate_task is structurally excluded from child registry (no recursion)

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::{SubagentConfig, ToolSchema};
use serde_json::json;
use tokio::sync::Semaphore;
use tracing::info;

use tokio_util::sync::CancellationToken;

use tokio::sync::RwLock;

use crate::memory_tool::SharedMemoryManager;
use crate::registry::{Tool, ToolRegistry};

/// Progress events emitted by subagent execution (D-19).
#[derive(Debug, Clone)]
pub enum SubagentProgress {
    /// Child agent started working on a task.
    Started { task_summary: String },
    /// Child agent is executing a tool call.
    ToolCall { tool_name: String },
    /// Child agent completed its task.
    Completed,
}

/// Callback for subagent progress events.
/// Parameters: (subagent_index: usize, event: SubagentProgress)
pub type SubagentProgressCallback = Arc<dyn Fn(usize, SubagentProgress) + Send + Sync>;

/// Callback for per-tool-call progress in child agents (D-19).
/// Parameters: (tool_name, args_preview)
pub type ChildToolProgressCallback = Box<dyn Fn(&str, &str) + Send + Sync + 'static>;

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

/// Structured summary instructions appended to child system prompts (D-10).
const STRUCTURED_SUMMARY_INSTRUCTIONS: &str =
    "\n\nWhen you complete the task, provide a structured summary with these sections:\n\
     - **Actions Taken**: What you did step by step\n\
     - **Files Modified**: Any files created or changed (with paths)\n\
     - **Findings**: Key results or information discovered\n\
     - **Issues Encountered**: Any problems or blockers (or 'None')";

/// Maps toolset group names to individual tool names (D-01).
pub fn resolve_toolset_tools(toolset: &str) -> anyhow::Result<Vec<&'static str>> {
    match toolset {
        "terminal" => Ok(vec!["terminal"]),
        "file" => Ok(vec!["read_file", "write_file", "patch", "search_files"]),
        "web" => Ok(vec!["web_search", "web_read"]),
        other => anyhow::bail!(
            "Unknown toolset group: {}. Valid groups: terminal, file, web",
            other
        ),
    }
}

/// Resolves a list of toolset group names to a deduplicated list of individual tool names.
pub fn resolve_toolsets(toolsets: &[String]) -> anyhow::Result<Vec<String>> {
    let mut tools: Vec<String> = Vec::new();
    for ts in toolsets {
        let resolved = resolve_toolset_tools(ts)?;
        for t in resolved {
            if !tools.contains(&t.to_string()) {
                tools.push(t.to_string());
            }
        }
    }
    Ok(tools)
}

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
    ///
    /// `model_override` allows routing the child to a different LLM model (D-23).
    /// When None, the child uses the parent's model.
    ///
    /// `cancel_token` enables cooperative cancellation (D-21). When the token
    /// is cancelled, the child agent loop should return early.
    ///
    /// `tool_progress` enables per-tool-call progress callbacks (D-19).
    /// The callback receives (tool_name, args_preview) for each tool execution.
    async fn run_child(
        &self,
        registry: Arc<RwLock<ToolRegistry>>,
        system_prompt: String,
        max_iterations: usize,
        model_override: Option<&str>,
        cancel_token: Option<CancellationToken>,
        tool_progress: Option<ChildToolProgressCallback>,
    ) -> anyhow::Result<Option<String>>;
}

/// Tool that delegates a focused task to a child agent with restricted tools.
pub struct DelegateTaskTool {
    runner: Arc<dyn SubagentRunner>,
    semaphore: Arc<Semaphore>,
    memory_manager: Option<SharedMemoryManager>,
    config: SubagentConfig,
    /// Parent's cancellation token for propagating interrupt to children (D-21).
    parent_cancel_token: Option<CancellationToken>,
    /// Optional progress callback for subagent events (D-19).
    progress_callback: Option<SubagentProgressCallback>,
}

impl DelegateTaskTool {
    pub fn new(
        runner: Arc<dyn SubagentRunner>,
        semaphore: Arc<Semaphore>,
        memory_manager: Option<SharedMemoryManager>,
        config: SubagentConfig,
        parent_cancel_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            runner,
            semaphore,
            memory_manager,
            config,
            parent_cancel_token,
            progress_callback: None,
        }
    }

    /// Set a progress callback for subagent events (D-19).
    pub fn with_progress_callback(mut self, cb: SubagentProgressCallback) -> Self {
        self.progress_callback = Some(cb);
        self
    }
}

impl DelegateTaskTool {
    /// Batch execution mode: spawn parallel child agents for multiple tasks (D-05).
    ///
    /// Tasks are truncated to `config.max_subagents` (D-06), spawned as tokio tasks
    /// sharing the global semaphore, and results are sorted by original index (D-07).
    async fn execute_batch(&self, tasks_val: &serde_json::Value, detach: bool) -> anyhow::Result<String> {
        let tasks_array = tasks_val
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("tasks must be an array"))?;

        // D-06: truncate to max_subagents (default 3)
        let max_batch = self.config.max_subagents;
        let tasks: Vec<_> = tasks_array.iter().take(max_batch).collect();
        if tasks_array.len() > max_batch {
            tracing::warn!(
                "Batch truncated from {} to {} tasks (max_subagents limit)",
                tasks_array.len(),
                max_batch
            );
        }

        // Spawn tokio tasks for parallel execution
        let mut handles = Vec::new();
        for (index, task_obj) in tasks.iter().enumerate() {
            let goal = task_obj["goal"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("tasks[{}].goal must be a string", index))?
                .to_string();
            let context = task_obj
                .get("context")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Resolve toolsets: per-task override or config default (D-01)
            let toolsets: Vec<String> = if let Some(ts) = task_obj.get("toolsets") {
                let arr = ts
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("tasks[{}].toolsets must be an array", index))?;
                arr.iter()
                    .map(|v| v.as_str().unwrap_or("").to_string())
                    .collect()
            } else {
                self.config.default_toolsets.clone()
            };
            let allowed_tools = resolve_toolsets(&toolsets)?;

            let runner = self.runner.clone();
            let semaphore = self.semaphore.clone();
            let memory_manager = self.memory_manager.clone();
            let config = self.config.clone();

            // D-21/D-22: Create child cancel token based on detach flag
            let child_cancel_token = if detach {
                None
            } else if let Some(ref parent_token) = self.parent_cancel_token {
                Some(parent_token.child_token())
            } else {
                None
            };

            // D-19: Create per-child tool progress callback
            let progress_cb = self.progress_callback.clone();
            let child_tool_progress: Option<ChildToolProgressCallback> =
                progress_cb.as_ref().map(|cb| {
                    let cb = cb.clone();
                    Box::new(move |tool_name: &str, _args: &str| {
                        cb(index, SubagentProgress::ToolCall { tool_name: tool_name.to_string() });
                    }) as ChildToolProgressCallback
                });

            let handle = tokio::spawn(async move {
                // D-19: Emit Started progress event
                if let Some(ref cb) = progress_cb {
                    let summary = if goal.len() > 50 {
                        let mut end = 50;
                        while !goal.is_char_boundary(end) {
                            end -= 1;
                        }
                        &goal[..end]
                    } else {
                        &goal
                    };
                    cb(index, SubagentProgress::Started { task_summary: summary.to_string() });
                }

                // Each task gets its own temp dir for isolation
                let child_dir = tempfile::TempDir::new()?;

                // Acquire semaphore (shared across all batch tasks + single tasks)
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|e| anyhow::anyhow!("Semaphore closed: {}", e))?;

                let child_registry =
                    build_child_registry(&allowed_tools, memory_manager, child_dir.path())?;

                let mut system_prompt = format!(
                    "You are a focused assistant. Complete the following task:\n\n{}",
                    goal
                );
                if !context.is_empty() {
                    system_prompt.push_str(&format!("\n\nContext:\n{}", context));
                }
                // Append structured summary instructions (D-10)
                system_prompt.push_str(STRUCTURED_SUMMARY_INSTRUCTIONS);

                let result = tokio::time::timeout(
                    Duration::from_secs(config.timeout_secs),
                    runner.run_child(
                        Arc::new(RwLock::new(child_registry)),
                        system_prompt,
                        config.max_iterations,
                        config.model.as_deref(),
                        child_cancel_token,
                        child_tool_progress,
                    ),
                )
                .await;

                let response = match result {
                    Ok(Ok(resp)) => resp.unwrap_or_else(|| "(no response)".to_string()),
                    Ok(Err(e)) => format!("Error: {}", e),
                    Err(_) => format!("Subagent timed out after {}s", config.timeout_secs),
                };

                // D-19: Emit Completed progress event
                if let Some(ref cb) = progress_cb {
                    cb(index, SubagentProgress::Completed);
                }

                Ok::<(usize, String), anyhow::Error>((index, response))
            });

            handles.push(handle);
        }

        // Collect results and sort by index (D-07)
        let mut results: Vec<(usize, String)> = Vec::new();
        for (expected_idx, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok((idx, response))) => results.push((idx, response)),
                Ok(Err(e)) => results.push((expected_idx, format!("Error: {}", e))),
                Err(e) => results.push((expected_idx, format!("Task panicked: {}", e))),
            }
        }
        results.sort_by_key(|(idx, _)| *idx);

        // Format batch results as numbered sections
        let output = results
            .iter()
            .map(|(idx, resp)| format!("## Task {} Result\n\n{}", idx + 1, resp))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        Ok(output)
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
    memory_manager: Option<SharedMemoryManager>,
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
                if let Some(ref mgr) = memory_manager {
                    registry.register(Box::new(
                        crate::memory_tool::MemoryTool::new_read_only(mgr.clone()),
                    ));
                } else {
                    tracing::warn!("memory tool requested but no MemoryManager available; skipping");
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
                    },
                    "toolsets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Toolset groups for the child agent: 'terminal', 'file', 'web'. Takes precedence over allowed_tools."
                    },
                    "tasks": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "goal": { "type": "string", "description": "Task description for the child agent." },
                                "context": { "type": "string", "description": "Additional context for the child agent." },
                                "toolsets": { "type": "array", "items": { "type": "string" }, "description": "Toolset groups for this task." }
                            },
                            "required": ["goal"]
                        },
                        "description": "Array of tasks for parallel batch execution. Max 3 tasks. Mutually exclusive with 'task' param."
                    },
                    "detach": {
                        "type": "boolean",
                        "description": "If true, child survives parent interrupt. Default: false.",
                        "default": false
                    }
                },
                "required": []
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        // Validate: either "task" or "tasks" must be present (mutually exclusive modes)
        if args.get("tasks").is_none() && args.get("task").is_none() {
            anyhow::bail!("Either 'task' (single mode) or 'tasks' (batch mode) is required");
        }

        // Detect mode: batch (tasks array) vs single (task string)
        if let Some(tasks_val) = args.get("tasks") {
            let detach = args.get("detach").and_then(|v| v.as_bool()).unwrap_or(false);
            return self.execute_batch(tasks_val, detach).await;
        }

        let task = args["task"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: task"))?;

        // Resolve tools: toolsets > allowed_tools > config.default_toolsets (D-01)
        let allowed_tools: Vec<String> = if let Some(toolsets) = args.get("toolsets") {
            let ts: Vec<String> = toolsets
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("toolsets must be an array"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| anyhow::anyhow!("toolsets entries must be strings"))
                        .map(|s| s.to_string())
                })
                .collect::<anyhow::Result<Vec<String>>>()?;
            resolve_toolsets(&ts)?
        } else if let Some(tools) = args.get("allowed_tools") {
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
            // D-01 default: resolve from config.default_toolsets
            resolve_toolsets(&self.config.default_toolsets)?
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
        // Phase 21.7 Plan 08 (D-09 / E-11 / T-21.7-08-05): 2s debounced
        // semaphore-wait warn. Fires once per blocked task that took more
        // than 2 seconds to acquire — the operator sees it via
        // `RUST_LOG=warn` (which is the default level the CLI promotes
        // to stderr). Target + message are locked by the grep gate in
        // `tests/delegate_task_semaphore_warn.rs` — if you refactor,
        // keep both literals verbatim.
        if wait_duration >= std::time::Duration::from_secs(2) {
            tracing::warn!(
                target: "ironhermes_tools::delegate_task",
                elapsed_ms = wait_duration.as_millis() as u64,
                "semaphore wait exceeded 2s threshold"
            );
        }

        // Build child registry (D-01..D-05)
        let child_registry = build_child_registry(
            &allowed_tools,
            self.memory_manager.clone(),
            child_dir.path(),
        )?;

        // Build system prompt with structured summary instructions (D-10)
        let system_prompt = format!(
            "You are a focused assistant. Complete the following task:\n\n{}{}",
            task, STRUCTURED_SUMMARY_INSTRUCTIONS
        );

        // D-21/D-22: Create child cancel token based on detach flag
        let detach = args.get("detach").and_then(|v| v.as_bool()).unwrap_or(false);
        let child_cancel_token = if detach {
            // D-22: independent token — parent interrupt does not cancel this child
            None
        } else if let Some(ref parent_token) = self.parent_cancel_token {
            // D-21: child token derived from parent — cancelling parent cascades
            Some(parent_token.child_token())
        } else {
            None
        };

        // D-19: Emit Started progress event
        if let Some(ref cb) = self.progress_callback {
            let summary = if task.len() > 50 {
                let mut end = 50;
                while !task.is_char_boundary(end) {
                    end -= 1;
                }
                &task[..end]
            } else {
                task
            };
            cb(0, SubagentProgress::Started { task_summary: summary.to_string() });
        }

        // D-19: Create per-child tool progress callback that routes through progress_callback
        let child_tool_progress: Option<ChildToolProgressCallback> =
            self.progress_callback.as_ref().map(|cb| {
                let cb = cb.clone();
                Box::new(move |tool_name: &str, _args: &str| {
                    cb(0, SubagentProgress::ToolCall { tool_name: tool_name.to_string() });
                }) as ChildToolProgressCallback
            });

        // Run child agent with timeout (D-08) and model override (D-23)
        let result = tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs),
            self.runner.run_child(
                Arc::new(RwLock::new(child_registry)),
                system_prompt,
                self.config.max_iterations,
                self.config.model.as_deref(),
                child_cancel_token,
                child_tool_progress,
            ),
        )
        .await;

        // D-19: Emit Completed progress event
        if let Some(ref cb) = self.progress_callback {
            cb(0, SubagentProgress::Completed);
        }

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
        use crate::memory_manager_handle::MemoryManagerHandle;
        use ironhermes_core::memory_store::MemoryResult;

        // Minimal mock handle — we only verify the tool is registered, not
        // that writes go anywhere.
        struct NoopManager;
        #[async_trait]
        impl MemoryManagerHandle for NoopManager {
            async fn handle_tool_call(
                &self,
                _name: &str,
                _args: serde_json::Value,
            ) -> MemoryResult {
                Ok("{}".to_string())
            }
        }

        let mgr: SharedMemoryManager =
            Arc::new(tokio::sync::Mutex::new(NoopManager));

        let registry = build_child_registry(
            &["memory".to_string()],
            Some(mgr),
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
            _registry: Arc<RwLock<ToolRegistry>>,
            _system_prompt: String,
            _max_iterations: usize,
            _model_override: Option<&str>,
            _cancel_token: Option<CancellationToken>,
            _tool_progress: Option<ChildToolProgressCallback>,
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
            None,
        )
    }

    #[test]
    fn test_delegate_task_name() {
        let tool = make_delegate_tool();
        assert_eq!(tool.name(), "delegate_task");
    }

    #[test]
    fn test_delegate_task_schema_task_and_tasks_are_mutually_exclusive_optional() {
        // Per WR-01 (commit bbf48db): `task` and `tasks` are mutually exclusive,
        // so neither appears in `required`. Runtime validation in `execute()` enforces
        // that at least one is present.
        let tool = make_delegate_tool();
        let schema = tool.schema();
        let params = &schema.function.parameters;
        let required = params["required"].as_array().unwrap();
        assert!(
            required.is_empty(),
            "schema 'required' must be empty — mutual exclusivity is enforced at runtime"
        );
        let props = &params["properties"];
        assert!(props.get("task").is_some(), "schema should expose 'task' property");
        assert!(props.get("tasks").is_some(), "schema should expose 'tasks' property");
    }

    #[tokio::test]
    async fn test_delegate_task_execute_rejects_when_neither_task_nor_tasks_provided() {
        let tool = make_delegate_tool();
        let err = tool.execute(json!({})).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("task") && msg.contains("tasks"),
            "error should mention both 'task' and 'tasks'; got: {msg}"
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
                _registry: Arc<RwLock<ToolRegistry>>,
                _system_prompt: String,
                _max_iterations: usize,
                _model_override: Option<&str>,
                _cancel_token: Option<CancellationToken>,
                _tool_progress: Option<ChildToolProgressCallback>,
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
                ..SubagentConfig::default()
            },
            None,
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
                _registry: Arc<RwLock<ToolRegistry>>,
                _system_prompt: String,
                _max_iterations: usize,
                _model_override: Option<&str>,
                _cancel_token: Option<CancellationToken>,
                _tool_progress: Option<ChildToolProgressCallback>,
            ) -> anyhow::Result<Option<String>> {
                Ok(None)
            }
        }

        let tool = DelegateTaskTool::new(
            Arc::new(NoResponseRunner),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig::default(),
            None,
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

        registry.register_delegate_task_tool(runner, semaphore, None, config, None, None);

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
            None,
            None,
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
                _registry: Arc<RwLock<ToolRegistry>>,
                system_prompt: String,
                _max_iterations: usize,
                _model_override: Option<&str>,
                _cancel_token: Option<CancellationToken>,
                _tool_progress: Option<ChildToolProgressCallback>,
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
            None,
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
                _registry: Arc<RwLock<ToolRegistry>>,
                _system_prompt: String,
                _max_iterations: usize,
                model_override: Option<&str>,
                _cancel_token: Option<CancellationToken>,
                _tool_progress: Option<ChildToolProgressCallback>,
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
            None,
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

    // -----------------------------------------------------------------------
    // Batch mode tests (D-05, D-06, D-07)
    // -----------------------------------------------------------------------

    /// A runner that records the index of completion via a small sleep offset,
    /// so we can verify result ordering regardless of completion order.
    struct OrderedMockRunner;

    #[async_trait]
    impl SubagentRunner for OrderedMockRunner {
        async fn run_child(
            &self,
            _registry: Arc<RwLock<ToolRegistry>>,
            system_prompt: String,
            _max_iterations: usize,
            _model_override: Option<&str>,
            _cancel_token: Option<CancellationToken>,
            _tool_progress: Option<ChildToolProgressCallback>,
        ) -> anyhow::Result<Option<String>> {
            // Extract the goal from the prompt to echo it back
            let goal = system_prompt
                .lines()
                .nth(2) // "You are a focused assistant..." line 0, blank line 1, goal line 2
                .unwrap_or("unknown")
                .to_string();
            Ok(Some(format!("Result: {}", goal)))
        }
    }

    fn make_batch_tool() -> DelegateTaskTool {
        DelegateTaskTool::new(
            Arc::new(OrderedMockRunner),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig::default(),
            None,
        )
    }

    #[tokio::test]
    async fn test_batch_basic_two_tasks() {
        let tool = make_batch_tool();
        let result = tool
            .execute(json!({
                "task": "ignored in batch mode",
                "tasks": [
                    { "goal": "Task A" },
                    { "goal": "Task B" }
                ]
            }))
            .await
            .unwrap();
        assert!(result.contains("Task 1 Result"), "should have Task 1 header");
        assert!(result.contains("Task 2 Result"), "should have Task 2 header");
        assert!(result.contains("Task A"), "should contain goal A result");
        assert!(result.contains("Task B"), "should contain goal B result");
    }

    #[tokio::test]
    async fn test_batch_truncates_to_max_subagents() {
        // D-06: max_subagents=2, so only first 2 tasks should run
        let tool = DelegateTaskTool::new(
            Arc::new(OrderedMockRunner),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig {
                max_subagents: 2,
                ..SubagentConfig::default()
            },
            None,
        );
        let result = tool
            .execute(json!({
                "task": "ignored",
                "tasks": [
                    { "goal": "A" },
                    { "goal": "B" },
                    { "goal": "C" },
                    { "goal": "D" }
                ]
            }))
            .await
            .unwrap();
        // Should have exactly 2 task results
        let task_count = result.matches("## Task").count();
        assert_eq!(task_count, 2, "should truncate to 2 tasks (max_subagents)");
    }

    #[tokio::test]
    async fn test_batch_result_ordering() {
        // D-07: results should be sorted by original index
        // Use a runner that delays based on reverse order to test ordering
        struct ReverseOrderRunner;

        #[async_trait]
        impl SubagentRunner for ReverseOrderRunner {
            async fn run_child(
                &self,
                _registry: Arc<RwLock<ToolRegistry>>,
                system_prompt: String,
                _max_iterations: usize,
                _model_override: Option<&str>,
                _cancel_token: Option<CancellationToken>,
                _tool_progress: Option<ChildToolProgressCallback>,
            ) -> anyhow::Result<Option<String>> {
                // Task with "slow" sleeps longer — it has a higher index but should
                // still appear in correct position in output
                if system_prompt.contains("slow") {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let goal = system_prompt.lines().nth(2).unwrap_or("?");
                Ok(Some(format!("Done: {}", goal)))
            }
        }

        let tool = DelegateTaskTool::new(
            Arc::new(ReverseOrderRunner),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig::default(),
            None,
        );

        let result = tool
            .execute(json!({
                "task": "ignored",
                "tasks": [
                    { "goal": "fast-first" },
                    { "goal": "slow-second" }
                ]
            }))
            .await
            .unwrap();

        // Task 1 should appear before Task 2
        let pos1 = result.find("Task 1").unwrap();
        let pos2 = result.find("Task 2").unwrap();
        assert!(pos1 < pos2, "Task 1 should appear before Task 2 regardless of completion order");
    }

    #[tokio::test]
    async fn test_batch_single_task_still_works() {
        // D-08: backward compatibility — single "task" param without "tasks"
        let tool = make_batch_tool();
        let result = tool
            .execute(json!({
                "task": "single task only"
            }))
            .await
            .unwrap();
        // Should use single-task mode, not batch
        assert!(!result.contains("## Task"), "single mode should not have batch headers");
    }

    #[tokio::test]
    async fn test_batch_semaphore_sharing() {
        // D-06: batch tasks share the global semaphore
        // With semaphore(2) and 3 tasks, only 2 can run at once
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingRunner {
            concurrent: Arc<AtomicUsize>,
            max_concurrent: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl SubagentRunner for CountingRunner {
            async fn run_child(
                &self,
                _registry: Arc<RwLock<ToolRegistry>>,
                _system_prompt: String,
                _max_iterations: usize,
                _model_override: Option<&str>,
                _cancel_token: Option<CancellationToken>,
                _tool_progress: Option<ChildToolProgressCallback>,
            ) -> anyhow::Result<Option<String>> {
                let current = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                // Update max if this is the highest seen
                self.max_concurrent.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                self.concurrent.fetch_sub(1, Ordering::SeqCst);
                Ok(Some("done".to_string()))
            }
        }

        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let tool = DelegateTaskTool::new(
            Arc::new(CountingRunner {
                concurrent: concurrent.clone(),
                max_concurrent: max_concurrent.clone(),
            }),
            Arc::new(Semaphore::new(2)), // Only 2 slots
            None,
            SubagentConfig::default(),
            None,
        );

        let _result = tool
            .execute(json!({
                "task": "ignored",
                "tasks": [
                    { "goal": "A" },
                    { "goal": "B" },
                    { "goal": "C" }
                ]
            }))
            .await
            .unwrap();

        assert!(
            max_concurrent.load(Ordering::SeqCst) <= 2,
            "max concurrent should be <= 2 (semaphore limit)"
        );
    }

    #[tokio::test]
    async fn test_batch_per_task_toolsets() {
        // Per-task toolsets should override config defaults
        let tool = make_batch_tool();
        let result = tool
            .execute(json!({
                "task": "ignored",
                "tasks": [
                    { "goal": "use terminal", "toolsets": ["terminal"] },
                    { "goal": "use web", "toolsets": ["web"] }
                ]
            }))
            .await
            .unwrap();
        assert!(result.contains("Task 1 Result"), "should complete with per-task toolsets");
        assert!(result.contains("Task 2 Result"), "should complete with per-task toolsets");
    }

    #[tokio::test]
    async fn test_batch_default_toolsets_when_not_specified() {
        // When per-task toolsets not provided, should use config.default_toolsets
        let tool = make_batch_tool();
        let result = tool
            .execute(json!({
                "task": "ignored",
                "tasks": [
                    { "goal": "no toolsets specified" }
                ]
            }))
            .await
            .unwrap();
        assert!(result.contains("Task 1 Result"), "should complete with default toolsets");
    }

    #[tokio::test]
    async fn test_batch_with_context() {
        struct ContextCapture;

        #[async_trait]
        impl SubagentRunner for ContextCapture {
            async fn run_child(
                &self,
                _registry: Arc<RwLock<ToolRegistry>>,
                system_prompt: String,
                _max_iterations: usize,
                _model_override: Option<&str>,
                _cancel_token: Option<CancellationToken>,
                _tool_progress: Option<ChildToolProgressCallback>,
            ) -> anyhow::Result<Option<String>> {
                Ok(Some(system_prompt))
            }
        }

        let tool = DelegateTaskTool::new(
            Arc::new(ContextCapture),
            Arc::new(Semaphore::new(3)),
            None,
            SubagentConfig::default(),
            None,
        );

        let result = tool
            .execute(json!({
                "task": "ignored",
                "tasks": [
                    { "goal": "my goal", "context": "extra context here" }
                ]
            }))
            .await
            .unwrap();
        assert!(result.contains("my goal"), "should contain the goal");
        assert!(result.contains("extra context here"), "should contain the context");
    }

    #[test]
    fn test_delegate_task_schema_has_tasks() {
        let tool = make_delegate_tool();
        let schema = tool.schema();
        let props = &schema.function.parameters["properties"];
        assert!(props.get("tasks").is_some(), "schema should have tasks property");
        assert_eq!(
            props["tasks"]["type"].as_str(),
            Some("array"),
            "tasks should be an array"
        );
    }
}
