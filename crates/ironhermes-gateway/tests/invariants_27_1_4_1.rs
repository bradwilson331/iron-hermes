//! Phase 27.1.4.1 static-grep regression gates.
//! Locks fallback wiring at the cron runner (PROV-07 gap closure).
//! Follows `include_str!` pattern from invariants_22_4.rs. No dev-deps.

const RUNNER_SOURCE: &str = include_str!("../src/runner.rs");

#[test]
fn cron_runner_wires_fallback_prov07() {
    assert!(
        RUNNER_SOURCE.contains("wire_fallback_if_configured(agent"),
        "PROV-07: crates/ironhermes-gateway/src/runner.rs must pass the cron \
         AgentLoop through wire_fallback_if_configured(agent, ...) so provider \
         fallback fires on primary model failure. See phase 27.1.4.1."
    );
}
