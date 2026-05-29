//! Store smoke tests for `KanbanStore` (Plan 02 Task 1, TDD RED phase).
//!
//! These tests drive the schema + store implementation. They exercise the
//! CRUD invariants from VALIDATION.md that don't require concurrency.

use std::path::PathBuf;
use tempfile::TempDir;

use ironhermes_kanban::{
    KanbanError,
    store::{CreateTaskOptions, KanbanStore, ListFilters},
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store(dir: &TempDir) -> KanbanStore {
    let path: PathBuf = dir.path().join("kanban.db");
    KanbanStore::new(&path).expect("open store")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Opening the same DB path twice must not fail or duplicate columns.
#[test]
fn idempotent_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kanban.db");
    KanbanStore::new(&path).expect("first open");
    KanbanStore::new(&path).expect("second open — must not fail (idempotent schema)");
}

/// Basic create / get / list round-trip.
#[test]
fn create_get_list() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let task = store
        .create_task("hello world", "alice", CreateTaskOptions::default())
        .expect("create_task");

    assert!(task.id.starts_with("t_"), "id prefix");
    assert_eq!(task.title, "hello world");
    assert_eq!(task.assignee, "alice");
    assert_eq!(task.status, "ready"); // no parents, not triage
    assert!(task.created_at > 0.0);

    // get by id
    let fetched = store.get_task(&task.id).expect("get_task");
    assert_eq!(fetched.id, task.id);
    assert_eq!(fetched.title, "hello world");

    // list returns the task
    let list = store
        .list_tasks(ListFilters {
            assignee: Some("alice".into()),
            ..Default::default()
        })
        .expect("list_tasks");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, task.id);

    // non-existent id returns TaskNotFound
    let err = store.get_task("t_nonexistent").unwrap_err();
    assert!(
        matches!(err, KanbanError::TaskNotFound(_)),
        "expected TaskNotFound, got {err:?}"
    );
}

/// `idempotency_key` deduplication: second create with same key returns existing task.
#[test]
fn idempotency_key_dedups() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let opts = CreateTaskOptions {
        idempotency_key: Some("idem-abc-123".into()),
        ..Default::default()
    };

    let first = store
        .create_task("task one", "alice", opts.clone())
        .expect("first create");

    let second = store
        .create_task("task two", "alice", opts)
        .expect("second create — must dedup");

    // Same task returned, no duplicate row.
    assert_eq!(
        first.id, second.id,
        "idempotency_key must return the existing task on re-create"
    );

    let list = store
        .list_tasks(ListFilters::default())
        .expect("list_tasks");
    assert_eq!(list.len(), 1, "only one row must exist");
}

/// Cross-tenant link is rejected with `TenantMismatch`.
#[test]
fn tenant_mismatch_link_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let parent = store
        .create_task(
            "parent",
            "alice",
            CreateTaskOptions {
                tenant: Some("tenant-a".into()),
                ..Default::default()
            },
        )
        .expect("create parent");

    let child = store
        .create_task(
            "child",
            "alice",
            CreateTaskOptions {
                tenant: Some("tenant-b".into()),
                ..Default::default()
            },
        )
        .expect("create child");

    let err = store.insert_link(&parent.id, &child.id).unwrap_err();
    assert!(
        matches!(err, KanbanError::TenantMismatch { .. }),
        "expected TenantMismatch, got {err:?}"
    );
}

/// Two tenant-less tasks CAN be linked (D-39: both NULL is allowed).
#[test]
fn tenant_link_both_none_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let parent = store
        .create_task("parent", "alice", CreateTaskOptions::default())
        .expect("create parent");

    let child = store
        .create_task("child", "alice", CreateTaskOptions::default())
        .expect("create child");

    store
        .insert_link(&parent.id, &child.id)
        .expect("linking two tenant-less tasks must succeed (D-39)");
}

/// Archiving a running task closes the run with outcome='reclaimed' and emits
/// a `reclaimed` event — reference.md "Reclaimed runs from status changes".
#[test]
fn archive_running_emits_reclaimed_event() {
    use ironhermes_kanban::cas::{atomic_claim, build_claim_lock, DEFAULT_CLAIM_TTL_SECONDS};
    use rusqlite::Connection;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kanban.db");
    let mut store = KanbanStore::new(&path).expect("open store");

    let task = store
        .create_task("running-task", "alice", CreateTaskOptions::default())
        .expect("create");

    // Claim it directly against the underlying connection via a separate Connection.
    let run_id = format!("r_{}", uuid::Uuid::new_v4().simple());
    let claim_lock = build_claim_lock("localhost", 9999);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();

    {
        let mut conn = Connection::open(&path).expect("open conn for claim");
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .unwrap();
        atomic_claim(
            &mut conn,
            &task.id,
            &claim_lock,
            9999,
            DEFAULT_CLAIM_TTL_SECONDS,
            now,
            &run_id,
        )
        .expect("claim");
    }

    // Re-open the store (simulate a different handle) and archive.
    let mut store2 = KanbanStore::new(&path).expect("reopen store");
    store2.archive_task(&task.id).expect("archive running task");

    let archived = store2.get_task(&task.id).expect("get archived");
    assert_eq!(archived.status, "archived");

    // Verify a reclaimed event was written.
    let events = store2
        .get_events(&task.id)
        .expect("get_events");
    let has_reclaimed = events.iter().any(|e| e.kind == "reclaimed");
    assert!(has_reclaimed, "archive of running task must emit 'reclaimed' event");
}
