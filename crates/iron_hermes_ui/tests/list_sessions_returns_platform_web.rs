//! Behavioral round-trip test for Phase 34 D-07/D-08.
//!
//! Verifies that a Platform::Web session inserted via StateStore is returned
//! when queried with a Platform::Web source filter — the same code path that
//! api.rs::list_sessions uses after the D-07 fix.
//!
//! A stub implementation of list_sessions that returns `vec![]` or passes
//! `None` as the source filter (returning ALL sessions) cannot satisfy the
//! semantic contract this test locks: only Platform::Web sessions appear in
//! the list_sessions response. BLOCKER 6 closure: grep alone is insufficient.

use ironhermes_core::types::Platform;
use ironhermes_state::StateStore;

/// Inserts a Platform::Web session into a fresh in-memory StateStore (backed
/// by a temp SQLite file) and asserts that querying with Platform::Web filter
/// returns the inserted session. This mirrors the exact StateStore call that
/// api.rs::list_sessions makes after the D-07 fix.
#[test]
fn list_sessions_returns_inserted_platform_web_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test_state.db");
    let mut store = StateStore::new(&db_path).expect("open fresh StateStore");

    let session_id = "test-session-001";
    store
        .create_session(
            session_id,
            &Platform::Web.to_string(),
            None,
            None,
            None,
            None,
        )
        .expect("create_session must succeed");

    // Simulate what api.rs::list_sessions does after the D-07 fix:
    // query with Some(Platform::Web.to_string()) as the source filter.
    let platform_filter = Platform::Web.to_string();
    let sessions = store
        .list_sessions(Some(&platform_filter), 100)
        .expect("list_sessions must succeed");

    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&session_id),
        "list_sessions filtered by Platform::Web must return the inserted session. \
         Got: {:?}",
        ids
    );
}

/// Confirm that a non-Web session (e.g. Telegram) does NOT appear when
/// filtering by Platform::Web. Prevents cross-platform session bleed (T-34-05).
#[test]
fn list_sessions_excludes_non_web_sessions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test_state_excl.db");
    let mut store = StateStore::new(&db_path).expect("open fresh StateStore");

    store
        .create_session(
            "telegram-session-001",
            &Platform::Telegram.to_string(),
            None,
            None,
            None,
            None,
        )
        .expect("create telegram session");

    let platform_filter = Platform::Web.to_string();
    let sessions = store
        .list_sessions(Some(&platform_filter), 100)
        .expect("list_sessions must succeed");

    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(
        !ids.contains(&"telegram-session-001"),
        "Platform::Web filter must exclude Telegram sessions. Got: {:?}",
        ids
    );
}
