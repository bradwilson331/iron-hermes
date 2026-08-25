//! Phase 39.1 Wave 0 test scaffold: concurrency module integration tests.
//!
//! Covers R39.1-04 (global ceiling), R39.1-05 (per-turn CancellationToken registry +
//! cancel-one / cancel-session), R39.1-09 (process-wide TurnRegistry + /agents).
//!
//! These tests operate purely on the registry and layer as data structures — no
//! AgentRuntime, no sleeping mock processes, no spawned tasks required at this layer.
//! Surface plans (02/03/04/07) add their own end-to-end fixtures.

use ironhermes_core::concurrency::{ConcurrencyLayer, Surface, TurnEntry, TurnId, TurnRegistry};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Inline test helper: build a TurnEntry with minimal fields
// ---------------------------------------------------------------------------

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
        started_at: Instant::now(),
        cancel,
    }
}

// ---------------------------------------------------------------------------
// registry_list_all (R39.1-09)
// ---------------------------------------------------------------------------

/// Register 3 turns across 2 sessions + 2 surfaces → list_all returns 3 summaries;
/// list_session(s1) returns only s1's turns.
#[tokio::test]
async fn registry_list_all() {
    let registry = TurnRegistry::new();

    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    let id3 = Uuid::new_v4();

    registry
        .register(make_entry(
            id1,
            "session-1",
            Surface::Web,
            CancellationToken::new(),
        ))
        .await;
    registry
        .register(make_entry(
            id2,
            "session-1",
            Surface::Gateway,
            CancellationToken::new(),
        ))
        .await;
    registry
        .register(make_entry(
            id3,
            "session-2",
            Surface::Cli,
            CancellationToken::new(),
        ))
        .await;

    // list_all returns all 3 turns
    let all = registry.list_all().await;
    assert_eq!(all.len(), 3, "Expected 3 summaries from list_all");

    // list_session("session-1") returns exactly 2 turns
    let s1 = registry.list_session("session-1").await;
    assert_eq!(s1.len(), 2, "Expected 2 summaries for session-1");
    let s1_ids: Vec<TurnId> = s1.iter().map(|t| t.turn_id).collect();
    assert!(s1_ids.contains(&id1), "id1 should be in session-1 list");
    assert!(s1_ids.contains(&id2), "id2 should be in session-1 list");

    // list_session("session-2") returns exactly 1 turn
    let s2 = registry.list_session("session-2").await;
    assert_eq!(s2.len(), 1, "Expected 1 summary for session-2");
    assert_eq!(s2[0].turn_id, id3);
    assert_eq!(s2[0].surface, Surface::Cli);

    // count_session is consistent with list_session
    assert_eq!(registry.count_session("session-1").await, 2);
    assert_eq!(registry.count_session("session-2").await, 1);
    assert_eq!(registry.count_session("session-unknown").await, 0);
}

// ---------------------------------------------------------------------------
// turn_cancellation (R39.1-05)
// ---------------------------------------------------------------------------

/// register a turn with a CancellationToken; cancel_one(id) → token.is_cancelled() true
/// and entry still present until deregister; cancel_session(s) cancels all of session s
/// and returns the count.
#[tokio::test]
async fn turn_cancellation() {
    let registry = TurnRegistry::new();

    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    let id3 = Uuid::new_v4();

    let token1 = CancellationToken::new();
    let token2 = CancellationToken::new();
    let token3 = CancellationToken::new();

    registry
        .register(make_entry(id1, "sess-a", Surface::Web, token1.clone()))
        .await;
    registry
        .register(make_entry(id2, "sess-a", Surface::Cli, token2.clone()))
        .await;
    registry
        .register(make_entry(id3, "sess-b", Surface::Gateway, token3.clone()))
        .await;

    // cancel_one on id1 → token1 is cancelled, id1 still in registry
    let cancelled = registry.cancel_one(id1).await;
    assert!(cancelled, "cancel_one should return true for a live entry");
    assert!(
        token1.is_cancelled(),
        "token1 should be cancelled after cancel_one(id1)"
    );
    assert!(!token2.is_cancelled(), "token2 should NOT be cancelled");
    assert!(!token3.is_cancelled(), "token3 should NOT be cancelled");

    // Entry is still present (task hasn't called deregister yet)
    assert_eq!(registry.count_session("sess-a").await, 2);

    // cancel_one on a non-existent id returns false
    let missing = registry.cancel_one(Uuid::new_v4()).await;
    assert!(!missing, "cancel_one for unknown id should return false");

    // cancel_session("sess-a") cancels id2 (id1's token already cancelled); returns 2
    // (both tokens are signalled — it's idempotent to call cancel() again on id1's token)
    let count = registry.cancel_session("sess-a").await;
    assert_eq!(count, 2, "cancel_session should return 2 for sess-a");
    assert!(token2.is_cancelled(), "token2 should now be cancelled");
    // sess-b untouched
    assert!(
        !token3.is_cancelled(),
        "token3 (sess-b) should NOT be cancelled"
    );

    // Deregister all and verify count drops
    registry.deregister(id1).await;
    registry.deregister(id2).await;
    assert_eq!(registry.count_session("sess-a").await, 0);
    assert_eq!(registry.count_session("sess-b").await, 1);
    registry.deregister(id3).await;
    assert_eq!(registry.list_all().await.len(), 0);
}

// ---------------------------------------------------------------------------
// global_ceiling (R39.1-04)
// ---------------------------------------------------------------------------

/// ConcurrencyLayer::new(session_cap=10, global_ceiling=2) → first 2 try_acquire return
/// Some, 3rd returns None (global exhausted); dropping a permit pair lets the next
/// try_acquire return Some again (RAII release).
#[tokio::test]
async fn global_ceiling() {
    let layer = ConcurrencyLayer::new(10, 2); // per-session=10 (headroom), global=2

    let pair1 = layer.try_acquire();
    assert!(pair1.is_some(), "1st acquire should succeed");

    let pair2 = layer.try_acquire();
    assert!(pair2.is_some(), "2nd acquire should succeed");

    // Global ceiling exhausted — 3rd must fail
    let pair3 = layer.try_acquire();
    assert!(
        pair3.is_none(),
        "3rd acquire should fail (global ceiling=2)"
    );

    // Drop one pair → RAII releases one global permit
    drop(pair1);

    // Now try_acquire should succeed again
    let pair4 = layer.try_acquire();
    assert!(
        pair4.is_some(),
        "4th acquire should succeed after RAII release"
    );

    // Clean up
    drop(pair2);
    drop(pair4);
}

// ---------------------------------------------------------------------------
// per_session_cap (R39.1-03)
// ---------------------------------------------------------------------------

/// ConcurrencyLayer::new(session_cap=2, global_ceiling=10) → 3rd per-session
/// acquire returns None.
#[tokio::test]
async fn per_session_cap() {
    let layer = ConcurrencyLayer::new(2, 10); // per-session=2, global=10 (headroom)

    let pair1 = layer.try_acquire();
    assert!(pair1.is_some(), "1st acquire should succeed");

    let pair2 = layer.try_acquire();
    assert!(pair2.is_some(), "2nd acquire should succeed");

    // Per-session cap exhausted — 3rd must fail
    let pair3 = layer.try_acquire();
    assert!(pair3.is_none(), "3rd acquire should fail (session_cap=2)");

    // Drop one pair → RAII releases the per-session permit
    drop(pair1);

    // Next acquire should succeed again
    let pair4 = layer.try_acquire();
    assert!(
        pair4.is_some(),
        "4th acquire should succeed after RAII release"
    );

    drop(pair2);
    drop(pair4);
}

// ---------------------------------------------------------------------------
// api_server_surface_renders_its_own_name (Phase 36.7.1 Plan 06 Task 1)
// ---------------------------------------------------------------------------

/// The REST API server's `Surface` variant renders a display string distinct
/// from all four pre-existing surfaces.
#[test]
fn api_server_surface_renders_its_own_name() {
    let names: Vec<String> = [
        Surface::Web,
        Surface::Gateway,
        Surface::Cli,
        Surface::Realtime,
        Surface::ApiServer,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        names.len(),
        "every Surface variant must render a distinct display string: {names:?}"
    );
    assert_eq!(Surface::ApiServer.to_string(), "api_server");
}

// ---------------------------------------------------------------------------
// get_one (Phase 36.7.1 Plan 06 Task 1)
// ---------------------------------------------------------------------------

/// Looking up a registered identifier yields a summary carrying the same
/// identifier, session and surface, with a non-negative elapsed measurement.
#[tokio::test]
async fn get_one_returns_a_summary_for_a_registered_turn() {
    let registry = TurnRegistry::new();
    let turn_id = Uuid::new_v4();
    registry
        .register(make_entry(
            turn_id,
            "session-get-one",
            Surface::ApiServer,
            CancellationToken::new(),
        ))
        .await;

    let summary = registry
        .get_one(turn_id)
        .await
        .expect("registered turn must be found");

    // Destructuring EVERY field with no `..` is a compile-time proof that
    // `TurnSummary` carries exactly these four fields — in particular, no
    // `cancel`/token field. If a token field were ever added to
    // `TurnSummary`, this destructure would fail to compile, catching the
    // D-08/T-36.7.1-46-style regression at build time rather than at review
    // time (`get_one_does_not_expose_the_cancellation_token`).
    let ironhermes_core::concurrency::TurnSummary {
        turn_id: got_id,
        session_id,
        surface,
        elapsed_ms: _,
    } = summary;
    assert_eq!(got_id, turn_id);
    assert_eq!(session_id, "session-get-one");
    assert_eq!(surface, Surface::ApiServer);
}

/// An identifier that was never registered, or was deregistered, yields
/// nothing rather than a default-valued summary.
#[tokio::test]
async fn get_one_returns_nothing_for_an_unknown_id() {
    let registry = TurnRegistry::new();

    // Never registered.
    assert!(registry.get_one(Uuid::new_v4()).await.is_none());

    // Registered then deregistered.
    let turn_id = Uuid::new_v4();
    registry
        .register(make_entry(
            turn_id,
            "session-deregistered",
            Surface::ApiServer,
            CancellationToken::new(),
        ))
        .await;
    assert!(registry.get_one(turn_id).await.is_some());
    registry.deregister(turn_id).await;
    assert!(
        registry.get_one(turn_id).await.is_none(),
        "a deregistered turn must not still be found"
    );
}

/// `TurnSummary` is the wire-safe projection with no cancellation-token
/// field — see the destructure in `get_one_returns_a_summary_for_a_registered_turn`
/// for the compile-time proof this test's name promises.
#[tokio::test]
async fn get_one_does_not_expose_the_cancellation_token() {
    let registry = TurnRegistry::new();
    let turn_id = Uuid::new_v4();
    registry
        .register(make_entry(
            turn_id,
            "session-no-token",
            Surface::ApiServer,
            CancellationToken::new(),
        ))
        .await;
    let summary = registry.get_one(turn_id).await.expect("must be found");
    // `TurnSummary` is `Clone + Debug + PartialEq + Eq` and holds no token —
    // a `CancellationToken` is neither `PartialEq` nor `Eq`, so this
    // reconstruction-and-compare only compiles because the type is token-free.
    assert_eq!(summary.clone(), summary);
}

/// Turns registered under all five surfaces are listed together and each
/// reports its own surface.
#[tokio::test]
async fn surfaces_remain_distinguishable_in_a_mixed_listing() {
    let registry = TurnRegistry::new();
    let surfaces = [
        Surface::Web,
        Surface::Gateway,
        Surface::Cli,
        Surface::Realtime,
        Surface::ApiServer,
    ];
    let mut ids = Vec::new();
    for (i, surface) in surfaces.iter().enumerate() {
        let turn_id = Uuid::new_v4();
        ids.push((turn_id, *surface));
        registry
            .register(make_entry(
                turn_id,
                &format!("session-mixed-{i}"),
                *surface,
                CancellationToken::new(),
            ))
            .await;
    }

    let all = registry.list_all().await;
    assert_eq!(all.len(), surfaces.len());
    for (turn_id, surface) in ids {
        let found = all
            .iter()
            .find(|s| s.turn_id == turn_id)
            .unwrap_or_else(|| panic!("turn {turn_id} should be listed"));
        assert_eq!(found.surface, surface);

        let single = registry
            .get_one(turn_id)
            .await
            .unwrap_or_else(|| panic!("get_one({turn_id}) should find it"));
        assert_eq!(single.surface, surface);
    }
}

// ---------------------------------------------------------------------------
// agents_list_command (R39.1-09, optional integration)
// ---------------------------------------------------------------------------

/// A CommandContext with an injected registry holding 2 turns dispatches
/// `/agents turns` → CommandResult::AgentsList(len==2).
///
/// Uses multi-thread flavor because cmd_agents's `turns` arm uses
/// `tokio::task::block_in_place` which requires a multi-threaded runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_list_command() {
    use ironhermes_core::TurnRegistry;
    use ironhermes_core::commands::context::CommandContext;
    use ironhermes_core::commands::handlers::dispatch;
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandResult, CommandRouter};
    use ironhermes_core::types::Platform;
    use std::sync::Arc;

    let registry = Arc::new(TurnRegistry::new());

    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();

    registry
        .register(make_entry(
            id1,
            "sess-cmd",
            Surface::Web,
            CancellationToken::new(),
        ))
        .await;
    registry
        .register(make_entry(
            id2,
            "sess-cmd",
            Surface::Cli,
            CancellationToken::new(),
        ))
        .await;

    let ctx = CommandContext::new(Platform::Local, "sess-cmd".to_string())
        .with_turn_registry(registry.clone());

    let router = CommandRouter::new(build_registry());
    let cmd_def = router
        .commands
        .iter()
        .find(|c| c.name == "agents")
        .cloned()
        .expect("agents command should be registered");

    // Dispatch /agents turns — block_in_place is used inside cmd_agents
    let result = tokio::task::block_in_place(|| dispatch(&cmd_def, &["turns"], &ctx, &router));

    match result {
        CommandResult::AgentsList(summaries) => {
            assert_eq!(summaries.len(), 2, "Expected 2 turn summaries");
            let ids: Vec<TurnId> = summaries.iter().map(|s| s.turn_id).collect();
            assert!(ids.contains(&id1));
            assert!(ids.contains(&id2));
        }
        other => panic!("Expected AgentsList, got: {:?}", other),
    }
}
