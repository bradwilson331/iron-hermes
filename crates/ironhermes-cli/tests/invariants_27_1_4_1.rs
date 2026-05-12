//! Phase 27.1.4.1 static-grep regression gates.
//! Locks fallback wiring at the CLI batch runner (PROV-07 gap closure).
//! Follows `include_str!` pattern from invariants_22_4.rs. No dev-deps.

const BATCH_RUNNER_SOURCE: &str = include_str!("../src/batch/runner.rs");

#[test]
fn batch_runner_wires_fallback_prov07() {
    assert!(
        BATCH_RUNNER_SOURCE.contains(".with_fallback("),
        "PROV-07: crates/ironhermes-cli/src/batch/runner.rs must chain \
         .with_fallback() on the AgentLoop so provider fallback \
         fires on primary model failure. See phase 27.1.4.1."
    );
}
