use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_exec::process_registry::{ProcessRegistry, SpawnSpec};
use serde_json::json;
use tokio::sync::RwLock;
use tokio::time::Duration;
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
///
/// Phase 42 EXEC-03 (D-04/D-06): both spawn paths call `env_clear()` then
/// `build_terminal_safe_env()` so that secrets in the parent process env (e.g.
/// `CLOUDFLARE_API_TOKEN`) cannot be exfiltrated by a subprocess that runs
/// `env`, `printenv`, or `curl -d "$(env)"`.
///
/// Phase 36.3.12 (D-01/D-04/D-06/D-12): the FOREGROUND path now routes
/// through `ironhermes_exec::backend::create_environment` (config-selected
/// Local/Docker/SSH `Environment`) instead of a bespoke `Command::new("sh")`
/// spawn. With `backend: local` (the default), observable output is
/// byte-identical to the pre-36.3.12 behavior. The BACKGROUND path (below) is
/// UNCHANGED — D-12 keeps `background=true` on the existing local
/// `ProcessRegistry` path; backend routing for background spawns is a
/// documented follow-up, not this phase's scope.
///
/// Phase 36.3.12 GAP 1: `backend_config: None` (every constructor's default)
/// is behaviorally equivalent to `backend: local`, but in PRODUCTION every
/// registered `TerminalTool` MUST carry a real `Some(TerminalConfig)` — see
/// `ToolRegistry::register_terminal_tool_with_process_registry`, the ONLY
/// production path that installs one via `with_backend_config`. `None` is
/// otherwise only expected from `TerminalTool::new()` / `register_defaults_except`'s
/// bare registration (test/non-production callers) — a production caller that
/// bypasses `register_terminal_tool_with_process_registry` silently loses
/// `terminal.backend` selection (this was, in fact, exactly the bug GAP 1 closed).
pub struct TerminalTool {
    cwd: Option<PathBuf>,
    /// Phase 42 EXEC-03 / D-05: global operator allowlist for terminal subprocesses.
    /// Empty by default — only `SAFE_ENV_KEYS` + XDG_* vars pass through.
    /// Populated via `with_env_allowlist()` from `TerminalConfig.terminal_env_allowlist`.
    env_allowlist: Vec<String>,
    /// Plan 21.7-06: Optional registry handle for background spawns. `None`
    /// leaves background-mode requests erroring out — foreground mode is
    /// always available regardless.
    process_registry: Option<Arc<RwLock<ProcessRegistry>>>,
    /// Phase 36.3.12 (D-06): resolved backend config consulted ONLY by the
    /// foreground path. `None` (the default from every constructor) behaves
    /// exactly as an implicit `backend: local` — this is what keeps the
    /// zero-config-change default byte-identical to pre-36.3.12 behavior.
    backend_config: Option<ironhermes_core::config::TerminalConfig>,
}

impl TerminalTool {
    pub fn new() -> Self {
        Self {
            cwd: None,
            env_allowlist: vec![],
            process_registry: None,
            backend_config: None,
        }
    }

    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            cwd: Some(cwd),
            env_allowlist: vec![],
            process_registry: None,
            backend_config: None,
        }
    }

    /// Plan 21.7-06 (D-29): install a shared `ProcessRegistry` handle so
    /// `background=true` calls are tracked + drained on session end.
    /// Foreground dispatch is unchanged regardless of this setter.
    pub fn with_process_registry(mut self, reg: Arc<RwLock<ProcessRegistry>>) -> Self {
        self.process_registry = Some(reg);
        self
    }

    /// Phase 42 EXEC-03 / D-05: install a per-instance env allowlist so that
    /// operator-opted-in vars (from `TerminalConfig.terminal_env_allowlist`) pass
    /// through to the child subprocess via `build_terminal_safe_env()`.
    ///
    /// Both the foreground and background spawn paths consult this list.
    /// Empty by default — only `SAFE_ENV_KEYS` (PATH/HOME/USER/…) + XDG_* pass.
    /// Must be called BEFORE `execute()` or the registry method creates a new instance.
    pub fn with_env_allowlist(mut self, allowlist: Vec<String>) -> Self {
        self.env_allowlist = allowlist;
        self
    }

    /// Phase 36.3.12 (D-06): install the resolved `TerminalConfig` so the
    /// FOREGROUND path constructs its `Environment` via
    /// `create_environment(&cfg, ...)` instead of the implicit `backend:
    /// local` default. `cfg.backend` is populated exclusively from parsed
    /// operator config (never an LLM/tool-call argument — D-06). The
    /// BACKGROUND path does not consult this at all (D-12).
    pub fn with_backend_config(mut self, cfg: ironhermes_core::config::TerminalConfig) -> Self {
        self.backend_config = Some(cfg);
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

    /// D-04 (Phase 41.3): opt out of the registry-level bound. `terminal`
    /// already honours its own `args["timeout"]` (default 30s, foreground
    /// only — background spawns return immediately) and an operator can
    /// still cap it from `tools.timeout_overrides` per D-06 level 1, which
    /// outranks this `None`.
    fn timeout_secs(&self) -> Option<u64> {
        None
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

            // EXEC-03 / D-06: pre-sanitize the env for the background SpawnSpec.
            // ProcessRegistry::spawn() will call env_clear() before applying this
            // list, so the child sees ONLY the safe allowlist (Task 2 wires that).
            let spec = SpawnSpec {
                command: command.to_string(),
                cwd: cwd_override,
                env: ironhermes_core::build_terminal_safe_env(&self.env_allowlist, &[])
                    .into_iter()
                    .collect(),
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

        // --- Foreground path. ---
        //
        // Phase 36.3.12 (D-01/D-04/D-06/D-12): routed through
        // `ironhermes_exec::backend::create_environment` instead of a bare
        // `Command::new("sh")` spawn. With `backend: local` (the default —
        // `backend_config: None` behaves identically), the spawned child, its
        // env-clear-then-scrub ordering (D-09), and the output-combining +
        // MAX_OUTPUT_LEN truncation logic below are unchanged in observable
        // effect from the pre-36.3.12 path.

        let timeout_secs = args["timeout"].as_u64().unwrap_or(30);

        debug!("Executing terminal command: {}", command);

        let fut = async {
            // `self.env_allowlist` (set via `with_env_allowlist`) is always
            // the authoritative credential allowlist for this tool instance
            // — override whatever `terminal_env_allowlist` the resolved
            // `TerminalConfig` carries so `with_backend_config` (backend
            // selection) and `with_env_allowlist` (credential allowlist)
            // stay independently controllable, exactly as before.
            let mut cfg = self.backend_config.clone().unwrap_or_default();
            cfg.terminal_env_allowlist = self.env_allowlist.clone();

            // D-06: task_id/profile only matter for docker/ssh
            // container/session naming (local, the default, ignores them). A
            // fresh id per call is sufficient — this path never persists an
            // Environment/Session across separate `execute()` invocations,
            // matching today's stateless-per-call foreground behavior.
            let task_id = uuid::Uuid::new_v4().to_string();
            let env = ironhermes_exec::backend::create_environment(&cfg, &task_id, "default")
                .await
                .map_err(|e| {
                    anyhow::anyhow!("terminal: failed to construct backend environment: {e}")
                })?;

            // An explicit `self.cwd` wins; otherwise inherit the process's
            // actual current directory — matching the pre-36.3.12 path,
            // which never called `.current_dir()` at all (the spawned child
            // simply inherited the parent's cwd).
            let initial_cwd = match &self.cwd {
                Some(dir) => dir.to_string_lossy().into_owned(),
                None => std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".to_string()),
            };

            // Phase 36.3.12 GAP 1, third `missing[]` item (VERIFICATION.md): the
            // Session-persistence scope fence, recorded here as a DECIDED,
            // DOCUMENTED limitation rather than an invisible one.
            //
            // 1. A fresh `Session` is constructed on every foreground `execute()`
            //    call (see the D-06 comment above), so `cd` and `export` do NOT
            //    persist across separate `terminal` tool calls. This is intentional
            //    parity with the pre-36.3.12 stateless-per-call behavior, not an
            //    oversight.
            // 2. As a consequence, the Session core's `ready` flag
            //    (`ironhermes_exec::backend::session::Session::ready`) is never set
            //    on this production path — `session.rs` only sets it to `true`
            //    under `#[cfg(test)]` — and `Session::bootstrap_script()` has no
            //    production call site. The cwd/env-persistence mechanism this phase
            //    built therefore has no observable effect on the production
            //    `terminal` tool today.
            // 3. Because `ready` is always false here, `ExecOptions.login`
            //    (`executor.rs`: `login: !sess.ready`) is effectively always true,
            //    which means the Docker backend injects its allow-listed `-e` env
            //    on EVERY exec rather than once at container init as D-09 describes.
            //    The D-09 allowlist (`terminal.forward_env`) still holds — nothing
            //    un-allow-listed crosses — so this is argv exposure of allow-listed
            //    values in the host's `ps` output, NOT an open credential leak.
            //    These are two different severities; do not conflate them.
            // 4. Wiring persistent Sessions into the production terminal tool is a
            //    deferred follow-up, tracked in the phase's deferred-items record
            //    (Plan 13 records the matching formal deferral).
            let mut sess = ironhermes_exec::Session::new(initial_cwd.clone());
            let result = ironhermes_exec::backend::execute(
                env.as_ref(),
                &mut sess,
                command,
                &initial_cwd,
                Duration::from_secs(timeout_secs),
                None,
            )
            .await?;

            let mut combined = result.output;
            if combined.is_empty() {
                combined = format!("(exit code: {})", result.returncode);
            } else if result.returncode != 0 {
                combined.push_str(&format!("\n(exit code: {})", result.returncode));
            }

            Ok::<String, anyhow::Error>(combined)
        };

        let result = fut.await?;

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

    // --- Phase 36.3.12 / D-01/D-04/D-06/D-12 — backend routing tests ------

    /// D-06 zero-change default: no `with_backend_config` call at all (the
    /// common case, e.g. every pre-36.3.12 call site) must behave exactly
    /// like `backend: local` — the foreground path routes through
    /// `create_environment` internally but produces identical output.
    #[tokio::test]
    async fn foreground_with_no_backend_config_behaves_as_local() {
        let tool = TerminalTool::new();
        let result = tool
            .execute(serde_json::json!({"command": "echo no-backend-config-set"}))
            .await
            .unwrap();
        assert!(result.contains("no-backend-config-set"));
    }

    /// D-06: explicitly setting `backend: local` via `with_backend_config`
    /// produces identical observable output to the implicit default.
    #[tokio::test]
    async fn foreground_with_explicit_local_backend_config() {
        let cfg = ironhermes_core::config::TerminalConfig {
            backend: "local".to_string(),
            ..Default::default()
        };
        let tool = TerminalTool::new().with_backend_config(cfg);
        let result = tool
            .execute(serde_json::json!({"command": "echo explicit-local-backend"}))
            .await
            .unwrap();
        assert!(result.contains("explicit-local-backend"));
    }

    /// D-05: an unavailable non-local backend must hard-error through the
    /// foreground path too — never silently execute the command locally.
    #[tokio::test]
    async fn foreground_with_unavailable_docker_backend_hard_errors() {
        let cfg = ironhermes_core::config::TerminalConfig {
            backend: "docker".to_string(),
            container_runtime: "definitely-absent-hermes-terminal-test-binary".to_string(),
            ..Default::default()
        };
        let tool = TerminalTool::new().with_backend_config(cfg);
        let result = tool
            .execute(serde_json::json!({"command": "echo should-not-run-locally"}))
            .await;
        let err =
            result.expect_err("docker backend unavailable must hard-error, not run locally");
        let msg = err.to_string();
        assert!(
            msg.contains("docker backend unavailable"),
            "error should name the failing backend: {msg}"
        );
    }

    /// D-09: `with_env_allowlist` remains authoritative over whatever
    /// `terminal_env_allowlist` a `with_backend_config`-supplied
    /// `TerminalConfig` carries — the two builders stay independently
    /// controllable, exactly as before.
    #[tokio::test]
    async fn with_env_allowlist_overrides_backend_config_allowlist() {
        let allowed_name = "EXEC_TERMINAL_BACKEND_CFG_ALLOWED_VAR";
        let allowed_val = "terminal-backend-cfg-allowed-value";
        // SAFETY: test-only; unique var name avoids cross-test races.
        unsafe { std::env::set_var(allowed_name, allowed_val) };

        // TerminalConfig's own terminal_env_allowlist is empty; the tool's
        // with_env_allowlist must still win.
        let cfg = ironhermes_core::config::TerminalConfig {
            backend: "local".to_string(),
            ..Default::default()
        };
        let tool = TerminalTool::new()
            .with_backend_config(cfg)
            .with_env_allowlist(vec![allowed_name.to_string()]);

        let output = tool
            .execute(serde_json::json!({
                "command": format!("printenv {allowed_name} 2>/dev/null; true")
            }))
            .await
            .unwrap();
        assert!(
            output.contains(allowed_val),
            "with_env_allowlist var must pass through even with backend_config set: {output:?}"
        );

        // SAFETY: cleanup.
        unsafe { std::env::remove_var(allowed_name) };
    }
}
