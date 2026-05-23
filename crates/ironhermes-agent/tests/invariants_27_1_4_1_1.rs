//! Phase 27.1.4.1.1 static-grep regression gates.
//! Locks transport-failure detection in classify_llm_error (PROV-07 extension).
//! Follows `include_str!` pattern from invariants_22_4.rs. No dev-deps.

const AGENT_LOOP_SOURCE: &str = include_str!("../src/agent_loop.rs");

#[test]
fn agent_loop_has_transport_failure_helper_prov07() {
    assert!(
        AGENT_LOOP_SOURCE.contains("fn is_transport_failure("),
        "PROV-07: crates/ironhermes-agent/src/agent_loop.rs must contain a \
         transport-failure detection helper (is_transport_failure) so \
         classify_llm_error triggers fallback on connection refused / DNS / \
         timeout errors. See phase 27.1.4.1.1."
    );
}

#[test]
fn agent_loop_locks_reqwest_error_sending_marker_prov07() {
    assert!(
        AGENT_LOOP_SOURCE.contains("error sending request for url"),
        "PROV-07: crates/ironhermes-agent/src/agent_loop.rs must contain the \
         literal \"error sending request for url\" in the transport-failure \
         allowlist so the reqwest Kind::Request marker cannot be silently \
         dropped by refactoring. See phase 27.1.4.1.1."
    );
}
