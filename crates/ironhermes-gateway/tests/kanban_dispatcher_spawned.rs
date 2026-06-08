//! Phase 36.3.7 Plan 08: kanban dispatcher gateway-embed tests.
//!
//! Tests the spawn predicate (D-09) and static-grep invariants for the
//! gateway-embedded dispatcher. Full end-to-end gateway-start tests
//! are deferred to plan 09 (heavyweight: require transport mocks + full
//! tokio runtime). These tests focus on the predicate decision and
//! structural correctness of the embed.
//!
//! Three test categories:
//!
//! 1. **Predicate unit tests** — validate the `dispatch_in_gateway &&
//!    dispatch_in_gw_env` boolean logic using `KanbanConfig` directly.
//! 2. **Static-grep invariants** — assert that the spawn call, env-override
//!    check, and startup log are present in runner.rs (regression gates).
//! 3. **Gateway-embed invariant** — assert runner.rs calls
//!    `run_dispatch_loop` inside a `join_set.spawn` block (D-09 contract).

// ---------------------------------------------------------------------------
// Source constants for static-grep invariants
// ---------------------------------------------------------------------------

const RUNNER_SOURCE: &str = include_str!("../src/runner.rs");

// ---------------------------------------------------------------------------
// 1. Predicate unit tests
// ---------------------------------------------------------------------------

/// Default KanbanConfig has `dispatch_in_gateway = true`.
/// With no env override, the predicate must be true.
#[test]
fn kanban_dispatcher_spawn_predicate_default_true() {
    // Simulate the runner's predicate: cfg.dispatch_in_gateway && env_ok
    let cfg = ironhermes_kanban::KanbanConfig::default();
    assert!(
        cfg.dispatch_in_gateway,
        "KanbanConfig::default() must have dispatch_in_gateway = true (D-09)"
    );

    // Simulate env_ok when HERMES_KANBAN_DISPATCH_IN_GATEWAY is absent
    // (as it would be in a fresh gateway startup without the env override).
    // We use a helper that mirrors the runner logic without mutating the env.
    let env_ok = dispatch_in_gw_env_predicate(None);
    assert!(
        env_ok,
        "dispatch_in_gw_env must be true when env var is absent"
    );

    assert!(
        cfg.dispatch_in_gateway && env_ok,
        "spawn predicate must be true with default config and no env override (D-09)"
    );
}

/// Setting HERMES_KANBAN_DISPATCH_IN_GATEWAY=0 must cause the env predicate to
/// return false, blocking the dispatcher spawn.
#[test]
fn kanban_dispatcher_predicate_respects_env_override() {
    let cfg = ironhermes_kanban::KanbanConfig::default();
    assert!(cfg.dispatch_in_gateway, "config still true");

    // Simulate the env override value "0"
    let env_ok = dispatch_in_gw_env_predicate(Some("0"));
    assert!(
        !env_ok,
        "HERMES_KANBAN_DISPATCH_IN_GATEWAY=0 must disable the dispatcher (D-09 env override)"
    );

    assert!(
        !(cfg.dispatch_in_gateway && env_ok),
        "spawn predicate must be false when HERMES_KANBAN_DISPATCH_IN_GATEWAY=0"
    );
}

/// Setting config.kanban.dispatch_in_gateway = false must disable the spawn
/// regardless of the env var.
#[test]
fn kanban_dispatcher_predicate_respects_config_disable() {
    let cfg = ironhermes_kanban::KanbanConfig {
        dispatch_in_gateway: false,
        ..ironhermes_kanban::KanbanConfig::default()
    };
    assert!(!cfg.dispatch_in_gateway, "config disabled");

    // Even with no env override, config=false wins
    let env_ok = dispatch_in_gw_env_predicate(None);
    assert!(
        !(cfg.dispatch_in_gateway && env_ok),
        "spawn predicate must be false when config.dispatch_in_gateway = false"
    );
}

/// Non-"0" values of HERMES_KANBAN_DISPATCH_IN_GATEWAY (e.g. "1", "true")
/// must NOT disable the dispatcher — only the exact string "0" suppresses it.
#[test]
fn kanban_dispatcher_env_non_zero_does_not_suppress() {
    for val in &["1", "true", "yes", "false", "off"] {
        let env_ok = dispatch_in_gw_env_predicate(Some(val));
        assert!(
            env_ok,
            "HERMES_KANBAN_DISPATCH_IN_GATEWAY={val:?} must NOT suppress the dispatcher \
             (only \"0\" is the disable signal, per D-09)"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Static-grep invariants — runner.rs structural gates
// ---------------------------------------------------------------------------

/// INV-36.3.7-08-01: runner.rs must call `run_dispatch_loop` so the
/// gateway-embedded dispatcher is actually started (D-09 contract).
#[test]
fn gateway_runner_calls_run_dispatch_loop() {
    assert!(
        RUNNER_SOURCE.contains("run_dispatch_loop"),
        "INV-36.3.7-08-01: runner.rs must call ironhermes_kanban::run_dispatch_loop \
         (D-09: gateway-embedded dispatcher). If this fails, the kanban dispatcher \
         is not wired into the gateway runtime."
    );
}

/// INV-36.3.7-08-02: runner.rs must contain the startup log line so operators
/// can confirm the dispatcher is running (plan 08 must_have truth #5).
#[test]
fn gateway_runner_logs_kanban_dispatch_started() {
    assert!(
        RUNNER_SOURCE.contains("Kanban dispatch task started"),
        "INV-36.3.7-08-02: runner.rs must emit 'Kanban dispatch task started (…s interval)' \
         so operators can confirm the dispatcher is active in gateway logs (plan 08 must_have truth #5)."
    );
}

/// INV-36.3.7-08-03: runner.rs must check `HERMES_KANBAN_DISPATCH_IN_GATEWAY`
/// env override so operators can disable the dispatcher without editing config
/// (D-09 env override, plan 08 must_have truth #2).
#[test]
fn gateway_runner_respects_dispatch_in_gateway_env() {
    assert!(
        RUNNER_SOURCE.contains("HERMES_KANBAN_DISPATCH_IN_GATEWAY"),
        "INV-36.3.7-08-03: runner.rs must check HERMES_KANBAN_DISPATCH_IN_GATEWAY env \
         override (D-09). Without this check, the dispatcher cannot be disabled without \
         editing config.yaml."
    );
}

/// INV-36.3.7-08-04: the dispatcher spawn uses the gateway's CancellationToken
/// clone — ensures clean shutdown propagation (plan 08 must_have truth #3).
#[test]
fn gateway_runner_dispatcher_uses_cancellation_token() {
    assert!(
        RUNNER_SOURCE.contains("kanban_cancel"),
        "INV-36.3.7-08-04: runner.rs must clone self.cancel into kanban_cancel and pass \
         it to run_dispatch_loop so shutdown propagates cleanly (plan 08 must_have truth #3)."
    );
}

/// INV-36.3.7-08-05: kanban.db open failure must be non-fatal — the gateway
/// must log a warning and continue without the dispatcher (T-36.3.7-08-02).
#[test]
fn gateway_runner_kanban_db_failure_is_non_fatal() {
    // The non-fatal path logs "will NOT start" rather than returning an error.
    assert!(
        RUNNER_SOURCE.contains("kanban dispatcher will NOT start"),
        "INV-36.3.7-08-05: runner.rs must handle KanbanStore::open_default() failure by \
         logging a warning and continuing (T-36.3.7-08-02). A failed kanban.db open must \
         NOT prevent the gateway from starting."
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Mirror of the runner's `dispatch_in_gw_env` predicate, accepting an
/// optional value string instead of reading the real env (avoids env mutation
/// in tests).
fn dispatch_in_gw_env_predicate(env_val: Option<&str>) -> bool {
    match env_val {
        Some(v) => v != "0",
        None => true, // env var absent → default true
    }
}
