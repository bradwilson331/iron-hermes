//! Phase 36.17.7 D-02-d: audio cache lifecycle / GC.
//!
//! Two entry points:
//!   - `gc_sweep_audio_cache(dir, max_age_days)` — sync sweep. Called from
//!     `AppState::init` on startup (before async runtime work) and from the
//!     periodic loop below. Removes files in `dir` whose mtime is older than
//!     `max_age_days`. NotFound on the dir is a no-op; per-file remove errors
//!     are logged via `tracing::warn!` and skipped (never panic).
//!   - `run_audio_cache_gc_loop(dir, max_age_days, sweep_interval_secs, cancel)`
//!     — async periodic loop. `tokio::select!` between a `sleep(interval)` and
//!     `cancel.cancelled()`. On cancel, logs `info!` and returns.
//!
//! Mirrors `kanban_ws.rs::run_kanban_tail_loop` for the cancel-token shape.
//! Uses plain `tokio::time::sleep` (not `tokio::time::interval`) and a plain
//! `CancellationToken` (no broadcast channel — there are no subscribers).

#![cfg(feature = "server")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;

/// Phase 36.17.7 D-02-d: sync sweep over `dir`. Removes files with
/// `mtime < now - max_age_days * 86400 s`. Errors logged via
/// `tracing::warn!`; never panics.
pub fn gc_sweep_audio_cache(dir: &Path, max_age_days: u32) {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(max_age_days as u64 * 86400))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Cache dir hasn't been created yet — no-op.
            return;
        }
        Err(e) => {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "audio_cache GC: read_dir failed; skipping sweep"
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "audio_cache GC: metadata read failed; skipping"
                );
                continue;
            }
        };
        if !meta.is_file() {
            continue;
        }
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "audio_cache GC: modified() read failed; skipping"
                );
                continue;
            }
        };
        if mtime < cutoff {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "audio_cache GC: remove_file failed; continuing"
                );
            }
        }
    }
}

/// Phase 36.17.7 D-02-d: async periodic sweep loop. Exits cleanly when
/// `cancel.cancelled()` fires.
pub async fn run_audio_cache_gc_loop(
    dir: PathBuf,
    max_age_days: u32,
    sweep_interval_secs: u64,
    cancel: CancellationToken,
) {
    let interval = Duration::from_secs(sweep_interval_secs);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                gc_sweep_audio_cache(&dir, max_age_days);
            }
            _ = cancel.cancelled() => {
                tracing::info!("audio_cache GC loop cancelled; exiting");
                break;
            }
        }
    }
}
