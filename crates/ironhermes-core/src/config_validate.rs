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
        let provider_entry = self.providers.get(&self.model.provider);
        let legacy_api_key_set = self
            .model
            .api_key
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let new_api_key_env_set = provider_entry
            .and_then(|p| p.api_key_env.as_deref())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        // Keyless self-hosted escape hatch (GAP-7 round-3 D6): a provider entry
        // with a non-empty base_url (llama.cpp / Ollama / vLLM / LAN box) or an
        // inline api_key needs no api_key_env, mirroring has_runnable_llm's
        // local-endpoint precedent (preflight.rs D-08) at the provider level.
        let provider_keyless_ok = provider_entry
            .map(|p| {
                p.base_url
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
                    || p.api_key.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
            })
            .unwrap_or(false);

        if !legacy_api_key_set && !new_api_key_env_set && !provider_keyless_ok {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProviderConfig};

    // =========================================================================
    // Plan 17 (GAP-7 round-3 D6): provider_keyless_ok exemption regression tests.
    // =========================================================================

    fn no_api_key_error(errors: &[ConfigValidationError]) -> bool {
        !errors.iter().any(|e| e.path == "model.api_key")
    }

    fn has_api_key_error(errors: &[ConfigValidationError]) -> bool {
        errors.iter().any(|e| {
            e.path == "model.api_key"
                && e.reason.starts_with("API key required")
                && e.suggested_fix.as_deref() == Some("hermes setup model")
        })
    }

    #[test]
    fn keyless_local_base_url_provider_validates_clean() {
        let mut config = Config::default();
        config.model.provider = "local".to_string();
        config.model.api_key = None;
        config.providers.insert(
            "local".to_string(),
            ProviderConfig {
                base_url: Some("http://127.0.0.1:8080/v1".to_string()),
                api_key: None,
                api_key_env: None,
                ..Default::default()
            },
        );

        let errors = config.validate();
        assert!(
            no_api_key_error(&errors),
            "keyless base_url provider must not trigger model.api_key error, got: {:?}",
            errors
        );
    }

    #[test]
    fn provider_inline_api_key_validates_clean() {
        let mut config = Config::default();
        config.model.provider = "p".to_string();
        config.model.api_key = None;
        config.providers.insert(
            "p".to_string(),
            ProviderConfig {
                base_url: None,
                api_key: Some("k".to_string()),
                api_key_env: None,
                ..Default::default()
            },
        );

        let errors = config.validate();
        assert!(
            no_api_key_error(&errors),
            "inline provider api_key must not trigger model.api_key error, got: {:?}",
            errors
        );
    }

    #[test]
    fn keyless_no_base_url_still_fails() {
        let mut config = Config::default();
        config.model.provider = "cloud".to_string();
        config.model.api_key = None;
        config.providers.insert(
            "cloud".to_string(),
            ProviderConfig {
                base_url: None,
                api_key: None,
                api_key_env: None,
                ..Default::default()
            },
        );

        let errors = config.validate();
        assert!(
            has_api_key_error(&errors),
            "a provider with no base_url/api_key/api_key_env must still fail model.api_key, got: {:?}",
            errors
        );
    }
}
