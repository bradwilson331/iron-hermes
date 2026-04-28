//! Property tests for Config::validate().

use ironhermes_core::config::Config;
use ironhermes_core::config_validate::ConfigValidationError;

fn valid_config() -> Config {
    let mut c = Config::default();
    c.model.api_key = Some("sk-test".into());
    c.model.default = "openrouter/qwen-2.5-coder-32b".into();
    c.model.provider = "openrouter".into();
    c
}

#[test]
fn validate_returns_vec_of_errors() {
    let config = Config::default();
    let errors: Vec<ConfigValidationError> = config.validate();
    // Stub returns empty; real rules implemented — api_key + model.default errors expected from default config.
    let _ = errors;
}

#[test]
fn validate_missing_api_key_returns_error() {
    let mut config = valid_config();
    config.model.api_key = None;
    let errors = config.validate();
    assert!(
        errors.iter().any(|e| e.path == "model.api_key"),
        "expected model.api_key error, got: {:?}", errors
    );
}

#[test]
fn validate_empty_api_key_returns_error() {
    let mut config = valid_config();
    config.model.api_key = Some("".into());
    let errors = config.validate();
    assert!(
        errors.iter().any(|e| e.path == "model.api_key"),
        "empty api_key should produce error"
    );
}

#[test]
fn validate_empty_model_default_returns_error() {
    let mut config = valid_config();
    config.model.default = "".into();
    let errors = config.validate();
    assert!(
        errors.iter().any(|e| e.path == "model.default"),
        "empty model.default should produce error"
    );
}

#[test]
fn validate_empty_provider_returns_error() {
    let mut config = valid_config();
    config.model.provider = "".into();
    let errors = config.validate();
    assert!(
        errors.iter().any(|e| e.path == "model.provider"),
        "empty model.provider should produce error"
    );
}

#[test]
fn validate_fully_populated_returns_empty_vec() {
    let config = valid_config();
    let errors = config.validate();
    assert!(errors.is_empty(), "fully populated config should produce no errors, got: {:?}", errors);
}

#[test]
fn validate_errors_have_non_empty_reason_and_suggested_fix() {
    let mut config = valid_config();
    config.model.api_key = None;
    let errors = config.validate();
    let api_key_err = errors.iter().find(|e| e.path == "model.api_key")
        .expect("should have model.api_key error");
    assert!(!api_key_err.reason.is_empty(), "reason should be non-empty");
    assert!(
        api_key_err.suggested_fix.as_deref().unwrap_or("").contains("hermes setup"),
        "suggested_fix should contain 'hermes setup'"
    );
}

#[test]
fn validate_yaml_roundtrip_stays_valid() {
    let config = valid_config();
    let yaml = serde_yaml::to_string(&config).unwrap();
    let roundtripped: Config = serde_yaml::from_str(&yaml).unwrap();
    let errors = roundtripped.validate();
    assert!(errors.is_empty(), "round-tripped config should still be valid, got: {:?}", errors);
}

#[test]
fn validate_default_config_has_errors() {
    // Config::default() has no api_key — must surface at least 1 error.
    let config = Config::default();
    let errors = config.validate();
    assert!(!errors.is_empty(), "Config::default() should have at least one validation error (missing api_key)");
    assert!(errors.iter().any(|e| e.path == "model.api_key"), "should flag missing api_key");
}
