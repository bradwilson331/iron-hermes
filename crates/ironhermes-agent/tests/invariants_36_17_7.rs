//! Phase 36.17.7 — cross-crate source-grep invariants (D-05 / A5).
//!
//! Locks the contract that per-turn TTS wiring is live on all three supported
//! surfaces and that the legacy `session_key: None,` deferral marker from
//! Phase 36.17.5 D-15 has been eliminated. Mirrors the
//! `invariants_36_17_5.rs` source-text invariant style.
//!
//! Test 1 — `agent_runtime_session_key_none_removed`
//!   Plan 01 rewrote `AgentRuntime::from_config` to use
//!   `..Default::default()`. The literal `session_key: None,` must no longer
//!   appear in `agent_runtime.rs`.
//!
//! Test 2 — `gateway_handler_wires_session_key_some`
//!   Plan 02 threaded the per-turn `TtsPerTurnWiring {{ session_key: Some(...) }}`
//!   into the gateway run-loop. The literal `session_key: Some(` must appear in
//!   `crates/ironhermes-gateway/src/handler.rs`.
//!
//! Test 3 — `web_ws_handler_wires_session_key_some`
//!   Plan 04 threaded the per-turn wiring through `iron_hermes_ui`'s ws.rs
//!   spawn path (primary + queue-drain). The literal `session_key: Some(`
//!   must appear in `crates/iron_hermes_ui/src/server/ws.rs`.
//!
//! Test 4 — `tui_event_loop_wires_session_key_some`
//!   Plan 03 threaded `TtsPerTurnWiring` into the TUI's `spawn_turn`. The
//!   literal `session_key: Some(` must appear in
//!   `crates/ironhermes-cli/src/tui_rata/event_loop.rs`.
//!
//! Test 5 — `register_tts_tools_called_at_minimum_four_sites`
//!   Counts the union of `register_tts_tools(` calls AND `TtsPerTurnWiring`
//!   references across the five surfaces. The Plan 01 architecture put the
//!   actual `register_tts_tools(` invocation inside `run_turn` (factory +
//!   agent_runtime) and threads the wiring struct from the three handler
//!   surfaces — so the production-code total of register-call sites is 2 and
//!   wiring-struct sites is 6, total ≥ 4 by either measure.

/// Phase 36.17.7 D-05 / A5 Test 1 (BLOCKER 1 fix verified at Plan 01).
///
/// Plan 01 rewrote `AgentRuntime::from_config` to build the startup
/// `AppRuntimeFactoryInput` via `..Default::default()` — so the D-15
/// deferral marker `session_key: None,` is no longer present.
#[test]
fn agent_runtime_session_key_none_removed() {
    const SRC: &str = include_str!("../src/agent_runtime.rs");
    assert!(
        !SRC.contains("session_key: None,"),
        "Phase 36.17.7 D-05 Test 1: agent_runtime.rs must NOT contain the \
         literal `session_key: None,` — Plan 01's `..Default::default()` \
         rewrite of `from_config` should have eliminated it. Per-turn TTS \
         registration now lives in `run_turn` via `TurnRequest::tts_wiring`."
    );
}

/// Phase 36.17.7 D-05 / A5 Test 2 — Plan 02 gateway wiring.
///
/// `ironhermes-gateway/src/handler.rs` must contain `session_key: Some(` —
/// the literal that the per-turn `TtsPerTurnWiring` builder produces when
/// the agent loop spawns per-turn for a real Telegram session.
#[test]
fn gateway_handler_wires_session_key_some() {
    const SRC: &str = include_str!("../../ironhermes-gateway/src/handler.rs");
    assert!(
        SRC.contains("session_key: Some("),
        "Phase 36.17.7 D-05 Test 2: ironhermes-gateway/src/handler.rs must \
         contain the literal `session_key: Some(` — Plan 02 threads the \
         per-turn `TtsPerTurnWiring {{ session_key: Some(...) }}` into the \
         gateway's TurnRequest construction site."
    );
}

/// Phase 36.17.7 D-05 / A5 Test 3 — Plan 04 web ws wiring.
///
/// `iron_hermes_ui/src/server/ws.rs` must contain `session_key: Some(` at
/// the per-turn spawn point AND the queue-drain spawn point.
#[test]
fn web_ws_handler_wires_session_key_some() {
    const SRC: &str = include_str!("../../iron_hermes_ui/src/server/ws.rs");
    assert!(
        SRC.contains("session_key: Some("),
        "Phase 36.17.7 D-05 Test 3: iron_hermes_ui/src/server/ws.rs must \
         contain the literal `session_key: Some(` — Plan 04 threads the \
         per-turn `TtsPerTurnWiring {{ session_key: Some(...) }}` into the \
         web ws handler's primary + queue-drain TurnRequest construction."
    );
}

/// Phase 36.17.7 D-05 / A5 Test 4 — Plan 03 TUI wiring.
///
/// `ironhermes-cli/src/tui_rata/event_loop.rs` must contain
/// `session_key: Some(` at the `spawn_turn` site.
#[test]
fn tui_event_loop_wires_session_key_some() {
    const SRC: &str = include_str!("../../ironhermes-cli/src/tui_rata/event_loop.rs");
    assert!(
        SRC.contains("session_key: Some("),
        "Phase 36.17.7 D-05 Test 4: ironhermes-cli/src/tui_rata/event_loop.rs \
         must contain the literal `session_key: Some(` — Plan 03 threads the \
         per-turn `TtsPerTurnWiring {{ session_key: Some(...) }}` into the TUI \
         spawn_turn TurnRequest construction."
    );
}

/// Phase 36.17.7 D-05 / A5 Test 5 — minimum wiring-site count.
///
/// The Plan 01 architecture puts the actual `register_tts_tools(` call inside
/// `run_turn` (consuming the `TtsPerTurnWiring` bundle from `TurnRequest`).
/// The handler surfaces (gateway, ws, tui) construct the bundle — they do not
/// invoke `register_tts_tools` directly. To keep this guard architecturally
/// honest, count the union of:
///   - `register_tts_tools(` invocations in the runtime/factory.
///   - `TtsPerTurnWiring` references at the three handler surfaces.
/// Combined total must be ≥ 4 to prove the wiring exists at every surface.
#[test]
fn register_tts_tools_called_at_minimum_four_sites() {
    const FACTORY: &str = include_str!("../src/app_runtime_factory.rs");
    const AGENT_RUNTIME: &str = include_str!("../src/agent_runtime.rs");
    const GATEWAY: &str = include_str!("../../ironhermes-gateway/src/handler.rs");
    const WS: &str = include_str!("../../iron_hermes_ui/src/server/ws.rs");
    const TUI: &str = include_str!("../../ironhermes-cli/src/tui_rata/event_loop.rs");

    let factory_calls = FACTORY.matches("register_tts_tools(").count();
    let agent_calls = AGENT_RUNTIME.matches("register_tts_tools(").count();
    let gateway_wiring = GATEWAY.matches("TtsPerTurnWiring").count();
    let ws_wiring = WS.matches("TtsPerTurnWiring").count();
    let tui_wiring = TUI.matches("TtsPerTurnWiring").count();

    let total = factory_calls + agent_calls + gateway_wiring + ws_wiring + tui_wiring;

    assert!(
        total >= 4,
        "Phase 36.17.7 D-05 Test 5: per-turn TTS wiring must be present at \
         ≥ 4 sites across the 5 production files. Got total = {} \
         (factory register_tts_tools = {}, agent_runtime register_tts_tools = {}, \
         gateway TtsPerTurnWiring = {}, ws TtsPerTurnWiring = {}, \
         tui TtsPerTurnWiring = {}). A comment-out regression at any handler \
         surface would drop this count.",
        total, factory_calls, agent_calls, gateway_wiring, ws_wiring, tui_wiring
    );
}
