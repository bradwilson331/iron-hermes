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
