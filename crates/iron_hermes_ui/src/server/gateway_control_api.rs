//! Phase 48.2 Plan 13 (G-48.2-6 slice b, T-48.2-13-*): start/stop/restart the
//! `ironhermes gateway` process from the Tools page RUNTIME section.
//!
//! # Security posture (read this before touching this file)
//!
//! The action vocabulary is fixed at exactly THREE values — `stop`, `start`,
//! `restart` — and no command, argument, binary path, profile string, or
//! environment value is ever accepted from the browser. The binary comes
//! from `cli_handoff::resolve_bot_worker_bin` (the crate's existing
//! web-spawn resolver); the argv is built entirely in this file from the
//! page's own [`ConfigScope`]; the environment is `env_clear()`-then-
//! allowlist, never inherited.
//!
//! The argon2id session boundary is inherited from `auth::require_auth` —
//! every server fn in this crate sits behind it already, and this module
//! does nothing of its own to re-check or weaken that.
//!
//! Every action requires BOTH `security.web_config_write_enabled` AND the
//! NEW `security.web_process_control_enabled` (T-48.2-13-01) — checked by
//! [`process_control_gate_message`] before anything else runs, in every
//! action, including the read-only-looking parts. A blocked attempt is
//! audited exactly like a successful one (T-48.2-13-08): refusals are the
//! event this audit trail exists to prove.
//!
//! Stop is graceful only — SIGTERM via `ironhermes_gateway::pid`, never
//! SIGKILL, never an escalation path. Restart aborts before starting when
//! the stop leg does not confirm death (T-48.2-13-10). No outcome ever
//! carries a raw errno, signal number, or OS error string
//! (T-48.2-13-11) — every variant of [`GatewayLifecycleOutcome`] is a fixed
//! shape with fixed, pre-written reason text.
//!
//! # Audit
//!
//! Every attempt — success or refusal — is appended through a freshly
//! constructed `ironhermes_core::AuditLog::load(config.audit.clone())`, the
//! same construction `run_web_turn` already uses at `state.rs:660` for its
//! own per-turn guardrail audit trail. `AppState` holds no persisted
//! `audit_log` field to reach through (its `gated_audit_log` there is
//! itself built fresh per turn), so this module mirrors that exact
//! construction rather than inventing a second mechanism — `AuditLog`'s own
//! contract is safe for multiple independent instances to append to the
//! same `audit.jsonl` (append-mode only, one `fsync` per entry, and this
//! project's gateway process already writes to the same file from its own
//! separately-constructed `AuditLog`).
//!
//! # No supervision loop (T-48.2-13-13)
//!
//! This module starts, stops, and restarts a process on an explicit
//! operator action. It never watches, restarts on crash, or health-checks
//! anything on a timer. `runtime_section.rs`'s REFRESH button and the
//! page's own `refresh_tick` are the only re-reads that exist.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::server::tools_config_api::ConfigScope;

/// Every outcome a lifecycle action can honestly report. One shared enum
/// across stop/start/restart — the UI's result line renders every variant,
/// and `restart` composes the stop leg's and start leg's own variants
/// directly rather than re-deriving them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GatewayLifecycleOutcome {
    /// STOP: a running gateway was signalled and its death was confirmed
    /// within the bounded deadline.
    Stopped { pid: u32 },
    /// STOP: a running gateway was signalled but death was NOT confirmed
    /// within the deadline — reported, never escalated to a harder signal.
    StopNotConfirmed { pid: u32 },
    /// STOP: nothing was running (no pidfile, or the pid it named was
    /// already gone).
    NotRunning,
    /// STOP/RESTART: refused — the pid is live but owned by another user;
    /// the real signal would fail with EPERM the same way.
    RefusedOtherUser { pid: u32 },
    /// STOP/RESTART: refused — the pid failed target validation (0, 1,
    /// this process, or this process's own process group).
    RefusedInvalidTarget,
    /// START/RESTART: refused — a gateway is already running. The pre-spawn
    /// guard, not the child's own `acquire_pid_lock` (which is the real
    /// guarantee against the actual race).
    RefusedAlreadyRunning { pid: u32 },
    /// START: spawned, detached, environment-scrubbed, and the pidfile +
    /// probe confirmed it came up within the bounded deadline.
    Started { pid: u32, log_path: String },
    /// START: spawned, but not confirmed live within the deadline — never
    /// claimed as success. Carries the log path so the operator can look.
    StartNotConfirmed { log_path: String },
    /// START: the resolved binary could not be spawned at all.
    SpawnFailed,
    /// RESTART: the stop leg did not confirm death, so start was never
    /// attempted — the gateway is down and was not brought back. Distinct
    /// from every other outcome so the operator is told the real state
    /// rather than a generic failure.
    StoppedButNotRestarted { pid: u32 },
    /// Refused before anything else ran — one or both config gates are
    /// closed. `message` names exactly which key is missing.
    GateClosed { message: String },
    /// Refused — the per-scope cooldown is still active (a stuck button or
    /// a scripted repeat).
    CooldownActive,
    /// Refused — the requested scope could not be resolved (an invalid
    /// profile name never reaches a path).
    InvalidScope { message: String },
    /// This platform cannot probe or signal pids at all (mirrors
    /// `GatewayRuntimeStatus::UnsupportedPlatform`).
    UnsupportedPlatform,
    /// An unexpected internal failure with no OS text attached — a fixed,
    /// pre-written reason only (T-48.2-13-11).
    InternalError { reason: String },
}

// =============================================================================
// The gate — checked first in every action (T-48.2-13-01).
// =============================================================================

/// `None` when both gates are open; otherwise the fixed message naming
/// exactly which key is missing. Pure — takes the already-loaded
/// `SecurityConfig` rather than reading disk itself, so it is directly
/// unit-testable without a `Config::load()` round trip.
#[cfg(not(target_arch = "wasm32"))]
fn process_control_gate_message(cfg: &ironhermes_core::config::SecurityConfig) -> Option<String> {
    if !cfg.web_config_write_enabled {
        return Some("security.web_config_write_enabled is false".to_string());
    }
    if !cfg.web_process_control_enabled {
        return Some("security.web_process_control_enabled is false".to_string());
    }
    None
}

/// The client-visible twin of [`process_control_gate_message`] — mirrors
/// `tools_config_api::root_gate_open`'s "read-side twin" precedent. The Tools
/// page (running on the wasm client) cannot call the server-only gate helper
/// directly, so `runtime_section.rs` fetches this DTO and renders the SAME
/// fixed message the gate helper would have used to refuse an action, rather
/// than a second, hand-written copy of that text that could drift.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProcessControlGateStatus {
    pub open: bool,
    pub message: Option<String>,
}

/// Read-only — always reads the ROOT config, exactly like
/// [`gate_and_resolve`] and `tools_config_api::check_tools_write_gate`
/// before it (module doc / "The write gate is always read from ROOT"
/// precedent in `tools_config_api.rs`): both gates are the operator's
/// policy for the web surface itself, not a property of whichever
/// profile's `config.yaml` is being viewed.
#[server]
pub async fn get_process_control_gate_status() -> Result<ProcessControlGateStatus, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let config = ironhermes_core::config::Config::load().unwrap_or_default();
        let message = process_control_gate_message(&config.security);
        Ok(ProcessControlGateStatus {
            open: message.is_none(),
            message,
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// Cooldown — a stuck button or a scripted repeat cannot issue a signal
// storm (T-48.2-13-05). Keyed per scope so root and a profile never share
// a cooldown clock.
// =============================================================================

const ACTION_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(3);

#[cfg(not(target_arch = "wasm32"))]
fn scope_key(scope: &ConfigScope) -> String {
    match scope {
        ConfigScope::Root => "root".to_string(),
        ConfigScope::Profile(name) => format!("profile:{name}"),
    }
}

/// Pure: `true` when enough time has elapsed since `last` (or there was no
/// `last`) for a new action to proceed. Unit-testable without the process-
/// global map below.
#[cfg(not(target_arch = "wasm32"))]
fn cooldown_allows(now: std::time::Instant, last: Option<std::time::Instant>) -> bool {
    match last {
        None => true,
        Some(last) => now.duration_since(last) >= ACTION_COOLDOWN,
    }
}

#[cfg(not(target_arch = "wasm32"))]
static ACTION_COOLDOWN_MAP: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = std::sync::OnceLock::new();

/// `true` and arms the cooldown clock for `scope` when the action may
/// proceed; `false` (clock left untouched) when still cooling down.
#[cfg(not(target_arch = "wasm32"))]
fn check_and_arm_cooldown(scope: &ConfigScope) -> bool {
    let map =
        ACTION_COOLDOWN_MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let key = scope_key(scope);
    let mut guard = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = std::time::Instant::now();
    if !cooldown_allows(now, guard.get(&key).copied()) {
        return false;
    }
    guard.insert(key, now);
    true
}

// =============================================================================
// Scope resolution — mirrors `gateway_status_api::read_gateway_runtime_status`'s
// validate-before-use discipline (T-48.2-11-03 precedent, carried forward
// here since a lifecycle action is a strictly higher-privilege surface).
// =============================================================================

/// Resolves `scope` to its home directory and, for a profile scope, the
/// VALIDATED profile name — the same string threaded into both the
/// pidfile lookup below and the spawn argv in Task 2, so a single
/// validation covers both uses. Never forwards `validate_profile_name`'s
/// own error text — a fixed reason only.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_home_for_scope(
    scope: &ConfigScope,
) -> Result<(std::path::PathBuf, Option<String>), String> {
    match scope {
        ConfigScope::Root => Ok((ironhermes_core::get_hermes_home(), None)),
        ConfigScope::Profile(name) => {
            let validated = ironhermes_core::profile::validate_profile_name(name)
                .map_err(|_| "invalid profile name".to_string())?;
            let home = crate::server::profile_api::profile_dir_for(&validated);
            Ok((home, Some(validated)))
        }
    }
}

// =============================================================================
// Audit — every attempt, including refusals, appended through a freshly
// constructed `AuditLog` (module doc explains why this is fresh-per-call
// rather than a shared `AppState` field).
// =============================================================================

/// `(decision, reason)` for the audit entry — `"approved"` for anything that
/// actually ran (even `NotConfirmed` outcomes: the action WAS taken, its
/// completion just was not observed in time), `"denied"` for every refusal.
#[cfg(not(target_arch = "wasm32"))]
fn audit_fields_for(outcome: &GatewayLifecycleOutcome) -> (&'static str, String) {
    use GatewayLifecycleOutcome::*;
    match outcome {
        Stopped { pid } => ("approved", format!("stopped pid {pid}")),
        StopNotConfirmed { pid } => (
            "approved",
            format!("SIGTERM sent to pid {pid}; death not confirmed within the deadline"),
        ),
        NotRunning => ("approved", "no gateway was running".to_string()),
        RefusedOtherUser { pid } => (
            "denied",
            format!("pid {pid} is owned by another user; signal not attempted"),
        ),
        RefusedInvalidTarget => (
            "denied",
            "target failed validation (pid 0, pid 1, self, or self's process group)".to_string(),
        ),
        RefusedAlreadyRunning { pid } => (
            "denied",
            format!("a gateway is already running (pid {pid})"),
        ),
        Started { pid, log_path } => (
            "approved",
            format!("started pid {pid}, confirmed live; log at {log_path}"),
        ),
        StartNotConfirmed { log_path } => (
            "approved",
            format!("spawned but not confirmed live within the deadline; log at {log_path}"),
        ),
        SpawnFailed => (
            "denied",
            "the gateway process could not be spawned".to_string(),
        ),
        StoppedButNotRestarted { pid } => (
            "denied",
            format!("stop leg for pid {pid} did not confirm death; start was not attempted"),
        ),
        GateClosed { message } => ("denied", message.clone()),
        CooldownActive => (
            "denied",
            "action cooldown is active for this scope".to_string(),
        ),
        InvalidScope { message } => ("denied", message.clone()),
        UnsupportedPlatform => (
            "denied",
            "this platform cannot probe or signal process ids".to_string(),
        ),
        InternalError { reason } => ("denied", reason.clone()),
    }
}

/// Appends one entry recording `action` against `scope` with `outcome`'s
/// derived decision/reason. An append failure is logged, never swallowed
/// (module doc) — the caller's real outcome is still returned regardless.
#[cfg(not(target_arch = "wasm32"))]
async fn audit_action(
    audit_log: &ironhermes_core::AuditLog,
    action: &str,
    scope: &ConfigScope,
    outcome: &GatewayLifecycleOutcome,
) {
    let (decision, reason) = audit_fields_for(outcome);
    let scope_label = scope_key(scope);
    let args = serde_json::json!({ "action": action, "scope": scope_label, "outcome": outcome });
    let entry = audit_log.make_entry(
        "gateway_lifecycle",
        format!("gateway.{action}"),
        None,
        scope_label.clone(),
        "web",
        scope_label,
        reason,
        decision,
        "operator",
        &args,
    );
    if let Err(e) = audit_log.append(&entry).await {
        tracing::warn!(
            target: "iron_hermes_ui::gateway_control",
            action,
            error = %e,
            "failed to append gateway lifecycle audit entry"
        );
    }
}

// =============================================================================
// The shared preamble every action runs first: gate -> cooldown -> resolve.
// =============================================================================

#[cfg(not(target_arch = "wasm32"))]
struct GatewayActionCtx {
    audit_log: ironhermes_core::AuditLog,
    home: std::path::PathBuf,
    /// The SAME validated name `home` was built from — read by
    /// `start_gateway`'s argv builder (Task 2) so a single validation
    /// covers both uses. `stop_gateway` reads only `ctx.home`.
    validated_profile: Option<String>,
}

/// The refusal half of [`gate_and_resolve`]'s result — boxed at the `Err`
/// site (clippy::result_large_err): `GatewayLifecycleOutcome` carries
/// `String` fields and `AuditLog` is not tiny either, so the un-boxed pair
/// would bloat every `Result<GatewayActionCtx, _>` on the stack even along
/// the common `Ok` path.
#[cfg(not(target_arch = "wasm32"))]
type GateRefusal = Box<(GatewayLifecycleOutcome, ironhermes_core::AuditLog)>;

/// `Ok` only when both gates are open, the cooldown allows it, and the
/// scope resolves — everything downstream (stop/start/restart's own logic)
/// can assume all three already held. `Err` carries the already-computed
/// refusal outcome AND a constructed `AuditLog`, so the caller can audit
/// the refusal with the exact same helper it uses for a success.
#[cfg(not(target_arch = "wasm32"))]
fn gate_and_resolve(scope: &ConfigScope) -> Result<GatewayActionCtx, GateRefusal> {
    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    let audit_log = ironhermes_core::AuditLog::load(config.audit.clone());

    if let Some(message) = process_control_gate_message(&config.security) {
        return Err(Box::new((
            GatewayLifecycleOutcome::GateClosed { message },
            audit_log,
        )));
    }
    if !check_and_arm_cooldown(scope) {
        return Err(Box::new((
            GatewayLifecycleOutcome::CooldownActive,
            audit_log,
        )));
    }
    match resolve_home_for_scope(scope) {
        Ok((home, validated_profile)) => Ok(GatewayActionCtx {
            audit_log,
            home,
            validated_profile,
        }),
        Err(message) => Err(Box::new((
            GatewayLifecycleOutcome::InvalidScope { message },
            audit_log,
        ))),
    }
}

// =============================================================================
// STOP (Task 1) — the thin end-to-end slice.
// =============================================================================

/// The blocking body: signal, then confirm death within the bounded
/// deadline. Runs off the async executor via `spawn_blocking` (the
/// production call site below) — `await_stopped` sleeps between polls.
#[cfg(not(target_arch = "wasm32"))]
fn stop_gateway_blocking(home: &std::path::Path) -> GatewayLifecycleOutcome {
    match ironhermes_gateway::pid::request_gateway_stop(home) {
        Ok(ironhermes_gateway::pid::StopSignalOutcome::NotRunning) => {
            GatewayLifecycleOutcome::NotRunning
        }
        Ok(ironhermes_gateway::pid::StopSignalOutcome::RefusedOtherUser { pid }) => {
            GatewayLifecycleOutcome::RefusedOtherUser { pid }
        }
        Ok(ironhermes_gateway::pid::StopSignalOutcome::RefusedInvalidTarget) => {
            GatewayLifecycleOutcome::RefusedInvalidTarget
        }
        Ok(ironhermes_gateway::pid::StopSignalOutcome::Unsupported) => {
            GatewayLifecycleOutcome::UnsupportedPlatform
        }
        Ok(ironhermes_gateway::pid::StopSignalOutcome::Signalled { pid }) => {
            match ironhermes_gateway::pid::await_stopped(
                pid,
                ironhermes_gateway::pid::STOP_CONFIRM_DEADLINE,
                ironhermes_gateway::pid::is_pid_alive,
            ) {
                ironhermes_gateway::pid::DeathConfirmation::Confirmed => {
                    GatewayLifecycleOutcome::Stopped { pid }
                }
                ironhermes_gateway::pid::DeathConfirmation::NotConfirmed => {
                    GatewayLifecycleOutcome::StopNotConfirmed { pid }
                }
            }
        }
        Err(_) => GatewayLifecycleOutcome::InternalError {
            reason: "gateway.pid could not be read".to_string(),
        },
    }
}

/// The stop leg's own action logic, with no gate/cooldown/audit of its own
/// — those belong to the caller (either `stop_gateway` directly, or
/// `restart_gateway` composing this with the start leg). Wraps the
/// blocking body in `spawn_blocking` since it sleeps for up to
/// `STOP_CONFIRM_DEADLINE`.
#[cfg(not(target_arch = "wasm32"))]
async fn perform_stop(home: std::path::PathBuf) -> GatewayLifecycleOutcome {
    tokio::task::spawn_blocking(move || stop_gateway_blocking(&home))
        .await
        .unwrap_or(GatewayLifecycleOutcome::InternalError {
            reason: "background task failed unexpectedly".to_string(),
        })
}

/// Stop the gateway recorded at `scope`'s home. Behind both config gates,
/// the cooldown, and an audited outcome regardless of what happens.
#[server]
pub async fn stop_gateway(scope: ConfigScope) -> Result<GatewayLifecycleOutcome, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let outcome = match gate_and_resolve(&scope) {
            Ok(ctx) => {
                let outcome = perform_stop(ctx.home).await;
                audit_action(&ctx.audit_log, "stop", &scope, &outcome).await;
                outcome
            }
            Err(refusal) => {
                let (outcome, audit_log) = *refusal;
                audit_action(&audit_log, "stop", &scope, &outcome).await;
                outcome
            }
        };
        Ok(outcome)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// START (Task 2) — a detached child with no inherited environment.
// =============================================================================

/// Bounded deadline for [`await_started`] — modest, matching
/// `pid::STOP_CONFIRM_DEADLINE`'s posture. A start that has not confirmed
/// within it is reported spawned-but-not-confirmed, with the log path, never
/// silently claimed as success (T-48.2-13-09).
const START_CONFIRM_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
const START_CONFIRM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// The gateway's own log file name, created under the resolved home with
/// owner-only permissions (T-48.2-13-12).
const START_LOG_FILENAME: &str = "gateway-web-start.log";

/// Phase 48.2 operator request (raised at plan 13's checkpoint): log-class
/// files belong under the home's `logs/` subdirectory, alongside
/// `blackbox-*.jsonl`, `kanban/`, and `audit.jsonl`, rather than loose at the
/// home root. `open_log_file_0600` creates this parent on demand, so a fresh
/// home with no `logs/` yet still starts a gateway successfully.
const START_LOG_DIRNAME: &str = "logs";

/// Poll `probe` until it returns `true` or `deadline` elapses. Generic and
/// bool-returning (unlike `pid::await_stopped`, which is `PidLiveness`-typed)
/// because this file's own probe checks two separate facts (pidfile present
/// AND its pid live) — `deadline` stays a parameter so a test never waits
/// out the real constant.
#[cfg(not(target_arch = "wasm32"))]
fn await_started<F: FnMut() -> bool>(deadline: std::time::Duration, mut probe: F) -> bool {
    let start = std::time::Instant::now();
    loop {
        if probe() {
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        std::thread::sleep(START_CONFIRM_POLL_INTERVAL);
    }
}

/// The gateway's own argv, built ENTIRELY from this code's already-validated
/// inputs — `[--profile, <name>, ]gateway`. Nothing from the request body
/// reaches this vector (T-48.2-13-03 / prohibitions). `ironhermes-cli`'s
/// `--profile` is a global flag (`main.rs`'s `Cli.profile`) that must
/// precede the subcommand — the same placement
/// `cli_handoff::build_bot_handoff_argv` already uses. The subcommand itself
/// is `Commands::Gateway { token, non_interactive }`, invoked bare as
/// `gateway` — there is no `gateway run` subcommand in this workspace.
#[cfg(not(target_arch = "wasm32"))]
fn build_start_argv(validated_profile: Option<&str>) -> Vec<String> {
    match validated_profile {
        None => vec!["gateway".to_string()],
        Some(name) => vec![
            "--profile".to_string(),
            name.to_string(),
            "gateway".to_string(),
        ],
    }
}

/// `env_clear()`-then-allowlist, mirroring `cli_handoff::build_bot_handoff_env`'s
/// contract exactly: copy only `BOT_HANDOFF_SAFE_SYSTEM_VARS` entries that
/// are actually present in THIS process's own environment, then pin
/// `IRONHERMES_HOME` to `home` (the resolved, VALIDATED target — overwriting
/// whatever the allowlist loop may have copied from the parent, same as the
/// precedent function). This ordering is the security guarantee (module doc
/// / T-48.2-13-02): the web server's own environment holds provider API
/// keys, and this is what stops them reaching a child the browser asked for.
#[cfg(not(target_arch = "wasm32"))]
fn build_start_env(home: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    let mut env = std::collections::BTreeMap::new();
    for var in crate::server::cli_handoff::BOT_HANDOFF_SAFE_SYSTEM_VARS {
        if let Ok(v) = std::env::var(var) {
            env.insert((*var).to_string(), v);
        }
    }
    env.insert(
        "IRONHERMES_HOME".to_string(),
        home.to_string_lossy().into_owned(),
    );
    env
}

/// Open `path` for append with 0600 permissions set AT CREATION (an
/// `OpenOptionsExt::mode` flag, not a `set_permissions` call after the
/// fact) — avoids the race window a create-then-chmod sequence would leave
/// between a world-readable file existing and its permissions being
/// tightened (T-48.2-13-12, matching the profile `.env` writer's own 0600
/// discipline).
#[cfg(all(not(target_arch = "wasm32"), unix))]
fn open_log_file_0600(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    // The log now lives under `logs/`, which may not exist on a fresh home.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

#[cfg(all(not(target_arch = "wasm32"), not(unix)))]
fn open_log_file_0600(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    // The log now lives under `logs/`, which may not exist on a fresh home.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

/// The pre-spawn guard's decision, pure and independent of any actual
/// spawn: `None` permits proceeding, `Some(outcome)` refuses immediately
/// with that outcome. Reuses 48.2-11's classifier (the caller passes the
/// SAME `GatewayRuntimeStatus` `get_gateway_runtime_status` produces)
/// rather than a second liveness read with its own semantics — running and
/// running-other-user both refuse (`RefusedAlreadyRunning`); not-running and
/// stale-pidfile both permit (the child's own `acquire_pid_lock` is the
/// real guarantee against the race this guard cannot fully close);
/// unsupported-platform and unknown each refuse with their own honest
/// reason rather than risking a spawn this process cannot later confirm.
#[cfg(not(target_arch = "wasm32"))]
fn pre_spawn_guard_outcome(
    status: &crate::server::gateway_status_api::GatewayRuntimeStatus,
) -> Option<GatewayLifecycleOutcome> {
    use crate::server::gateway_status_api::GatewayRuntimeStatus::*;
    match status {
        Running { pid, .. } | RunningOtherUser { pid, .. } => {
            Some(GatewayLifecycleOutcome::RefusedAlreadyRunning { pid: *pid })
        }
        UnsupportedPlatform => Some(GatewayLifecycleOutcome::UnsupportedPlatform),
        Unknown { .. } => Some(GatewayLifecycleOutcome::InternalError {
            reason: "gateway status could not be determined".to_string(),
        }),
        NotRunning | StalePidfile { .. } => None,
    }
}

/// The blocking body: resolve the log file, spawn detached with a scrubbed
/// environment and no pipe, reap it on its own thread (this server remains
/// its OS parent even though `process_group(0)` detaches it from OUR
/// process group's signals — without a `wait()` it would zombie on exit),
/// then confirm it actually came up before claiming success. Never claims
/// success from the fact that `spawn()` returned (T-48.2-13-09 / module
/// doc's "the same class of dishonesty as the AVAILABLE pill this whole gap
/// came from").
#[cfg(not(target_arch = "wasm32"))]
fn start_gateway_blocking(
    worker_bin: &str,
    argv: &[String],
    home: &std::path::Path,
    env: &std::collections::BTreeMap<String, String>,
    log_path: &std::path::Path,
) -> GatewayLifecycleOutcome {
    let log_path_str = log_path.to_string_lossy().into_owned();

    let Ok(stdout_file) = open_log_file_0600(log_path) else {
        return GatewayLifecycleOutcome::SpawnFailed;
    };
    let Ok(stderr_file) = stdout_file.try_clone() else {
        return GatewayLifecycleOutcome::SpawnFailed;
    };

    let mut cmd = std::process::Command::new(worker_bin);
    cmd.args(argv)
        // CRITICAL: env_clear() BEFORE envs() — mirrors
        // `cli_handoff::spawn_and_capture`'s own ordering exactly.
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        // Never a pipe (T-48.2-13-06 / prohibitions): this child outlives
        // the request that spawned it, nothing would ever drain a pipe, and
        // a full pipe buffer would stop a long-lived daemon dead. Both
        // streams go to the SAME append-mode, owner-only log file instead —
        // the precedent function next door (`cli_handoff::spawn_and_capture`)
        // uses pipes correctly for its own short spawn-wait-capture
        // lifetime; that is NOT this lifetime, and must not be copied here.
        .stdout(std::process::Stdio::from(stdout_file))
        .stderr(std::process::Stdio::from(stderr_file));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Detach into its own process group so the child survives this web
        // server's own process-group signals (e.g. a Ctrl-C aimed at the
        // parent) rather than dying with it.
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return GatewayLifecycleOutcome::SpawnFailed,
    };

    // Reap on a detached thread — this process is still the child's OS
    // parent (process_group(0) only detaches it from OUR process GROUP's
    // signals), so without this the child would zombie once it exits,
    // however long from now that is.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    let home_for_probe = home.to_path_buf();
    let confirmed = await_started(START_CONFIRM_DEADLINE, move || {
        matches!(
            ironhermes_gateway::pid::read_gateway_pid(&home_for_probe),
            Ok(Some(r)) if ironhermes_gateway::pid::is_pid_alive(r.pid) == ironhermes_gateway::pid::PidLiveness::Live
        )
    });

    if !confirmed {
        return GatewayLifecycleOutcome::StartNotConfirmed {
            log_path: log_path_str,
        };
    }
    match ironhermes_gateway::pid::read_gateway_pid(home) {
        Ok(Some(record)) => GatewayLifecycleOutcome::Started {
            pid: record.pid,
            log_path: log_path_str,
        },
        // The confirmation poll just proved the pidfile existed with a live
        // pid — a read failing here immediately after is treated the same
        // honest way as never confirming, never as a different error class.
        _ => GatewayLifecycleOutcome::StartNotConfirmed {
            log_path: log_path_str,
        },
    }
}

/// The start leg's own action logic, with no gate/cooldown/audit of its own
/// (same split as `perform_stop`). Runs the pre-spawn guard by calling the
/// EXISTING `gateway_status_api::get_gateway_runtime_status` directly — same
/// server binary, so this is a plain in-process async call, not a network
/// round trip — reusing 48.2-11's classifier rather than a second liveness
/// read with its own semantics.
#[cfg(not(target_arch = "wasm32"))]
async fn perform_start(
    scope: &ConfigScope,
    home: std::path::PathBuf,
    validated_profile: Option<String>,
) -> GatewayLifecycleOutcome {
    let status =
        match crate::server::gateway_status_api::get_gateway_runtime_status(scope.clone()).await {
            Ok(status) => status,
            Err(_) => {
                return GatewayLifecycleOutcome::InternalError {
                    reason: "gateway status could not be read".to_string(),
                };
            }
        };
    if let Some(refused) = pre_spawn_guard_outcome(&status) {
        return refused;
    }

    let worker_bin = match crate::server::cli_handoff::resolve_bot_worker_bin() {
        Ok(b) => b,
        Err(_) => return GatewayLifecycleOutcome::SpawnFailed,
    };
    let argv = build_start_argv(validated_profile.as_deref());
    let env = build_start_env(&home);
    let log_path = home.join(START_LOG_DIRNAME).join(START_LOG_FILENAME);

    tokio::task::spawn_blocking(move || {
        start_gateway_blocking(&worker_bin, &argv, &home, &env, &log_path)
    })
    .await
    .unwrap_or(GatewayLifecycleOutcome::InternalError {
        reason: "background task failed unexpectedly".to_string(),
    })
}

/// Start the gateway for `scope`'s home. Refuses when one is already
/// running (the pre-spawn guard) — the child's own `acquire_pid_lock`
/// remains the actual guarantee against the race this guard cannot fully
/// close. Behind both config gates, the cooldown, and an audited outcome
/// regardless of what happens.
#[server]
pub async fn start_gateway(scope: ConfigScope) -> Result<GatewayLifecycleOutcome, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let outcome = match gate_and_resolve(&scope) {
            Ok(ctx) => {
                let outcome = perform_start(&scope, ctx.home, ctx.validated_profile).await;
                audit_action(&ctx.audit_log, "start", &scope, &outcome).await;
                outcome
            }
            Err(refusal) => {
                let (outcome, audit_log) = *refusal;
                audit_action(&audit_log, "start", &scope, &outcome).await;
                outcome
            }
        };
        Ok(outcome)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

// =============================================================================
// RESTART (Task 3) — stop, confirm death, then start, with a hard abort.
// =============================================================================

/// The load-bearing composition rule (T-48.2-13-10 / prohibitions),
/// extracted as a pure function so it is directly unit-testable with
/// INJECTED leg outcomes — never a real process. `Ok(())` means "the stop
/// leg confirmed the process is gone (or nothing was running); proceed to
/// start." `Err(outcome)` is the FINAL restart outcome and means start must
/// NOT run: `StopNotConfirmed` composes into the distinct
/// `StoppedButNotRestarted` (so the operator is told the gateway is down
/// and was not brought back, not left guessing behind a generic failure);
/// every other stop-leg outcome (refused-other-user, refused-invalid-
/// target, unsupported-platform, or an internal error) means nothing about
/// the gateway changed, so it is returned as-is.
#[cfg(not(target_arch = "wasm32"))]
fn restart_after_stop(
    stop_outcome: GatewayLifecycleOutcome,
) -> Result<(), GatewayLifecycleOutcome> {
    match stop_outcome {
        GatewayLifecycleOutcome::Stopped { .. } | GatewayLifecycleOutcome::NotRunning => Ok(()),
        GatewayLifecycleOutcome::StopNotConfirmed { pid } => {
            Err(GatewayLifecycleOutcome::StoppedButNotRestarted { pid })
        }
        other => Err(other),
    }
}

/// Composes `perform_stop` and `perform_start` — never a third
/// implementation of either leg.
#[cfg(not(target_arch = "wasm32"))]
async fn perform_restart(
    scope: &ConfigScope,
    home: std::path::PathBuf,
    validated_profile: Option<String>,
) -> GatewayLifecycleOutcome {
    let stop_outcome = perform_stop(home.clone()).await;
    match restart_after_stop(stop_outcome) {
        Ok(()) => perform_start(scope, home, validated_profile).await,
        Err(final_outcome) => final_outcome,
    }
}

/// Restart the gateway for `scope`'s home: stop, confirm death, then start
/// — composing the two actions' own implementations. Audited as ONE action
/// with its composite outcome, not as two unrelated entries, so the log
/// reads the way the operator's intent did. Behind both config gates, the
/// cooldown, and an audited outcome regardless of what happens.
#[server]
pub async fn restart_gateway(scope: ConfigScope) -> Result<GatewayLifecycleOutcome, ServerFnError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let outcome = match gate_and_resolve(&scope) {
            Ok(ctx) => {
                let outcome = perform_restart(&scope, ctx.home, ctx.validated_profile).await;
                audit_action(&ctx.audit_log, "restart", &scope, &outcome).await;
                outcome
            }
            Err(refusal) => {
                let (outcome, audit_log) = *refusal;
                audit_action(&audit_log, "restart", &scope, &outcome).await;
                outcome
            }
        };
        Ok(outcome)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = scope;
        unreachable!("server fn body never runs on the wasm client")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn sec(write: bool, control: bool) -> ironhermes_core::config::SecurityConfig {
        ironhermes_core::config::SecurityConfig {
            web_config_write_enabled: write,
            web_process_control_enabled: control,
            remote_blueprint_run_enabled: false,
        }
    }

    #[test]
    fn gate_refuses_when_write_flag_alone_is_off() {
        let msg = process_control_gate_message(&sec(false, true));
        assert!(msg.unwrap().contains("web_config_write_enabled"));
    }

    #[test]
    fn gate_refuses_when_control_flag_alone_is_off() {
        let msg = process_control_gate_message(&sec(true, false));
        assert!(msg.unwrap().contains("web_process_control_enabled"));
    }

    #[test]
    fn gate_refuses_when_both_flags_are_off() {
        assert!(process_control_gate_message(&sec(false, false)).is_some());
    }

    #[test]
    fn gate_passes_only_with_both_flags_on() {
        assert!(process_control_gate_message(&sec(true, true)).is_none());
    }

    // -------------------------------------------------------------------
    // Cooldown
    // -------------------------------------------------------------------

    #[test]
    fn cooldown_allows_a_first_action_with_no_prior_record() {
        assert!(cooldown_allows(std::time::Instant::now(), None));
    }

    #[test]
    fn cooldown_refuses_an_action_immediately_after_the_last_one() {
        let now = std::time::Instant::now();
        assert!(!cooldown_allows(now, Some(now)));
    }

    #[test]
    fn cooldown_allows_an_action_once_the_window_has_elapsed() {
        let last =
            std::time::Instant::now() - ACTION_COOLDOWN - std::time::Duration::from_millis(1);
        assert!(cooldown_allows(std::time::Instant::now(), Some(last)));
    }

    #[test]
    fn check_and_arm_cooldown_refuses_a_second_immediate_call_for_the_same_scope() {
        // Unique scope name per test run so parallel test threads never
        // collide on the process-global cooldown map.
        let scope = ConfigScope::Profile(format!("cooldown-test-{}", std::process::id()));
        assert!(check_and_arm_cooldown(&scope), "first call must be allowed");
        assert!(
            !check_and_arm_cooldown(&scope),
            "an immediate second call for the SAME scope must be refused"
        );
    }

    // -------------------------------------------------------------------
    // Scope resolution
    // -------------------------------------------------------------------

    #[test]
    fn resolve_home_for_root_scope_has_no_validated_profile() {
        let (home, profile) = resolve_home_for_scope(&ConfigScope::Root).unwrap();
        assert_eq!(home, ironhermes_core::get_hermes_home());
        assert!(profile.is_none());
    }

    #[test]
    fn resolve_home_for_valid_profile_scope_returns_the_validated_name() {
        let (_home, profile) =
            resolve_home_for_scope(&ConfigScope::Profile("work".to_string())).unwrap();
        assert_eq!(profile, Some("work".to_string()));
    }

    #[test]
    fn resolve_home_for_traversal_shaped_profile_name_is_refused_before_a_path_is_built() {
        let result = resolve_home_for_scope(&ConfigScope::Profile("../../etc/passwd".to_string()));
        assert_eq!(result, Err("invalid profile name".to_string()));
    }

    // -------------------------------------------------------------------
    // stop_gateway_blocking — real disk I/O against a tempdir, no real
    // gateway process (mirrors `gateway_status_api`'s own "live check"
    // tests).
    // -------------------------------------------------------------------

    #[test]
    fn stop_gateway_blocking_with_no_pidfile_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = stop_gateway_blocking(dir.path());
        assert_eq!(outcome, GatewayLifecycleOutcome::NotRunning);
    }

    #[test]
    fn stop_gateway_blocking_refuses_a_pidfile_naming_this_test_process() {
        // Mirrors `pid.rs`'s own `request_gateway_stop_refuses_a_pidfile_naming_this_process`
        // test one layer up — proves the outcome mapping preserves the
        // refusal rather than mis-classifying it.
        let dir = tempfile::tempdir().unwrap();
        ironhermes_gateway::pid::write_gateway_pid(
            dir.path(),
            &ironhermes_gateway::pid::GatewayPidRecord {
                pid: std::process::id(),
                started_at: "2026-01-01T00:00:00Z".to_string(),
                profile: "test".to_string(),
            },
        )
        .unwrap();
        let outcome = stop_gateway_blocking(dir.path());
        assert_eq!(outcome, GatewayLifecycleOutcome::RefusedInvalidTarget);
    }

    // -------------------------------------------------------------------
    // Audit field mapping — decision must be "denied" for every refusal
    // shape and "approved" for every outcome where the action actually ran.
    // -------------------------------------------------------------------

    #[test]
    fn audit_decision_is_denied_for_every_refusal_outcome() {
        for outcome in [
            GatewayLifecycleOutcome::RefusedOtherUser { pid: 1234 },
            GatewayLifecycleOutcome::RefusedInvalidTarget,
            GatewayLifecycleOutcome::RefusedAlreadyRunning { pid: 1234 },
            GatewayLifecycleOutcome::SpawnFailed,
            GatewayLifecycleOutcome::StoppedButNotRestarted { pid: 1234 },
            GatewayLifecycleOutcome::GateClosed {
                message: "x".to_string(),
            },
            GatewayLifecycleOutcome::CooldownActive,
            GatewayLifecycleOutcome::InvalidScope {
                message: "x".to_string(),
            },
            GatewayLifecycleOutcome::UnsupportedPlatform,
            GatewayLifecycleOutcome::InternalError {
                reason: "x".to_string(),
            },
        ] {
            let (decision, _) = audit_fields_for(&outcome);
            assert_eq!(decision, "denied", "{outcome:?} must audit as denied");
        }
    }

    #[test]
    fn audit_decision_is_approved_for_every_outcome_where_the_action_ran() {
        for outcome in [
            GatewayLifecycleOutcome::Stopped { pid: 1 },
            GatewayLifecycleOutcome::StopNotConfirmed { pid: 1 },
            GatewayLifecycleOutcome::NotRunning,
            GatewayLifecycleOutcome::Started {
                pid: 1,
                log_path: "x".to_string(),
            },
            GatewayLifecycleOutcome::StartNotConfirmed {
                log_path: "x".to_string(),
            },
        ] {
            let (decision, _) = audit_fields_for(&outcome);
            assert_eq!(decision, "approved", "{outcome:?} must audit as approved");
        }
    }

    // -------------------------------------------------------------------
    // Phase 48.2 Plan 13 Task 2 — START: argv, env, pre-spawn guard, and
    // the confirmation poll.
    // -------------------------------------------------------------------

    #[test]
    fn start_argv_for_root_scope_is_exactly_gateway() {
        assert_eq!(build_start_argv(None), vec!["gateway".to_string()]);
    }

    #[test]
    fn start_argv_for_profile_scope_places_profile_flag_before_the_subcommand() {
        assert_eq!(
            build_start_argv(Some("work")),
            vec![
                "--profile".to_string(),
                "work".to_string(),
                "gateway".to_string()
            ]
        );
    }

    #[test]
    fn start_env_never_forwards_a_sentinel_variable_outside_the_allowlist() {
        let _g = crate::server::test_support::env_lock();
        // SAFETY: serialized by env_lock() above.
        unsafe { std::env::set_var("IH_GATEWAY_CONTROL_TEST_SENTINEL", "should-not-leak") };

        let env = build_start_env(std::path::Path::new("/tmp/does-not-matter"));

        unsafe { std::env::remove_var("IH_GATEWAY_CONTROL_TEST_SENTINEL") };

        assert!(
            !env.contains_key("IH_GATEWAY_CONTROL_TEST_SENTINEL"),
            "a sentinel variable set in the test process must NOT reach the built environment: {env:?}"
        );
        // The positive half of the same contract: the allowlist entries that
        // ARE present in this test process's own environment (PATH always
        // is) DO make it through.
        assert!(
            env.contains_key("PATH"),
            "PATH is allowlisted and must be forwarded when present"
        );
        assert_eq!(
            env.get("IRONHERMES_HOME").map(String::as_str),
            Some("/tmp/does-not-matter"),
            "IRONHERMES_HOME must be pinned to the resolved home, not inherited"
        );
    }

    #[test]
    fn start_env_pins_home_even_when_parent_process_already_has_ironhermes_home_set() {
        let _g = crate::server::test_support::env_lock();
        // SAFETY: serialized by env_lock() above.
        unsafe { std::env::set_var("IRONHERMES_HOME", "/some/other/parent/home") };

        let env = build_start_env(std::path::Path::new("/the/resolved/home"));

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert_eq!(
            env.get("IRONHERMES_HOME").map(String::as_str),
            Some("/the/resolved/home"),
            "the explicit pin must overwrite whatever the allowlist loop copied from the parent"
        );
    }

    #[test]
    fn pre_spawn_guard_refuses_for_running_and_running_other_user() {
        use crate::server::gateway_status_api::GatewayRuntimeStatus;
        let running = GatewayRuntimeStatus::Running {
            pid: 42,
            started_at: "t".to_string(),
            profile: "default".to_string(),
        };
        let running_other = GatewayRuntimeStatus::RunningOtherUser {
            pid: 42,
            started_at: "t".to_string(),
            profile: "default".to_string(),
        };
        assert_eq!(
            pre_spawn_guard_outcome(&running),
            Some(GatewayLifecycleOutcome::RefusedAlreadyRunning { pid: 42 })
        );
        assert_eq!(
            pre_spawn_guard_outcome(&running_other),
            Some(GatewayLifecycleOutcome::RefusedAlreadyRunning { pid: 42 })
        );
    }

    #[test]
    fn pre_spawn_guard_permits_for_not_running_and_stale() {
        use crate::server::gateway_status_api::GatewayRuntimeStatus;
        assert_eq!(
            pre_spawn_guard_outcome(&GatewayRuntimeStatus::NotRunning),
            None
        );
        assert_eq!(
            pre_spawn_guard_outcome(&GatewayRuntimeStatus::StalePidfile { pid: 7 }),
            None
        );
    }

    #[test]
    fn pre_spawn_guard_refuses_unsupported_and_unknown() {
        use crate::server::gateway_status_api::GatewayRuntimeStatus;
        assert_eq!(
            pre_spawn_guard_outcome(&GatewayRuntimeStatus::UnsupportedPlatform),
            Some(GatewayLifecycleOutcome::UnsupportedPlatform)
        );
        assert!(matches!(
            pre_spawn_guard_outcome(&GatewayRuntimeStatus::Unknown {
                reason: "x".to_string()
            }),
            Some(GatewayLifecycleOutcome::InternalError { .. })
        ));
    }

    #[test]
    fn await_started_reports_true_once_the_probe_flips() {
        let mut calls = 0u32;
        let confirmed = await_started(std::time::Duration::from_secs(1), || {
            calls += 1;
            calls >= 3
        });
        assert!(confirmed);
        assert!(calls >= 3);
    }

    #[test]
    fn await_started_reports_false_when_the_probe_never_flips() {
        // A tiny injected deadline — never waits out the real
        // `START_CONFIRM_DEADLINE` constant.
        let confirmed = await_started(std::time::Duration::from_millis(50), || false);
        assert!(!confirmed);
    }

    // -------------------------------------------------------------------
    // Phase 48.2 Plan 13 Task 3 — RESTART's composition rule. Every case
    // drives `restart_after_stop` with an INJECTED stop-leg outcome, never
    // a real process.
    // -------------------------------------------------------------------

    #[test]
    fn restart_proceeds_to_start_when_stop_confirmed_death() {
        assert_eq!(
            restart_after_stop(GatewayLifecycleOutcome::Stopped { pid: 1 }),
            Ok(())
        );
    }

    #[test]
    fn restart_proceeds_to_start_when_nothing_was_running() {
        assert_eq!(
            restart_after_stop(GatewayLifecycleOutcome::NotRunning),
            Ok(())
        );
    }

    #[test]
    fn restart_aborts_before_start_when_stop_leg_does_not_confirm_death() {
        let result = restart_after_stop(GatewayLifecycleOutcome::StopNotConfirmed { pid: 99 });
        assert_eq!(
            result,
            Err(GatewayLifecycleOutcome::StoppedButNotRestarted { pid: 99 }),
            "a not-confirmed stop leg must produce the DISTINCT stopped-but-not-restarted \
             outcome, never a generic failure and never proceed to start"
        );
    }

    #[test]
    fn restart_passes_through_every_other_stop_leg_refusal_unchanged() {
        for outcome in [
            GatewayLifecycleOutcome::RefusedOtherUser { pid: 1 },
            GatewayLifecycleOutcome::RefusedInvalidTarget,
            GatewayLifecycleOutcome::UnsupportedPlatform,
            GatewayLifecycleOutcome::InternalError {
                reason: "x".to_string(),
            },
        ] {
            assert_eq!(
                restart_after_stop(outcome.clone()),
                Err(outcome),
                "a stop-leg refusal unrelated to confirmation must pass through unchanged, \
                 with start never attempted"
            );
        }
    }
}
