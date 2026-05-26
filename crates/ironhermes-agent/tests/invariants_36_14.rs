//! Phase 36.14 static-grep regression gates.
//! Locks SSE error propagation wiring in client.rs and agent_loop.rs (PROV-07 extension).
//! Follows `include_str!` pattern from invariants_22_4.rs / invariants_27_1_4_1.rs. No dev-deps.

const CLIENT_SOURCE: &str = include_str!("../src/client.rs");
const AGENT_LOOP_SOURCE: &str = include_str!("../src/agent_loop.rs");

#[test]
fn client_has_sse_provider_error_variant_prov07() {
    assert!(
        CLIENT_SOURCE.contains("ProviderError(String)"),
        "PROV-07 (phase 36.14): crates/ironhermes-agent/src/client.rs must contain \
         the StreamEvent::ProviderError(String) variant so SSE-body errors are \
         propagated to classify_llm_error_typed and the fallback chain activates. \
         See phase 36.14-sse-stream-error-fallback-gap."
    );
}

#[test]
fn client_has_sse_error_token_prov07() {
    assert!(
        CLIENT_SOURCE.contains("SSE error"),
        "PROV-07 (phase 36.14): crates/ironhermes-agent/src/client.rs must contain \
         the literal 'SSE error' in the fallback error string so unknown-code SSE \
         errors are distinguishable in logs from HTTP-status errors. \
         See phase 36.14-sse-stream-error-fallback-gap."
    );
}

#[test]
fn agent_loop_handles_sse_provider_error_prov07() {
    assert!(
        AGENT_LOOP_SOURCE.contains("StreamEvent::ProviderError"),
        "PROV-07 (phase 36.14): crates/ironhermes-agent/src/agent_loop.rs must contain \
         a StreamEvent::ProviderError match arm in call_llm_streaming so SSE errors \
         surface as Err(...) and reach the fallback/retry block in run(). \
         See phase 36.14-sse-stream-error-fallback-gap."
    );
}

#[test]
fn agent_loop_provider_error_returns_err_anyhow_prov07() {
    // Codex LOW #5 (strengthened invariant): co-occurrence check —
    // StreamEvent::ProviderError(body) must be IMMEDIATELY followed by
    // `return Err(anyhow` (within a 400-byte window). This protects against
    // drive-by deletion of the Err return (replacing it with a `continue` or
    // a `break` would silently re-introduce the gap).
    let needle = "StreamEvent::ProviderError(body)";
    let idx = AGENT_LOOP_SOURCE.find(needle).expect(
        "PROV-07 (phase 36.14): agent_loop.rs must contain `StreamEvent::ProviderError(body)` \
         match arm. See phase 36.14-sse-stream-error-fallback-gap."
    );
    let window_end = (idx + needle.len() + 400).min(AGENT_LOOP_SOURCE.len());
    let window = &AGENT_LOOP_SOURCE[idx..window_end];
    assert!(
        window.contains("return Err(anyhow"),
        "PROV-07 (phase 36.14): agent_loop.rs `StreamEvent::ProviderError(body)` arm must \
         return Err(anyhow!(body)) — i.e. `return Err(anyhow` must appear within 400 bytes \
         of the match-arm pattern. Replacing the Err return with `continue` or `break` would \
         silently re-introduce the SSE-body fallback gap. \
         See phase 36.14-sse-stream-error-fallback-gap."
    );
}
