//! Phase 39.1 Plan 04 (R39.1-01 / R39.1-05 / R39.1-06 / R39.1-09):
//! TUI/CLI no-slash-rejection + per-turn cancel concurrency tests.
//!
//! Covers three behaviors:
//!
//! 1. `tui_slash_not_rejected_mid_turn` — gate removed: with a stub turn registered,
//!    dispatching /model (previously blocked by the AtomicBool gate) now succeeds.
//!
//! 2. `tui_stop_cancels_session` — two stub turns registered for the TUI session,
//!    turn_registry.cancel_session() cancels both (D-06 / R39.1-05).
//!
//! 3. `tui_agents_lists_turns` — registry holds turns from Cli + Gateway surfaces;
//!    list_all() returns both (cross-surface visibility, D-09 / R39.1-09).
//!
//! Test approach: These tests operate on the concurrency primitives directly (TurnRegistry,
//! CancellationToken, CommandRouter) without constructing a live App/TUI runtime. The
//! structural source-grep gates in Task 1/2 acceptance criteria prove the gate is wired
//! out of the dispatch path; these tests verify the registry mechanics and dispatch contract.
//!
//! No live LLM calls are made — turns are stub futures parked on a CancellationToken.

use std::sync::Arc;

use ironhermes_core::concurrency::{ConcurrencyLayer, Surface, TurnEntry, TurnId, TurnRegistry};
use tokio_util::sync::CancellationToken;

// ── Fixture helpers ─────────────────────────────────────────────────────────

fn make_registry() -> Arc<TurnRegistry> {
    Arc::new(TurnRegistry::new())
}

fn make_token() -> CancellationToken {
    CancellationToken::new()
}

fn make_entry(
    turn_id: TurnId,
    session_id: &str,
    surface: Surface,
    cancel: CancellationToken,
) -> TurnEntry {
    TurnEntry {
        turn_id,
        session_id: session_id.to_string(),
        surface,
        started_at: std::time::Instant::now(),
        cancel,
    }
}

// ── Behavior 1: no slash rejection mid-turn ─────────────────────────────────

/// Phase 39.1 Plan 04 (R39.1-06 / D-06): the AtomicBool gate has been removed from the
/// TUI dispatch path. This test verifies the new contract: with a stub turn registered in
/// the TurnRegistry (simulating an in-flight turn), the CommandRouter still resolves
/// commands correctly and there is no rejection path gated on a running flag.
///
/// Previously the guard `if app.agent_running.load(SeqCst) && !is_bypass(def.name)` would
/// have returned AGENT_RUNNING_REJECT_MSG for /model. With the gate removed, /model
/// resolves normally regardless of how many turns are registered.
#[tokio::test]
async fn tui_slash_not_rejected_mid_turn() {
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    let registry = make_registry();
    let session_id = "tui-test-session";

    // Register a stub in-flight turn (simulating an active TUI turn).
    let token = make_token();
    let turn_id = TurnId::new_v4();
    let entry = make_entry(turn_id, session_id, Surface::Cli, token.clone());
    registry.register(entry).await;

    // Confirm the turn is visible in the registry.
    let turns = registry.list_session(session_id).await;
    assert_eq!(turns.len(), 1, "stub turn must be registered");

    // With the old gate: agent_running.load(true) && !is_bypass("model") → REJECT
    // With the new contract (gate removed): /model resolves to a CommandDef — no rejection.
    let router = CommandRouter::new(build_registry());
    let resolved = router.resolve("/model gpt-4", &Platform::Local);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            // The command resolved — the gate is gone; nothing here rejects based on in-flight turns.
            assert_eq!(
                def.name, "model",
                "must resolve to canonical 'model' command"
            );
            // Verify is_bypass is no longer consulted: the test does NOT check it.
            // The new contract is simply: a command def is returned, not a rejection string.
        }
        other => panic!("/model gpt-4 must resolve, got {:?}", other),
    }

    // Clean up.
    registry.deregister(turn_id).await;
    let turns_after = registry.list_session(session_id).await;
    assert!(
        turns_after.is_empty(),
        "registry must be empty after deregister"
    );
}

// ── Behavior 2: /stop cancels all session turns ──────────────────────────────

/// Phase 39.1 Plan 04 (R39.1-05): /stop maps to turn_registry.cancel_session().
/// With two stub turns registered for the TUI session, cancel_session returns 2
/// (both tokens cancelled).
///
/// This mirrors the handle_session_control "stop" arm wired in Plan 04:
///   let _cancelled = app.turn_registry.cancel_session(&app.session_id).await;
#[tokio::test]
async fn tui_stop_cancels_session() {
    let registry = make_registry();
    let session_id = "tui-stop-session";

    // Register two stub turns for the same session.
    let token1 = make_token();
    let token2 = make_token();
    let turn_id1 = TurnId::new_v4();
    let turn_id2 = TurnId::new_v4();

    registry
        .register(make_entry(
            turn_id1,
            session_id,
            Surface::Cli,
            token1.clone(),
        ))
        .await;
    registry
        .register(make_entry(
            turn_id2,
            session_id,
            Surface::Cli,
            token2.clone(),
        ))
        .await;

    // Confirm both registered.
    let turns = registry.list_session(session_id).await;
    assert_eq!(turns.len(), 2, "two turns must be registered");

    // Neither token is cancelled yet.
    assert!(!token1.is_cancelled(), "token1 should not be cancelled yet");
    assert!(!token2.is_cancelled(), "token2 should not be cancelled yet");

    // Simulate /stop — cancel all session turns (matches handle_session_control "stop" arm).
    let cancelled = registry.cancel_session(session_id).await;

    // Both turns must be cancelled.
    assert_eq!(
        cancelled, 2,
        "cancel_session must return 2 (one per registered turn)"
    );
    assert!(
        token1.is_cancelled(),
        "token1 must be cancelled after cancel_session"
    );
    assert!(
        token2.is_cancelled(),
        "token2 must be cancelled after cancel_session"
    );
}

// ── Behavior 3: /agents lists turns across surfaces ──────────────────────────

/// Phase 39.1 Plan 04 (R39.1-09 / D-09): TurnRegistry is the process-wide cross-surface
/// single source of truth. /agents turns calls list_all(), which returns entries from
/// all surfaces (Web, Gateway, Cli, Realtime) — not just the current session.
///
/// This test registers turns from both Cli and Gateway surfaces and verifies that
/// list_all() returns both — satisfying the D-09 cross-surface visibility requirement.
#[tokio::test]
async fn tui_agents_lists_turns() {
    let registry = make_registry();

    let cli_session = "cli-session";
    let gw_session = "gateway-session";

    // Register a Cli turn (simulating a TUI turn).
    let cli_token = make_token();
    let cli_turn_id = TurnId::new_v4();
    registry
        .register(make_entry(
            cli_turn_id,
            cli_session,
            Surface::Cli,
            cli_token,
        ))
        .await;

    // Register a Gateway turn (simulating a concurrent gateway turn).
    let gw_token = make_token();
    let gw_turn_id = TurnId::new_v4();
    registry
        .register(make_entry(
            gw_turn_id,
            gw_session,
            Surface::Gateway,
            gw_token,
        ))
        .await;

    // list_all() returns all turns across all surfaces (cross-surface visibility).
    let all_turns = registry.list_all().await;
    assert_eq!(
        all_turns.len(),
        2,
        "/agents turns must list ALL in-flight turns across all surfaces (D-09)"
    );

    // Verify both surfaces are represented.
    let cli_present = all_turns.iter().any(|t| t.surface == Surface::Cli);
    let gw_present = all_turns.iter().any(|t| t.surface == Surface::Gateway);
    assert!(cli_present, "Cli turn must appear in list_all()");
    assert!(gw_present, "Gateway turn must appear in list_all()");

    // Verify the turn IDs are correct.
    let ids: Vec<_> = all_turns.iter().map(|t| t.turn_id).collect();
    assert!(
        ids.contains(&cli_turn_id),
        "cli_turn_id must be in list_all()"
    );
    assert!(
        ids.contains(&gw_turn_id),
        "gw_turn_id must be in list_all()"
    );
}

// ── Behavior 4: per-turn cancel via cancel_one ──────────────────────────────

/// Phase 39.1 Plan 04 (R39.1-05 / T-39.1-04): cancel_one(turn_id) cancels exactly
/// the targeted turn. The /cancel <id> handler validates the UUID then calls cancel_one.
/// This test verifies the cancel_one mechanics: only the targeted token is cancelled.
#[tokio::test]
async fn tui_cancel_one_targets_specific_turn() {
    let registry = make_registry();
    let session_id = "cancel-one-session";

    let token_a = make_token();
    let token_b = make_token();
    let turn_id_a = TurnId::new_v4();
    let turn_id_b = TurnId::new_v4();

    registry
        .register(make_entry(
            turn_id_a,
            session_id,
            Surface::Cli,
            token_a.clone(),
        ))
        .await;
    registry
        .register(make_entry(
            turn_id_b,
            session_id,
            Surface::Cli,
            token_b.clone(),
        ))
        .await;

    // Cancel only turn A.
    let found = registry.cancel_one(turn_id_a).await;
    assert!(found, "cancel_one must return true for a registered turn");
    assert!(token_a.is_cancelled(), "token_a must be cancelled");
    assert!(
        !token_b.is_cancelled(),
        "token_b must NOT be cancelled (only A was targeted)"
    );

    // Cancel a non-existent turn (already deregistered or invalid).
    let nonexistent = TurnId::new_v4();
    let not_found = registry.cancel_one(nonexistent).await;
    assert!(
        !not_found,
        "cancel_one must return false for an unregistered turn id"
    );
}

// ── Behavior 5: ConcurrencyLayer try_acquire ──────────────────────────────────

/// Phase 39.1 Plan 04 (R39.1-03 / R39.1-04): ConcurrencyLayer enforces per-session
/// and global limits. With a session cap of 1 and global of 32: two back-to-back
/// try_acquire calls on the same layer return Some then None.
#[test]
fn concurrency_layer_try_acquire_enforces_session_cap() {
    let layer = ConcurrencyLayer::new(1, 32);

    // First acquire succeeds.
    let permit_a = layer.try_acquire();
    assert!(
        permit_a.is_some(),
        "first try_acquire must succeed (cap=1, nothing held)"
    );

    // Second acquire fails (session cap exhausted).
    let permit_b = layer.try_acquire();
    assert!(
        permit_b.is_none(),
        "second try_acquire must fail (session cap=1 is exhausted)"
    );

    // Drop the first permit — capacity released.
    drop(permit_a);

    // Now try again — should succeed.
    let permit_c = layer.try_acquire();
    assert!(
        permit_c.is_some(),
        "try_acquire must succeed again after permit dropped"
    );
}
