//! Shared FIFO queue abstraction (Phase 36.17.3, D-01).
//!
//! `MessageQueue<K>` is the cross-consumer trait that all per-session
//! FIFO queues (gateway `SessionQueue`, TUI app, future web/Discord/Slack/REST
//! consumers) implement. Surface is intentionally FIFO data ops only —
//! every consumer owns its own dispatch and drain orchestration per D-01.
//!
//! ## Surface
//! - `try_push` — bounded push (returns `QueueError::CapacityReached` at cap)
//! - `pop` — remove + return the oldest queued message text
//! - `len` — current depth for the key (0 if no queue allocated)
//! - `clear` — drop every queued message for the key (idempotent)
//!
//! `peek` is intentionally omitted (Resolution 3 / D-01). If a consumer needs
//! a non-destructive look at the head, it can layer that on its own concrete
//! type — the trait stays minimal.
//!
//! ## Key bound
//! `K: Hash + Eq + Clone + Send + Sync + 'static` — generic so each consumer
//! can pick its own keying scheme (gateway uses `SessionKey`, web/Discord/Slack
//! pick their own). Per D-01 (Resolution 3).
//!
//! ## Caps
//! `MAX_QUEUE_DEPTH = 128` matches the gateway constant — keeping the cap as
//! a `const` so it is locked at compile time (T-01 / T-36.17.3 mitigation).
//! `WARN_QUEUE_DEPTH = 96` is the 75% soft-warn threshold (D-11 from 36.17.1).

use std::hash::Hash;

/// Per-session hard cap. Once a session's queue reaches this depth, further
/// `try_push` calls return `Err(QueueError::CapacityReached)`.
///
/// Per-session (NOT global) so one attacker bucket is capped without
/// starving other sessions. Drop-newest + reject — the oldest queued event
/// is what the consumer is about to act on; dropping it would break causal
/// order. Primary DoS mitigation across every consumer of the trait
/// (T-36.17.3 / T-36.17.1-01).
pub const MAX_QUEUE_DEPTH: usize = 128;

/// Soft warning threshold — 75% of `MAX_QUEUE_DEPTH`. Concrete impls should
/// emit a `tracing::warn!` once a session's queue reaches this depth on push,
/// but the push still succeeds. Visible-but-not-yet-rejecting territory; gives
/// operators observability before the hard cap fires.
pub const WARN_QUEUE_DEPTH: usize = 96;

/// Errors returned by `MessageQueue` operations.
///
/// Currently only `CapacityReached` is defined; future phases may add variants
/// without breaking existing match sites (D-11/D-12 scope inherited from
/// Phase 36.17.1).
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum QueueError {
    /// The per-session queue is full. The message was NOT pushed. The caller
    /// is responsible for surfacing cap-hit UX (inline transcript error in the
    /// TUI, HTTP 429 in webhook adapters, etc.).
    #[error("Queue at capacity ({max}) for session {session_key}")]
    CapacityReached { session_key: String, max: usize },
}

/// Shared FIFO queue abstraction.
///
/// Implementors hold per-key FIFOs of `String` payloads. The `String` surface
/// (Resolution 5, D-02) keeps the trait usable by TUI/web/Discord/Slack/REST
/// consumers without forcing them to construct a gateway-specific
/// `MessageEvent`; the gateway's concrete `SessionQueue` implements this trait
/// via an internal adapter that wraps the `String` into a `MessageEvent`.
///
/// All methods are object-safe-friendly synchronous calls; trait objects
/// (`Arc<dyn MessageQueue<K>>`) are supported.
pub trait MessageQueue<K>: Send + Sync
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
{
    /// Push `message` onto the back of the per-key FIFO.
    ///
    /// Returns `Err(QueueError::CapacityReached)` if the queue is already at
    /// `MAX_QUEUE_DEPTH` for `key`. The message is NOT enqueued in that case;
    /// the caller is responsible for cap-hit UX.
    fn try_push(&self, key: &K, message: String) -> Result<(), QueueError>;

    /// Pop the oldest queued message for `key`, or `None` if the queue is
    /// empty (or was never allocated).
    fn pop(&self, key: &K) -> Option<String>;

    /// Current depth for `key`, or `0` if no queue is allocated yet.
    fn len(&self, key: &K) -> usize;

    /// Drop every queued message for `key`. No-op if the key has no queue.
    /// Idempotent.
    fn clear(&self, key: &K);
}
