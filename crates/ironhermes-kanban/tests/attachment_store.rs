//! Phase 46.4 Plan 03 Task 1 — `KanbanStore::add_attachment` coverage.
//!
//! Validates D-05 (single traversal-safe writer, loose files under
//! `kanban_attachments_dir(task_id)`, DB-id on-disk names) and T-46.4-03
//! (path-traversal rejection: `".."` or a path separator in the client
//! filename must be rejected BEFORE any filesystem operation).

use std::sync::Mutex;

use ironhermes_kanban::paths::kanban_attachments_dir;
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};

/// `IRONHERMES_HOME` is a process-global env var read by `kanban_attachments_dir`
/// (via `ironhermes_core::get_hermes_home()`). Serialize all tests in this file
/// that mutate it so they don't race under cargo's default parallel-thread
/// test runner (mirrors the `ENV_LOCK` pattern in `tests/tools_smoke.rs`).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Opens a store rooted at a fresh tempdir and sets `IRONHERMES_HOME` to the
/// same dir so `kanban_attachments_dir(task_id)` resolves under it. Leaks the
/// tempdir so the DB file + attachment files survive the test.
fn make_store() -> KanbanStore {
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
    std::mem::forget(dir);
    KanbanStore::open_default().unwrap()
}

fn seed_task(store: &mut KanbanStore) -> String {
    store
        .create_task("Attachment Test", "dev", CreateTaskOptions::default())
        .unwrap()
        .id
}

#[test]
fn add_attachment_writes_id_dir_original_named_file_and_row() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = make_store();
    let task_id = seed_task(&mut store);

    let meta = store
        .add_attachment(
            &task_id,
            "report.pdf",
            b"hello world",
            Some("application/pdf"),
            Some("dev"),
        )
        .expect("add_attachment should succeed for a valid filename");

    // Display metadata preserves the client filename.
    assert_eq!(meta.filename, "report.pdf");
    assert_eq!(meta.task_id, task_id);
    assert_eq!(meta.size_bytes, 11);
    assert_eq!(meta.content_type.as_deref(), Some("application/pdf"));

    // On-disk layout: <id>/<original filename> — openable with the correct
    // name + extension, confined by the opaque id directory (UAT gap fix).
    let dir = kanban_attachments_dir(&task_id);
    let expected_path = dir.join(&meta.id).join("report.pdf");
    assert!(
        expected_path.exists(),
        "attachment bytes must be written to {}/<id>/report.pdf, got stored_path={}",
        dir.display(),
        meta.stored_path
    );
    assert!(
        !dir.join("report.pdf").exists(),
        "the client filename must NEVER be used directly under kanban_attachments_dir(task_id) \
         — it must live inside the <id> subdirectory"
    );
    let bytes = std::fs::read(&expected_path).unwrap();
    assert_eq!(bytes, b"hello world");
    assert_eq!(meta.stored_path, expected_path.to_string_lossy());

    // task_attachments row exists and list_attachments returns it.
    let listed = store.list_attachments(&task_id).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, meta.id);
    assert_eq!(listed[0].filename, "report.pdf");
}

#[test]
fn add_attachment_rejects_empty_bytes() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = make_store();
    let task_id = seed_task(&mut store);

    let result = store.add_attachment(&task_id, "empty.txt", &[], None, None);
    assert!(
        result.is_err(),
        "an empty byte payload must be rejected — no 0-byte artifact should persist"
    );

    // No directory/file was created for the rejected empty upload.
    let dir = kanban_attachments_dir(&task_id);
    if dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(
            entries.is_empty(),
            "no attachment directory should have been created for an empty payload"
        );
    }
    assert_eq!(store.list_attachments(&task_id).unwrap().len(), 0);
}

#[test]
fn add_attachment_rejects_parent_traversal_filename() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = make_store();
    let task_id = seed_task(&mut store);

    let result = store.add_attachment(&task_id, "../evil.txt", b"pwned", None, None);
    assert!(
        result.is_err(),
        "a filename containing '..' must be rejected"
    );

    // No file escaped the attachments dir, and the dir holds nothing.
    let dir = kanban_attachments_dir(&task_id);
    if dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(
            entries.is_empty(),
            "no file should have been written for a rejected filename"
        );
    }
    let parent_escape = dir.parent().unwrap().join("evil.txt");
    assert!(
        !parent_escape.exists(),
        "attachment must not escape kanban_attachments_dir(task_id)"
    );

    // No task_attachments row either.
    assert_eq!(store.list_attachments(&task_id).unwrap().len(), 0);
}

#[test]
fn add_attachment_rejects_path_separator_filename() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = make_store();
    let task_id = seed_task(&mut store);

    let result = store.add_attachment(&task_id, "a/b.txt", b"pwned", None, None);
    assert!(
        result.is_err(),
        "a filename containing a path separator must be rejected"
    );

    let dir = kanban_attachments_dir(&task_id);
    assert!(
        !dir.join("a").exists(),
        "no subdirectory should be created from a separator-bearing filename"
    );
    assert_eq!(store.list_attachments(&task_id).unwrap().len(), 0);
}

#[test]
fn add_attachment_rejects_backslash_separator_filename() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = make_store();
    let task_id = seed_task(&mut store);

    let result = store.add_attachment(&task_id, r"a\b.txt", b"pwned", None, None);
    assert!(
        result.is_err(),
        "a filename containing a backslash separator must be rejected"
    );
    assert_eq!(store.list_attachments(&task_id).unwrap().len(), 0);
}

#[test]
fn add_attachment_rejects_empty_filename() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = make_store();
    let task_id = seed_task(&mut store);

    let result = store.add_attachment(&task_id, "", b"pwned", None, None);
    assert!(result.is_err(), "an empty filename must be rejected");
    assert_eq!(store.list_attachments(&task_id).unwrap().len(), 0);
}

#[test]
fn list_attachments_orders_by_created_at_ascending() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = make_store();
    let task_id = seed_task(&mut store);

    let first = store
        .add_attachment(&task_id, "first.txt", b"1", None, None)
        .unwrap();
    let second = store
        .add_attachment(&task_id, "second.txt", b"2", None, None)
        .unwrap();

    let listed = store.list_attachments(&task_id).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, first.id);
    assert_eq!(listed[1].id, second.id);
}
