//! Phase 26.3.2 — Chrome SingletonLock resilience.
//!
//! Provides `reconcile_singleton_lock(profile_dir: &Path) -> SingletonOutcome`
//! called from both `BrowserSession::spawn()` copies before profile selection.
//! The liveness probe is dep-free (no `nix`/`libc`/`sysinfo`/`hostname` crates — D-02):
//! `/proc/<pid>` on Linux; `kill -0` via `std::process::Command` on other Unix.

use std::path::Path;
use tracing::{debug, warn};

/// The outcome of the singleton lock check. Returned by `reconcile_singleton_lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingletonOutcome {
    /// Profile is available: either no lock, or the stale lock was cleaned.
    /// Caller MUST call `builder.user_data_dir(profile_dir)`.
    UseProfile,
    /// Lock is held by a live process. Caller MUST NOT call `builder.user_data_dir(...)`.
    /// Chromiumoxide will use a fresh ephemeral temp dir.
    UseEphemeral,
}

/// Check and optionally repair the `SingletonLock` in `profile_dir`.
///
/// Logic (D-01..D-06, D-09):
///  - No lock → `UseProfile` (debug log)
///  - Lock present, stale (dead pid or hostname mismatch) → remove sentinels, `UseProfile` (warn log)
///  - Lock present, live → `UseEphemeral` (warn log)
///  - Lock unparseable (not a symlink, or target has no trailing `-<pid>`) → `UseProfile`,
///    do NOT delete (debug log)
///
/// On non-Unix platforms this is a no-op returning `UseProfile` unconditionally (D-09).
pub fn reconcile_singleton_lock(profile_dir: &Path) -> SingletonOutcome {
    #[cfg(unix)]
    return reconcile_unix(profile_dir);
    #[cfg(not(unix))]
    {
        let _ = profile_dir;
        SingletonOutcome::UseProfile
    }
}

#[cfg(unix)]
fn reconcile_unix(profile_dir: &Path) -> SingletonOutcome {
    let lock_path = profile_dir.join("SingletonLock");

    // D-01: read_link; if absent or not-a-symlink, proceed normally (do NOT delete).
    let target = match std::fs::read_link(&lock_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            debug!(path = %lock_path.display(), "SingletonLock absent, launching normally");
            return SingletonOutcome::UseProfile;
        }
        Err(_) => {
            // Not a symlink (EINVAL on Linux/macOS for read_link on a non-symlink) or
            // other I/O error — unparseable per D-01, do not delete.
            debug!(
                path = %lock_path.display(),
                "SingletonLock present but not a symlink; treating as unparseable, proceeding without cleanup"
            );
            return SingletonOutcome::UseProfile;
        }
    };

    let target_str = target.to_string_lossy();

    // D-01: parse pid from last '-' segment; failure → unparseable, do NOT delete.
    let Some(pid) = parse_singleton_target(&target_str) else {
        debug!(
            path = %lock_path.display(),
            target = %target_str,
            "SingletonLock target unparseable (no trailing -<pid>), proceeding without cleanup"
        );
        return SingletonOutcome::UseProfile;
    };

    // D-03: hostname check — if different host, it's stale regardless of pid.
    if let Some(lock_host) = parse_hostname_from_target(&target_str) {
        if let Some(our_host) = current_hostname() {
            if lock_host != our_host.as_str() {
                warn!(
                    path = %lock_path.display(),
                    lock_host = %lock_host,
                    our_host = %our_host,
                    "SingletonLock written by different host (shared profile dir?); cleaning stale sentinels"
                );
                cleanup_sentinels(profile_dir);
                return SingletonOutcome::UseProfile;
            }
        }
        // If current_hostname() returns None: fall through to pid-liveness check (D-03 fallback).
    }

    // D-02 / D-05 / D-06: pid liveness check — stale check always runs before any cleanup.
    if is_pid_alive(pid) {
        warn!(
            path = %profile_dir.display(),
            pid = pid,
            "browser profile locked by live process (pid {pid}); falling back to ephemeral profile (no cookie/login persistence for this session)"
        );
        return SingletonOutcome::UseEphemeral;
    }

    // Stale lock — clean sentinels then launch with persistent profile (D-04).
    warn!(
        path = %profile_dir.display(),
        pid = pid,
        "SingletonLock is stale (pid {pid} no longer running); cleaning sentinel files"
    );
    cleanup_sentinels(profile_dir);
    SingletonOutcome::UseProfile
}

/// Parse the pid from a SingletonLock symlink target (`<hostname>-<pid>`).
/// The pid is the substring after the LAST `-`. Returns `None` if no `-` is found
/// or the trailing segment does not parse as `u32`.
#[cfg(unix)]
fn parse_singleton_target(target: &str) -> Option<u32> {
    let last_dash = target.rfind('-')?;
    target[last_dash + 1..].parse::<u32>().ok()
}

/// Parse the hostname segment from a SingletonLock symlink target.
/// Returns everything before the LAST `-`. Returns `None` if no `-` found or
/// the dash is at index 0 (no hostname before it).
#[cfg(unix)]
fn parse_hostname_from_target(target: &str) -> Option<&str> {
    let last_dash = target.rfind('-')?;
    if last_dash == 0 {
        return None;
    }
    Some(&target[..last_dash])
}

/// Remove `SingletonLock`, `SingletonSocket`, and `SingletonCookie` from `profile_dir`.
/// Each removal is best-effort (D-04): on non-NotFound errors a `warn!` is emitted
/// but the function never aborts.
#[cfg(unix)]
fn cleanup_sentinels(profile_dir: &Path) {
    for name in &["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let path = profile_dir.join(name);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to remove singleton sentinel (best-effort, continuing)"
                );
            }
        }
    }
}

/// Dep-free process liveness probe (D-02).
/// On Linux: checks `/proc/<pid>` existence (no subprocess, no EPERM issues).
/// On other Unix: shells out to `/bin/kill -0 <pid>` — exit 0 = alive, non-zero = dead.
/// On non-Unix this function is not compiled (the `#[cfg(not(unix))]` path in
/// `reconcile_singleton_lock` returns `UseProfile` without calling this).
#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    // macOS and other Unix: shell out to kill -0.
    // Exit 0 = alive (same-user process). Non-zero = dead (ESRCH) or EPERM (different user).
    // EPERM on macOS /bin/kill returns exit 1, treated as dead — acceptable because
    // Chromium is always spawned by the same user as ironhermes (see RESEARCH.md OQ-1).
    #[cfg(not(target_os = "linux"))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false) // spawn failure → treat as dead (safe conservative choice)
    }
}

/// Read the current machine's hostname via `hostname` command (D-03 / OQ-2).
/// Returns `None` if the command fails or produces empty output; callers fall back
/// to pid-liveness-only in that case.
#[cfg(unix)]
fn current_hostname() -> Option<String> {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    // ----- fixture helpers -----

    /// Returns the current hostname or a fallback string that matches what
    /// `current_hostname()` would return, so fixtures bypass the D-03 host check
    /// and reach the D-02 pid-liveness branch.
    fn host() -> String {
        current_hostname().unwrap_or_else(|| "localhost".to_string())
    }

    fn stale_lock(dir: &TempDir) {
        // i32::MAX as u32 = 2_147_483_647 — far above any real PID on macOS or Linux
        // (macOS max ~99,998; Linux max 4,194,304). Matches gateway/pid.rs precedent.
        // u32::MAX would wrap to -1 as i32 (POSIX "send to all processes"), which
        // returns Ok(()) on macOS even with no real target — a false Live result.
        // Use the real hostname so D-03 host check passes and we reach the D-02 pid check.
        symlink(
            format!("{}-{}", host(), i32::MAX as u32),
            dir.path().join("SingletonLock"),
        )
        .unwrap();
    }

    fn live_lock(dir: &TempDir) {
        // std::process::id() is guaranteed alive for the duration of this test.
        // Use the real hostname so D-03 host check passes and we reach the D-02 pid check.
        symlink(
            format!("{}-{}", host(), std::process::id()),
            dir.path().join("SingletonLock"),
        )
        .unwrap();
    }

    fn unparseable_lock_regular_file(dir: &TempDir) {
        // read_link() on a non-symlink returns Err (EINVAL) on Linux and macOS.
        std::fs::write(dir.path().join("SingletonLock"), b"not-a-symlink").unwrap();
    }

    fn unparseable_lock_bad_target(dir: &TempDir) {
        // "nopid" after rfind('-') does not parse as u32 → unparseable path, no D-03 check.
        // Use a clearly-unparseable target with no valid pid suffix.
        symlink("nodash-nopid", dir.path().join("SingletonLock")).unwrap();
    }

    // ----- SL-01: no lock → UseProfile -----

    #[test]
    fn no_lock_returns_use_profile() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            reconcile_singleton_lock(dir.path()),
            SingletonOutcome::UseProfile
        );
    }

    // ----- SL-02: unparseable regular file → UseProfile, NOT deleted -----

    #[test]
    fn unparseable_regular_file_returns_use_profile_without_deletion() {
        let dir = TempDir::new().unwrap();
        unparseable_lock_regular_file(&dir);
        let outcome = reconcile_singleton_lock(dir.path());
        assert_eq!(outcome, SingletonOutcome::UseProfile);
        assert!(
            dir.path().join("SingletonLock").exists(),
            "unparseable lock (regular file) must NOT be removed (D-01)"
        );
    }

    // ----- SL-03: unparseable symlink target → UseProfile, NOT deleted -----

    #[test]
    fn unparseable_bad_target_returns_use_profile_without_deletion() {
        let dir = TempDir::new().unwrap();
        unparseable_lock_bad_target(&dir);
        let outcome = reconcile_singleton_lock(dir.path());
        assert_eq!(outcome, SingletonOutcome::UseProfile);
        // Use symlink_metadata (lstat) — NOT exists() — because exists() follows the symlink
        // and returns false when the target path does not resolve to a real file.
        // The symlink directory entry must still be present (D-01: no deletion on unparseable).
        assert!(
            std::fs::symlink_metadata(dir.path().join("SingletonLock")).is_ok(),
            "unparseable lock target must NOT be removed (D-01)"
        );
    }

    // ----- SL-04: stale lock → UseProfile + SingletonLock removed -----

    #[test]
    fn stale_lock_returns_use_profile_and_removes_sentinel() {
        let dir = TempDir::new().unwrap();
        stale_lock(&dir);
        let outcome = reconcile_singleton_lock(dir.path());
        assert_eq!(outcome, SingletonOutcome::UseProfile);
        assert!(
            !dir.path().join("SingletonLock").exists(),
            "stale SingletonLock must be removed"
        );
    }

    // ----- SL-05: stale lock → all 3 sentinels removed -----

    #[test]
    fn stale_lock_also_removes_socket_and_cookie() {
        let dir = TempDir::new().unwrap();
        stale_lock(&dir);
        // Pre-create the other two sentinels as symlinks (matching real Chromium behaviour).
        symlink("/tmp/fake-socket", dir.path().join("SingletonSocket")).unwrap();
        symlink("fake-cookie-value", dir.path().join("SingletonCookie")).unwrap();
        let outcome = reconcile_singleton_lock(dir.path());
        assert_eq!(outcome, SingletonOutcome::UseProfile);
        assert!(!dir.path().join("SingletonLock").exists());
        assert!(!dir.path().join("SingletonSocket").exists());
        assert!(!dir.path().join("SingletonCookie").exists());
    }

    // ----- SL-06: live lock → UseEphemeral + SingletonLock preserved -----

    #[test]
    fn live_lock_returns_use_ephemeral_and_preserves_sentinel() {
        let dir = TempDir::new().unwrap();
        live_lock(&dir);
        let outcome = reconcile_singleton_lock(dir.path());
        assert_eq!(outcome, SingletonOutcome::UseEphemeral);
        // Use symlink_metadata (lstat) — NOT exists() — because exists() follows the symlink
        // and returns false when the target path (a relative "<hostname>-<pid>" string) does
        // not resolve to a real file. The symlink itself must still be present (D-05).
        assert!(
            std::fs::symlink_metadata(dir.path().join("SingletonLock")).is_ok(),
            "live SingletonLock must NOT be removed (D-05)"
        );
    }

    // ----- SL-07: parse_singleton_target handles hostnames with embedded dashes -----

    #[test]
    fn parse_singleton_target_handles_hostname_with_dashes() {
        // Real production value: "Brads-MacBook-Pro.local-20704"
        assert_eq!(
            parse_singleton_target("Brads-MacBook-Pro.local-20704"),
            Some(20704)
        );
        assert_eq!(parse_singleton_target("simple-12345"), Some(12345));
        assert_eq!(parse_singleton_target("no-digits-here"), None);
        assert_eq!(parse_singleton_target("nodash"), None);
        assert_eq!(parse_singleton_target("-99"), Some(99));
    }

    // ----- Extra: parse_hostname_from_target handles dashes in hostname -----

    #[test]
    fn parse_hostname_from_target_handles_dashes() {
        assert_eq!(
            parse_hostname_from_target("Brads-MacBook-Pro.local-20704"),
            Some("Brads-MacBook-Pro.local")
        );
        assert_eq!(
            parse_hostname_from_target("simple-12345"),
            Some("simple")
        );
        assert_eq!(parse_hostname_from_target("nodash"), None);
        // Dash at index 0 → no hostname segment
        assert_eq!(parse_hostname_from_target("-99"), None);
    }
}
