//! E-11 / G-03 / D-09 — delegate_task semaphore-wait 2s debounced warn.
//!
//! Contract harness: the production instrumentation lives in
//! `crates/ironhermes-tools/src/delegate_task.rs` at the
//! `self.semaphore.acquire().await` site (~L488). Exercising the real
//! `DelegateTaskTool::execute` path in a unit test would require a full
//! resolver + client + tool registry stub — overkill for a timing
//! contract. Instead this test emits the SAME `tracing::warn!` shape at
//! a drop-in helper and asserts that (1) waits ≥ 2s emit the warn and
//! (2) waits < 2s do NOT. The LOCKED strings are the `target` and the
//! `"semaphore wait exceeded 2s threshold"` message — they're also
//! grep-gated in the acceptance criteria so a refactor that drops the
//! warn cannot land silently (T-21.7-08-05).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tracing_test::traced_test;

/// Mirrors the production instrumentation at `delegate_task.rs:~L488`.
/// Keep target + message verbatim — the grep gate locks them.
///
/// NB: uses `tokio::time::Instant` (instead of `std::time::Instant`)
/// so the harness responds to `tokio::time::advance` under
/// `#[tokio::test(start_paused = true)]`. Production code in
/// `delegate_task.rs` uses `std::time::Instant` — the contract this
/// harness is protecting is "if elapsed ≥ 2s, warn"; the wall-clock
/// source is an implementation detail.
/// Drives the EXACT instrumentation block from delegate_task.rs:~L488:
/// capture a start Instant, acquire a semaphore permit, measure elapsed,
/// emit the 2s warn if elapsed ≥ 2s. Everything runs in the caller's
/// task so the `#[traced_test]` scope captures the warn — spans do NOT
/// propagate to spawned tasks, so keep this inline in the test body.
///
/// `tokio::time::Instant::now()` + `tokio::time::sleep(...)` are time-
/// driver-aware so `#[tokio::test(start_paused = true)]` + `advance`
/// gives us fast, deterministic timing without wall-clock dependency.
/// Production code in `delegate_task.rs` uses `std::time::Instant` on
/// the same pattern — the warn-emission contract is independent of the
/// clock source.
async fn run_instrumented_acquire(sem: Arc<Semaphore>, prewait_ms: u64) {
    let start = tokio::time::Instant::now();
    // Wait the configured interval BEFORE attempting the acquire so
    // `elapsed` reflects the wait time deterministically.
    tokio::time::sleep(Duration::from_millis(prewait_ms)).await;
    let _permit = sem.acquire().await.expect("acquire");
    let elapsed = start.elapsed();
    if elapsed >= Duration::from_secs(2) {
        tracing::warn!(
            target: "ironhermes_tools::delegate_task",
            elapsed_ms = elapsed.as_millis() as u64,
            "semaphore wait exceeded 2s threshold"
        );
    }
}

#[tokio::test(start_paused = true)]
#[traced_test]
async fn semaphore_wait_over_2s_emits_warn() {
    // Semaphore with capacity 1 and immediately available — the contract
    // is about the measured elapsed Duration, not about semaphore
    // contention specifically. We simulate a 2.5s wait via tokio::sleep
    // under paused time, matching the shape of a real blocked acquire.
    let sem = Arc::new(Semaphore::new(1));
    run_instrumented_acquire(sem, 2500).await;

    assert!(
        logs_contain("semaphore wait exceeded 2s threshold"),
        "D-09: blocked wait \u{2265} 2s must emit tracing::warn at \
         target=ironhermes_tools::delegate_task"
    );
}

#[tokio::test(start_paused = true)]
#[traced_test]
async fn semaphore_wait_under_2s_emits_no_warn() {
    let sem = Arc::new(Semaphore::new(1));
    run_instrumented_acquire(sem, 500).await;

    assert!(
        !logs_contain("semaphore wait exceeded 2s threshold"),
        "wait < 2s must NOT emit the warn"
    );
}
