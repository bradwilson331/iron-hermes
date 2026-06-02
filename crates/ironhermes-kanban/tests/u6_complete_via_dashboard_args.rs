//! U6 carve-out repro — Phase 36.3.7.11 UAT discrepancy.
//!
//! During the 2026-06-02 UAT pass, the tester observed that clicking
//! "Complete" in the Kanban dashboard's modal transitioned a card to
//! READY instead of DONE. The UAT script expected DONE. Code reading
//! showed the dashboard calls `KanbanStore::complete_task` with the
//! exact arg vector:
//!
//!     (Some(summary), metadata, None, None, None, "ui")
//!
//! (see `crates/iron_hermes_ui/src/server/kanban_api.rs:531-540`)
//!
//! and `complete_task` ends with the unconditional
//! `UPDATE tasks SET status='done', ended_at=?1 WHERE id=?2`
//! at `crates/ironhermes-kanban/src/store.rs:1866-1870`.
//!
//! This test pins that contract: with the exact arg vector the dashboard
//! uses (including `expected_run_id: None` — the CAS gate is skipped
//! because the gate fires only `if let Some(eid)`), the post-call status
//! is `"done"`, not `"ready"`. If this test ever flips, the bug is in
//! the store layer. If it stays green, U6's READY observation has to
//! live in the dashboard's display layer (column-binning,
//! refresh-on-success, or tester misobservation).

use ironhermes_kanban::{CreateTaskOptions, KanbanStore};
use tempfile::TempDir;

/// Pin the store-level contract that the dashboard's Complete modal
/// depends on: after `complete_task` with the dashboard's exact arg
/// vector, the task row's status is `"done"`.
#[test]
fn complete_task_with_dashboard_args_lands_in_done_status() {
    let dir = TempDir::new().expect("tempdir");
    let mut store = KanbanStore::open(dir.path().join("kanban.db")).expect("open store");

    // Default-status create (no parents, no `triage` flag) lands in READY
    // per store.rs:424-431. This matches the UAT's seed card
    // `cargo run -p ironhermes-cli -- kanban create --assignee dev "UAT ready card"`.
    let task = store
        .create_task("U6 repro card", "dev", CreateTaskOptions::default())
        .expect("create_task");
    assert_eq!(
        task.status, "ready",
        "default-status create must land in READY (store.rs:424-431)"
    );

    // EXACT arg vector the dashboard passes — kanban_api.rs:531-540.
    // task_id, summary, metadata, result, expected_run_id, created_cards, current_profile
    //                                     ^^^^^^^^^^^^^^^^ None → CAS gate skipped (cas.rs:155-175)
    store
        .complete_task(
            &task.id,
            Some("U6 repro summary"),
            None,
            None,
            None,
            None,
            "ui",
        )
        .expect("complete_task with dashboard args must succeed");

    let after = store.get_task(&task.id).expect("get_task after complete");
    assert_eq!(
        after.status, "done",
        "U6 carve-out: dashboard Complete-via-modal flow must leave task in `done`, not `ready`. \
         If this assertion fails, the bug is in the store layer (UPDATE statement at store.rs:1866-1870). \
         If it passes (expected), U6's READY observation is a UI rendering issue, not a store defect."
    );
    assert!(
        after.ended_at.is_some(),
        "complete_task must set ended_at (store.rs:1868)"
    );
}

/// Pin the protective branch too: confirm that calling complete_task
/// with `expected_run_id: Some(...)` against a task whose current_run_id
/// is None DOES reject (StaleRunId), so the dashboard's choice to pass
/// `None` is the correct one — passing some random value would brick
/// the modal.
#[test]
fn complete_task_with_expected_run_id_against_uninitialized_run_rejects() {
    let dir = TempDir::new().expect("tempdir");
    let mut store = KanbanStore::open(dir.path().join("kanban.db")).expect("open store");

    let task = store
        .create_task("U6 CAS guard", "dev", CreateTaskOptions::default())
        .expect("create_task");

    // A freshly-created task has current_run_id = NULL. Passing any
    // non-empty expected_run_id MUST trip the StaleRunId guard.
    let result = store.complete_task(
        &task.id,
        Some("would-be summary"),
        None,
        None,
        Some("synthetic-run-id-that-does-not-match"),
        None,
        "ui",
    );

    assert!(
        result.is_err(),
        "complete_task with expected_run_id != actual MUST reject \
         (this is exactly why kanban_api.rs:536 passes None — \
         flipping it to Some would brick every dashboard Complete write)"
    );
}
