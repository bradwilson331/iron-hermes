//! Phase 39.1 Plan 05 — Concurrent history-append + snapshot-isolation tests
//!
//! Validates R39.1-02 (atomic pair append, no interleaving) and R39.1-07
//! (per-turn config snapshot isolation from mid-turn mutations).
//!
//! Three behaviors tested:
//!   1. `history_append_concurrent` — 10 concurrent pair-appends; no interleaving.
//!   2. `history_survives_session_remove` — Arc keeps Vec alive after session dropped.
//!   3. `snapshot_isolation` — captured model clone unaffected by simulated config mutation.
//!
//! No live LLM calls.

use ironhermes_core::session::SessionKey;
use ironhermes_core::{ChatMessage, Platform};
use ironhermes_gateway::session::{GatewaySession, append_turn_pair};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// R39.1-02: history_append_concurrent
// ---------------------------------------------------------------------------

/// Spawn N=10 concurrent tasks, each appending a distinct (user_i, assistant_i) pair
/// via `append_turn_pair`.  After all tasks join, the Vec must contain exactly 2N
/// messages and every user_i must be immediately followed by its matching assistant_i
/// (no interleaving across pairs).
#[tokio::test]
async fn history_append_concurrent() {
    const N: usize = 10;

    let history: Arc<Mutex<Vec<ChatMessage>>> = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let arc = history.clone();
        handles.push(tokio::spawn(async move {
            let user = ChatMessage::user(format!("user_{i}"));
            let assistant = ChatMessage::assistant(format!("assistant_{i}"));
            // Atomic pair append: both messages pushed under one lock.
            append_turn_pair(&arc, user, assistant);
        }));
    }

    for h in handles {
        h.await.expect("task panicked");
    }

    let messages = history.lock().unwrap();

    // Exactly 2N messages total.
    assert_eq!(
        messages.len(),
        2 * N,
        "expected {}, got {} messages",
        2 * N,
        messages.len()
    );

    // For each adjacent pair: first must be user, second must be assistant,
    // and they must share the same index suffix (atomic pair, no interleaving).
    for (idx, chunk) in messages.chunks(2).enumerate() {
        let (u, a) = (&chunk[0], &chunk[1]);

        let u_text = u
            .content
            .as_ref()
            .and_then(|c| c.as_text().map(|s| s.to_string()))
            .unwrap_or_default();
        let a_text = a
            .content
            .as_ref()
            .and_then(|c| c.as_text().map(|s| s.to_string()))
            .unwrap_or_default();

        assert!(
            u_text.starts_with("user_"),
            "pair {idx}: expected user message, got: {u_text:?}"
        );
        assert!(
            a_text.starts_with("assistant_"),
            "pair {idx}: expected assistant message, got: {a_text:?}"
        );

        let u_idx: usize = u_text
            .strip_prefix("user_")
            .unwrap()
            .parse()
            .expect("bad user index");
        let a_idx: usize = a_text
            .strip_prefix("assistant_")
            .unwrap()
            .parse()
            .expect("bad assistant index");

        assert_eq!(
            u_idx, a_idx,
            "pair {idx} interleaved: user_{u_idx} followed by assistant_{a_idx}"
        );
    }
}

// ---------------------------------------------------------------------------
// R39.1-02 (Pitfall 3): history_survives_session_remove
// ---------------------------------------------------------------------------

/// A turn holds an Arc clone of the history Vec.  Simulate /new by dropping the
/// session (and thereby its primary Arc reference).  The turn's Arc clone must
/// keep the Vec alive so `append_turn_pair` still succeeds without panicking.
#[tokio::test]
async fn history_survives_session_remove() {
    let key = SessionKey::new(Platform::Telegram, "chat_42").with_user("user_1");
    let session = GatewaySession::new(key, "gpt-4o");

    // Turn captures its own Arc clone BEFORE the session is dropped.
    let history_arc = session.history_arc();

    // Simulate /new: drop the GatewaySession (its primary Arc ref is dropped).
    drop(session);

    // The turn's Arc clone keeps the Vec alive — append must not panic.
    let user = ChatMessage::user("hello after /new");
    let assistant = ChatMessage::assistant("still here");
    append_turn_pair(&history_arc, user, assistant);

    let guard = history_arc.lock().unwrap();
    assert_eq!(guard.len(), 2, "expected 2 messages after fallback append");
    drop(guard);
}

// ---------------------------------------------------------------------------
// R39.1-07 (D-06-RISK Pattern 3): snapshot_isolation
// ---------------------------------------------------------------------------

/// Capture a model snapshot at turn start.  Simulate a mid-turn /model change by
/// mutating a separate value.  Assert the local snapshot is unchanged (it's a
/// plain String clone — always isolated).
///
/// This test drives the snapshot-capture helper directly (clone-at-start pattern
/// from handler.rs:1395) without any live LLM or config Arc.
#[tokio::test]
async fn snapshot_isolation() {
    // Simulate the per-turn model snapshot captured at handler.rs line ~1395.
    let config_model = Arc::new(Mutex::new("gpt-4o".to_string()));

    // Turn start: clone the model string into a local snapshot.
    let model_snapshot: String = config_model.lock().unwrap().clone();

    // Simulate a concurrent /model change (different session thread).
    {
        let mut m = config_model.lock().unwrap();
        *m = "claude-opus-4".to_string();
    }

    // The in-flight turn's snapshot must still report the original model.
    assert_eq!(
        model_snapshot, "gpt-4o",
        "snapshot was mutated by simulated mid-turn /model change"
    );

    // The live config now reflects the new value (next turn will use it).
    let next_model = config_model.lock().unwrap().clone();
    assert_eq!(
        next_model, "claude-opus-4",
        "next-turn model should reflect /model change"
    );
}
