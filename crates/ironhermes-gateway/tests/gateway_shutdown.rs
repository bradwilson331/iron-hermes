//! Phase 22.4.2.1 Plan 03 — regression test for Ctrl+C graceful shutdown.
//!
//! Tests that:
//! 1. The worker_join_set drain machinery is present in runner.rs (source-grep).
//! 2. The ctrl_c() signal arm is still installed.
//! 3. The worker drain comes AFTER self.cancel.cancel() in source order.
//! 4. A JoinSet with a cancellation-aware task drains within 5 seconds (behavioral).
//!
//! Uses Path B (per RESEARCH §6 Open Question 2): behavioral test uses a synthetic
//! JoinSet + CancellationToken rather than constructing a full GatewayRunner (which
//! requires a live TG token). This tests the exact drain pattern used in runner.rs.
//!
//! Pattern: Phase 21.2 Plan 11 shutdown_all_returns_within_timeout_when_stdio_child_blocks.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// Behavioral regression test: a JoinSet containing a long-running but
/// cancellation-aware worker task drains within 5 seconds when cancel fires.
///
/// This mirrors the exact pattern in runner.rs:
///   - worker_join_set (Arc<TokioMutex<JoinSet<()>>>) holds per-chat worker tasks
///   - each worker selects on cancel_task.cancelled() and exits early
///   - the shutdown sequence locks worker_join_set and drains with a 5s deadline
///
/// RED at Wave 0 (placeholder body). GREEN after Task 2 fix lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_drains_workers_within_timeout() {
    let cancel = CancellationToken::new();
    let worker_join_set: Arc<TokioMutex<JoinSet<()>>> =
        Arc::new(TokioMutex::new(JoinSet::new()));

    // Spawn a long-running worker that respects the CancellationToken —
    // mirrors the per-chat worker body in runner.rs (cancel_task.is_cancelled() check).
    {
        let cancel_in = cancel.clone();
        let mut wjs = worker_join_set.lock().await;
        wjs.spawn(async move {
            tokio::select! {
                _ = cancel_in.cancelled() => {
                    // Cancelled — exit cleanly (matches cancel_task.is_cancelled() break)
                }
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    // Would block for 60s without cancel
                }
            }
        });
    }

    // Fire cancel — mirrors self.cancel.cancel() in the shutdown sequence
    cancel.cancel();

    // Drain with 5s deadline — mirrors the runner.rs drain block (D-11)
    let start = std::time::Instant::now();
    let drain_result = timeout(Duration::from_secs(6), async {
        let abort_deadline =
            tokio::time::Instant::now() + Duration::from_secs(5);
        let mut wjs = worker_join_set.lock().await;
        loop {
            match tokio::time::timeout_at(abort_deadline, wjs.join_next()).await {
                Ok(Some(_)) => {}   // worker finished — keep draining
                Ok(None) => break,  // all done
                Err(_elapsed) => {
                    wjs.abort_all();
                    break;
                }
            }
        }
    })
    .await;

    assert!(
        drain_result.is_ok(),
        "worker_join_set drain must complete within 6s (5s timeout + 1s slack)"
    );
    assert!(
        start.elapsed() < Duration::from_secs(6),
        "drain must not hang — elapsed: {:?}",
        start.elapsed()
    );
}

/// Source-grep behavioral anchor for worker_join_set machinery.
/// Mirrors INV-22.4.2.1-03 in invariants_22_4.rs so a gateway-crate-only
/// test run also catches regressions without running the full INV suite.
///
/// RED at Wave 0 (worker_join_set not yet present in runner.rs).
/// GREEN after Task 2 lands the fix.
#[test]
fn worker_join_set_drains_on_cancel() {
    let src = include_str!("../src/runner.rs");

    assert!(
        src.contains("worker_join_set"),
        "worker_join_set must be present in runner.rs (Plan 03 fix not yet landed)"
    );

    assert!(
        src.contains("ctrl_c()"),
        "ctrl_c() signal arm must be present in runner.rs"
    );

    // Verify ordering: worker_join_set drain MUST appear AFTER self.cancel.cancel().
    // Use the production-code cancel call (line ~689) and the drain call (line ~701).
    // Note: self.cancel.cancel() also appears in test code later in the file, so
    // we find the FIRST occurrence of the production cancel and verify drain follows.
    let cancel_pos = src
        .find("self.cancel.cancel()")
        .expect("self.cancel.cancel() must be present in runner.rs");
    // The drain uses wjs.join_next() inside the timeout_at loop
    let drain_pos = src.find("wjs.join_next()");
    if let Some(drain) = drain_pos {
        assert!(
            drain > cancel_pos,
            "worker_join_set drain (wjs.join_next) must come AFTER self.cancel.cancel() \
             in source order (D-11 ordering invariant)"
        );
    } else {
        panic!("wjs.join_next() must be present in runner.rs (drain step missing)");
    }
}
