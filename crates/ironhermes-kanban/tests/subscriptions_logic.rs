//! Phase 36.3.7.5 Plan 01 (BUG-36.3.7.5-01 + BUG-36.3.7.5-02 + part of BUG-36.3.7.5-07).
//! Receiver-end tests for the kanban_subscriptions schema + 5 CRUD APIs.
//!
//! Bilateral-tracing principle (LEARNINGS 2026-05-29): every store API change
//! ships with a paired test in the SAME plan, not deferred.
//!
//! Test map:
//! - schema_migration_creates_table_and_indexes — schema DDL + idempotency.
//! - append_then_list_by_task — append + list_subscriptions_for_task happy path.
//! - list_by_chat_strict_thread_filter — None vs Some(t) thread-id semantics.
//! - remove_filtered_by_platform_and_chat — remove_subscriptions filter cases.
//! - remove_all_for_task — remove_all_subscriptions_for_task convenience alias.
//! - unique_constraint_blocks_duplicate — UNIQUE (task, platform, chat, thread).
//! - check_constraint_blocks_invalid_source — CHECK (source IN ('auto','explicit')).
//! - empty_thread_id_distinct_from_seven — '' and '7' are distinct keys.

use ironhermes_kanban::store::CreateTaskOptions;
use ironhermes_kanban::{KanbanStore, Subscription};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store() -> (TempDir, KanbanStore) {
    let dir = TempDir::new().expect("tempdir");
    let store = KanbanStore::new(dir.path().join("kanban.db")).expect("open store");
    (dir, store)
}

/// Create a task via the existing public API and return its id.
///
/// `CreateTaskOptions` carries scheduling/skills/tenant fields; this helper
/// uses `Default::default()` because none of the subscription CRUD methods
/// inspect any task field beyond the id.
fn create_task(store: &mut KanbanStore, title: &str) -> String {
    store
        .create_task(title, "alice", CreateTaskOptions::default())
        .expect("create_task")
        .id
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// BUG-36.3.7.5-01: opening the store creates `kanban_subscriptions` + 2 indexes.
/// Re-opening the same DB file is idempotent (the DDL uses `IF NOT EXISTS`).
#[test]
fn schema_migration_creates_table_and_indexes() {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("kanban.db");

    let store = KanbanStore::new(&db_path).expect("open #1 must succeed (BUG-36.3.7.5-01)");

    let table_count: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master \
             WHERE type='table' AND name='kanban_subscriptions'",
            [],
            |r| r.get(0),
        )
        .expect("query table");
    assert_eq!(
        table_count, 1,
        "kanban_subscriptions table must exist (BUG-36.3.7.5-01)"
    );

    let index_count: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master \
             WHERE type='index' AND name IN ('idx_subs_task_id', 'idx_subs_chat')",
            [],
            |r| r.get(0),
        )
        .expect("query indexes");
    assert_eq!(
        index_count, 2,
        "idx_subs_task_id + idx_subs_chat indexes must both exist (BUG-36.3.7.5-01)"
    );

    drop(store);

    // Idempotency: re-opening the same DB file MUST NOT error.
    let _store_again = KanbanStore::new(&db_path)
        .expect("re-open must be idempotent — CREATE TABLE IF NOT EXISTS (BUG-36.3.7.5-01)");
}

/// BUG-36.3.7.5-02: append_subscription writes rows; list_subscriptions_for_task
/// returns them all in id-ASC order.
#[test]
fn append_then_list_by_task() {
    let (_dir, mut store) = open_store();
    let task_id = create_task(&mut store, "subs-list-by-task");

    let id1 = store
        .append_subscription(&task_id, "telegram", "42", None, "auto")
        .expect("append #1 (BUG-36.3.7.5-02)");
    let id2 = store
        .append_subscription(&task_id, "discord", "99", None, "explicit")
        .expect("append #2 (BUG-36.3.7.5-02)");
    assert!(id2 > id1, "AUTOINCREMENT id must be monotone");

    let rows = store
        .list_subscriptions_for_task(&task_id)
        .expect("list_subscriptions_for_task (BUG-36.3.7.5-02)");
    assert_eq!(
        rows.len(),
        2,
        "list_subscriptions_for_task must return both rows (BUG-36.3.7.5-02)"
    );
    // id ASC ordering — first row is the first insert.
    assert_eq!(
        rows[0].platform, "telegram",
        "row 0 must be the auto insert"
    );
    assert_eq!(rows[0].source, "auto");
    assert_eq!(rows[0].thread_id, "", "None thread_id stored as ''");
    assert_eq!(
        rows[1].platform, "discord",
        "row 1 must be the explicit insert"
    );
    assert_eq!(rows[1].source, "explicit");
}

/// BUG-36.3.7.5-02: list_subscriptions_for_chat applies strict thread-id
/// filtering — `None` matches '' rows; `Some(t)` matches exact `t`.
#[test]
fn list_by_chat_strict_thread_filter() {
    let (_dir, mut store) = open_store();
    let task_a = create_task(&mut store, "task-a");
    let task_b = create_task(&mut store, "task-b");

    store
        .append_subscription(&task_a, "telegram", "42", None, "auto")
        .expect("append a");
    store
        .append_subscription(&task_a, "telegram", "42", Some("7"), "auto")
        .expect("append b");
    store
        .append_subscription(&task_b, "telegram", "42", None, "explicit")
        .expect("append c");

    let no_thread: Vec<Subscription> = store
        .list_subscriptions_for_chat("telegram", "42", None)
        .expect("list_subscriptions_for_chat None (BUG-36.3.7.5-02)");
    assert_eq!(
        no_thread.len(),
        2,
        "thread_id=None must match the 2 empty-string rows (BUG-36.3.7.5-02)"
    );
    for row in &no_thread {
        assert_eq!(row.thread_id, "", "all rows must have '' thread_id");
    }

    let thread7: Vec<Subscription> = store
        .list_subscriptions_for_chat("telegram", "42", Some("7"))
        .expect("list_subscriptions_for_chat Some('7') (BUG-36.3.7.5-02)");
    assert_eq!(
        thread7.len(),
        1,
        "thread_id=Some('7') must match the single thread-7 row (BUG-36.3.7.5-02)"
    );
    assert_eq!(thread7[0].thread_id, "7");

    let other_platform: Vec<Subscription> = store
        .list_subscriptions_for_chat("discord", "42", None)
        .expect("list_subscriptions_for_chat discord");
    assert!(
        other_platform.is_empty(),
        "discord/42 has no rows (BUG-36.3.7.5-02)"
    );
}

/// BUG-36.3.7.5-02: remove_subscriptions filter cases (platform-only, then
/// platform+chat) delete exactly the targeted rows.
#[test]
fn remove_filtered_by_platform_and_chat() {
    let (_dir, mut store) = open_store();
    let task_a = create_task(&mut store, "task-a");

    store
        .append_subscription(&task_a, "telegram", "42", None, "auto")
        .expect("append #1");
    store
        .append_subscription(&task_a, "telegram", "99", None, "auto")
        .expect("append #2");
    store
        .append_subscription(&task_a, "discord", "42", None, "explicit")
        .expect("append #3");

    let n = store
        .remove_subscriptions(&task_a, Some("telegram"), Some("42"))
        .expect("remove (task, telegram, 42) (BUG-36.3.7.5-02)");
    assert_eq!(n, 1, "must remove exactly the (telegram, 42) row");

    let remaining = store
        .list_subscriptions_for_task(&task_a)
        .expect("list after first remove");
    assert_eq!(remaining.len(), 2, "two rows remain after first delete");

    let n = store
        .remove_subscriptions(&task_a, Some("telegram"), None)
        .expect("remove (task, telegram, None) (BUG-36.3.7.5-02)");
    assert_eq!(n, 1, "must remove the (telegram, 99) row");

    let remaining = store
        .list_subscriptions_for_task(&task_a)
        .expect("list after second remove");
    assert_eq!(
        remaining.len(),
        1,
        "one (discord, 42) row remains (BUG-36.3.7.5-02)"
    );
    assert_eq!(remaining[0].platform, "discord");
    assert_eq!(remaining[0].chat_id, "42");
}

/// BUG-36.3.7.5-02: remove_all_subscriptions_for_task deletes every row for
/// the task and returns the count.
#[test]
fn remove_all_for_task() {
    let (_dir, mut store) = open_store();
    let task_a = create_task(&mut store, "task-a");

    store
        .append_subscription(&task_a, "telegram", "1", None, "auto")
        .expect("append #1");
    store
        .append_subscription(&task_a, "telegram", "2", None, "auto")
        .expect("append #2");
    store
        .append_subscription(&task_a, "discord", "3", None, "explicit")
        .expect("append #3");

    let n = store
        .remove_all_subscriptions_for_task(&task_a)
        .expect("remove_all_subscriptions_for_task (BUG-36.3.7.5-02)");
    assert_eq!(n, 3, "must remove all 3 rows for the task");

    let remaining = store
        .list_subscriptions_for_task(&task_a)
        .expect("list after remove_all");
    assert!(
        remaining.is_empty(),
        "remove_all_subscriptions_for_task must leave no rows (BUG-36.3.7.5-02)"
    );
}

/// BUG-36.3.7.5-01: UNIQUE constraint blocks duplicate
/// `(task_id, platform, chat_id, thread_id)` tuples.
#[test]
fn unique_constraint_blocks_duplicate() {
    let (_dir, mut store) = open_store();
    let task_a = create_task(&mut store, "task-a");

    store
        .append_subscription(&task_a, "telegram", "42", Some("7"), "auto")
        .expect("first insert (BUG-36.3.7.5-01)");
    let err = store
        .append_subscription(&task_a, "telegram", "42", Some("7"), "auto")
        .expect_err(
            "duplicate (task, telegram, 42, '7') tuple must violate UNIQUE (BUG-36.3.7.5-01)",
        );
    let msg = err.to_string();
    assert!(
        msg.contains("UNIQUE") || msg.contains("unique") || msg.contains("constraint"),
        "error must cite the UNIQUE / constraint violation, got: {msg}"
    );
}

/// BUG-36.3.7.5-01: CHECK constraint blocks a `source` value outside
/// `('auto', 'explicit')`.
#[test]
fn check_constraint_blocks_invalid_source() {
    let (_dir, mut store) = open_store();
    let task_a = create_task(&mut store, "task-a");

    let err = store
        .append_subscription(&task_a, "telegram", "42", None, "other")
        .expect_err("source='other' must violate CHECK constraint (BUG-36.3.7.5-01)");
    let msg = err.to_string();
    assert!(
        msg.contains("CHECK") || msg.contains("check") || msg.contains("constraint"),
        "error must cite the CHECK / constraint violation, got: {msg}"
    );
}

/// BUG-36.3.7.5-01: empty-string thread_id and `'7'` thread_id are distinct
/// UNIQUE keys — both inserts succeed.
#[test]
fn empty_thread_id_distinct_from_seven() {
    let (_dir, mut store) = open_store();
    let task_a = create_task(&mut store, "task-a");

    let id1 = store
        .append_subscription(&task_a, "telegram", "42", None, "auto")
        .expect("None thread_id append (BUG-36.3.7.5-01)");
    let id2 = store
        .append_subscription(&task_a, "telegram", "42", Some("7"), "auto")
        .expect("Some('7') thread_id append must NOT violate UNIQUE (BUG-36.3.7.5-01)");
    assert!(id2 > id1, "second insert must succeed with a fresh id");

    let rows = store
        .list_subscriptions_for_task(&task_a)
        .expect("list after both inserts");
    assert_eq!(
        rows.len(),
        2,
        "'' and '7' are distinct keys — both rows must persist (BUG-36.3.7.5-01)"
    );
}

// ---------------------------------------------------------------------------
// Post-Phase-36.3.7.6 housekeeping (2026-05-30): orphan table removal.
// ---------------------------------------------------------------------------
// During Phase 36.3.7.5 planning the subscriptions table went through an
// intermediate name `kanban_notify_subs` before the locked D-table-name
// decision renamed it to `kanban_subscriptions`. Any DB created against a
// pre-rename build retains the orphan (live observation on 2026-05-30 against
// ~/.ironhermes/kanban.db). This test seeds a fresh DB with the orphan and
// asserts KanbanStore::new drops it via the DROP TABLE IF EXISTS line added
// to SCHEMA_SQL.

#[test]
fn orphaned_kanban_notify_subs_table_is_dropped_on_open() {
    use rusqlite::Connection;

    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("kanban.db");

    // Seed the DB with the orphan table to simulate a pre-rename install.
    {
        let conn = Connection::open(&db_path).expect("open raw conn for seed");
        conn.execute_batch(
            "CREATE TABLE kanban_notify_subs (
                task_id       TEXT NOT NULL,
                platform      TEXT NOT NULL,
                chat_id       TEXT NOT NULL,
                thread_id     TEXT NOT NULL DEFAULT '',
                user_id       TEXT,
                notifier_profile TEXT,
                created_at    INTEGER NOT NULL,
                last_event_id INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (task_id, platform, chat_id, thread_id)
            );",
        )
        .expect("seed orphan");
    }

    // Sanity: confirm the orphan is present before KanbanStore opens.
    {
        let conn = Connection::open(&db_path).expect("re-open for sanity");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master \
                 WHERE type='table' AND name='kanban_notify_subs'",
                [],
                |row| row.get(0),
            )
            .expect("pre-open count");
        assert_eq!(count, 1, "seed must create the orphan table");
    }

    // Open KanbanStore — SCHEMA_SQL runs including DROP TABLE IF EXISTS kanban_notify_subs.
    let _store = KanbanStore::new(&db_path).expect("open store");

    // The orphan is gone.
    {
        let conn = Connection::open(&db_path).expect("re-open for verify");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master \
                 WHERE type='table' AND name='kanban_notify_subs'",
                [],
                |row| row.get(0),
            )
            .expect("post-open count");
        assert_eq!(
            count, 0,
            "DROP TABLE IF EXISTS kanban_notify_subs must remove the orphan (2026-05-30 housekeeping)"
        );
    }

    // The new kanban_subscriptions table must still be created normally
    // (the rest of SCHEMA_SQL keeps running after the DROP).
    {
        let conn = Connection::open(&db_path).expect("re-open for kanban_subscriptions check");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master \
                 WHERE type='table' AND name='kanban_subscriptions'",
                [],
                |row| row.get(0),
            )
            .expect("subs count");
        assert_eq!(
            count, 1,
            "kanban_subscriptions must still be created normally"
        );
    }

    // And re-opening is idempotent (DROP runs again, still no-op).
    let _store2 = KanbanStore::new(&db_path).expect("idempotent re-open");
}
