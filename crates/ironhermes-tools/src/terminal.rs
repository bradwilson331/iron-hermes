use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_exec::process_registry::{ProcessRegistry, SpawnSpec};
use serde_json::json;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::{Duration, timeout};
use tracing::debug;

use crate::registry::Tool;

const MAX_OUTPUT_LEN: usize = 50_000;

/// `terminal` tool — runs a shell command.
///
/// Phase 21.7-06 (D-29): gained an optional `background` argument. When
/// `background=true` AND a `ProcessRegistry` handle has been wired via
/// `with_process_registry`, the command is spawned into the registry and
/// the tool returns a structured `{"process_id": "...", "pid": ...}` JSON
/// immediately. Foreground mode (`background=false` or absent) keeps the
/// original synchronous output-capture path exactly as before.
pub struct TerminalTool {
    cwd: Option<PathBuf>,
    /// Plan 21.7-06: Optional registry handle for background spawns. `None`
    /// leaves background-mode requests erroring out — foreground mode is
    /// always available regardless.
    process_registry: Option<Arc<RwLock<ProcessRegistry>>>,
}

impl TerminalTool {
    pub fn new() -> Self {
        Self {
            cwd: None,
            process_registry: None,
        }
    }

    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            cwd: Some(cwd),
            process_registry: None,
        }
    }

    /// Plan 21.7-06 (D-29): install a shared `ProcessRegistry` handle so
    /// `background=true` calls are tracked + drained on session end.
    /// Foreground dispatch is unchanged regardless of this setter.
    pub fn with_process_registry(mut self, reg: Arc<RwLock<ProcessRegistry>>) -> Self {
        self.process_registry = Some(reg);
        self
    }
}

impl Default for TerminalTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TerminalTool {
    fn name(&self) -> &str {
        "terminal"
    }

    fn toolset(&self) -> &str {
        "code"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output (stdout + stderr combined). \
         Set background=true to spawn a long-running process tracked by the \
         process registry; returns {process_id, pid} immediately."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "terminal",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30). Ignored when background=true.",
                        "default": 30
                    },
                    "background": {
                        "type": "boolean",
                        "description": "When true, spawn the command as a tracked background process. \
                                        Returns {process_id, pid} instead of captured output. The process is drained + killed automatically on session end.",
                        "default": false
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for the spawned process. Only applied in background mode; foreground mode uses the tool's configured cwd.",
                        "nullable": true
                    },
                    "watch_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Regex patterns to match against stdout/stderr lines. Matched lines are fanned out via the process registry watch channel (rate-limited to 8/10s per process). Only used in background mode.",
                        "default": []
                    }
                },
                "required": ["command"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: command"))?;

        // --- Plan 21.7-06 / D-29: background branch. ---
        let background = args
            .get("background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if background {
            let reg_arc = self.process_registry.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "terminal: background=true requires a ProcessRegistry to be wired via with_process_registry"
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

            let cwd_override = args
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .or_else(|| self.cwd.clone());

            let spec = SpawnSpec {
                command: command.to_string(),
                cwd: cwd_override,
                env: vec![],
                watch_patterns,
            };

            let id = {
                let mut r = reg_arc.write().await;
                r.spawn(spec).await?
            };

            // Attach output drain tasks so stdout/stderr flow into the
            // session's rolling buffer + watch rate limiter. Best-effort —
            // if this fails the child still runs, we just lose output
            // streaming; kill/drain semantics are unaffected.
            if let Err(e) = ProcessRegistry::start_output_drain(reg_arc.clone(), &id).await {
                tracing::warn!(
                    id = %id,
                    error = %e,
                    "terminal: failed to attach output drain task (process still tracked)"
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

        // --- Foreground path (unchanged from pre-21.7-06 behaviour). ---

        let timeout_secs = args["timeout"].as_u64().unwrap_or(30);

        debug!("Executing terminal command: {}", command);

        let fut = async {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(command);
            if let Some(ref dir) = self.cwd {
                cmd.current_dir(dir);
            }
            let output = cmd.output().await?;

            let mut combined = String::new();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !stdout.is_empty() {
                combined.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&stderr);
            }

            if combined.is_empty() {
                combined = format!("(exit code: {})", output.status.code().unwrap_or(-1));
            } else if output.status.code().unwrap_or(0) != 0 {
                combined.push_str(&format!(
                    "\n(exit code: {})",
                    output.status.code().unwrap_or(-1)
                ));
            }

            Ok::<String, anyhow::Error>(combined)
        };

        let result = timeout(Duration::from_secs(timeout_secs), fut)
            .await
            .map_err(|_| anyhow::anyhow!("Command timed out after {}s", timeout_secs))??;

        if result.len() > MAX_OUTPUT_LEN {
            // Find the nearest char boundary at or before MAX_OUTPUT_LEN
            let mut end = MAX_OUTPUT_LEN;
            while !result.is_char_boundary(end) {
                end -= 1;
            }
            let truncated = &result[..end];
            Ok(format!("{}\n[truncated]", truncated))
        } else {
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_terminal_new_no_cwd() {
        let tool = TerminalTool::new();
        assert!(tool.cwd.is_none());
        let result = tool
            .execute(serde_json::json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(result.contains("hello"));
    }

    #[tokio::test]
    async fn test_terminal_with_cwd() {
        let dir = tempfile::tempdir().unwrap();
        // Create a marker file in the temp dir
        std::fs::write(dir.path().join("marker.txt"), "found-it").unwrap();
        let tool = TerminalTool::with_cwd(dir.path().to_path_buf());
        assert!(tool.cwd.is_some());
        let result = tool
            .execute(serde_json::json!({"command": "cat marker.txt"}))
            .await
            .unwrap();
        assert!(
            result.contains("found-it"),
            "should execute in specified CWD, got: {result}"
        );
    }

    #[tokio::test]
    async fn test_terminal_with_cwd_pwd() {
        let dir = tempfile::tempdir().unwrap();
        let tool = TerminalTool::with_cwd(dir.path().to_path_buf());
        let result = tool
            .execute(serde_json::json!({"command": "pwd"}))
            .await
            .unwrap();
        let expected = dir.path().canonicalize().unwrap();
        let result_path = std::path::PathBuf::from(result.trim());
        let result_canon = result_path.canonicalize().unwrap_or(result_path);
        assert_eq!(result_canon, expected, "pwd should match CWD");
    }

    // --- Plan 21.7-06 / D-29 — background path tests ------------------------

    /// background=true without a registry wired must error — defensive so a
    /// wiring bug at the composition root surfaces loud (rather than silently
    /// dropping the command on the foreground path).
    #[tokio::test]
    async fn background_without_registry_errors() {
        let tool = TerminalTool::new();
        let result = tool
            .execute(serde_json::json!({"command": "sleep 5", "background": true}))
            .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("ProcessRegistry"),
            "error should mention ProcessRegistry wiring: {msg}"
        );
    }

    /// background=true with a registry spawns the process, returns the
    /// structured JSON, and leaves the process tracked in the registry.
    #[tokio::test]
    #[cfg(unix)]
    async fn background_true_spawns_into_registry() {
        let reg = Arc::new(RwLock::new(ProcessRegistry::new_for_session(
            "t-terminal-bg",
        )));
        let tool = TerminalTool::new().with_process_registry(reg.clone());

        let resp = tool
            .execute(serde_json::json!({"command": "sleep 30", "background": true}))
            .await
            .expect("background spawn must succeed");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("JSON response");
        assert_eq!(parsed["background"], true);
        let process_id = parsed["process_id"].as_str().unwrap().to_string();
        assert!(process_id.starts_with("proc_"));
        assert!(parsed["pid"].as_u64().is_some());

        // Registry accounting reflects the tracked process.
        {
            let r = reg.read().await;
            assert_eq!(
                r.running_count(),
                1,
                "must be tracked after background spawn"
            );
        }

        // Clean up (avoid leaking a real `sleep` child across tests).
        reg.write().await.drain_and_kill().await.ok();
    }

    /// Foreground regression — explicit background=false must still use the
    /// synchronous path and return captured output (not structured JSON).
    #[tokio::test]
    async fn background_false_keeps_foreground_path() {
        let tool = TerminalTool::new();
        let result = tool
            .execute(serde_json::json!({"command": "echo hi-foreground", "background": false}))
            .await
            .expect("foreground call must succeed");
        // Plain text, not JSON — matches pre-21.7-06 behaviour exactly.
        assert!(
            result.contains("hi-foreground"),
            "foreground stdout: {result}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(&result).is_err() || !result.starts_with('{'),
            "foreground output must not be JSON-wrapped"
        );
    }
}
