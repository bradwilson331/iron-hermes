//! Phase 25.3-14 verifier-blocker regression test: gateway sessions persist
//! `workspace_root` onto the SQLite `sessions` row when SessionStore was given
//! a Workspace via `set_workspace`.
//!
//! Locks the contract that `25.3-VERIFICATION.md` FAILED on at must-have #28.
//! Without this test, a future refactor could re-introduce the hardcoded-None
//! regression that shipped with Plan 8 and was caught by the verifier — the
//! parity-guard tests in `invariants_25_3.rs` (INV-25.3-09 / INV-25.3-11)
//! lock the source-text shape, but ONLY this test exercises the actual
//! round-trip into SQLite.
//!
//! Coverage:
//! - `gateway_session_with_workspace_persists_workspace_root` — the happy path:
//!   SessionStore given a Workspace via `set_workspace`, then `get_or_create`
//!   inserts a row, and the SQLite row has `workspace_root = Some(<root>)`.
//! - `gateway_session_without_workspace_persists_null` — the global-mode path:
//!   SessionStore created without `set_workspace`, `get_or_create` inserts a
//!   row, and the SQLite row has `workspace_root = None`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ironhermes_core::Platform;
use ironhermes_core::workspace::Workspace;
use ironhermes_gateway::session::{SessionKey, SessionStore};
use ironhermes_state::StateStore;

/// Open a fresh SQLite state store inside `dir/state.db`. Wrapped in
/// `Arc<Mutex<_>>` to match the production `SessionStore::new` signature.
fn fresh_state(dir: &std::path::Path) -> Arc<Mutex<StateStore>> {
    let db_path = dir.join("state.db");
    let store = StateStore::new(&db_path).expect("open fresh state.db");
    Arc::new(Mutex::new(store))
}

/// Construct a minimal `Workspace` by hand. We don't need `resolve_from_cwd`
/// since this test exercises the persistence path, not the resolution path.
fn synthetic_workspace(root: PathBuf) -> Workspace {
    Workspace {
        soul_path: None,
        agents_chain: vec![],
        memory_dir: root.join(".ironhermes/memory"),
        skills_dir: root.join("skills"),
        tools_config: None,
        root,
    }
}

#[test]
fn gateway_session_with_workspace_persists_workspace_root() {
    let tmp = tempfile::tempdir().expect("mk tempdir");
    let proj = tmp.path().join("myproj");
    std::fs::create_dir_all(&proj).expect("mkdir proj");
    std::fs::create_dir_all(proj.join(".ironhermes")).expect("mkdir .ironhermes");

    let state = fresh_state(tmp.path());
    let mut store = SessionStore::new(state.clone());
    let ws = Arc::new(synthetic_workspace(proj.clone()));
    store.set_workspace(ws.clone());

    let key = SessionKey::new(Platform::Telegram, "chat-1".to_string());
    let session_id = {
        let sess = store.get_or_create(key, "test-model", "telegram");
        sess.session_id.clone()
    };

    // Read back from SQLite — the row must carry workspace_root.
    let row = {
        let state_locked = state.lock().expect("lock state");
        state_locked
            .get_session(&session_id)
            .expect("query session")
            .expect("session row exists")
    };

    let expected = proj.to_str().expect("UTF-8 tempdir path").to_string();
    assert_eq!(
        row.workspace_root,
        Some(expected),
        "Phase 25.3-14: gateway sessions with a resolved Workspace MUST persist \
         workspace_root onto the sessions row. Without this, /sessions --workspace \
         and Phase 25.4 Curator are starved on the primary user-facing surface."
    );
}

#[test]
fn gateway_session_without_workspace_persists_null() {
    let tmp = tempfile::tempdir().expect("mk tempdir");
    let state = fresh_state(tmp.path());
    let mut store = SessionStore::new(state.clone());
    // Intentionally NOT calling set_workspace — simulates the global-mode
    // gateway launched outside any workspace.

    let key = SessionKey::new(Platform::Telegram, "chat-2".to_string());
    let session_id = {
        let sess = store.get_or_create(key, "test-model", "telegram");
        sess.session_id.clone()
    };

    let row = {
        let state_locked = state.lock().expect("lock state");
        state_locked
            .get_session(&session_id)
            .expect("query session")
            .expect("session row exists")
    };

    assert_eq!(
        row.workspace_root, None,
        "Phase 25.3-14: gateway sessions with no resolved Workspace MUST persist \
         workspace_root=NULL — global-mode behavior."
    );
}
