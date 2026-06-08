//! Phase 36.17.4 regression tests — slash-command queue controls.
//!
//! Locks the D-04 / D-04a ordering invariants, the D-05 negative invariant
//! (/stop must NOT abort the in-flight JoinHandle on the web surface),
//! D-06 cap-hit message text, D-08 downstream `is_bypass` contract for
//! `pause` / `unpause`, and the pause/unpause `AtomicBool` state-machine.
//!
//! Source-text anchors via `include_str!` mirror the
//! `running_agent_guard_web_tests.rs:448-496` pattern. The state-machine
//! tests operate directly on the `Arc<AtomicBool>` primitive that
//! `AppState::get_or_create_paused_flag` returns, so no Axum spawn or real
//! WebSocket handshake is needed (D-10: highest-leverage seam).

use ironhermes_core::commands::running_agent::is_bypass;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ─────────────────────────────────────────────────────────────────────────────
// Source-text anchor — pin ws.rs to the tokens Plan 04 shipped.
// ─────────────────────────────────────────────────────────────────────────────

const WS_SOURCE: &str = include_str!("../src/server/ws.rs");

// ─────────────────────────────────────────────────────────────────────────────
// Helpers — paused-flag map free-fns mirror AppState::get_or_create_paused_flag.
// ─────────────────────────────────────────────────────────────────────────────

fn make_paused_map() -> Arc<Mutex<HashMap<String, Arc<AtomicBool>>>> {
    Arc::new(Mutex::new(HashMap::new()))
}

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
// D-08: downstream consumer contract for is_bypass.
// Mirrors Plan 01's is_bypass_locked_list inline test from a downstream
// crate's perspective — if Plan 01 regresses, this test fails loud.
// ─────────────────────────────────────────────────────────────────────────────

/// D-08: `is_bypass("pause")` and `is_bypass("unpause")` must hold so that
/// pause/unpause slash commands are dispatched even while a turn is in
/// flight. The pre-Plan-01 bypass list did NOT include these two — this
/// test failing is the canary that the extension regressed.
#[test]
fn is_bypass_includes_pause_and_unpause() {
    assert!(
        is_bypass("pause"),
        "D-08: `pause` must bypass the running-agent guard (Plan 01 extension)"
    );
    assert!(
        is_bypass("unpause"),
        "D-08: `unpause` must bypass the running-agent guard (Plan 01 extension)"
    );
    // The pre-existing bypass members must still hold — verify the
    // extension did not delete them by accident.
    assert!(is_bypass("stop"), "stop must still bypass");
    assert!(is_bypass("new"), "new must still bypass");
    assert!(is_bypass("status"), "status must still bypass");
    assert!(is_bypass("queue"), "queue must still bypass");
}

// ─────────────────────────────────────────────────────────────────────────────
// D-04: /new ordering — queue.clear MUST appear before reset_web_session
// within the NewSession arm body.
// ─────────────────────────────────────────────────────────────────────────────

/// D-04 ordering invariant: within the `CommandResult::NewSession` arm,
/// `queue.clear(&key)` must run BEFORE `reset_web_session(&session_id)` so a
/// `/new` slash command discards queued messages BEFORE the per-session
/// state is rotated.
///
/// Implementation note: scope the search to the slice of WS_SOURCE that
/// starts at `CommandResult::NewSession`. The `reset_web_session` token only
/// occurs inside the NewSession arm (it has no other call sites in ws.rs),
/// but we still slice to make the test resilient to future call-site adds.
#[test]
fn ws_rs_new_clears_queue_before_session_reset() {
    let arm_start = WS_SOURCE
        .find("CommandResult::NewSession")
        .expect("ws.rs must contain a `CommandResult::NewSession` arm");
    // Anchor an upper bound on the arm body — first occurrence of the next
    // `CommandResult::` after NewSession, or end of file if none found.
    let after = &WS_SOURCE[arm_start + "CommandResult::NewSession".len()..];
    let arm_end_rel = after.find("CommandResult::").unwrap_or(after.len());
    let arm_body =
        &WS_SOURCE[arm_start..arm_start + "CommandResult::NewSession".len() + arm_end_rel];

    let clear_pos = arm_body
        .find("queue.clear")
        .expect("D-04: NewSession arm must call `queue.clear`");
    let reset_pos = arm_body
        .find("reset_web_session")
        .expect("D-04: NewSession arm must call `reset_web_session`");

    assert!(
        clear_pos < reset_pos,
        "D-04 ordering: `queue.clear` (rel pos {clear_pos}) must appear BEFORE \
         `reset_web_session` (rel pos {reset_pos}) within the NewSession arm body"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-05: /stop must NOT abort the in-flight JoinHandle on the web surface.
// This is the documented divergence from TUI 36.17.3's D-08 (no
// CancellationToken on AppState yet) — the in-flight turn finishes naturally.
// ─────────────────────────────────────────────────────────────────────────────

/// D-05 negative invariant: within a 1500-byte window after the
/// `def.name == "stop"` intercept, neither `handle.abort` nor
/// `cancel_token.cancel` may appear. The /stop arm is clear-queue-only.
#[test]
fn ws_rs_stop_arm_no_abort_or_cancel() {
    let intercept = WS_SOURCE
        .find("def.name == \"stop\"")
        .expect("ws.rs must contain the `def.name == \"stop\"` intercept (Plan 04 Edit E)");
    let window_end = (intercept + 1500).min(WS_SOURCE.len());
    let window = &WS_SOURCE[intercept..window_end];

    assert!(
        !window.contains("handle.abort"),
        "D-05: /stop arm must NOT call `handle.abort` (clear-queue-only \
         divergence from TUI 36.17.3 D-08) — the in-flight JoinHandle \
         finishes naturally"
    );
    assert!(
        !window.contains("cancel_token.cancel"),
        "D-05: /stop arm must NOT call `cancel_token.cancel` (no \
         CancellationToken on AppState yet)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-04a: /stop ordering — queue.clear → paused.store(false) → QueueUpdated
//                       → "Queue cleared. Current turn finishing." → Finished.
// ─────────────────────────────────────────────────────────────────────────────

/// D-04a ordering invariant inside the /stop window.
///
/// `paused.store(false, ...)` is multi-line in the production source (the
/// `false,` literal sits on its own line after `.store(`); we anchor on
/// the `.store(` token, which always appears between `queue.clear` and the
/// `QueueUpdated` emission.
#[test]
fn ws_rs_stop_arm_ordering() {
    let intercept = WS_SOURCE
        .find("def.name == \"stop\"")
        .expect("ws.rs must contain the `def.name == \"stop\"` intercept");
    let window_end = (intercept + 1500).min(WS_SOURCE.len());
    let window = &WS_SOURCE[intercept..window_end];

    let p_clear = window
        .find("queue.clear")
        .expect("D-04a: /stop window must call `queue.clear`");
    let p_store = window
        .find(".store(")
        .expect("D-04a: /stop window must call `.store(` to clear the paused flag");
    let p_qu = window
        .find("QueueUpdated")
        .expect("D-04a: /stop window must emit `QueueUpdated`");
    let p_delta = window
        .find("Queue cleared. Current turn finishing.")
        .expect("D-04a: /stop window must emit the canonical Delta message");
    let p_finished = window
        .find("Finished {")
        .expect("D-04a: /stop window must emit a `Finished` event");

    assert!(
        p_clear < p_store,
        "D-04a: `queue.clear` ({p_clear}) must precede `.store(` ({p_store})"
    );
    assert!(
        p_store < p_qu,
        "D-04a: `.store(` ({p_store}) must precede `QueueUpdated` ({p_qu})"
    );
    assert!(
        p_qu < p_delta,
        "D-04a: `QueueUpdated` ({p_qu}) must precede the Delta confirmation ({p_delta})"
    );
    assert!(
        p_delta < p_finished,
        "D-04a: Delta ({p_delta}) must precede `Finished` ({p_finished})"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-06: cap-hit message text frozen.
// ─────────────────────────────────────────────────────────────────────────────

/// D-06: the in-flight push branch emits the exact format-string
/// `Queue is full ({max}/{max}). /stop or /flush to drain.` — this test
/// asserts the format string is the one ws.rs uses. The runtime-evaluated
/// `(128/128)` form is locked separately in `web_queue_protocol.rs`.
#[test]
fn ws_rs_cap_hit_message_locked() {
    assert!(
        WS_SOURCE.contains("Queue is full ({max}/{max}). /stop or /flush to drain."),
        "D-06: ws.rs must emit the canonical cap-hit Delta format string \
         `Queue is full ({{max}}/{{max}}). /stop or /flush to drain.`"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Plan 04 anchors: PauseQueue / UnpauseQueue arms plus the atomic primitives
// that implement their toggle / explicit-set semantics.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ws_rs_contains_pause_unpause_arms() {
    assert!(
        WS_SOURCE.contains("CommandResult::PauseQueue"),
        "ws.rs must contain the `CommandResult::PauseQueue` arm"
    );
    assert!(
        WS_SOURCE.contains("CommandResult::UnpauseQueue"),
        "ws.rs must contain the `CommandResult::UnpauseQueue` arm"
    );
    // The production source formats `paused_flag.fetch_xor(\n  true,\n  ...)`
    // across multiple lines (rustfmt), so we anchor on the method-call site
    // and then look at the next ~80 bytes for the `true` literal that proves
    // this is the toggle call (not some other fetch_xor).
    let xor_pos = WS_SOURCE
        .find("fetch_xor(")
        .expect("D-01a: /pause arm must call `paused_flag.fetch_xor(...)`");
    let xor_tail = &WS_SOURCE[xor_pos..(xor_pos + 80).min(WS_SOURCE.len())];
    assert!(
        xor_tail.contains("true"),
        "D-01a: `fetch_xor(...)` must be invoked with `true` (toggle to set/clear): \
         got `{xor_tail}`"
    );

    let swap_pos = WS_SOURCE
        .find("paused_flag.swap(")
        .expect("D-07: /unpause arm must call `paused_flag.swap(...)`");
    let swap_tail = &WS_SOURCE[swap_pos..(swap_pos + 80).min(WS_SOURCE.len())];
    assert!(
        swap_tail.contains("false"),
        "D-07: `paused_flag.swap(...)` must be invoked with `false` (explicit-set): \
         got `{swap_tail}`"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Pause/unpause state-machine — direct unit test on AtomicBool.
// Mirrors the runtime semantics Plan 04's ws.rs implements without needing
// to drive the actual ws recv loop.
// ─────────────────────────────────────────────────────────────────────────────

/// D-01a / D-07: pause toggle + explicit-set semantics on the paused-flag
/// `AtomicBool`. /pause flips false→true via `fetch_xor(true)` (returns the
/// OLD value, false); /unpause explicit-sets to false via `swap(false)`
/// (returns the OLD value, true for the was-paused branch). A second
/// /unpause on an already-unpaused session returns false (the
/// `Queue was not paused.` branch).
#[test]
fn pause_toggle_and_double_unpause() {
    let map = make_paused_map();
    let flag = get_or_create_paused(&map, "sess-1");

    // /pause: fetch_xor(true) flips false→true, returns old=false.
    let was = flag.fetch_xor(true, Ordering::SeqCst);
    assert!(
        !was,
        "D-01a: was_paused must be false before the first /pause"
    );
    assert!(
        flag.load(Ordering::SeqCst),
        "D-01a: flag must be true after fetch_xor(true)"
    );

    // /unpause: swap(false) returns the OLD value (true here).
    let was2 = flag.swap(false, Ordering::SeqCst);
    assert!(
        was2,
        "D-07: first /unpause must return was=true (was paused before unpause)"
    );
    assert!(
        !flag.load(Ordering::SeqCst),
        "D-07: flag must be false after swap(false)"
    );

    // Double-unpause: swap(false) on an already-false flag returns false —
    // ws.rs branches into the `Queue was not paused.` informational path.
    let was3 = flag.swap(false, Ordering::SeqCst);
    assert!(
        !was3,
        "D-07: second /unpause must return was=false \
         (the `Queue was not paused.` branch)"
    );
}
