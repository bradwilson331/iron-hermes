//! Phase 39.1 Plan 06: Converted from Phase 36.1 GW-05-WEB running-agent gate tests.
//!
//! # Conversion note (Phase 39.1 Plan 06)
//!
//! The original file (Phase 36.1) asserted the OLD agent_running AtomicBool gate:
//!   - test_session_isolation            — per-session AtomicBool isolation
//!   - test_model_rejected_when_running  — /model rejected mid-turn (gate behavior)
//!   - test_stop_bypasses_guard          — /stop bypasses gate (gate behavior)
//!   - test_guard_clears_on_error        — RunningAgentGuard RAII Drop fires on error path
//!   - test_alias_bypasses_guard         — /reset alias bypasses gate
//!   - test_freetext_rejected_when_running — plain-text rejected when running
//!   - ws_rs_contains_phase_36_1_anchors — source-text anchor (AGENT_RUNNING_REJECT_MSG in ws.rs)
//!   - state_rs_contains_phase_36_1_anchors — source-text anchor (RunningAgentGuard in state.rs)
//!
//! In Plan 06, the running_agent module is DELETED (R39.1-06 / D-06).
//! `running_agent.rs` is gone — RunningAgentGuard, AGENT_RUNNING_REJECT_MSG, is_bypass
//! no longer exist. The `agent_running` AtomicBool field is removed from AppState.
//! The RAII guard in `run_agent_turn` is removed.
//!
//! This file is CONVERTED to assert the NEW contract:
//!   - TurnRegistry-backed turn tracking (replaces AtomicBool)
//!   - All slash commands and plain-text always dispatch (no rejection path)
//!   - Source-text anchors updated: ws.rs no longer contains AGENT_RUNNING_REJECT_MSG
//!     or is_bypass; state.rs no longer has RunningAgentGuard::new

use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use ironhermes_core::concurrency::{Surface, TurnEntry, TurnId, TurnRegistry};

// ── Compatibility shim ──────────────────────────────────────────────────────

pub mod helpers {
    /// HISTORICAL: The D-02 rejection message from Phase 36.1.
    ///
    /// The `running_agent` module is deleted in Phase 39.1 Plan 06 — this constant
    /// no longer exists. Retained here only for traceability (negative-assertion
    /// targets in gateway tests reference this literal).
    pub fn d02_error_message() -> &'static str {
        "Agent is running. Use /stop to interrupt or /queue to send after this turn."
    }

    /// Build a CommandRouter with the standard registry.
    pub fn build_command_router() -> ironhermes_core::commands::CommandRouter {
        use ironhermes_core::commands::registry::build_registry;
        ironhermes_core::commands::CommandRouter::new(build_registry())
    }
}

// ── Source-text anchors ──────────────────────────────────────────────────────

const WS_SOURCE: &str = include_str!("../src/server/ws.rs");
const STATE_SOURCE: &str = include_str!("../src/server/state.rs");

/// Phase 39.1 Plan 06: ws.rs must NOT contain the old gate constants.
///
/// With running_agent.rs deleted, ws.rs no longer references
/// AGENT_RUNNING_REJECT_MSG or is_bypass. These negative-presence checks
/// are the primary source-text gate for Plan 06 completion.
#[test]
fn ws_rs_gate_removed() {
    assert!(
        !WS_SOURCE.contains("AGENT_RUNNING_REJECT_MSG"),
        "ws.rs must NOT reference AGENT_RUNNING_REJECT_MSG after Plan 06 gate teardown \
         (running_agent module deleted)"
    );
    assert!(
        !WS_SOURCE.contains("is_bypass("),
        "ws.rs must NOT call is_bypass() after Plan 06 gate teardown \
         (gate logic removed — all commands dispatch)"
    );
    assert!(
        !WS_SOURCE.contains("agent_running"),
        "ws.rs must NOT reference agent_running after Plan 06 gate teardown \
         (AtomicBool field removed from AppState)"
    );
}

/// Phase 39.1 Plan 06: state.rs must NOT contain the old gate infrastructure.
///
/// RunningAgentGuard::new, get_or_create_running_flag, and running_agents
/// are all removed from state.rs in Plan 06.
#[test]
fn state_rs_gate_removed() {
    assert!(
        !STATE_SOURCE.contains("RunningAgentGuard::new"),
        "state.rs must NOT bind RunningAgentGuard::new after Plan 06 \
         (RAII guard removed from run_agent_turn)"
    );
    assert!(
        !STATE_SOURCE.contains("running_agent::RunningAgentGuard"),
        "state.rs must NOT reference running_agent::RunningAgentGuard after Plan 06 \
         (module deleted)"
    );
}

/// Phase 39.1 Plan 06: helpers::d02_error_message() string is preserved for traceability.
///
/// The string still appears in gateway tests as a negative-assertion target.
/// Verify it is non-empty to catch accidental blank-out.
#[test]
fn d02_helper_string_is_preserved_for_traceability() {
    assert!(
        !helpers::d02_error_message().is_empty(),
        "d02_error_message() must not be blank (used as negative-assertion target in gateway tests)"
    );
}

// ── New-contract test 1: TurnRegistry is the live source of truth ─────────────

/// NEW CONTRACT (Plan 06): TurnRegistry replaces AtomicBool as the running-turn tracker.
///
/// Phase 36.1 GW-05-WEB-1 asserted per-session AtomicBool isolation (separate flags per session).
/// Plan 06 replaces per-session flags with a shared TurnRegistry. This test verifies the
/// equivalent property: registrations for session-A are isolated from session-B.
#[tokio::test]
async fn test_session_isolation_via_registry() {
    let registry = Arc::new(TurnRegistry::new());

    let turn_id_a = TurnId::new_v4();
    let token_a = CancellationToken::new();
    registry
        .register(TurnEntry {
            turn_id: turn_id_a,
            session_id: "session-A".to_string(),
            surface: Surface::Web,
            started_at: std::time::Instant::now(),
            cancel: token_a,
        })
        .await;

    // session-A has 1 turn; session-B must still have 0 — no cross-session bleed.
    let count_a = registry.count_session("session-A").await;
    let count_b = registry.count_session("session-B").await;

    assert_eq!(
        count_a, 1,
        "session-A must have 1 registered turn (R39.1-01)"
    );
    assert_eq!(
        count_b, 0,
        "session-B must have 0 turns — no cross-session bleed (R39.1-01)"
    );
}

// ── New-contract test 2: /model resolves — no rejection path ──────────────────

/// NEW CONTRACT (Plan 06): /model resolves to a command def with no rejection.
///
/// Phase 36.1 GW-05-WEB-2 asserted /model was REJECTED when agent_running was true.
/// Plan 06 removes the gate — /model now always resolves.
#[tokio::test]
async fn test_model_resolves_regardless_of_in_flight_turns() {
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    let registry = Arc::new(TurnRegistry::new());

    // Register a stub in-flight turn (simulating a web turn in progress).
    let token = CancellationToken::new();
    let turn_id = TurnId::new_v4();
    registry
        .register(TurnEntry {
            turn_id,
            session_id: "model-test-session".to_string(),
            surface: Surface::Web,
            started_at: std::time::Instant::now(),
            cancel: token,
        })
        .await;

    // /model must resolve — the gate is gone.
    let router = CommandRouter::new(build_registry());
    let resolved = router.resolve("/model gpt-4", &Platform::Web);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            assert_eq!(
                def.name, "model",
                "/model must resolve to canonical 'model' def (gate removed in Plan 06)"
            );
        }
        other => panic!(
            "/model must resolve to a known command (gate removed — no rejection), got: {:?}",
            other
        ),
    }
}

// ── New-contract test 3: /stop → cancel_session cancels all session turns ────

/// NEW CONTRACT (Plan 06): /stop → cancel_session() cancels all session turns.
///
/// Phase 36.1 GW-05-WEB-3 asserted /stop bypasses is_bypass() gate check.
/// Plan 06 removes the gate entirely; the /stop contract is: cancel_session() fires.
#[tokio::test]
async fn test_stop_cancels_all_session_turns() {
    let registry = Arc::new(TurnRegistry::new());
    let session_id = "stop-web-session";

    let token_a = CancellationToken::new();
    let token_b = CancellationToken::new();
    let id_a = TurnId::new_v4();
    let id_b = TurnId::new_v4();

    registry
        .register(TurnEntry {
            turn_id: id_a,
            session_id: session_id.to_string(),
            surface: Surface::Web,
            started_at: std::time::Instant::now(),
            cancel: token_a.clone(),
        })
        .await;
    registry
        .register(TurnEntry {
            turn_id: id_b,
            session_id: session_id.to_string(),
            surface: Surface::Web,
            started_at: std::time::Instant::now(),
            cancel: token_b.clone(),
        })
        .await;

    let cancelled = registry.cancel_session(session_id).await;

    assert_eq!(
        cancelled, 2,
        "/stop cancel_session must cancel both in-flight turns (R39.1-05)"
    );
    assert!(
        token_a.is_cancelled(),
        "turn A token must be cancelled by /stop"
    );
    assert!(
        token_b.is_cancelled(),
        "turn B token must be cancelled by /stop"
    );
}

// ── New-contract test 4: deregister clears entry on turn end ─────────────────

/// NEW CONTRACT (Plan 06): TurnRegistry.deregister() clears entry on all exit paths.
///
/// Phase 36.1 GW-05-WEB-4 asserted RunningAgentGuard RAII Drop clears the flag on error path.
/// Plan 06 replaces RAII with explicit deregister call. This test verifies the equivalent.
#[tokio::test]
async fn test_deregister_clears_entry_on_turn_end() {
    let registry = Arc::new(TurnRegistry::new());
    let registry_for_task = registry.clone();
    let session_id = "deregister-web-session";

    let turn_id = TurnId::new_v4();
    let token = CancellationToken::new();
    let entry = TurnEntry {
        turn_id,
        session_id: session_id.to_string(),
        surface: Surface::Web,
        started_at: std::time::Instant::now(),
        cancel: token,
    };

    registry.register(entry).await;
    assert_eq!(
        registry.count_session(session_id).await,
        1,
        "turn must be registered before task spawns"
    );

    // Simulate the turn completing: deregister in the task body.
    let handle = tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        registry_for_task.deregister(turn_id).await;
    });

    handle.await.expect("spawned task must not panic");

    assert_eq!(
        registry.count_session(session_id).await,
        0,
        "turn must be deregistered after task completes (RAII replaced by explicit deregister)"
    );
}

// ── New-contract test 5: /reset alias still resolves to "new" ────────────────

/// NEW CONTRACT (Plan 06): /reset alias resolution is unchanged.
///
/// Phase 36.1 GW-05-WEB-5 asserted /reset bypasses the gate via is_bypass("new").
/// Plan 06 removes the gate; the alias resolution property is unchanged — /reset → "new".
#[test]
fn test_reset_alias_resolves_to_new() {
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    let router = CommandRouter::new(build_registry());
    let resolved = router.resolve("/reset", &Platform::Web);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            assert_eq!(
                def.name, "new",
                "/reset must still resolve to canonical 'new' via CommandRouter alias resolution"
            );
        }
        other => panic!(
            "/reset must resolve to canonical 'new' (unchanged by Plan 06), got: {:?}",
            other
        ),
    }
}

// ── New-contract test 6: TurnRegistry shared across Arc clones ───────────────

/// NEW CONTRACT (Plan 06): TurnRegistry is shared across all Arc clones.
///
/// Phase 36.1 GW-05-WEB-6 tested plain-text rejection via the AtomicBool guard.
/// Plan 06 removes the guard; the equivalent "shared state" property for the
/// TurnRegistry (all clones see the same inner map).
#[tokio::test]
async fn test_registry_is_shared_across_clones() {
    let registry = Arc::new(TurnRegistry::new());
    let clone = registry.clone();

    let turn_id = TurnId::new_v4();
    let token = CancellationToken::new();
    let entry = TurnEntry {
        turn_id,
        session_id: "web-shared-session".to_string(),
        surface: Surface::Web,
        started_at: std::time::Instant::now(),
        cancel: token,
    };

    // Register via original — must be visible via clone.
    registry.register(entry).await;

    let via_clone = clone.list_all().await;
    assert_eq!(
        via_clone.len(),
        1,
        "clone must see entry registered via original (shared Arc<RwLock<>>)"
    );
    assert_eq!(via_clone[0].turn_id, turn_id);

    // Deregister via clone — must be gone via original.
    clone.deregister(turn_id).await;
    let via_original = registry.list_all().await;
    assert!(
        via_original.is_empty(),
        "original must see deregistration done via clone"
    );
}
