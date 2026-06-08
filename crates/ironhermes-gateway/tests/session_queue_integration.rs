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
use ironhermes_gateway::adapter::PlatformAdapter;
use ironhermes_gateway::session::{SessionKey, SessionStore};
use ironhermes_gateway::session_queue::{MAX_QUEUE_DEPTH, SessionQueue};
use ironhermes_gateway::{GatewayMessageHandler, GatewayRunner};
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

    async fn edit_message(&self, _chat_id: &str, _message_id: &str, _content: &str) -> Result<()> {
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
        // Phase 36.17.2.2-05: record into the same `sent` log as plain
        // send_message so existing assertions on send-count still hold
        // if a future overflow-chunk path lands on this adapter.
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

    async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> Result<()> {
        Ok(())
    }

    async fn add_reaction(&self, chat_id: &str, message_id: &str, emoji: &str) -> Result<()> {
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

    async fn edit_message(&self, _chat_id: &str, _message_id: &str, _content: &str) -> Result<()> {
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
        // Phase 36.17.2.2-05: intentionally fails like send_message so
        // the fixture's fast-exit semantics carry over to any future
        // overflow-chunk path that lands on this adapter (records before
        // returning Err).
        self.sent
            .lock()
            .await
            .push((chat_id.to_string(), content.to_string()));
        Err(anyhow::anyhow!(
            "FailingPlatformAdapter::send_message_markdown_v2 — intentional test failure to fast-exit run_agent"
        ))
    }

    async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> Result<()> {
        Ok(())
    }

    async fn add_reaction(&self, _chat_id: &str, _message_id: &str, _emoji: &str) -> Result<()> {
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
    let placeholders: Vec<&(String, String)> =
        msgs.iter().filter(|(_, c)| c == "\u{2588}").collect();
    assert_eq!(
        placeholders.len(),
        3,
        "expected 3 placeholder sends (one per drained run_agent invocation); got {:?}",
        msgs
    );
}

// ===========================================================================
// Phase 36.17.1 Plan 03 Task 2 — /queue dispatch + /new clear-ordering tests
// ===========================================================================

/// Helper: dispatch the literal `/queue <text>` command through the public
/// MessageHandler::handle entry point so we exercise the FULL gateway slash
/// dispatch path including handle_slash_command's CommandResult::Queued arm.
fn make_slash_event(chat_id: &str, slash_text: &str) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        // Unique message_id so cap-hit reaction assertions can target it.
        message_id: format!("slashmsg-{}", slash_text.replace(' ', "_")),
        chat_id: chat_id.to_string(),
        sender_id: "u1".to_string(),
        content: slash_text.to_string(),
        attachments: vec![],
        thread_id: None,
        chat_type: "dm".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    }
}

/// Plan 03 Task 2 behavior 1:
/// `/queue <message>` dispatched via MessageHandler::handle goes through
/// the Queued arm of handle_slash_command, synthesizes a MessageEvent that
/// inherits the triggering event's identity, pushes onto SessionQueue, and
/// sends a depth-aware confirmation:
///   - First push (depth == 1): "Queued for the next turn."
///   - Second push (depth == 2): "Queued for the next turn. (2 queued)"
#[tokio::test]
async fn test_cmd_queue_enqueues_and_replies_with_depth() {
    use ironhermes_gateway::adapter::MessageHandler;

    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = RecordingPlatformAdapter::new();

    let key = test_session_key("chat-q");
    // Pre-create the session so SessionStore.get_running_flag returns a
    // stable Arc<AtomicBool> for the slash-dispatch path.
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }

    // Pre-condition: queue is empty.
    assert_eq!(queue.len(&key), 0, "queue must start empty");

    // First /queue dispatch.
    let ev1 = make_slash_event("chat-q", "/queue hello");
    handler
        .handle(&ev1, adapter.clone(), CancellationToken::new())
        .await
        .expect("first /queue handles Ok");

    // After push: depth == 1; one chat reply containing the singular text.
    assert_eq!(queue.len(&key), 1, "first /queue must push (depth == 1)");
    let msgs1 = adapter.messages().await;
    assert_eq!(
        msgs1.len(),
        1,
        "expected exactly 1 send_message, got {:?}",
        msgs1
    );
    assert_eq!(
        msgs1[0].1, "Queued for the next turn.",
        "depth==1 reply must use the singular form"
    );

    // Second /queue dispatch.
    let ev2 = make_slash_event("chat-q", "/queue world");
    handler
        .handle(&ev2, adapter.clone(), CancellationToken::new())
        .await
        .expect("second /queue handles Ok");

    // After push: depth == 2; second chat reply contains plural form.
    assert_eq!(queue.len(&key), 2, "second /queue must push (depth == 2)");
    let msgs2 = adapter.messages().await;
    assert_eq!(
        msgs2.len(),
        2,
        "expected exactly 2 send_message after 2 pushes"
    );
    assert!(
        msgs2[1].1.contains("(2 queued)"),
        "depth==2 reply must include depth suffix; got {:?}",
        msgs2[1].1
    );

    // T-36.17.1-02 mitigation evidence: the synthesized MessageEvents in the
    // queue inherit identity from the triggering event (chat_id, sender_id),
    // and content == the args (hello / world), not the raw "/queue hello".
    // Pop and inspect.
    let popped1 = queue.pop(&key).expect("first pop yields event");
    assert_eq!(popped1.content, "hello", "first queued content == 'hello'");
    assert_eq!(
        popped1.chat_id, "chat-q",
        "chat_id inherits from triggering event"
    );
    assert_eq!(
        popped1.sender_id, "u1",
        "sender_id inherits from triggering event"
    );

    let popped2 = queue.pop(&key).expect("second pop yields event");
    assert_eq!(popped2.content, "world", "second queued content == 'world'");
    assert_eq!(popped2.chat_id, "chat-q");
    assert_eq!(popped2.sender_id, "u1");
}

/// Plan 03 Task 2 behavior 2:
/// `/queue` cap-hit: pre-fill the queue to MAX_QUEUE_DEPTH (128), dispatch a
/// `/queue overflow` slash command and assert the cap-hit branch fires:
///   - one add_reaction("❌") targeting the slash message's message_id
///   - one chat reply containing "Queue is full (128 messages)"
///   - queue depth still 128 (T-36.17.1-01 cap held)
#[tokio::test]
async fn test_cmd_queue_cap_hit_emits_reaction() {
    use ironhermes_gateway::adapter::MessageHandler;

    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = RecordingPlatformAdapter::new();

    let key = test_session_key("chat-q-cap");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }

    // Pre-fill the queue to the cap directly via the public SessionQueue API.
    for i in 0..MAX_QUEUE_DEPTH {
        let prefill_event = make_event("chat-q-cap", &format!("prefill-{}", i));
        queue
            .try_push(&key, prefill_event)
            .expect("prefill within cap");
    }
    assert_eq!(queue.len(&key), MAX_QUEUE_DEPTH, "queue pre-filled to cap");

    // /queue command — must trip the CapacityReached branch of the Queued
    // arm, not the Ok branch.
    let slash_event = make_slash_event("chat-q-cap", "/queue overflow");
    handler
        .handle(&slash_event, adapter.clone(), CancellationToken::new())
        .await
        .expect("handle returns Ok on cap-hit (UX sent, return Ok)");

    // Cap held — push was rejected.
    assert_eq!(
        queue.len(&key),
        MAX_QUEUE_DEPTH,
        "cap must hold at {}; new push must be dropped",
        MAX_QUEUE_DEPTH
    );

    // Exactly one ❌ reaction targeting the slash-message_id.
    let reactions = adapter.reactions().await;
    assert_eq!(
        reactions.len(),
        1,
        "expected exactly 1 add_reaction call; got {:?}",
        reactions
    );
    assert_eq!(reactions[0].emoji, "❌");
    assert_eq!(reactions[0].chat_id, "chat-q-cap");
    assert_eq!(
        reactions[0].message_id, "slashmsg-/queue_overflow",
        "reaction targets the /queue slash message_id"
    );

    // Exactly one chat reply containing the cap-hit literal.
    let msgs = adapter.messages().await;
    let cap_hit_msgs: Vec<&(String, String)> = msgs
        .iter()
        .filter(|(_, c)| c.contains("Queue is full (128 messages)"))
        .collect();
    assert_eq!(
        cap_hit_msgs.len(),
        1,
        "expected exactly 1 cap-hit chat reply via Queued arm; got messages: {:?}",
        msgs
    );
}

/// Plan 03 Task 2 behavior 3 — Pitfall 5 ordering proof:
/// /new (slash command) MUST call session_queue.clear(&session_key) BEFORE
/// session_store.remove(&session_key). Pre-fill the queue with 3 events for
/// a session_key; dispatch `/new`; assert that AFTER the dispatch:
///   - queue_len(key) == 0 (clear ran)
///   - session_store no longer contains the key (remove ran)
/// The relative ordering is enforced at source-code level by the gateway
/// handler's clear-then-remove block; this test confirms both ran and the
/// post-state is consistent.
#[tokio::test]
async fn test_new_clears_queue_before_session_remove() {
    use ironhermes_gateway::adapter::MessageHandler;

    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = RecordingPlatformAdapter::new();

    let key = test_session_key("chat-new");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }

    // Pre-fill queue with 3 events for this session.
    for c in ["A", "B", "C"] {
        queue
            .try_push(&key, make_event("chat-new", c))
            .expect("pre-fill under cap");
    }
    assert_eq!(queue.len(&key), 3, "queue pre-filled with 3 events");

    // Pre-condition: session exists in store.
    {
        let s = store.read().await;
        assert!(
            s.get(&key).is_some(),
            "session must exist in store before /new"
        );
    }

    // Dispatch /new.
    let new_event = make_slash_event("chat-new", "/new");
    handler
        .handle(&new_event, adapter.clone(), CancellationToken::new())
        .await
        .expect("/new handles Ok");

    // Post-condition 1: queue is empty (clear ran).
    assert_eq!(
        queue.len(&key),
        0,
        "Pitfall 5: queue must be empty after /new (clear before remove)"
    );

    // Post-condition 2: session removed from store.
    {
        let s = store.read().await;
        assert!(
            s.get(&key).is_none(),
            "session must be removed from store after /new"
        );
    }

    // Confirmation reply was sent ("Conversation cleared. Starting fresh.").
    let msgs = adapter.messages().await;
    let confirmation_msgs: Vec<&(String, String)> = msgs
        .iter()
        .filter(|(_, c)| c.contains("Conversation cleared") || c.contains("Ready for a new one"))
        .collect();
    assert_eq!(
        confirmation_msgs.len(),
        1,
        "expected /new confirmation reply; got messages: {:?}",
        msgs
    );
}

// ===========================================================================
// Phase 36.17.1 Plan 05 Task 1 — Telegram end-to-end flow tests
// ===========================================================================
//
// Three new tests that lock D-02 (Telegram-only ship requirement) end-to-end
// against the real GatewayMessageHandler + GatewayRunner code paths:
//
//   1. `test_telegram_busy_path_enqueues` — Telegram MessageEvent arrives while
//      agent_running == true → enqueues onto SessionQueue with ZERO chat replies
//      (D-13: free-text enqueue is silent).
//   2. `test_telegram_cap_hit_full_ux` — pre-fill to 128; 129th Telegram event
//      triggers exactly one ❌ reaction + one "Queue is full (128 messages)"
//      chat reply; cap held at 128.
//   3. `test_telegram_post_turn_drain_in_arrival_order` — enqueue 3 events
//      "A","B","C" via the real `runner.try_enqueue` path; release busy flag;
//      invoke `runner.drain_pending(...)` directly; assert queue drains 3 → 0
//      via VecDeque FIFO (proven elsewhere by the proptest suite — depth count
//      proves the loop iterated for every event).
//
// NOTE on Pitfall 6 (Telegram getUpdates offset advance, D-13):
// The Telegram poll loop in runner.rs:723-736 advances the `offset` to
// `update.update_id + 1` UNCONDITIONALLY before each update is forwarded into
// the `msg_tx` mpsc channel. The cap-hit UX (and any handler-level work) fires
// strictly AFTER that offset advance — so a cap-hit cannot cause Telegram to
// re-deliver the dropped update on the next getUpdates call. This is a
// runner-loop invariant and cannot be exercised from a handler-level test;
// the live verification lives in the UAT runbook
// (`tests/session_queue_telegram_uat.md`, Scenario 3 step e).

/// Plan 05 Task 1 behavior 1 — Telegram busy-enqueue silent path:
/// A Telegram MessageEvent arriving via `MessageHandler::handle` while
/// `agent_running == true` is enqueued onto `SessionQueue`. The handler's
/// busy-branch records ZERO `send_message` AND ZERO `add_reaction` calls
/// (D-13: free-text enqueue is silent; UserQueueManager's transport-layer
/// 👁 reaction is a separate layer not exercised in handler-level tests).
#[tokio::test]
async fn test_telegram_busy_path_enqueues() {
    use ironhermes_gateway::adapter::MessageHandler;

    // Pitfall 6: Telegram getUpdates offset advance happens at
    // runner.rs:723-736 BEFORE dispatch. Handler-level test cannot verify
    // offset advance — see session_queue_telegram_uat.md for the live
    // verification step.

    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = RecordingPlatformAdapter::new();

    // Build the canonical Telegram SessionKey (Platform::Telegram + chat_id
    // "test_chat" + sender_id "test_user") matching the planner's spec.
    let key = SessionKey::new(Platform::Telegram, "test_chat").with_user("test_user");
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

    assert_eq!(queue.len(&key), 0, "queue must start empty");

    // Telegram MessageEvent — platform=Telegram, chat_id, sender_id matching
    // the SessionKey, content "hello".
    let event = MessageEvent {
        platform: Platform::Telegram,
        message_id: "tg-msg-1".to_string(),
        chat_id: "test_chat".to_string(),
        sender_id: "test_user".to_string(),
        content: "hello".to_string(),
        attachments: vec![],
        thread_id: None,
        chat_type: "private".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    };

    handler
        .handle(&event, adapter.clone(), CancellationToken::new())
        .await
        .expect("handle returns Ok on Telegram enqueue path");

    // Depth 1 — busy-branch must enqueue, not reject.
    assert_eq!(
        queue.len(&key),
        1,
        "Telegram busy-branch must enqueue (depth == 1 after one push)"
    );

    // D-13: free-text enqueue is silent. The handler's Queued/busy branch
    // MUST NOT emit a chat reply or a reaction. UserQueueManager's
    // transport-layer 👁 is a separate layer above the handler.
    let msgs = adapter.messages().await;
    assert!(
        msgs.is_empty(),
        "D-13: free-text enqueue is silent; expected zero send_message from the handler, got {:?}",
        msgs
    );
    let reactions = adapter.reactions().await;
    assert!(
        reactions.is_empty(),
        "D-13: handler's Queued/busy branch must not emit reactions; \
         transport-layer 👁 is UserQueueManager's job, not the handler's. Got {:?}",
        reactions
    );
}

/// Plan 05 Task 1 behavior 2 — Telegram cap-hit full UX:
/// Pre-fill SessionQueue to MAX_QUEUE_DEPTH (128). Dispatch a 129th Telegram
/// event with chat_id="test_chat", message_id="msg_id_129". Assertions:
///   - RecordingPlatformAdapter.add_reaction calls == 1 with emoji "❌"
///     matching the 129th event's chat_id + message_id (drop-newest, D-13).
///   - RecordingPlatformAdapter.send_message calls == 1 with content
///     containing "Queue is full (128 messages)" (D-13 literal).
///   - SessionQueue.len(&key) == 128 after the attempt (cap held,
///     T-36.17.1-01 mitigation evidence).
///
/// The poll-loop offset advance is OUTSIDE handler scope (runner.rs:723-736);
/// see Pitfall 6 note above and the live UAT runbook for the offset-advance
/// verification step.
#[tokio::test]
async fn test_telegram_cap_hit_full_ux() {
    use ironhermes_gateway::adapter::MessageHandler;

    // Pitfall 6: Telegram getUpdates offset advance happens at
    // runner.rs:723-736 BEFORE dispatch. Handler-level test cannot verify
    // offset advance — see session_queue_telegram_uat.md for the live
    // verification step.

    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = RecordingPlatformAdapter::new();

    let key = SessionKey::new(Platform::Telegram, "test_chat").with_user("test_user");
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

    // Pre-fill the queue to MAX_QUEUE_DEPTH with distinct content
    // "msg_001"..="msg_128". The numbering deviates from the integer-typed
    // pre-fill in `test_cap_hit_emits_reaction` to make the planner's spec
    // (msg_001..msg_128 / msg_129) literally observable in pop order if
    // anyone wants to inspect.
    for i in 1..=MAX_QUEUE_DEPTH {
        let content = format!("msg_{:03}", i);
        let prefill_event = MessageEvent {
            platform: Platform::Telegram,
            message_id: format!("tg-prefill-{:03}", i),
            chat_id: "test_chat".to_string(),
            sender_id: "test_user".to_string(),
            content,
            attachments: vec![],
            thread_id: None,
            chat_type: "private".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        };
        queue
            .try_push(&key, prefill_event)
            .expect("prefill within cap");
    }
    assert_eq!(queue.len(&key), MAX_QUEUE_DEPTH, "queue pre-filled to cap");

    // 129th event — message_id="msg_id_129" matches the planner's spec
    // so the reaction assertion is byte-for-byte against the dropped id.
    let event_129 = MessageEvent {
        platform: Platform::Telegram,
        message_id: "msg_id_129".to_string(),
        chat_id: "test_chat".to_string(),
        sender_id: "test_user".to_string(),
        content: "msg_129".to_string(),
        attachments: vec![],
        thread_id: None,
        chat_type: "private".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    };
    handler
        .handle(&event_129, adapter.clone(), CancellationToken::new())
        .await
        .expect("handle returns Ok on cap-hit (UX sent, return Ok)");

    // T-36.17.1-01: cap holds at 128 — the 129th push was dropped.
    assert_eq!(
        queue.len(&key),
        MAX_QUEUE_DEPTH,
        "cap must hold at {}; 129th push must be dropped (drop-newest, D-10)",
        MAX_QUEUE_DEPTH
    );

    // Exactly one ❌ reaction recorded targeting the 129th message's
    // chat_id="test_chat" and message_id="msg_id_129".
    let reactions = adapter.reactions().await;
    assert_eq!(
        reactions.len(),
        1,
        "expected exactly 1 add_reaction call on cap-hit; got {:?}",
        reactions
    );
    assert_eq!(reactions[0].emoji, "❌", "D-13 cap-hit emoji literal");
    assert_eq!(
        reactions[0].chat_id, "test_chat",
        "reaction targets the cap-hit message's chat_id"
    );
    assert_eq!(
        reactions[0].message_id, "msg_id_129",
        "reaction targets the 129th message's message_id (D-13: drop-newest)"
    );

    // Exactly one chat reply whose content contains the literal D-13 string.
    let msgs = adapter.messages().await;
    let cap_hit_msgs: Vec<&(String, String)> = msgs
        .iter()
        .filter(|(_, c)| c.contains("Queue is full (128 messages)"))
        .collect();
    assert_eq!(
        cap_hit_msgs.len(),
        1,
        "expected exactly 1 cap-hit chat reply containing the D-13 literal; got messages: {:?}",
        msgs
    );
    assert_eq!(
        cap_hit_msgs[0].0, "test_chat",
        "cap-hit reply goes to event.chat_id"
    );
}

/// Plan 05 Task 1 behavior 3 — Telegram post-turn drain in arrival order:
/// Enqueue 3 Telegram events with content "A","B","C" via the real
/// `runner.try_enqueue` path while agent_running==true. Release the busy
/// flag, then invoke `runner.drain_pending(&key, &handler, adapter, cancel)`
/// DIRECTLY (NOT a substitute pop-sequence test — the planner is explicit
/// that the test must exercise the real drain helper).
///
/// Post-conditions:
///   - `runner.queue_len(&key) == 0`: drain consumed all queued events.
///   - The recording adapter saw exactly 3 placeholder `█` send_message
///     attempts (one per `run_agent` invocation), proving the drain loop
///     iterated 3 times. The FIFO arrival order — A, B, C — is a property
///     of `SessionQueue::pop`'s VecDeque backing, proven exhaustively by
///     the 1024-case proptest in `session_queue.rs::parity`; depth==3 + 3
///     iterations + a FIFO pop primitive implies A→B→C ordering by
///     construction. (D-01 explicit: one full agent turn per queued item.)
#[tokio::test]
async fn test_telegram_post_turn_drain_in_arrival_order() {
    let store = build_test_session_store();
    let queue = Arc::new(SessionQueue::new());
    // The handler still needs a SessionQueue Arc for the busy-branch path,
    // even though drain_pending pops from the runner's own Arc. Mirrors
    // the pattern in `test_drain_after_turn` (Plan 02 Task 3).
    let handler = build_test_handler_with_queue(store.clone(), queue.clone());
    let adapter = FailingPlatformAdapter::new();

    let config = Config::default();
    let resolver = ProviderResolver::build(&config).expect("resolver");
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let runner = GatewayRunner::new(config, resolver, tool_registry);

    let key = SessionKey::new(Platform::Telegram, "test_chat").with_user("test_user");
    {
        let mut s = store.write().await;
        s.get_or_create(key.clone(), "model", "test");
    }

    // First set agent_running=true (mirroring the live flow: events arrive
    // and queue while the agent's previous turn is mid-flight).
    {
        let s = store.read().await;
        s.get(&key)
            .expect("session exists")
            .running
            .store(true, Ordering::SeqCst);
    }

    // Enqueue 3 Telegram events with content "A","B","C" in arrival order
    // through the runner's PUBLIC `try_enqueue` API. This is the same path
    // the live gateway uses when the per-chat worker observes agent_running.
    for c in ["A", "B", "C"] {
        let event = MessageEvent {
            platform: Platform::Telegram,
            message_id: format!("tg-msg-{}", c),
            chat_id: "test_chat".to_string(),
            sender_id: "test_user".to_string(),
            content: c.to_string(),
            attachments: vec![],
            thread_id: None,
            chat_type: "private".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        };
        runner.try_enqueue(&key, event).expect("enqueue under cap");
    }
    assert_eq!(
        runner.queue_len(&key),
        3,
        "pre-load saw 3 events for Telegram session"
    );

    // Release the busy flag — the live flow does this when the previous
    // run_agent invocation completes and RunningAgentGuard drops.
    {
        let s = store.read().await;
        s.get(&key)
            .expect("session exists")
            .running
            .store(false, Ordering::SeqCst);
    }

    // Invoke the REAL drain helper directly — runner.drain_pending(...).
    // This is the same `pub async fn drain_pending` exposed by Plan 02
    // Task 3; the planner forbids a hand-rolled pop loop in this test.
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    runner
        .drain_pending(&key, &handler, adapter_dyn, CancellationToken::new())
        .await
        .expect("drain_pending returns Ok (errors are logged+continued)");

    // Drain consumed everything — queue depth went 3 → 2 → 1 → 0 in FIFO
    // arrival order (VecDeque::pop_front).
    assert_eq!(
        runner.queue_len(&key),
        0,
        "drain must consume every queued event for the Telegram session"
    );

    // Exactly 3 placeholder sends — one per `run_agent` invocation. This
    // proves the drain loop iterated 3 times (no merging — D-01 explicit:
    // one full agent turn per queued item). The FIFO arrival order is a
    // property of SessionQueue::pop (VecDeque::pop_front) proven by the
    // proptest suite in session_queue.rs::parity (1024 cases).
    let msgs = adapter.messages().await;
    let placeholders: Vec<&(String, String)> =
        msgs.iter().filter(|(_, c)| c == "\u{2588}").collect();
    assert_eq!(
        placeholders.len(),
        3,
        "expected 3 placeholder sends (one per drained run_agent); got {:?}",
        msgs
    );
}
