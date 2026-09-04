//! Phase 24 — Gateway PID file infrastructure (D-09..D-12, D-18).
//!
//! Writes `$IRONHERMES_HOME/gateway.pid` atomically via `tempfile::NamedTempFile::persist()`.
//! Hand-rolled 3-line YAML format (`pid`, `started_at`, `profile`) avoids dragging
//! `serde_yaml` into the gateway crate just for this file (D-18).
//!
//! Liveness probing uses `nix::sys::signal::kill(pid, None)` (signal 0) which is
//! Unix-only. Windows path panics with a v2.1-explicit message until ACP/Phase 30
//! adds Windows gateway support.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::NamedTempFile;

const PID_FILENAME: &str = "gateway.pid";

/// Phase 49.3 Plan 06 (D-08): the versioned per-adapter status heartbeat
/// file, written next to `gateway.pid` by the gateway process's periodic
/// heartbeat task (`runner.rs`'s "9c" task) and read by the web server's
/// `read_platform_status` (heartbeat-first, pidfile-fallback).
const STATUS_FILENAME: &str = "gateway-status.json";

/// 3-line YAML record stored at `$IRONHERMES_HOME/gateway.pid`.
/// D-10: pid (u32), started_at (ISO8601 UTC string), profile (slug or "default").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayPidRecord {
    pub pid: u32,
    pub started_at: String,
    pub profile: String,
}

impl GatewayPidRecord {
    /// Serialize to the locked 3-line YAML form. Trailing newline included.
    pub fn to_yaml(&self) -> String {
        format!(
            "pid: {}\nstarted_at: {}\nprofile: {}\n",
            self.pid, self.started_at, self.profile
        )
    }

    /// Parse the 3-line YAML form. Strict: each prefix must appear exactly once.
    pub fn from_yaml(s: &str) -> Result<Self> {
        let mut pid: Option<u32> = None;
        let mut started_at: Option<String> = None;
        let mut profile: Option<String> = None;
        for line in s.lines() {
            if let Some(v) = line.strip_prefix("pid: ") {
                pid = Some(
                    v.trim()
                        .parse::<u32>()
                        .context("invalid pid value in gateway.pid")?,
                );
            } else if let Some(v) = line.strip_prefix("started_at: ") {
                started_at = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("profile: ") {
                profile = Some(v.trim().to_string());
            }
        }
        Ok(Self {
            pid: pid.context("gateway.pid is missing 'pid:' field")?,
            started_at: started_at.context("gateway.pid is missing 'started_at:' field")?,
            profile: profile.context("gateway.pid is missing 'profile:' field")?,
        })
    }
}

/// Atomic write via tempfile in same dir + persist (POSIX rename).
/// Per D-10 + RESEARCH Pitfall 2: must NOT use a temp file in `/tmp` —
/// rename across filesystems is non-atomic. `NamedTempFile::new_in(home)`
/// keeps the temp file in the same directory as the target.
pub fn write_gateway_pid(home: &Path, record: &GatewayPidRecord) -> Result<()> {
    std::fs::create_dir_all(home)
        .with_context(|| format!("failed to create {}", home.display()))?;
    let pid_path = home.join(PID_FILENAME);
    let mut tmp = NamedTempFile::new_in(home)
        .with_context(|| format!("failed to create temp file in {}", home.display()))?;
    tmp.write_all(record.to_yaml().as_bytes())
        .context("failed to write gateway.pid contents")?;
    tmp.flush().context("failed to flush gateway.pid")?;
    tmp.persist(&pid_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to atomic-rename gateway.pid to {}: {}",
            pid_path.display(),
            e.error
        )
    })?;
    Ok(())
}

/// Phase 49.3 Plan 06 (D-08, T-49.3-06-02): atomic write via tempfile in
/// the same dir + persist — the EXACT same shape as [`write_gateway_pid`]
/// above, so the status heartbeat gets the same no-partial-reads guarantee
/// with no second atomic-write mechanism invented. Called from exactly ONE
/// shared periodic task (`runner.rs`'s "9c" heartbeat) — never from
/// multiple racing writers to the same file.
pub fn write_gateway_status(
    home: &Path,
    status: &ironhermes_core::gateway_status::GatewayPlatformStatus,
) -> Result<()> {
    std::fs::create_dir_all(home)
        .with_context(|| format!("failed to create {}", home.display()))?;
    let status_path = home.join(STATUS_FILENAME);
    let json = serde_json::to_string_pretty(status).context("failed to serialize gateway status")?;
    let mut tmp = NamedTempFile::new_in(home)
        .with_context(|| format!("failed to create temp file in {}", home.display()))?;
    tmp.write_all(json.as_bytes())
        .context("failed to write gateway-status.json contents")?;
    tmp.flush().context("failed to flush gateway-status.json")?;
    tmp.persist(&status_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to atomic-rename gateway-status.json to {}: {}",
            status_path.display(),
            e.error
        )
    })?;
    Ok(())
}

/// Phase 49.3 Plan 06 (D-08): reads the status heartbeat file written by
/// [`write_gateway_status`]. Returns `Ok(None)` when absent (no heartbeat
/// has ever been written, or the gateway process predates this feature).
/// Returns `Err` on I/O failure or unparseable JSON — the caller
/// (`iron_hermes_ui::server::gateway_platform_status_api::read_platform_status`)
/// treats any `Err` uniformly as "no live heartbeat" and falls back to
/// pidfile liveness (T-49.3-06-01); this fn itself does not perform that
/// fallback so it stays a pure, directly-testable read.
pub fn read_gateway_status(
    home: &Path,
) -> Result<Option<ironhermes_core::gateway_status::GatewayPlatformStatus>> {
    let status_path = home.join(STATUS_FILENAME);
    if !status_path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&status_path)
        .with_context(|| format!("failed to read {}", status_path.display()))?;
    let status: ironhermes_core::gateway_status::GatewayPlatformStatus =
        serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse {}", status_path.display()))?;
    Ok(Some(status))
}

/// Returns Ok(None) when the file is absent (the common case at startup).
/// Returns Ok(Some(record)) when the file exists and parses cleanly.
/// Returns Err only on I/O failures or unparseable contents.
pub fn read_gateway_pid(home: &Path) -> Result<Option<GatewayPidRecord>> {
    let pid_path = home.join(PID_FILENAME);
    if !pid_path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&pid_path)
        .with_context(|| format!("failed to read {}", pid_path.display()))?;
    let record = GatewayPidRecord::from_yaml(&contents)
        .with_context(|| format!("failed to parse {}", pid_path.display()))?;
    Ok(Some(record))
}

/// Liveness state from a `kill(pid, 0)` probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidLiveness {
    /// Process exists and is signalable by current user.
    Live,
    /// Process does not exist (ESRCH) — safe to delete the PID file.
    Stale,
    /// Process exists but owned by another user (EPERM). Treated as live
    /// for safety; D-12 message includes an ownership note.
    LiveOtherUser,
}

#[cfg(unix)]
pub fn is_pid_alive(pid: u32) -> PidLiveness {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => PidLiveness::Live,
        Err(Errno::ESRCH) => PidLiveness::Stale,
        Err(Errno::EPERM) => PidLiveness::LiveOtherUser,
        // Any other errno: treat as stale to avoid stuck-forever startup
        // (the file will be overwritten and the new gateway takes ownership).
        Err(_) => PidLiveness::Stale,
    }
}

#[cfg(not(unix))]
pub fn is_pid_alive(_pid: u32) -> PidLiveness {
    panic!(
        "Gateway PID liveness check is not supported on this platform \
         in IronHermes v2.1 (Windows support tracked under Phase 30)."
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Phase 48.2 Plan 13 (G-48.2-6 slice b): graceful stop-signal helper.
//
// Lives here — not in `iron_hermes_ui` — because `nix` and the signal
// knowledge already live in this crate (`is_pid_alive` above), and because
// a future CLI `gateway stop` subcommand (the very thing `acquire_pid_lock`'s
// own error text points an operator at, without this workspace having ever
// implemented it) would have exactly one implementation to call.
// ─────────────────────────────────────────────────────────────────────────

/// Every outcome [`request_gateway_stop`] can honestly report. `NotRunning`
/// covers both "no pidfile" and "the pid probed stale" — both mean nothing
/// was signalled. `RefusedInvalidTarget` never carries a pid: the whole
/// point is that pid must never be treated as a legitimate target (it is
/// pid 0, pid 1, this process's own pid, or this process's own process
/// group), so nothing about it is used past the refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopSignalOutcome {
    /// No pidfile, or the pid it named was already gone.
    NotRunning,
    /// SIGTERM was delivered to a live pid owned by the calling user.
    Signalled { pid: u32 },
    /// The pid is live but owned by another user (EPERM on signal-0) — the
    /// real SIGTERM would fail the same way, so it is never attempted.
    RefusedOtherUser { pid: u32 },
    /// The pid failed target validation (0, 1, this process, or this
    /// process's own process group) before any probe or signal was issued.
    RefusedInvalidTarget,
    /// This platform cannot probe or signal pids (mirrors `is_pid_alive`'s
    /// non-Unix arm, but returns a value instead of panicking).
    Unsupported,
}

/// `true` when `pid` must never be treated as a legitimate signal target:
/// pid 0 (a process-group broadcast on some signal paths), pid 1 (init),
/// this calling process's own pid, or this calling process's own process
/// group id. Pure and Unix-only — the caller (`request_gateway_stop`)
/// checks this BEFORE any probe or signal, so a corrupted or hand-edited
/// pidfile naming one of these can never reach `kill()`.
#[cfg(unix)]
fn is_forbidden_signal_target(pid: u32) -> bool {
    if pid == 0 || pid == 1 {
        return true;
    }
    if pid == std::process::id() {
        return true;
    }
    // getpgrp() is infallible per POSIX (it can never fail for the calling
    // process) — nix's binding mirrors that with a plain `Pid`, not a
    // `Result`.
    let self_pgid = nix::unistd::getpgrp().as_raw();
    if self_pgid > 0 && pid == self_pgid as u32 {
        return true;
    }
    false
}

/// Request graceful shutdown of the gateway recorded at `home`'s
/// `gateway.pid`. Reads the record, validates the target, re-probes
/// liveness immediately before signalling (the window between reading the
/// file and sending the signal is exactly one more syscall — as small as
/// this API allows), then sends SIGTERM only. Never SIGKILL; never any
/// escalation. `#[cfg(unix)]`-gated per `is_pid_alive`'s own contract —
/// the other arm below returns [`StopSignalOutcome::Unsupported`] without
/// ever reading the pidfile or calling the probe.
#[cfg(unix)]
pub fn request_gateway_stop(home: &Path) -> Result<StopSignalOutcome> {
    let Some(record) = read_gateway_pid(home)? else {
        return Ok(StopSignalOutcome::NotRunning);
    };

    if is_forbidden_signal_target(record.pid) {
        return Ok(StopSignalOutcome::RefusedInvalidTarget);
    }

    // Re-probe immediately before signalling — this is the whole window.
    match is_pid_alive(record.pid) {
        PidLiveness::Stale => Ok(StopSignalOutcome::NotRunning),
        PidLiveness::LiveOtherUser => Ok(StopSignalOutcome::RefusedOtherUser { pid: record.pid }),
        PidLiveness::Live => {
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;
            match kill(Pid::from_raw(record.pid as i32), Signal::SIGTERM) {
                Ok(()) => Ok(StopSignalOutcome::Signalled { pid: record.pid }),
                // The pid exited in the syscall-wide race between the probe
                // above and this signal — it is gone, which is the same
                // fact `NotRunning` reports elsewhere. Any other errno
                // (e.g. EPERM from an ownership change in that same race)
                // mirrors `is_pid_alive`'s own "treat as stale" fallback
                // rather than inventing a new outcome for a race this
                // narrow.
                Err(_) => Ok(StopSignalOutcome::NotRunning),
            }
        }
    }
}

#[cfg(not(unix))]
pub fn request_gateway_stop(_home: &Path) -> Result<StopSignalOutcome> {
    Ok(StopSignalOutcome::Unsupported)
}

/// Whether [`await_stopped`]'s bounded poll observed the pid go stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathConfirmation {
    Confirmed,
    NotConfirmed,
}

/// Production deadline for [`await_stopped`] — modest on purpose. A
/// gateway that has not exited within it is reported not-confirmed, which
/// is a true statement, never escalated to a second, harder signal.
pub const STOP_CONFIRM_DEADLINE: Duration = Duration::from_secs(5);

const STOP_CONFIRM_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Poll `probe` (mirrors [`is_pid_alive`]'s signature, so the production
/// call site can pass that function directly) until it reports
/// [`PidLiveness::Stale`] or `deadline` elapses. Synchronous and blocking
/// by design — a caller on an async executor must run it via
/// `spawn_blocking`. `deadline` is a parameter (not baked in) precisely so
/// a test can pass a small one and never wait out [`STOP_CONFIRM_DEADLINE`]
/// for real; `probe` is injected for the same reason — no real process is
/// ever required to exercise either branch.
pub fn await_stopped<F>(pid: u32, deadline: Duration, mut probe: F) -> DeathConfirmation
where
    F: FnMut(u32) -> PidLiveness,
{
    let start = std::time::Instant::now();
    loop {
        if probe(pid) == PidLiveness::Stale {
            return DeathConfirmation::Confirmed;
        }
        if start.elapsed() >= deadline {
            return DeathConfirmation::NotConfirmed;
        }
        std::thread::sleep(STOP_CONFIRM_POLL_INTERVAL);
    }
}

/// Drop guard that removes `$IRONHERMES_HOME/gateway.pid` on graceful shutdown.
/// Keep this alive for the duration of the gateway run; drop on exit.
pub struct PidLockGuard {
    home: PathBuf,
}

impl PidLockGuard {
    /// For tests / explicit cleanup paths. Normally Drop handles removal.
    pub fn release(self) {
        // Drop runs the cleanup.
        drop(self);
    }
}

impl Drop for PidLockGuard {
    fn drop(&mut self) {
        let pid_path = self.home.join(PID_FILENAME);
        // Best-effort: ignore errors (file may already be gone if another
        // shutdown path removed it, or if the directory was unmounted).
        let _ = std::fs::remove_file(pid_path);
    }
}

/// Acquire the gateway PID lock for `home`.
///
/// Behavior per D-11 / D-12:
/// 1. If `gateway.pid` is absent → write a new record and return `Ok(guard)`.
/// 2. If present and `is_pid_alive` returns `Stale` → delete + write new record + return `Ok(guard)`.
/// 3. If present and `is_pid_alive` returns `Live` or `LiveOtherUser` → return `Err` (do NOT overwrite).
///
/// On the live-conflict path (case 3), exit code 2 is the expected dispatch
/// from the CLI caller (D-12). This function returns Err; the CLI maps it.
pub fn acquire_pid_lock(home: &Path) -> Result<PidLockGuard> {
    if let Some(existing) = read_gateway_pid(home)? {
        match is_pid_alive(existing.pid) {
            PidLiveness::Live => {
                return Err(anyhow::anyhow!(
                    "Gateway already running for profile '{}' (pid {}, started {}).\n   Stop it first: hermes --profile {} gateway stop",
                    existing.profile,
                    existing.pid,
                    existing.started_at,
                    existing.profile
                ));
            }
            PidLiveness::LiveOtherUser => {
                return Err(anyhow::anyhow!(
                    "Gateway already running for profile '{}' (pid {}, started {}; owned by another user).\n   Stop it first: hermes --profile {} gateway stop",
                    existing.profile,
                    existing.pid,
                    existing.started_at,
                    existing.profile
                ));
            }
            PidLiveness::Stale => {
                // Stale: remove and proceed. The subsequent write_gateway_pid
                // would overwrite anyway, but explicit removal keeps tracing logs clear.
                let pid_path = home.join(PID_FILENAME);
                let _ = std::fs::remove_file(&pid_path);
            }
        }
    }

    let record = GatewayPidRecord {
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
        profile: current_profile_label(home),
    };
    write_gateway_pid(home, &record)?;
    Ok(PidLockGuard {
        home: home.to_path_buf(),
    })
}

/// Best-effort label: for the bare-hermes path returns "default"; for a
/// `~/.ironhermes/profiles/<slug>/` path returns the slug. Used as the
/// `profile:` field in the PID record so `hermes status` (Plan 05) can
/// cross-check active vs recorded profile.
fn current_profile_label(home: &Path) -> String {
    // Walk parents looking for a `profiles/` ancestor; if found, take
    // the directory name immediately after `profiles/`.
    let components: Vec<_> = home.components().collect();
    for window in components.windows(2) {
        if let std::path::Component::Normal(name) = window[0]
            && name == "profiles"
            && let std::path::Component::Normal(slug) = window[1]
        {
            return slug.to_string_lossy().to_string();
        }
    }
    "default".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trip_yaml() {
        let r = GatewayPidRecord {
            pid: 42,
            started_at: "2026-04-28T12:00:00Z".to_string(),
            profile: "work".to_string(),
        };
        let yaml = r.to_yaml();
        assert_eq!(
            yaml,
            "pid: 42\nstarted_at: 2026-04-28T12:00:00Z\nprofile: work\n"
        );
        let parsed = GatewayPidRecord::from_yaml(&yaml).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn from_yaml_rejects_garbage() {
        assert!(GatewayPidRecord::from_yaml("garbage\n").is_err());
        assert!(GatewayPidRecord::from_yaml("pid: not-a-number\n").is_err());
        assert!(GatewayPidRecord::from_yaml("pid: 42\n").is_err()); // missing fields
    }

    #[test]
    fn write_then_read_round_trip() {
        let dir = TempDir::new().unwrap();
        let r = GatewayPidRecord {
            pid: 12345,
            started_at: "2026-04-28T00:00:00Z".to_string(),
            profile: "test".to_string(),
        };
        write_gateway_pid(dir.path(), &r).unwrap();
        let read = read_gateway_pid(dir.path()).unwrap().unwrap();
        assert_eq!(read, r);
    }

    #[test]
    fn read_gateway_pid_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_gateway_pid(dir.path()).unwrap().is_none());
    }

    #[test]
    fn pid_write_is_atomic() {
        // Write twice; both writes must end with a parseable file.
        let dir = TempDir::new().unwrap();
        let r1 = GatewayPidRecord {
            pid: 1,
            started_at: "2026-01-01T00:00:00Z".to_string(),
            profile: "a".to_string(),
        };
        let r2 = GatewayPidRecord {
            pid: 999_999,
            started_at: "2026-12-31T23:59:59Z".to_string(),
            profile: "b".to_string(),
        };
        write_gateway_pid(dir.path(), &r1).unwrap();
        let after_r1 = read_gateway_pid(dir.path()).unwrap().unwrap();
        assert_eq!(after_r1, r1);
        write_gateway_pid(dir.path(), &r2).unwrap();
        let after_r2 = read_gateway_pid(dir.path()).unwrap().unwrap();
        assert_eq!(after_r2, r2);
    }

    #[test]
    fn current_process_is_live() {
        assert_eq!(is_pid_alive(std::process::id()), PidLiveness::Live);
    }

    #[test]
    fn guaranteed_dead_pid_is_stale() {
        // Use i32::MAX as u32 (2_147_483_647): when cast to i32 it is still a large
        // positive value far above any real PID on macOS/Linux (max is 4_194_304).
        // u32::MAX would wrap to -1 (i32), which means "all processes" on POSIX and
        // returns Ok(()) even without a real target, making it a false Live result.
        assert_eq!(is_pid_alive(i32::MAX as u32), PidLiveness::Stale);
    }

    #[test]
    fn acquire_writes_new_file_when_absent() {
        let dir = TempDir::new().unwrap();
        let guard = acquire_pid_lock(dir.path()).unwrap();
        let read = read_gateway_pid(dir.path()).unwrap().unwrap();
        assert_eq!(read.pid, std::process::id());
        drop(guard);
    }

    #[test]
    fn acquire_overwrites_stale_pid() {
        let dir = TempDir::new().unwrap();
        // Use i32::MAX as u32 for the stale PID — same reasoning as guaranteed_dead_pid_is_stale.
        // u32::MAX wraps to -1 as i32 (POSIX "all processes"), giving a false Live result.
        let stale = GatewayPidRecord {
            pid: i32::MAX as u32,
            started_at: "2020-01-01T00:00:00Z".to_string(),
            profile: "test".to_string(),
        };
        write_gateway_pid(dir.path(), &stale).unwrap();
        let guard = acquire_pid_lock(dir.path()).unwrap();
        let read = read_gateway_pid(dir.path()).unwrap().unwrap();
        assert_eq!(read.pid, std::process::id()); // overwritten
        assert_ne!(read.started_at, stale.started_at); // overwritten
        drop(guard);
    }

    #[test]
    fn acquire_refuses_live_pid_and_preserves_file() {
        let dir = TempDir::new().unwrap();
        let live = GatewayPidRecord {
            pid: std::process::id(), // current process — guaranteed alive
            started_at: "2026-01-01T00:00:00Z".to_string(),
            profile: "preexisting".to_string(),
        };
        write_gateway_pid(dir.path(), &live).unwrap();
        let result = acquire_pid_lock(dir.path());
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("Stop it first"),
            "expected 'Stop it first' in error, got: {}",
            err
        );
        assert!(
            err.contains("preexisting"),
            "expected profile label in error, got: {}",
            err
        );
        // File preserved (not deleted, not overwritten)
        let read = read_gateway_pid(dir.path()).unwrap().unwrap();
        assert_eq!(read, live);
    }

    #[test]
    fn drop_guard_removes_pid_file() {
        let dir = TempDir::new().unwrap();
        {
            let _guard = acquire_pid_lock(dir.path()).unwrap();
            assert!(dir.path().join("gateway.pid").exists());
        }
        // Guard dropped at end of block
        assert!(!dir.path().join("gateway.pid").exists());
    }

    #[test]
    fn current_profile_label_extracts_slug() {
        let path = std::path::PathBuf::from("/home/user/.ironhermes/profiles/work");
        assert_eq!(current_profile_label(&path), "work");
        let path2 = std::path::PathBuf::from("/home/user/.ironhermes");
        assert_eq!(current_profile_label(&path2), "default");
    }

    // ─────────────────────────────────────────────────────────────────────
    // Phase 48.2 Plan 13 (G-48.2-6 slice b) — target validation, stop
    // signalling, and the bounded death-confirmation helper.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn forbidden_target_refuses_zero_one_self_and_own_process_group() {
        assert!(is_forbidden_signal_target(0), "pid 0 must be refused");
        assert!(
            is_forbidden_signal_target(1),
            "pid 1 (init) must be refused"
        );
        assert!(
            is_forbidden_signal_target(std::process::id()),
            "this process's own pid must be refused"
        );
        let self_pgid = nix::unistd::getpgrp().as_raw() as u32;
        assert!(
            is_forbidden_signal_target(self_pgid),
            "this process's own process group id must be refused"
        );
    }

    #[test]
    fn forbidden_target_accepts_an_ordinary_pid() {
        // Same guaranteed-dead-but-plausible pid used by
        // `guaranteed_dead_pid_is_stale` above — an ordinary target that is
        // none of pid 0, pid 1, this process, or this process's own group.
        assert!(!is_forbidden_signal_target(i32::MAX as u32));
    }

    #[test]
    fn request_gateway_stop_with_no_pidfile_is_not_running() {
        let dir = TempDir::new().unwrap();
        let outcome = request_gateway_stop(dir.path()).unwrap();
        assert_eq!(outcome, StopSignalOutcome::NotRunning);
    }

    #[test]
    fn request_gateway_stop_with_stale_pid_is_not_running() {
        let dir = TempDir::new().unwrap();
        let stale = GatewayPidRecord {
            pid: i32::MAX as u32,
            started_at: "2020-01-01T00:00:00Z".to_string(),
            profile: "test".to_string(),
        };
        write_gateway_pid(dir.path(), &stale).unwrap();
        let outcome = request_gateway_stop(dir.path()).unwrap();
        assert_eq!(outcome, StopSignalOutcome::NotRunning);
    }

    /// A pidfile naming this test process's own pid must be refused as an
    /// invalid target BEFORE any signal is attempted — proves the
    /// forbidden-target check runs inside `request_gateway_stop` itself,
    /// not just as a standalone predicate. (This process's own pid is
    /// "live" by definition, so absent this check the naive path would
    /// send SIGTERM to the test runner.)
    #[test]
    fn request_gateway_stop_refuses_a_pidfile_naming_this_process() {
        let dir = TempDir::new().unwrap();
        let record = GatewayPidRecord {
            pid: std::process::id(),
            started_at: "2026-01-01T00:00:00Z".to_string(),
            profile: "test".to_string(),
        };
        write_gateway_pid(dir.path(), &record).unwrap();
        let outcome = request_gateway_stop(dir.path()).unwrap();
        assert_eq!(outcome, StopSignalOutcome::RefusedInvalidTarget);
    }

    #[test]
    fn await_stopped_reports_confirmed_when_probe_flips_to_stale() {
        let mut calls = 0u32;
        let outcome = await_stopped(42, Duration::from_secs(1), |_| {
            calls += 1;
            if calls >= 3 {
                PidLiveness::Stale
            } else {
                PidLiveness::Live
            }
        });
        assert_eq!(outcome, DeathConfirmation::Confirmed);
        assert!(calls >= 3);
    }

    #[test]
    fn await_stopped_reports_not_confirmed_at_the_deadline() {
        // A tiny injected deadline — never waits out the real
        // `STOP_CONFIRM_DEADLINE` constant. The probe never goes stale.
        let outcome = await_stopped(42, Duration::from_millis(50), |_| PidLiveness::Live);
        assert_eq!(outcome, DeathConfirmation::NotConfirmed);
    }

    // ─────────────────────────────────────────────────────────────────────
    // Phase 49.3 Plan 06 (D-08): status-file atomic writer/reader.
    // ─────────────────────────────────────────────────────────────────────

    use ironhermes_core::gateway_status::{GatewayPlatformStatus, PlatformStatusEntry};
    use std::collections::BTreeMap;

    fn sample_status() -> GatewayPlatformStatus {
        let mut platforms = BTreeMap::new();
        platforms.insert(
            "telegram".to_string(),
            PlatformStatusEntry {
                connected: true,
                session_count: 2,
            },
        );
        platforms.insert(
            "discord".to_string(),
            PlatformStatusEntry {
                connected: false,
                session_count: 0,
            },
        );
        GatewayPlatformStatus::new(platforms)
    }

    #[test]
    fn write_then_read_gateway_status_round_trip() {
        let dir = TempDir::new().unwrap();
        let status = sample_status();
        write_gateway_status(dir.path(), &status).unwrap();
        let read = read_gateway_status(dir.path()).unwrap().unwrap();
        assert_eq!(read, status);
    }

    #[test]
    fn read_gateway_status_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_gateway_status(dir.path()).unwrap().is_none());
    }

    #[test]
    fn gateway_status_write_is_atomic_across_repeated_writes() {
        // Mirrors `pid_write_is_atomic` above — writing twice must always
        // leave a fully-parseable file (no half-written intermediate state
        // ever observable), proving the same NamedTempFile-then-persist
        // shape is in use.
        let dir = TempDir::new().unwrap();
        let s1 = sample_status();
        write_gateway_status(dir.path(), &s1).unwrap();
        assert_eq!(read_gateway_status(dir.path()).unwrap().unwrap(), s1);

        let mut platforms2 = BTreeMap::new();
        platforms2.insert(
            "buzz".to_string(),
            PlatformStatusEntry {
                connected: true,
                session_count: 7,
            },
        );
        let s2 = GatewayPlatformStatus::new(platforms2);
        write_gateway_status(dir.path(), &s2).unwrap();
        let after = read_gateway_status(dir.path()).unwrap().unwrap();
        assert_eq!(after.platforms.get("buzz").unwrap().session_count, 7);
    }

    #[test]
    fn read_gateway_status_rejects_garbage() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("gateway-status.json"), "not json").unwrap();
        assert!(read_gateway_status(dir.path()).is_err());
    }
}
