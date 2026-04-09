use anyhow::{Context, Result};
use ironhermes_core::get_hermes_home;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use tracing::debug;

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

pub mod delivery;
pub mod job;
pub mod parser;
pub mod scanner;
pub mod store;
pub mod tick;

pub use delivery::*;
pub use job::*;
pub use parser::*;
pub use scanner::scan_cron_prompt;
pub use store::*;
pub use tick::*;

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
            debug!("Tick lock already held: {}", lock_path.display());
            Ok(None)
        }
        Err(e) => Err(e).with_context(|| {
            format!("failed to acquire tick lock: {}", lock_path.display())
        }),
    }
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
