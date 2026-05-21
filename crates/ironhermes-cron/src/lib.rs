use anyhow::{Context, Result};
use ironhermes_core::get_hermes_home;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use tracing::debug;

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

pub mod adapter;
pub mod delivery;
pub mod display;
pub mod job;
pub mod parser;
pub mod scanner;
pub mod store;
pub mod tick;

pub use adapter::TgSendApi;
pub use delivery::*;
pub use job::*;
pub use parser::*;
pub use scanner::scan_cron_prompt;
pub use store::*;
pub use tick::*;

/// Process-global serialization lock for tests that mutate shared env vars
/// (`IRONHERMES_HOME`, `*_HOME_CHANNEL`, ...). Every env-mutating test across all
/// modules MUST hold this single lock — independent per-module mutexes do NOT
/// serialize against each other, so concurrent tests in different modules
/// otherwise stomp each other's env state and produce flaky cross-module failures.
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// File-based tick lock
// ---------------------------------------------------------------------------

/// An RAII guard that removes the lock file on drop.
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        debug!("Released tick lock: {}", self.path.display());
    }
}

/// Try to acquire an exclusive tick lock using a `.tick.lock` file.
///
/// Returns `Ok(Some(guard))` if the lock was acquired, or `Ok(None)` if
/// another process already holds it (caller should skip this tick).
pub fn acquire_tick_lock() -> Result<Option<LockGuard>> {
    acquire_tick_lock_at(get_hermes_home().join("cron"))
}

/// Like [`acquire_tick_lock`] but at a caller-specified directory.
pub fn acquire_tick_lock_at(dir: PathBuf) -> Result<Option<LockGuard>> {
    let lock_path = dir.join(".tick.lock");

    // Ensure the directory exists.
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("failed to create cron dir: {}", dir.display()))?;
    }

    // Use O_CREAT | O_EXCL for an atomic create-or-fail.
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut f) => {
            // Write our PID so operators can inspect the lock.
            let _ = write!(f, "{}", std::process::id());
            debug!("Acquired tick lock: {}", lock_path.display());
            Ok(Some(LockGuard { path: lock_path }))
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            // Check if the lock holder is still alive; if not, remove the
            // stale lock and retry once.
            if let Some(guard) = try_recover_stale_lock(&lock_path)? {
                return Ok(Some(guard));
            }
            debug!("Tick lock already held: {}", lock_path.display());
            Ok(None)
        }
        Err(e) => {
            Err(e).with_context(|| format!("failed to acquire tick lock: {}", lock_path.display()))
        }
    }
}

// ---------------------------------------------------------------------------
// Stale lock recovery
// ---------------------------------------------------------------------------

/// Check whether the PID recorded in a lock file is still alive.
/// If the process is dead, remove the stale lock and attempt to re-acquire.
/// Returns `Some(guard)` on successful recovery, `None` if the holder is
/// still alive or the lock file cannot be parsed.
fn try_recover_stale_lock(lock_path: &std::path::Path) -> Result<Option<LockGuard>> {
    let pid_str = match fs::read_to_string(lock_path) {
        Ok(s) => s,
        Err(_) => return Ok(None), // can't read — assume held
    };

    let pid: u32 = match pid_str.trim().parse() {
        Ok(p) => p,
        Err(_) => return Ok(None), // can't parse — assume held
    };

    if is_process_alive(pid) {
        return Ok(None);
    }

    debug!(
        "Removing stale tick lock (PID {} is dead): {}",
        pid,
        lock_path.display()
    );
    let _ = fs::remove_file(lock_path);

    // Retry acquisition once
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(mut f) => {
            let _ = io::Write::write_fmt(&mut f, format_args!("{}", std::process::id()));
            debug!(
                "Re-acquired tick lock after stale recovery: {}",
                lock_path.display()
            );
            Ok(Some(LockGuard {
                path: lock_path.to_path_buf(),
            }))
        }
        Err(_) => Ok(None), // someone else grabbed it between remove and retry
    }
}

/// Platform-appropriate process liveness check.
#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    // kill(pid, 0) checks existence without sending a signal.
    // Returns 0 if the process exists and we have permission to signal it.
    // Returns -1 with ESRCH if the process does not exist.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    // On non-Unix platforms, assume the process is alive (conservative).
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp_cron_dir() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        (dir, cron_dir)
    }

    #[test]
    fn test_tick_lock() {
        let (_dir, cron_dir) = tmp_cron_dir();
        let g1 = acquire_tick_lock_at(cron_dir.clone()).expect("lock1");
        assert!(g1.is_some());

        // A second attempt should fail to acquire.
        let g2 = acquire_tick_lock_at(cron_dir.clone()).expect("lock2");
        assert!(g2.is_none());

        // After dropping the first guard the lock file is gone.
        drop(g1);
        let g3 = acquire_tick_lock_at(cron_dir).expect("lock3");
        assert!(g3.is_some());
    }
}
