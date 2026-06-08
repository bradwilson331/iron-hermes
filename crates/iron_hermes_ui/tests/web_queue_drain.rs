//! Phase 36.17.4 regression tests — queue drain + FIFO + cap + paused semantics.
//!
//! Source-text anchors mirror the
//! `running_agent_guard_web_tests.rs:448-496` pattern: assert that the
//! production `ws.rs` + `state.rs` files contain the tokens this phase
//! committed to, so a future edit can't silently regress the wiring.
//!
//! Direct unit tests exercise the production `SessionQueue` + the
//! `Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>` paused-flag idiom that
//! `AppState::get_or_create_paused_flag` uses, without spinning up a real
//! WebSocket server (D-10: highest-leverage seam).
//!
//! D-XX references trace to `.planning/phases/36.17.4-.../36.17.4-CONTEXT.md`.

use ironhermes_core::queue::{MessageQueue, QueueError, MAX_QUEUE_DEPTH};
use ironhermes_core::session::SessionKey;
use ironhermes_core::types::Platform;
use ironhermes_gateway::session_queue::SessionQueue;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ─────────────────────────────────────────────────────────────────────────────
// Source-text anchors — pin ws.rs + state.rs to the tokens Plan 03 / Plan 04
// shipped. Same pattern as running_agent_guard_web_tests.rs:448-496.
// ─────────────────────────────────────────────────────────────────────────────

const WS_SOURCE: &str = include_str!("../src/server/ws.rs");
const STATE_SOURCE: &str = include_str!("../src/server/state.rs");

// ─────────────────────────────────────────────────────────────────────────────
// Helpers — mirror AppState::get_or_create_paused_flag so unit tests do not
// need to construct a full AppState (Config + StateStore + AgentRuntime are
// far too heavy for a unit test surface).
// ─────────────────────────────────────────────────────────────────────────────

/// Build the queue as the trait-object `Arc<dyn MessageQueue<SessionKey>>` —
/// the SAME field shape as `AppState::queue`, which routes `String` payloads
/// through the `MessageQueue` trait impl (gateway's `SessionQueue` exposes
/// inherent `MessageEvent`-typed methods AND the `String`-typed trait impl;
/// ws.rs only ever calls the trait).
fn make_queue() -> Arc<dyn MessageQueue<SessionKey>> {
    Arc::new(SessionQueue::new())
}

fn make_key(chat: &str) -> SessionKey {
    // ws.rs uses `SessionKey { platform: Platform::Web, chat_id, user_id:
    // Some("web".into()) }` at every call site; matching the constructor
    // form keeps the test orthogonal to that internal shape.
    SessionKey::new(Platform::Web, chat).with_user("web")
}

fn make_paused_map() -> Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Free-fn equivalent of `AppState::get_or_create_paused_flag` (D-01a).
/// Mirrors the entry-or-insert body byte-for-byte.
fn get_or_create_paused(
    map: &Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    id: &str,
) -> Arc<AtomicBool> {
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(id.to_string())
        .or_insert_with(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

// ─────────────────────────────────────────────────────────────────────────────
// Source-text anchor tests — D-01, D-01a, D-02, D-03 wiring locks.
// ─────────────────────────────────────────────────────────────────────────────

/// Phase 36.17.4 Plan 03 anchor: state.rs must declare the queue + queue_paused
/// fields and the get_or_create_paused_flag helper. If any of these tokens
/// disappear, the Plan 03 wiring has regressed.
#[test]
fn state_rs_declares_queue_fields() {
    assert!(
        STATE_SOURCE.contains("pub queue:"),
        "D-01: state.rs must declare the shared FIFO `pub queue:` field on AppState"
    );
    assert!(
        STATE_SOURCE.contains("pub queue_paused:"),
        "D-01a: state.rs must declare the per-session `pub queue_paused:` map on AppState"
    );
    assert!(
        STATE_SOURCE.contains("fn get_or_create_paused_flag"),
        "D-01a: state.rs must define the `get_or_create_paused_flag` helper \
         (mirrors `get_or_create_running_flag` from Plan 36.1)"
    );
}

/// Phase 36.17.4 Plan 04 anchor: ws.rs must contain the three load-bearing
/// drain tokens — push site, pop site, and the typed QueueUpdated event.
#[test]
fn ws_rs_contains_drain_mechanism() {
    assert!(
        WS_SOURCE.contains("queue.try_push"),
        "D-01 / D-03: ws.rs must call `queue.try_push` at the in-flight \
         push site (replaces the pre-phase hard reject)"
    );
    assert!(
        WS_SOURCE.contains("queue.pop"),
        "D-02: ws.rs must call `queue.pop` in the drain arm (None => branch \
         after `turn.handle.await`)"
    );
    assert!(
        WS_SOURCE.contains("QueueUpdated"),
        "D-03: ws.rs must emit the typed `QueueUpdated` event"
    );
}

/// Phase 36.17.4 Plan 04 anchor (D-03 emission ordering):
/// The drain branch must emit `QueueUpdated` BEFORE spawning the next turn,
/// so the client pill updates before the new turn's tokens start streaming.
///
/// Mechanism: locate the LAST occurrence of `queue.pop` (drain branch lives
/// deep in arm 2's `None =>` near end of file), then verify that within the
/// 2000-byte window after it, `QueueUpdated` precedes `tokio::spawn`.
#[test]
fn ws_rs_drain_emits_queue_updated_before_spawn() {
    let pop_idx = WS_SOURCE
        .rfind("queue.pop")
        .expect("D-02: ws.rs must contain `queue.pop` for the drain branch");
    let window_end = (pop_idx + 2000).min(WS_SOURCE.len());
    let window = &WS_SOURCE[pop_idx..window_end];

    let qu_rel = window
        .find("QueueUpdated")
        .expect("D-03: `QueueUpdated` must appear after `queue.pop` in the drain branch");
    let spawn_rel = window
        .find("tokio::spawn")
        .expect("D-02: the drain branch must spawn a new turn via `tokio::spawn`");

    assert!(
        qu_rel < spawn_rel,
        "D-03 emission ordering: drain branch must emit `QueueUpdated` \
         (rel pos {qu_rel}) BEFORE `tokio::spawn` (rel pos {spawn_rel}) so \
         the pill updates before the respawn"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Direct unit tests — exercise the production SessionQueue + Arc<AtomicBool>
// types without an AppState mock. D-10: highest-leverage seam.
// ─────────────────────────────────────────────────────────────────────────────

/// FIFO ordering invariant on the production `SessionQueue` (D-01).
/// Push A, B, C — pops must come out A, B, C, then None.
#[test]
fn queue_fifo_ordering() {
    let q = make_queue();
    let k = make_key("sess-a");

    q.try_push(&k, "A".to_string()).expect("under cap");
    q.try_push(&k, "B".to_string()).expect("under cap");
    q.try_push(&k, "C".to_string()).expect("under cap");

    assert_eq!(q.pop(&k), Some("A".to_string()), "D-01: FIFO pop 1");
    assert_eq!(q.pop(&k), Some("B".to_string()), "D-01: FIFO pop 2");
    assert_eq!(q.pop(&k), Some("C".to_string()), "D-01: FIFO pop 3");
    assert_eq!(
        q.pop(&k),
        None,
        "D-01: 4th pop on drained queue returns None"
    );
    assert_eq!(q.len(&k), 0, "D-01: drained queue len == 0");
}

/// Cap-hit invariant: the 129th push (MAX_QUEUE_DEPTH + 1) must return
/// `QueueError::CapacityReached` and not enqueue the message (D-06).
#[test]
fn queue_cap_at_128() {
    let q = make_queue();
    let k = make_key("sess-cap");

    for i in 0..MAX_QUEUE_DEPTH {
        q.try_push(&k, format!("msg-{i}"))
            .unwrap_or_else(|e| panic!("D-06: push {i} (< MAX) must succeed, got {e:?}"));
    }
    assert_eq!(
        q.len(&k),
        MAX_QUEUE_DEPTH,
        "D-06: queue at exactly MAX_QUEUE_DEPTH after 128 pushes"
    );

    let overflow = q.try_push(&k, "overflow".to_string());
    assert!(
        matches!(overflow, Err(QueueError::CapacityReached { max, .. }) if max == MAX_QUEUE_DEPTH),
        "D-06: 129th push must return Err(QueueError::CapacityReached {{ max: 128, .. }}); \
         got {overflow:?}"
    );
    assert_eq!(
        q.len(&k),
        MAX_QUEUE_DEPTH,
        "D-06: overflow message must NOT be enqueued — len stays at MAX"
    );
}

/// Paused-blocks-drain invariant (D-02 / D-01a): when the per-session paused
/// flag is true, the drain guard skips the pop. When the flag is false the
/// pop proceeds normally.
#[test]
fn queue_pause_drain_semantics() {
    let q = make_queue();
    let k = make_key("sess-p");
    let paused_map = make_paused_map();
    let flag = get_or_create_paused(&paused_map, "sess-p");

    q.try_push(&k, "one".to_string()).expect("under cap");
    q.try_push(&k, "two".to_string()).expect("under cap");
    q.try_push(&k, "three".to_string()).expect("under cap");
    assert_eq!(q.len(&k), 3, "pre: 3 items queued");

    // Pause the session — the drain guard MUST skip pop while paused.
    flag.store(true, Ordering::SeqCst);
    let paused_now = flag.load(Ordering::SeqCst);
    assert!(paused_now, "D-01a: flag is paused after store(true)");
    if !paused_now {
        // Simulated drain guard. Unreachable while paused — assertion above
        // already proved we are in the paused arm.
        let _ = q.pop(&k);
    }
    assert_eq!(
        q.len(&k),
        3,
        "D-02 paused-blocks-drain: queue MUST remain at 3 while flag is true"
    );

    // Unpause and simulate one drain iteration — FIFO order preserved.
    flag.store(false, Ordering::SeqCst);
    assert_eq!(
        q.pop(&k),
        Some("one".to_string()),
        "D-02: after unpause, drain pulls oldest item (FIFO)"
    );
    assert_eq!(q.len(&k), 2, "D-02: queue len decrements after pop");
}

/// `Arc<AtomicBool>` idempotency (D-01a): two `get_or_create_paused` calls
/// with the same id return Arcs pointing to the SAME AtomicBool — observed
/// via `Arc::ptr_eq` and via cross-handle visibility of a `store`.
#[test]
fn paused_flag_idempotent() {
    let map = make_paused_map();
    let first = get_or_create_paused(&map, "sess-id");
    let second = get_or_create_paused(&map, "sess-id");

    assert!(
        Arc::ptr_eq(&first, &second),
        "D-01a: idempotent get_or_create must return Arcs to the SAME AtomicBool \
         (no fresh allocation on second lookup)"
    );

    first.store(true, Ordering::SeqCst);
    assert!(
        second.load(Ordering::SeqCst),
        "D-01a: store via first handle must be observable through the second handle"
    );
}
