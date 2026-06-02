//! Quick task 260602-nd7 — producer-end regression test for U9 fix.
//!
//! Locks the contract that `KanbanStore::add_comment` appends a
//! `task_events` row (kind=`edited`, payload tagged with `subkind:comment`)
//! in addition to the existing `task_comments` row. Without this row the
//! D-15 dashboard tail consumer has nothing to broadcast, the D-21
//! per-task event counter never bumps, and the drawer's COMMENTS
//! `use_resource` never re-runs — i.e. the original U9 failure mode.
//!
//! See `.planning/quick/260602-nd7-fix-u9-drawer-comments-auto-refresh/`
//! for the plan + SUMMARY. The bilateral consumer-end test lives in
//! `crates/iron_hermes_ui/tests/kanban_drawer.rs`
//! (`comments_resource_reads_per_task_event_counter_for_d21`).
//!
//! Fixture style mirrors `tests/store_smoke.rs` — tempfile-backed
//! `KanbanStore::new(path)`, raw `store.conn` rusqlite queries.

use std::path::PathBuf;
use tempfile::TempDir;

use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
use rusqlite::params;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store(dir: &TempDir) -> KanbanStore {
    let path: PathBuf = dir.path().join("kanban.db");
    KanbanStore::new(&path).expect("open store")
}

fn count_events_for_task(store: &KanbanStore, task_id: &str) -> i64 {
    store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM task_events WHERE task_id = ?1",
            params![task_id],
            |r| r.get::<_, i64>(0),
        )
        .expect("count task_events")
}

fn count_all_events(store: &KanbanStore) -> i64 {
    store
        .conn
        .query_row("SELECT COUNT(*) FROM task_events", [], |r| {
            r.get::<_, i64>(0)
        })
        .expect("count all task_events")
}

// ---------------------------------------------------------------------------
// Test 1: comment write emits exactly one task_events row for the same task_id,
// with kind = "edited" (the snake_case string form of KanbanEventKind::Edited).
// ---------------------------------------------------------------------------

#[test]
fn comment_emits_task_event_row() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let task = store
        .create_task("nd7 producer test", "alice", CreateTaskOptions::default())
        .expect("create_task");

    let before = count_events_for_task(&store, &task.id);
    // create_task itself emits a `created` event — `before` is expected >= 1.
    assert!(
        before >= 1,
        "create_task should have emitted at least a `created` event row \
         (got {} for task_id={})",
        before,
        task.id,
    );

    let _comment = store
        .add_comment(&task.id, "tester", "live test from nd7")
        .expect("add_comment");

    let after = count_events_for_task(&store, &task.id);
    assert_eq!(
        after,
        before + 1,
        "add_comment must append exactly ONE task_events row (before={} \
         after={} for task_id={}) — this is the producer end of the D-21 \
         dashboard live-update pipeline; without it the U9 UAT failure \
         from Phase 36.3.7.11 re-ships",
        before,
        after,
        task.id,
    );

    // Inspect the newest event row for this task — its kind must equal
    // the snake_case form of KanbanEventKind::Edited.
    let (latest_task_id, latest_kind): (String, String) = store
        .conn
        .query_row(
            "SELECT task_id, kind FROM task_events \
             WHERE task_id = ?1 \
             ORDER BY id DESC LIMIT 1",
            params![task.id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .expect("query latest event for task");

    assert_eq!(
        latest_task_id, task.id,
        "newest task_events row's task_id must match the commented task"
    );
    assert_eq!(
        latest_kind, "edited",
        "newest task_events row's kind must be `edited` (events.rs is \
         frozen surface per Phase 36.3.7.6 — we reuse Edited with a \
         payload.subkind=\"comment\" tag rather than adding a new variant)"
    );
}

// ---------------------------------------------------------------------------
// Test 2: the event row's payload is JSON {subkind:"comment", comment_id:<id>}
// so downstream consumers can disambiguate a comment-emitted Edited event
// from a real title/assignee-change Edited event (T-quick-nd7-04 mitigation).
// ---------------------------------------------------------------------------

#[test]
fn comment_event_carries_subkind_and_comment_id_in_payload() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let task = store
        .create_task("nd7 payload test", "bob", CreateTaskOptions::default())
        .expect("create_task");

    let comment = store
        .add_comment(&task.id, "tester-2", "payload test")
        .expect("add_comment");

    let payload_str: Option<String> = store
        .conn
        .query_row(
            "SELECT payload FROM task_events \
             WHERE task_id = ?1 \
             ORDER BY id DESC LIMIT 1",
            params![task.id],
            |r| r.get::<_, Option<String>>(0),
        )
        .expect("query latest event payload");

    let payload_str = payload_str
        .expect("comment-emitted event row must carry a non-NULL payload");
    let payload: serde_json::Value = serde_json::from_str(&payload_str)
        .expect("payload must parse as JSON");

    assert_eq!(
        payload["subkind"], "comment",
        "payload.subkind must equal \"comment\" so downstream filters \
         (e.g. an `Edited`-only consumer) can disambiguate this from a \
         title/assignee-change Edited event — got payload={}",
        payload,
    );
    assert_eq!(
        payload["comment_id"], comment.id,
        "payload.comment_id must equal the newly-created TaskComment's id \
         so consumers can fetch the comment body via the comments API — \
         got payload={}",
        payload,
    );
}

// ---------------------------------------------------------------------------
// Test 3: add_comment on a nonexistent task_id returns Err AND does NOT emit
// a stray task_events row — the `self.get_task(task_id)?` precondition at
// store.rs::add_comment is the gate; this test pins it against future
// regressions (e.g. someone moving the get_task check after the inserts).
// ---------------------------------------------------------------------------

#[test]
fn comment_does_not_emit_event_when_task_missing() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let before = count_all_events(&store);

    let result = store.add_comment("t_nonexistent_id_for_nd7", "tester", "body");
    assert!(
        result.is_err(),
        "add_comment on a nonexistent task_id MUST return Err (the \
         existing get_task precondition at store.rs::add_comment) — \
         got Ok={:?}",
        result.ok(),
    );

    let after = count_all_events(&store);
    assert_eq!(
        after, before,
        "no task_events row may land when add_comment rejects the call \
         (before={} after={}) — the transaction in add_comment must \
         abort cleanly when the precondition fails",
        before, after,
    );
}
