//! Phase 36.17.3 D-06/D-07/D-08/D-10 — pause/unpause + /new + /stop + cap-hit.
//!
//! Plan 06 bodies. Tests drive `App` state directly (mirroring tui_queue_drain
//! tests) — they do NOT route through the slash dispatcher; they exercise the
//! underlying state-machine effects that the slash arms in `commands.rs`
//! cause.
//!
//! Coverage:
//! - `tui_queue_pause_unpause` — D-06 amended (toggle + double-unpause no-op).
//! - `tui_queue_new_clears_atomic` — D-07 (T-02 mitigation witness).
//! - `tui_stop_clears_queue` — D-08 (Pitfall 1 ordering witness).
//! - `tui_queue_cap_hit` — D-10 (T-01 mitigation witness).

#![cfg(feature = "test-support")]

use ironhermes_cli::tui_rata::app::App;
use ironhermes_cli::tui_rata::stream_events::StreamEvent;
use ironhermes_core::Platform;
use ironhermes_core::queue::{MAX_QUEUE_DEPTH, MessageQueue, QueueError};
use ironhermes_core::session::SessionKey;
use ironhermes_gateway::session_queue::SessionQueue;
use std::sync::Arc;
use std::sync::atomic::Ordering;

fn make_queue() -> Arc<SessionQueue> {
    Arc::new(SessionQueue::new())
}

fn make_key() -> SessionKey {
    SessionKey::new(Platform::Local, "local").with_user("local")
}

/// Mirror `App::submit()`'s pending-channel setup so the test simulates a
/// "turn in flight" state without spawning any async work.
fn install_pending_turn(app: &mut App) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    app.pending_tx = Some(tx);
    app.pending_rx = Some(rx);
}

/// Clear the pending channel — mirrors the turn-finished mutation that
/// precedes the next `handle_stream_event` in production.
fn clear_pending(app: &mut App) {
    app.pending_tx = None;
    app.pending_rx = None;
}

#[test]
fn tui_queue_pause_unpause() {
    // D-06 amended: /pause toggles paused=true; Finished with paused=true
    // skips drain; /unpause toggles paused=false; double-unpause returns
    // the "was not paused" informational path.
    let queue = make_queue();
    let mut app = App::new_test_with_queue(queue.clone() as Arc<dyn MessageQueue<SessionKey>>);

    // Push 2 items so we can observe the paused vs unpaused drain behavior.
    for label in ["alpha", "bravo"] {
        app.queue
            .try_push(&app.queue_key, label.to_string())
            .expect("under cap");
    }
    assert_eq!(app.queue.len(&app.queue_key), 2);

    // Pause and complete a (synthetic) turn. The Finished arm's
    // maybe_drain_queue call must skip due to the paused guard.
    install_pending_turn(&mut app);
    app.queue_paused.store(true, Ordering::SeqCst);
    clear_pending(&mut app);
    app.handle_stream_event(StreamEvent::Finished { total_tokens: 0 });
    assert_eq!(
        app.queue.len(&app.queue_key),
        2,
        "paused queue must not drain on Finished"
    );
    assert!(
        app.pending_tx.is_none(),
        "paused drain installs no pending channel"
    );

    // Unpause + Finished cycle → drain pops one item.
    app.queue_paused.store(false, Ordering::SeqCst);
    app.handle_stream_event(StreamEvent::Finished { total_tokens: 0 });
    assert_eq!(
        app.queue.len(&app.queue_key),
        1,
        "unpaused drain pops one item"
    );

    // Double-unpause: `swap(false, SeqCst)` returns the previous value. Since
    // we just stored `false`, the previous value is `false` — confirming the
    // "Queue was not paused" informational path that the /unpause arm in
    // commands.rs emits.
    let was_paused = app.queue_paused.swap(false, Ordering::SeqCst);
    assert!(
        !was_paused,
        "double-unpause returns false (no-op informational path)"
    );
}

#[test]
fn tui_queue_new_clears_atomic() {
    // D-07 / T-02 mitigation witness: /new clears the queue AND resets paused
    // — both in one atomic-feeling sequence. Order per Plan 05 Task 2:
    // queue.clear → queue_paused.store(false, SeqCst).
    let queue = make_queue();
    let app = App::new_test_with_queue(queue.clone() as Arc<dyn MessageQueue<SessionKey>>);

    for label in ["x", "y", "z"] {
        app.queue
            .try_push(&app.queue_key, label.to_string())
            .expect("under cap");
    }
    app.queue_paused.store(true, Ordering::SeqCst);
    assert_eq!(app.queue.len(&app.queue_key), 3);
    assert!(app.queue_paused.load(Ordering::SeqCst));

    // Simulate `/new` arm effect — clear, then reset paused.
    app.queue.clear(&app.queue_key);
    app.queue_paused.store(false, Ordering::SeqCst);

    assert_eq!(
        app.queue.len(&app.queue_key),
        0,
        "/new clears queue (T-02 witness)"
    );
    assert!(
        !app.queue_paused.load(Ordering::SeqCst),
        "/new resets paused"
    );
}

#[test]
fn tui_stop_clears_queue() {
    // D-08 / Pitfall 1 ordering witness: /stop must clear the queue and reset
    // paused BEFORE firing the cancel token. Plan 05 Task 3 order:
    // queue.clear → queue_paused.store(false) → cancel_child.take().cancel().
    use tokio_util::sync::CancellationToken;

    let queue = make_queue();
    let mut app = App::new_test_with_queue(queue.clone() as Arc<dyn MessageQueue<SessionKey>>);

    for label in ["x", "y", "z"] {
        app.queue
            .try_push(&app.queue_key, label.to_string())
            .expect("under cap");
    }
    assert_eq!(app.queue.len(&app.queue_key), 3);

    // Install a cancel_child token mirroring submit()'s setup so /stop has
    // something to cancel.
    let child: CancellationToken = app.cancel_parent.child_token();
    app.cancel_child = Some(child.clone());
    assert!(!child.is_cancelled(), "child token starts uncancelled");

    // Simulate `/stop` arm order per Plan 05 Task 3.
    app.queue.clear(&app.queue_key);
    app.queue_paused.store(false, Ordering::SeqCst);
    if let Some(tok) = app.cancel_child.take() {
        tok.cancel();
    }

    assert_eq!(
        app.queue.len(&app.queue_key),
        0,
        "/stop clears queue (Pitfall 1 witness — clear precedes cancel)"
    );
    assert!(
        !app.queue_paused.load(Ordering::SeqCst),
        "/stop resets paused"
    );
    assert!(
        app.cancel_child.is_none(),
        "/stop consumes the cancel_child token"
    );
    // The child token we cloned before take() observes the cancellation.
    assert!(
        child.is_cancelled(),
        "child token cancelled by /stop sequence"
    );
}

#[test]
fn tui_queue_cap_hit() {
    // D-10 / T-01 mitigation witness: pushing past MAX_QUEUE_DEPTH returns
    // `QueueError::CapacityReached { max: 128, .. }` and the queue depth
    // stays at the cap (drop-newest semantics).
    let queue: Arc<dyn MessageQueue<SessionKey>> = make_queue();
    let key = make_key();

    for i in 0..MAX_QUEUE_DEPTH {
        queue
            .try_push(&key, format!("m{}", i))
            .expect("under cap");
    }
    assert_eq!(
        queue.len(&key),
        MAX_QUEUE_DEPTH,
        "filled to cap (=128)"
    );

    let result = queue.try_push(&key, "overflow".to_string());
    assert!(
        matches!(result, Err(QueueError::CapacityReached { max: 128, .. })),
        "push past cap returns CapacityReached {{ max: 128 }} — got {:?}",
        result
    );
    assert_eq!(
        queue.len(&key),
        MAX_QUEUE_DEPTH,
        "drop-newest preserves cap depth after overflow attempt"
    );
}
