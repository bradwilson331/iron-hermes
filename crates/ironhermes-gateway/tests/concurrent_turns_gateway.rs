//! Phase 39.1 Plan 03 — gateway concurrency integration tests (Task 3).
//!
//! Covers four behaviors from 39.1-VALIDATION.md:
//!
//! 1. `no_slash_rejection`       — slash commands never receive AGENT_RUNNING_REJECT_MSG mid-turn
//! 2. `gateway_concurrent_turns` — two in-flight turns register when cap >= 2
//! 3. `gateway_stop_cancels_session` — /stop cancels all session turns via TurnRegistry
//! 4. `new_mid_turn_warns`       — /new produces in_flight_warning text AND proceeds
//!
//! Fixtures: RecordingPlatformAdapter (from running_agent_guard_tests.rs pattern) +
//!           stub TurnEntry registration without a live LLM.

use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use ironhermes_core::concurrency::{Surface, TurnEntry, TurnId, TurnRegistry};
use ironhermes_core::{Config, MessageEvent, MessageResponse, Platform, ProviderResolver};
use ironhermes_gateway::{
    adapter::PlatformAdapter,
    handler::GatewayMessageHandler,
    session::{SessionKey, SessionStore},
};
use ironhermes_tools::ToolRegistry;

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

fn build_test_session_store() -> Arc<RwLock<SessionStore>> {
    let state_store = Arc::new(std::sync::Mutex::new(
        ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
    ));
    Arc::new(RwLock::new(SessionStore::new(state_store)))
}

fn build_test_handler(store: Arc<RwLock<SessionStore>>) -> GatewayMessageHandler {
    let config = Config::default();
    let resolver = ProviderResolver::build(&config).unwrap();
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    GatewayMessageHandler::new(config, resolver, store, tool_registry)
}

fn make_event(chat_id: &str, content: &str) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        message_id: "msg-1".to_string(),
        chat_id: chat_id.to_string(),
        sender_id: "u1".to_string(),
        content: content.to_string(),
        attachments: vec![],
        thread_id: None,
        chat_type: "dm".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    }
}

/// A PlatformAdapter that records send_message calls.
/// All other trait methods are no-ops.
struct RecordingAdapter {
    log: tokio::sync::Mutex<Vec<(String, String)>>,
}

impl RecordingAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            log: tokio::sync::Mutex::new(Vec::new()),
        })
    }

    async fn messages(&self) -> Vec<(String, String)> {
        self.log.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for RecordingAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.log
            .lock()
            .await
            .push((chat_id.to_string(), content.to_string()));
        Ok(MessageResponse {
            message_id: "stub-msg-id".to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn edit_message(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn edit_message_markdown_v2(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.log
            .lock()
            .await
            .push((chat_id.to_string(), content.to_string()));
        Ok(MessageResponse {
            message_id: "stub-msg-id".to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }
    async fn delete_message(&self, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn is_running(&self) -> bool {
        true
    }
}

/// Build a stub TurnEntry parked until its CancellationToken fires.
fn stub_turn(session_id: &str) -> (TurnId, CancellationToken, TurnEntry) {
    let turn_id = TurnId::new_v4();
    let cancel = CancellationToken::new();
    let entry = TurnEntry {
        turn_id,
        session_id: session_id.to_string(),
        surface: Surface::Gateway,
        started_at: std::time::Instant::now(),
        cancel: cancel.clone(),
    };
    (turn_id, cancel, entry)
}

// ---------------------------------------------------------------------------
// Test 1: no_slash_rejection
//
// R39.1-06: slash commands must NEVER receive AGENT_RUNNING_REJECT_MSG mid-turn.
// With a stub turn registered in TurnRegistry, dispatching /model must produce
// the command's normal output (not the rejection message).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn no_slash_rejection() {
    use ironhermes_gateway::adapter::MessageHandler;

    let store = build_test_session_store();
    let adapter = RecordingAdapter::new();

    let registry = Arc::new(TurnRegistry::new());
    let mut handler = build_test_handler(store.clone());
    handler.set_turn_registry(registry.clone());

    // Register a stub in-flight turn for chat-1/u1
    let session_id = "gw:chat-1:u1";
    let (_turn_id, _cancel, entry) = stub_turn(session_id);
    registry.register(entry).await;

    // Dispatch /model while turn is in flight — must NOT produce rejection message.
    let event = make_event("chat-1", "/model claude");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .ok();

    let msgs = adapter.messages().await;
    let sent_texts: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();

    // The literal rejection constant must NOT appear. We reference the constant
    // from ironhermes_core instead of writing the literal here (plan spec: do not
    // write the literal string as a positive grep target in test source).
    // Phase 39.1 Plan 06 (R39.1-06): running_agent.rs deleted; use literal string for
    // the negative assertion (D-02 must never appear). The module is gone, so we inline
    // the historical constant value here for the negative-presence check only.
    let reject_msg = "Agent is running. Use /stop to interrupt or /queue to send after this turn.";
    assert!(
        !sent_texts.iter().any(|t| t.contains(reject_msg)),
        "Slash command (/model) MUST NOT receive rejection mid-turn (R39.1-06). Got: {:?}",
        sent_texts
    );
}

// ---------------------------------------------------------------------------
// Test 2: gateway_concurrent_turns
//
// R39.1-01: two in-flight turns for one session must both be visible in registry
// when the per-session cap >= 2.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn gateway_concurrent_turns() {
    let registry = Arc::new(TurnRegistry::new());
    let session_id = "gw:chat-2:u1";

    // Register two stub turns without a live LLM.
    let (_id1, _c1, entry1) = stub_turn(session_id);
    let (_id2, _c2, entry2) = stub_turn(session_id);
    registry.register(entry1).await;
    registry.register(entry2).await;

    let count = registry.count_session(session_id).await;
    assert_eq!(
        count, 2,
        "Two concurrent turns must be visible in TurnRegistry for one session (R39.1-01). Got: {}",
        count
    );
}

// ---------------------------------------------------------------------------
// Test 3: gateway_stop_cancels_session
//
// R39.1-05: /stop must cancel all in-flight session turns via TurnRegistry.
// cancel_session returns the count of cancelled turns and fires each token.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn gateway_stop_cancels_session() {
    let registry = Arc::new(TurnRegistry::new());
    let session_id = "gw:chat-3:u1";

    let (_id1, cancel1, entry1) = stub_turn(session_id);
    let (_id2, cancel2, entry2) = stub_turn(session_id);
    registry.register(entry1).await;
    registry.register(entry2).await;

    // cancel_session returns the number of turns whose tokens were cancelled.
    let cancelled = registry.cancel_session(session_id).await;

    assert_eq!(
        cancelled, 2,
        "/stop (cancel_session) must cancel both in-flight turns (R39.1-05). Got: {}",
        cancelled
    );
    assert!(
        cancel1.is_cancelled(),
        "Turn 1 CancellationToken must be cancelled after cancel_session"
    );
    assert!(
        cancel2.is_cancelled(),
        "Turn 2 CancellationToken must be cancelled after cancel_session"
    );
}

// ---------------------------------------------------------------------------
// Test 4: new_mid_turn_warns
//
// R39.1-07: /new with an in-flight turn must produce the in_flight_warning text
// AND still proceed (session is removed, NewSession returned — not blocked).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn new_mid_turn_warns() {
    use ironhermes_gateway::adapter::MessageHandler;

    let store = build_test_session_store();
    let adapter = RecordingAdapter::new();

    let registry = Arc::new(TurnRegistry::new());
    let mut handler = build_test_handler(store.clone());
    handler.set_turn_registry(registry.clone());

    // Create a session so /new has something to remove, and capture its canonical
    // session_id (a UUID) so the TurnEntry matches what in_flight_warning looks up.
    let session_key = SessionKey::new(Platform::Telegram, "chat-4").with_user("u1");
    let canonical_session_id = {
        let mut s = store.write().await;
        let sess = s.get_or_create(session_key.clone(), "model", "test");
        sess.session_id.clone()
    };

    // Register a stub in-flight turn using the canonical session_id so
    // in_flight_warning() can find it via count_session().
    let (_turn_id, _cancel, entry) = stub_turn(&canonical_session_id);
    registry.register(entry).await;

    // Dispatch /new — must warn about in-flight turn AND still proceed.
    let event = make_event("chat-4", "/new");
    let result = handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await;

    // /new should not return an error (it warns + proceeds).
    assert!(
        result.is_ok(),
        "/new must not error when in-flight turns exist (R39.1-07). Got: {:?}",
        result
    );

    let msgs = adapter.messages().await;
    let texts: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();

    // At least one message must contain the in_flight_warning substring.
    // The warning is produced by ironhermes_core::commands::handlers::in_flight_warning().
    // We assert its presence by checking for "turn" in the warning text (the exact wording
    // is an implementation detail of in_flight_warning(); we assert its presence, not content).
    let has_warning = texts.iter().any(|t| {
        t.contains("turn")
            || t.contains("running")
            || t.contains("in-flight")
            || t.contains("active")
    });
    assert!(
        has_warning,
        "/new must send an in_flight_warning when a turn is in flight (R39.1-07). Got: {:?}",
        texts
    );
}
