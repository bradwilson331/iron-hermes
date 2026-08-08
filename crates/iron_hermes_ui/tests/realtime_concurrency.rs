//! Phase 39.1 Plan 07 integration tests — realtime concurrency control (R39.1-10).
//!
//! Exercises two behaviours introduced in Plan 07:
//!
//! 1. `realtime_acquire_refused_at_ceiling` — `ConcurrencyLayer::try_acquire` returns
//!    `None` when the global ceiling is reached, and `Some` again after a permit is
//!    dropped (the RAII guarantee that keeps the cap tight).
//!
//! 2. `realtime_cancel_maps_to_cancelled_signal` — registering a `Surface::Realtime`
//!    turn in the `TurnRegistry`, checking `get_cancel_token` returns `Continue`-
//!    equivalent (token not yet cancelled), calling `cancel_one`, verifying the token
//!    is now cancelled, then deregistering and confirming `list_all` is empty.
//!
//! ## Why no import from `iron_hermes_ui`
//!
//! `iron_hermes_ui` is a binary crate (no `[lib]` target), so integration tests
//! cannot `use iron_hermes_ui::...`.  The types under test (`ConcurrencyLayer`,
//! `TurnRegistry`, `TurnEntry`, `Surface`) all live in `ironhermes_core` and are
//! directly importable here.

use ironhermes_core::{ConcurrencyLayer, Surface, TurnEntry, TurnId, TurnRegistry};

// ── Test 1: ConcurrencyLayer ceiling ─────────────────────────────────────────

/// `try_acquire` refuses a third concurrent permit when the global ceiling is 2,
/// and grants it again after one existing permit is dropped.
///
/// Covers R39.1-10 "global ceiling" at token-mint time — the same code path that
/// `issue_realtime_token` exercises in production.
#[tokio::test]
async fn realtime_acquire_refused_at_ceiling() {
    // session_cap=10 (generous), global_ceiling=2.
    let layer = ConcurrencyLayer::new(10, 2);

    // Acquire first two permits — both must succeed.
    let p1 = layer.try_acquire();
    assert!(p1.is_some(), "first acquire should succeed (below ceiling)");

    let p2 = layer.try_acquire();
    assert!(p2.is_some(), "second acquire should succeed (at ceiling)");

    // Third acquire must be refused (global ceiling reached).
    let p3 = layer.try_acquire();
    assert!(
        p3.is_none(),
        "third acquire must be refused at global ceiling=2"
    );

    // Drop one permit — ceiling opens by one.
    drop(p1);

    // Now a new acquire must succeed again.
    let p4 = layer.try_acquire();
    assert!(
        p4.is_some(),
        "acquire after permit drop must succeed (ceiling freed)"
    );

    // Cleanup — drop remaining permits (implicit, but explicit for clarity).
    drop(p2);
    drop(p4);
}

// ── Test 2: TurnRegistry cancel → Cancelled signal ───────────────────────────

/// Registering a `Surface::Realtime` turn, confirming `get_cancel_token` returns
/// the live token (Continue-equivalent — not yet cancelled), calling `cancel_one`,
/// confirming the token is now cancelled, deregistering, and verifying `list_all`
/// is empty.
///
/// Covers R39.1-10 "cooperative cancel": `turn_cancel` calls `cancel_one` in
/// production; `realtime_heartbeat` maps `is_cancelled()` to `RealtimeTurnSignal`.
#[tokio::test]
async fn realtime_cancel_maps_to_cancelled_signal() {
    let registry = TurnRegistry::new();
    let turn_id: TurnId = TurnId::new_v4();
    let cancel = tokio_util::sync::CancellationToken::new();

    // Register a Surface::Realtime turn (register-before-handoff discipline).
    registry
        .register(TurnEntry {
            turn_id,
            session_id: "test-session".to_string(),
            surface: Surface::Realtime,
            started_at: std::time::Instant::now(),
            cancel: cancel.clone(),
        })
        .await;

    // Before cancel: get_cancel_token returns Some with is_cancelled() == false
    // (Continue-equivalent — the heartbeat would return RealtimeTurnSignal::Continue).
    let token = registry
        .get_cancel_token(turn_id)
        .await
        .expect("token must be present after register");
    assert!(
        !token.is_cancelled(),
        "token must NOT be cancelled before cancel_one is called"
    );

    // Cancel the turn via the registry (mirrors turn_cancel server fn).
    let found = registry.cancel_one(turn_id).await;
    assert!(found, "cancel_one must return true for a registered turn");

    // After cancel: get_cancel_token returns Some with is_cancelled() == true
    // (Cancelled-equivalent — heartbeat would return RealtimeTurnSignal::Cancelled).
    let token_after = registry
        .get_cancel_token(turn_id)
        .await
        .expect("token must still be present — deregister not called yet");
    assert!(
        token_after.is_cancelled(),
        "token must be cancelled after cancel_one"
    );

    // Deregister (mirrors realtime_session_end / teardown path).
    registry.deregister(turn_id).await;

    // list_all must now be empty.
    let remaining = registry.list_all().await;
    assert!(
        remaining.is_empty(),
        "registry must be empty after deregister; got {} entries",
        remaining.len()
    );
}
