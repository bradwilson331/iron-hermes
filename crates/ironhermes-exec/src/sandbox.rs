/// Sandbox child-process orchestration with env stripping and timeout.
// CommandExt for pre_exec is re-exported by tokio::process::Command on unix
#[allow(unused_imports)]
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::rpc_server::RpcServer;
use crate::{SandboxConfig, ToolDispatch, HERMES_TOOLS_PY};

/// D-35: Env var name patterns that indicate secrets — stripped from child environment.
const SECRET_PATTERNS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL", "PASSWD", "AUTH"];

/// D-36: Safe system vars that always pass through regardless of secret patterns.
const SAFE_VARS: &[&str] = &[
    "PATH", "HOME", "LANG", "SHELL", "USER", "LOGNAME", "TERM",
    "PYTHONPATH", "VIRTUAL_ENV", "PYTHONDONTWRITEBYTECODE", "TMPDIR",
];

/// Result of a sandboxed Python script execution.
#[derive(Debug, Clone)]
pub struct SandboxResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// D-39: true when execution was interrupted by user cancellation.
    pub interrupted: bool,
    /// Number of RPC tool calls made during execution (D-25).
    pub tool_calls_made: u32,
    /// Wall-clock execution duration in seconds (D-25).
    pub duration_seconds: f64,
}

/// Sandbox that spawns Python child processes in an isolated environment
/// with env stripping, timeout enforcement, and output truncation.
pub struct Sandbox {
    config: SandboxConfig,
}

impl Sandbox {
    /// Create a new sandbox with the given configuration.
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Execute a Python script in the sandbox.
    ///
    /// 1. Creates a temp directory with the script and hermes_tools.py
    /// 2. Starts a UDS RPC server for tool calls
    /// 3. Spawns Python with pattern-based env stripping (D-35..D-37)
    /// 4. Enforces timeout via SIGTERM -> 5s -> SIGKILL on process group (D-31..D-34)
    /// 5. Races child completion against timeout AND optional cancellation (D-38)
    /// 6. Truncates stdout at max_output_bytes, stderr at max_stderr_bytes (D-28..D-30)
    /// 7. Reports tool_calls_made and duration_seconds (D-25)
    pub async fn run(
        &self,
        script: &str,
        tool_dispatch: Arc<dyn ToolDispatch>,
        cancel: Option<CancellationToken>,
    ) -> anyhow::Result<SandboxResult> {
        let dir = tempfile::TempDir::new()?;

        // Write embedded Python helper module
        let helper_path = dir.path().join("hermes_tools.py");
        std::fs::write(&helper_path, HERMES_TOOLS_PY)?;

        // Write script to temp file
        let script_path = dir.path().join("script.py");
        std::fs::write(&script_path, script)?;

        // Create UDS socket path inside the tempdir
        let socket_path = dir.path().join("rpc.sock");

        // Bind listener — dir is a TempDir kept alive until end of function
        let listener = tokio::net::UnixListener::bind(&socket_path)?;

        // Create shared call counter for RPC server and SandboxResult
        let call_count = Arc::new(AtomicU32::new(0));

        // Create and spawn RPC server
        let rpc_server = RpcServer::new(listener, tool_dispatch, self.config.max_rpc_calls, Arc::clone(&call_count));
        let rpc_handle = tokio::spawn(async move {
            if let Err(e) = rpc_server.serve().await {
                debug!("RPC server ended: {}", e);
            }
        });

        // Build filtered environment (D-35, D-36, D-37)
        let env_vars = self.build_env(dir.path(), &socket_path);

        // Spawn Python child process with filtered environment
        let mut cmd = tokio::process::Command::new(&self.config.python_path);
        cmd.arg(&script_path)
            .env_clear()
            .envs(env_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // D-32: child runs in its own process group via setpgid
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn()?;

        // Capture PID for process group kill (D-31)
        let child_pid = child.id();

        // Record start time for duration_seconds (D-25)
        let start = std::time::Instant::now();

        // Take stdout/stderr handles for concurrent draining (Pitfall 1 from RESEARCH)
        let mut stdout_handle = child.stdout.take().expect("stdout piped");
        let mut stderr_handle = child.stderr.take().expect("stderr piped");

        // Race child completion against timeout and optional cancellation (D-38)
        let timeout_duration = Duration::from_secs(self.config.timeout_secs);

        enum Outcome {
            Completed(Result<std::process::ExitStatus, std::io::Error>, Vec<u8>, Vec<u8>),
            TimedOut,
            Interrupted,
        }

        let outcome = tokio::select! {
            result = async {
                let stdout_task = tokio::spawn(async move {
                    let mut buf = Vec::new();
                    stdout_handle.read_to_end(&mut buf).await.ok();
                    buf
                });
                let stderr_task = tokio::spawn(async move {
                    let mut buf = Vec::new();
                    stderr_handle.read_to_end(&mut buf).await.ok();
                    buf
                });

                let status = child.wait().await;
                let stdout_bytes = stdout_task.await.unwrap_or_default();
                let stderr_bytes = stderr_task.await.unwrap_or_default();

                (status, stdout_bytes, stderr_bytes)
            } => {
                let (status, stdout_bytes, stderr_bytes) = result;
                Outcome::Completed(status, stdout_bytes, stderr_bytes)
            }
            _ = tokio::time::sleep(timeout_duration) => {
                Outcome::TimedOut
            }
            _ = async {
                match &cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => {
                Outcome::Interrupted
            }
        };

        // Abort the RPC server regardless of outcome
        rpc_handle.abort();

        match outcome {
            Outcome::Completed(status, stdout_bytes, stderr_bytes) => {
                let status = status?;
                let stdout = self.maybe_truncate(String::from_utf8_lossy(&stdout_bytes).into_owned());
                let stderr = self.maybe_truncate_stderr(String::from_utf8_lossy(&stderr_bytes).into_owned());

                Ok(SandboxResult {
                    stdout,
                    stderr,
                    exit_code: status.code(),
                    timed_out: false,
                    interrupted: false,
                    tool_calls_made: call_count.load(Ordering::SeqCst),
                    duration_seconds: start.elapsed().as_secs_f64(),
                })
            }
            Outcome::TimedOut => {
                warn!("Sandbox timeout after {}s", self.config.timeout_secs);
                // D-31: SIGTERM -> 5s grace -> SIGKILL on process group
                if let Some(pid) = child_pid {
                    let pgid = pid as i32;
                    unsafe { libc::killpg(pgid, libc::SIGTERM); }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    unsafe { libc::killpg(pgid, libc::SIGKILL); }
                }

                Ok(SandboxResult {
                    stdout: String::new(),
                    stderr: format!(
                        "Script timed out after {}s and was killed.",
                        self.config.timeout_secs
                    ),
                    exit_code: None,
                    timed_out: true,
                    interrupted: false,
                    tool_calls_made: call_count.load(Ordering::SeqCst),
                    duration_seconds: start.elapsed().as_secs_f64(),
                })
            }
            Outcome::Interrupted => {
                warn!("Sandbox interrupted by user cancellation");
                // D-39: Kill process group immediately (SIGKILL, no grace period)
                if let Some(pid) = child_pid {
                    let pgid = pid as i32;
                    unsafe { libc::killpg(pgid, libc::SIGKILL); }
                }

                Ok(SandboxResult {
                    stdout: String::new(),
                    stderr: "[execution interrupted -- user sent a new message]".into(),
                    exit_code: None,
                    timed_out: false,
                    interrupted: true,
                    tool_calls_made: call_count.load(Ordering::SeqCst),
                    duration_seconds: start.elapsed().as_secs_f64(),
                })
            }
        }
    }

    /// Build the filtered environment for the child process (D-35, D-36, D-37).
    ///
    /// Strategy: inherit all env vars EXCEPT those matching secret patterns,
    /// unless the var is in the safe list. Then inject sandbox-specific overrides.
    fn build_env(&self, temp_dir: &std::path::Path, socket_path: &std::path::Path) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = Vec::new();

        // Start with full environment, strip secrets
        for (name, value) in std::env::vars() {
            let upper = name.to_uppercase();
            // D-36: safe vars always pass through
            let is_safe = SAFE_VARS.iter().any(|s| upper == *s) || upper.starts_with("XDG_");
            // D-35: strip vars containing secret patterns (case-insensitive)
            let is_secret = SECRET_PATTERNS.iter().any(|p| upper.contains(p));

            if is_safe || !is_secret {
                env.push((name, value));
            }
        }

        // D-37: always inject these (overriding if already present)
        env.retain(|(n, _)| n != "IRONHERMES_RPC_ADDR" && n != "IRONHERMES_SESSION_ID" && n != "PYTHONPATH");
        env.push(("IRONHERMES_RPC_ADDR".into(), socket_path.to_str().unwrap_or_default().into()));
        env.push(("IRONHERMES_SESSION_ID".into(), "sandbox".into()));
        env.push(("PYTHONPATH".into(), temp_dir.to_str().unwrap_or_default().into()));

        env
    }

    /// Truncate stdout if it exceeds max_output_bytes, appending a notice.
    fn maybe_truncate(&self, output: String) -> String {
        if output.len() <= self.config.max_output_bytes {
            return output;
        }

        // Find a safe UTF-8 boundary
        let boundary = output.floor_char_boundary(self.config.max_output_bytes);
        let mut truncated = output[..boundary].to_string();
        truncated.push_str("\n[truncated: output exceeded limit]");
        truncated
    }

    /// Truncate stderr if it exceeds max_stderr_bytes (D-28, D-29, D-30).
    fn maybe_truncate_stderr(&self, stderr: String) -> String {
        if stderr.len() <= self.config.max_stderr_bytes {
            return stderr;
        }

        let boundary = stderr.floor_char_boundary(self.config.max_stderr_bytes);
        let mut truncated = stderr[..boundary].to_string();
        truncated.push_str("\n[stderr truncated at 10KB]");
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolDispatch;

    /// No-op dispatch for tests that don't need RPC.
    struct NoOpDispatch;

    #[async_trait::async_trait]
    impl ToolDispatch for NoOpDispatch {
        async fn dispatch(
            &self,
            _tool_name: &str,
            _args: serde_json::Value,
        ) -> anyhow::Result<String> {
            Ok("noop".into())
        }
    }

    fn test_config() -> SandboxConfig {
        SandboxConfig {
            python_path: "python3".into(),
            timeout_secs: 30,
            max_rpc_calls: 50,
            max_output_bytes: 50_000,
            max_stderr_bytes: 10_240,
        }
    }

    #[tokio::test]
    async fn test_execute_simple_script() {
        let sandbox = Sandbox::new(test_config());
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        let result = sandbox
            .run(r#"print("hello world")"#, dispatch, None)
            .await
            .expect("should succeed");

        assert!(result.stdout.contains("hello world"));
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_env_stripping() {
        // Set test env vars to verify pattern-based stripping
        // SAFETY: test runs single-threaded (--test-threads=1)
        unsafe {
            std::env::set_var("TEST_SECRET_VALUE", "should_be_stripped");
            std::env::set_var("MY_API_KEY", "should_be_stripped");
            std::env::set_var("DB_PASSWORD", "should_be_stripped");
            std::env::set_var("SAFE_NORMAL_VAR", "should_pass_through");
        }

        let sandbox = Sandbox::new(test_config());
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        let script = r#"
import os
env = dict(os.environ)
print(repr(env))
"#;
        let result = sandbox.run(script, dispatch, None).await.expect("should succeed");

        let output = &result.stdout;

        // D-35: Must NOT contain vars matching secret patterns
        assert!(
            !output.contains("TEST_SECRET_VALUE"),
            "stdout must not contain TEST_SECRET_VALUE (SECRET pattern)"
        );
        assert!(
            !output.contains("MY_API_KEY"),
            "stdout must not contain MY_API_KEY (KEY pattern)"
        );
        assert!(
            !output.contains("DB_PASSWORD"),
            "stdout must not contain DB_PASSWORD (PASSWORD pattern)"
        );

        // D-36: Safe system vars must pass through
        assert!(output.contains("PATH"), "stdout must contain PATH");
        assert!(output.contains("HOME"), "stdout must contain HOME");
        assert!(
            output.contains("IRONHERMES_RPC_ADDR"),
            "stdout must contain IRONHERMES_RPC_ADDR"
        );

        // Non-secret vars pass through
        assert!(
            output.contains("SAFE_NORMAL_VAR"),
            "stdout must contain SAFE_NORMAL_VAR (no secret pattern)"
        );

        // Cleanup
        unsafe {
            std::env::remove_var("TEST_SECRET_VALUE");
            std::env::remove_var("MY_API_KEY");
            std::env::remove_var("DB_PASSWORD");
            std::env::remove_var("SAFE_NORMAL_VAR");
        }
    }

    #[tokio::test]
    async fn test_timeout_kills_process() {
        let config = SandboxConfig {
            timeout_secs: 2,
            ..test_config()
        };
        let sandbox = Sandbox::new(config);
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        // Wrap the entire test in a 5-second timeout to ensure it doesn't hang
        let test_result = tokio::time::timeout(Duration::from_secs(10), async {
            sandbox
                .run("import time; time.sleep(999)", dispatch, None)
                .await
                .expect("should succeed even on timeout")
        })
        .await
        .expect("test must complete within 10 seconds");

        assert!(test_result.timed_out, "should have timed out");
    }

    #[tokio::test]
    async fn test_output_truncation() {
        let config = SandboxConfig {
            max_output_bytes: 100,
            ..test_config()
        };
        let sandbox = Sandbox::new(config);
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        let script = r#"print("A" * 60000)"#;
        let result = sandbox.run(script, dispatch, None).await.expect("should succeed");

        // stdout should be truncated: 100 bytes + truncation notice
        let notice = "\n[truncated: output exceeded limit]";
        assert!(
            result.stdout.len() <= 100 + notice.len() + 1,
            "stdout should be truncated, got {} bytes",
            result.stdout.len()
        );
        assert!(
            result.stdout.contains("[truncated: output exceeded"),
            "should contain truncation notice"
        );
    }

    #[tokio::test]
    async fn test_stderr_captured() {
        let sandbox = Sandbox::new(test_config());
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        let script = r#"import sys; sys.stderr.write("err msg")"#;
        let result = sandbox.run(script, dispatch, None).await.expect("should succeed");

        assert!(
            result.stderr.contains("err msg"),
            "stderr should contain 'err msg', got: {}",
            result.stderr
        );
    }

    #[tokio::test]
    async fn test_nonzero_exit() {
        let sandbox = Sandbox::new(test_config());
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        let script = r#"import sys; sys.exit(42)"#;
        let result = sandbox.run(script, dispatch, None).await.expect("should succeed");

        assert_eq!(result.exit_code, Some(42));
    }

    #[tokio::test]
    async fn test_stderr_truncation() {
        let config = SandboxConfig {
            max_stderr_bytes: 100,
            ..test_config()
        };
        let sandbox = Sandbox::new(config);
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        // Write >100 bytes to stderr
        let script = r#"import sys; sys.stderr.write("X" * 60000)"#;
        let result = sandbox.run(script, dispatch, None).await.expect("should succeed");

        let notice = "\n[stderr truncated at 10KB]";
        assert!(
            result.stderr.len() <= 100 + notice.len() + 1,
            "stderr should be truncated, got {} bytes",
            result.stderr.len()
        );
        assert!(
            result.stderr.contains("[stderr truncated at 10KB]"),
            "should contain stderr truncation notice, got: {}",
            result.stderr
        );
    }

    #[tokio::test]
    async fn test_duration_seconds_populated() {
        let sandbox = Sandbox::new(test_config());
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        let result = sandbox
            .run(r#"import time; time.sleep(0.1); print("done")"#, dispatch, None)
            .await
            .expect("should succeed");

        assert!(
            result.duration_seconds >= 0.05,
            "duration_seconds should be >= 0.05, got {}",
            result.duration_seconds
        );
    }
}
