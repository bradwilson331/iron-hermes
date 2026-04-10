/// Sandbox child-process orchestration with env stripping and timeout.
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tracing::{debug, warn};

use crate::rpc_server::RpcServer;
use crate::{SandboxConfig, ToolDispatch, HERMES_TOOLS_PY};

/// Result of a sandboxed Python script execution.
#[derive(Debug, Clone)]
pub struct SandboxResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
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
    /// 3. Spawns Python with a clean environment (env_clear + allowlist)
    /// 4. Enforces timeout via tokio::time::timeout + SIGKILL
    /// 5. Truncates stdout at max_output_bytes
    pub async fn run(
        &self,
        script: &str,
        tool_dispatch: Arc<dyn ToolDispatch>,
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

        // Bind listener BEFORE dir is declared (so dir drops AFTER listener)
        // Actually, listener is declared after dir — but dir is a TempDir that
        // we keep alive until end of function. The key is that listener must be
        // dropped before dir so the socket file is released before dir cleanup.
        let listener = tokio::net::UnixListener::bind(&socket_path)?;

        // Create and spawn RPC server
        let rpc_server = RpcServer::new(listener, tool_dispatch, self.config.max_rpc_calls);
        let rpc_handle = tokio::spawn(async move {
            if let Err(e) = rpc_server.serve().await {
                debug!("RPC server ended: {}", e);
            }
        });

        // Spawn Python child process with clean environment (D-01)
        let mut child = tokio::process::Command::new(&self.config.python_path)
            .arg(&script_path)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", std::env::var("HOME").unwrap_or_default())
            .env("LANG", std::env::var("LANG").unwrap_or_default())
            .env(
                "PYTHONPATH",
                dir.path().to_str().unwrap_or_default(),
            )
            .env(
                "IRONHERMES_RPC_ADDR",
                socket_path.to_str().unwrap_or_default(),
            )
            .env("IRONHERMES_SESSION_ID", "sandbox")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        // Take stdout/stderr handles for concurrent draining (Pitfall 1 from RESEARCH)
        let mut stdout_handle = child.stdout.take().expect("stdout piped");
        let mut stderr_handle = child.stderr.take().expect("stderr piped");

        // Drain stdout and stderr concurrently, then wait for child
        let timeout_duration = Duration::from_secs(self.config.timeout_secs);

        let result = tokio::time::timeout(timeout_duration, async {
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
        })
        .await;

        // Abort the RPC server regardless of outcome
        rpc_handle.abort();

        match result {
            Ok((status, stdout_bytes, stderr_bytes)) => {
                let status = status?;
                let stdout = self.maybe_truncate(String::from_utf8_lossy(&stdout_bytes).into_owned());
                let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

                Ok(SandboxResult {
                    stdout,
                    stderr,
                    exit_code: status.code(),
                    timed_out: false,
                })
            }
            Err(_elapsed) => {
                warn!("Sandbox timeout after {}s — killing child process", self.config.timeout_secs);
                // child is killed on drop via kill_on_drop(true)
                // but we also explicitly try to kill to be safe
                // (child was moved into the async block, so kill_on_drop handles it)

                Ok(SandboxResult {
                    stdout: String::new(),
                    stderr: format!(
                        "Process killed: timeout after {}s",
                        self.config.timeout_secs
                    ),
                    exit_code: None,
                    timed_out: true,
                })
            }
        }
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
        }
    }

    #[tokio::test]
    async fn test_execute_simple_script() {
        let sandbox = Sandbox::new(test_config());
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        let result = sandbox
            .run(r#"print("hello world")"#, dispatch)
            .await
            .expect("should succeed");

        assert!(result.stdout.contains("hello world"));
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_env_stripping() {
        let sandbox = Sandbox::new(test_config());
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(NoOpDispatch);

        let script = r#"
import os
env = dict(os.environ)
print(repr(env))
"#;
        let result = sandbox.run(script, dispatch).await.expect("should succeed");

        // Must NOT contain common secret env var names
        let output = &result.stdout;
        assert!(
            !output.contains("OPENAI_API_KEY"),
            "stdout must not contain OPENAI_API_KEY"
        );
        assert!(
            !output.contains("ANTHROPIC_API_KEY"),
            "stdout must not contain ANTHROPIC_API_KEY"
        );
        assert!(
            !output.contains("OPENROUTER_API_KEY"),
            "stdout must not contain OPENROUTER_API_KEY"
        );

        // Must contain allowlisted vars
        assert!(output.contains("PATH"), "stdout must contain PATH");
        assert!(output.contains("HOME"), "stdout must contain HOME");
        assert!(
            output.contains("IRONHERMES_RPC_ADDR"),
            "stdout must contain IRONHERMES_RPC_ADDR"
        );
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
                .run("import time; time.sleep(999)", dispatch)
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
        let result = sandbox.run(script, dispatch).await.expect("should succeed");

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
        let result = sandbox.run(script, dispatch).await.expect("should succeed");

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
        let result = sandbox.run(script, dispatch).await.expect("should succeed");

        assert_eq!(result.exit_code, Some(42));
    }
}
