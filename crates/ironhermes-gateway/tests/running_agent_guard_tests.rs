//! Phase 36 GW-05 running-agent guard integration tests.
//!
//! All 11 sub-behaviors from 36-VALIDATION.md are tested here.
//! Plan 36-02 implements the production guard; this file makes them live.
//!
//! ## GW-05 sub-behaviors covered
//!
//! 1. `test_session_isolation` — per-session state: session A Running does not block session B (Idle)
//! 2. `test_model_rejected_when_running` — `/model` rejected during active turn (D-04, D-02)
//! 3. `test_stop_bypasses_guard` — `/stop` dispatches even when flag is true (D-01)
//! 4. `test_new_bypasses_guard` — `/new` dispatches even when flag is true (D-01)
//! 5. `test_status_bypasses_guard` — `/status` dispatches even when flag is true (D-01)
//! 6. `test_queue_bypasses_guard` — `/queue` dispatches even when flag is true (D-01)
//! 7. `test_guard_clears_on_success` — flag clears on `run_agent` returning `Ok(...)` (D-06)
//! 8. `test_guard_clears_on_error` — flag clears on `run_agent` returning `Err(...)` (D-06)
//! 9. `test_alias_bypasses_guard` — `/reset` (alias for `new`) bypasses guard (D-01)
//! 10. `test_freetext_rejected_when_running` — non-slash free-text rejected during active turn (Pitfall 1)
//! 11. `test_stop_reads_real_flag` — `cmd_stop` reads a non-false `agent_running` on gateway (integration)

pub mod helpers {
    use std::sync::Arc;
    use tokio::sync::{Mutex as TokioMutex, RwLock};

    use anyhow::Result;
    use async_trait::async_trait;
    use ironhermes_core::{Config, MessageEvent, MessageResponse, Platform, ProviderResolver};
    use ironhermes_gateway::{
        handler::GatewayMessageHandler,
        session::{SessionKey, SessionStore},
    };
    use ironhermes_tools::ToolRegistry;

    /// Build an in-memory `SessionStore` wrapped in `Arc<RwLock<...>>`, mirroring
    /// the pattern at `handler.rs:1261`. Callers may inject sessions and flip flags
    /// directly through the returned handle.
    pub fn build_test_session_store() -> Arc<RwLock<SessionStore>> {
        let state_store = Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
        ));
        Arc::new(RwLock::new(SessionStore::new(state_store)))
    }

    /// A `PlatformAdapter` that records every `send_message` call as a
    /// `(chat_id, text)` tuple so tests can assert on sent messages without
    /// a live Telegram connection.
    ///
    /// All other trait methods are no-ops that return `Ok(Default::default())`.
    pub struct RecordingPlatformAdapter {
        log: TokioMutex<Vec<(String, String)>>,
    }

    impl RecordingPlatformAdapter {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                log: TokioMutex::new(Vec::new()),
            })
        }

        /// Return a snapshot of all recorded `(chat_id, text)` pairs.
        pub async fn messages(&self) -> Vec<(String, String)> {
            self.log.lock().await.clone()
        }
    }

    #[async_trait]
    impl ironhermes_gateway::adapter::PlatformAdapter for RecordingPlatformAdapter {
        fn platform(&self) -> Platform {
            Platform::Telegram
        }

        async fn send_message(
            &self,
            chat_id: &str,
            content: &str,
            _thread_id: Option<&str>,
        ) -> Result<MessageResponse> {
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

        async fn edit_message(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _content: &str,
        ) -> Result<()> {
            Ok(())
        }

        async fn edit_message_markdown_v2(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _content: &str,
        ) -> Result<()> {
            Ok(())
        }

        async fn send_message_markdown_v2(
            &self,
            chat_id: &str,
            content: &str,
            _thread_id: Option<&str>,
        ) -> Result<MessageResponse> {
            // Phase 36.17.2.2-05: record into the same `log` as plain
            // send_message so the GW-05 guard tests' assertions on sent
            // messages still hold if a future overflow-chunk path lands
            // on this adapter.
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

        async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> Result<()> {
            Ok(())
        }

        fn is_running(&self) -> bool {
            true
        }
    }

    /// Build a `GatewayMessageHandler` for testing, replicating the construction
    /// logic from `handler.rs:1226` (`make_handler()` is `pub(crate)` and not
    /// reachable from integration tests — so we replicate it here).
    ///
    /// Source pattern: `crates/ironhermes-gateway/src/handler.rs:1226`
    pub fn build_test_handler(store: Arc<RwLock<SessionStore>>) -> GatewayMessageHandler {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        GatewayMessageHandler::new(config, resolver, store, tool_registry)
    }

    /// Construct a canonical test `SessionKey` for `chat_id` with a fixed user `u1`.
    pub fn test_session_key(chat_id: &str) -> SessionKey {
        SessionKey::new(Platform::Telegram, chat_id).with_user("u1")
    }

    /// Build a `MessageEvent` for the given chat + content.
    pub fn make_event(chat_id: &str, content: &str) -> MessageEvent {
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

    /// The D-02 error message — HISTORICAL (Phase 39.1 Plan 06).
    ///
    /// `running_agent.rs` is deleted in Plan 06. This helper is retained for
    /// backward-compat with assertions in this file that check the message
    /// is NOT sent (negative assertions remain valid — D-02 must never appear).
    pub fn d02_error_message() -> &'static str {
        "Agent is running. Use /stop to interrupt or /queue to send after this turn."
    }
}

// ---------------------------------------------------------------------------
// GW-05 sub-behavior tests
// ---------------------------------------------------------------------------

use std::sync::atomic::Ordering;

/// GW-05-1: Per-session isolation (Phase 39.1 updated).
///
/// Phase 39.1 removes all agent_running gate sites (R39.1-06). Neither session A
/// nor session B receives a D-02 rejection — commands always dispatch regardless
/// of whether a turn is in flight. This test verifies the gate is gone.
#[tokio::test]
async fn test_session_isolation() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key_a = helpers::test_session_key("chat-A");
    let key_b = helpers::test_session_key("chat-B");

    {
        let mut s = store.write().await;
        s.get_or_create(key_a.clone(), "model", "test");
        s.get_or_create(key_b.clone(), "model", "test");
    }

    // Set session A running (simulates in-flight turn on session A).
    {
        let s = store.read().await;
        s.get(&key_a).unwrap().running.store(true, Ordering::SeqCst);
    }

    // Dispatch /model to session B — must NOT receive D-02 rejection.
    let event_b = helpers::make_event("chat-B", "/model claude");
    handler
        .handle(&event_b, adapter.clone(), CancellationToken::new())
        .await
        .ok();

    let msgs = adapter.messages().await;
    let sent_texts: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent_texts.contains(&helpers::d02_error_message()),
        "Session B must NOT be rejected with D-02. Got: {:?}",
        sent_texts
    );

    // Phase 39.1 (R39.1-06): session A also must NOT receive D-02 — gate is removed.
    let adapter_a = helpers::RecordingPlatformAdapter::new();
    let event_a = helpers::make_event("chat-A", "/model claude");
    handler
        .handle(&event_a, adapter_a.clone(), CancellationToken::new())
        .await
        .ok();

    let msgs_a = adapter_a.messages().await;
    let sent_a: Vec<&str> = msgs_a.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent_a.contains(&helpers::d02_error_message()),
        "Session A must NOT be rejected with D-02 — all gates removed (R39.1-06). Got: {:?}",
        sent_a
    );
}

/// GW-05-2: `/model` dispatches when agent is running (Phase 39.1 updated).
///
/// Phase 39.1 removes the D-04 gate (R39.1-06). `/model` during an active turn
/// must NOT receive D-02 — the command always dispatches.
#[tokio::test]
async fn test_model_rejected_when_running() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key = helpers::test_session_key("chat-1");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }

    // Set running = true.
    {
        let s = store.read().await;
        s.get(&key).unwrap().running.store(true, Ordering::SeqCst);
    }

    let event = helpers::make_event("chat-1", "/model gpt-4");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .ok();

    let msgs = adapter.messages().await;
    let sent: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent.contains(&helpers::d02_error_message()),
        "/model must NOT be rejected with D-02 — gate removed (R39.1-06). Got: {:?}",
        sent
    );
}

/// GW-05-3: `/stop` bypasses the guard when agent is running.
///
/// Session flag is `true`. Sending `/stop` must dispatch (not be rejected with D-02).
#[tokio::test]
async fn test_stop_bypasses_guard() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key = helpers::test_session_key("chat-1");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    {
        let s = store.read().await;
        s.get(&key).unwrap().running.store(true, Ordering::SeqCst);
    }

    let event = helpers::make_event("chat-1", "/stop");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .unwrap();

    let msgs = adapter.messages().await;
    let sent: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent.contains(&helpers::d02_error_message()),
        "/stop must NOT receive D-02; guard must bypass. Got: {:?}",
        sent
    );
}

/// GW-05-4: `/new` bypasses the guard when agent is running.
#[tokio::test]
async fn test_new_bypasses_guard() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key = helpers::test_session_key("chat-1");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    {
        let s = store.read().await;
        s.get(&key).unwrap().running.store(true, Ordering::SeqCst);
    }

    let event = helpers::make_event("chat-1", "/new");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .unwrap();

    let msgs = adapter.messages().await;
    let sent: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent.contains(&helpers::d02_error_message()),
        "/new must NOT receive D-02; guard must bypass. Got: {:?}",
        sent
    );
}

/// GW-05-5: `/status` bypasses the guard when agent is running.
#[tokio::test]
async fn test_status_bypasses_guard() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key = helpers::test_session_key("chat-1");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    {
        let s = store.read().await;
        s.get(&key).unwrap().running.store(true, Ordering::SeqCst);
    }

    let event = helpers::make_event("chat-1", "/status");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .unwrap();

    let msgs = adapter.messages().await;
    let sent: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent.contains(&helpers::d02_error_message()),
        "/status must NOT receive D-02; guard must bypass. Got: {:?}",
        sent
    );
}

/// GW-05-6: `/queue` bypasses the guard when agent is running.
#[tokio::test]
async fn test_queue_bypasses_guard() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key = helpers::test_session_key("chat-1");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    {
        let s = store.read().await;
        s.get(&key).unwrap().running.store(true, Ordering::SeqCst);
    }

    let event = helpers::make_event("chat-1", "/queue");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .unwrap();

    let msgs = adapter.messages().await;
    let sent: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent.contains(&helpers::d02_error_message()),
        "/queue must NOT receive D-02; guard must bypass. Got: {:?}",
        sent
    );
}

// GW-05-7: REMOVED in Phase 39.1 Plan 06.
//
// `RunningAgentGuard` and the `agent_running` AtomicBool are deleted as part of
// Plan 06 (R39.1-06 / D-06). The guard tests are no longer applicable.
// The TurnRegistry (Plan 01) owns turn-lifecycle tracking going forward.
// Structural equivalent: `test_deregister_clears_entry_on_turn_end` in
// `running_agent_guard_tui_tests.rs` (Plan 04).

// GW-05-8: REMOVED in Phase 39.1 Plan 06.
//
// `RunningAgentGuard` deleted — see GW-05-7 note above.

/// GW-05-9: Alias `/reset` (resolves to canonical `"new"`) bypasses the guard.
///
/// `CommandRouter::resolve("/reset")` returns `Exact(def)` where `def.name == "new"`.
/// The guard checks `resolved_def.name` (post-alias), so `/reset` must bypass because
/// `is_bypass("new") == true` (Pitfall 4 mitigation).
#[tokio::test]
async fn test_alias_bypasses_guard() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key = helpers::test_session_key("chat-1");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    {
        let s = store.read().await;
        s.get(&key).unwrap().running.store(true, Ordering::SeqCst);
    }

    // /reset is an alias for "new" in the command registry.
    // If the alias is not registered, the command falls through to PassThrough/NotFound
    // and run_agent is skipped (run_agent guard blocks it anyway).
    // Either way, D-02 must NOT be sent.
    let event = helpers::make_event("chat-1", "/reset");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .ok(); // ignore any run_agent error; we care about rejection presence

    let msgs = adapter.messages().await;
    let sent: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent.contains(&helpers::d02_error_message()),
        "/reset (alias -> new) must NOT receive D-02 (guard uses def.name post-alias). Got: {:?}",
        sent
    );
}

/// GW-05-10: Non-slash free-text message dispatches when agent is running (Phase 39.1 updated).
///
/// Phase 39.1 removes the free-text gate (R39.1-06). A plain-text message during an
/// active turn must NOT be rejected with D-02 — concurrent turns are now supported.
/// The message attempts to run the agent (which requires AgentRuntime; without it
/// the handler returns an error, but D-02 must never be sent).
#[tokio::test]
async fn test_freetext_rejected_when_running() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key = helpers::test_session_key("chat-1");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    {
        let s = store.read().await;
        s.get(&key).unwrap().running.store(true, Ordering::SeqCst);
    }

    // Plain (non-slash) message — gate is removed, proceeds to run_agent.
    let event = helpers::make_event("chat-1", "hello world");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .ok(); // AgentRuntime not wired — may error, but D-02 must not be sent.

    let msgs = adapter.messages().await;
    let sent: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent.contains(&helpers::d02_error_message()),
        "Free-text during active turn must NOT receive D-02 — gate removed (R39.1-06). Got: {:?}",
        sent
    );
}

/// GW-05-11: `cmd_stop` reads a non-false `agent_running` on gateway.
///
/// After Phase 36, `handle_slash_command` populates `CommandContext.agent_running`
/// with the REAL per-session `Arc<AtomicBool>`, not a hardcoded false one.
/// `/stop` bypasses the guard and reaches dispatch; `cmd_stop` reads the flag.
/// When the flag is true, `cmd_stop` returns the "Stopping..." / non-idle message.
///
/// This asserts that the response from `/stop` when running is NOT the
/// "No agent is currently running" message (which cmd_stop emits when flag==false).
#[tokio::test]
async fn test_stop_reads_real_flag() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key = helpers::test_session_key("chat-1");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }

    // Set running = true so cmd_stop sees a non-false flag.
    {
        let s = store.read().await;
        s.get(&key).unwrap().running.store(true, Ordering::SeqCst);
    }

    let event = helpers::make_event("chat-1", "/stop");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .unwrap();

    let msgs = adapter.messages().await;
    // cmd_stop emits "No agent is currently running..." when flag == false (the old shim behavior).
    // When flag == true it emits the "Stopping agent..." or "Stopped N background process(es)." path.
    // We assert the idle message is NOT sent (proves the real flag was read).
    let idle_msg = "No agent is currently running.";
    let no_agent_msg = "No agent is currently running. Use Ctrl-C to cancel an \
                        in-flight turn.";
    for (_, text) in &msgs {
        assert!(
            !text.starts_with(idle_msg) && !text.contains(no_agent_msg),
            "cmd_stop must NOT say 'No agent is currently running' when flag==true. \
             This would mean CommandContext.agent_running received a false shim. Got: {:?}",
            text
        );
    }
    // Also confirm at least one message was sent (stop handler replied).
    assert!(
        !msgs.is_empty(),
        "cmd_stop must send a response when the session is running. Got no messages."
    );
}
