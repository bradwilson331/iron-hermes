use ironhermes_core::ChatMessage;
use ironhermes_state::{sanitize_fts_query, SearchFilter, StateStore};
use tempfile::NamedTempFile;

/// Create a fresh StateStore backed by a temporary file.
fn temp_store() -> (StateStore, NamedTempFile) {
    let f = NamedTempFile::new().unwrap();
    let store = StateStore::new(f.path()).unwrap();
    (store, f)
}

// ---------------------------------------------------------------------------
// SESS-01: Persistence across reopens
// ---------------------------------------------------------------------------

#[test]
fn test_state_store_persistence() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_path_buf();

    // Create store, add session + message, then drop.
    {
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("s1", "cli", None, None, None, None)
            .unwrap();
        store
            .add_message("s1", &ChatMessage::user("hello world"))
            .unwrap();
    }

    // Re-open and verify data survived.
    let store = StateStore::new(&path).unwrap();
    let session = store.get_session("s1").unwrap();
    assert!(session.is_some(), "session should survive reopen");
    let msgs = store.get_messages("s1").unwrap();
    assert_eq!(msgs.len(), 1, "message should survive reopen");
    assert_eq!(msgs[0].content.as_deref(), Some("hello world"));
}

// ---------------------------------------------------------------------------
// SESS-03: Session lineage (parent_session_id)
// ---------------------------------------------------------------------------

#[test]
fn test_session_lineage() {
    let (mut store, _f) = temp_store();

    // Parent must exist (FK enforced).
    store
        .create_session("parent_id", "cli", None, None, None, None)
        .unwrap();
    store
        .create_session("child", "cli", None, None, Some("parent_id"), None)
        .unwrap();

    let child = store.get_session("child").unwrap().unwrap();
    assert_eq!(
        child.parent_session_id.as_deref(),
        Some("parent_id"),
        "child should reference parent"
    );
}

// ---------------------------------------------------------------------------
// SESS-04: Title lookup (unique index)
// ---------------------------------------------------------------------------

#[test]
fn test_session_title_lookup() {
    let (mut store, _f) = temp_store();

    store
        .create_session("s1", "cli", None, None, None, None)
        .unwrap();
    store.update_session_title("s1", "My Chat").unwrap();

    let found = store.get_session_by_title("My Chat").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, "s1");

    let not_found = store.get_session_by_title("nonexistent").unwrap();
    assert!(not_found.is_none());

    // Duplicate title on another session should fail (unique partial index).
    store
        .create_session("s2", "cli", None, None, None, None)
        .unwrap();
    let dup = store.update_session_title("s2", "My Chat");
    assert!(
        dup.is_err(),
        "duplicate title should violate unique constraint"
    );
}

// ---------------------------------------------------------------------------
// SESS-05: FTS5 input sanitization
// ---------------------------------------------------------------------------

#[test]
fn test_fts_sanitize() {
    assert_eq!(sanitize_fts_query("hello AND world"), "hello world");
    assert_eq!(sanitize_fts_query("test*"), "test");
    assert_eq!(sanitize_fts_query("\"quoted phrase\""), "quoted phrase");
    assert_eq!(sanitize_fts_query("   "), "");
    // Bare operators become empty.
    assert_eq!(sanitize_fts_query("AND"), "");
    assert_eq!(sanitize_fts_query("OR NOT NEAR"), "");
}

// ---------------------------------------------------------------------------
// SESS-06: Search snippet with << >> markers
// ---------------------------------------------------------------------------

#[test]
fn test_search_snippet() {
    let (mut store, _f) = temp_store();

    store
        .create_session("s1", "cli", None, None, None, None)
        .unwrap();
    store
        .add_message(
            "s1",
            &ChatMessage::user("The quick brown fox jumps over the lazy dog"),
        )
        .unwrap();

    let filter = SearchFilter {
        query: Some("fox".into()),
        limit: 10,
        ..SearchFilter::default()
    };
    let results = store.search_messages(&filter).unwrap();
    assert!(!results.is_empty(), "search should find 'fox'");

    let snippet = results[0].snippet.as_ref().expect("snippet should exist");
    assert!(
        snippet.contains("<<") && snippet.contains(">>"),
        "snippet should contain << >> markers, got: {snippet}"
    );
}

// ---------------------------------------------------------------------------
// SESS-06: Context window (before/after)
// ---------------------------------------------------------------------------

#[test]
fn test_search_context_window() {
    let (mut store, _f) = temp_store();

    store
        .create_session("s1", "cli", None, None, None, None)
        .unwrap();

    // Add 3 messages with small sleeps to ensure distinct timestamps.
    store
        .add_message("s1", &ChatMessage::user("before message alpha"))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(15));
    store
        .add_message("s1", &ChatMessage::assistant("target message beta"))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(15));
    store
        .add_message("s1", &ChatMessage::user("after message gamma"))
        .unwrap();

    let filter = SearchFilter {
        query: Some("target".into()),
        limit: 10,
        ..SearchFilter::default()
    };
    let results = store.search_messages(&filter).unwrap();
    assert!(!results.is_empty(), "should find 'target'");

    let r = &results[0];
    let before = r.context_before.as_deref().unwrap_or("");
    let after = r.context_after.as_deref().unwrap_or("");
    assert!(
        before.contains("before message"),
        "context_before should contain preceding message, got: {before}"
    );
    assert!(
        after.contains("after message"),
        "context_after should contain following message, got: {after}"
    );
}

// ---------------------------------------------------------------------------
// SESS-07: Search filter by source, role, date range
// ---------------------------------------------------------------------------

#[test]
fn test_search_filter() {
    let (mut store, _f) = temp_store();

    store
        .create_session("cli1", "cli", None, None, None, None)
        .unwrap();
    store
        .create_session("tg1", "telegram", None, None, None, None)
        .unwrap();

    store
        .add_message("cli1", &ChatMessage::user("cli searchable content"))
        .unwrap();
    store
        .add_message("tg1", &ChatMessage::user("telegram searchable content"))
        .unwrap();

    // Filter by source.
    let filter = SearchFilter {
        query: Some("searchable".into()),
        source: Some("cli".into()),
        limit: 10,
        ..SearchFilter::default()
    };
    let results = store.search_messages(&filter).unwrap();
    assert!(!results.is_empty(), "should find cli result");
    for r in &results {
        assert_eq!(
            r.session_source.as_deref(),
            Some("cli"),
            "all results should be from cli source"
        );
    }

    // Filter by role.
    store
        .add_message("cli1", &ChatMessage::assistant("assistant reply"))
        .unwrap();
    let filter = SearchFilter {
        source: Some("cli".into()),
        role: Some("assistant".into()),
        limit: 10,
        ..SearchFilter::default()
    };
    let results = store.search_messages(&filter).unwrap();
    assert!(!results.is_empty(), "should find assistant message");
    for r in &results {
        assert_eq!(r.role, "assistant");
    }
}

// ---------------------------------------------------------------------------
// SESS-08: Export session (single + bulk)
// ---------------------------------------------------------------------------

#[test]
fn test_export_session() {
    let (mut store, _f) = temp_store();

    store
        .create_session("s1", "cli", None, None, None, None)
        .unwrap();
    store
        .add_message("s1", &ChatMessage::user("msg1"))
        .unwrap();
    store
        .add_message("s1", &ChatMessage::assistant("msg2"))
        .unwrap();

    let export = store.export_session("s1").unwrap();
    assert_eq!(export.session.id, "s1");
    assert_eq!(export.messages.len(), 2);

    // Verify JSON structure via serde_json.
    let val = serde_json::to_value(&export).unwrap();
    assert!(val.get("session").is_some(), "JSON should have 'session' key");
    assert!(
        val.get("messages").is_some(),
        "JSON should have 'messages' key"
    );

    // Non-existent session returns error.
    let err = store.export_session("nonexistent");
    assert!(err.is_err());
}

#[test]
fn test_export_sessions_bulk() {
    let (mut store, _f) = temp_store();

    store
        .create_session("c1", "cli", None, None, None, None)
        .unwrap();
    store
        .create_session("t1", "telegram", None, None, None, None)
        .unwrap();
    store
        .add_message("c1", &ChatMessage::user("cli msg"))
        .unwrap();
    store
        .add_message("t1", &ChatMessage::user("tg msg"))
        .unwrap();

    // All sessions.
    let all = store.export_sessions(None).unwrap();
    assert_eq!(all.len(), 2);

    // Filtered by source.
    let cli_only = store.export_sessions(Some("cli")).unwrap();
    assert_eq!(cli_only.len(), 1);
    assert_eq!(cli_only[0].session.source, "cli");
}

// ---------------------------------------------------------------------------
// SESS-09: Prune ended sessions
// ---------------------------------------------------------------------------

#[test]
fn test_prune_sessions() {
    let (mut store, _f) = temp_store();

    // Create two sessions.
    store
        .create_session("ended1", "cli", None, None, None, None)
        .unwrap();
    store
        .create_session("active1", "cli", None, None, None, None)
        .unwrap();

    // Add messages to both.
    store
        .add_message("ended1", &ChatMessage::user("will be pruned"))
        .unwrap();
    store
        .add_message("active1", &ChatMessage::user("will survive"))
        .unwrap();

    // End one session.
    store.end_session("ended1", "completed").unwrap();

    // Small sleep so ended_at is strictly less than the cutoff computed by prune.
    std::thread::sleep(std::time::Duration::from_millis(20));

    // Prune with older_than_days=0 -> cutoff = now, so ended_at < now is true.
    let deleted = store.prune_sessions(0, None).unwrap();
    assert_eq!(deleted, 1, "should delete exactly the ended session");

    // Ended session gone.
    assert!(store.get_session("ended1").unwrap().is_none());
    // Its messages also gone.
    assert!(store.get_messages("ended1").unwrap().is_empty());

    // Active session untouched.
    assert!(store.get_session("active1").unwrap().is_some());
    assert_eq!(store.get_messages("active1").unwrap().len(), 1);

    // Pruning again deletes nothing (active session has no ended_at).
    let deleted2 = store.prune_sessions(0, None).unwrap();
    assert_eq!(deleted2, 0);
}

// ---------------------------------------------------------------------------
// SESS-10: Migration idempotent (reopen)
// ---------------------------------------------------------------------------

#[test]
fn test_migration_idempotent() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_path_buf();

    // Open, close, reopen — no error.
    {
        let _store = StateStore::new(&path).unwrap();
    }
    let _store2 = StateStore::new(&path).unwrap();
}

// ---------------------------------------------------------------------------
// SESS-11: WAL checkpoint
// ---------------------------------------------------------------------------

#[test]
fn test_wal_checkpoint() {
    let (store, _f) = temp_store();
    // Should succeed without error on a fresh store.
    store.wal_checkpoint().unwrap();
}
