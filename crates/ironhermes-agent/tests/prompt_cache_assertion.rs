//! Phase 36.2 Plan 05 — prompt caching config + cached-layer-stability assertion.
//!
//! Covers PromptCachingConfig + CacheTtl deserialization (round-trip, strict TTL,
//! pre-36.2 backward compat via `#[serde(default)]`, helper accessor) and the
//! cache-control marker envelope shape on the Anthropic request body.
//!
//! The `cached_layers_must_be_stable()` assertion tests live INLINE in
//! `prompt_builder.rs` (`#[cfg(test)] mod tests`) because that function is
//! `pub(crate)` and unreachable from an integration test.

use ironhermes_core::config::{CacheTtl, Config, PromptCachingConfig};

// =============================================================================
// Config — PromptCachingConfig round-trip
// =============================================================================

#[test]
fn prompt_caching_config_round_trip_one_hour() {
    let yaml = r#"
prompt_caching:
  ttl: "1h"
  enabled: true
"#;
    let cfg: Config = serde_yaml::from_str(yaml).expect("deserialize Config");
    assert_eq!(cfg.prompt_caching.ttl, CacheTtl::OneHour);
    assert!(cfg.prompt_caching.enabled);
}

#[test]
fn prompt_caching_config_round_trip_five_minutes() {
    let yaml = r#"
prompt_caching:
  ttl: "5m"
"#;
    let cfg: Config = serde_yaml::from_str(yaml).expect("deserialize Config");
    assert_eq!(cfg.prompt_caching.ttl, CacheTtl::FiveMinutes);
}

#[test]
fn prompt_caching_config_rejects_unknown_ttl() {
    let yaml = r#"
prompt_caching:
  ttl: "30s"
"#;
    let result: Result<Config, _> = serde_yaml::from_str(yaml);
    assert!(
        result.is_err(),
        "Phase 36.2: unknown TTL must fail deserialization (CacheTtl is a closed enum)"
    );
}

#[test]
fn prompt_caching_config_defaults_when_absent() {
    // Pre-36.2 config with no prompt_caching key — must parse and use defaults.
    let yaml = r#"
model:
  default: "claude-opus-4-7"
"#;
    let cfg: Config = serde_yaml::from_str(yaml).expect("deserialize legacy Config");
    assert_eq!(
        cfg.prompt_caching.ttl,
        CacheTtl::OneHour,
        "Phase 36.2: default ttl is 1h"
    );
    assert!(
        cfg.prompt_caching.enabled,
        "Phase 36.2: prompt caching enabled by default"
    );
}

#[test]
fn cache_ttl_as_anthropic_ttl_strings() {
    assert_eq!(CacheTtl::OneHour.as_anthropic_ttl(), "1h");
    assert_eq!(CacheTtl::FiveMinutes.as_anthropic_ttl(), "5m");
}

#[test]
fn prompt_caching_config_default_matches_constructor() {
    let from_default = PromptCachingConfig::default();
    assert_eq!(from_default.ttl, CacheTtl::OneHour);
    assert!(from_default.enabled);
}

// =============================================================================
// Anthropic cache_control envelope — Task 2 tests
// =============================================================================

use ironhermes_agent::anthropic_client::{
    AnthropicClient, build_anthropic_request_for_test, serialize_request_for_test,
};
use ironhermes_core::ChatMessage;

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

#[test]
fn cache_control_envelope_serializes_with_one_hour_ttl() {
    let cfg = PromptCachingConfig {
        ttl: CacheTtl::OneHour,
        enabled: true,
    };
    let req = build_anthropic_request_for_test(&synthetic_conversation(2), "claude-opus-4-7", &cfg);
    let json = serialize_request_for_test(&req).expect("serialize request");
    assert!(
        json.contains(r#""cache_control":{"type":"ephemeral","ttl":"1h"}"#),
        "expected cache_control envelope with ttl=1h in JSON, got: {}",
        json
    );
}

#[test]
fn cache_control_envelope_serializes_with_five_minute_ttl() {
    let cfg = PromptCachingConfig {
        ttl: CacheTtl::FiveMinutes,
        enabled: true,
    };
    let req = build_anthropic_request_for_test(&synthetic_conversation(2), "claude-opus-4-7", &cfg);
    let json = serialize_request_for_test(&req).expect("serialize request");
    assert!(
        json.contains(r#""ttl":"5m""#),
        "expected ttl=5m in JSON, got: {}",
        json
    );
}

#[test]
fn cache_control_disabled_produces_zero_markers() {
    let cfg = PromptCachingConfig {
        ttl: CacheTtl::OneHour,
        enabled: false,
    };
    let req = build_anthropic_request_for_test(&synthetic_conversation(5), "claude-opus-4-7", &cfg);
    let json = serialize_request_for_test(&req).expect("serialize request");
    assert!(
        !json.contains("cache_control"),
        "expected zero cache_control markers when disabled, got: {}",
        json
    );
}

#[test]
fn cache_control_system_and_three_count_with_five_messages() {
    let cfg = PromptCachingConfig::default();
    let req = build_anthropic_request_for_test(&synthetic_conversation(5), "claude-opus-4-7", &cfg);
    let json = serialize_request_for_test(&req).expect("serialize request");
    let count = json.matches("cache_control").count();
    assert_eq!(
        count, 4,
        "expected 4 cache_control markers (1 system + 3 last messages) for 5-msg convo, got {}: {}",
        count, json
    );
}

#[test]
fn cache_control_system_and_two_saturating_when_only_two_messages() {
    let cfg = PromptCachingConfig::default();
    let req = build_anthropic_request_for_test(&synthetic_conversation(2), "claude-opus-4-7", &cfg);
    let json = serialize_request_for_test(&req).expect("serialize request");
    let count = json.matches("cache_control").count();
    assert_eq!(
        count, 3,
        "expected 3 cache_control markers (1 system + 2 messages) for 2-msg convo (saturating_sub), got {}: {}",
        count, json
    );
}

#[test]
fn anthropic_client_constructor_carries_prompt_caching_config() {
    // Smoke test: AnthropicClient can be constructed with PromptCachingConfig.
    let cfg = PromptCachingConfig::default();
    let _client = AnthropicClient::new_with_prompt_caching(
        "https://api.anthropic.com",
        "sk-test",
        "claude-opus-4-7",
        cfg,
    );
}
