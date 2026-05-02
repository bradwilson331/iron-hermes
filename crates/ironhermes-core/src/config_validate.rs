//! `config_validate.rs` — Config::validate() + ConfigValidationError (D-06).
//!
//! Validation is infallible — errors are returned as a Vec of structured
//! data, not Result. The preflight middleware (Plan 23-02) inspects this
//! Vec to decide whether to launch fix-mode wizard.

use crate::config::Config;

/// A single config validation failure, keyed by dotted path (D-06, D-08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationError {
    pub path: String,
    pub reason: String,
    pub suggested_fix: Option<String>,
}

impl Config {
    /// Validate this config and return all detected problems.
    /// Empty Vec means the config is ready to dispatch (D-06).
    pub fn validate(&self) -> Vec<ConfigValidationError> {
        let mut errors = Vec::new();

        // API key required — accept EITHER legacy model.api_key OR
        // providers.<main-provider>.api_key_env (Phase 26 canonical schema).
        // model.api_key is kept one release as a deprecated fallback; either
        // presence satisfies the validator. Structural check only — actual
        // env-var resolution happens at ProviderResolver::build() time.
        let legacy_api_key_set = self
            .model
            .api_key
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let new_api_key_env_set = self
            .providers
            .get(&self.model.provider)
            .and_then(|p| p.api_key_env.as_deref())
            .map(|s| !s.is_empty())
            .unwrap_or(false);

        if !legacy_api_key_set && !new_api_key_env_set {
            errors.push(ConfigValidationError {
                path: "model.api_key".into(),
                reason: format!(
                    "API key required — set providers.{}.api_key_env (preferred) or model.api_key (deprecated)",
                    self.model.provider
                ),
                suggested_fix: Some("hermes setup model".into()),
            });
        }

        // model.default — required, non-empty.
        if self.model.default.trim().is_empty() {
            errors.push(ConfigValidationError {
                path: "model.default".into(),
                reason: "Default model identifier is required".into(),
                suggested_fix: Some("hermes setup model".into()),
            });
        }

        // model.provider — required, non-empty.
        if self.model.provider.trim().is_empty() {
            errors.push(ConfigValidationError {
                path: "model.provider".into(),
                reason: "Provider name is required (e.g., openrouter, anthropic)".into(),
                suggested_fix: Some("hermes setup model".into()),
            });
        }

        // memory.provider — must be one of the known backends if memory is enabled.
        if self.memory.memory_enabled {
            let valid = ["file", "sqlite", "grafeo", "duckdb"];
            if !valid.contains(&self.memory.provider.as_str()) {
                errors.push(ConfigValidationError {
                    path: "memory.provider".into(),
                    reason: format!(
                        "Memory provider {:?} is not one of: file, sqlite, grafeo, duckdb",
                        self.memory.provider
                    ),
                    suggested_fix: Some("hermes setup memory".into()),
                });
            }
        }

        errors
    }
}
