//! Phase 27.1.4.1 static-grep regression gates.
//! Locks fallback wiring at the cron runner (PROV-07 gap closure).
//! Follows `include_str!` pattern from invariants_22_4.rs. No dev-deps.
//!
//! Plan 32.1-07: cron execution moved from ironhermes-gateway to
//! ironhermes-cron-runner. The PROV-07 gate now checks the cron-runner's
//! runner.rs where wire_fallback_if_configured is called.

const CRON_RUNNER_SOURCE: &str =
    include_str!("../../../crates/ironhermes-cron-runner/src/runner.rs");

#[test]
fn cron_runner_wires_fallback_prov07() {
    assert!(
        CRON_RUNNER_SOURCE.contains("wire_fallback_if_configured(agent"),
        "PROV-07: crates/ironhermes-cron-runner/src/runner.rs must pass the cron \
         AgentLoop through wire_fallback_if_configured(agent, ...) so provider \
         fallback fires on primary model failure. See phase 27.1.4.1. \
         (Plan 32.1-07: execution moved from ironhermes-gateway to ironhermes-cron-runner)"
    );
}
