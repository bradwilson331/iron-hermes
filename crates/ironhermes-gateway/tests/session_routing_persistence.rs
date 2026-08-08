//! Phase 36.17.9 — gateway session-routing persistence across restarts.
//!
//! Verifies that, with `persist_sessions` on, an inbound message after a
//! simulated gateway restart RESUMES its prior session (same session_id +
//! rehydrated history + voice mode) instead of minting a fresh one — and that
//! disabling the flag restores the legacy stateless behavior.

use std::sync::{Arc, Mutex};

use ironhermes_core::session::SessionKey;
use ironhermes_core::types::{
    ChatMessage, FunctionCall, MessageContent, Platform, Role, ToolCall,
};
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

/// Phase 47.5 (W4): an assistant message carrying a tool call, for building
/// mixed-role fixtures that exercise `resume_rehydration_head_is_user_aligned`.
fn assistant_tool_call_msg(tool_call_id: &str) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: None,
        tool_calls: Some(vec![ToolCall {
            id: tool_call_id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "noop".to_string(),
                arguments: "{}".to_string(),
            },
        }]),
        tool_call_id: None,
        name: None,
        is_recall_context: false,
    }
}

/// Phase 47.5 (W4): the `tool_result` completing an `assistant_tool_call_msg`'s
/// tool call — the message class that must never lead a rehydrated history.
fn tool_result_msg(tool_call_id: &str, text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::Tool,
        content: Some(MessageContent::Text(text.to_string())),
        tool_calls: None,
        tool_call_id: Some(tool_call_id.to_string()),
        name: None,
        is_recall_context: false,
    }
}

/// Open a SessionStore backed by the state.db at `db_path`. Each call opens a
/// fresh connection — calling it twice on the same path simulates a restart.
fn open_store(db_path: &std::path::Path, persist: bool) -> SessionStore {
    let state = Arc::new(Mutex::new(StateStore::new(db_path).expect("open state.db")));
    let mut store = SessionStore::new(state);
    store.set_persist_sessions(persist);
    store
}

#[test]
fn resume_across_restart_rehydrates_session_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let key = SessionKey::new(Platform::Telegram, "chat-1").with_user("u1");

    // ── First boot: create a session and exchange a message. ──
    let session_id_first;
    {
        let mut store = open_store(&db, true);
        let sess = store.get_or_create(key.clone(), "model-x", "telegram");
        session_id_first = sess.session_id.clone();
        store.add_message_to_session(&key, user_msg("hello from before the restart"));
    } // store + its StateStore connection dropped == process exit

    // ── Restart: a NEW store on the same db must resume the SAME session. ──
    {
        let mut store = open_store(&db, true);
        let sess = store.get_or_create(key.clone(), "model-x", "telegram");
        assert_eq!(
            sess.session_id, session_id_first,
            "restart must resume the prior session_id, not mint a fresh one"
        );
        assert_eq!(
            sess.messages.len(),
            1,
            "prior message history must be rehydrated on resume"
        );
        assert_eq!(
            sess.messages[0].content.as_ref().and_then(|c| c.as_text()),
            Some("hello from before the restart"),
            "the rehydrated message content must match"
        );
    }
}

#[test]
fn stateless_when_persist_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let key = SessionKey::new(Platform::Telegram, "chat-2");

    let id_first = {
        let mut store = open_store(&db, false);
        store
            .get_or_create(key.clone(), "m", "telegram")
            .session_id
            .clone()
    };
    let id_second = {
        let mut store = open_store(&db, false);
        store
            .get_or_create(key.clone(), "m", "telegram")
            .session_id
            .clone()
    };
    assert_ne!(
        id_first, id_second,
        "with persist_sessions=false, a restart must start a fresh session (legacy D-02)"
    );
}

#[test]
fn voice_mode_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let key = SessionKey::new(Platform::Discord, "room-7");

    // First boot: create the session, then set voice mode to "tts".
    {
        let mut store = open_store(&db, true);
        store.get_or_create(key.clone(), "m", "discord");
        assert!(
            store.set_voice_mode(&key, "tts"),
            "live session present → in-memory update returns true"
        );
        assert_eq!(store.voice_mode(&key).as_deref(), Some("tts"));
    }

    // Restart: the resumed session carries the persisted voice mode.
    {
        let mut store = open_store(&db, true);
        let sess = store.get_or_create(key.clone(), "m", "discord");
        assert_eq!(
            sess.voice_mode, "tts",
            "voice mode must survive a restart via gateway_routes"
        );
    }
}

/// Phase 47.5 (W4): a session with `MAX_RESUME_REHYDRATE_MESSAGES + 50`
/// persisted user messages resumes with AT MOST `MAX_RESUME_REHYDRATE_MESSAGES`
/// messages, and the LAST message is the most recently persisted one.
#[test]
fn resume_rehydration_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let key = SessionKey::new(Platform::Telegram, "chat-bounded");
    let total_seeded = SessionStore::MAX_RESUME_REHYDRATE_MESSAGES + 50;

    let session_id_first;
    {
        let mut store = open_store(&db, true);
        let sess = store.get_or_create(key.clone(), "model-x", "telegram");
        session_id_first = sess.session_id.clone();
        for i in 0..total_seeded {
            store.add_message_to_session(&key, user_msg(&format!("msg {i}")));
        }
    }

    {
        let mut store = open_store(&db, true);
        let sess = store.get_or_create(key.clone(), "model-x", "telegram");
        assert_eq!(
            sess.session_id, session_id_first,
            "restart must resume the prior session_id, not mint a fresh one"
        );
        assert_eq!(
            sess.messages.len(),
            SessionStore::MAX_RESUME_REHYDRATE_MESSAGES,
            "rehydrated history must be bounded to MAX_RESUME_REHYDRATE_MESSAGES (an \
             all-user fixture has no head-alignment loss, so it hits the cap exactly)"
        );
        let last = sess.messages.last().expect("at least one message");
        assert_eq!(
            last.content.as_ref().and_then(|c| c.as_text()),
            Some(format!("msg {}", total_seeded - 1)).as_deref(),
            "the rehydrated history's last message must be the most recently persisted one"
        );
    }
}

/// Phase 47.5 (W4), review MEDIUM: a mixed-role fixture (user / assistant-with-
/// tool-calls / tool_result triplets) whose truncation boundary lands MID
/// tool-pair. Proves the rehydrated head is advanced to the first `Role::User`
/// so no orphaned `tool_result`/assistant message leads the history. The
/// fixture asserts its OWN pre-truncation boundary role is not `Role::User`
/// before asserting the post-resume head IS `Role::User`, so it cannot
/// silently stop exercising the rule it exists to prove.
#[test]
fn resume_rehydration_head_is_user_aligned() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let key = SessionKey::new(Platform::Telegram, "chat-mixed");
    let max = SessionStore::MAX_RESUME_REHYDRATE_MESSAGES;

    // Triplet count chosen so the truncation boundary falls mid tool-pair
    // (on a tool_result or assistant message), never on the triplet's leading
    // user message.
    let n_triplets = (max + 50) / 3 + 1;
    let total_before = n_triplets * 3;
    assert!(
        total_before > max,
        "fixture must exceed the cap to exercise truncation at all"
    );

    // Self-check: the pre-truncation boundary role (the first message that
    // WOULD be retained by a naive front-drain, before head alignment) must
    // NOT be Role::User — otherwise this fixture degenerates into the
    // all-user case and never exercises the head-alignment rule.
    let drop_front = total_before - max;
    let boundary_role_in_triplet = drop_front % 3;
    assert_ne!(
        boundary_role_in_triplet, 0,
        "fixture self-check failed: pre-truncation boundary lands on a \
         triplet-leading user message (index {drop_front} of {total_before}) — \
         adjust n_triplets so the boundary lands mid tool-pair"
    );

    let last_result_text = format!("result {}", n_triplets - 1);

    {
        let mut store = open_store(&db, true);
        store.get_or_create(key.clone(), "model-x", "telegram");
        for i in 0..n_triplets {
            let call_id = format!("call-{i}");
            store.add_message_to_session(&key, user_msg(&format!("user {i}")));
            store.add_message_to_session(&key, assistant_tool_call_msg(&call_id));
            store.add_message_to_session(
                &key,
                tool_result_msg(&call_id, &format!("result {i}")),
            );
        }
    }

    {
        let mut store = open_store(&db, true);
        let sess = store.get_or_create(key.clone(), "model-x", "telegram");
        assert!(
            sess.messages.len() <= max,
            "rehydrated history must be bounded, got {}",
            sess.messages.len()
        );
        assert_eq!(
            sess.messages[0].role,
            Role::User,
            "rehydrated head must be advanced to the first Role::User message — \
             no orphaned tool_result or mid-pair assistant message may lead"
        );
        let last = sess.messages.last().expect("at least one message");
        assert_eq!(
            last.content.as_ref().and_then(|c| c.as_text()),
            Some(last_result_text.as_str()),
            "the rehydrated history's last message must be the most recently persisted one"
        );
    }
}
