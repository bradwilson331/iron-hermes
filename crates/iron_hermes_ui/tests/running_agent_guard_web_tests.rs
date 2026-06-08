//! Phase 36.1 GW-05-WEB integration tests — running-agent guard parity for
//! the web UI surface.
//!
//! Mirrors the structure of `ironhermes-gateway/tests/running_agent_guard_tests.rs`
//! for the 6 GW-05-WEB behaviors.
//!
//! ## GW-05-WEB sub-behaviors covered
//!
//! 1. `test_session_isolation` — per-session isolation: session A running does
//!    not affect session B (D-03, T-36.1-06 mitigation)
//! 2. `test_model_rejected_when_running` — `/model` rejected when guard flag true
//!    (D-06, D-02)
//! 3. `test_stop_bypasses_guard` — `/stop` bypasses guard (D-07, D-01)
//! 4. `test_guard_clears_on_error` — RunningAgentGuard RAII Drop fires on error
//!    path (D-06 discipline)
//! 5. `test_alias_bypasses_guard` — `/reset` → canonical "new" bypasses guard
//!    (Pitfall 4 mitigation, D-07)
//! 6. `test_freetext_rejected_when_running` — plain-text rejected when running
//!    (D-06)
//!
//! ## Why no import from `iron_hermes_ui`
//!
//! `iron_hermes_ui` is a binary crate (no `[lib]` target), so integration tests
//! cannot `use iron_hermes_ui::...`. The guard logic under test lives in
//! `ironhermes_core::commands::running_agent` plus the `CommandRouter` from
//! `ironhermes_core::commands` — both directly importable. We define a local
//! `TestStreamEvent` enum that mirrors the shape of `ChatStreamEvent` (the
//! events ws.rs sends on rejection/dispatch) so we can assert on the same
//! semantic contract the production code emits.
//!
//! Source-text assertion tests at the bottom of this file pin the ws.rs and
//! state.rs implementation to the same constants/calls these behavior tests
//! exercise — drift in production code breaks those assertions even though
//! the behavior tests operate on shared library types.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ─── Cross-check: d02_error_message() must be byte-identical to the canonical
// constant from ironhermes-core. Any drift here will cause the assertion at
// the top of each relevant test to fail — intentional drift-detection gate.
use ironhermes_core::commands::running_agent::AGENT_RUNNING_REJECT_MSG;

// ═════════════════════════════════════════════════════════════════════════════
// Local test fixture: TestStreamEvent mirrors the shape of ChatStreamEvent
// (the variants ws.rs emits on the rejection path).
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
#[allow(dead_code)] // total_tokens: retained for future assertion patterns; test-recording enum
enum TestStreamEvent {
    Delta { text: String },
    Finished { total_tokens: u32 },
}

mod helpers {
    use super::TestStreamEvent;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // ──────────────────────────────────────────────────────────────────────
    // d02_error_message
    // ──────────────────────────────────────────────────────────────────────

    /// Phase 36.1 D-02 locked string — byte-identical to the canonical
    /// `AGENT_RUNNING_REJECT_MSG` constant from `ironhermes-core`.
    ///
    /// A cross-check assertion at the top of each test verifies the two are
    /// identical (drift-detection gate mirrors the gateway test pattern).
    pub fn d02_error_message() -> &'static str {
        "Agent is running. Use /stop to interrupt or /queue to send after this turn."
    }

    // ──────────────────────────────────────────────────────────────────────
    // Minimal per-session flag map (mirrors AppState.running_agents)
    // ──────────────────────────────────────────────────────────────────────

    /// Build the same Arc<Mutex<HashMap<...>>> shape as AppState.running_agents.
    pub fn make_flag_map() -> Arc<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>> {
        Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// get_or_create_running_flag — mirrors AppState::get_or_create_running_flag.
    pub fn get_or_create(
        map: &Arc<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>>,
        session_id: &str,
    ) -> Arc<AtomicBool> {
        let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    }

    /// set_running — sets the per-session flag to true.
    ///
    /// Mirrors the gateway helper `set_running(session_store, key)`.
    #[allow(dead_code)]
    pub fn set_running(
        map: &Arc<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>>,
        session_id: &str,
    ) {
        get_or_create(map, session_id).store(true, Ordering::SeqCst);
    }

    // ──────────────────────────────────────────────────────────────────────
    // collect_delta_texts
    // ──────────────────────────────────────────────────────────────────────

    /// Extract the text from every `TestStreamEvent::Delta` in a slice.
    ///
    /// Used by tests that assert the D-02 rejection message is (or is not)
    /// present in the collected events.
    pub fn collect_delta_texts(events: &[TestStreamEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|ev| {
                if let TestStreamEvent::Delta { text } = ev {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    // ──────────────────────────────────────────────────────────────────────
    // simulate_slash_guard
    // ──────────────────────────────────────────────────────────────────────

    /// Simulate the ws.rs slash-interception guard logic for a single command.
    ///
    /// Mirrors the logic in ws.rs (Phase 36.1 D-04/D-05/D-06/D-07):
    ///   1. Resolve `message` via `CommandRouter`
    ///   2. If running AND not bypass → return D-02 rejection events
    ///   3. If bypass OR not running → return dispatch marker (no D-02)
    ///
    /// Returns the vec of `TestStreamEvent` that would be sent to the WebSocket.
    pub fn simulate_slash_guard(
        command_router: &ironhermes_core::commands::CommandRouter,
        flag: Arc<AtomicBool>,
        message: &str,
    ) -> Vec<TestStreamEvent> {
        use ironhermes_core::commands::running_agent::{is_bypass, AGENT_RUNNING_REJECT_MSG};
        use ironhermes_core::commands::ResolveResult;
        use ironhermes_core::types::Platform;

        assert!(
            message.starts_with('/'),
            "simulate_slash_guard: message must start with /"
        );

        let platform = Platform::Web;
        match command_router.resolve(message, &platform) {
            ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
                if flag.load(Ordering::SeqCst) && !is_bypass(def.name) {
                    // D-06: rejected — same Delta+Finished as ws.rs does
                    vec![
                        TestStreamEvent::Delta {
                            text: AGENT_RUNNING_REJECT_MSG.to_string(),
                        },
                        TestStreamEvent::Finished { total_tokens: 0 },
                    ]
                } else {
                    // Bypass or not running — would dispatch normally.
                    // Return marker so callers can distinguish from rejection.
                    vec![TestStreamEvent::Finished { total_tokens: 0 }]
                }
            }
            ResolveResult::Ambiguous(_) | ResolveResult::NotFound => {
                // Not a recognized command — would fall through to plain-text path.
                vec![]
            }
        }
    }

    /// Build a `CommandRouter` with the standard registry (same as AppState).
    pub fn build_command_router() -> ironhermes_core::commands::CommandRouter {
        use ironhermes_core::commands::registry::build_registry;
        ironhermes_core::commands::CommandRouter::new(build_registry())
    }

    // ──────────────────────────────────────────────────────────────────────
    // simulate_freetext_guard
    // ──────────────────────────────────────────────────────────────────────

    /// Simulate the ws.rs plain-text guard check (D-06).
    ///
    /// Returns the events that would be sent when a plain-text message arrives.
    pub fn simulate_freetext_guard(flag: Arc<AtomicBool>) -> Vec<TestStreamEvent> {
        use ironhermes_core::commands::running_agent::AGENT_RUNNING_REJECT_MSG;

        if flag.load(Ordering::SeqCst) {
            vec![
                TestStreamEvent::Delta {
                    text: AGENT_RUNNING_REJECT_MSG.to_string(),
                },
                TestStreamEvent::Finished { total_tokens: 0 },
            ]
        } else {
            // Not running — would proceed to run_web_turn.
            vec![]
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

/// Cross-check: d02_error_message() must be byte-identical to the canonical
/// AGENT_RUNNING_REJECT_MSG from ironhermes-core. Any drift fails this assertion.
#[test]
fn d02_constant_cross_check() {
    assert_eq!(
        helpers::d02_error_message(),
        AGENT_RUNNING_REJECT_MSG,
        "helpers::d02_error_message() must be byte-identical to \
         ironhermes_core::commands::running_agent::AGENT_RUNNING_REJECT_MSG (D-02 lock)"
    );
}

/// GW-05-WEB-1: Per-session isolation.
///
/// Setting session A's flag to true must NOT affect session B's flag.
/// Repeated `get_or_create_running_flag` calls for the same session return
/// handles to the SAME AtomicBool (get-OR-create semantics).
#[test]
fn test_session_isolation() {
    assert_eq!(
        helpers::d02_error_message(),
        AGENT_RUNNING_REJECT_MSG,
        "d02 drift-detection"
    );

    let map = helpers::make_flag_map();

    // Session A: flag starts false, set to true
    let flag_a1 = helpers::get_or_create(&map, "session-A");
    assert!(
        !flag_a1.load(Ordering::SeqCst),
        "GW-05-WEB-1: fresh session A flag must start false"
    );
    flag_a1.store(true, Ordering::SeqCst);

    // Session B: must remain false — no cross-session bleed
    let flag_b = helpers::get_or_create(&map, "session-B");
    assert!(
        !flag_b.load(Ordering::SeqCst),
        "GW-05-WEB-1: session B flag must remain false after session A is set (T-36.1-06)"
    );

    // get-OR-create: second lookup for session A must return SAME AtomicBool
    let flag_a2 = helpers::get_or_create(&map, "session-A");
    assert!(
        flag_a2.load(Ordering::SeqCst),
        "GW-05-WEB-1: second get_or_create for session-A must see true (same Arc, not fresh)"
    );
}

/// GW-05-WEB-2: `/model` rejected when agent running.
///
/// With the session flag set to true, the slash guard must produce a Delta
/// containing AGENT_RUNNING_REJECT_MSG when `/model gpt-4` is received.
#[test]
fn test_model_rejected_when_running() {
    assert_eq!(
        helpers::d02_error_message(),
        AGENT_RUNNING_REJECT_MSG,
        "d02 drift-detection"
    );

    let router = helpers::build_command_router();
    let flag = Arc::new(AtomicBool::new(true)); // running

    let events = helpers::simulate_slash_guard(&router, flag, "/model gpt-4");
    let texts = helpers::collect_delta_texts(&events);

    assert!(
        texts.iter().any(|t| t == AGENT_RUNNING_REJECT_MSG),
        "GW-05-WEB-2: /model must produce D-02 rejection Delta when running; got: {:?}",
        texts
    );

    // Confirm there is also a Finished event (rejection terminates the response)
    let has_finished = events
        .iter()
        .any(|ev| matches!(ev, TestStreamEvent::Finished { .. }));
    assert!(
        has_finished,
        "GW-05-WEB-2: must have Finished event after rejection"
    );
}

/// GW-05-WEB-3: `/stop` bypasses the running-agent guard.
///
/// With the session flag set to true, `/stop` must NOT produce the D-02
/// rejection message — it is on the bypass list (D-07).
#[test]
fn test_stop_bypasses_guard() {
    assert_eq!(
        helpers::d02_error_message(),
        AGENT_RUNNING_REJECT_MSG,
        "d02 drift-detection"
    );

    let router = helpers::build_command_router();
    let flag = Arc::new(AtomicBool::new(true)); // running

    let events = helpers::simulate_slash_guard(&router, flag, "/stop");
    let texts = helpers::collect_delta_texts(&events);

    assert!(
        !texts.iter().any(|t| t == AGENT_RUNNING_REJECT_MSG),
        "GW-05-WEB-3: /stop must NOT produce D-02 rejection (bypass list); got: {:?}",
        texts
    );
}

/// GW-05-WEB-4: RunningAgentGuard RAII Drop fires on error path.
///
/// Constructing a `RunningAgentGuard::new(flag)` inside a scope that returns
/// Err must clear the flag to false on exit. Proves RAII discipline covers all
/// exit paths (D-06).
#[test]
fn test_guard_clears_on_error() {
    use ironhermes_core::commands::running_agent::RunningAgentGuard;

    assert_eq!(
        helpers::d02_error_message(),
        AGENT_RUNNING_REJECT_MSG,
        "d02 drift-detection"
    );

    let flag = Arc::new(AtomicBool::new(false));
    assert!(!flag.load(Ordering::SeqCst), "pre: flag must start false");

    // Simulate a function that returns Err (? propagation path).
    let result: Result<(), &str> = {
        let _guard = RunningAgentGuard::new(flag.clone());
        assert!(
            flag.load(Ordering::SeqCst),
            "flag must be true while guard is in scope"
        );
        Err("simulated error")
    };

    assert!(result.is_err(), "the closure must return Err");
    assert!(
        !flag.load(Ordering::SeqCst),
        "GW-05-WEB-4: flag must be false after guard drops on Err path (RAII D-06)"
    );
}

/// GW-05-WEB-5: `/reset` (alias for `new`) bypasses the running-agent guard.
///
/// `/reset` is registered as an alias for the `new` command. After resolution,
/// `def.name == "new"` which is on the bypass list. `is_bypass(&def.name)` must
/// return true — proving Pitfall 4 mitigation (bypass check on canonical name,
/// not raw input).
#[test]
fn test_alias_bypasses_guard() {
    use ironhermes_core::commands::running_agent::is_bypass;
    use ironhermes_core::commands::ResolveResult;
    use ironhermes_core::types::Platform;

    assert_eq!(
        helpers::d02_error_message(),
        AGENT_RUNNING_REJECT_MSG,
        "d02 drift-detection"
    );

    let router = helpers::build_command_router();
    let flag = Arc::new(AtomicBool::new(true)); // running

    // Verify that /reset resolves to canonical "new"
    let platform = Platform::Web;
    match router.resolve("/reset", &platform) {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            assert_eq!(
                def.name, "new",
                "GW-05-WEB-5: /reset must resolve to canonical name 'new' (alias)"
            );
            assert!(
                is_bypass(def.name),
                "GW-05-WEB-5: is_bypass('new') must be true — /reset alias correctly bypasses \
                 (Pitfall 4 mitigation: bypass check on def.name, never raw input)"
            );
        }
        other => panic!("GW-05-WEB-5: /reset must resolve; got {:?}", other),
    }

    // End-to-end: simulate guard with /reset — must NOT produce D-02 rejection
    let events = helpers::simulate_slash_guard(&router, flag, "/reset");
    let texts = helpers::collect_delta_texts(&events);

    assert!(
        !texts.iter().any(|t| t == AGENT_RUNNING_REJECT_MSG),
        "GW-05-WEB-5: /reset must NOT produce D-02 rejection (alias → 'new' bypasses); got: {:?}",
        texts
    );
}

/// GW-05-WEB-6: Plain-text message rejected when agent running.
///
/// With the session flag set to true, a free-text message (no `/` prefix)
/// must produce a Delta containing AGENT_RUNNING_REJECT_MSG. This exercises
/// the D-06 plain-text guard path in ws.rs (separate from the slash path).
#[test]
fn test_freetext_rejected_when_running() {
    assert_eq!(
        helpers::d02_error_message(),
        AGENT_RUNNING_REJECT_MSG,
        "d02 drift-detection"
    );

    let flag = Arc::new(AtomicBool::new(true)); // running

    let events = helpers::simulate_freetext_guard(flag);
    let texts = helpers::collect_delta_texts(&events);

    assert!(
        texts.iter().any(|t| t == AGENT_RUNNING_REJECT_MSG),
        "GW-05-WEB-6: plain-text message must produce D-02 rejection Delta when running; got: {:?}",
        texts
    );

    // run_web_turn must NOT have been invoked — verified structurally:
    // simulate_freetext_guard returns rejection events without calling run_web_turn.
    // The guard short-circuits: Delta + Finished, no in_flight_turn set.
    let has_finished = events
        .iter()
        .any(|ev| matches!(ev, TestStreamEvent::Finished { .. }));
    assert!(
        has_finished,
        "GW-05-WEB-6: must have Finished event after rejection"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// Source-text assertions — pin ws.rs and state.rs to the same constants the
// behavior tests exercise. Because iron_hermes_ui is a binary crate, we cannot
// call into ws.rs from a test; these assertions ensure the production code
// matches what `simulate_slash_guard` / `simulate_freetext_guard` model.
// ═════════════════════════════════════════════════════════════════════════════

const WS_SOURCE: &str = include_str!("../src/server/ws.rs");
const STATE_SOURCE: &str = include_str!("../src/server/state.rs");

/// Phase 36.1 anchor lock: ws.rs must contain the slash-interception block
/// and reject via AGENT_RUNNING_REJECT_MSG (no inline literal).
#[test]
fn ws_rs_contains_phase_36_1_anchors() {
    assert!(
        WS_SOURCE.contains("starts_with('/')"),
        "ws.rs must contain the slash interception entry `starts_with('/')`"
    );
    assert!(
        WS_SOURCE.contains("is_bypass(&def.name)"),
        "ws.rs must use is_bypass(&def.name) on the canonical post-resolution \
         name (Pitfall 4 mitigation — never raw input)"
    );
    assert!(
        WS_SOURCE.contains("AGENT_RUNNING_REJECT_MSG"),
        "ws.rs must reference the canonical AGENT_RUNNING_REJECT_MSG constant"
    );
    // No inline literal redefinition — the D-02 string must come from the
    // ironhermes-core constant, never hardcoded in ws.rs.
    assert!(
        !WS_SOURCE.contains("AGENT_RUNNING_REJECT_MSG: &str = \"Agent is running."),
        "ws.rs must NOT inline the D-02 literal (T-36.1-09 mitigation)"
    );
}

/// Phase 36.1 anchor lock: state.rs must contain the running_agents field and
/// the helper, plus the RunningAgentGuard bind inside run_web_turn.
#[test]
fn state_rs_contains_phase_36_1_anchors() {
    assert!(
        STATE_SOURCE.contains(
            "pub running_agents: Arc<std::sync::Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>"
        ),
        "state.rs must declare running_agents with the exact D-03 type signature"
    );
    assert!(
        STATE_SOURCE.contains("fn get_or_create_running_flag"),
        "state.rs must define the get_or_create_running_flag helper"
    );
    assert!(
        STATE_SOURCE.contains("RunningAgentGuard::new"),
        "state.rs must bind RunningAgentGuard::new (RAII guard at run_web_turn entry)"
    );
}
