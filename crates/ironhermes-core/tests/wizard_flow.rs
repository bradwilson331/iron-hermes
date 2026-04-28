//! Pure-function tests for wizard.rs apply_*_answer functions.
//! No rustyline, no I/O, no async.

use ironhermes_core::config::Config;
use ironhermes_core::wizard::{
    apply_model_answer, apply_provider_answer, apply_api_key_answer,
    apply_learning_loop_answer, apply_memory_provider_answer, apply_hermes_home_answer,
    LEARNING_LOOP_FRAMING,
};

// ─── Task 2: Wave 0 scaffolding tests ─────────────────────────────────────────

#[test]
fn apply_model_uses_default_on_empty_input() {
    let mut config = Config::default();
    apply_model_answer(&mut config, "", "openrouter/qwen-2.5-coder-32b");
    assert_eq!(config.model.default, "openrouter/qwen-2.5-coder-32b");
}

#[test]
fn learning_loop_framing_locked_phrases_present() {
    // Locked by D-16 — verbatim phrases must remain.
    assert!(LEARNING_LOOP_FRAMING.contains("Learning Loop"));
    assert!(LEARNING_LOOP_FRAMING.contains("grow with you"));
    assert!(LEARNING_LOOP_FRAMING.contains("hermes config set memory.enabled false"));
}

// ─── Task 3: provider + api_key + model helpers ────────────────────────────────

#[test]
fn apply_provider_keeps_existing_on_empty_input() {
    let mut config = Config::default();
    // default provider is "openrouter"
    apply_provider_answer(&mut config, "", "openrouter");
    assert_eq!(config.model.provider, "openrouter");
}

#[test]
fn apply_provider_sets_explicit_value() {
    let mut config = Config::default();
    apply_provider_answer(&mut config, "anthropic", "openrouter");
    assert_eq!(config.model.provider, "anthropic");
}

#[test]
fn apply_api_key_sets_key_on_non_empty_input() {
    let mut config = Config::default();
    apply_api_key_answer(&mut config, "sk-test");
    assert_eq!(config.model.api_key, Some("sk-test".to_string()));
}

#[test]
fn apply_api_key_does_not_clear_existing_on_empty_input() {
    let mut config = Config::default();
    config.model.api_key = Some("sk-existing".to_string());
    apply_api_key_answer(&mut config, "");
    assert_eq!(config.model.api_key, Some("sk-existing".to_string()));
}

#[test]
fn apply_api_key_does_not_clear_existing_on_whitespace_input() {
    let mut config = Config::default();
    config.model.api_key = Some("sk-existing".to_string());
    apply_api_key_answer(&mut config, "   ");
    assert_eq!(config.model.api_key, Some("sk-existing".to_string()));
}

#[test]
fn apply_model_uses_explicit_input_trimmed() {
    let mut config = Config::default();
    apply_model_answer(&mut config, "  openrouter/qwen-2.5-coder-32b  ", "default-model");
    assert_eq!(config.model.default, "openrouter/qwen-2.5-coder-32b");
}

// ─── Task 4: Learning Loop + memory helpers ────────────────────────────────────

#[test]
fn learning_loop_yes_writes_full_block() {
    let mut config = Config::default();
    let block = apply_learning_loop_answer(&mut config, "y");
    assert!(config.memory.memory_enabled);
    assert!(config.memory.user_profile_enabled);
    assert_eq!(block[&serde_yaml::Value::String("skill_generation_enabled".into())], serde_yaml::Value::Bool(true));
    assert_eq!(block[&serde_yaml::Value::String("periodic_nudge_interval_seconds".into())], serde_yaml::Value::Number(300u64.into()));
    assert_eq!(block[&serde_yaml::Value::String("reflection_depth".into())], serde_yaml::Value::String("standard".into()));
    assert_eq!(block[&serde_yaml::Value::String("skill_eval".into())], serde_yaml::Value::Bool(true));
    assert_eq!(block[&serde_yaml::Value::String("max_skills".into())], serde_yaml::Value::Number(500u64.into()));
}

#[test]
fn learning_loop_no_writes_explicit_false_never_absent() {
    let mut config = Config::default();
    let block = apply_learning_loop_answer(&mut config, "n");
    assert!(!config.memory.memory_enabled, "n must write explicit false to memory_enabled");
    assert!(!config.memory.user_profile_enabled, "n must write explicit false to user_profile_enabled");
    // skill_generation_enabled MUST be present and false (D-14 final paragraph).
    assert_eq!(
        block[&serde_yaml::Value::String("skill_generation_enabled".into())],
        serde_yaml::Value::Bool(false),
        "skill_generation_enabled must be present as explicit false, not absent"
    );
    assert_eq!(block[&serde_yaml::Value::String("skill_eval".into())], serde_yaml::Value::Bool(false));
    // Others remain at sentinel defaults — they are not user-toggled by the Y/n switch.
    assert_eq!(block[&serde_yaml::Value::String("periodic_nudge_interval_seconds".into())], serde_yaml::Value::Number(300u64.into()));
    assert_eq!(block[&serde_yaml::Value::String("reflection_depth".into())], serde_yaml::Value::String("standard".into()));
    assert_eq!(block[&serde_yaml::Value::String("max_skills".into())], serde_yaml::Value::Number(500u64.into()));
}

#[test]
fn learning_loop_empty_input_defaults_to_yes() {
    let mut config = Config::default();
    let block = apply_learning_loop_answer(&mut config, "");
    assert!(config.memory.memory_enabled, "empty = default YES per D-14");
    assert_eq!(block[&serde_yaml::Value::String("skill_generation_enabled".into())], serde_yaml::Value::Bool(true));
}

#[test]
fn learning_loop_case_variants_are_yes() {
    for input in &["Y", "yes", "YES"] {
        let mut config = Config::default();
        apply_learning_loop_answer(&mut config, input);
        assert!(config.memory.memory_enabled, "'{input}' should enable memory");
    }
}

#[test]
fn apply_memory_provider_rejects_unknown() {
    let mut config = Config::default();
    let result = apply_memory_provider_answer(&mut config, "unknown_provider", "file");
    assert!(result.is_err(), "unknown provider should return Err");
}

#[test]
fn apply_memory_provider_accepts_valid_providers() {
    for provider in &["file", "sqlite", "grafeo", "duckdb"] {
        let mut config = Config::default();
        let result = apply_memory_provider_answer(&mut config, provider, "file");
        assert!(result.is_ok(), "valid provider {provider} should be accepted");
        assert_eq!(config.memory.provider, *provider);
    }
}

#[test]
fn apply_hermes_home_returns_default_on_empty() {
    let result = apply_hermes_home_answer("", "~/.ironhermes");
    assert_eq!(result, "~/.ironhermes");
}

#[test]
fn apply_hermes_home_returns_trimmed_explicit_input() {
    let result = apply_hermes_home_answer("  /custom/path  ", "~/.ironhermes");
    assert_eq!(result, "/custom/path");
}
