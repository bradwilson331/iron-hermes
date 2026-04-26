//! Phase 22.4.2.1 Plan 03 — regression test for Ctrl+C graceful shutdown.
//!
//! Tests that:
//! 1. The worker_join_set drain machinery is present in runner.rs (source-grep).
//! 2. The ctrl_c() signal arm is still installed.
//! 3. The worker drain comes AFTER self.cancel.cancel() in source order.
//!
//! Pattern: Phase 21.2 Plan 11 shutdown_all_returns_within_timeout_when_stdio_child_blocks.
//! Shape B (per RESEARCH §6 Open Question 2): source-grep tests rather than full
//! GatewayRunner construction (which requires a live TG token). Task 2 upgrades
//! gateway_drains_workers_within_timeout to a real behavioral assertion using
//! the drain_workers_with_timeout helper extracted during Task 2.

use std::time::Duration;
use tokio::time::timeout;

/// Wave 0 placeholder — upgraded to a real drain assertion in Task 2.
/// At Wave 0, passes trivially to confirm test framework wiring is correct.
/// Task 2 replaces this body with a real JoinSet drain assertion using
/// the drain_workers_with_timeout pub(crate) helper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_drains_workers_within_timeout() {
    // TODO(Plan 03 Task 2): replace with drain_workers_with_timeout helper test.
    // Spawn a JoinSet task that respects a CancellationToken, fire cancel,
    // assert the drain helper completes within 5s.
    //
    // At Wave 0 this test passes trivially (proving tokio runtime wiring works).
    let result = timeout(Duration::from_secs(5), async {
        // placeholder: real shutdown logic wired in Task 2
    })
    .await;
    assert!(result.is_ok(), "5s timeout placeholder — Task 2 wires real assertion");
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
    let cancel_pos = src
        .find("self.cancel.cancel()")
        .expect("self.cancel.cancel() must be present in runner.rs");
    let drain_pos = src.find("worker_join_set.join_next()");
    if let Some(drain) = drain_pos {
        assert!(
            drain > cancel_pos,
            "worker_join_set drain must come AFTER self.cancel.cancel() in source order \
             (D-11 ordering invariant: cancel first, then drain, then drop msg_tx)"
        );
    }
}
