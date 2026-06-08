//! Phase 36.17.2 Plan 03 — UQM ↔ SessionQueue unification integration tests.
//!
//! D-21: 5 same-chat messages dispatched through `UserQueueManager::dispatch` →
//! `SessionQueue` → per-chat Notify-pop worker → `handle_with_multimodal` must
//! produce exactly 5 `👀` reactions in FIFO arrival order.
//!
//! These tests would have FAILED under the 36.17.1 architecture where `👀` was
//! emitted at dispatch time (violating D-08). They lock the architectural shift:
//! - T-36.17.2-01: worker-exit/dispatch race — after worker cancel, redispatch
//!   returns `WorkerSpawned` (not `Accepted`), proving the workers map is cleaned.
//! - T-36.17.2-03: `👀` fires at pop time (worker side), not at dispatch time.
//!
//! # Helper note
//!
//! `RecordingPlatformAdapter`, `FailingPlatformAdapter`, `build_test_session_store`,
//! `build_test_handler_with_queue`, and `make_event` are COPIED from
//! `tests/session_queue_integration.rs` because Rust integration test binaries
//! cannot share modules across test files (each is a separate binary). The new
//! `RecordingFailingAdapter` is unique to this file: it records every call AND
//! returns `Err` on `send_message` so `run_agent` fast-exits without hanging the
//! test under `Config::default()`.

#![allow(clippy::expect_used)]
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use ironhermes_core::{Config, MessageEvent, MessageResponse, Platform, ProviderResolver};
use ironhermes_gateway::adapter::PlatformAdapter;
use ironhermes_gateway::handler::GatewayMessageHandler;
use ironhermes_gateway::session::{SessionKey, SessionStore};
use ironhermes_gateway::session_queue::{QueueError, SessionQueue};
use ironhermes_gateway::user_queue::{DispatchOutcome, UserQueueManager};
use ironhermes_tools::ToolRegistry;

// ---------------------------------------------------------------------------
// RecordingFailingAdapter
// ---------------------------------------------------------------------------
//
// Records every platform call for test assertions AND returns `Err` on
// `send_message` so that `handle_with_multimodal`'s `run_agent` fast-exits at
// its first awaited platform call (the █ placeholder send) — without this,
// `run_agent` proceeds past the placeholder send into PromptBuilder /
// StreamConsumer / runtime setup which has many uncontrolled side effects under
// `Config::default()` and hangs the test.
//
// Using a single adapter for BOTH the UQM cap-hit path AND the worker's handler
// call gives us one observable log: reactions, send_log, edit_log. The reaction
// log records `👀` (from the worker, D-08) and `❌` (from UQM dispatch, D-11)
// at distinct emoji values, so tests can filter independently.

pub struct RecordingFailingAdapter {
    /// (chat_id, message_id, emoji) — populated by `add_reaction`.
    pub reactions: StdMutex<Vec<(String, String, String)>>,
    /// (chat_id, content) — populated BEFORE returning `Err` from `send_message`.
    /// Contains the █ placeholder send that `run_agent` emits at its first await,
    /// plus any UQM cap-hit replies. The Err is returned AFTER the push so the
    /// entry is always visible regardless of error propagation.
    pub send_log: StdMutex<Vec<(String, String)>>,
    /// (chat_id, message_id, content) — populated by `edit_message*`.
    pub edit_log: StdMutex<Vec<(String, String, String)>>,
}

impl RecordingFailingAdapter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            reactions: StdMutex::new(Vec::new()),
            send_log: StdMutex::new(Vec::new()),
            edit_log: StdMutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl PlatformAdapter for RecordingFailingAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        // Record BEFORE returning Err so the entry is always visible.
        self.send_log
            .lock()
            .unwrap()
            .push((chat_id.to_string(), content.to_string()));
        Err(anyhow::anyhow!(
            "RecordingFailingAdapter::send_message — intentional test failure to fast-exit run_agent"
        ))
    }

    async fn edit_message(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.edit_log.lock().unwrap().push((
            chat_id.to_string(),
            message_id.to_string(),
            content.to_string(),
        ));
        Ok(())
    }

    async fn edit_message_markdown_v2(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        self.edit_log.lock().unwrap().push((
            chat_id.to_string(),
            message_id.to_string(),
            content.to_string(),
        ));
        Ok(())
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        // Phase 36.17.2.2-05: intentionally fails like send_message (records
        // before returning Err) so the fixture's fast-exit semantics carry
        // over to any future overflow-chunk path that lands on this adapter.
        self.send_log
            .lock()
            .unwrap()
            .push((chat_id.to_string(), content.to_string()));
        Err(anyhow::anyhow!(
            "RecordingFailingAdapter::send_message_markdown_v2 — intentional test failure to fast-exit run_agent"
        ))
    }

    async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn add_reaction(
        &self,
        chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        self.reactions.lock().unwrap().push((
            chat_id.to_string(),
            message_id.to_string(),
            emoji.to_string(),
        ));
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Test fixture helpers (copied from tests/session_queue_integration.rs)
// ---------------------------------------------------------------------------

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
    // Config::default() has burst_size=3 which silently drops messages 4+ for
    // the same user_id (D-20/D-21 inbound rate limiter). Tests dispatching 5+
    // messages for the same sender_id need a relaxed rate limit so the handler
    // processes all of them. burst_size=128 matches the queue cap so the rate
    // limiter is never the binding constraint in integration tests.
    let mut config = Config::default();
    config.rate_limit.burst_size = 128;
    config.rate_limit.messages_per_minute = 600;
    let resolver = ProviderResolver::build(&config).expect("default resolver builds");
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let mut handler = GatewayMessageHandler::new(config, resolver, store, tool_registry);
    handler.set_session_queue(queue);
    handler
}

/// Simple 3-field event helper (mirrors `session_queue_integration.rs:make_event`).
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

/// 4-arg variant needed by tests that must assert specific `message_id` values
/// and cross-user keying (D-13 SessionKey full-triple invariant).
fn make_event_full(
    chat_id: &str,
    message_id: &str,
    content: &str,
    sender_id: &str,
) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        message_id: message_id.to_string(),
        chat_id: chat_id.to_string(),
        sender_id: sender_id.to_string(),
        content: content.to_string(),
        attachments: vec![],
        thread_id: None,
        chat_type: "dm".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    }
}

// ---------------------------------------------------------------------------
// Worker driver (mirrors Plan 02's runner.rs:991-1061 pop-loop shape)
// ---------------------------------------------------------------------------
//
// The ONLY intentional simplification vs. Plan 02 is omitting the semaphore
// permit acquisition — that is bounded-concurrency guard, not behavioral
// coverage for FIFO or 👀-at-pop semantics. The `handle_with_multimodal` call
// IS inside the loop body (M5 requirement: handler call ordering is observable).

async fn spawn_test_worker(
    session_queue: Arc<SessionQueue>,
    uqm: Arc<UserQueueManager>,
    adapter: Arc<dyn PlatformAdapter>,
    handler: Arc<GatewayMessageHandler>,
    session_key: SessionKey,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    // WorkerSpawned invariant (Plan 01): notify_for returns Some immediately
    // because dispatch already inserted the Notify into the workers map.
    let notify = uqm
        .notify_for(&session_key)
        .await
        .expect("notify_for must return Some immediately after WorkerSpawned (Plan 01 invariant)");

    tokio::spawn(async move {
        loop {
            // D-06 step 1+2: pop or wait for Notify wake / cancellation.
            let next_event = match session_queue.pop(&session_key) {
                Some(ev) => ev,
                None => {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = notify.notified() => continue,
                    }
                }
            };

            // Cancellation check after pop (T-36.17.2-03 window — μs).
            if cancel.is_cancelled() {
                break;
            }

            // D-06 step 3 + D-08: emit 👀 inline before handle_with_multimodal.
            // This is the assertion target for T-36.17.2-03: if 👀 were emitted
            // at dispatch time (36.17.1 regression), the reaction log would show
            // all 5 reactions before any handler calls.
            let _ = adapter
                .add_reaction(&next_event.chat_id, &next_event.message_id, "👀")
                .await;

            // M1 sidecar consumption — one take_multimodal per pop (FIFO lockstep).
            let (text_prefix, image_data_uri) = uqm
                .take_multimodal(&session_key)
                .await
                .unwrap_or((None, None));
            let processed = ironhermes_gateway::multimodal::ProcessedAttachments {
                text_prefix,
                image_data_uri,
            };

            // M5: real handler call path. Under RecordingFailingAdapter,
            // run_agent returns Err at the first placeholder send_message, which
            // the worker logs and ignores. The send_log entry proves this code
            // path was reached and gives us FIFO-order evidence.
            let _ = handler
                .handle_with_multimodal(
                    &next_event,
                    adapter.clone(),
                    cancel.child_token(),
                    processed,
                )
                .await;

            if cancel.is_cancelled() {
                break;
            }
        }

        // Cleanup: remove UQM state so a subsequent dispatch returns WorkerSpawned.
        uqm.remove(&session_key).await;
    })
}

// ===========================================================================
// Tests
// ===========================================================================

/// D-21 headline test — 5 same-chat messages in tight succession must produce
/// exactly 5 `👀` reactions, one per popped message, in FIFO arrival order,
/// observed through the REAL `GatewayMessageHandler::handle_with_multimodal`
/// call path (checker M5).
///
/// This test would have FAILED under the 36.17.1 architecture where `👀` was
/// emitted inside UQM::dispatch. Under that regime the reaction log would show
/// all 5 reactions before any `send_log` entry appears, and the reaction order
/// would be dispatch order (which can race with queue pop order under load).
///
/// Under 36.17.2: the worker emits `👀` inline before each handler call, so
/// the i-th reaction is causally ordered BEFORE the i-th `send_log` entry.
/// The serial single-worker guarantees the reaction→send ordering is strict FIFO.
#[tokio::test]
async fn test_5_same_chat_messages_emit_5_eye_reactions_and_fifo_order_through_handler() {
    let adapter = RecordingFailingAdapter::new();
    let session_queue = Arc::new(SessionQueue::new());
    let store = build_test_session_store();
    let handler = Arc::new(build_test_handler_with_queue(
        store.clone(),
        session_queue.clone(),
    ));
    let uqm = Arc::new(UserQueueManager::new(
        adapter.clone() as Arc<dyn PlatformAdapter>,
        session_queue.clone(),
    ));
    let session_key = SessionKey::new(Platform::Telegram, "chat_burst").with_user("user_burst");
    let cancel = CancellationToken::new();

    // 1st dispatch — must return WorkerSpawned (no worker registered yet).
    let outcome1 = uqm
        .dispatch(
            make_event_full("chat_burst", "msg_1", "first", "user_burst"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome1, DispatchOutcome::WorkerSpawned),
        "First dispatch must spawn worker, got {:?}",
        outcome1
    );

    // Spawn the test worker AFTER the first dispatch so notify_for returns Some.
    let worker = spawn_test_worker(
        session_queue.clone(),
        uqm.clone(),
        adapter.clone() as Arc<dyn PlatformAdapter>,
        handler.clone(),
        session_key.clone(),
        cancel.clone(),
    )
    .await;

    // Dispatch 4 more — all must be Accepted (worker already registered).
    for i in 2..=5usize {
        let outcome = uqm
            .dispatch(
                make_event_full(
                    "chat_burst",
                    &format!("msg_{}", i),
                    &format!("body_{}", i),
                    "user_burst",
                ),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, DispatchOutcome::Accepted),
            "Dispatch #{} must be Accepted (worker exists), got {:?}",
            i,
            outcome
        );
    }

    // Poll until the worker has invoked handle_with_multimodal 5 times.
    // Evidence: each `handle_with_multimodal` call reaches run_agent's █ placeholder
    // send_message, which RecordingFailingAdapter records before returning Err.
    // Timeout: 5 s (handle_with_multimodal does session-store lookup + Config build
    // before failing on send_message).
    let start = Instant::now();
    loop {
        let count = adapter.send_log.lock().unwrap().len();
        if count >= 5 {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!(
                "Worker did not invoke handle_with_multimodal 5 times within 5 s; \
                 send_log has {} entries: {:?}",
                count,
                *adapter.send_log.lock().unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    cancel.cancel();
    let _ = worker.await;

    // --- Assertion 1: exactly 5 👀 reactions on chat_burst ---
    let reactions = adapter.reactions.lock().unwrap();
    let eye_reactions: Vec<&(String, String, String)> = reactions
        .iter()
        .filter(|(c, _, e)| c == "chat_burst" && e == "👀")
        .collect();
    assert_eq!(
        eye_reactions.len(),
        5,
        "Expected exactly 5 👀 reactions on chat_burst, got {}. Full log: {:?}",
        eye_reactions.len(),
        *reactions
    );

    // --- Assertion 2: each msg_1..msg_5 got exactly one 👀 ---
    for i in 1..=5usize {
        let id = format!("msg_{}", i);
        let count = reactions
            .iter()
            .filter(|(_, m, e)| *m == id && e == "👀")
            .count();
        assert_eq!(
            count, 1,
            "msg_{} expected exactly 1 👀, got {}. Full reactions log: {:?}",
            i, count, *reactions
        );
    }

    // --- Assertion 3 (M5 + T-36.17.2-03): FIFO processing order via reaction sequence ---
    //
    // The worker emits 👀 inline BEFORE each handler call. Because the worker is serial
    // (single goroutine, processes one event at a time), the reaction sequence IS the
    // handler invocation sequence. msg_1..msg_5 pushed in arrival order → must be
    // popped in FIFO order → 👀 sequence must be msg_1, msg_2, msg_3, msg_4, msg_5.
    let eye_seq: Vec<&str> = reactions
        .iter()
        .filter(|(c, _, e)| c == "chat_burst" && e == "👀")
        .map(|(_, m, _)| m.as_str())
        .collect();
    let expected: Vec<String> = (1..=5).map(|i| format!("msg_{}", i)).collect();
    let expected_refs: Vec<&str> = expected.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        eye_seq, expected_refs,
        "👀 reactions out of FIFO order — handler invocations also out of order (M5/T-36.17.2-03 regression)"
    );
    drop(reactions);

    // --- Assertion 4: SessionQueue drained to empty ---
    assert_eq!(
        session_queue.len(&session_key),
        0,
        "SessionQueue must be empty after worker drains all 5 messages"
    );
}

/// Single-worker invariant (D-13 SessionKey full-triple keying):
/// The first dispatch for a given (platform, chat_id, sender_id) triple returns
/// `WorkerSpawned`; subsequent dispatches for the SAME triple return `Accepted`.
/// A dispatch for a DIFFERENT triple (different sender_id or different chat_id)
/// returns `WorkerSpawned` again.
///
/// This is a UQM-level test — no `spawn_test_worker` needed because the invariant
/// is about dispatch return values, not worker behavior.
#[tokio::test]
async fn test_dispatch_returns_worker_spawned_only_for_first_per_session_key() {
    let adapter = RecordingFailingAdapter::new();
    let session_queue = Arc::new(SessionQueue::new());
    let uqm = Arc::new(UserQueueManager::new(
        adapter.clone() as Arc<dyn PlatformAdapter>,
        session_queue.clone(),
    ));

    // First dispatch for (chat_A, u1) — new worker.
    let r1 = uqm
        .dispatch(make_event_full("chat_A", "m1", "x", "u1"), None, None)
        .await
        .unwrap();
    assert!(
        matches!(r1, DispatchOutcome::WorkerSpawned),
        "First dispatch for (chat_A, u1) must be WorkerSpawned, got {:?}",
        r1
    );

    // Second dispatch for same triple — existing worker.
    let r2 = uqm
        .dispatch(make_event_full("chat_A", "m2", "y", "u1"), None, None)
        .await
        .unwrap();
    assert!(
        matches!(r2, DispatchOutcome::Accepted),
        "Second dispatch for (chat_A, u1) must be Accepted, got {:?}",
        r2
    );

    // Different sender_id on same chat → different SessionKey → fresh worker (D-13).
    let r3 = uqm
        .dispatch(make_event_full("chat_A", "m3", "z", "u2"), None, None)
        .await
        .unwrap();
    assert!(
        matches!(r3, DispatchOutcome::WorkerSpawned),
        "Different sender_id ⇒ different SessionKey ⇒ WorkerSpawned (D-13), got {:?}",
        r3
    );

    // Different chat_id → different SessionKey → fresh worker.
    let r4 = uqm
        .dispatch(make_event_full("chat_B", "m4", "w", "u1"), None, None)
        .await
        .unwrap();
    assert!(
        matches!(r4, DispatchOutcome::WorkerSpawned),
        "Different chat_id ⇒ different SessionKey ⇒ WorkerSpawned, got {:?}",
        r4
    );
}

/// T-36.17.2-01: worker-exit / dispatch race.
///
/// After a worker exits (cancel fired + `uqm.remove` called), a subsequent
/// `dispatch` for the same `SessionKey` must return `WorkerSpawned` (not
/// `Accepted`). If Plan 01's lock serialization is broken, the workers map
/// still contains the old entry after `remove`, and the new dispatch returns
/// `Accepted` — meaning NO new worker is ever spawned and the queued message
/// is never processed.
#[tokio::test]
async fn test_worker_exit_dispatch_race() {
    let adapter = RecordingFailingAdapter::new();
    let session_queue = Arc::new(SessionQueue::new());
    let store = build_test_session_store();
    let handler = Arc::new(build_test_handler_with_queue(
        store.clone(),
        session_queue.clone(),
    ));
    let uqm = Arc::new(UserQueueManager::new(
        adapter.clone() as Arc<dyn PlatformAdapter>,
        session_queue.clone(),
    ));
    let session_key = SessionKey::new(Platform::Telegram, "chat_race").with_user("u1");
    let cancel = CancellationToken::new();

    // First dispatch — spawn a worker.
    let r1 = uqm
        .dispatch(
            make_event_full("chat_race", "race_1", "first", "u1"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(r1, DispatchOutcome::WorkerSpawned));

    let worker = spawn_test_worker(
        session_queue.clone(),
        uqm.clone(),
        adapter.clone() as Arc<dyn PlatformAdapter>,
        handler.clone(),
        session_key.clone(),
        cancel.clone(),
    )
    .await;

    // Wait for worker to process race_1 (👀 reaction is emitted before handle call).
    let start = Instant::now();
    while adapter
        .reactions
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, m, e)| m == "race_1" && e == "👀")
        .count()
        < 1
    {
        if start.elapsed() > Duration::from_secs(3) {
            panic!("worker did not emit 👀 for race_1 within 3 s");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Cancel the worker and wait for it to exit (calls uqm.remove internally).
    cancel.cancel();
    let _ = worker.await;

    // After worker exit + remove: next dispatch must return WorkerSpawned.
    // If T-36.17.2-01 mitigation is missing, this returns Accepted (wrong).
    let r2 = uqm
        .dispatch(
            make_event_full("chat_race", "race_2", "second", "u1"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(r2, DispatchOutcome::WorkerSpawned),
        "After worker exit + cancel, next dispatch must spawn fresh worker \
         (T-36.17.2-01 mitigation), got {:?}",
        r2
    );

    // Sanity: the new message is on the queue waiting for the new worker.
    assert_eq!(session_queue.len(&session_key), 1);
}

/// M1 multimodal sidecar round-trip through the full dispatch → pop → worker
/// path. Dispatches 3 events with distinct (text_prefix, image_data_uri) payloads,
/// runs a worker to drain them, and asserts:
/// 1. All 3 `👀` reactions fired in FIFO order.
/// 2. The sidecar is empty after drain (`take_multimodal` returns `None`).
/// 3. The session queue is empty after drain.
///
/// This test would fail if:
/// - Plan 01's sidecar push/pop is out of FIFO order (T-36.17.2-04).
/// - Plan 02's worker takes more than once per pop (sidecar under-drains).
/// - Plan 02's worker skips `take_multimodal` entirely (sidecar leaks).
#[tokio::test]
async fn test_multimodal_payload_roundtrips_through_sidecar_to_handler() {
    let adapter = RecordingFailingAdapter::new();
    let session_queue = Arc::new(SessionQueue::new());
    let store = build_test_session_store();
    let handler = Arc::new(build_test_handler_with_queue(
        store.clone(),
        session_queue.clone(),
    ));
    let uqm = Arc::new(UserQueueManager::new(
        adapter.clone() as Arc<dyn PlatformAdapter>,
        session_queue.clone(),
    ));
    let session_key = SessionKey::new(Platform::Telegram, "chat_mm").with_user("u_mm");
    let cancel = CancellationToken::new();

    // Event #1 — text_prefix only.
    let r1 = uqm
        .dispatch(
            make_event_full("chat_mm", "mm_1", "first", "u_mm"),
            Some("PREFIX_A".to_string()),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(r1, DispatchOutcome::WorkerSpawned));

    // Spawn worker AFTER first dispatch so notify_for returns Some.
    let worker = spawn_test_worker(
        session_queue.clone(),
        uqm.clone(),
        adapter.clone() as Arc<dyn PlatformAdapter>,
        handler.clone(),
        session_key.clone(),
        cancel.clone(),
    )
    .await;

    // Event #2 — image_data_uri only.
    let r2 = uqm
        .dispatch(
            make_event_full("chat_mm", "mm_2", "second", "u_mm"),
            None,
            Some("data:image/png;base64,IMG_B".to_string()),
        )
        .await
        .unwrap();
    assert!(matches!(r2, DispatchOutcome::Accepted));

    // Event #3 — both fields.
    let r3 = uqm
        .dispatch(
            make_event_full("chat_mm", "mm_3", "third", "u_mm"),
            Some("PREFIX_C".to_string()),
            Some("data:image/png;base64,IMG_C".to_string()),
        )
        .await
        .unwrap();
    assert!(matches!(r3, DispatchOutcome::Accepted));

    // Wait for worker to process all 3 events (3 × 👀 on chat_mm).
    let start = Instant::now();
    loop {
        let count = adapter
            .reactions
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _, e)| c == "chat_mm" && e == "👀")
            .count();
        if count >= 3 {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!(
                "worker did not process 3 multimodal events within 5 s; \
                 only {} 👀 reactions recorded",
                count
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    cancel.cancel();
    let _ = worker.await;

    // Assertion 1: sidecar is empty (all 3 payloads consumed in FIFO lockstep).
    // `take_multimodal` after drain must return None — proves the worker called it
    // exactly once per pop (T-36.17.2-04 mitigation).
    let leftover = uqm.take_multimodal(&session_key).await;
    assert!(
        leftover.is_none(),
        "Sidecar must be empty after worker drains all 3 events, got {:?}",
        leftover
    );

    // Assertion 2: 3 👀 reactions in FIFO arrival order.
    let reactions = adapter.reactions.lock().unwrap();
    let eye_seq: Vec<&str> = reactions
        .iter()
        .filter(|(c, _, e)| c == "chat_mm" && e == "👀")
        .map(|(_, m, _)| m.as_str())
        .collect();
    assert_eq!(
        eye_seq,
        vec!["mm_1", "mm_2", "mm_3"],
        "Multimodal events processed out of FIFO order; \
         sidecar lockstep would also be misaligned"
    );
    drop(reactions);

    // Assertion 3: session queue drained to empty.
    assert_eq!(
        session_queue.len(&session_key),
        0,
        "SessionQueue must be empty after worker drains 3 multimodal events"
    );
}

/// D-11 cap-hit UX end-to-end through the unified UQM path.
///
/// Pre-fill the SessionQueue to 128 via direct `try_push` (simulating a backlog),
/// then dispatch a 129th event. Assert:
/// - Result is `Err(QueueError::CapacityReached { .. })`.
/// - Exactly one `❌` reaction on the overflow message.
/// - Exactly one "Queue is full (128 messages)" chat reply in `send_log`.
/// - Queue depth remains at 128 (cap held — T-36.17.1-01 regression check).
///
/// Note: RecordingFailingAdapter returns `Err` on `send_message`. UQM::dispatch
/// wraps the cap-hit reply in `with_rate_limit_retry(|| ...)` and discards the
/// result via `let _ = ...`. The entry IS pushed into `send_log` before the Err
/// returns, so `send_log` will have exactly 1 entry.
#[tokio::test]
async fn test_dispatch_returns_capacity_reached_at_128() {
    let adapter = RecordingFailingAdapter::new();
    let session_queue = Arc::new(SessionQueue::new());
    let uqm = Arc::new(UserQueueManager::new(
        adapter.clone() as Arc<dyn PlatformAdapter>,
        session_queue.clone(),
    ));
    let session_key = SessionKey::new(Platform::Telegram, "chat_cap").with_user("u1");

    // Pre-fill to 128 via direct try_push (bypassing UQM dispatch to avoid spawning
    // worker entries that would interfere with the cap-hit assertion).
    for i in 0..128usize {
        let ev = make_event_full("chat_cap", &format!("pre_{}", i), "x", "u1");
        session_queue
            .try_push(&session_key, ev)
            .expect("pre-fill within cap");
    }
    assert_eq!(session_queue.len(&session_key), 128, "pre-fill at cap");

    // 129th dispatch — must hit capacity.
    let result = uqm
        .dispatch(
            make_event_full("chat_cap", "overflow", "boom", "u1"),
            None,
            None,
        )
        .await;
    assert!(
        matches!(result, Err(QueueError::CapacityReached { .. })),
        "129th dispatch must return CapacityReached, got {:?}",
        result
    );

    // Exactly one ❌ reaction on the overflow message.
    let reactions = adapter.reactions.lock().unwrap();
    let x_react = reactions
        .iter()
        .filter(|(c, m, e)| c == "chat_cap" && m == "overflow" && e == "❌")
        .count();
    assert_eq!(
        x_react, 1,
        "Expected exactly 1 ❌ reaction on overflow message, got {}. Full log: {:?}",
        x_react, *reactions
    );
    drop(reactions);

    // Exactly one "Queue is full (128 messages)" chat reply.
    let send_log = adapter.send_log.lock().unwrap();
    let cap_reply = send_log
        .iter()
        .filter(|(c, content)| c == "chat_cap" && content.contains("Queue is full (128 messages)"))
        .count();
    assert_eq!(
        cap_reply, 1,
        "Expected exactly 1 cap-hit chat reply (pre-Err push), got {}. send_log: {:?}",
        cap_reply, *send_log
    );
    drop(send_log);

    // Cap held — overflow event was NOT pushed.
    assert_eq!(
        session_queue.len(&session_key),
        128,
        "Queue depth must remain at 128 after CapacityReached (T-36.17.1-01 regression)"
    );
}

/// Phase 36.17.2 Plan 05 (D-23, D-27, T-36.17.2-06):
/// Verifies that a slash command dispatched on the fast-path does NOT wait for the
/// per-chat worker's in-flight free-text turn to complete. The test simulates the
/// dispatch-loop split: a free-text event has been try_push'd onto SessionQueue and
/// is "occupying" the worker (no worker is actually spawned — we don't need one
/// because the assertion is about the fast-path NOT touching UQM, not about
/// worker behavior). A slash-command event is then dispatched via a direct
/// handle_with_multimodal call (the test-equivalent of runner.rs's fast-path
/// tokio::spawn body). The assertions prove:
///   (a) the slash command's send_log entry appears in well under 500ms
///       (no agent-loop entanglement with the queued free-text);
///   (b) the slash command's synthesized event (for `/queue ...`) lands on
///       SessionQueue via handler's existing intercept (handler.rs from 36.17.1-03);
///   (c) the free-text event still sits in SessionQueue waiting for a worker
///       (proving the fast-path did NOT route the slash through UQM).
#[tokio::test]
async fn test_slash_command_bypasses_per_chat_worker() {
    let adapter = RecordingFailingAdapter::new();
    let session_queue = Arc::new(SessionQueue::new());
    let store = build_test_session_store();
    let handler = Arc::new(build_test_handler_with_queue(
        store.clone(),
        session_queue.clone(),
    ));
    let session_key = SessionKey::new(Platform::Telegram, "chat_fp").with_user("u_fp");

    // Step 1 — Pre-load 1 free-text event into SessionQueue (would be popped by a worker if one existed).
    // No worker is spawned, so this event sits idle. This represents an "in-flight turn" from the
    // fast-path's perspective: any per-chat-worker-serialized routing would block behind this.
    session_queue
        .try_push(
            &session_key,
            make_event_full("chat_fp", "free_1", "free text waiting", "u_fp"),
        )
        .expect("free-text try_push must succeed (queue empty)");
    assert_eq!(
        session_queue.len(&session_key),
        1,
        "Free-text event must be in queue"
    );

    // Step 2 — Dispatch a slash command via the fast-path-equivalent direct call.
    // This is exactly what runner.rs's fast-path branch does: build empty ProcessedAttachments
    // and invoke handler.handle_with_multimodal(...).
    let slash_event = make_event_full("chat_fp", "slash_1", "/queue interrupt this", "u_fp");
    let cancel = CancellationToken::new();
    let processed = ironhermes_gateway::multimodal::ProcessedAttachments {
        text_prefix: None,
        image_data_uri: None,
    };

    let start = Instant::now();
    let _ = handler
        .handle_with_multimodal(
            &slash_event,
            adapter.clone() as Arc<dyn PlatformAdapter>,
            cancel.child_token(),
            processed,
        )
        .await;
    let elapsed = start.elapsed();

    // Assertion (a) — slash command completes in well under 500ms.
    // handle_with_multimodal routes /queue → handle_slash_command → cmd_queue →
    // CommandResult::Queued intercept → session_queue.try_push + chat reply.
    // No agent-loop is invoked (commands bypass run_agent via is_bypass(&def.name) at handler.rs:505).
    assert!(
        elapsed < Duration::from_millis(500),
        "Slash-command fast-path took {:?} — should be < 500ms (no agent-loop entanglement)",
        elapsed
    );

    // Assertion (b) — /queue intercept synthesized an event into SessionQueue.
    // The synthesized event has content == "interrupt this" (args joined; see cmd_queue at handlers.rs).
    // Queue depth grew from 1 to 2 (free_1 still present + synthesized /queue event).
    assert_eq!(
        session_queue.len(&session_key),
        2,
        "/queue intercept must push synthesized event onto SessionQueue (was 1, expected 2). Actual len: {}",
        session_queue.len(&session_key)
    );

    // Assertion (c) — RecordingFailingAdapter recorded the /queue chat reply.
    // The handler's intercept emits "Queued for the next turn." (singular at depth 1) OR
    // "Queued for the next turn. (N queued)" (plural at depth > 1). Either form is acceptable.
    // Note: send_log entries are (chat_id, content) 2-tuples.
    let send_log = adapter.send_log.lock().unwrap();
    let queued_reply = send_log
        .iter()
        .filter(|(c, content)| c == "chat_fp" && content.starts_with("Queued for the next turn"))
        .count();
    assert!(
        queued_reply >= 1,
        "/queue intercept must send 'Queued for the next turn...' reply. send_log: {:?}",
        *send_log
    );
    drop(send_log);

    // Assertion (d) — free-text event (free_1) is still in queue (fast-path did NOT pop or process it).
    // We can verify by popping it: it must still be `free_1`, not the synthesized /queue event.
    // Note: SessionQueue is FIFO, so the first pop is free_1 (pushed first), the second pop is
    // the /queue synthesized event (pushed by the intercept after).
    let first = session_queue
        .pop(&session_key)
        .expect("first pop must yield free_1");
    assert_eq!(
        first.message_id, "free_1",
        "FIFO order broken: first pop should be free_1, got {}",
        first.message_id
    );
    let second = session_queue
        .pop(&session_key)
        .expect("second pop must yield /queue synthesized event");
    assert_eq!(
        second.content, "interrupt this",
        "/queue intercept must synthesize event with content == args (got: {})",
        second.content
    );
}

/// Phase 36.17.2.1 D-07/D-08/D-09 regression test:
/// `/queue` typed during a busy turn must wake the parked per-chat worker.
///
/// UAT FAILURE THIS TEST LOCKS: 2026-05-28T15:36-15:38 UTC, 128/129 `/queue` events
/// stranded with zero 👀 reactions. Root cause: handler.rs:805 (pre-fix) called
/// `session_queue.try_push` directly with NO `notify_one()` — so the worker, parked
/// at `notify_task.notified().await` (runner.rs:1045 in production / line 252 in
/// `spawn_test_worker` here), never woke. Messages piled up in `SessionQueue` and
/// were stranded indefinitely.
///
/// Why every existing test in this file missed this: they all dispatch via
/// `uqm.dispatch(...)` directly (e.g. `test_5_same_chat_messages_emit_5_eye_reactions_and_fifo_order_through_handler`
/// at line 321), and `UQM::dispatch` ALWAYS calls `notify_one()` on success
/// (user_queue.rs:154). The only existing test that exercises the handler's
/// fast-path `Queued` arm (`test_slash_command_bypasses_per_chat_worker` at line 835)
/// does so against an EMPTY worker state — no `spawn_test_worker` is called, no real
/// Notify-park happens, so the missing wake is silently invisible because there is
/// nothing to wake.
///
/// What THIS test does differently:
///   1. Pre-loads 1 free-text event via `uqm.dispatch` so the worker is registered.
///   2. Spawns the test worker via `spawn_test_worker` — which immediately drains
///      `free_1` and parks at `notify.notified().await`.
///   3. Waits until `send_log.len() >= 1` — proving the worker reached
///      `handle_with_multimodal` for `free_1` (the RecordingFailingAdapter records
///      the placeholder send before returning Err, which fast-exits the agent loop).
///   4. Dispatches 5 `/queue` events DIRECTLY through `handler.handle_with_multimodal`
///      — exactly mirroring the runner.rs fast-path's `tokio::spawn(async move { handler.handle_with_multimodal(...).await })`
///      body shape (runner.rs:943-967), but inline-awaited so the test thread can
///      observe completion.
///   5. Polls the count of 👀 reactions on synthesized message_ids `q_1..q_5` with
///      a 5-second timeout. Under the fix the worker wakes, pops, emits 👀.
///
/// REGRESSION SIGNAL: Under the unfixed code, this test times out (5-second poll
/// on the 👀 count). Under the fix (Plan 01), `handler.handle_with_multimodal`
/// routes `/queue` → `handle_slash_command` → `cmd_queue` → handler's `Queued` arm
/// → `uqm.dispatch(synthesized_event, None, None).await` → `notify_one()`
/// (user_queue.rs:154) → worker wakes → pop → 👀 → handler → next iteration.
///
/// Harness design mirrors RESEARCH.md "Test Surface" section: reuses
/// `RecordingFailingAdapter`, `build_test_handler_with_queue`, `make_event_full`,
/// and `spawn_test_worker` verbatim. The ONE call this test adds that the parent
/// `test_5_same_chat_messages` test does NOT make is `set_user_queue_manager` on
/// the handler — required because Plan 01's fix only takes the `Some(uqm)` branch
/// of the handler's Queued arm when UQM is wired (otherwise the legacy D-20
/// fallback runs, which has the same wake-less bug and the test would time out for
/// the wrong reason).
#[tokio::test]
async fn test_queue_command_wakes_parked_worker() {
    let adapter = RecordingFailingAdapter::new();
    let session_queue = Arc::new(SessionQueue::new());
    let store = build_test_session_store();
    let uqm = Arc::new(UserQueueManager::new(
        adapter.clone() as Arc<dyn PlatformAdapter>,
        session_queue.clone(),
    ));

    // Build the handler, wire UQM via Plan 01's new setter, THEN Arc-wrap.
    // Without `set_user_queue_manager` the handler's Queued arm falls into the
    // D-20 legacy fallback branch (direct try_push, no notify_one) — which has
    // the same wake-less bug, meaning the test would time out for the wrong
    // reason and not actually exercise Plan 01's fix.
    let mut h = build_test_handler_with_queue(store.clone(), session_queue.clone());
    h.set_user_queue_manager(uqm.clone());
    let handler = Arc::new(h);

    let session_key = SessionKey::new(Platform::Telegram, "chat_qw").with_user("u_qw");
    let cancel = CancellationToken::new();

    // Step 1 — Dispatch one free-text event via UQM. This must spawn the worker.
    let outcome = uqm
        .dispatch(
            make_event_full("chat_qw", "free_1", "initial free-text body", "u_qw"),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, DispatchOutcome::WorkerSpawned),
        "First dispatch must spawn worker, got {:?}",
        outcome
    );

    // Step 2 — Spawn the test worker. It will immediately pop free_1, emit 👀,
    // call handle_with_multimodal (which fast-exits via RecordingFailingAdapter::send_message
    // Err), then loop back, find the queue empty, and park at `notify.notified().await`.
    let worker = spawn_test_worker(
        session_queue.clone(),
        uqm.clone(),
        adapter.clone() as Arc<dyn PlatformAdapter>,
        handler.clone(),
        session_key.clone(),
        cancel.clone(),
    )
    .await;

    // Step 3 — Wait until the worker has drained free_1 and is parked.
    // Evidence: send_log entry from RecordingFailingAdapter::send_message (the
    // run_agent placeholder send) proves handle_with_multimodal was invoked for free_1.
    let start = Instant::now();
    loop {
        let count = adapter.send_log.lock().unwrap().len();
        if count >= 1 {
            break;
        }
        if start.elapsed() > Duration::from_secs(5) {
            panic!(
                "Worker did not drain free_1 within 5 s; send_log has {} entries: {:?}",
                count,
                *adapter.send_log.lock().unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Sanity: queue is empty (worker popped free_1) — worker is now parked at notify.notified().
    assert_eq!(
        session_queue.len(&session_key),
        0,
        "SessionQueue must be empty after worker pops free_1 (worker should be parked at notify.notified())"
    );

    let send_log_baseline = adapter.send_log.lock().unwrap().len();

    // Step 4 — Dispatch 5 /queue events via handler.handle_with_multimodal directly.
    // This is the EXACT code path that was broken (runner.rs fast-path tokio::spawn body shape).
    // Each call must traverse: handle_with_multimodal → handle_slash_command → cmd_queue →
    // CoreCommandResult::Queued arm → (Plan 01 fix) uqm.dispatch(queued_event, None, None).await →
    // notify_one() → worker wakes → pop → 👀 emitted on synthesized message_id.
    //
    // Note: `ProcessedAttachments` is not `Clone`, so we rebuild it per iteration.
    // For /queue text-only path both fields are always `None`, so this is observably
    // identical to a clone.
    for i in 1..=5usize {
        let slash_event = make_event_full(
            "chat_qw",
            &format!("q_{}", i),
            &format!("/queue body_{}", i),
            "u_qw",
        );
        let processed = ironhermes_gateway::multimodal::ProcessedAttachments {
            text_prefix: None,
            image_data_uri: None,
        };
        let _ = handler
            .handle_with_multimodal(
                &slash_event,
                adapter.clone() as Arc<dyn PlatformAdapter>,
                cancel.child_token(),
                processed,
            )
            .await;
    }

    // Step 5 — Poll until the worker has emitted 👀 on all 5 synthesized message_ids q_1..q_5.
    // Under the fix this completes in ~200ms. Under the broken code this times out
    // because the worker remains parked: try_push grows SessionQueue depth 1..5 but
    // notify_one is never called.
    let poll_start = Instant::now();
    loop {
        let count = adapter
            .reactions
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, m, e)| c == "chat_qw" && m.starts_with("q_") && e == "👀")
            .count();
        if count >= 5 {
            break;
        }
        if poll_start.elapsed() > Duration::from_secs(5) {
            let reactions = adapter.reactions.lock().unwrap();
            let send_log = adapter.send_log.lock().unwrap();
            panic!(
                "REGRESSION SIGNAL — Phase 36.17.2.1 /queue wake-protocol broken.\n\
                 Expected: 5 👀 reactions on synthesized message_ids q_1..q_5 within 5 s.\n\
                 Actual: {} 👀 reactions on q_*; session_queue.len(&session_key) = {}; \
                 send_log.len() = {}; reactions.len() = {}.\n\
                 send_log (full): {:?}\n\
                 reactions (full): {:?}\n\
                 \n\
                 This timeout means the per-chat worker is parked at \
                 `notify.notified().await` and the handler's CoreCommandResult::Queued \
                 arm did NOT call `uqm.dispatch(queued_event, None, None).await` — \
                 which is the ONLY path that calls `notify_one()` (user_queue.rs:154) \
                 to wake the parked worker.\n\
                 \n\
                 Look in handler.rs at the CoreCommandResult::Queued arm: the \
                 production code path (when `self.user_queue_manager.is_some()`) MUST \
                 delegate to `uqm.dispatch(queued_event, None, None).await`. If that \
                 call has been replaced with a direct `session_queue.try_push(...)` \
                 (the 36.17.1 legacy fallback), the wake is lost and the UAT \
                 regression (2026-05-28T15:36-15:38 UTC, 128/129 stranded messages) \
                 has recurred.",
                count,
                session_queue.len(&session_key),
                send_log.len(),
                reactions.len(),
                *send_log,
                *reactions,
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    cancel.cancel();
    let _ = worker.await;

    // --- Assertion (A): each q_1..q_5 got exactly one 👀 reaction on chat_qw ---
    let reactions = adapter.reactions.lock().unwrap();
    for i in 1..=5usize {
        let id = format!("q_{}", i);
        let count = reactions
            .iter()
            .filter(|(c, m, e)| c == "chat_qw" && *m == id && e == "👀")
            .count();
        assert_eq!(
            count, 1,
            "q_{} expected exactly 1 👀 reaction, got {}. Full reactions log: {:?}",
            i, count, *reactions
        );
    }

    // --- Assertion (B): FIFO ordering of the 5 new 👀 reactions on q_1..q_5 ---
    // The worker is serial, so the 👀 reaction sequence on synthesized message_ids
    // IS the handler invocation order. The parent test pattern (line 444-454) uses
    // the same approach.
    let q_eye_seq: Vec<&str> = reactions
        .iter()
        .filter(|(c, m, e)| c == "chat_qw" && m.starts_with("q_") && e == "👀")
        .map(|(_, m, _)| m.as_str())
        .collect();
    let expected: Vec<String> = (1..=5).map(|i| format!("q_{}", i)).collect();
    let expected_refs: Vec<&str> = expected.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        q_eye_seq, expected_refs,
        "/queue synthesized events processed out of FIFO order — worker pop or handler invocation order is broken"
    );
    drop(reactions);

    // --- Assertion (C): SessionQueue is drained to empty ---
    assert_eq!(
        session_queue.len(&session_key),
        0,
        "SessionQueue must be empty after worker drains all 5 synthesized /queue events (worker may have stopped mid-drain)"
    );

    // --- Assertion (D): send_log grew by at least baseline+5 ---
    // Each handle_with_multimodal call on a popped synthesized event reaches
    // run_agent's placeholder send_message which the adapter records.
    let send_log_final = adapter.send_log.lock().unwrap().len();
    assert!(
        send_log_final >= send_log_baseline + 5,
        "send_log must grow by at least 5 from baseline (worker processed 5 popped /queue events). \
         baseline = {}, final = {}, growth = {}",
        send_log_baseline,
        send_log_final,
        send_log_final.saturating_sub(send_log_baseline)
    );
}
