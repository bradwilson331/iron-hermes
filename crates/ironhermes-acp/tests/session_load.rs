//! `session/load` tests (Phase 36.8, plan 02, task 2): the pure bounded head-aligned
//! rehydration algorithm (D-11) plus `AcpSessionManager::load`'s idempotency, unknown-id
//! error, and cwd-rebind contract.
//!
//! Plan 06 UAT gap fix (found via live Zed UAT, task 3 checkpoint step 12): the tests
//! above only ever asserted on `AcpSessionManager::load`'s in-memory `messages` buffer —
//! never on what actually reaches the wire. A real ACP client (Zed) showed an EMPTY
//! conversation after `session/load` even though the manager-level rehydration was
//! correct, because `handle_session_load` never emitted any `session/update`
//! notifications at all. The protocol-level test below drives `session/load` through the
//! real `run_acp_over` entry point over an in-memory duplex and asserts the
//! editor-observable contract: `user_message_chunk`/`agent_message_chunk` replay
//! notifications for prior history arrive before the `session/load` response.

use std::sync::{Arc, Mutex as StdMutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, LoadSessionRequest, SessionNotification, SessionUpdate,
};
use agent_client_protocol::{Channel, Client};
use ironhermes_acp::session_manager::{
    ACP_SESSION_SOURCE, AcpSessionManager, MAX_RESUME_REHYDRATE_MESSAGES,
    bounded_head_aligned_rehydrate,
};
use ironhermes_core::{
    ChatMessage, Config, MessageContent, ModelsCache, ProviderConfig, ProviderResolver, Role,
};
use ironhermes_state::StateStore;

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

// ── pure `bounded_head_aligned_rehydrate` tests ─────────────────────────────────────

#[test]
fn rehydrate_under_cap_is_unchanged() {
    let messages: Vec<ChatMessage> = (0..5).map(|i| msg(Role::User, &format!("m{i}"))).collect();
    let expected_texts: Vec<Option<String>> = messages
        .iter()
        .map(|m| m.content.as_ref().and_then(|c| c.as_text().map(str::to_owned)))
        .collect();
    let result = bounded_head_aligned_rehydrate(messages);
    assert_eq!(
        result.len(),
        5,
        "a session with 5 stored messages must rehydrate all 5"
    );
    let result_texts: Vec<Option<String>> = result
        .iter()
        .map(|m| m.content.as_ref().and_then(|c| c.as_text().map(str::to_owned)))
        .collect();
    assert_eq!(
        result_texts, expected_texts,
        "under-cap rehydration must be byte-identical"
    );
}

#[test]
fn rehydrate_250_messages_retains_at_most_200_head_aligned_to_user() {
    // Alternating Assistant/User starting with Assistant (even index = Assistant, odd =
    // User) so the drop-front boundary (250-200=50, an even index) lands ON an Assistant
    // message — exercising the head-alignment trim, not just the bound.
    let messages: Vec<ChatMessage> = (0..250)
        .map(|i| {
            if i % 2 == 0 {
                msg(Role::Assistant, &format!("a{i}"))
            } else {
                msg(Role::User, &format!("u{i}"))
            }
        })
        .collect();
    let result = bounded_head_aligned_rehydrate(messages);
    assert!(
        result.len() <= MAX_RESUME_REHYDRATE_MESSAGES,
        "rehydrated length must never exceed the {MAX_RESUME_REHYDRATE_MESSAGES}-message bound, got {}",
        result.len()
    );
    assert_eq!(
        result.first().expect("non-empty result").role,
        Role::User,
        "the first retained message after head-alignment must be a user message"
    );
}

#[test]
fn rehydrate_window_with_no_user_message_retains_nothing() {
    let messages: Vec<ChatMessage> = (0..250)
        .map(|i| msg(Role::Assistant, &format!("a{i}")))
        .collect();
    let result = bounded_head_aligned_rehydrate(messages);
    assert!(
        result.is_empty(),
        "a retained window with no Role::User must clear to empty rather than an orphaned head"
    );
}

// ── `AcpSessionManager::load` integration tests ─────────────────────────────────────

#[tokio::test]
async fn load_unknown_session_id_returns_error() {
    let (mut manager, _state_tmp) = build_manager();
    let dir = tempfile::tempdir().expect("tempdir");
    let result = manager.load("does-not-exist", dir.path().to_path_buf()).await;
    assert!(
        result.is_err(),
        "loading an unknown session id must return an error, not silently create one"
    );
}

#[tokio::test]
async fn load_is_idempotent_for_a_live_session() {
    let (mut manager, _state_tmp) = build_manager();
    let dir = tempfile::tempdir().expect("tempdir");
    let id = manager
        .create(dir.path().to_path_buf(), None)
        .await
        .expect("create session");

    manager
        .load(&id, dir.path().to_path_buf())
        .await
        .expect("loading an already-live session must succeed as a no-op");
    assert!(manager.get(&id).is_some());
}

#[tokio::test]
async fn load_rehydrates_a_removed_session_from_state_store_and_rebinds_cwd() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();
    let mut manager = AcpSessionManager::new(state_store.clone(), config, resolver);

    let dir_original = tempfile::tempdir().expect("tempdir original");
    let id = manager
        .create(dir_original.path().to_path_buf(), None)
        .await
        .expect("create session");

    // Seed 5 stored messages directly via StateStore (prior conversation turns).
    {
        let mut state = state_store.lock().unwrap();
        for i in 0..5 {
            let role = if i % 2 == 0 { Role::User } else { Role::Assistant };
            state
                .add_message(&id, &msg(role, &format!("message {i}")))
                .expect("add_message");
        }
    }

    // Simulate the editor closing the session: the live in-memory session goes away,
    // but the StateStore row + messages remain (remove() ends the row, it does not
    // delete it).
    assert!(manager.remove(&id));
    assert!(manager.get(&id).is_none());

    // Editor reopens the project from a DIFFERENT path (D-12).
    let dir_new = tempfile::tempdir().expect("tempdir new");
    manager
        .load(&id, dir_new.path().to_path_buf())
        .await
        .expect("load of a known, removed session must succeed");

    let session = manager.get(&id).expect("session must be live after load");
    assert_eq!(
        session.messages.len(),
        5,
        "loading a session with 5 stored messages rehydrates all 5"
    );
    assert_eq!(
        session.cwd,
        dir_new.path(),
        "load must rebind the runtime to the cwd supplied on the load request"
    );
}

// ── protocol-level `session/load` replay test (plan 06 UAT gap fix) ────────────────

/// One event observed by the test client, in wire-arrival order: either a `session/update`
/// notification, or the terminal marker that the `session/load` JSON-RPC response itself
/// has arrived.
#[derive(Debug)]
enum ReplayEvent {
    // Boxed: `SessionUpdate` is far larger than the data-less `LoadResponded` variant
    // (clippy::large_enum_variant) — this enum only ever holds a handful of events in a
    // test-local Vec, so the extra indirection costs nothing that matters here.
    Update(Box<SessionUpdate>),
    LoadResponded,
}

/// Drives `session/load` through the REAL `run_acp_over` entry point over an in-memory
/// duplex (no manager-level shortcut) and asserts the editor-observable contract a real
/// Zed client depends on: replay `session/update` notifications for the session's prior
/// user AND agent turns arrive on the wire strictly BEFORE the `session/load` JSON-RPC
/// response. This is exactly the assertion the plan-02 manager-level tests above never
/// made — they only ever checked `AcpSession.messages` (an internal buffer), not what
/// reaches a real client, which is why they stayed green while live Zed showed an empty
/// conversation.
#[tokio::test]
async fn session_load_replays_user_and_agent_history_before_responding() {
    let (config, resolver) = build_config_and_resolver();
    let (state_store, _state_tmp) = build_state_store();

    let session_id = "acp_replay-test-session".to_string();
    let original_dir = tempfile::tempdir().expect("tempdir for seeded session cwd");
    {
        let mut state = state_store.lock().unwrap();
        state
            .create_session(
                &session_id,
                ACP_SESSION_SOURCE,
                None,
                None,
                None,
                Some(&original_dir.path().to_string_lossy()),
            )
            .expect("create_session");
        state
            .add_message(&session_id, &msg(Role::User, "what does this project do?"))
            .expect("add stored user message");
        state
            .add_message(
                &session_id,
                &msg(Role::Assistant, "It is an ACP adapter for editors."),
            )
            .expect("add stored assistant message");
    }

    let (agent_channel, client_channel) = Channel::duplex();
    let agent_task = tokio::spawn(async move {
        ironhermes_acp::entry::run_acp_over(config, resolver, state_store, agent_channel).await
    });

    let load_dir = tempfile::tempdir().expect("tempdir for load cwd");
    let load_path = load_dir.path().to_path_buf();
    let session_id_for_load = session_id.clone();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ReplayEvent>();
    let notif_tx = tx.clone();

    let client_result = Client
        .builder()
        .name("acp-session-load-replay-test-client")
        .on_receive_notification(
            move |notif: SessionNotification, _cx| {
                let notif_tx = notif_tx.clone();
                async move {
                    let _ = notif_tx.send(ReplayEvent::Update(Box::new(notif.update)));
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(client_channel, async move |cx| {
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let resp_tx = tx.clone();
            cx.send_request(LoadSessionRequest::new(session_id_for_load, load_path))
                .on_receiving_result(async move |result| {
                    result?;
                    let _ = resp_tx.send(ReplayEvent::LoadResponded);
                    Ok(())
                })?;

            // Drain events until the load-response marker arrives, timing out (rather
            // than hanging CI) if it never does.
            let mut events = Vec::new();
            loop {
                let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                    .await
                    .expect(
                        "timed out waiting for session/load replay events — response never \
                         arrived",
                    )
                    .expect("replay event channel closed unexpectedly");
                let is_response = matches!(event, ReplayEvent::LoadResponded);
                events.push(event);
                if is_response {
                    break;
                }
            }

            // Every element up to (but not including) the final LoadResponded marker is,
            // by construction of the loop above, a session/update notification that
            // arrived strictly before the session/load response — exactly the
            // editor-observable ordering contract this test exists to prove.
            let (pre_response, terminal) = events.split_at(events.len() - 1);
            assert!(
                matches!(terminal[0], ReplayEvent::LoadResponded),
                "the last collected event must be the LoadResponded marker"
            );

            let user_texts: Vec<String> = pre_response
                .iter()
                .filter_map(|e| match e {
                    ReplayEvent::Update(update) => match update.as_ref() {
                        SessionUpdate::UserMessageChunk(chunk) => match &chunk.content {
                            ContentBlock::Text(t) => Some(t.text.clone()),
                            _ => None,
                        },
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            let agent_texts: Vec<String> = pre_response
                .iter()
                .filter_map(|e| match e {
                    ReplayEvent::Update(update) => match update.as_ref() {
                        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                            ContentBlock::Text(t) => Some(t.text.clone()),
                            _ => None,
                        },
                        _ => None,
                    },
                    _ => None,
                })
                .collect();

            assert!(
                user_texts
                    .iter()
                    .any(|t| t.contains("what does this project do")),
                "expected a user_message_chunk replaying the stored user turn BEFORE the \
                 session/load response, got pre-response events: {pre_response:?}"
            );
            assert!(
                agent_texts.iter().any(|t| t.contains("ACP adapter")),
                "expected an agent_message_chunk replaying the stored assistant turn BEFORE \
                 the session/load response, got pre-response events: {pre_response:?}"
            );

            Ok(())
        })
        .await;

    client_result.expect("session/load replay exchange should succeed");
    agent_task.abort();
}
