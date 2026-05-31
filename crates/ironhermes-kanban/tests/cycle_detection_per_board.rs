//! D-05 cycle-detection no-code-change invariant tests (Phase 36.3.7.9 Plan 09).
//!
//! After the multi-board changes (Plans 01-08), cycle detection must still work
//! correctly because cross-board links are forbidden by construction: each board's
//! SQLite DB has its own `tasks` and `task_links` tables — `WITH RECURSIVE`
//! ancestor walks only see same-board task IDs.
//!
//! Three tests:
//!
//! 1. `cycle_detection_works_unchanged_on_default_board`
//!    Re-locks the Phase 36.3.7.6 A→B→C+C→A invariant on the default board.
//!
//! 2. `cycle_detection_works_independently_on_named_board`
//!    Same A→B→C+C→A scenario seeded entirely on the "alpha" named board.
//!    Proves the ancestor-walk SQL runs per-board correctly.
//!
//! 3. `cross_board_link_is_blocked_by_construction_not_by_cycle_sql`
//!    Creates task X on default and task Y on alpha, then attempts
//!    `insert_link_checked(X, Y)` against the alpha connection.
//!    Asserts `TaskNotFound` rejection — NOT `LinkCycle` — demonstrating that
//!    cross-board links fail at the lookup layer (D-05 rationale: cycle SQL never
//!    sees cross-board IDs because they don't exist in the connected DB).

use std::sync::Mutex;

use ironhermes_kanban::error::KanbanError;
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};

/// Serialize all tests that mutate `IRONHERMES_HOME`.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Seed a simple task in `store` and return its id.
fn seed_task(store: &mut KanbanStore, title: &str) -> String {
    store
        .create_task(title, "test-agent", CreateTaskOptions::default())
        .unwrap_or_else(|e| panic!("create_task({title}) failed: {e}"))
        .id
}

// ---------------------------------------------------------------------------
// Test 1: default board cycle detection unchanged (D-05 re-lock of Phase 36.3.7.6)
// ---------------------------------------------------------------------------

#[test]
fn cycle_detection_works_unchanged_on_default_board() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

    let mut store = KanbanStore::open_default().expect("open_default");

    // Seed A → B → C chain.
    let a = seed_task(&mut store, "A");
    let b = seed_task(&mut store, "B");
    let c = seed_task(&mut store, "C");

    // A → B
    store
        .insert_link_checked(&a, &b)
        .expect("A→B should succeed");

    // B → C
    store
        .insert_link_checked(&b, &c)
        .expect("B→C should succeed");

    // C → A would close the cycle — must be rejected with LinkCycle.
    let result = store.insert_link_checked(&c, &a);
    match result {
        Err(KanbanError::LinkCycle { parent_id, child_id }) => {
            assert_eq!(parent_id, c, "parent_id in LinkCycle must be C");
            assert_eq!(child_id, a, "child_id in LinkCycle must be A");
        }
        Err(e) => panic!("expected LinkCycle but got: {e}"),
        Ok(()) => panic!("C→A should have been rejected as a cycle"),
    }

    unsafe { std::env::remove_var("IRONHERMES_HOME") };
}

// ---------------------------------------------------------------------------
// Test 2: named board cycle detection independent of default board
// ---------------------------------------------------------------------------

#[test]
fn cycle_detection_works_independently_on_named_board() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

    let mut store = KanbanStore::open_for_board("alpha").expect("open_for_board('alpha')");

    // Seed the same A→B→C chain, this time entirely on the alpha board.
    let a = seed_task(&mut store, "A");
    let b = seed_task(&mut store, "B");
    let c = seed_task(&mut store, "C");

    store
        .insert_link_checked(&a, &b)
        .expect("alpha: A→B should succeed");

    store
        .insert_link_checked(&b, &c)
        .expect("alpha: B→C should succeed");

    // C → A closes the cycle on the alpha board — must be rejected with LinkCycle.
    let result = store.insert_link_checked(&c, &a);
    match result {
        Err(KanbanError::LinkCycle { parent_id, child_id }) => {
            assert_eq!(parent_id, c, "alpha: parent_id in LinkCycle must be C");
            assert_eq!(child_id, a, "alpha: child_id in LinkCycle must be A");
        }
        Err(e) => panic!("alpha: expected LinkCycle but got: {e}"),
        Ok(()) => panic!("alpha: C→A should have been rejected as a cycle"),
    }

    unsafe { std::env::remove_var("IRONHERMES_HOME") };
}

// ---------------------------------------------------------------------------
// Test 3: cross-board link fails at lookup layer (TaskNotFound), NOT LinkCycle
// ---------------------------------------------------------------------------

/// D-05 rationale: cross-board links are blocked at the foreign-key / task-lookup
/// layer, NOT at the cycle-detection layer. The `WITH RECURSIVE` ancestor walk
/// only sees task IDs that exist in the connected DB. Task IDs from other boards
/// simply don't exist in that DB, so the pre-check returns `TaskNotFound` before
/// the cycle SQL ever runs.
///
/// This test creates task X on the default board and task Y on the alpha board,
/// then attempts `insert_link_checked(parent=X, child=Y)` against the alpha
/// connection. Since X doesn't exist in alpha's DB, the pre-check returns
/// `TaskNotFound(X)` — NOT `LinkCycle`.
#[test]
fn cross_board_link_is_blocked_by_construction_not_by_cycle_sql() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

    // Create task X on the default board.
    let task_x_id = {
        let mut default_store = KanbanStore::open_default().expect("open_default");
        seed_task(&mut default_store, "X-on-default")
    };

    // Create task Y on the alpha board, then attempt the cross-board link.
    let mut alpha_store = KanbanStore::open_for_board("alpha").expect("open_for_board('alpha')");
    let task_y_id = seed_task(&mut alpha_store, "Y-on-alpha");

    // Attempt insert_link_checked(parent=X, child=Y) against ALPHA's connection.
    // X does not exist in alpha's DB, so this MUST return TaskNotFound(X).
    let result = alpha_store.insert_link_checked(&task_x_id, &task_y_id);
    match result {
        Err(KanbanError::TaskNotFound(id)) => {
            assert_eq!(
                id, task_x_id,
                "TaskNotFound should identify X (the cross-board task), got: {id}"
            );
        }
        Err(KanbanError::LinkCycle { .. }) => {
            panic!(
                "cross-board link must fail with TaskNotFound (not LinkCycle) — \
                 D-05 rationale: cycle SQL never executes because the lookup pre-check \
                 catches the missing task first"
            );
        }
        Err(e) => panic!("expected TaskNotFound but got unexpected error: {e}"),
        Ok(()) => panic!(
            "cross-board insert_link_checked must fail — task X doesn't exist in alpha's DB"
        ),
    }

    unsafe { std::env::remove_var("IRONHERMES_HOME") };
}
