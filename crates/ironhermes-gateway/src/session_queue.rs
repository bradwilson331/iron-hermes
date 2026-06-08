//! Per-session FIFO queue (Phase 36.17.1).
//!
//! Production data structure backing the `/queue` command and busy-agent
//! routing. Python parity: `gateway/run.py` §2304-2415 (`_enqueue_fifo`,
//! `_promote_queued_event`, `_queue_depth`) and §1007 (`_dequeue_pending_event`).
//!
//! See `.planning/phases/36.17.1-in-mem-fifo-queuing-parity-of-python-deque-for-chat-sessions/`
//! for the full spec.
//!
//! ## Concurrency
//!
//! All methods are **synchronous** (D-17). The internal `Mutex` is
//! `std::sync::Mutex` (D-16). The async-mutex flavor and the third-party
//! spinlock crate are deliberately not used here. Critical sections are
//! O(1) and microseconds long; chat-message arrival rates do not warrant
//! `DashMap`/`RwLock` complexity.
//!
//! Callers MUST NOT hold the lock guard across `.await`. None of the methods
//! here are `async`, and `tracing::warn!` does not await — so the guard is
//! safe to hold across the soft-warn emission.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use ironhermes_core::MessageEvent;
use tracing::warn;

use crate::session::SessionKey;

// Phase 36.17.3 (Resolution 3 / D-01): the per-session cap and warn thresholds
// were relocated into `ironhermes-core::queue` so every `MessageQueue<K>`
// consumer enforces the same cap at compile time (T-01 / T-36.17.1-01
// mitigation across all transports). The gateway re-exports them so all
// existing `crate::session_queue::MAX_QUEUE_DEPTH` / `WARN_QUEUE_DEPTH` call
// sites continue to compile unchanged.
pub use ironhermes_core::queue::{MAX_QUEUE_DEPTH, WARN_QUEUE_DEPTH};

// Phase 36.17.3 (Resolution 3 / D-01): `QueueError` was relocated into
// `ironhermes-core::queue` so every `MessageQueue<K>` consumer returns the
// same error type. The gateway re-exports it so existing
// `use ironhermes_gateway::session_queue::QueueError;` call sites continue
// to compile unchanged.
pub use ironhermes_core::queue::QueueError;

/// Per-session FIFO message queue.
///
/// One `VecDeque<MessageEvent>` per `SessionKey` (platform + chat_id + user_id
/// triple). Replaces hermes-agent's two-layer `pending_slot` + `overflow_list`
/// design with a single contiguous queue (D-06). The two-layer Python structure
/// is reproduced byte-for-byte as a `#[cfg(test)] mod parity` reference
/// implementation (Plan 01 Task 3) that proptest proves observably equivalent
/// to this type — zero runtime cost (D-07).
///
/// # Python parity
/// - `try_push` ↔ `_enqueue_fifo` (gateway/run.py:2315)
/// - `pop` ↔ `_dequeue_pending_event` (gateway/run.py:1007)
/// - `len` ↔ `_queue_depth` (gateway/run.py:2366)
/// - `clear` ↔ /new + /reset clearing call-sites
/// - `retain` ↔ `_clear_goal_pending_continuations` layout (goal predicate
///   deferred per D-04)
///
/// # Concurrency
/// Sync-only (D-17). The inner `Mutex` is held only for O(1) HashMap +
/// VecDeque ops. The lock guard is `!Send`, so the compiler enforces that it
/// never crosses an `.await` boundary.
pub struct SessionQueue {
    queues: Mutex<HashMap<SessionKey, VecDeque<MessageEvent>>>,
}

impl SessionQueue {
    /// Construct an empty queue. No per-session state is allocated until the
    /// first `try_push` for that key.
    pub fn new() -> Self {
        Self {
            queues: Mutex::new(HashMap::new()),
        }
    }

    /// Push an event onto the back of the per-session FIFO.
    ///
    /// Returns `Err(QueueError::CapacityReached)` if the session's queue
    /// already holds `MAX_QUEUE_DEPTH` events — the event is dropped and the
    /// caller is responsible for cap-hit UX.
    ///
    /// Emits `tracing::warn!` once the queue reaches `WARN_QUEUE_DEPTH`
    /// (counting the just-attempted push).
    ///
    /// # Python parity
    /// `_enqueue_fifo` (gateway/run.py:2315). The two-layer split — slot vs
    /// overflow — collapses to a single `VecDeque::push_back`. Observable
    /// behavior is identical (proven by `mod parity` proptest in Task 3).
    pub fn try_push(&self, key: &SessionKey, event: MessageEvent) -> Result<(), QueueError> {
        let mut map = self.queues.lock().unwrap();
        let q = map.entry(key.clone()).or_default();
        if q.len() >= MAX_QUEUE_DEPTH {
            return Err(QueueError::CapacityReached {
                session_key: key.to_string_key(),
                max: MAX_QUEUE_DEPTH,
            });
        }
        // Soft-warn at 75% capacity (counting the about-to-push event).
        // `tracing::warn!` does not await — safe to emit inside the guard.
        if q.len() + 1 >= WARN_QUEUE_DEPTH {
            warn!(
                session_key = %key.to_string_key(),
                depth = q.len() + 1,
                "SessionQueue soft-warn: approaching capacity"
            );
        }
        q.push_back(event);
        Ok(())
    }

    /// Pop the oldest event from the per-session FIFO, or `None` if empty.
    ///
    /// Removes the entire map entry when the inner `VecDeque` becomes empty
    /// to avoid empty-deque accretion across long-lived gateway processes.
    ///
    /// # Python parity
    /// `_dequeue_pending_event` (gateway/run.py:1007). In Python this pops
    /// from `adapter._pending_messages`; here it pops the head of the
    /// single contiguous queue.
    pub fn pop(&self, key: &SessionKey) -> Option<MessageEvent> {
        let mut map = self.queues.lock().unwrap();
        let q = map.get_mut(key)?;
        let item = q.pop_front();
        if q.is_empty() {
            map.remove(key);
        }
        item
    }

    /// Current depth of the per-session FIFO, or `0` if the session has no
    /// queue allocated yet.
    ///
    /// # Python parity
    /// `_queue_depth` (gateway/run.py:2366). The Python version sums slot +
    /// overflow lengths; the unified Rust queue returns `VecDeque::len`.
    pub fn len(&self, key: &SessionKey) -> usize {
        self.queues
            .lock()
            .unwrap()
            .get(key)
            .map(|q| q.len())
            .unwrap_or(0)
    }

    /// Returns true iff `len(key) == 0`. Convenience method — semantically
    /// equivalent to `self.len(key) == 0`.
    pub fn is_empty(&self, key: &SessionKey) -> bool {
        self.len(key) == 0
    }

    /// Drop every queued event for the given session (no-op if missing).
    ///
    /// Called by `/new` and `/reset` slash-command handlers immediately
    /// BEFORE `SessionStore::remove` (per RESEARCH.md Pitfall 5).
    pub fn clear(&self, key: &SessionKey) {
        self.queues.lock().unwrap().remove(key);
    }

    /// Retain only events matching `predicate`, in arrival order. The entry
    /// is removed entirely if the retained queue is empty.
    ///
    /// # Python parity
    /// `_clear_goal_pending_continuations` (gateway/run.py:2385) — selective
    /// clear. The goal-continuation predicate is deferred per D-04 (no `/goal`
    /// in IronHermes yet); this method is the general mechanism it would
    /// build on.
    pub fn retain<F: Fn(&MessageEvent) -> bool>(&self, key: &SessionKey, predicate: F) {
        let mut map = self.queues.lock().unwrap();
        if let Some(q) = map.get_mut(key) {
            q.retain(|e| predicate(e));
            if q.is_empty() {
                map.remove(key);
            }
        }
    }
}

impl Default for SessionQueue {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Phase 36.17.3 (D-01 / D-02 / Resolution 5): MessageQueue<SessionKey> adapter
// =============================================================================
//
// `SessionQueue` implements the cross-consumer `ironhermes_core::queue::MessageQueue`
// trait via a `String`->`MessageEvent` adapter. The trait surface uses `String`
// so non-gateway consumers (TUI, future web/Discord/Slack/REST) don't have to
// construct a gateway-specific `MessageEvent` — the wrapping is the gateway's
// concern. Every existing concrete-API call site (handler.rs / user_queue.rs /
// runner.rs / tests) continues to use `SessionQueue::try_push(&self, key,
// MessageEvent)` unchanged — the trait impl is purely additive (T-03
// mitigation). Per the threat model and Resolution 5.

impl ironhermes_core::queue::MessageQueue<SessionKey> for SessionQueue {
    fn try_push(
        &self,
        key: &SessionKey,
        message: String,
    ) -> Result<(), ironhermes_core::queue::QueueError> {
        // Adapter: wrap the bare `String` into a minimal `MessageEvent`
        // mirroring the field set the handler populates today. `sender_id`
        // is sourced from `key.user_id` (empty when absent, matching the
        // handler's behavior for non-user-keyed channels). All other fields
        // are gateway-internal metadata that the trait surface deliberately
        // does not carry.
        let event = MessageEvent {
            content: message,
            platform: key.platform.clone(),
            chat_id: key.chat_id.clone(),
            sender_id: key.user_id.clone().unwrap_or_default(),
            message_id: String::new(),
            attachments: Vec::new(),
            thread_id: None,
            chat_type: "queue".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        };
        SessionQueue::try_push(self, key, event)
    }

    fn pop(&self, key: &SessionKey) -> Option<String> {
        SessionQueue::pop(self, key).map(|e| e.content)
    }

    fn len(&self, key: &SessionKey) -> usize {
        SessionQueue::len(self, key)
    }

    fn clear(&self, key: &SessionKey) {
        SessionQueue::clear(self, key);
    }
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::Platform;

    /// Build a deterministic-shaped `MessageEvent` whose `content` is the only
    /// distinguishing field — used to assert FIFO order and predicate hits.
    fn fixture_event(content: &str) -> MessageEvent {
        MessageEvent {
            platform: Platform::Telegram,
            message_id: "msg-0".to_string(),
            chat_id: "chat-0".to_string(),
            sender_id: "user-0".to_string(),
            content: content.to_string(),
            attachments: Vec::new(),
            thread_id: None,
            chat_type: "dm".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        }
    }

    fn key(chat: &str) -> SessionKey {
        SessionKey::new(Platform::Telegram, chat).with_user("u1")
    }

    #[test]
    fn test_fifo_order() {
        let q = SessionQueue::new();
        let k = key("c1");
        q.try_push(&k, fixture_event("A")).unwrap();
        q.try_push(&k, fixture_event("B")).unwrap();
        q.try_push(&k, fixture_event("C")).unwrap();

        assert_eq!(q.pop(&k).unwrap().content, "A");
        assert_eq!(q.pop(&k).unwrap().content, "B");
        assert_eq!(q.pop(&k).unwrap().content, "C");
        assert!(q.pop(&k).is_none());
    }

    #[test]
    fn test_cap_at_128() {
        let q = SessionQueue::new();
        let k = key("c1");

        // 128 pushes succeed.
        for i in 0..MAX_QUEUE_DEPTH {
            q.try_push(&k, fixture_event(&format!("msg-{i}"))).unwrap();
        }
        assert_eq!(q.len(&k), MAX_QUEUE_DEPTH);

        // 129th push returns CapacityReached.
        let err = q.try_push(&k, fixture_event("overflow")).unwrap_err();
        match err {
            QueueError::CapacityReached { session_key, max } => {
                assert_eq!(max, MAX_QUEUE_DEPTH);
                assert_eq!(session_key, k.to_string_key());
            }
        }
        // Depth unchanged.
        assert_eq!(q.len(&k), MAX_QUEUE_DEPTH);

        // First message is still the oldest (drop-newest, NOT drop-oldest per D-10).
        assert_eq!(q.pop(&k).unwrap().content, "msg-0");
    }

    #[test]
    fn test_soft_warn_at_96() {
        use std::sync::{Arc, Mutex as StdMutex};
        use tracing::Level;
        use tracing_subscriber::fmt::MakeWriter;

        /// Minimal MakeWriter that captures every emitted record into a Vec<u8>.
        #[derive(Clone)]
        struct VecWriter(Arc<StdMutex<Vec<u8>>>);
        impl std::io::Write for VecWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for VecWriter {
            type Writer = VecWriter;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buf: Arc<StdMutex<Vec<u8>>> = Arc::new(StdMutex::new(Vec::new()));
        let writer = VecWriter(Arc::clone(&buf));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(Level::WARN)
            .with_writer(writer)
            .with_ansi(false)
            .finish();

        let q = SessionQueue::new();
        let k = key("c-warn");

        tracing::subscriber::with_default(subscriber, || {
            // First 95 pushes: NO warn (depth-after-push 1..=95 < 96).
            for i in 0..(WARN_QUEUE_DEPTH - 1) {
                q.try_push(&k, fixture_event(&format!("ok-{i}"))).unwrap();
            }
            let mid = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
            assert!(
                !mid.contains("approaching capacity"),
                "soft-warn fired before reaching WARN_QUEUE_DEPTH: {mid}"
            );

            // 96th push: depth-after = 96 == WARN_QUEUE_DEPTH → warn emits.
            q.try_push(&k, fixture_event("warn-trigger")).unwrap();
        });

        let captured = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            captured.contains("approaching capacity"),
            "expected soft-warn at depth {WARN_QUEUE_DEPTH}, got: {captured}"
        );
        assert!(
            captured.contains(&k.to_string_key()),
            "warn should include session_key, got: {captured}"
        );
    }

    #[test]
    fn test_clear_removes_all() {
        let q = SessionQueue::new();
        let k = key("c1");
        for i in 0..5 {
            q.try_push(&k, fixture_event(&format!("m-{i}"))).unwrap();
        }
        assert_eq!(q.len(&k), 5);
        q.clear(&k);
        assert_eq!(q.len(&k), 0);
        assert!(q.pop(&k).is_none());
    }

    #[test]
    fn test_retain_predicate() {
        let q = SessionQueue::new();
        let k = key("c1");
        // Alternate KEEP / DROP contents.
        for i in 0..5 {
            let tag = if i % 2 == 0 { "KEEP" } else { "DROP" };
            q.try_push(&k, fixture_event(&format!("{tag}-{i}")))
                .unwrap();
        }
        assert_eq!(q.len(&k), 5);
        q.retain(&k, |e| e.content.starts_with("KEEP"));
        assert_eq!(q.len(&k), 3); // i=0,2,4

        // Order preserved.
        assert_eq!(q.pop(&k).unwrap().content, "KEEP-0");
        assert_eq!(q.pop(&k).unwrap().content, "KEEP-2");
        assert_eq!(q.pop(&k).unwrap().content, "KEEP-4");
        assert!(q.pop(&k).is_none());
    }

    #[test]
    fn test_session_isolation() {
        let q = SessionQueue::new();
        let a = key("chat-A");
        let b = key("chat-B");

        q.try_push(&a, fixture_event("A1")).unwrap();
        q.try_push(&a, fixture_event("A2")).unwrap();
        q.try_push(&b, fixture_event("B1")).unwrap();

        assert_eq!(q.len(&a), 2);
        assert_eq!(q.len(&b), 1);

        assert_eq!(q.pop(&a).unwrap().content, "A1");
        // Popping from A did not affect B.
        assert_eq!(q.len(&b), 1);
        assert_eq!(q.pop(&b).unwrap().content, "B1");
        assert!(q.pop(&b).is_none());
        // A still has A2.
        assert_eq!(q.pop(&a).unwrap().content, "A2");
    }

    #[test]
    fn test_empty_pop_returns_none() {
        let q = SessionQueue::new();
        let k = key("never-pushed");
        // pop on an empty/missing key returns None and does NOT panic.
        assert!(q.pop(&k).is_none());
        assert_eq!(q.len(&k), 0);
    }

    #[test]
    fn test_clear_idempotent() {
        let q = SessionQueue::new();
        let k = key("never-pushed");
        // clear on a missing key is a no-op (no panic).
        q.clear(&k);
        q.clear(&k); // idempotent
        assert_eq!(q.len(&k), 0);
    }

    #[test]
    fn test_session_key_full_triple_used() {
        // T-36.17.1-02 mitigation: SessionKeys differing ONLY in user_id
        // must produce independent queues. Proves no cross-session leak
        // when two distinct users share the same chat_id+platform.
        let q = SessionQueue::new();
        let alice = SessionKey::new(Platform::Telegram, "shared-chat").with_user("alice");
        let bob = SessionKey::new(Platform::Telegram, "shared-chat").with_user("bob");

        q.try_push(&alice, fixture_event("alice-1")).unwrap();
        q.try_push(&bob, fixture_event("bob-1")).unwrap();
        q.try_push(&alice, fixture_event("alice-2")).unwrap();

        assert_eq!(q.len(&alice), 2);
        assert_eq!(q.len(&bob), 1);

        // Alice pops alice's events, bob's untouched.
        assert_eq!(q.pop(&alice).unwrap().content, "alice-1");
        assert_eq!(q.pop(&alice).unwrap().content, "alice-2");
        assert!(q.pop(&alice).is_none());
        assert_eq!(q.len(&bob), 1);

        // Bob's event still there.
        assert_eq!(q.pop(&bob).unwrap().content, "bob-1");
    }
}

// =============================================================================
// Parity mirror — #[cfg(test)] only (D-07: zero runtime cost)
// =============================================================================
//
// Mirrors Python's two-layer `pending_slot` + `overflow` layout from
// `gateway/run.py` §2304-2415 byte-for-byte, then drives both `SessionQueue`
// and `SplitSlotQueue` through the same arbitrary op sequence via proptest
// (1024 cases) to prove observable equivalence.
//
// Nothing in this module is referenced by the production binary. A release
// build of `ironhermes-gateway` does not link any of these symbols.

#[cfg(test)]
mod parity {
    use super::*;
    use ironhermes_core::Platform;
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// Common surface exercised by both queue implementations under proptest.
    /// `&str` keys (not `SessionKey`) — the mirror only needs string keys for
    /// equivalence; the production type already proves SessionKey isolation in
    /// `tests::test_session_isolation` and `tests::test_session_key_full_triple_used`.
    trait SessionFifoQueue {
        fn push(&mut self, key: &str, event: MessageEvent) -> Result<(), QueueError>;
        fn pop(&mut self, key: &str) -> Option<MessageEvent>;
        fn len(&self, key: &str) -> usize;
        fn clear(&mut self, key: &str);
    }

    /// Build a fixture event whose `content` carries the proptest identity.
    /// All other fields are constant — equivalence is asserted on `content`.
    fn fixture(content: &str) -> MessageEvent {
        MessageEvent {
            platform: Platform::Telegram,
            message_id: "m".to_string(),
            chat_id: "c".to_string(),
            sender_id: "s".to_string(),
            content: content.to_string(),
            attachments: Vec::new(),
            thread_id: None,
            chat_type: "dm".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        }
    }

    fn str_to_session_key(key: &str) -> SessionKey {
        SessionKey::new(Platform::Telegram, key).with_user("proptest")
    }

    // ---------- Adapter over the real production SessionQueue ----------

    impl SessionFifoQueue for SessionQueue {
        fn push(&mut self, key: &str, event: MessageEvent) -> Result<(), QueueError> {
            SessionQueue::try_push(self, &str_to_session_key(key), event)
        }
        fn pop(&mut self, key: &str) -> Option<MessageEvent> {
            SessionQueue::pop(self, &str_to_session_key(key))
        }
        fn len(&self, key: &str) -> usize {
            SessionQueue::len(self, &str_to_session_key(key))
        }
        fn clear(&mut self, key: &str) {
            SessionQueue::clear(self, &str_to_session_key(key));
        }
    }

    // ---------- Python parity mirror ----------

    /// Two-layer FIFO mirroring `gateway/run.py` §2304-2415:
    ///   - `pending_slot`: adapter._pending_messages (single-slot per session)
    ///   - `overflow`:     self._queued_events (overflow list per session)
    ///
    /// Push: if slot occupied → append overflow; else → fill slot.
    /// Pop : take slot, then promote overflow head into slot (Python's
    ///       `_dequeue_pending_event` + `_promote_queued_event` composed).
    /// Len : `(slot occupied) + overflow.len()`  (Python `_queue_depth`).
    /// Clear: drop slot AND overflow for this session.
    struct SplitSlotQueue {
        pending_slot: HashMap<String, MessageEvent>,
        overflow: HashMap<String, Vec<MessageEvent>>,
    }

    impl SplitSlotQueue {
        fn new() -> Self {
            Self {
                pending_slot: HashMap::new(),
                overflow: HashMap::new(),
            }
        }
    }

    impl SessionFifoQueue for SplitSlotQueue {
        // _enqueue_fifo (run.py:2315) + MAX_QUEUE_DEPTH cap (D-09)
        fn push(&mut self, key: &str, event: MessageEvent) -> Result<(), QueueError> {
            let depth = self.len(key);
            if depth >= MAX_QUEUE_DEPTH {
                return Err(QueueError::CapacityReached {
                    session_key: str_to_session_key(key).to_string_key(),
                    max: MAX_QUEUE_DEPTH,
                });
            }
            if self.pending_slot.contains_key(key) {
                // Slot occupied → append to overflow (run.py:2326).
                self.overflow
                    .entry(key.to_string())
                    .or_default()
                    .push(event);
            } else {
                // Slot free → install (run.py:2328).
                self.pending_slot.insert(key.to_string(), event);
            }
            Ok(())
        }

        // _dequeue_pending_event (run.py:1007) followed by _promote_queued_event (run.py:2331).
        // Composed because callers observe one logical "pop" — the slot value is
        // returned, and the overflow head (if any) is promoted into the slot for
        // the next pop. This mirrors what `VecDeque::pop_front` does in one step.
        fn pop(&mut self, key: &str) -> Option<MessageEvent> {
            let taken = self.pending_slot.remove(key)?;
            // Promote overflow head into the slot.
            let overflow_now_empty = match self.overflow.get_mut(key) {
                Some(o) if !o.is_empty() => {
                    let next = o.remove(0);
                    self.pending_slot.insert(key.to_string(), next);
                    o.is_empty()
                }
                Some(o) => o.is_empty(),
                None => true,
            };
            if overflow_now_empty {
                self.overflow.remove(key);
            }
            Some(taken)
        }

        // _queue_depth (run.py:2366).
        fn len(&self, key: &str) -> usize {
            let in_slot = if self.pending_slot.contains_key(key) {
                1
            } else {
                0
            };
            let in_overflow = self.overflow.get(key).map(|v| v.len()).unwrap_or(0);
            in_slot + in_overflow
        }

        // /new + /reset clearing (run.py:6788 and adapter clearing).
        fn clear(&mut self, key: &str) {
            self.pending_slot.remove(key);
            self.overflow.remove(key);
        }
    }

    // ---------- Op alphabet driving both queues ----------

    #[derive(Debug, Clone)]
    enum Op {
        Push(String, String),
        Pop(String),
        Len(String),
        Clear(String),
    }

    /// Proptest strategy: bounded key/content alphabets so the cap is exercised
    /// within `0..200` ops per case. Three keys × ~200 ops per case lets some
    /// runs reach `MAX_QUEUE_DEPTH = 128` (and is uniform across queues).
    fn op_strategy() -> impl Strategy<Value = Op> {
        let key_strategy = prop_oneof!["a", "b", "c"];
        let payload_strategy = "[a-z]{1,4}";
        prop_oneof![
            (key_strategy.clone(), payload_strategy)
                .prop_map(|(k, v)| Op::Push(k.to_string(), v.to_string())),
            key_strategy.clone().prop_map(|k| Op::Pop(k.to_string())),
            key_strategy.clone().prop_map(|k| Op::Len(k.to_string())),
            key_strategy.prop_map(|k| Op::Clear(k.to_string())),
        ]
    }

    /// Observable side-effects from running a single Op against a queue.
    ///   - Push:  emits `PushResult(Ok/Err)`.
    ///   - Pop:   emits `Popped(Option<content>)`.
    ///   - Len:   emits `Length(usize)`.
    ///   - Clear: emits no observation (no return value).
    #[derive(Debug, PartialEq, Eq)]
    enum Observation {
        PushOk,
        PushErr,
        Popped(Option<String>),
        Length(usize),
    }

    fn drive<Q: SessionFifoQueue>(queue: &mut Q, ops: &[Op]) -> Vec<Observation> {
        let mut out = Vec::with_capacity(ops.len());
        for op in ops {
            match op {
                Op::Push(k, content) => {
                    let event = fixture(content);
                    match queue.push(k, event) {
                        Ok(()) => out.push(Observation::PushOk),
                        Err(_) => out.push(Observation::PushErr),
                    }
                }
                Op::Pop(k) => {
                    let popped = queue.pop(k).map(|e| e.content);
                    out.push(Observation::Popped(popped));
                }
                Op::Len(k) => {
                    out.push(Observation::Length(queue.len(k)));
                }
                Op::Clear(k) => {
                    queue.clear(k);
                }
            }
        }
        out
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 1024,
            .. ProptestConfig::default()
        })]

        /// For any sequence of push/pop/len/clear ops, `SessionQueue` and
        /// `SplitSlotQueue` produce identical observation sequences.
        ///
        /// Closes T-36.17.1-01 (behavioral DoS-bound parity) and proves D-07
        /// (the production unified `VecDeque` is observably equivalent to
        /// Python's two-layer pending_slot + overflow layout).
        #[test]
        fn proptest_equivalence(ops in prop::collection::vec(op_strategy(), 0..200)) {
            let mut session_queue = SessionQueue::new();
            let mut split_slot = SplitSlotQueue::new();

            let observed_session = drive(&mut session_queue, &ops);
            let observed_split = drive(&mut split_slot, &ops);

            prop_assert_eq!(observed_session, observed_split);
        }

        /// Push-only sequences never let `SessionQueue::len` exceed
        /// `MAX_QUEUE_DEPTH` — explicit T-36.17.1-01 memory-DoS evidence.
        #[test]
        fn proptest_cap_invariant(
            keys in prop::collection::vec(prop_oneof!["a", "b", "c"], 0..300)
        ) {
            let q = SessionQueue::new();
            for k in &keys {
                let _ = q.try_push(&str_to_session_key(k), fixture("x"));
                prop_assert!(q.len(&str_to_session_key(k)) <= MAX_QUEUE_DEPTH);
            }
            for k in ["a", "b", "c"] {
                prop_assert!(q.len(&str_to_session_key(k)) <= MAX_QUEUE_DEPTH);
            }
        }
    }

    /// Smoke test for the mirror itself — pre-flight check that
    /// `SplitSlotQueue` implements FIFO correctly before proptest compares it
    /// against `SessionQueue`.
    #[test]
    fn split_slot_smoke() {
        let mut q = SplitSlotQueue::new();
        q.push("k", fixture("A")).unwrap();
        q.push("k", fixture("B")).unwrap();
        q.push("k", fixture("C")).unwrap();
        assert_eq!(q.len("k"), 3);

        assert_eq!(q.pop("k").unwrap().content, "A");
        assert_eq!(q.len("k"), 2);
        assert_eq!(q.pop("k").unwrap().content, "B");
        assert_eq!(q.len("k"), 1);
        assert_eq!(q.pop("k").unwrap().content, "C");
        assert_eq!(q.len("k"), 0);
        assert!(q.pop("k").is_none());
    }

    /// Sanity: the mirror also rejects at MAX_QUEUE_DEPTH (so push-cap parity
    /// is real, not a coincidence from sequences not reaching the cap).
    #[test]
    fn split_slot_cap_at_128() {
        let mut q = SplitSlotQueue::new();
        for i in 0..MAX_QUEUE_DEPTH {
            q.push("k", fixture(&format!("m-{i}"))).unwrap();
        }
        assert_eq!(q.len("k"), MAX_QUEUE_DEPTH);
        let err = q.push("k", fixture("overflow")).unwrap_err();
        match err {
            QueueError::CapacityReached { max, .. } => assert_eq!(max, MAX_QUEUE_DEPTH),
        }
        assert_eq!(q.len("k"), MAX_QUEUE_DEPTH);
    }
}
