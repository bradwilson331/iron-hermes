//! `Executor` — the unified `execute()` flow (§1.2/§9) plus the shared
//! timeout/cancel/drain contract (§8) every backend's `run_bash` reuses.
//!
//! Ports `docs/EXEC-BACKENDS-ARCHITECTURE.md` §8's table verbatim in spirit:
//! normal exit → child rc, timeout → 124, interrupt/cancel → 130, and the
//! backgrounded-pipe guard (Pitfall 4) — stop draining shortly after the
//! wrapped `bash -c` exits rather than reading to EOF, since a backgrounded
//! grandchild (`sleep 999 &`) can hold the pipe open indefinitely. Mirrors
//! `sandbox.rs`'s `tokio::select!` timeout/cancel race pattern.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::{Environment, ExecResult, RunOpts, Session};

/// Bounded post-exit read-drain deadline (Pitfall 4). Once the wrapped
/// `bash -c` child has exited, we stop waiting for its stdout/stderr pipes to
/// reach EOF after this much additional time — a backgrounded grandchild
/// (`sleep 999 &`) can keep the pipe open long after the parent shell exits,
/// and reading to EOF unconditionally would hang `execute()` for as long as
/// that grandchild runs.
const DRAIN_DEADLINE: Duration = Duration::from_millis(300);

/// The unified `execute()` flow (§1.2, adapted per §9's Rust sketch):
/// `before_execute` → `wrap_command` → `env.run_bash` → `extract_cwd`,
/// persisting the newly observed cwd back into `sess` so the next caller-
/// supplied `cwd` naturally continues from wherever the last `cd` landed.
///
/// The 124 (timeout) / 130 (cancel) mapping is NOT re-raced here — `run_bash`
/// is the per-backend primitive, and the layer that owns the spawned
/// `tokio::process::Child` (a `run_bash` implementation, via
/// [`run_bash_with_limits`]) is the layer that can actually kill it. This
/// function threads `timeout`/`cancel` into [`RunOpts`] so every backend
/// enforces the same contract through the same shared helper.
pub async fn execute(
    env: &dyn Environment,
    sess: &mut Session,
    command: &str,
    cwd: &str,
    timeout: Duration,
    cancel: Option<CancellationToken>,
) -> anyhow::Result<ExecResult> {
    env.before_execute().await?;

    let wrapped = sess.wrap_command(command, cwd);
    let opts = RunOpts {
        login: !sess.ready,
        timeout,
        stdin: None,
        cancel,
    };

    let mut result = env.run_bash(&wrapped, opts).await?;

    if let Some(new_cwd) = sess.extract_cwd(&mut result.output) {
        sess.cwd = new_cwd;
    }

    Ok(result)
}

/// Shared timeout/cancel/drain helper (§8) — the reusable implementation of
/// the 124/130/no-hang contract that every backend spawning a raw
/// `tokio::process::Child` (Local — Plan 04; Docker/SSH `exec` — Plans 05/06)
/// calls from inside its own `Environment::run_bash`, rather than re-deriving
/// the race per backend.
///
/// Races `child.wait()` against `tokio::time::sleep(timeout)` and
/// `cancel.cancelled()` (mirroring `sandbox.rs`'s 3-way select): normal exit
/// returns the child's real exit code, timeout returns 124 (and kills the
/// child), cancellation returns 130 (and kills the child). Stdout/stderr are
/// drained concurrently and merged into `ExecResult.output` (§1.2 — stderr
/// merged into stdout); once the child itself has exited, draining is capped
/// at [`DRAIN_DEADLINE`] rather than reading to EOF unconditionally, so a
/// backgrounded grandchild holding the pipe open cannot hang the caller.
pub async fn run_bash_with_limits(
    mut child: tokio::process::Child,
    timeout: Duration,
    cancel: Option<CancellationToken>,
) -> anyhow::Result<ExecResult> {
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let stdout_drain = spawn_drain(stdout_handle);
    let stderr_drain = spawn_drain(stderr_handle);

    enum Outcome {
        Completed(std::io::Result<std::process::ExitStatus>),
        TimedOut,
        Cancelled,
    }

    let cancel_fut = async {
        match &cancel {
            Some(token) => token.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };

    let outcome = tokio::select! {
        status = child.wait() => Outcome::Completed(status),
        _ = tokio::time::sleep(timeout) => Outcome::TimedOut,
        _ = cancel_fut => Outcome::Cancelled,
    };

    let returncode = match &outcome {
        Outcome::Completed(status) => status
            .as_ref()
            .ok()
            .and_then(|s| s.code())
            .unwrap_or(-1),
        Outcome::TimedOut => 124,
        Outcome::Cancelled => 130,
    };

    if !matches!(outcome, Outcome::Completed(_)) {
        // Best-effort kill; a backend that also set `kill_on_drop(true)` on
        // its `Command` gets a second chance to reap the child when this
        // function returns and drops it.
        let _ = child.start_kill();
    }

    let mut output_bytes = Vec::new();
    if let Some(drain) = stdout_drain {
        output_bytes.extend(drain.finish(DRAIN_DEADLINE).await);
    }
    if let Some(drain) = stderr_drain {
        output_bytes.extend(drain.finish(DRAIN_DEADLINE).await);
    }

    Ok(ExecResult {
        output: String::from_utf8_lossy(&output_bytes).into_owned(),
        returncode,
    })
}

/// A background reader task plus the shared buffer it appends into.
///
/// Deliberately NOT `read_to_end` into a task-local `Vec` returned on
/// completion: if the task is aborted mid-read (Pitfall 4 — a backgrounded
/// grandchild keeps the pipe open past `DRAIN_DEADLINE`), any bytes already
/// read would be lost along with the task's stack. Instead the task appends
/// each chunk into a `buf` shared via `Arc<Mutex<_>>`, so `finish` can read
/// out whatever was captured so far even after aborting the task.
struct PipeDrain {
    task: tokio::task::JoinHandle<()>,
    buf: Arc<StdMutex<Vec<u8>>>,
}

impl PipeDrain {
    /// Wait up to `deadline` for the reader to reach EOF; abort it otherwise.
    /// Either way, returns whatever bytes were captured.
    async fn finish(self, deadline: Duration) -> Vec<u8> {
        let abort_handle = self.task.abort_handle();
        if tokio::time::timeout(deadline, self.task).await.is_err() {
            abort_handle.abort();
        }
        self.buf.lock().expect("drain buffer mutex poisoned").clone()
    }
}

/// Spawns a background reader that incrementally appends bytes from `handle`
/// into a shared buffer, returning `None` if there's no pipe to read.
fn spawn_drain(
    handle: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
) -> Option<PipeDrain> {
    let mut handle = handle?;
    let buf = Arc::new(StdMutex::new(Vec::new()));
    let buf_writer = Arc::clone(&buf);

    let task = tokio::spawn(async move {
        let mut chunk = [0u8; 8192];
        loop {
            match handle.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf_writer
                        .lock()
                        .expect("drain buffer mutex poisoned")
                        .extend_from_slice(&chunk[..n]);
                }
            }
        }
    });

    Some(PipeDrain { task, buf })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Minimal `Environment` backed by real `tokio::process` `bash -c`,
    /// implementing `run_bash` via the shared [`run_bash_with_limits`]
    /// helper — proving the helper works end-to-end through `execute()`,
    /// not just in isolation.
    struct TestBashEnv;

    #[async_trait::async_trait]
    impl Environment for TestBashEnv {
        async fn run_bash(&self, cmd: &str, opts: RunOpts) -> anyhow::Result<ExecResult> {
            // Note: deliberately ignores `opts.login` (no `-l`) — a login
            // shell would source the test runner's profile files, adding
            // nondeterministic latency unrelated to what this task tests
            // (the 124/130/drain contract). `Session::wrap_command`'s
            // `ready` toggle is what's under test, not the login fallback.
            let mut command = tokio::process::Command::new("bash");
            command
                .arg("-c")
                .arg(cmd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let child = command.spawn()?;
            run_bash_with_limits(child, opts.timeout, opts.cancel).await
        }

        async fn cleanup(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn execute_normal_command_returns_child_exit_code() {
        let env = TestBashEnv;
        let mut sess = Session::new("/tmp");

        let result = execute(
            &env,
            &mut sess,
            "exit 7",
            "/tmp",
            Duration::from_secs(5),
            None,
        )
        .await
        .expect("execute should succeed");

        assert_eq!(result.returncode, 7);
    }

    #[tokio::test]
    async fn execute_timeout_returns_124() {
        let env = TestBashEnv;
        let mut sess = Session::new("/tmp");

        let result = tokio::time::timeout(
            Duration::from_secs(3),
            execute(
                &env,
                &mut sess,
                "sleep 5",
                "/tmp",
                Duration::from_millis(200),
                None,
            ),
        )
        .await
        .expect("execute must not hang past its own timeout")
        .expect("execute should succeed");

        assert_eq!(result.returncode, 124);
    }

    #[tokio::test]
    async fn execute_cancel_returns_130() {
        let env = TestBashEnv;
        let mut sess = Session::new("/tmp");
        let token = CancellationToken::new();

        let cancel_after = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            cancel_after.cancel();
        });

        let result = tokio::time::timeout(
            Duration::from_secs(3),
            execute(
                &env,
                &mut sess,
                "sleep 5",
                "/tmp",
                Duration::from_secs(10),
                Some(token),
            ),
        )
        .await
        .expect("execute must not hang past cancellation")
        .expect("execute should succeed");

        assert_eq!(result.returncode, 130);
    }

    #[tokio::test]
    async fn execute_backgrounded_process_does_not_hang_the_drain() {
        let env = TestBashEnv;
        let mut sess = Session::new("/tmp");

        // `sleep 999 &` backgrounds a grandchild that inherits the wrapped
        // `bash -c` process's stdout pipe. Once the wrapped script's own
        // `exit $__hermes_ec` runs, that grandchild is the only thing still
        // holding the write end open — naive "read to EOF" would hang for
        // ~999s. `run_bash_with_limits`'s bounded post-exit drain must return
        // well within a few hundred ms instead. The orphaned `sleep 999`
        // keeps running in the background after this test — an accepted,
        // documented artifact of exercising this exact pitfall (matches the
        // Python reference's own known limitation, §8).
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            execute(
                &env,
                &mut sess,
                "sleep 999 & echo main-done",
                "/tmp",
                Duration::from_secs(10),
                None,
            ),
        )
        .await
        .expect("execute must not hang on a backgrounded grandchild")
        .expect("execute should succeed");

        assert_eq!(result.returncode, 0);
        assert!(
            result.output.contains("main-done"),
            "output should contain main-done, got: {:?}",
            result.output
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "execute took {:?}, expected it to return promptly via the drain deadline",
            started.elapsed()
        );
    }
}
