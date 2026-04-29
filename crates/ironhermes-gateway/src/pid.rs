//! Phase 24 — Gateway PID file infrastructure (D-09..D-12, D-18).
//!
//! Writes `$HERMES_HOME/gateway.pid` atomically via `tempfile::NamedTempFile::persist()`.
//! Hand-rolled 3-line YAML format (`pid`, `started_at`, `profile`) avoids dragging
//! `serde_yaml` into the gateway crate just for this file (D-18).
//!
//! Liveness probing uses `nix::sys::signal::kill(pid, None)` (signal 0) which is
//! Unix-only. Windows path panics with a v2.1-explicit message until ACP/Phase 30
//! adds Windows gateway support.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const PID_FILENAME: &str = "gateway.pid";

/// 3-line YAML record stored at `$HERMES_HOME/gateway.pid`.
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

/// Drop guard that removes `$HERMES_HOME/gateway.pid` on graceful shutdown.
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
        if let std::path::Component::Normal(name) = window[0] {
            if name == "profiles" {
                if let std::path::Component::Normal(slug) = window[1] {
                    return slug.to_string_lossy().to_string();
                }
            }
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
}
