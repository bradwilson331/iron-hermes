//! Backend-agnostic execution foundation.
//!
//! Ports `docs/EXEC-BACKENDS-ARCHITECTURE.md` §2 (Session core — snapshot/cwd
//! persistence across fresh `bash -c` calls), §8 (timeout/cancel/drain
//! contract), and §9 (the `Environment` trait / Rust design sketch) into
//! `ironhermes-exec::backend`. See also `.planning/phases/36.3.12-.../
//! 36.3.12-RESEARCH.md` "Pattern 1: Session core" and "Pattern 2: Environment
//! trait + factory".
//!
//! This module defines the two-primitive contract every backend
//! (Local/Docker/SSH — Plans 04/05/06) implements: `Environment::run_bash`
//! and `Environment::cleanup`. Everything else — snapshotting, cwd tracking,
//! timeout/cancel/drain — is backend-agnostic and lives in [`session`] /
//! [`executor`].

pub mod docker;
pub mod executor;
pub mod file_sync;
pub mod local;
pub mod session;
pub mod ssh;

pub use executor::execute;
pub use session::Session;

use std::fmt;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// The two-primitive contract every backend implements (§9): "run this bash
/// string in my environment, give me a handle to poll" + "release my
/// resources." Everything else (snapshotting, cwd tracking, timeout/cancel/
/// drain) is backend-agnostic and lives in `session` / `executor`.
#[async_trait::async_trait]
pub trait Environment: Send + Sync {
    /// Spawn the wrapped bash string in this environment and await its
    /// completion, honoring `opts.timeout` / `opts.cancel` (§8 — timeout maps
    /// to returncode 124, cancellation maps to 130). Backends that own a
    /// `tokio::process::Child` should implement this via the shared
    /// [`executor::run_bash_with_limits`] helper so the 124/130/drain
    /// contract is identical across backends.
    async fn run_bash(&self, cmd: &str, opts: RunOpts) -> anyhow::Result<ExecResult>;

    /// Release resources (container/connection/sandbox).
    async fn cleanup(&mut self) -> anyhow::Result<()>;

    /// Remote backends override to push file sync before each command.
    async fn before_execute(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Default temp dir inside the environment (Local may override).
    fn temp_dir(&self) -> &str {
        "/tmp"
    }
}

/// Options for a single [`Environment::run_bash`] call.
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// True when the session's snapshot isn't ready yet — the backend should
    /// fall back to a login shell (`bash -l -c`) for this call (§2.1).
    pub login: bool,
    /// Timeout for this specific run. Backends enforce this via
    /// [`executor::run_bash_with_limits`] (or an equivalent race) so
    /// `ExecResult.returncode == 124` on expiry.
    pub timeout: Duration,
    /// Optional stdin to feed the process.
    pub stdin: Option<String>,
    /// Cancellation signal for this run. Not part of the original §9 sketch's
    /// `RunOpts` — added here (Rule 2) because `Environment::run_bash` is the
    /// only per-call hook a backend has, and the D-04 §8 contract requires
    /// `Executor::execute` to be able to cancel whichever `tokio::process::
    /// Child` a backend spawned to honor `ExecResult.returncode == 130`.
    pub cancel: Option<CancellationToken>,
}

/// Result of a single [`Environment::run_bash`] call (§1.2 return contract —
/// stderr is merged into stdout).
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub output: String,
    pub returncode: i32,
}

// =============================================================================
// D-05 error taxonomy (Researcher Decision: D-05) — kept as two distinct
// types on purpose so a future maintainer cannot collapse them into a
// single catch-all that swallows a hard failure into a silent local
// fallback. See `.planning/phases/36.3.12-.../36.3.12-RESEARCH.md`
// "Researcher Decision: D-05".
// =============================================================================

/// D-05: the selected backend could not be constructed at all (runtime
/// binary missing, daemon down, host unreachable). This is a hard,
/// cross-backend failure — `create_environment` maps it into an error
/// naming the fix and NEVER falls back to `LocalEnvironment`.
#[derive(Debug)]
pub struct BackendUnavailableError(pub String);

impl fmt::Display for BackendUnavailableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BackendUnavailableError {}

/// D-02/D-05: a same-backend, self-healable condition encountered mid-
/// session (e.g. Docker's "No such container" after an out-of-band `docker
/// rm`, per Pitfall 5). Backends (Plans 05/06) retry/recreate on the SAME
/// backend when they see this — it is never routed through
/// `create_environment`'s cross-backend fallback logic, because there is
/// none.
#[derive(Debug)]
pub struct RecoverableEnvError(pub String);

impl fmt::Display for RecoverableEnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RecoverableEnvError {}

/// D-06/D-05: select and construct the `Environment` for this profile from
/// frozen operator config. Mirrors `sandbox.rs`'s `Sandbox::new(config)`
/// config-driven-orchestrator convention (config struct in, orchestrator
/// out) and its numbered-step doc-comment style:
///
/// 1. Reads `cfg.backend` — a value populated exclusively from parsed
///    `Config.terminal` (D-06: no LLM/tool-call argument path exists to
///    influence this selection; see `config_schema.rs`'s
///    `terminal_config_backend_is_config_only_d06` lock in Plan 01).
/// 2. `"local"` constructs a [`local::LocalEnvironment`] — always succeeds,
///    no external dependency to preflight.
/// 3. `"docker"` delegates to [`docker::DockerEnvironment::connect`], which
///    owns its own D-05 availability preflight (Plan 04 stub: preflight
///    only; Plan 05 fills in the container lifecycle as an own-file edit).
/// 4. `"ssh"` delegates to [`ssh::SshEnvironment::connect`] likewise (Plan
///    06 fills in the connection lifecycle).
/// 5. A construction failure for `"docker"`/`"ssh"` is mapped into an error
///    whose message names the fix — it is NEVER downgraded to
///    `LocalEnvironment` (D-05, Researcher Decision: D-05 — no
///    cross-backend fallback, ever, only same-backend self-heal via
///    `RecoverableEnvError`).
/// 6. Any other `cfg.backend` value is an "unknown terminal.backend" error
///    — also never a local fallback.
pub async fn create_environment(
    cfg: &ironhermes_core::config::TerminalConfig,
    task_id: &str,
    profile: &str,
) -> anyhow::Result<Box<dyn Environment>> {
    match cfg.backend.as_str() {
        "local" => Ok(Box::new(local::LocalEnvironment::new(
            cfg.cwd.clone(),
            Duration::from_secs(cfg.timeout),
            cfg.terminal_env_allowlist.clone(),
        )) as Box<dyn Environment>),
        "docker" => docker::DockerEnvironment::connect(cfg, task_id, profile)
            .await
            .map(|e| Box::new(e) as Box<dyn Environment>)
            .map_err(|e| {
                anyhow::anyhow!(
                    "docker backend unavailable: {e} — check the `{}` runtime binary is installed and running, or set terminal.backend: local",
                    cfg.container_runtime
                )
            }),
        "ssh" => ssh::SshEnvironment::connect(cfg, task_id)
            .await
            .map(|e| Box::new(e) as Box<dyn Environment>)
            .map_err(|e| {
                anyhow::anyhow!(
                    "ssh backend unavailable: {e} — check host reachability and terminal.ssh config, or set terminal.backend: local"
                )
            }),
        other => Err(anyhow::anyhow!(
            "unknown terminal.backend: {other} — expected \"local\", \"docker\", or \"ssh\""
        )),
    }
}

#[cfg(test)]
mod factory_tests {
    use super::*;

    /// D-05: an unrecognized `terminal.backend` string must error — there is
    /// no arm that falls through to `LocalEnvironment`.
    #[tokio::test]
    async fn create_environment_unknown_backend_errors_no_local_fallback() {
        let cfg = ironhermes_core::config::TerminalConfig {
            backend: "totally-unknown-backend".to_string(),
            ..Default::default()
        };

        let result = create_environment(&cfg, "test-task", "test-profile").await;
        let err = result
            .err()
            .expect("unknown backend must error, never fall back to local");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown terminal.backend"),
            "error should name the unrecognized backend: {msg}"
        );
    }

    /// D-06 default: `backend: local` (the struct default) always succeeds.
    #[tokio::test]
    async fn create_environment_local_default_succeeds() {
        let cfg = ironhermes_core::config::TerminalConfig::default();
        assert_eq!(cfg.backend, "local");
        let result = create_environment(&cfg, "test-task", "test-profile").await;
        assert!(
            result.is_ok(),
            "backend: local must always construct successfully"
        );
    }
}
