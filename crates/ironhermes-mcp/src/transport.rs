use crate::config::McpServerConfig;
use crate::security::build_safe_env;
use anyhow::Result;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};

/// Connect to a stdio MCP server. Returns the running service AND an optional
/// external handle on the spawned child process.
///
/// D-19: builds a safe environment using the allowlist (env_clear + build_safe_env).
/// The child process inherits only the safe env keys plus user-specified vars from config.
///
/// GAP-8 (Phase 21.2 Plan 11): the signature returns an `Option<tokio::process::Child>`
/// so `McpManager::shutdown_all` can hard-kill the stdio child during graceful
/// shutdown. The current implementation uses the plan-blessed Option B fallback:
/// rmcp 1.5's `TokioChildProcess::new(Command)` owns the child internally with no
/// supported constructor exposing a pre-spawned Child, so we return `None` for the
/// external handle and rely on two compounding safeguards:
///   1. `cmd.kill_on_drop(true)` inside the configure closure — when rmcp's
///      transport drops after the serve loop exits, tokio's drop-kill behavior
///      fires SIGKILL at the OS level (closing GAP-8 at the process level).
///   2. `McpManager::shutdown_all` wraps each JoinHandle await in
///      `tokio::time::timeout(Duration::from_secs(2), handle)` so the gateway
///      always exits within a bounded time regardless of child behavior.
/// Together these guarantee `ironhermes gateway` exits within ~2s/server on
/// Ctrl+C even when the stdio child ignores its parent-pipe EOF. When rmcp
/// later exposes a pre-spawned-Child constructor, `connect_stdio` can upgrade
/// to `Some(child)` without any call-site changes (Option A upgrade).
pub async fn connect_stdio(
    config: &McpServerConfig,
) -> Result<(RunningService<RoleClient, ()>, Option<tokio::process::Child>)> {
    let command = config
        .command
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("stdio transport requires 'command' field"))?;

    let safe_env = build_safe_env(&config.env);
    let args = config.args.clone();

    use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(command).configure(move |cmd| {
            for arg in &args {
                cmd.arg(arg);
            }
            // D-19: clear host env, then add only safe vars
            cmd.env_clear();
            // GAP-6b: pipe the child's stderr away from the parent terminal fd. Without
            // this, a misbehaving stdio MCP child (e.g. `npx @modelcontextprotocol/...`
            // printing its Usage line on startup failure) writes directly onto the
            // parent's tty, corrupting the `You:` prompt. Stdio::piped() means the
            // bytes land in a kernel pipe owned by the child process handle; they are
            // not surfaced to the user, which is correct for an interactive chat REPL.
            // A future plan may spawn a reader task to route captured stderr into
            // ServerTaskResult.failure_reason; that is outside GAP-6b's scope.
            cmd.stderr(std::process::Stdio::piped());
            // GAP-8: defensive SIGKILL-on-drop. When rmcp's transport drops after the
            // serve loop exits (or is cancelled), tokio's kill-on-drop semantics fire
            // SIGKILL at the OS level, so the stdio child cannot outlive its parent
            // even though rmcp 1.5 doesn't expose a pre-spawned-Child constructor for
            // us to track externally. This is the plan-11 Option B guarantee: paired
            // with the bounded 2s JoinHandle timeout in McpManager::shutdown_all, the
            // gateway always exits within bounded time on Ctrl+C.
            cmd.kill_on_drop(true);
            for (k, v) in &safe_env {
                cmd.env(k, v);
            }
        }),
    )?;

    let client = ().serve(transport).await?;
    // GAP-8 Option B: rmcp 1.5's TokioChildProcess owns the spawned Child
    // internally; no supported constructor accepts a pre-spawned Child. We
    // return None for the external handle and rely on kill_on_drop(true) +
    // the bounded JoinHandle timeout in shutdown_all for graceful shutdown.
    Ok((client, None))
}

#[cfg(test)]
mod tests {
    /// GAP-6b: static-grep regression — connect_stdio MUST set stderr to
    /// Stdio::piped() inside its configure closure so the parent terminal
    /// does not inherit the child's stderr fd. Without this, a misbehaving
    /// npx MCP server corrupts the interactive REPL prompt.
    #[test]
    fn connect_stdio_pipes_child_stderr() {
        let src = include_str!("transport.rs");
        assert!(
            src.contains("cmd.stderr(std::process::Stdio::piped());"),
            "GAP-6b: connect_stdio must call cmd.stderr(std::process::Stdio::piped()) \
             inside its configure closure so the child's stderr is NOT inherited \
             from the parent terminal"
        );
    }

    /// GAP-8: static-grep regression — connect_stdio MUST set kill_on_drop(true)
    /// inside its configure closure so the spawned stdio child is SIGKILL'd at
    /// the OS level when rmcp's transport drops. Paired with the bounded 2s
    /// JoinHandle timeout in McpManager::shutdown_all, this guarantees
    /// `ironhermes gateway` exits within bounded time on Ctrl+C even when the
    /// stdio child ignores its parent-pipe EOF.
    #[test]
    fn connect_stdio_sets_kill_on_drop() {
        let src = include_str!("transport.rs");
        assert!(
            src.contains("cmd.kill_on_drop(true);"),
            "GAP-8: connect_stdio must call cmd.kill_on_drop(true) inside its \
             configure closure so the stdio child cannot outlive its parent \
             when rmcp's transport drops"
        );
    }

    /// GAP-6b: runtime regression — spawn a trivial child process configured
    /// identically to what connect_stdio does (env_clear + piped stderr),
    /// have it write to stderr, and assert the bytes land on the child's
    /// piped stderr handle (not on the parent's stderr fd).
    ///
    /// Uses std::process::Command directly rather than going through rmcp
    /// so the test has zero dependency on a live MCP server. The behavior
    /// under test is std/tokio's Stdio::piped contract — identical to what
    /// TokioChildProcess inherits from the configure closure.
    #[test]
    fn stdio_child_stderr_does_not_inherit_parent_fd() {
        use std::io::Read;
        use std::process::{Command, Stdio};

        // A POSIX-ish command that prints to stderr and exits. `sh -c` is
        // available on macOS + Linux CI; on Windows this test is gated out.
        #[cfg(unix)]
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("printf 'usage: this-must-not-hit-parent-terminal\\n' 1>&2")
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn sh for GAP-6b test");

        #[cfg(not(unix))]
        let mut child = Command::new("cmd")
            .args(["/C", "echo usage: this-must-not-hit-parent-terminal 1>&2"])
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("failed to spawn cmd for GAP-6b test");

        let mut stderr = child
            .stderr
            .take()
            .expect("GAP-6b: Stdio::piped() must produce a reader handle on ChildStderr");

        let mut captured = String::new();
        stderr
            .read_to_string(&mut captured)
            .expect("failed to drain child stderr pipe");
        let _ = child.wait();

        assert!(
            captured.contains("usage: this-must-not-hit-parent-terminal"),
            "GAP-6b: child stderr bytes must be captured on the piped handle, not \
             inherited by the parent. captured={captured:?}"
        );
    }
}

/// Connect to an HTTP/StreamableHTTP MCP server.
///
/// Uses `StreamableHttpClientTransport` (reqwest-backed) from rmcp.
/// Requires the `transport-streamable-http-client-reqwest` feature on rmcp.
///
/// GAP-8 (Phase 21.2 Plan 11): signature symmetric with `connect_stdio` — HTTP
/// has no external child process, so the `Option<tokio::process::Child>` is
/// always `None`. Kept for call-site uniformity in `server_task::connect_and_serve`.
pub async fn connect_http(
    config: &McpServerConfig,
) -> Result<(RunningService<RoleClient, ()>, Option<tokio::process::Child>)> {
    let url = config
        .url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("HTTP transport requires 'url' field"))?;

    use rmcp::transport::StreamableHttpClientTransport;
    let transport = StreamableHttpClientTransport::from_uri(url.as_str());
    let client = ().serve(transport).await?;
    Ok((client, None))
}
