//! Phase 47.5 Plan 01 (D-04, D-07) — durable `/new` session reset persistence.
//!
//! D-07: this is the RED-first harness (the phase's TRACER). Proves that
//! `SessionStore::reset_session` — replacing the memory-only `remove` — makes
//! `/new` durable: no resume across a simulated restart, from EITHER state
//! source. Test 3 is the review consensus-HIGH finding: a durable-only reset
//! (nothing in the in-memory map, e.g. `/new` as the first message after a
//! gateway restart) must still end the session and clear the route — this is
//! the incident's exact signature (245-message resurrection after
//! "Conversation cleared. Starting fresh.").

use std::sync::{Arc, Mutex};

use ironhermes_core::session::SessionKey;
use ironhermes_core::types::{ChatMessage, MessageContent, Platform, Role};
use ironhermes_gateway::session::SessionStore;
use ironhermes_state::StateStore;

fn user_msg(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: Some(MessageContent::Text(text.to_string())),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        is_recall_context: false,
    }
}

/// Open a SessionStore backed by the state.db at `db_path`. Each call opens a
/// fresh connection — calling it twice on the same path simulates a restart.
fn open_store(db_path: &std::path::Path) -> SessionStore {
    let state = Arc::new(Mutex::new(StateStore::new(db_path).expect("open state.db")));
    SessionStore::new(state)
}

#[test]
fn new_command_prevents_resume_and_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let key = SessionKey::new(Platform::Telegram, "chat-reset-1").with_user("u1");

    // Boot 1: create a session, add an off-topic message, reset it exactly as
    // the `/new` arm will, then simulate a process exit.
    let session_id_first;
    {
        let mut store = open_store(&db);
        let sess = store.get_or_create(key.clone(), "model-x", "telegram");
        session_id_first = sess.session_id.clone();
        store.add_message_to_session(&key, user_msg("off-topic reel URL"));
        assert!(
            store.reset_session(&key, "new"),
            "a session existed in memory — reset must report true"
        );
    } // store + its StateStore connection dropped == process exit

    // Boot 2: the SAME key must mint a FRESH session with EMPTY history — not
    // resume the reset-away one (the incident's 245-message resurrection).
    {
        let mut store = open_store(&db);
        let sess = store.get_or_create(key.clone(), "model-x", "telegram");
        assert_ne!(
            sess.session_id, session_id_first,
            "a reset session must never be resumed — a fresh id is required"
        );
        assert!(
            sess.messages.is_empty(),
            "the reset session's history must NOT resurface after restart"
        );
    }
}

#[test]
fn new_command_marks_session_ended_and_deletes_route() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let key = SessionKey::new(Platform::Telegram, "chat-reset-2").with_user("u2");
    let string_key = key.to_string_key();

    let session_id_first;
    {
        let state = Arc::new(Mutex::new(StateStore::new(&db).expect("open state.db")));
        let mut store = SessionStore::new(state.clone());
        let sess = store.get_or_create(key.clone(), "model-x", "telegram");
        session_id_first = sess.session_id.clone();

        // D-VOICE: set a non-default voice mode BEFORE the reset — it must
        // survive as the resume-inert placeholder row the reset re-creates.
        state
            .lock()
            .unwrap()
            .set_route_voice_mode(&string_key, "on")
            .unwrap();

        assert!(store.reset_session(&key, "new"));
    } // simulated process exit

    // Open a StateStore directly on the same db to inspect durable state.
    let state = StateStore::new(&db).expect("open state.db");

    let row = state
        .get_session(&session_id_first)
        .unwrap()
        .expect("old session row must still exist — /new ENDS, never deletes (archive substrate)");
    assert!(row.ended_at.is_some(), "old session must be marked ended");
    assert_eq!(
        row.end_reason.as_deref(),
        Some("new"),
        "end_reason must record which command reset the session"
    );

    let route = state.get_route(&string_key).unwrap();
    let resolves_to_old = route
        .as_ref()
        .map(|r| r.session_id == session_id_first)
        .unwrap_or(false);
    assert!(
        !resolves_to_old,
        "the durable route must no longer resolve to the reset session"
    );

    // D-VOICE: voice mode survives the reset via the empty-session_id
    // placeholder row.
    let voice_mode = route.map(|r| r.voice_mode);
    assert_eq!(
        voice_mode.as_deref(),
        Some("on"),
        "voice_mode must survive /new (D-VOICE — a chat preference, not session content)"
    );
}

#[test]
fn new_command_resets_durable_only_session() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let key = SessionKey::new(Platform::Telegram, "chat-reset-3").with_user("u3");
    let string_key = key.to_string_key();
    let session_id_first = "session-durable-only-1".to_string();

    // Seed durable-only state directly at the StateStore level (mirrors what
    // an ordinary get_or_create + restart would have produced) WITHOUT ever
    // calling `SessionStore::get_or_create` — this test's whole point is that
    // `reset_session` must work when the in-memory map has never been
    // touched (the post-restart `/new` case).
    {
        let mut state = StateStore::new(&db).expect("open state.db");
        state
            .create_session(
                &session_id_first,
                "telegram",
                Some("model-x"),
                None,
                None,
                None,
            )
            .unwrap();
        state.upsert_route(&string_key, &session_id_first).unwrap();
        state
            .add_message(&session_id_first, &user_msg("pre-restart message"))
            .unwrap();
    } // simulated restart — connection dropped

    // Boot 2: a FRESH SessionStore with an EMPTY in-memory map. Call
    // `reset_session` FIRST — before any `get_or_create` — so only durable
    // state exists. This is the incident's exact signature: `/new` as the
    // first message after a restart.
    let mut store = open_store(&db);
    let had_session = store.reset_session(&key, "new");
    assert!(
        had_session,
        "durable-only reset must report true (truthful had_session) — \
         the handler branches its reply text on this bool"
    );

    {
        let state = StateStore::new(&db).expect("open state.db");
        let row = state
            .get_session(&session_id_first)
            .unwrap()
            .expect("old session row must still exist");
        assert!(row.ended_at.is_some(), "old session must be marked ended");
        assert_eq!(row.end_reason.as_deref(), Some("new"));
    }

    let sess = store.get_or_create(key.clone(), "model-x", "telegram");
    assert_ne!(
        sess.session_id, session_id_first,
        "the durable-only reset must not be resumable"
    );
    assert!(sess.messages.is_empty());
}
