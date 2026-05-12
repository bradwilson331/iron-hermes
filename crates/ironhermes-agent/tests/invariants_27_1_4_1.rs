//! Phase 27.1.4.1 static-grep regression gates.
//! Locks fallback wiring at the subagent runner (PROV-07 gap closure).
//! Follows `include_str!` pattern from invariants_22_4.rs. No dev-deps.

const SUBAGENT_SOURCE: &str = include_str!("../src/subagent_runner.rs");

#[test]
fn subagent_runner_wires_fallback_prov07() {
    assert!(
        SUBAGENT_SOURCE.contains(".with_fallback("),
        "PROV-07: crates/ironhermes-agent/src/subagent_runner.rs must chain \
         .with_fallback() on the child AgentLoop so provider fallback \
         fires on primary model failure. See phase 27.1.4.1."
    );
}
