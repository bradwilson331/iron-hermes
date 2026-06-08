//! Phase 36.2 Plan 11 (OPTIONAL STRETCH per D-CACHE-01) — OpenRouter Claude
//! `cache_control` attachment on the `AnyClient::ChatCompletions` arm.
//!
//! Validates the provider+model gating predicate `is_openrouter_claude` and the
//! request-body builder that attaches `cache_control` markers using the same
//! `{ type: "ephemeral", ttl: "5m" | "1h" }` envelope as native Anthropic
//! (Assumption A1 — verified against OpenRouter docs 2026-05-25).
//!
//! Three matrix slots verified:
//!   (a) openrouter + claude        → markers attached
//!   (b) openrouter + non-claude    → ZERO markers (avoids upstream rejection)
//!   (c) non-openrouter             → ZERO markers (ChatCompletions arm stays
//!                                    cache-free for OpenAI/Nous/custom endpoints)
//!
//! The Plan 05 invariant `grep -c 'cache_control' any_client.rs == 0` is
//! RELAXED to `>= 1` for wave 3 — gated by the `is_openrouter_claude`
//! predicate. The Anthropic-side PARITY from Plan 05 is preserved
//! (AnthropicMessages arm untouched).

use ironhermes_agent::any_client::{
    build_openrouter_chat_request_for_test, is_openrouter_claude,
    serialize_openrouter_request_for_test,
};
use ironhermes_core::ChatMessage;
use ironhermes_core::config::{CacheTtl, PromptCachingConfig};

fn synthetic_conversation(non_system_turns: usize) -> Vec<ChatMessage> {
    let mut msgs = vec![ChatMessage::system("cached system prompt — layers 1-8")];
    for i in 0..non_system_turns {
        let text = format!("turn {}", i + 1);
        if i % 2 == 0 {
            msgs.push(ChatMessage::user(text));
        } else {
            msgs.push(ChatMessage::assistant(text));
        }
    }
    msgs
}

// =============================================================================
// Test 1-4: provider+model gating predicate
// =============================================================================

#[test]
fn is_openrouter_claude_true_for_openrouter_anthropic_claude_prefix() {
    assert!(
        is_openrouter_claude("openrouter", "anthropic/claude-opus-4-7"),
        "openrouter + anthropic/claude-* must enable cache_control gating"
    );
}

#[test]
fn is_openrouter_claude_false_for_openrouter_non_claude() {
    assert!(
        !is_openrouter_claude("openrouter", "openai/gpt-4o"),
        "openrouter + non-Claude model must NOT enable cache_control (provider rejection risk)"
    );
}

#[test]
fn is_openrouter_claude_false_for_non_openrouter_provider() {
    assert!(
        !is_openrouter_claude("anthropic", "claude-opus-4-7"),
        "non-openrouter provider must NOT trigger this gating predicate (AnthropicMessages arm handles native Anthropic — Plan 05)"
    );
    assert!(
        !is_openrouter_claude("openai", "gpt-4o"),
        "openai provider must NOT trigger gating"
    );
    assert!(
        !is_openrouter_claude("nous", "Hermes-3"),
        "nous provider must NOT trigger gating"
    );
}

#[test]
fn is_openrouter_claude_true_for_openrouter_bare_claude_prefix() {
    // OpenRouter sometimes routes Claude models without the `anthropic/` prefix.
    assert!(
        is_openrouter_claude("openrouter", "claude-opus-4-7"),
        "openrouter + bare 'claude-' prefix must also enable gating"
    );
    assert!(
        is_openrouter_claude("openrouter", "claude-sonnet-4-6"),
        "openrouter + bare 'claude-sonnet-*' must enable gating"
    );
}

#[test]
fn is_openrouter_claude_false_for_unrelated_models_on_openrouter() {
    assert!(
        !is_openrouter_claude("openrouter", "meta-llama/llama-3.1-70b-instruct"),
        "openrouter + Llama must NOT enable gating"
    );
    assert!(
        !is_openrouter_claude("openrouter", "google/gemini-pro-1.5"),
        "openrouter + Gemini must NOT enable gating"
    );
}

// =============================================================================
// Test 5: OpenRouter Claude — 4 markers attached (system + 3 messages)
// =============================================================================

#[test]
fn openrouter_claude_request_attaches_four_cache_control_markers_with_one_hour_ttl() {
    let messages = synthetic_conversation(4); // system + 4 non-system → 5 total
    let cfg = PromptCachingConfig::default(); // enabled=true, ttl=OneHour
    let request = build_openrouter_chat_request_for_test(
        "openrouter",
        "anthropic/claude-opus-4-7",
        &messages,
        &cfg,
    );
    let json = serialize_openrouter_request_for_test(&request).expect("serialize");
    let marker_count = json.matches("\"cache_control\"").count();
    assert_eq!(
        marker_count, 4,
        "system_and_3 strategy must attach exactly 4 cache_control markers (1 system + 3 messages); got {marker_count}.\nJSON: {json}"
    );
    let envelope_count = json
        .matches("\"cache_control\":{\"type\":\"ephemeral\",\"ttl\":\"1h\"}")
        .count();
    assert_eq!(
        envelope_count, 4,
        "all 4 markers must use the Anthropic-identical envelope {{ type: ephemeral, ttl: 1h }}; got {envelope_count}.\nJSON: {json}"
    );
}

// =============================================================================
// Test 6: OpenRouter non-Claude — ZERO markers (provider rejection avoidance)
// =============================================================================

#[test]
fn openrouter_non_claude_request_attaches_zero_cache_control_markers() {
    let messages = synthetic_conversation(4);
    let cfg = PromptCachingConfig::default();
    let request =
        build_openrouter_chat_request_for_test("openrouter", "openai/gpt-4o", &messages, &cfg);
    let json = serialize_openrouter_request_for_test(&request).expect("serialize");
    assert_eq!(
        json.matches("cache_control").count(),
        0,
        "openrouter + non-Claude model must produce ZERO cache_control markers (some providers reject the field).\nJSON: {json}"
    );
}

// =============================================================================
// Test 7: non-OpenRouter — ZERO markers (AnthropicMessages arm handles native)
// =============================================================================

#[test]
fn non_openrouter_request_attaches_zero_cache_control_markers() {
    let messages = synthetic_conversation(4);
    let cfg = PromptCachingConfig::default();
    // Even with a Claude-shaped model name, non-openrouter providers MUST NOT
    // attach markers on the ChatCompletions arm. The AnthropicMessages arm
    // (Plan 05) handles native Anthropic; OpenAI/Nous/custom endpoints don't
    // support the field.
    let request =
        build_openrouter_chat_request_for_test("openai", "claude-opus-4-7", &messages, &cfg);
    let json = serialize_openrouter_request_for_test(&request).expect("serialize");
    assert_eq!(
        json.matches("cache_control").count(),
        0,
        "non-openrouter provider must produce ZERO cache_control markers on ChatCompletions arm.\nJSON: {json}"
    );
}

// =============================================================================
// Test 8: disabled config — ZERO markers
// =============================================================================

#[test]
fn openrouter_claude_request_with_caching_disabled_attaches_zero_markers() {
    let messages = synthetic_conversation(4);
    let cfg = PromptCachingConfig {
        ttl: CacheTtl::OneHour,
        enabled: false,
    };
    let request = build_openrouter_chat_request_for_test(
        "openrouter",
        "anthropic/claude-opus-4-7",
        &messages,
        &cfg,
    );
    let json = serialize_openrouter_request_for_test(&request).expect("serialize");
    assert_eq!(
        json.matches("cache_control").count(),
        0,
        "enabled=false must produce ZERO markers even for openrouter+Claude (operator opt-out).\nJSON: {json}"
    );
}

// =============================================================================
// Test 9: 5m TTL — markers carry "5m" ttl string
// =============================================================================

#[test]
fn openrouter_claude_request_with_five_minute_ttl_serializes_correctly() {
    let messages = synthetic_conversation(4);
    let cfg = PromptCachingConfig {
        ttl: CacheTtl::FiveMinutes,
        enabled: true,
    };
    let request = build_openrouter_chat_request_for_test(
        "openrouter",
        "anthropic/claude-opus-4-7",
        &messages,
        &cfg,
    );
    let json = serialize_openrouter_request_for_test(&request).expect("serialize");
    let envelope_count = json
        .matches("\"cache_control\":{\"type\":\"ephemeral\",\"ttl\":\"5m\"}")
        .count();
    assert_eq!(
        envelope_count, 4,
        "all 4 markers must carry ttl=5m when config selects FiveMinutes.\nJSON: {json}"
    );
}

// =============================================================================
// Test 10: saturating boundary — 2-message conversation produces 3 markers
// =============================================================================

#[test]
fn openrouter_claude_request_saturating_when_only_two_messages() {
    // system + user + assistant → 3 messages total (1 system + 2 non-system).
    // system_and_3 strategy: 1 system marker + min(3,2)=2 message markers = 3 total.
    let messages = synthetic_conversation(2);
    let cfg = PromptCachingConfig::default();
    let request = build_openrouter_chat_request_for_test(
        "openrouter",
        "anthropic/claude-opus-4-7",
        &messages,
        &cfg,
    );
    let json = serialize_openrouter_request_for_test(&request).expect("serialize");
    let marker_count = json.matches("\"cache_control\"").count();
    assert_eq!(
        marker_count, 3,
        "saturating_sub(3) on a 2-message non-system tail must produce 3 markers (1 system + 2 messages); got {marker_count}.\nJSON: {json}"
    );
}

// =============================================================================
// Test 11: cut-line invariant — Anthropic-side PARITY preserved (Plan 05)
// =============================================================================

#[test]
fn plan_05_anthropic_messages_arm_remains_cache_control_aware() {
    // This test asserts that the Plan 05 builder (AnthropicMessages arm) still
    // attaches markers — proving Plan 11's wave-3 changes did not regress the
    // Plan 05 native-Anthropic PARITY claim.
    use ironhermes_agent::anthropic_client::{
        build_anthropic_request_for_test, serialize_request_for_test,
    };
    let messages = synthetic_conversation(4);
    let cfg = PromptCachingConfig::default();
    let req = build_anthropic_request_for_test(&messages, "claude-opus-4-7", &cfg);
    let json = serialize_request_for_test(&req).expect("serialize anthropic req");
    let marker_count = json.matches("\"cache_control\"").count();
    assert_eq!(
        marker_count, 4,
        "Plan 11 must NOT regress Plan 05's Anthropic-side PARITY — system_and_3 markers must still attach on AnthropicMessages arm.\nJSON: {json}"
    );
}
