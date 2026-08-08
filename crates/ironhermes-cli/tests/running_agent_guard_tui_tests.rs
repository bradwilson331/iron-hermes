//! Phase 39.1 Plan 04: Converted from Phase 36.1 GW-05-TUI running-agent gate tests.
//!
//! # Conversion note (Phase 39.1 Plan 04)
//!
//! The original file (Phase 36.1) asserted the OLD agent_running AtomicBool gate:
//!   - test_agent_running_is_live_arc      — live Arc invariant (gate behavior)
//!   - test_guard_clears_on_turn_end       — RunningAgentGuard RAII (gate behavior)
//!   - test_model_rejected_when_running    — /model rejected mid-turn (gate behavior)
//!   - test_stop_bypasses_guard            — /stop bypasses gate (gate behavior)
//!   - test_alias_reset_bypasses_guard     — /reset alias bypasses gate (gate behavior)
//!
//! In Plan 04, the gate is REMOVED (R39.1-06 / D-06). The `is_bypass` check and
//! `AGENT_RUNNING_REJECT_MSG` path no longer exist in tui_rata/commands.rs dispatch.
//! RunningAgentGuard is no longer used in spawn_turn.
//!
//! This file is CONVERTED to assert the NEW contract:
//!   - Registry-backed turn tracking (TurnRegistry replaces AtomicBool)
//!   - All slashes dispatch mid-turn (no rejection path)
//!   - Per-turn cancel via cancel_one (R39.1-05)
//!   - /stop → cancel_session cancels all session turns
//!
//! The old gate-specific tests are replaced with structurally equivalent tests that
//! exercise the registry rather than the removed AtomicBool. The helpers module is
//! retained for compatibility with any downstream grep tools, but d02_error_message()
//! is annotated as HISTORICAL — the constant still exists in core (other surfaces may
//! still use it) but is no longer applied in the TUI dispatch path.

use std::sync::Arc;

use ironhermes_core::concurrency::{Surface, TurnEntry, TurnId, TurnRegistry};
use tokio_util::sync::CancellationToken;

// ── Compatibility shim ──────────────────────────────────────────────────────

pub mod helpers {
    /// HISTORICAL: The D-02 rejection message from Phase 36.1.
    ///
    /// This constant still exists in ironhermes_core::commands::running_agent but is NO
    /// LONGER applied in the tui_rata dispatch path (Plan 04 removed the gate). Retained
    /// here only for traceability — test files that imported this helper won't break.
    ///
    /// The tui_rata commands.rs gate that used AGENT_RUNNING_REJECT_MSG was removed in
    /// Phase 39.1 Plan 04. The constant remains in core for Gateway and classic-TUI surfaces.
    pub fn d02_error_message() -> &'static str {
        "Agent is running. Use /stop to interrupt or /queue to send after this turn."
    }
}

/// Phase 39.1 Plan 06 (R39.1-06): `running_agent.rs` deleted — constant no longer exists.
///
/// The traceability check that compared helpers::d02_error_message() to
/// `ironhermes_core::commands::running_agent::AGENT_RUNNING_REJECT_MSG` is removed
/// because the module and constant are gone. The helper function is retained for
/// code-search traceability only (see module doc-comment in helpers).
#[test]
fn d02_helper_string_is_preserved_for_traceability() {
    // The string is still used as a negative-assertion target in gateway tests
    // (concurrent_turns_gateway.rs, running_agent_guard_tests.rs). Verify it's
    // non-empty to catch accidental blank-out.
    assert!(
        !helpers::d02_error_message().is_empty(),
        "d02_error_message() must not be blank (used as negative-assertion target in gateway tests)"
    );
}

// ── New-contract test 1: TurnRegistry is the live source of truth ────────────

/// NEW CONTRACT (Plan 04): TurnRegistry replaces AtomicBool as the running-turn tracker.
///
/// Phase 36.1 GW-05-TUI-1 asserted that app.agent_running was a live Arc (not a snapshot).
/// Plan 04 replaces that field with app.turn_registry. This test verifies the equivalent
/// property for the new type: all clones of an Arc<TurnRegistry> share the same inner map.
#[tokio::test]
async fn test_registry_is_shared_across_clones() {
    let registry = Arc::new(TurnRegistry::new());
    let clone = registry.clone();

    let turn_id = TurnId::new_v4();
    let token = CancellationToken::new();
    let entry = TurnEntry {
        turn_id,
        session_id: "session-a".to_string(),
        surface: Surface::Cli,
        started_at: std::time::Instant::now(),
        cancel: token,
    };

    // Register via original — must be visible via clone (shared inner map).
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

// ── New-contract test 2: deregister on task completion ───────────────────────

/// NEW CONTRACT (Plan 04): TurnRegistry.deregister() is called on all turn exit paths.
///
/// Phase 36.1 GW-05-TUI-2 asserted RunningAgentGuard RAII clears the flag on Drop.
/// Plan 04 replaces this with an explicit deregister call at the end of the spawned
/// task. This test verifies the deregister call removes the entry from the registry.
#[tokio::test]
async fn test_deregister_clears_entry_on_turn_end() {
    let registry = Arc::new(TurnRegistry::new());
    let registry_for_task = registry.clone();
    let session_id = "session-deregister";

    let turn_id = TurnId::new_v4();
    let token = CancellationToken::new();
    let entry = TurnEntry {
        turn_id,
        session_id: session_id.to_string(),
        surface: Surface::Cli,
        started_at: std::time::Instant::now(),
        cancel: token,
    };

    // Register before spawning (register-before-spawn discipline).
    registry.register(entry).await;
    assert_eq!(
        registry.count_session(session_id).await,
        1,
        "turn must be registered before spawn"
    );

    // Simulate the task completing: deregister in the task body.
    let handle = tokio::spawn(async move {
        // Simulate agent work.
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        // Deregister on completion (what event_loop.rs spawn_turn does).
        registry_for_task.deregister(turn_id).await;
    });

    handle.await.expect("spawned task must not panic");

    // After the future completes, the entry must be gone.
    assert_eq!(
        registry.count_session(session_id).await,
        0,
        "turn must be deregistered after task completes"
    );
}

// ── New-contract test 3: /model resolves — no rejection path ─────────────────

/// NEW CONTRACT (Plan 04): /model resolves to a command def with no rejection.
///
/// Phase 36.1 GW-05-TUI-3 asserted that /model was REJECTED (AGENT_RUNNING_REJECT_MSG)
/// when agent_running was true. Plan 04 removes the gate — /model now always resolves.
/// This test verifies that /model resolves to the "model" command def regardless of
/// how many turns are in the registry.
#[tokio::test]
async fn test_model_resolves_regardless_of_in_flight_turns() {
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    let registry = Arc::new(TurnRegistry::new());

    // Register a stub turn (simulating an in-flight CLI turn).
    let token = CancellationToken::new();
    let turn_id = TurnId::new_v4();
    registry
        .register(TurnEntry {
            turn_id,
            session_id: "model-test".to_string(),
            surface: Surface::Cli,
            started_at: std::time::Instant::now(),
            cancel: token,
        })
        .await;

    // /model must resolve — the gate is gone, no rejection based on registry contents.
    let router = CommandRouter::new(build_registry());
    let resolved = router.resolve("/model gpt-4", &Platform::Local);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            assert_eq!(
                def.name, "model",
                "/model must resolve to canonical 'model' def (gate removed in Plan 04)"
            );
            // OLD behavior was: check is_bypass("model") → false → reject.
            // NEW behavior: no bypass check, no rejection — the def is returned directly.
        }
        other => panic!(
            "/model must resolve to a known command (gate removed — no rejection), got: {:?}",
            other
        ),
    }
}

// ── New-contract test 4: /stop resolves and cancel_session covers all turns ──

/// NEW CONTRACT (Plan 04): /stop → cancel_session() cancels all session turns.
///
/// Phase 36.1 GW-05-TUI-4 asserted is_bypass("stop") == true (so /stop bypasses the gate).
/// Plan 04 removes the gate entirely, so the bypass list is irrelevant in the TUI dispatch.
/// The new /stop behavior is: after clearing the queue, call turn_registry.cancel_session().
/// This test verifies cancel_session cancels all registered session turns.
#[tokio::test]
async fn test_stop_cancels_all_session_turns_via_registry() {
    let registry = Arc::new(TurnRegistry::new());
    let session_id = "stop-test-session";

    let token_a = CancellationToken::new();
    let token_b = CancellationToken::new();
    let id_a = TurnId::new_v4();
    let id_b = TurnId::new_v4();

    registry
        .register(TurnEntry {
            turn_id: id_a,
            session_id: session_id.to_string(),
            surface: Surface::Cli,
            started_at: std::time::Instant::now(),
            cancel: token_a.clone(),
        })
        .await;
    registry
        .register(TurnEntry {
            turn_id: id_b,
            session_id: session_id.to_string(),
            surface: Surface::Cli,
            started_at: std::time::Instant::now(),
            cancel: token_b.clone(),
        })
        .await;

    // Simulate /stop arm: turn_registry.cancel_session(&app.session_id).await
    let cancelled = registry.cancel_session(session_id).await;

    assert_eq!(
        cancelled, 2,
        "/stop cancel_session must return 2 (both turns cancelled)"
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

// ── New-contract test 5: /reset alias still resolves to "new" ─────────────────

/// NEW CONTRACT (Plan 04): /reset alias resolution is unchanged.
///
/// Phase 36.1 GW-05-TUI-5 asserted /reset resolves to canonical "new" (Pitfall 4).
/// This property is unchanged — the CommandRouter alias resolution is independent of
/// the gate removal. This test verifies /reset still resolves to "new".
#[test]
fn test_reset_alias_resolves_to_new() {
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    let router = CommandRouter::new(build_registry());
    let resolved = router.resolve("/reset", &Platform::Local);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            assert_eq!(
                def.name, "new",
                "/reset must still resolve to canonical 'new' via CommandRouter alias resolution"
            );
            // With gate removed: the canonical name is used for routing to handle_session_control
            // "new" arm — alias resolution is load-bearing for correct /reset behavior.
        }
        other => panic!(
            "/reset must resolve to canonical 'new' (Pitfall 4 — unchanged by Plan 04), got: {:?}",
            other
        ),
    }
}
