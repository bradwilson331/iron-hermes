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

        // model.api_key — required, non-empty.
        let api_key_empty = self.model.api_key.as_deref().unwrap_or("").is_empty();
        if api_key_empty {
            errors.push(ConfigValidationError {
                path: "model.api_key".into(),
                reason: "API key is required to call the LLM provider".into(),
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
