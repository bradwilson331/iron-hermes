//! `LocalEnvironment` — the default backend (D-06 zero-change default):
//! spawns `bash -c <wrapped>` directly on the host, reproducing
//! `crates/ironhermes-tools/src/terminal.rs`'s pre-Phase-36.3.12 foreground
//! behavior (env-clear-then-scrub, D-09) through the shared
//! `Executor`/`Session` primitives instead of a bespoke `Command` call.
//!
//! Analog: `crates/ironhermes-tools/src/terminal.rs` foreground path (lines
//! 217-281, pre-Plan-04) — the env-clear-before-scrub ordering below is
//! copied verbatim in spirit from there (see
//! `.planning/phases/36.3.12-.../36.3.12-PATTERNS.md` §local.rs).

use std::process::Stdio;
use std::time::Duration;

use super::executor::run_bash_with_limits;
use super::{Environment, ExecResult, RunOpts};

/// Runs commands directly on the host via `bash -c`. Constructed by
/// `create_environment` for `backend: local` (the default) — always
/// succeeds, since there is no external dependency to preflight (unlike
/// Docker/SSH).
pub struct LocalEnvironment {
    /// Kept for parity with the Python reference's constructor shape
    /// (`LocalEnvironment(cwd, timeout)`) and possible future use (e.g. a
    /// `temp_dir()` override); the actual per-call cwd travels through
    /// `Executor::execute`'s `cwd`/`Session.cwd`, not this field.
    #[allow(dead_code)]
    cwd: String,
    /// Default per-call timeout. `Executor::execute` callers typically pass
    /// their own timeout explicitly; kept here for parity with the Python
    /// reference and as a fallback for direct `run_bash` callers.
    #[allow(dead_code)]
    timeout: Duration,
    /// D-09: operator-declared credential allowlist
    /// (`TerminalConfig.terminal_env_allowlist`), forwarded into
    /// `build_terminal_safe_env` on every `run_bash` call.
    env_allowlist: Vec<String>,
}

impl LocalEnvironment {
    /// 1. `cwd` seeds this environment's default working directory.
    /// 2. `timeout` is this environment's default per-call timeout.
    /// 3. `env_allowlist` is the D-09 operator-declared credential allowlist,
    ///    forwarded into `build_terminal_safe_env` on every `run_bash` call.
    pub fn new(cwd: impl Into<String>, timeout: Duration, env_allowlist: Vec<String>) -> Self {
        Self {
            cwd: cwd.into(),
            timeout,
            env_allowlist,
        }
    }
}

#[async_trait::async_trait]
impl Environment for LocalEnvironment {
    /// Spawns `bash -c cmd` (the already-`Session::wrap_command`-wrapped
    /// script) with `env_clear()` applied FIRST, then ONLY the sanitized
    /// `build_terminal_safe_env(allowlist)` set (D-09 — `env_clear()` MUST
    /// precede `envs()`; reversing it is additive-only and leaks inherited
    /// secrets — the exact anti-pattern `terminal.rs`'s pre-existing
    /// foreground path and `sandbox.rs`'s env stripping both guard against),
    /// then drives the child through the shared [`run_bash_with_limits`]
    /// helper so timeout→124 / cancel→130 / drain apply identically to
    /// every other backend.
    ///
    /// Deliberately ignores `opts.login` (no `-l`), matching Plan 02's own
    /// `TestBashEnv` precedent in `executor.rs`'s test module — a login
    /// shell would source profile files, adding nondeterministic latency
    /// unrelated to what `Session::wrap_command`'s `ready` toggle already
    /// handles (conditionally sourcing the snapshot).
    async fn run_bash(&self, cmd: &str, opts: RunOpts) -> anyhow::Result<ExecResult> {
        let mut command = tokio::process::Command::new("bash");
        command
            .arg("-c")
            .arg(cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // D-09: env_clear() MUST precede envs() — see doc comment above.
        command.env_clear();
        command.envs(ironhermes_core::build_terminal_safe_env(
            &self.env_allowlist,
            &[],
        ));

        let child = command.spawn()?;
        run_bash_with_limits(child, opts.timeout, opts.cancel).await
    }

    /// No external resources to release — the host process IS the
    /// environment, so there is nothing to tear down.
    async fn cleanup(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Session, execute};

    /// Proves `LocalEnvironment` reproduces the sanitized foreground
    /// behavior end-to-end through `Executor::execute`: echo output is
    /// captured, and a `cd` in one call persists to the next via the
    /// Session's cwd tracking (the Executor's job, exercised here through
    /// the real `LocalEnvironment` rather than Plan 02's minimal
    /// `TestBashEnv`).
    #[tokio::test]
    async fn round_trip_echo_and_cwd_persistence_via_executor() {
        let env = LocalEnvironment::new("/tmp", Duration::from_secs(5), vec![]);
        let mut sess = Session::new("/tmp");

        let result = execute(
            &env,
            &mut sess,
            "echo hello-local",
            "/tmp",
            Duration::from_secs(5),
            None,
        )
        .await
        .expect("execute should succeed");
        assert_eq!(result.returncode, 0);
        assert!(
            result.output.contains("hello-local"),
            "expected echo output, got: {:?}",
            result.output
        );

        // A `cd` in this call should be reflected in sess.cwd afterward
        // (Executor::execute's job — extract_cwd + persist); the next call
        // reads `pwd -P` from that persisted cwd and must match exactly.
        let expected_cwd = sess.cwd.clone();
        let result2 = execute(
            &env,
            &mut sess,
            "pwd -P",
            &expected_cwd,
            Duration::from_secs(5),
            None,
        )
        .await
        .expect("execute should succeed");
        assert_eq!(result2.returncode, 0);
        assert_eq!(
            result2.output.trim(),
            expected_cwd,
            "pwd -P should report the persisted cwd exactly"
        );
    }

    /// D-09: `env_clear()` runs before `build_terminal_safe_env` — a secret
    /// planted in THIS test process's env must not leak into the spawned
    /// child, while PATH (part of `SAFE_ENV_KEYS`) must still be present.
    #[tokio::test]
    async fn run_bash_sanitizes_env_clear_before_scrub() {
        let secret_name = "EXEC_BACKEND_LOCAL_TEST_SECRET";
        let secret_val = "local-backend-secret-placeholder-xyz";
        // SAFETY: test-only; unique var name avoids cross-test races.
        unsafe { std::env::set_var(secret_name, secret_val) };

        let env = LocalEnvironment::new("/tmp", Duration::from_secs(5), vec![]);
        let mut sess = Session::new("/tmp");

        let result = execute(
            &env,
            &mut sess,
            &format!("printenv {secret_name} 2>/dev/null; printenv PATH"),
            "/tmp",
            Duration::from_secs(5),
            None,
        )
        .await
        .expect("execute should succeed");

        assert!(
            !result.output.contains(secret_val),
            "secret leaked into LocalEnvironment child: {:?}",
            result.output
        );
        assert!(
            !result.output.trim().is_empty(),
            "PATH should still be present (env_clear must precede envs): {:?}",
            result.output
        );

        // SAFETY: cleanup.
        unsafe { std::env::remove_var(secret_name) };
    }
}
