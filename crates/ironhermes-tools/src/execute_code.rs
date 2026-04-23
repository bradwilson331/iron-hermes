//! `execute_code` tool — executes Python scripts in an isolated sandbox
//! with RPC access to a safe subset of agent tools.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::{ExecConfig, SkillRecord, ToolSchema};
use ironhermes_exec::process_registry::{ProcessRegistry, SpawnSpec};
use ironhermes_exec::{CancellationToken, Sandbox, SandboxConfig, ToolDispatch};
use serde_json::json;
use tokio::sync::RwLock;
use tracing::debug;

use crate::registry::{Tool, ToolRegistry};
use crate::skills_tool::active_skill_env_names;

// =============================================================================
// RegistryDispatch — adapter bridging ToolRegistry to ToolDispatch trait
// =============================================================================

/// Adapter that implements `ironhermes_exec::ToolDispatch` by wrapping an
/// `Arc<ToolRegistry>`. Lives in ironhermes-tools (not ironhermes-exec) to
/// avoid circular crate dependencies.
pub(crate) struct RegistryDispatch {
    registry: Arc<ToolRegistry>,
}

#[async_trait]
impl ToolDispatch for RegistryDispatch {
    async fn dispatch(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        self.registry.dispatch(tool_name, args).await
    }
}

// =============================================================================
// ExecuteCodeTool
// =============================================================================

/// Agent tool that executes Python scripts in an isolated sandbox.
///
/// The sandbox has access to a safe subset of agent tools (D-07) via
/// JSON-RPC over a Unix domain socket. The RPC dispatch registry is
/// separate from the main registry and intentionally excludes `terminal`
/// and `execute_code` (defense-in-depth against recursion).
pub struct ExecuteCodeTool {
    /// Registry containing only D-07 safe tools for RPC dispatch.
    rpc_registry: Arc<ToolRegistry>,
    /// Execution configuration (python path, timeout, limits).
    config: ExecConfig,
    /// Cancellation token — when triggered (e.g., by user sending a new message),
    /// the running sandbox child process is killed immediately.
    /// Integration: The gateway/handler creates a CancellationToken per tool execution
    /// and triggers it when a new message arrives from the same user.
    /// TODO: Wire CancellationToken creation in gateway handler (Phase 8 follow-up).
    cancel_token: Option<CancellationToken>,
    /// Phase 19 Plan 06 (D-05): shared list of currently-active skills. Used to
    /// compute the skill-declared env-var whitelist passed into the sandbox so
    /// skill-declared keys bypass the secret-strip in `Sandbox::build_env`.
    /// `None` when wired without skill support (tests / legacy construction).
    active_skills: Option<Arc<std::sync::Mutex<Vec<SkillRecord>>>>,
    /// Plan 21.7-06 (D-29): optional `ProcessRegistry` handle for the
    /// `background=true` branch. When set AND background=true, the tool
    /// spawns the Python command line into the registry (same SpawnSpec
    /// shape as terminal) instead of running via the sandbox. Foreground
    /// mode (default) is unchanged.
    process_registry: Option<Arc<RwLock<ProcessRegistry>>>,
}

impl ExecuteCodeTool {
    /// Create a new ExecuteCodeTool.
    ///
    /// `rpc_registry` must contain ONLY D-07 safe tools (no terminal, no execute_code).
    /// `config` provides python path, timeout, call limit, and output cap.
    /// `cancel_token` is optionally provided by the gateway to support user interruption (D-40).
    pub fn new(
        rpc_registry: Arc<ToolRegistry>,
        config: ExecConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            rpc_registry,
            config,
            cancel_token,
            active_skills: None,
            process_registry: None,
        }
    }

    /// Phase 19 Plan 06 (D-05): Create an ExecuteCodeTool with shared access to
    /// the active-skills list so skill-declared env vars bypass the sandbox
    /// secret-strip in `Sandbox::build_env`.
    pub fn with_active_skills(
        rpc_registry: Arc<ToolRegistry>,
        config: ExecConfig,
        cancel_token: Option<CancellationToken>,
        active_skills: Arc<std::sync::Mutex<Vec<SkillRecord>>>,
    ) -> Self {
        Self {
            rpc_registry,
            config,
            cancel_token,
            active_skills: Some(active_skills),
            process_registry: None,
        }
    }

    /// Plan 21.7-06 (D-29): install a shared `ProcessRegistry` handle so
    /// `background=true` calls are tracked + drained on session end.
    /// Foreground (sandbox) dispatch is unchanged regardless of this setter.
    pub fn with_process_registry(mut self, reg: Arc<RwLock<ProcessRegistry>>) -> Self {
        self.process_registry = Some(reg);
        self
    }
}

#[async_trait]
impl Tool for ExecuteCodeTool {
    fn name(&self) -> &str {
        "execute_code"
    }

    fn toolset(&self) -> &str {
        "code"
    }

    fn description(&self) -> &str {
        "Execute a Python script in an isolated sandbox. The script can call agent tools \
         (read_file, web_search, etc.) via 'from hermes_tools import <tool>'. Returns \
         stdout, stderr, and exit code. Set background=true to run as a tracked \
         background process (no sandbox; returns {process_id, pid} immediately)."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "execute_code",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python script source code to execute."
                    },
                    "background": {
                        "type": "boolean",
                        "description": "When true, spawn the script as a tracked background process (no sandbox, no RPC). Returns {process_id, pid} instead of captured output. Drained + killed on session end.",
                        "default": false
                    },
                    "watch_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Regex patterns to match against stdout/stderr lines (background mode only). Matches are fanned out via the process registry watch channel (rate-limited 8/10s).",
                        "default": []
                    }
                },
                "required": ["code"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let code = args["code"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: code"))?;

        // --- Plan 21.7-06 / D-29: background branch. ---
        let background = args
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if background {
            let reg_arc = self.process_registry.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "execute_code: background=true requires a ProcessRegistry to be wired via with_process_registry"
                )
            })?;

            let watch_patterns: Vec<String> = args
                .get("watch_patterns")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            // Persist the script to a tempfile and invoke the configured
            // python interpreter on it. The tempfile is intentionally leaked
            // (into_temp_path + keep) so the background child can still read
            // it after this function returns; the file is small (the script)
            // and fully tied to session lifetime via the process registry
            // drain on session end.
            let tmp = tempfile::Builder::new()
                .prefix("execute_code-bg-")
                .suffix(".py")
                .tempfile()
                .map_err(|e| anyhow::anyhow!("execute_code: failed to create tempfile: {}", e))?;
            std::fs::write(tmp.path(), code)
                .map_err(|e| anyhow::anyhow!("execute_code: failed to write script: {}", e))?;
            let script_path = tmp.into_temp_path().keep().map_err(|e| {
                anyhow::anyhow!("execute_code: failed to persist tempfile: {}", e)
            })?;

            // shell_words-safe: quote the path with single-quotes; paths from
            // tempfile::Builder do not contain single-quotes on unix.
            let command = format!(
                "{} '{}'",
                self.config.python_path,
                script_path.display()
            );

            let spec = SpawnSpec {
                command,
                cwd: None,
                env: vec![],
                watch_patterns,
            };

            let id = {
                let mut r = reg_arc.write().await;
                r.spawn(spec).await?
            };

            if let Err(e) = ProcessRegistry::start_output_drain(reg_arc.clone(), &id).await {
                tracing::warn!(
                    id = %id,
                    error = %e,
                    "execute_code: failed to attach output drain task (process still tracked)"
                );
            }

            let pid_opt = {
                let r = reg_arc.read().await;
                r.poll(&id).await.and_then(|s| s.pid)
            };

            return Ok(json!({
                "background": true,
                "process_id": id,
                "pid": pid_opt,
            })
            .to_string());
        }

        // --- Foreground (sandbox) path — unchanged from pre-21.7-06. ---

        debug!("Executing Python script ({} bytes)", code.len());

        // Build sandbox config from ExecConfig
        let sandbox_config = SandboxConfig {
            python_path: self.config.python_path.clone(),
            timeout_secs: self.config.timeout_secs,
            max_rpc_calls: self.config.max_rpc_calls,
            max_output_bytes: self.config.max_output_bytes,
            max_stderr_bytes: self.config.max_stderr_bytes,
        };

        let sandbox = Sandbox::new(sandbox_config);

        // Create the ToolDispatch adapter wrapping the RPC-safe registry
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(RegistryDispatch {
            registry: Arc::clone(&self.rpc_registry),
        });

        // Phase 19 Plan 06 (D-05): compute skill-declared env whitelist
        let skill_env_whitelist: Vec<String> = match &self.active_skills {
            Some(active) => match active.lock() {
                Ok(guard) => active_skill_env_names(&guard),
                Err(poisoned) => {
                    // Defensive: read past a poisoned lock rather than failing the
                    // whole tool — absent whitelist just means no bypass.
                    active_skill_env_names(&poisoned.into_inner())
                }
            },
            None => Vec::new(),
        };

        let result = sandbox
            .run(code, dispatch, self.cancel_token.clone(), &skill_env_whitelist)
            .await?;

        // D-25..D-27: Structured JSON response format
        let status = if result.interrupted {
            "interrupted"
        } else if result.timed_out {
            "timeout"
        } else if result.exit_code == Some(0) {
            "success"
        } else {
            "error"
        };

        let mut response = json!({
            "status": status,
            "output": result.stdout,
            "exit_code": result.exit_code.unwrap_or(-1),
            "tool_calls_made": result.tool_calls_made,
            "duration_seconds": result.duration_seconds,
        });

        // D-30: include stderr when non-empty (truncation already handled by sandbox)
        if !result.stderr.is_empty() {
            response["stderr"] = serde_json::Value::String(result.stderr);
        }

        Ok(response.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_code_tool_basic() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        let result = tool
            .execute(json!({"code": "print('hello from python')"}))
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result)
            .expect("response should be valid JSON");
        assert_eq!(parsed["status"], "success");
        assert!(parsed["output"].as_str().unwrap().contains("hello from python"));
        assert_eq!(parsed["exit_code"], 0);
        assert!(parsed["duration_seconds"].as_f64().unwrap() >= 0.0);
        assert_eq!(parsed["tool_calls_made"], 0);
    }

    #[tokio::test]
    async fn test_execute_code_tool_with_rpc() {
        // Build registry with file tools for RPC
        let mut rpc_registry = ToolRegistry::new();
        rpc_registry.register_defaults();
        let rpc_registry = Arc::new(rpc_registry);
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        // Write a temp file, then have Python read it via hermes_tools
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "test content from rust").unwrap();
        let script = format!(
            "from hermes_tools import read_file\nresult = read_file('{}')\nprint(result)",
            tmp.path().display()
        );
        let result = tool.execute(json!({"code": script})).await.unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result)
            .expect("response should be valid JSON");
        assert_eq!(parsed["status"], "success");
        assert!(
            parsed["output"].as_str().unwrap().contains("test content from rust"),
            "RPC read_file should return file content, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_execute_code_tool_timeout_format() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let mut config = ExecConfig::default();
        config.timeout_secs = 2;
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tool.execute(json!({"code": "import time; time.sleep(999)"})),
        )
        .await
        .expect("test must complete within 10s")
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result)
            .expect("response should be valid JSON");
        assert_eq!(parsed["status"], "timeout");
        assert!(parsed["stderr"].as_str().unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn test_execute_code_result_format() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        let result = tool
            .execute(json!({"code": "import sys; print('out'); sys.stderr.write('err')"}))
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result)
            .expect("response should be valid JSON");
        assert_eq!(parsed["status"], "success");
        assert!(parsed["output"].as_str().unwrap().contains("out"), "missing stdout content");
        assert_eq!(parsed["stderr"], "err", "missing stderr content");
        assert_eq!(parsed["exit_code"], 0);
        assert!(parsed.get("tool_calls_made").is_some(), "missing tool_calls_made");
        assert!(parsed.get("duration_seconds").is_some(), "missing duration_seconds");
    }

    #[tokio::test]
    async fn test_execute_code_missing_code_param() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        let result = tool.execute(json!({})).await;
        assert!(result.is_err(), "should error on missing code param");
        assert!(
            result.unwrap_err().to_string().contains("code"),
            "error should mention 'code'"
        );
    }

    // --- Plan 21.7-06 / D-29 — background path tests ------------------------

    /// background=true without a registry must error — defensive wiring gate.
    #[tokio::test]
    async fn background_without_registry_errors() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);
        let result = tool
            .execute(json!({"code": "print('x')", "background": true}))
            .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("ProcessRegistry"),
            "error must mention ProcessRegistry: {msg}"
        );
    }

    /// background=true with a registry spawns into the registry and returns
    /// structured JSON with process_id + pid.
    #[tokio::test]
    #[cfg(unix)]
    async fn background_true_spawns_into_registry() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let reg = Arc::new(RwLock::new(ProcessRegistry::new_for_session(
            "t-execute_code-bg",
        )));
        let tool = ExecuteCodeTool::new(rpc_registry, config, None)
            .with_process_registry(reg.clone());

        let resp = tool
            .execute(
                json!({"code": "import time\nprint('hi')\ntime.sleep(30)\n", "background": true}),
            )
            .await
            .expect("background spawn");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("JSON");
        assert_eq!(parsed["background"], true);
        assert!(parsed["process_id"]
            .as_str()
            .unwrap()
            .starts_with("proc_"));
        assert!(parsed["pid"].as_u64().is_some());
        {
            let r = reg.read().await;
            assert_eq!(r.running_count(), 1);
        }
        reg.write().await.drain_and_kill().await.ok();
    }
}
