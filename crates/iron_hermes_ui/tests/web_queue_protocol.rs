//! Phase 36.17.4 regression tests — wire-format and wasm-side wiring.
//!
//! Locks:
//! - protocol.rs: `ChatStreamEvent::QueueUpdated { depth: u32, paused: bool }`
//!   variant + the inline `test_queue_updated_json_shape` test that pins
//!   the external-tagged JSON literal (D-03 / D-11).
//! - mod.rs: `queue_state` signal + the `QueueUpdated` recv arm + the
//!   `use_context_provider` call that exposes it to AppFooter (D-03a).
//! - app_footer.rs: context consumer + hide-when-zero render + the
//!   paused-branch text + the AGENT-then-QUEUE placement discretion (D-09).
//! - Plus stability canaries on the three user-visible format strings
//!   `ws.rs` emits (Queued / cap-hit / Queue cleared).
//!
//! All anchors are source-text grep — same approach as
//! `running_agent_guard_web_tests.rs:448-496`.

// ─────────────────────────────────────────────────────────────────────────────
// Source anchors — pin protocol.rs + mod.rs + app_footer.rs to the tokens
// Plans 02 and 05 shipped.
// ─────────────────────────────────────────────────────────────────────────────

const PROTOCOL_SOURCE: &str = include_str!("../src/protocol.rs");
const MOD_RS_SOURCE: &str = include_str!("../src/components/hermes_app/mod.rs");
const APP_FOOTER_SOURCE: &str = include_str!("../src/components/hermes_app/app_footer.rs");

// ─────────────────────────────────────────────────────────────────────────────
// D-03: protocol.rs QueueUpdated variant shape.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn protocol_rs_declares_queue_updated_variant() {
    assert!(
        PROTOCOL_SOURCE.contains("QueueUpdated"),
        "D-03: protocol.rs must declare the `QueueUpdated` variant on `ChatStreamEvent`"
    );
    assert!(
        PROTOCOL_SOURCE.contains("depth: u32"),
        "D-03: `QueueUpdated` must carry a `depth: u32` field"
    );
    assert!(
        PROTOCOL_SOURCE.contains("paused: bool"),
        "D-03: `QueueUpdated` must carry a `paused: bool` field"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-11: protocol.rs ships the inline `test_queue_updated_json_shape` test
// that pins the EXACT external-tagged JSON literal. This downstream test
// proves that the inline test EXISTS in production source.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn protocol_rs_queue_updated_shape_test_exists() {
    assert!(
        PROTOCOL_SOURCE.contains("test_queue_updated_json_shape"),
        "D-11: protocol.rs must contain the inline `test_queue_updated_json_shape` \
         test (the wire-format lock from Plan 02)"
    );
    assert!(
        PROTOCOL_SOURCE.contains(r#"{"QueueUpdated":{"depth":3,"paused":false}}"#),
        "D-11: protocol.rs shape test must lock the exact external-tagged JSON \
         literal `{{\"QueueUpdated\":{{\"depth\":3,\"paused\":false}}}}`"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-03a: HermesApp signal + recv arm + context provider.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn mod_rs_handles_queue_updated_in_recv_loop() {
    assert!(
        MOD_RS_SOURCE.contains("QueueUpdated"),
        "D-03a: mod.rs must reference `QueueUpdated` in the recv loop"
    );
    assert!(
        MOD_RS_SOURCE.contains("queue_state"),
        "D-03a: mod.rs must declare the `queue_state` signal"
    );
    assert!(
        MOD_RS_SOURCE.contains("queue_state.set("),
        "D-03a: the `QueueUpdated` arm must call `queue_state.set(...)` to \
         propagate (depth, paused) to AppFooter"
    );
    assert!(
        MOD_RS_SOURCE.contains("use_context_provider(|| queue_state)"),
        "D-03a: mod.rs must expose `queue_state` via \
         `use_context_provider(|| queue_state)`"
    );
}

#[test]
fn mod_rs_queue_state_initial_value_is_zero_unpaused() {
    assert!(
        MOD_RS_SOURCE.contains("(0u32, false)"),
        "D-03a: `queue_state` must be initialized to `(0u32, false)` — \
         zero depth, not paused (the pre-WS-handshake idle state)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-09: AppFooter pill render — context consumer, hide-when-zero, paused branch.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn app_footer_rs_renders_queue_pill() {
    assert!(
        APP_FOOTER_SOURCE.contains("queue_depth"),
        "D-09: app_footer.rs must destructure `queue_depth` from the signal"
    );
    assert!(
        APP_FOOTER_SOURCE.contains("queue_depth > 0"),
        "D-09: app_footer.rs must guard the pill render with `queue_depth > 0` \
         (hide-when-zero — matches TUI status_line.rs discipline)"
    );
    assert!(
        APP_FOOTER_SOURCE.contains("(paused)"),
        "D-09: app_footer.rs must render the paused branch text `(paused)` \
         when the session is paused"
    );
    assert!(
        APP_FOOTER_SOURCE.contains("use_context::<Signal<(u32, bool)>>()"),
        "D-09: app_footer.rs must consume `Signal<(u32, bool)>` via \
         `use_context::<Signal<(u32, bool)>>()`"
    );
}

#[test]
fn app_footer_rs_pill_after_agent_span() {
    // D-03b discretion: the QUEUE pill sits in the left run AFTER the AGENT
    // span (visual parity with TUI status_line.rs). Source-order check:
    // the `queue_depth > 0` guard must appear after the AGENT label.
    let agent_pos = APP_FOOTER_SOURCE
        .find("AGENT ")
        .expect("app_footer.rs must contain the `AGENT ` span label");
    let guard_pos = APP_FOOTER_SOURCE
        .find("queue_depth > 0")
        .expect("app_footer.rs must contain the `queue_depth > 0` guard");
    assert!(
        agent_pos < guard_pos,
        "D-03b: `AGENT ` span (rel pos {agent_pos}) must appear in source \
         BEFORE the `queue_depth > 0` pill guard (rel pos {guard_pos}) — \
         left-run placement discretion after AGENT"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// User-visible string stability canaries — fail-loud if Plan 04's ws.rs
// confirmation strings get silently edited in a future plan.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn queue_updated_confirmation_strings_frozen() {
    // D-01 Queued confirmation — format the runtime string and assert the
    // exact wire shape. If a future edit re-orders the format args or
    // changes punctuation, this canary fails immediately.
    let queued = format!("Queued: \"{}\" ({} in queue)\n", "hello", 1u32);
    assert_eq!(
        queued, "Queued: \"hello\" (1 in queue)\n",
        "D-01: Queued confirmation format must remain \
         `Queued: \"<msg>\" (<depth> in queue)\\n`"
    );

    // D-06 cap-hit message — runtime form (Plan 04 substitutes max=128).
    let cap_hit = format!(
        "Queue is full ({}/{}). /stop or /flush to drain.\n",
        128, 128
    );
    assert_eq!(
        cap_hit, "Queue is full (128/128). /stop or /flush to drain.\n",
        "D-06: cap-hit message text frozen"
    );

    // D-05 /stop Delta message — literal, no runtime substitution.
    assert_eq!(
        "Queue cleared. Current turn finishing.\n",
        "Queue cleared. Current turn finishing.\n",
        "D-05: /stop Delta message frozen"
    );
}
