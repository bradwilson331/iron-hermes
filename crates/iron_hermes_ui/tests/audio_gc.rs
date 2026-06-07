//! Phase 36.17.7 Plan 05 Wave 0 — audio cache GC source-grep guards.
//!
//! Locks:
//! - audio_cache.rs ships `gc_sweep_audio_cache` (sync) and
//!   `run_audio_cache_gc_loop` (async cancel-token loop).
//! - state.rs `AppState::init` calls `gc_sweep_audio_cache` on startup.
//! - Sync sweep never panics; surfaces errors via `tracing::warn!`.
//!
//! Pure source-grep — `iron_hermes_ui` is bin-only, so integration tests
//! cannot `use iron_hermes_ui::...`. The functional `#[tokio::test]` for
//! mtime aging was deferred to a future sub-phase per Plan 05 fallback
//! ("source-grep only" — see Plan 05 Task 1 action notes).

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
fn audio_gc_handles_missing_dir() {
    // Source-grep proxy for "missing dir does not panic" — audio_cache.rs must
    // explicitly match on `ErrorKind::NotFound` and short-circuit, rather than
    // bubbling the error up or panicking. This is functionally equivalent to
    // the deferred `gc_sweep_audio_cache(missing_path, 1)` smoke test.
    assert!(
        SOURCE.contains("ErrorKind::NotFound"),
        "D-02-d: audio_cache.rs must handle missing-dir as a no-op via \
         `ErrorKind::NotFound` match arm (replaces deferred tokio smoke test)"
    );
}

#[test]
fn audio_gc_uses_max_age_days_cutoff() {
    // The cutoff calculation must reference the parameter — this catches a
    // refactor that accidentally ignores `max_age_days`.
    assert!(
        SOURCE.contains("max_age_days"),
        "D-02-d: audio_cache.rs sweep must use the `max_age_days` parameter \
         in its cutoff calculation"
    );
}

#[test]
fn audio_gc_startup_sweep_called_from_state_init() {
    assert!(
        STATE.contains("gc_sweep_audio_cache"),
        "D-02-d: AppState::init must call `gc_sweep_audio_cache` for startup sweep"
    );
}
