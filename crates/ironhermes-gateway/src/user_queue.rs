use crate::adapter::PlatformAdapter;
use crate::rate_limiter::with_rate_limit_retry;
use crate::session::SessionKey;
use crate::session_queue::{QueueError, SessionQueue};
use ironhermes_core::MessageEvent;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, warn};

/// Outcome returned by `UserQueueManager::dispatch` on a successful push (D-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Message landed on the queue; an existing worker will pick it up via Notify wake.
    Accepted,
    /// Message landed AND the caller (runner.rs dispatch loop) must spawn a fresh worker
    /// for this SessionKey.
    WorkerSpawned,
}

/// Manages per-chat message queuing via `SessionQueue` (D-01, D-03).
///
/// Each unique `SessionKey` (platform + chat_id + user_id — D-13/D-14) gets an entry in
/// the `workers` map: an `Arc<Notify>` that the per-chat worker parks on when the queue is
/// empty, and that `dispatch` signals on every successful push (D-19).
///
/// A parallel sidecar `pending_multimodal` preserves `(text_prefix, image_data_uri)` per
/// dispatched event in FIFO lockstep with `SessionQueue` (D-02 — multimodal pass-through
/// MUST be preserved across the 36.17.1 → 36.17.2 architectural shift).
///
/// The `❌` cap-hit UX fires inside `dispatch` (D-11) so all transports get it for free.
///
/// The `👀` reaction does NOT fire here — emission moves to the per-chat worker (D-08).
pub struct UserQueueManager {
    /// Worker presence + Notify wake-up registry, keyed by full SessionKey triple (D-13, D-19).
    workers: Mutex<HashMap<SessionKey, Arc<Notify>>>,
    /// FIFO sidecar for multimodal payload — preserves (text_prefix, image_data_uri) per
    /// dispatched event in lockstep with SessionQueue. Worker calls take_multimodal() once per
    /// pop, restoring 1-to-1 FIFO ordering between event and payload (D-02 — multimodal
    /// pass-through MUST be preserved across the 36.17.1 → 36.17.2 architectural shift;
    /// only 👀-timing and cap-hit signal are user-visible changes per CONTEXT.md).
    #[allow(clippy::type_complexity)]
    // multimodal FIFO per session; type alias would only exist here, inline is clearer
    // Tuple is (text_prefix, image_data_uri, image_cache_path) — the third element
    // carries the inbound photo's cache PATH so the worker can offer it to the model
    // for image-to-video (video_animate needs a path, not the vision data URI).
    pending_multimodal:
        Mutex<HashMap<SessionKey, VecDeque<(Option<String>, Option<String>, Option<String>)>>>,
    /// Shared SessionQueue handle (D-03 — UQM holds Arc<SessionQueue>, NOT Arc<GatewayRunner>).
    session_queue: Arc<SessionQueue>,
    /// Outbound transport adapter for cap-hit UX (D-11).
    adapter: Arc<dyn PlatformAdapter>,
}

impl UserQueueManager {
    /// Create a new `UserQueueManager`.
    ///
    /// `session_queue` is the shared `Arc<SessionQueue>` already on `GatewayRunner`
    /// (per 36.17.1-02 SUMMARY — D-03: UQM holds Arc<SessionQueue>, not Arc<GatewayRunner>).
    pub fn new(adapter: Arc<dyn PlatformAdapter>, session_queue: Arc<SessionQueue>) -> Self {
        Self {
            workers: Mutex::new(HashMap::new()),
            pending_multimodal: Mutex::new(HashMap::new()),
            session_queue,
            adapter,
        }
    }

    /// Push a multimodal payload onto the per-session sidecar FIFO.
    ///
    /// Called immediately after a successful `SessionQueue::try_push` so the payload is
    /// FIFO-aligned with the event itself (D-02 — T-36.17.2-04 mitigation).
    pub async fn push_multimodal(
        &self,
        key: &SessionKey,
        payload: (Option<String>, Option<String>, Option<String>),
    ) {
        let mut map = self.pending_multimodal.lock().await;
        map.entry(key.clone()).or_default().push_back(payload);
    }

    /// Pop the oldest multimodal payload for this session.
    ///
    /// Called by the per-chat worker (Plan 02) once per `SessionQueue::pop`, restoring
    /// 1-to-1 FIFO ordering between event and payload.
    ///
    /// Returns `None` if no payload is queued (sidecar is empty OR key never had multimodal
    /// traffic — normal for text-only messages).
    pub async fn take_multimodal(
        &self,
        key: &SessionKey,
    ) -> Option<(Option<String>, Option<String>, Option<String>)> {
        let mut map = self.pending_multimodal.lock().await;
        map.get_mut(key).and_then(|q| q.pop_front())
    }

    /// Dispatch a message to the appropriate per-chat `SessionQueue` slot.
    ///
    /// Returns:
    /// - `Ok(DispatchOutcome::WorkerSpawned)` — message landed AND the caller must spawn a
    ///   fresh worker for this `SessionKey`.
    /// - `Ok(DispatchOutcome::Accepted)` — message landed; existing worker will wake via Notify.
    /// - `Err(QueueError::CapacityReached)` — queue full; ❌ reaction + chat reply already sent.
    ///
    /// The 👀 reaction does NOT fire here — emission is Plan 02's exclusive responsibility (D-08).
    pub async fn dispatch(
        &self,
        event: MessageEvent,
        text_prefix: Option<String>,
        image_data_uri: Option<String>,
        image_cache_path: Option<String>,
    ) -> Result<DispatchOutcome, QueueError> {
        // Step 1 — Build full SessionKey (D-14, matches runner.rs:994-998 + handler.rs construction).
        let session_key =
            SessionKey::new(event.platform.clone(), &event.chat_id).with_user(&event.sender_id);
        let chat_id = event.chat_id.clone();
        let message_id = event.message_id.clone();

        // Step 2 — Push event onto SessionQueue (sync, returns immediately; MutexGuard is !Send
        // so it cannot cross an await — D-18 + Pitfall 2 from 36.17.1 RESEARCH.md).
        if let Err(e) = self.session_queue.try_push(&session_key, event) {
            // Step 3 (cap-hit branch, D-11) — fire ❌ reaction (best-effort, ignore failure)
            // then send "Queue is full" chat reply via with_rate_limit_retry.
            let _ = self.adapter.add_reaction(&chat_id, &message_id, "❌").await;
            // with_rate_limit_retry now lives in rate_limiter.rs as pub(crate) per M2.
            let adapter = self.adapter.clone();
            let cid = chat_id.clone();
            let _ = with_rate_limit_retry(|| {
                let a = adapter.clone();
                let c = cid.clone();
                async move {
                    a.send_message(
                        &c,
                        "⏳ Queue is full (128 messages). Wait for the agent to drain before sending more.",
                        None,
                    )
                    .await
                    .map(|_| ())
                }
            })
            .await;
            warn!(
                session = %session_key.to_string_key(),
                "UserQueueManager: capacity reached, message dropped (Phase 36.17.2 D-11)"
            );
            return Err(e);
        }

        // Step 4 — Push multimodal payload onto sidecar FIFO in lockstep with the event
        // (D-02 preservation, T-36.17.2-04: only push sidecar on successful try_push).
        self.push_multimodal(
            &session_key,
            (text_prefix, image_data_uri, image_cache_path),
        )
        .await;

        // Step 5 — Acquire workers map under tokio::sync::Mutex (D-18 — guard may cross await
        // safely because SessionQueue's std::sync::MutexGuard already dropped at end of Step 2).
        let mut workers = self.workers.lock().await;

        // Step 6 (T-36.17.2-01 — race re-check under lock) — decide Accepted vs WorkerSpawned.
        if let Some(notify) = workers.get(&session_key).cloned() {
            // Worker exists. Drop the lock BEFORE signaling (D-19 — never notify under lock).
            drop(workers);
            notify.notify_one();
            debug!(session = %session_key.to_string_key(), "Message accepted by existing worker");
            Ok(DispatchOutcome::Accepted)
        } else {
            // No worker. Insert a fresh Arc<Notify> and instruct caller to spawn.
            let notify = Arc::new(Notify::new());
            workers.insert(session_key.clone(), notify);
            drop(workers);
            debug!(session = %session_key.to_string_key(), "First message — caller must spawn worker");
            Ok(DispatchOutcome::WorkerSpawned)
        }
    }

    /// Return a clone of the `Arc<Notify>` for this session, if a worker has been registered.
    ///
    /// Plan 02's worker spawn block calls this immediately after observing
    /// `DispatchOutcome::WorkerSpawned` so it can park on `notify.notified().await` when the
    /// queue drains. Returns `None` if no worker is registered.
    ///
    /// `pub async fn` because `workers` uses `tokio::sync::Mutex` (D-18 + checker M4).
    pub async fn notify_for(&self, key: &SessionKey) -> Option<Arc<Notify>> {
        let workers = self.workers.lock().await;
        workers.get(key).cloned()
    }

    /// Remove a session's worker registration and any leftover multimodal payload.
    ///
    /// Called by the per-chat worker (Plan 02) when it exits its pop loop. Idempotent —
    /// calling on an absent `SessionKey` is a no-op (D-13, D-16).
    pub async fn remove(&self, session_key: &SessionKey) {
        {
            let mut workers = self.workers.lock().await;
            workers.remove(session_key);
        }
        {
            let mut sidecar = self.pending_multimodal.lock().await;
            sidecar.remove(session_key);
        }
        debug!(
            session = %session_key.to_string_key(),
            "Per-chat worker exited, removed UQM state (Phase 36.17.2)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::{MessageResponse, Platform};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---------------------------------------------------------------------------
    // MockAdapter — records calls for test assertions
    // ---------------------------------------------------------------------------

    struct MockAdapter {
        reaction_count: Arc<AtomicUsize>,
        last_reaction_emoji: Arc<Mutex<Option<String>>>,
        send_message_count: Arc<AtomicUsize>,
        last_send_content: Arc<Mutex<Option<String>>>,
    }

    impl MockAdapter {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                reaction_count: Arc::new(AtomicUsize::new(0)),
                last_reaction_emoji: Arc::new(Mutex::new(None)),
                send_message_count: Arc::new(AtomicUsize::new(0)),
                last_send_content: Arc::new(Mutex::new(None)),
            })
        }
    }

    #[async_trait]
    impl PlatformAdapter for MockAdapter {
        fn platform(&self) -> Platform {
            Platform::Telegram
        }

        async fn send_message(
            &self,
            chat_id: &str,
            content: &str,
            _thread_id: Option<&str>,
        ) -> anyhow::Result<MessageResponse> {
            self.send_message_count.fetch_add(1, Ordering::SeqCst);
            *self.last_send_content.lock().await = Some(content.to_string());
            Ok(MessageResponse {
                message_id: "1".into(),
                chat_id: chat_id.to_string(),
                platform: Platform::Telegram,
            })
        }

        async fn edit_message(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _content: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn edit_message_markdown_v2(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _content: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn send_message_markdown_v2(
            &self,
            chat_id: &str,
            content: &str,
            thread_id: Option<&str>,
        ) -> anyhow::Result<MessageResponse> {
            // Phase 36.17.2.2-05: this fixture exercises the cap-hit reply
            // path which uses plain `send_message`; the MarkdownV2 sibling
            // is not on any tested path here. Delegate to send_message so
            // any future test that does exercise it sees identical
            // behavior.
            self.send_message(chat_id, content, thread_id).await
        }

        async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn add_reaction(
            &self,
            _chat_id: &str,
            _message_id: &str,
            emoji: &str,
        ) -> anyhow::Result<()> {
            self.reaction_count.fetch_add(1, Ordering::SeqCst);
            *self.last_reaction_emoji.lock().await = Some(emoji.to_string());
            Ok(())
        }

        fn is_running(&self) -> bool {
            true
        }
    }

    // ---------------------------------------------------------------------------
    // Event helpers
    // ---------------------------------------------------------------------------

    fn make_event(chat_id: &str, message_id: &str) -> MessageEvent {
        MessageEvent {
            platform: Platform::Telegram,
            message_id: message_id.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "user1".to_string(),
            content: "hello".to_string(),
            attachments: vec![],
            thread_id: None,
            chat_type: "dm".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        }
    }

    fn make_event_with_sender(chat_id: &str, message_id: &str, sender_id: &str) -> MessageEvent {
        MessageEvent {
            platform: Platform::Telegram,
            message_id: message_id.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: sender_id.to_string(),
            content: "hello".to_string(),
            attachments: vec![],
            thread_id: None,
            chat_type: "dm".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        }
    }

    fn make_uqm(adapter: Arc<MockAdapter>) -> UserQueueManager {
        UserQueueManager::new(adapter, Arc::new(SessionQueue::new()))
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_first_dispatch_returns_worker_spawned() {
        let adapter = MockAdapter::new();
        let session_queue = Arc::new(SessionQueue::new());
        let manager = UserQueueManager::new(adapter, session_queue.clone());

        let key = SessionKey::new(Platform::Telegram, "chat1").with_user("user1");
        let result = manager
            .dispatch(make_event("chat1", "msg1"), None, None, None)
            .await;
        assert_eq!(result, Ok(DispatchOutcome::WorkerSpawned));
        assert_eq!(session_queue.len(&key), 1);
    }

    #[tokio::test]
    async fn test_second_dispatch_returns_accepted_and_does_not_react() {
        let adapter = MockAdapter::new();
        let session_queue = Arc::new(SessionQueue::new());
        let manager = UserQueueManager::new(adapter.clone(), session_queue.clone());

        let key = SessionKey::new(Platform::Telegram, "chat1").with_user("user1");

        // First dispatch — WorkerSpawned
        let r1 = manager
            .dispatch(make_event("chat1", "msg1"), None, None, None)
            .await;
        assert_eq!(r1, Ok(DispatchOutcome::WorkerSpawned));

        // Second dispatch — Accepted (worker registered from first dispatch)
        let r2 = manager
            .dispatch(make_event("chat1", "msg2"), None, None, None)
            .await;
        assert_eq!(r2, Ok(DispatchOutcome::Accepted));

        // No 👀 reaction should have fired — that's Plan 02's job (D-08)
        assert_eq!(
            adapter.reaction_count.load(Ordering::SeqCst),
            0,
            "UQM must not emit 👀 reaction — that is Plan 02's responsibility"
        );
        assert_eq!(session_queue.len(&key), 2);
    }

    #[tokio::test]
    async fn test_different_session_keys_get_independent_workers() {
        let adapter = MockAdapter::new();
        let manager = make_uqm(adapter.clone());

        let r1 = manager
            .dispatch(make_event("chat1", "msg1"), None, None, None)
            .await;
        let r2 = manager
            .dispatch(make_event("chat2", "msg2"), None, None, None)
            .await;

        assert_eq!(r1, Ok(DispatchOutcome::WorkerSpawned));
        assert_eq!(r2, Ok(DispatchOutcome::WorkerSpawned));
        // No reactions for first messages of each chat
        assert_eq!(adapter.reaction_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_remove_clears_entry_and_new_dispatch_creates_worker() {
        let adapter = MockAdapter::new();
        let manager = make_uqm(adapter);

        let key = SessionKey::new(Platform::Telegram, "chat1").with_user("user1");

        let _r1 = manager
            .dispatch(make_event("chat1", "msg1"), None, None, None)
            .await;
        manager.remove(&key).await;

        // After remove, next dispatch should return WorkerSpawned (fresh worker needed)
        let r2 = manager
            .dispatch(make_event("chat1", "msg2"), None, None, None)
            .await;
        assert_eq!(
            r2,
            Ok(DispatchOutcome::WorkerSpawned),
            "After remove, dispatch should create a new worker"
        );
    }

    #[tokio::test]
    async fn test_cap_hit_emits_x_reaction_and_chat_reply() {
        let adapter = MockAdapter::new();
        let session_queue = Arc::new(SessionQueue::new());
        let manager = UserQueueManager::new(adapter.clone(), session_queue.clone());

        let key = SessionKey::new(Platform::Telegram, "chat1").with_user("user1");

        // Pre-fill session_queue to 128 events via direct try_push
        for i in 0..128 {
            session_queue
                .try_push(&key, make_event("chat1", &format!("pre-{i}")))
                .unwrap();
        }
        assert_eq!(session_queue.len(&key), 128);

        // dispatch should hit cap on try_push → fire ❌ reaction + chat reply → Err
        let result = manager
            .dispatch(make_event("chat1", "overflow"), None, None, None)
            .await;

        assert!(
            matches!(result, Err(QueueError::CapacityReached { .. })),
            "Expected CapacityReached, got: {:?}",
            result
        );
        assert_eq!(
            adapter.reaction_count.load(Ordering::SeqCst),
            1,
            "Should add one ❌ reaction"
        );
        assert_eq!(
            *adapter.last_reaction_emoji.lock().await,
            Some("❌".to_string())
        );
        assert_eq!(
            adapter.send_message_count.load(Ordering::SeqCst),
            1,
            "Should send one cap-hit message"
        );
        let content = adapter
            .last_send_content
            .lock()
            .await
            .clone()
            .unwrap_or_default();
        assert!(
            content.contains("Queue is full (128 messages)"),
            "Cap-hit message should mention queue depth, got: {content}"
        );
        // Queue remains at cap, overflow event was NOT pushed
        assert_eq!(session_queue.len(&key), 128);
    }

    #[tokio::test]
    async fn test_session_key_uses_full_triple() {
        // D-13 — two events with same chat_id but different sender_id → distinct SessionKey
        // triples → distinct worker entries → both return WorkerSpawned.
        let adapter = MockAdapter::new();
        let manager = make_uqm(adapter.clone());

        let r1 = manager
            .dispatch(
                make_event_with_sender("chat1", "msg1", "alice"),
                None,
                None,
                None,
            )
            .await;
        let r2 = manager
            .dispatch(
                make_event_with_sender("chat1", "msg2", "bob"),
                None,
                None,
                None,
            )
            .await;

        assert_eq!(r1, Ok(DispatchOutcome::WorkerSpawned));
        assert_eq!(r2, Ok(DispatchOutcome::WorkerSpawned));
        assert_eq!(adapter.reaction_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_notify_for_returns_arc_after_dispatch() {
        let adapter = MockAdapter::new();
        let manager = make_uqm(adapter);

        let key = SessionKey::new(Platform::Telegram, "chat1").with_user("user1");

        // Before dispatch: None
        assert!(manager.notify_for(&key).await.is_none());

        // After dispatch: Some
        let _ = manager
            .dispatch(make_event("chat1", "msg1"), None, None, None)
            .await;
        assert!(
            manager.notify_for(&key).await.is_some(),
            "notify_for should return Some after WorkerSpawned dispatch"
        );

        // After remove: None again
        manager.remove(&key).await;
        assert!(
            manager.notify_for(&key).await.is_none(),
            "notify_for should return None after remove"
        );
    }

    #[tokio::test]
    async fn test_multimodal_sidecar_fifo() {
        // M1 — dispatch three events with distinct multimodal payloads, verify FIFO pop order.
        let adapter = MockAdapter::new();
        let manager = make_uqm(adapter);

        let key = SessionKey::new(Platform::Telegram, "chat1").with_user("user1");

        let _ = manager
            .dispatch(
                make_event("chat1", "msg1"),
                Some("prefix_A".into()),
                None,
                None,
            )
            .await;
        let _ = manager
            .dispatch(
                make_event("chat1", "msg2"),
                None,
                Some("data:image/png;base64,IMG_B".into()),
                // image_cache_path must survive the sidecar round-trip alongside the data URI.
                Some("/cache/images/IMG_B.jpg".into()),
            )
            .await;
        let _ = manager
            .dispatch(
                make_event("chat1", "msg3"),
                Some("prefix_C".into()),
                Some("data:image/png;base64,IMG_C".into()),
                None,
            )
            .await;

        // FIFO pop order must match push order
        assert_eq!(
            manager.take_multimodal(&key).await,
            Some((Some("prefix_A".into()), None, None))
        );
        assert_eq!(
            manager.take_multimodal(&key).await,
            Some((
                None,
                Some("data:image/png;base64,IMG_B".into()),
                Some("/cache/images/IMG_B.jpg".into())
            ))
        );
        assert_eq!(
            manager.take_multimodal(&key).await,
            Some((
                Some("prefix_C".into()),
                Some("data:image/png;base64,IMG_C".into()),
                None
            ))
        );
        // Sidecar drained — 4th take returns None
        assert_eq!(manager.take_multimodal(&key).await, None);
    }

    #[tokio::test]
    async fn test_take_multimodal_from_empty_returns_none() {
        let adapter = MockAdapter::new();
        let manager = make_uqm(adapter);

        let key = SessionKey::new(Platform::Telegram, "chat1").with_user("user1");

        // Key never dispatched → None
        assert_eq!(manager.take_multimodal(&key).await, None);

        // Dispatch one event with (None, None) payload
        let _ = manager
            .dispatch(make_event("chat1", "msg1"), None, None, None)
            .await;
        // First take: Some((None, None, None))
        assert_eq!(
            manager.take_multimodal(&key).await,
            Some((None, None, None))
        );
        // Second take: None (sidecar drained)
        assert_eq!(manager.take_multimodal(&key).await, None);
    }
}
