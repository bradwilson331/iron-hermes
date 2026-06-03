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

    /// The locked D-02 error message. Production emitted string MUST equal this
    /// byte-for-byte; all rejection assertions reference this helper.
    pub fn d02_error_message() -> &'static str {
        "Agent is running. Use /stop to interrupt or /queue to send after this turn."
    }
}

// ---------------------------------------------------------------------------
// GW-05 sub-behavior tests
// ---------------------------------------------------------------------------

use std::sync::atomic::Ordering;

/// GW-05-1: Per-session isolation.
///
/// Session A is set to Running (`running` flag = true). Session B remains Idle.
/// Dispatching `/model claude` to session B must succeed (not be rejected with D-02).
///
/// Verifies that the guard uses per-session state, not a global flag (codex HIGH-2).
#[tokio::test]
async fn test_session_isolation() {
    use ironhermes_gateway::adapter::MessageHandler;
    use tokio_util::sync::CancellationToken;

    let store = helpers::build_test_session_store();
    let adapter = helpers::RecordingPlatformAdapter::new();
    let handler = helpers::build_test_handler(store.clone());

    let key_a = helpers::test_session_key("chat-A");
    let key_b = helpers::test_session_key("chat-B");

    // Get-or-create both sessions so the store has entries with real running flags.
    {
        let mut s = store.write().await;
        s.get_or_create(key_a.clone(), "model", "test");
        s.get_or_create(key_b.clone(), "model", "test");
    }

    // Set session A running (simulates in-flight turn on session A).
    {
        let s = store.read().await;
        s.get(&key_a)
            .unwrap()
            .running
            .store(true, Ordering::SeqCst);
    }

    // Dispatch /model to session B — must NOT receive D-02 rejection.
    // The command may fail with "model not found" etc., but must not be the D-02 guard string.
    let event_b = helpers::make_event("chat-B", "/model claude");
    handler
        .handle(&event_b, adapter.clone(), CancellationToken::new())
        .await
        .ok(); // ignore result — we care about what was sent, not whether it errored

    let msgs = adapter.messages().await;
    let sent_texts: Vec<&str> = msgs.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        !sent_texts.contains(&helpers::d02_error_message()),
        "Session B must NOT be rejected with D-02 when only session A is running. Got: {:?}",
        sent_texts
    );

    // Dispatch /model to session A — MUST receive D-02 rejection.
    let adapter_a = helpers::RecordingPlatformAdapter::new();
    let event_a = helpers::make_event("chat-A", "/model claude");
    handler
        .handle(&event_a, adapter_a.clone(), CancellationToken::new())
        .await
        .ok();

    let msgs_a = adapter_a.messages().await;
    let sent_a: Vec<&str> = msgs_a.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        sent_a.contains(&helpers::d02_error_message()),
        "Session A MUST be rejected with D-02 when running. Got: {:?}",
        sent_a
    );
}

/// GW-05-2: `/model` rejected when agent is running.
///
/// Session flag is `true`. Sending `/model gpt-4` must trigger the D-02 rejection.
/// D-04: `/model` during active turn is rejected. Closes codex HIGH-2 TOCTOU.
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
        .unwrap();

    let msgs = adapter.messages().await;
    assert_eq!(
        msgs.len(),
        1,
        "Expected exactly 1 message (D-02 reject). Got: {:?}",
        msgs
    );
    assert_eq!(
        msgs[0].1,
        helpers::d02_error_message(),
        "/model must be rejected with D-02 verbatim string when running"
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

/// GW-05-7: Running flag clears on agent success.
///
/// `RunningAgentGuard` (D-06) sets flag=true on new(), clears to false on Drop.
/// After the guard goes out of scope (Ok path), flag must be false.
#[tokio::test]
async fn test_guard_clears_on_success() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use ironhermes_gateway::RunningAgentGuard;

    let flag = Arc::new(AtomicBool::new(false));

    {
        let _guard = RunningAgentGuard::new(flag.clone());
        assert!(
            flag.load(Ordering::SeqCst),
            "Flag must be true while guard is alive"
        );
        // Guard drops here (end of scope = success path)
    }

    assert!(
        !flag.load(Ordering::SeqCst),
        "Drop must clear flag to false on success exit (D-06)"
    );
}

/// GW-05-8: Running flag clears on agent error.
///
/// `RunningAgentGuard` must fire `Drop` even when the scope exits via an error.
/// After the error path, the flag must be `false`.
#[tokio::test]
async fn test_guard_clears_on_error() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use ironhermes_gateway::RunningAgentGuard;

    let flag = Arc::new(AtomicBool::new(false));

    let result: anyhow::Result<()> = {
        let _guard = RunningAgentGuard::new(flag.clone());
        assert!(flag.load(Ordering::SeqCst), "Flag must be true inside guard");
        // Simulate error propagation via ?
        Err(anyhow::anyhow!("simulated run_agent error"))
    };

    assert!(result.is_err(), "Result must be Err (sanity check)");
    assert!(
        !flag.load(Ordering::SeqCst),
        "Drop must clear flag to false on Err/? exit (D-06)"
    );
}

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

/// GW-05-10: Non-slash free-text message rejected when agent is running.
///
/// A plain-text (non-`/`) message dispatches through `MessageHandler::handle`
/// to `run_agent` directly. This path must ALSO check the session flag and reject
/// with the D-02 message when running (Pitfall 1 mitigation).
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

    // Plain (non-slash) message — goes through non-slash guard path.
    let event = helpers::make_event("chat-1", "hello world");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .unwrap();

    let msgs = adapter.messages().await;
    assert_eq!(
        msgs.len(),
        1,
        "Expected exactly 1 message (D-02 reject). Got: {:?}",
        msgs
    );
    assert_eq!(
        msgs[0].1,
        helpers::d02_error_message(),
        "Free-text during active turn must be rejected with D-02 verbatim (Pitfall 1 guard)"
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
