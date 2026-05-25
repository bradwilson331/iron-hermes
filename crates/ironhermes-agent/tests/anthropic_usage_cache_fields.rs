//! Phase 36.2 Plan 01 — AnthropicUsage cache field static invariants.
//!
//! `AnthropicUsage` is module-private (no `pub` modifier), so the JSON
//! round-trip behavior is covered by inline tests inside
//! `crates/ironhermes-agent/src/anthropic_client.rs`. This integration file
//! locks the structural invariant — the literal source text — against
//! accidental removal or regression via `include_str!` static-grep.
//!
//! The four assertions cover:
//! 1. The struct field for `cache_read_input_tokens` exists (`Option<usize>`).
//! 2. The struct field for `cache_creation_input_tokens` exists.
//! 3. Both parse sites (non-streaming + streaming) populate the outer `Usage`
//!    from the response instead of the prior hardcoded `None`.
//! 4. No leftover `cache_read_input_tokens: None,` / `cache_creation_input_tokens: None,`
//!    hardcoded literals remain at either parse site.

const ANTHROPIC_CLIENT_SOURCE: &str = include_str!("../src/anthropic_client.rs");

#[test]
fn anthropic_usage_struct_carries_cache_read_field() {
    assert!(
        ANTHROPIC_CLIENT_SOURCE.contains("cache_read_input_tokens: Option<usize>"),
        "Phase 36.2 Plan 01: AnthropicUsage must declare \
         `cache_read_input_tokens: Option<usize>` (with #[serde(default)])"
    );
}

#[test]
fn anthropic_usage_struct_carries_cache_creation_field() {
    assert!(
        ANTHROPIC_CLIENT_SOURCE.contains("cache_creation_input_tokens: Option<usize>"),
        "Phase 36.2 Plan 01: AnthropicUsage must declare \
         `cache_creation_input_tokens: Option<usize>` (with #[serde(default)])"
    );
}

#[test]
fn parse_sites_populate_cache_read_from_response_usage() {
    // The non-streaming parse_anthropic_response site (around line 537) must
    // populate cache_read_input_tokens from response.usage.cache_read_input_tokens.
    assert!(
        ANTHROPIC_CLIENT_SOURCE
            .contains("cache_read_input_tokens: response.usage.cache_read_input_tokens"),
        "Phase 36.2 Plan 01: non-streaming parse site must populate \
         cache_read_input_tokens from response.usage, not hardcode None"
    );
}

#[test]
fn parse_sites_populate_cache_creation_from_response_usage() {
    assert!(
        ANTHROPIC_CLIENT_SOURCE
            .contains("cache_creation_input_tokens: response.usage.cache_creation_input_tokens"),
        "Phase 36.2 Plan 01: non-streaming parse site must populate \
         cache_creation_input_tokens from response.usage, not hardcode None"
    );
}

#[test]
fn no_hardcoded_none_cache_fields_remain_in_anthropic_client() {
    // After Phase 36.2 Plan 01, neither parse site (non-streaming at ~537,
    // streaming at ~823) should still carry `cache_*_input_tokens: None,`.
    // Both sites now copy through from their respective usage structs.
    assert!(
        !ANTHROPIC_CLIENT_SOURCE.contains("cache_read_input_tokens: None,"),
        "Phase 36.2 Plan 01: no parse site may still hardcode \
         `cache_read_input_tokens: None,` — populate from usage struct"
    );
    assert!(
        !ANTHROPIC_CLIENT_SOURCE.contains("cache_creation_input_tokens: None,"),
        "Phase 36.2 Plan 01: no parse site may still hardcode \
         `cache_creation_input_tokens: None,` — populate from usage struct"
    );
}

#[test]
fn streaming_parse_site_populates_cache_fields_from_sse_usage() {
    // Streaming MessageDelta site (around line 823) must populate cache fields
    // from the SseUsage that was extended in lockstep with AnthropicUsage.
    assert!(
        ANTHROPIC_CLIENT_SOURCE.contains("cache_read_input_tokens: u.cache_read_input_tokens"),
        "Phase 36.2 Plan 01: streaming MessageDelta site must populate \
         cache_read_input_tokens from the SseUsage delta"
    );
    assert!(
        ANTHROPIC_CLIENT_SOURCE
            .contains("cache_creation_input_tokens: u.cache_creation_input_tokens"),
        "Phase 36.2 Plan 01: streaming MessageDelta site must populate \
         cache_creation_input_tokens from the SseUsage delta"
    );
}
