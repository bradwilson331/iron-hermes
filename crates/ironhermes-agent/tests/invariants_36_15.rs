//! Phase 36.15 static-grep regression gates.
//! Locks PROV-11 wiring (resolved_extras.clone replaces literal None at agent_loop.rs call_llm
//! and call_llm_streaming) and D-06 out-of-scope markers (any_client.rs::build_openrouter_chat_request_full
//! untouched; anthropic_client.rs::_extra still underscore-ignored).
//! Follows `include_str!` pattern from invariants_36_14.rs / invariants_27_1_4_1.rs. No dev-deps.

const AGENT_LOOP_SOURCE: &str = include_str!("../src/agent_loop.rs");
#[allow(dead_code)]
const AGENT_RUNTIME_SOURCE: &str = include_str!("../src/agent_runtime.rs");
const ANY_CLIENT_SOURCE: &str = include_str!("../src/any_client.rs");
const ANTHROPIC_CLIENT_SOURCE: &str = include_str!("../src/anthropic_client.rs");
const CLIENT_SOURCE: &str = include_str!("../src/client.rs");

/// Strip Rust line comments (`//` to end-of-line) so that grep-gate assertions
/// are not fooled by commentary that happens to contain the forbidden substring.
fn strip_comments(s: &str) -> String {
    s.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn prov_11_agent_loop_no_longer_passes_literal_none_for_extra() {
    let stripped = strip_comments(AGENT_LOOP_SOURCE);
    assert!(
        !stripped.contains("chat_completion(messages, tools, None, None, None, None)"),
        "PROV-11 (phase 36.15): agent_loop.rs call_llm must pass self.resolved_extras.clone() \
         — not literal None — for the extra parameter. If this test fails, Plan 04's substitution \
         at the call_llm site has regressed. The final argument to chat_completion must be \
         self.resolved_extras.clone(), not None."
    );
    assert!(
        !stripped.contains("chat_completion_stream(messages, tools, None, None, None, None)"),
        "PROV-11 (phase 36.15): agent_loop.rs call_llm_streaming must pass self.resolved_extras.clone() \
         — not literal None — for the extra parameter. If this test fails, Plan 04's substitution \
         at the call_llm_streaming site has regressed. The final argument to chat_completion_stream \
         must be self.resolved_extras.clone(), not None."
    );
}

#[test]
fn prov_11_agent_loop_passes_resolved_extras_clone() {
    let stripped = strip_comments(AGENT_LOOP_SOURCE);
    let count = stripped.matches("self.resolved_extras.clone()").count();
    assert!(
        count >= 2,
        "PROV-11 (phase 36.15): agent_loop.rs must contain `self.resolved_extras.clone()` at \
         least twice — once in call_llm and once in call_llm_streaming. Found {} occurrence(s). \
         If this test fails, one or both call sites no longer forward resolved_extras to the \
         LLM client, breaking per-provider extra-request-options passthrough.",
        count
    );
}

#[test]
fn d_06_openrouter_claude_path_still_present_and_untouched() {
    let stripped = strip_comments(ANY_CLIENT_SOURCE);
    assert!(
        stripped.contains("fn build_openrouter_chat_request_full"),
        "D-06: build_openrouter_chat_request_full is intentionally out-of-scope in phase 36.15. \
         If this signature is missing, the function was removed or renamed — verify before continuing. \
         The OpenRouter-Claude cache builder must remain present and unwired from resolved_extras \
         until a future phase explicitly wires it."
    );
}

#[test]
fn d_06_anthropic_client_underscore_extra_still_ignored() {
    let stripped = strip_comments(ANTHROPIC_CLIENT_SOURCE);
    assert!(
        stripped.contains("_extra: Option<HashMap"),
        "D-06: AnthropicClient _extra is intentionally ignored in phase 36.15 (leading underscore \
         signals 'unused'). If the underscore is gone, the extras flow has been accidentally wired \
         into the native Anthropic path — revert and defer. The _extra parameter must remain \
         underscore-prefixed until a future phase explicitly wires Anthropic native extras."
    );
}

#[test]
fn client_rs_stream_options_include_usage_floor_preserved() {
    let stripped = strip_comments(CLIENT_SOURCE);
    assert!(
        stripped.contains("or_insert_with"),
        "D-09 invariant: client.rs `or_insert_with` floor for stream_options.include_usage MUST \
         remain. If this test fails, Plan 04 unintentionally removed the floor — restore from git. \
         The or_insert_with entry sets stream_options.include_usage = true for streaming requests."
    );
    assert!(
        stripped.contains("stream_options"),
        "D-09 invariant: client.rs must contain `stream_options` (the JSON field that enables \
         usage reporting for streaming LLM calls). If this test fails, the streaming floor was \
         removed — restore from git."
    );
    assert!(
        stripped.contains("include_usage"),
        "D-09 invariant: client.rs must contain `include_usage` (the stream_options sub-field \
         that requests token usage in streaming responses). If this test fails, the streaming \
         floor was removed — restore from git."
    );
}

// NOTE: PROV-07 invariants from Phase 36.14 are NOT duplicated here.
// They live in `tests/invariants_36_14.rs` and the verify step for this plan
// confirms they still pass by running `cargo test -p ironhermes-agent --test invariants_36_14`.
// agent_runtime.rs is included via include_str! so that a compile-time error surfaces
// if the file is moved or removed; its resolve_extras call is regression-gated by
// agent_runtime.rs's own inline test (run_turn_calls_resolve_extras).
