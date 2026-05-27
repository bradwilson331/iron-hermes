//! Phase 36.17.1 Plan 02 Task 3 integration tests.
//!
//! Exercises the live busy-branch enqueue, cap-hit UX, and post-turn drain
//! helper end-to-end against the real `GatewayMessageHandler` and
//! `GatewayRunner` code paths.
//!
//! Tests covered (matching the plan's Task 3 `<behavior>` block):
//!
//! 1. `test_busy_agent_enqueues_event` — when agent_running == true, free-text
//!    arrival enqueues onto `SessionQueue` instead of rejecting; no extra
//!    `send_message` is recorded (D-13: free-text enqueue is silent).
//! 2. `test_cap_hit_emits_reaction` — pre-fill the queue to 128, send a
//!    129th message; assert ❌ reaction + ⏳ chat reply are recorded and the
//!    cap holds at 128 (T-36.17.1-01 mitigation evidence).
//! 3. `test_drain_after_turn` — pre-load 3 events; call
//!    `runner.drain_pending(...)` DIRECTLY (not a substitute pop-sequence
//!    unit test); assert queue is empty after and drain made progress.

#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{Mutex as TokioMutex, RwLock};
use tokio_util::sync::CancellationToken;

use ironhermes_core::{Config, MessageEvent, MessageResponse, Platform, ProviderResolver};
use ironhermes_gateway::{
    GatewayMessageHandler, GatewayRunner,
};
use ironhermes_gateway::adapter::PlatformAdapter;
use ironhermes_gateway::session::{SessionKey, SessionStore};
use ironhermes_gateway::session_queue::{MAX_QUEUE_DEPTH, SessionQueue};
use ironhermes_tools::ToolRegistry;

// --------------------------------------------------------------------------
// RecordingPlatformAdapter
// --------------------------------------------------------------------------
//
// Captures every send_message AND add_reaction call so tests can assert on
// the D-13 cap-hit UX without a live Telegram connection. Mirrors the
// pattern used in tests/running_agent_guard_tests.rs but extends it with
// reaction capture.

#[derive(Clone, Debug)]
pub struct RecordedReaction {
    pub chat_id: String,
    pub message_id: String,
    pub emoji: String,
}

pub struct RecordingPlatformAdapter {
    /// (chat_id, content) pairs.
    sent: TokioMutex<Vec<(String, String)>>,
    /// Recorded `add_reaction` calls.
    reactions: TokioMutex<Vec<RecordedReaction>>,
}

impl RecordingPlatformAdapter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sent: TokioMutex::new(Vec::new()),
            reactions: TokioMutex::new(Vec::new()),
        })
    }

    pub async fn messages(&self) -> Vec<(String, String)> {
        self.sent.lock().await.clone()
    }

    pub async fn reactions(&self) -> Vec<RecordedReaction> {
        self.reactions.lock().await.clone()
    }
}

#[async_trait]
impl PlatformAdapter for RecordingPlatformAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        self.sent
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

    async fn edit_message_markdown(
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

    async fn add_reaction(
        &self,
        chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        self.reactions.lock().await.push(RecordedReaction {
            chat_id: chat_id.to_string(),
            message_id: message_id.to_string(),
            emoji: emoji.to_string(),
        });
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

// --------------------------------------------------------------------------
// FailingPlatformAdapter
// --------------------------------------------------------------------------
//
// Records every send_message call (chat_id, content) for test assertions, then
// returns Err. Used by `test_drain_after_turn` to make `run_agent` fail-fast
// at its first awaited adapter call — without this, `run_agent` proceeds past
// the placeholder send into PromptBuilder/StreamConsumer/runtime setup which
// has many uncontrolled side effects under `Config::default()` and hangs the
// test. The drain helper's Rule 2 deviation (log+continue per event) is what
// the test actually exercises: every iteration's `run_agent` errors, drain
// keeps popping until queue is empty.

pub struct FailingPlatformAdapter {
    sent: TokioMutex<Vec<(String, String)>>,
}

impl FailingPlatformAdapter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sent: TokioMutex::new(Vec::new()),
        })
    }

    pub async fn messages(&self) -> Vec<(String, String)> {
        self.sent.lock().await.clone()
    }
}

#[async_trait]
impl PlatformAdapter for FailingPlatformAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        self.sent
            .lock()
            .await
            .push((chat_id.to_string(), content.to_string()));
        Err(anyhow::anyhow!(
            "FailingPlatformAdapter::send_message — intentional test failure to fast-exit run_agent"
        ))
    }

    async fn edit_message(
        &self,
        _chat_id: &str,
        _message_id: &str,
        _content: &str,
    ) -> Result<()> {
        Ok(())
    }

    async fn edit_message_markdown(
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

    async fn add_reaction(
        &self,
        _chat_id: &str,
        _message_id: &str,
        _emoji: &str,
    ) -> Result<()> {
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

// --------------------------------------------------------------------------
// Test fixture helpers
// --------------------------------------------------------------------------

fn build_test_session_store() -> Arc<RwLock<SessionStore>> {
    let state_store = Arc::new(std::sync::Mutex::new(
        ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
    ));
    Arc::new(RwLock::new(SessionStore::new(state_store)))
}

fn build_test_handler_with_queue(
    store: Arc<RwLock<SessionStore>>,
    queue: Arc<SessionQueue>,
) -> GatewayMessageHandler {
    let config = Config::default();
    let resolver = ProviderResolver::build(&config).expect("default resolver builds");
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let mut handler = GatewayMessageHandler::new(config, resolver, store, tool_registry);
    handler.set_session_queue(queue);
    handler
}

fn test_session_key(chat_id: &str) -> SessionKey {
    SessionKey::new(Platform::Telegram, chat_id).with_user("u1")
}

fn make_event(chat_id: &str, content: &str) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        message_id: format!("msg-{}", content),
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

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

/// Plan 02 Task 3 behavior 1:
/// When agent_running == true at `handle_with_multimodal`, the free-text
/// event is enqueued onto SessionQueue and NOT rejected with the
/// `AGENT_RUNNING_REJECT_MSG` chat reply.
///
/// D-13 mandate: free-text enqueue is silent — UserQueueManager's transport
/// 👁 reaction is the visible signal, not a gateway chat reply. So
/// RecordingPlatformAdapter must observe ZERO send_message calls from
/// the busy-branch enqueue path.
#[tokio::test]
async fn test_busy_agent_enqueues_event() {
    use ironhermes_gateway::adapter::MessageHandler;

    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = RecordingPlatformAdapter::new();

    // Set agent_running = true for the target session.
    let key = test_session_key("chat-busy");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    {
        let s = store.read().await;
        s.get(&key)
            .expect("session was just created")
            .running
            .store(true, Ordering::SeqCst);
    }

    // Pre-condition: queue is empty for this session.
    assert_eq!(queue.len(&key), 0, "queue must start empty");

    // Dispatch a free-text message — goes through handle_with_multimodal's
    // non-slash busy branch.
    let event = make_event("chat-busy", "hello while busy");
    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .expect("handle returns Ok on enqueue path");

    // Post-condition: depth == 1; no reject message sent.
    assert_eq!(
        queue.len(&key),
        1,
        "busy-branch must enqueue (depth == 1 after one push)"
    );
    let msgs = adapter.messages().await;
    assert!(
        msgs.is_empty(),
        "D-13: free-text enqueue is silent; expected zero send_message, got {:?}",
        msgs
    );
}

/// Plan 02 Task 3 behavior 2:
/// When the SessionQueue is full (depth == 128) and another event arrives
/// during busy, the cap-hit branch fires:
///   - `add_reaction(chat_id, message_id, "❌")` recorded
///   - `send_message(chat_id, "⏳ Queue is full (128 messages). …")` recorded
///   - queue depth stays at 128 (T-36.17.1-01: cap held).
#[tokio::test]
async fn test_cap_hit_emits_reaction() {
    use ironhermes_gateway::adapter::MessageHandler;

    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = RecordingPlatformAdapter::new();

    let key = test_session_key("chat-cap");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    {
        let s = store.read().await;
        s.get(&key)
            .expect("session")
            .running
            .store(true, Ordering::SeqCst);
    }

    // Pre-fill the queue to MAX_QUEUE_DEPTH (128).
    for i in 0..MAX_QUEUE_DEPTH {
        let prefill_event = make_event("chat-cap", &format!("prefill-{}", i));
        queue
            .try_push(&key, prefill_event)
            .expect("prefill within cap");
    }
    assert_eq!(queue.len(&key), MAX_QUEUE_DEPTH, "queue pre-filled to cap");

    // 129th event — must trigger cap-hit branch.
    let event_129 = make_event("chat-cap", "the-129th");
    handler
        .handle(&event_129, adapter.clone(), CancellationToken::new())
        .await
        .expect("handle returns Ok on cap-hit (UX sent, return Ok)");

    // Assert: queue depth still 128 (cap held — T-36.17.1-01).
    assert_eq!(
        queue.len(&key),
        MAX_QUEUE_DEPTH,
        "cap must hold at {}; new push must be dropped",
        MAX_QUEUE_DEPTH
    );

    // Assert: exactly one ❌ reaction recorded targeting the 129th message_id.
    let reactions = adapter.reactions().await;
    assert_eq!(
        reactions.len(),
        1,
        "expected exactly 1 add_reaction call; got {:?}",
        reactions
    );
    assert_eq!(reactions[0].emoji, "❌", "emoji must be ❌");
    assert_eq!(
        reactions[0].chat_id, "chat-cap",
        "reaction targets the cap-hit message's chat_id"
    );
    assert_eq!(
        reactions[0].message_id, "msg-the-129th",
        "reaction targets the 129th message's message_id (D-13: drop-newest)"
    );

    // Assert: exactly one chat reply containing the literal cap-hit string.
    let msgs = adapter.messages().await;
    let cap_hit_msgs: Vec<&(String, String)> = msgs
        .iter()
        .filter(|(_, c)| c.contains("Queue is full (128 messages)"))
        .collect();
    assert_eq!(
        cap_hit_msgs.len(),
        1,
        "expected exactly 1 cap-hit chat reply; got messages: {:?}",
        msgs
    );
    assert_eq!(
        cap_hit_msgs[0].0, "chat-cap",
        "cap-hit reply goes to event.chat_id"
    );
}

/// Plan 02 Task 3 behavior 3:
/// Direct invocation of `runner.drain_pending(...)` — the test MUST call the
/// real drain helper, not a hand-rolled pop loop. Pre-load 3 events for a
/// session_key, run drain, assert queue is empty afterwards and drain made
/// progress against the recording adapter.
///
/// The FIFO ordering of `SessionQueue::pop` is proven by the proptest suite
/// in `session_queue.rs::parity` (Plan 01 Task 3 — 1024-case equivalence vs.
/// the Python-layout `SplitSlotQueue`). This integration test confirms that
/// `drain_pending` itself pops + invokes run_agent for every event — the
/// post-condition `queue_len == 0` after exactly 3 enqueued events means
/// the drain loop ran 3 iterations (Pitfall 4 mitigation: each iteration
/// goes through `run_agent` directly, NOT `handle_with_multimodal`).
///
/// To keep this test bounded under `Config::default()` (no AgentRuntime, no
/// API keys), the adapter intentionally fails `send_message`. That makes
/// `run_agent` return Err at its first awaited platform call (the █
/// placeholder send) — before PromptBuilder, StreamConsumer, or the runtime
/// fallback path execute. The production `drain_pending` logs+continues per
/// the Rule 2 deviation (single bad event must not poison the entire drain),
/// so the queue still drains to empty even when every `run_agent` errors.
#[tokio::test]
async fn test_drain_after_turn() {
    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = FailingPlatformAdapter::new();

    // Build a GatewayRunner so we can call its `drain_pending` method
    // directly (acceptance criterion: `runner.drain_pending(...)` must
    // appear in this test). The runner owns its own SessionQueue Arc and
    // it is NOT exposed externally (D-15) — so we pre-load via the
    // runner's public `try_enqueue` and `queue_len` API. The handler we
    // pass to `drain_pending` uses a different SessionQueue Arc (created
    // above as `queue`). That's fine for this test because:
    //   - `drain_pending` POPS from `self.session_queue` (the runner's Arc)
    //   - `drain_pending` then CALLS handler.run_agent(...) for each
    //   - the handler's own queue is irrelevant to the drain loop
    let config = Config::default();
    let resolver = ProviderResolver::build(&config).expect("resolver");
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let runner = GatewayRunner::new(config, resolver, tool_registry);
    // Use the runner's queue accessor — the test handler builder takes a
    // separate Arc, but for the drain test we don't need them to be the
    // same queue. We need run_agent to be reachable AND the runner's
    // SessionQueue to be populated; the handler's session_queue can be
    // a separate Arc since drain_pending pops from the runner's Arc.

    let key = test_session_key("chat-drain");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }
    // Drain runs with the busy flag cleared (per RESEARCH Pitfall 4
    // — the RAII guard inside run_agent re-sets the flag during each
    // iteration). At the test entry, assert it is false.
    {
        let s = store.read().await;
        assert!(
            !s.get(&key)
                .expect("session exists")
                .running
                .load(Ordering::SeqCst),
            "agent_running must be false at drain entry"
        );
    }

    // Pre-load 3 events with content "A", "B", "C" in arrival order.
    for c in ["A", "B", "C"] {
        runner
            .try_enqueue(&key, make_event("chat-drain", c))
            .expect("enqueue under cap");
    }
    assert_eq!(runner.queue_len(&key), 3, "pre-load saw 3 events");

    // Sanity: queue empty for an unrelated session key — drain must be
    // session-scoped, not global.
    let other_key = test_session_key("chat-drain-OTHER");
    assert_eq!(runner.queue_len(&other_key), 0);

    // Call the REAL drain helper directly. This is the deliverable per
    // D-01 part (b) — `pub async fn drain_pending(...)` on GatewayRunner.
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    runner
        .drain_pending(&key, &handler, adapter_dyn, CancellationToken::new())
        .await
        .expect("drain_pending returns Ok (errors are logged+continued)");

    // Post-condition: queue is empty for the drained session (depth went
    // 3 → 2 → 1 → 0 by VecDeque FIFO).
    assert_eq!(
        runner.queue_len(&key),
        0,
        "drain must consume every queued event for the session"
    );

    // Unrelated session still empty.
    assert_eq!(runner.queue_len(&other_key), 0);

    // The recording adapter saw at least 3 placeholder send_message
    // attempts (one per `run_agent` invocation). Each iteration ran the
    // real `run_agent` and sent the █ placeholder before failing on the
    // missing AgentRuntime. That count proves the drain loop iterated
    // exactly 3 times — the queue went 3 → 0 and pop returned None after.
    let msgs = adapter.messages().await;
    let placeholders: Vec<&(String, String)> = msgs
        .iter()
        .filter(|(_, c)| c == "\u{2588}")
        .collect();
    assert_eq!(
        placeholders.len(),
        3,
        "expected 3 placeholder sends (one per drained run_agent invocation); got {:?}",
        msgs
    );
}
