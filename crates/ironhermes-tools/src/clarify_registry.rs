// Phase 36.3.8 Plan 02 — PendingClarifyRegistry: in-memory awaiter map for clarify turns.
//
// MUST use tokio::sync::Mutex (not the std-lib Mutex) — Pitfall 2: holding a
// blocking Mutex across an async boundary causes a clippy error and potential
// deadlock. With tokio::sync::Mutex the guard's lifetime ends at the end of the
// lock statement before any await, keeping the critical section synchronous.
//
// The registry stores BOTH the choices (Vec<String>) alongside the oneshot Sender.
// This is what lets the gateway (Plan 03) recover the human-readable label by
// index — callback_data stays compact ("clarify:<clarify_id>:<index>") and only
// carries the index; the label is recovered from choices[index] here.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

// ---------------------------------------------------------------------------
// ClarifyAnswer — the value delivered through the oneshot channel
// ---------------------------------------------------------------------------

/// The answer delivered from the gateway (or CLI fallback) back to a suspended
/// `ClarifyTool::execute_clarify()` call.
///
/// `label` is the human-readable choice string; `index` is its 0-based position
/// in the `choices` slice that was presented to the user.
#[derive(Debug, Clone)]
pub struct ClarifyAnswer {
    /// The text label of the chosen option (e.g. "Yes").
    pub label: String,
    /// 0-based index into the choices slice.
    pub index: usize,
}

// ---------------------------------------------------------------------------
// PendingClarify — a single in-flight registry entry
// ---------------------------------------------------------------------------

/// One pending clarify awaiter: the original choices (for label recovery) plus
/// the oneshot Sender that unblocks `ClarifyTool::execute_clarify()`.
///
/// Stored in `PendingClarifyRegistry` while a turn is suspended waiting for
/// a human response. Removed on every exit path — answered, timeout, cancel,
/// and error — to prevent resource leaks (Pitfall 5).
pub struct PendingClarify {
    /// The choices that were presented to the user, in order.
    /// The gateway uses `choices[index]` to recover the label string without
    /// bloating callback_data beyond `clarify:<clarify_id>:<index>`.
    pub choices: Vec<String>,
    /// Oneshot sender; `tx.send(answer)` unblocks the suspended turn.
    pub sender: oneshot::Sender<ClarifyAnswer>,
}

// ---------------------------------------------------------------------------
// PendingClarifyRegistry
// ---------------------------------------------------------------------------

/// Thread-safe in-memory map of pending clarify awaiters, keyed by `clarify_id`.
///
/// `clarify_id` is constructed by `ClarifyTool` to encode the session's
/// `chat_id` so the gateway dispatcher (Plan 03) can route an inbound
/// `callback_query` back to the correct suspended turn.
///
/// # Concurrency contract
///
/// Uses `tokio::sync::Mutex` (not the stdlib blocking mutex) so that clippy never
/// fires "mutex lock held across await point." The lock is always dropped at
/// the end of a synchronous statement — the caller uses the entry (or sends
/// on the oneshot) **after** the lock has been released.
///
/// # Clone semantics
///
/// `Clone` is derived via the inner `Arc` — cloning produces a second handle
/// to the **same** map, matching the `Arc<Mutex<HashMap<...>>>` pattern used
/// by `skill_overlays` / `nudge_turns` in `ironhermes-gateway/src/handler.rs`.
#[derive(Clone)]
pub struct PendingClarifyRegistry {
    /// Arc so the registry can be cloned into multiple owners (ClarifyTool +
    /// gateway ClarifyDispatcher impl) without copying the underlying data.
    pending: Arc<Mutex<HashMap<String, PendingClarify>>>,
}

impl PendingClarifyRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Insert a new pending clarify entry.
    ///
    /// `choices` must match the options that were (or will be) sent to the user
    /// so that the gateway can recover `choices[index]` from the compact
    /// `callback_data` without an extra round-trip.
    ///
    /// The insert happens **before** the question is sent to the platform to
    /// eliminate the race where a very fast human response arrives before the
    /// entry is registered.
    ///
    /// # Concurrent clarify guard (WR-02)
    ///
    /// Only one clarify can be active per `clarify_id` (i.e. per chat) at a
    /// time in v1. If an entry already exists for the key this method returns
    /// `Err` rather than silently overwriting the first awaiter. The first
    /// suspended `execute_clarify` is left intact and the second clarify call
    /// surfaces a clear error to the model instead of producing a misleading
    /// `"reason":"timeout"` for the evicted turn.
    pub async fn insert(
        &self,
        clarify_id: String,
        choices: Vec<String>,
        sender: oneshot::Sender<ClarifyAnswer>,
    ) -> anyhow::Result<()> {
        // Lock, check, insert, drop guard — no await inside the lock.
        let mut map = self.pending.lock().await;
        if map.contains_key(&clarify_id) {
            anyhow::bail!(
                "clarify: a question is already pending for this chat \
                 (clarify_id={clarify_id}); only one active clarify per chat \
                 is supported in v1 — wait for the current question to be \
                 answered or time out before issuing another"
            );
        }
        map.insert(clarify_id, PendingClarify { choices, sender });
        Ok(())
    }

    /// Remove and return the entry for `clarify_id`, if present.
    ///
    /// # Lock contract
    ///
    /// The lock is acquired, the entry is removed, and the lock is **released**
    /// before this method returns. The caller MUST use `entry.sender` (and
    /// `.await` any sends) **after** this call returns — never inside a lock
    /// guard. This prevents Pitfall 2 (lock held across await).
    pub async fn take(&self, clarify_id: &str) -> Option<PendingClarify> {
        // Lock, remove, return — guard drops at end of this statement.
        self.pending.lock().await.remove(clarify_id)
    }

    /// Remove and discard the entry for `clarify_id` (cleanup on exit paths
    /// that don't need the sender, e.g. timeout or cancel — Pitfall 5).
    pub async fn remove(&self, clarify_id: &str) {
        self.pending.lock().await.remove(clarify_id);
    }
}

impl Default for PendingClarifyRegistry {
    fn default() -> Self {
        Self::new()
    }
}
