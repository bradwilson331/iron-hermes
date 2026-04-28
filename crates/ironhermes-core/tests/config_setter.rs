//! Round-trip tests for config_setter dotted-path get/set.

use ironhermes_core::config_setter::{config_set, config_get, is_cache_breaking};
use ironhermes_core::config_schema::schema;

#[test]
fn config_setter_creates_file_and_sets_model_default() {
    let tmp = tempfile::TempDir::new().unwrap();
    config_set(tmp.path(), "model.default", "openrouter/qwen-2.5-coder-32b").unwrap();
    let got = config_get(tmp.path(), "model.default").unwrap();
    assert_eq!(got.as_deref(), Some("openrouter/qwen-2.5-coder-32b"));
}

#[test]
fn config_set_preserves_other_keys() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Set two keys
    config_set(tmp.path(), "model.default", "model-a").unwrap();
    config_set(tmp.path(), "model.provider", "openrouter").unwrap();
    // Mutate one, check the other survives
    config_set(tmp.path(), "model.default", "model-b").unwrap();
    let provider = config_get(tmp.path(), "model.provider").unwrap();
    assert_eq!(provider.as_deref(), Some("openrouter"), "model.provider should survive mutation of model.default");
}

#[test]
fn config_set_creates_deep_nested_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    config_set(tmp.path(), "gateway.platforms.telegram.token", "bot-token-123").unwrap();
    let got = config_get(tmp.path(), "gateway.platforms.telegram.token").unwrap();
    assert_eq!(got.as_deref(), Some("bot-token-123"));
}

#[test]
fn unknown_keys_survive_roundtrip_d15() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Pre-seed config.yaml with a learning.* block (Phase 32/33 reservation).
    std::fs::write(
        tmp.path().join("config.yaml"),
        "learning:\n  skill_generation_enabled: true\n  periodic_nudge_interval_seconds: 300\nmodel:\n  default: foo\n",
    ).unwrap();
    // Mutate model.default — learning.* MUST survive.
    config_set(tmp.path(), "model.default", "bar").unwrap();
    let got = config_get(tmp.path(), "learning.skill_generation_enabled").unwrap();
    assert_eq!(got.as_deref(), Some("true"), "learning.skill_generation_enabled must survive — D-15 reservation");
    let got_int = config_get(tmp.path(), "learning.periodic_nudge_interval_seconds").unwrap();
    assert_eq!(got_int.as_deref(), Some("300"));
}

#[test]
fn config_get_returns_some_for_existing_path_and_none_for_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    config_set(tmp.path(), "model.default", "my-model").unwrap();
    let existing = config_get(tmp.path(), "model.default").unwrap();
    assert_eq!(existing.as_deref(), Some("my-model"));
    let missing = config_get(tmp.path(), "nonexistent.key").unwrap();
    assert_eq!(missing, None, "missing path should return None");
}

#[test]
fn is_cache_breaking_uses_schema_correctly() {
    let s = schema();
    assert!(is_cache_breaking("model.default", &s), "model.default is cache_breaking per D-13");
    assert!(!is_cache_breaking("memory.user_profile_enabled", &s), "memory.user_profile_enabled is NOT cache_breaking");
}

#[test]
fn config_set_returns_old_value_when_key_existed() {
    let tmp = tempfile::TempDir::new().unwrap();
    // First set — no previous value.
    let first = config_set(tmp.path(), "model.default", "model-a").unwrap();
    assert_eq!(first, None, "no prior value should return None");
    // Second set — prior value was "model-a".
    let second = config_set(tmp.path(), "model.default", "model-b").unwrap();
    assert_eq!(second.as_deref(), Some("model-a"), "second set should return old value");
}

#[test]
fn unknown_top_level_section_survives_roundtrip() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Pre-seed with an unknown section that the Config struct doesn't know about.
    std::fs::write(
        tmp.path().join("config.yaml"),
        "xyz:\n  foo: bar\nmodel:\n  default: original\n",
    ).unwrap();
    // Set model.default — xyz.foo MUST survive.
    config_set(tmp.path(), "model.default", "updated").unwrap();
    let xyz_foo = config_get(tmp.path(), "xyz.foo").unwrap();
    assert_eq!(xyz_foo.as_deref(), Some("bar"), "xyz.foo must survive after setting model.default — Anti-Pattern #1 trap");
    let model_default = config_get(tmp.path(), "model.default").unwrap();
    assert_eq!(model_default.as_deref(), Some("updated"));
}
