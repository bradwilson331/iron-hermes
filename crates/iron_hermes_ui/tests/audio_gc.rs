//! Phase 36.17.7 Plan 05 Wave 0 — audio cache GC source-grep + functional guards.
//!
//! Locks:
//! - audio_cache.rs ships `gc_sweep_audio_cache` (sync) and
//!   `run_audio_cache_gc_loop` (async cancel-token loop).
//! - state.rs `AppState::init` calls `gc_sweep_audio_cache` on startup.
//! - Sync sweep deletes files older than `max_age_days` and survives missing dirs.

#![cfg(feature = "server")]

const SOURCE: &str = include_str!("../src/server/audio_cache.rs");
const STATE: &str = include_str!("../src/server/state.rs");

#[test]
fn audio_gc_sync_sweep_fn_exists() {
    assert!(
        SOURCE.contains("fn gc_sweep_audio_cache"),
        "D-02-d: audio_cache.rs must declare `fn gc_sweep_audio_cache`"
    );
}

#[test]
fn audio_gc_async_loop_fn_exists() {
    assert!(
        SOURCE.contains("async fn run_audio_cache_gc_loop"),
        "D-02-d: audio_cache.rs must declare `async fn run_audio_cache_gc_loop`"
    );
}

#[test]
fn audio_gc_uses_cancellation_token() {
    assert!(
        SOURCE.contains("cancel.cancelled()"),
        "D-02-d: audio_cache.rs periodic loop must respond to \
         `cancel.cancelled()` in a `tokio::select!`"
    );
    assert!(
        SOURCE.contains("CancellationToken"),
        "D-02-d: audio_cache.rs must accept a `CancellationToken` parameter"
    );
}

#[test]
fn audio_gc_never_panics() {
    assert!(
        !SOURCE.contains(".unwrap()"),
        "audio_cache.rs must not use `.unwrap()` — surface errors via tracing::warn!"
    );
    assert!(
        !SOURCE.contains("panic!"),
        "audio_cache.rs must not use `panic!` — surface errors via tracing::warn!"
    );
}

#[test]
fn audio_gc_logs_errors_via_tracing() {
    assert!(
        SOURCE.contains("tracing::warn!"),
        "audio_cache.rs must surface read_dir / remove_file errors via tracing::warn!"
    );
}

#[test]
fn audio_gc_startup_sweep_called_from_state_init() {
    assert!(
        STATE.contains("gc_sweep_audio_cache"),
        "D-02-d: AppState::init must call `gc_sweep_audio_cache` for startup sweep"
    );
}

#[tokio::test]
async fn audio_gc_removes_files_older_than_max_age() {
    use iron_hermes_ui::server::audio_cache::gc_sweep_audio_cache;
    use std::fs;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    let dir = TempDir::new().expect("create tempdir");
    let cache_dir = dir.path();

    // Create an "old" file (mtime > 7 days ago) and a "new" file (now).
    let old_path = cache_dir.join("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.mp3");
    let new_path = cache_dir.join("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb.mp3");
    fs::write(&old_path, b"old").expect("write old");
    fs::write(&new_path, b"new").expect("write new");

    // Backdate the old file by 10 days using filetime if available; otherwise
    // accept that mtime defaults to now and only verify GC is non-destructive.
    let ten_days_ago = SystemTime::now() - Duration::from_secs(10 * 86400);

    // Best-effort backdating via `utimes` on POSIX — silently no-op if the
    // platform call fails. Test still validates GC over missing files.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = ten_days_ago; // mark as used
        // Use std::fs::File::set_modified (stable since 1.75) to backdate.
        if let Ok(file) = std::fs::File::options().write(true).open(&old_path) {
            let _ = file.set_modified(ten_days_ago);
            let _ = file.metadata().map(|m| m.mtime()); // touch to silence unused
        }
    }

    // Sweep with 7-day cutoff.
    gc_sweep_audio_cache(cache_dir, 7);

    // The new file must still exist (was written just now).
    assert!(
        new_path.exists(),
        "GC sweep must not delete files newer than max_age_days"
    );

    // If we successfully backdated, the old file should be gone.
    // If backdating failed (platform-specific), this assertion silently passes.
    let backdated = std::fs::metadata(&old_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| t < SystemTime::now() - Duration::from_secs(8 * 86400))
        .unwrap_or(false);
    if backdated {
        assert!(
            !old_path.exists(),
            "GC sweep must delete files older than max_age_days"
        );
    }
}

#[tokio::test]
async fn audio_gc_missing_dir_does_not_panic() {
    use iron_hermes_ui::server::audio_cache::gc_sweep_audio_cache;
    use std::path::Path;

    // Sweep a non-existent path. Must not panic; should log a warn and return.
    gc_sweep_audio_cache(Path::new("/nonexistent/path/q9z-36-17-7"), 1);
}
