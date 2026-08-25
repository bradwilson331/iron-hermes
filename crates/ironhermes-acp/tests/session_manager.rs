//! `AcpSessionManager` lifecycle tests (Phase 36.8, plan 02, task 1): empty-list,
//! stable-ordering, idempotent-remove, distinct-concurrent-create, and the D-13
//! close/cleanup pair. Task 3 adds the fork tests to this same file.

use std::sync::{Arc, Mutex as StdMutex};

use ironhermes_acp::session_manager::AcpSessionManager;
use ironhermes_core::{
    ChatMessage, Config, MessageContent, ModelsCache, ProviderConfig, ProviderResolver, Role,
};
use ironhermes_state::StateStore;

fn msg(role: Role, text: &str) -> ChatMessage {
    ChatMessage {
        role,
        content: Some(MessageContent::text(text.to_string())),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        is_recall_context: false,
    }
}

/// Build a `Config`/`ProviderResolver` pair that resolves without touching real
/// provider credentials — mirrors `acp_e2e.rs`'s helper.
fn build_config_and_resolver() -> (Arc<Config>, Arc<ProviderResolver>) {
    let mut config = Config::default();
    // `Config::default()`'s main provider is `openrouter` with its real production base_url,
    // and `ProviderResolver::build` resolves API keys from the process environment while
    // reading the operator's real `$IRONHERMES_HOME/models-cache.json` — so an unpinned
    // helper sends this test's prompt and a real credential to a third party on any machine
    // with `OPENROUTER_API_KEY`/`ANTHROPIC_API_KEY`/`OPENAI_API_KEY` exported. Pin the
    // endpoint to loopback port 1 (refuses instantly, never leaves the machine) with a
    // literal key so no env var is consulted, and use `build_with_cache` so the static model
    // table wins regardless of what is on the operator's disk.
    config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            base_url: Some("http://127.0.0.1:1/v1".to_string()),
            api_key: Some("test-key-not-a-real-credential".to_string()),
            ..Default::default()
        },
    );
    let resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
        .expect("ProviderResolver::build_with_cache with isolated Config");
    (Arc::new(config), Arc::new(resolver))
}

/// An isolated, tempdir-backed `StateStore` — never touches the real state.db. The
/// returned `TempDir` must outlive the manager under test.
fn build_state_store() -> (Arc<StdMutex<StateStore>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir for state.db");
    let store = StateStore::new(tmp.path().join("state.db")).expect("StateStore::new");
    (Arc::new(StdMutex::new(store)), tmp)
}

fn build_manager() -> (AcpSessionManager, tempfile::TempDir) {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, state_tmp) = build_state_store();
    (
        AcpSessionManager::new(state_store, config, resolver),
        state_tmp,
    )
}

#[tokio::test]
async fn list_on_empty_manager_returns_empty_vec() {
    let (manager, _state_tmp) = build_manager();
    assert!(
        manager.list().is_empty(),
        "list() on a manager with no sessions must return an empty vector, not an error"
    );
}

#[tokio::test]
async fn cleanup_is_safe_on_empty_manager() {
    let (mut manager, _state_tmp) = build_manager();
    manager.cleanup(); // must not panic
    assert!(manager.list().is_empty());
}

#[tokio::test]
async fn get_unknown_id_returns_none() {
    let (manager, _state_tmp) = build_manager();
    assert!(manager.get("does-not-exist").is_none());
}

#[tokio::test]
async fn list_returns_stable_creation_order() {
    let (mut manager, _state_tmp) = build_manager();
    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");
    let dir_c = tempfile::tempdir().expect("tempdir c");

    let id_a = manager
        .create(dir_a.path().to_path_buf(), None)
        .await
        .expect("create session a");
    let id_b = manager
        .create(dir_b.path().to_path_buf(), None)
        .await
        .expect("create session b");
    let id_c = manager
        .create(dir_c.path().to_path_buf(), None)
        .await
        .expect("create session c");

    let expected = vec![id_a, id_b, id_c];
    assert_eq!(manager.list(), expected, "list() must return creation order");
    // Repeated calls must return the identical order — not HashMap-arbitrary.
    assert_eq!(manager.list(), expected);
    assert_eq!(manager.list(), expected);
}

#[tokio::test]
async fn distinct_creates_yield_distinct_ids_and_bindings() {
    let (mut manager, _state_tmp) = build_manager();
    let dir_a = tempfile::tempdir().expect("tempdir a");
    let dir_b = tempfile::tempdir().expect("tempdir b");

    let id_a = manager
        .create(dir_a.path().to_path_buf(), None)
        .await
        .expect("create session a");
    let id_b = manager
        .create(dir_b.path().to_path_buf(), None)
        .await
        .expect("create session b");

    assert_ne!(id_a, id_b, "two creates must yield distinct session ids");

    let session_a = manager.get(&id_a).expect("session a must exist");
    let session_b = manager.get(&id_b).expect("session b must exist");
    assert_eq!(session_a.cwd, dir_a.path());
    assert_eq!(session_b.cwd, dir_b.path());
    assert_ne!(
        session_a.cwd, session_b.cwd,
        "each session must be bound to its own cwd"
    );

    // Each session gets its OWN ApprovalsStore instance (D-14 / RESEARCH Pitfall 5) —
    // proven by pointer inequality of the Arc allocations.
    assert!(
        !Arc::ptr_eq(&session_a.approvals, &session_b.approvals),
        "each ACP session must own a fresh ApprovalsStore, never a process-wide shared one"
    );
}

#[tokio::test]
async fn remove_is_idempotent() {
    let (mut manager, _state_tmp) = build_manager();
    let dir = tempfile::tempdir().expect("tempdir");
    let id = manager
        .create(dir.path().to_path_buf(), None)
        .await
        .expect("create session");

    assert!(manager.remove(&id), "first remove must return true");
    assert!(
        !manager.remove(&id),
        "second remove on an already-removed id must be a no-op success (false), not an error"
    );
    assert!(manager.get(&id).is_none());
}

#[tokio::test]
async fn create_writes_through_to_state_store_with_acp_source() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let mut manager = AcpSessionManager::new(state_store.clone(), config, resolver);

    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path().to_path_buf();
    let id = manager
        .create(cwd.clone(), None)
        .await
        .expect("create session");

    let row = state_store
        .lock()
        .unwrap()
        .get_session(&id)
        .expect("get_session should not error")
        .expect("session row must exist in StateStore (D-10)");
    assert_eq!(row.source, "acp");
    assert_eq!(row.workspace_root.as_deref(), Some(cwd.to_string_lossy().as_ref()));
}

#[tokio::test]
async fn close_marks_session_closed_and_archives_via_state_store() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let mut manager = AcpSessionManager::new(state_store.clone(), config, resolver);

    let dir = tempfile::tempdir().expect("tempdir");
    let id = manager
        .create(dir.path().to_path_buf(), None)
        .await
        .expect("create session");

    assert!(manager.close(&id).await, "close on a live session must return true");
    assert!(
        manager.get(&id).expect("session still present after close").closed,
        "close() must mark the session closed"
    );

    let row = state_store
        .lock()
        .unwrap()
        .get_session(&id)
        .expect("get_session should not error")
        .expect("row must still exist");
    assert!(row.ended_at.is_some(), "close() must end the StateStore row (D-13)");

    // cleanup() now removes it.
    manager.cleanup();
    assert!(manager.get(&id).is_none(), "cleanup() must remove closed sessions");
}

#[tokio::test]
async fn close_unknown_id_returns_false_not_panic() {
    let (mut manager, _state_tmp) = build_manager();
    assert!(!manager.close("does-not-exist").await);
}

// ── task 3: fork ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fork_unknown_source_returns_error_not_panic() {
    let (mut manager, _state_tmp) = build_manager();
    let result = manager.fork("does-not-exist").await;
    assert!(result.is_err(), "forking an unknown source id must error, not panic");
}

#[tokio::test]
async fn fork_produces_distinct_id_with_parent_linkage_and_copied_history() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let mut manager = AcpSessionManager::new(state_store.clone(), config, resolver);

    let dir = tempfile::tempdir().expect("tempdir");
    let source_id = manager
        .create(dir.path().to_path_buf(), None)
        .await
        .expect("create source session");

    {
        let mut state = state_store.lock().unwrap();
        state
            .add_message(&source_id, &msg(Role::User, "hello"))
            .unwrap();
        state
            .add_message(&source_id, &msg(Role::Assistant, "hi there"))
            .unwrap();
    }

    let fork_id = manager.fork(&source_id).await.expect("fork must succeed");
    assert_ne!(fork_id, source_id, "fork must allocate a new id, not reuse the source's");

    let fork_row = state_store
        .lock()
        .unwrap()
        .get_session(&fork_id)
        .unwrap()
        .expect("forked session's StateStore row must exist");
    assert_eq!(
        fork_row.parent_session_id.as_deref(),
        Some(source_id.as_str()),
        "forked session's StateStore row must record parent_session_id = source_id"
    );

    let fork_messages = state_store
        .lock()
        .unwrap()
        .get_chat_messages(&fork_id)
        .unwrap();
    assert_eq!(
        fork_messages.len(),
        2,
        "forked session's message history must equal the source session's history at fork time"
    );

    let fork_session = manager.get(&fork_id).expect("fork must be live in-process");
    assert_eq!(
        fork_session.messages.len(),
        2,
        "the live forked AcpSession must be seeded with the copied history"
    );

    // A fork must get its OWN ApprovalsStore — never inherit the parent's grants.
    let source_session = manager.get(&source_id).expect("source must still be live");
    assert!(
        !Arc::ptr_eq(&source_session.approvals, &fork_session.approvals),
        "a fork must not inherit the parent's ApprovalsStore instance"
    );
}

#[tokio::test]
async fn fork_of_empty_source_produces_valid_empty_child() {
    let (mut manager, _state_tmp) = build_manager();
    let dir = tempfile::tempdir().expect("tempdir");
    let source_id = manager
        .create(dir.path().to_path_buf(), None)
        .await
        .expect("create source session (zero messages)");

    let fork_id = manager
        .fork(&source_id)
        .await
        .expect("forking a zero-message source must still succeed");
    let fork_session = manager.get(&fork_id).expect("fork must be live");
    assert!(
        fork_session.messages.is_empty(),
        "forking a session with zero messages must produce a valid, empty child session"
    );
}

#[tokio::test]
async fn forking_same_source_twice_produces_distinct_child_ids() {
    let (mut manager, _state_tmp) = build_manager();
    let dir = tempfile::tempdir().expect("tempdir");
    let source_id = manager
        .create(dir.path().to_path_buf(), None)
        .await
        .expect("create source session");

    let fork_1 = manager.fork(&source_id).await.expect("first fork");
    let fork_2 = manager.fork(&source_id).await.expect("second fork");
    assert_ne!(fork_1, fork_2, "two forks of the same source must produce distinct child ids");
}

#[tokio::test]
async fn appending_to_fork_does_not_mutate_source_and_vice_versa() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let mut manager = AcpSessionManager::new(state_store.clone(), config, resolver);

    let dir = tempfile::tempdir().expect("tempdir");
    let source_id = manager
        .create(dir.path().to_path_buf(), None)
        .await
        .expect("create source session");
    {
        let mut state = state_store.lock().unwrap();
        state
            .add_message(&source_id, &msg(Role::User, "seed"))
            .unwrap();
    }
    let fork_id = manager.fork(&source_id).await.expect("fork must succeed");

    // Append independently to each session's StateStore row after the fork.
    {
        let mut state = state_store.lock().unwrap();
        state
            .add_message(&source_id, &msg(Role::User, "only on source"))
            .unwrap();
        state
            .add_message(&fork_id, &msg(Role::User, "only on fork"))
            .unwrap();
    }

    let source_count = state_store
        .lock()
        .unwrap()
        .get_chat_messages(&source_id)
        .unwrap()
        .len();
    let fork_count = state_store
        .lock()
        .unwrap()
        .get_chat_messages(&fork_id)
        .unwrap()
        .len();
    assert_eq!(source_count, 2, "source: seed + only-on-source");
    assert_eq!(fork_count, 2, "fork: seed (copied) + only-on-fork");
}
