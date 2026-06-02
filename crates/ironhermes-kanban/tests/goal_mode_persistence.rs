//! Phase 36.3.7.12 Plan 01 Task 2 — bilateral persistence test for goal-mode
//! producer surface.
//!
//! Validates that `CreateTaskOptions` carrying goal_mode + goal_max_turns
//! round-trips through `KanbanStore::create_task` + `get_task` per CONTEXT.md
//! D-01 and the D-03 producer-level 0 → 20 coercion contract.

use std::path::PathBuf;
use tempfile::TempDir;

use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};

fn open_store(dir: &TempDir) -> KanbanStore {
    let path: PathBuf = dir.path().join("kanban.db");
    KanbanStore::new(&path).expect("open store")
}

/// CreateTaskOptions{ goal_mode=true, goal_max_turns=5 } persists and is
/// hydrated by get_task with goal_turns_used at its column default (0).
#[test]
fn goal_mode_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let opts = CreateTaskOptions {
        goal_mode: true,
        goal_max_turns: 5,
        ..Default::default()
    };
    let created = store
        .create_task("port docs to French", "alice", opts)
        .expect("create_task");

    assert!(created.goal_mode, "created task must carry goal_mode=true");
    assert_eq!(
        created.goal_max_turns, 5,
        "created task must carry the requested goal_max_turns=5"
    );
    assert_eq!(
        created.goal_turns_used, 0,
        "freshly created task must start with goal_turns_used=0"
    );

    let fetched = store.get_task(&created.id).expect("get_task");
    assert!(fetched.goal_mode, "hydrated task must carry goal_mode=true");
    assert_eq!(fetched.goal_max_turns, 5);
    assert_eq!(fetched.goal_turns_used, 0);
}

/// D-03 producer-level default: when caller leaves goal_max_turns at 0
/// (the `#[derive(Default)]` zero value), create_task must coerce to 20.
#[test]
fn goal_max_turns_zero_coerces_to_twenty() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let opts = CreateTaskOptions {
        goal_mode: true,
        goal_max_turns: 0, // caller did not override
        ..Default::default()
    };
    let created = store
        .create_task("evaluate goal", "alice", opts)
        .expect("create_task");

    assert!(created.goal_mode);
    assert_eq!(
        created.goal_max_turns, 20,
        "0 must be coerced to D-03 default of 20 by the producer"
    );

    // Confirm the persisted value (re-fetch) also reflects the coercion.
    let fetched = store.get_task(&created.id).expect("get_task");
    assert_eq!(fetched.goal_max_turns, 20);
}

/// Default options (goal_mode=false, goal_max_turns=0): even though the caller
/// did not opt in, the row must have a sane goal_max_turns of 20 (the column
/// DDL DEFAULT) so a later flip to goal_mode=true does not surface a 0 budget.
#[test]
fn default_options_have_goal_mode_false_and_default_budget() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let created = store
        .create_task("plain task", "alice", CreateTaskOptions::default())
        .expect("create_task");

    assert!(
        !created.goal_mode,
        "default opts must produce goal_mode=false"
    );
    assert_eq!(
        created.goal_max_turns, 20,
        "default opts must land at goal_max_turns=20 (DDL DEFAULT honored)"
    );
    assert_eq!(created.goal_turns_used, 0);
}
