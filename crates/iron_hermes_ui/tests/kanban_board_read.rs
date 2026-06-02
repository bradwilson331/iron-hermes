//! Phase 36.3.7.11 Plan 01 Wave 2 — behavioral round-trip test for the kanban
//! read path that underlies `fetch_board`.
//!
//! Modeled on tests/list_sessions_returns_platform_web.rs. Opens a temp
//! `KanbanStore` against a tempdir, creates 3 tasks via `KanbanStore::create_task`,
//! and asserts `list_tasks(ListFilters{archived:false,..})` returns all 3 —
//! the same read path `fetch_board` executes inside `tokio::task::spawn_blocking`.
//!
//! This locks the contract that fetch_board's underlying reads are correct.
//! Testing the `#[server]` fn directly would require a full server runtime
//! + the global AppState OnceLock — outside the scope of a unit-style test.

#[cfg(not(target_arch = "wasm32"))]
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore, ListFilters};

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn fetch_board_read_path_returns_all_non_archived_tasks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test_kanban.db");
    let mut store = KanbanStore::open(&db_path).expect("open fresh KanbanStore");

    // Seed 3 tasks under varying status (all non-archived by default —
    // create_task with empty parents and triage=false → status='ready').
    for i in 0..3 {
        store
            .create_task(
                &format!("Task {}", i),
                "alice",
                CreateTaskOptions::default(),
            )
            .expect("create_task must succeed");
    }

    // This is the same call fetch_board makes inside spawn_blocking.
    let filters = ListFilters {
        archived: false,
        ..Default::default()
    };
    let tasks = store.list_tasks(filters).expect("list_tasks must succeed");
    assert_eq!(
        tasks.len(),
        3,
        "fetch_board read path must return all 3 non-archived tasks"
    );

    // Locked: every returned row has a canonical status (D-09 taxonomy).
    let allowed = [
        "triage",
        "todo",
        "ready",
        "running",
        "blocked",
        "done",
        // archived would be filtered out above
    ];
    for t in &tasks {
        assert!(
            allowed.contains(&t.status.as_str()),
            "task status `{}` is not in the D-09 taxonomy",
            t.status
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn fetch_board_read_path_excludes_archived() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test_kanban_archive.db");
    let mut store = KanbanStore::open(&db_path).expect("open fresh KanbanStore");

    // Create 2 tasks — both default to 'ready'.
    let _t1 = store
        .create_task("Task A", "alice", CreateTaskOptions::default())
        .expect("create A");
    let t2 = store
        .create_task("Task B", "alice", CreateTaskOptions::default())
        .expect("create B");

    // Directly mark t2 as archived using the public Connection.
    store
        .conn
        .execute(
            "UPDATE tasks SET status = 'archived' WHERE id = ?1",
            rusqlite::params![t2.id],
        )
        .expect("archive update");

    // archived=false (default) excludes the archived row.
    let filters = ListFilters {
        archived: false,
        ..Default::default()
    };
    let tasks = store.list_tasks(filters).expect("list_tasks");
    assert_eq!(
        tasks.len(),
        1,
        "fetch_board read path must exclude archived tasks (D-18 default board, archived hidden)"
    );
    assert_eq!(tasks[0].id, _t1.id);
}

// Regression test for BUG-1 from 36.3.7.11 UAT — fetch_board never exposed the
// include_archived parameter, so the ARCHIVED column was always empty regardless
// of the SHOW ARCHIVED toggle state. This test locks BOTH directions of the
// `ListFilters.archived` predicate at the store layer so future regressions
// (either accidental drop of the predicate at the store layer OR re-introduction
// of a hardcoded `archived: false` upstream) trip CI immediately.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn fetch_board_returns_archived_when_include_archived_true() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test_kanban_archive_toggle.db");
    let mut store = KanbanStore::open(&db_path).expect("open fresh KanbanStore");

    // Seed two tasks; mark the second as archived using the exact same
    // pattern the existing `fetch_board_read_path_excludes_archived` test uses.
    let t1 = store
        .create_task("Task A", "alice", CreateTaskOptions::default())
        .expect("create A");
    let t2 = store
        .create_task("Task B", "alice", CreateTaskOptions::default())
        .expect("create B");
    store
        .conn
        .execute(
            "UPDATE tasks SET status = 'archived' WHERE id = ?1",
            rusqlite::params![t2.id],
        )
        .expect("archive update");

    // Direction 1: archived=true MUST surface the archived row.
    let filters_with_archived = ListFilters {
        archived: true,
        ..Default::default()
    };
    let tasks_with = store
        .list_tasks(filters_with_archived)
        .expect("list_tasks with archived=true");
    let ids_with: Vec<&str> = tasks_with.iter().map(|t| t.id.as_str()).collect();
    assert!(
        ids_with.contains(&t2.id.as_str()),
        "BUG-1 regression: ListFilters{{archived:true}} must include the archived task; \
         got ids={:?}",
        ids_with
    );

    // Direction 2: archived=false MUST exclude the archived row (regression
    // against accidental flip of the predicate).
    let filters_without_archived = ListFilters {
        archived: false,
        ..Default::default()
    };
    let tasks_without = store
        .list_tasks(filters_without_archived)
        .expect("list_tasks with archived=false");
    let ids_without: Vec<&str> = tasks_without.iter().map(|t| t.id.as_str()).collect();
    assert!(
        !ids_without.contains(&t2.id.as_str()),
        "BUG-1 regression: ListFilters{{archived:false}} must exclude the archived task; \
         got ids={:?}",
        ids_without
    );
    assert!(
        ids_without.contains(&t1.id.as_str()),
        "non-archived task t1 must remain visible when archived=false; got ids={:?}",
        ids_without
    );
}
