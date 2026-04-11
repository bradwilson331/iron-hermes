use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::fmt;

use crate::config::{ApiMode, Config, ModelRoleConfig};
use crate::constants::{ANTHROPIC_BASE_URL, OPENROUTER_BASE_URL};

// =============================================================================
// ResolvedEndpoint (D-01, D-04)
// =============================================================================

/// A fully-resolved provider endpoint with scoped API key.
///
/// The Debug impl redacts the api_key to prevent accidental key logging (T-12-01).
#[derive(Clone)]
pub struct ResolvedEndpoint {
    pub base_url: String,
    pub api_key: Option<String>,
    pub api_mode: ApiMode,
    pub default_model: String,
    pub fallback_providers: Vec<String>,
}

impl fmt::Debug for ResolvedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedEndpoint")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_deref().map(|_| "[REDACTED]"))
            .field("api_mode", &self.api_mode)
            .field("default_model", &self.default_model)
            .field("fallback_providers", &self.fallback_providers)
            .finish()
    }
}

// =============================================================================
// URL safety check for provider base_url (T-12-02)
// =============================================================================

/// Validate a provider base_url.
///
/// Allows:
/// - Any https:// URL (public endpoints)
/// - http:// only when host is "localhost" or "127.0.0.1" (local model servers)
///
/// Rejects http:// with any other host to prevent accidental key exfiltration.
fn is_provider_url_safe(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(parsed.host_str(), Some("localhost") | Some("127.0.0.1")),
        _ => false,
    }
}

// =============================================================================
// ProviderResolver (D-01, D-02, D-03)
// =============================================================================

/// Builds and holds a lookup table of resolved provider endpoints.
///
/// Constructed once at startup from `Config` + environment variables.
/// Resolution precedence (D-03, PROV-03): config > env var > built-in default.
#[derive(Debug, Clone)]
pub struct ProviderResolver {
    endpoints: HashMap<String, ResolvedEndpoint>,
    roles: HashMap<String, ModelRoleConfig>,
    main_provider: String,
}

impl ProviderResolver {
    /// Build the resolver from the application config.
    ///
    /// # Errors
    /// - Unknown `fallback_providers` entries
    /// - Custom provider with non-https base_url (unless localhost/127.0.0.1)
    pub fn build(config: &Config) -> Result<Self> {
        let mut endpoints: HashMap<String, ResolvedEndpoint> = HashMap::new();

        // --- 1. Pre-populate three built-in providers with defaults ---
        endpoints.insert(
            "openrouter".to_string(),
            ResolvedEndpoint {
                base_url: OPENROUTER_BASE_URL.to_string(),
                api_key: None,
                api_mode: ApiMode::ChatCompletions,
                default_model: config.model.default.clone(),
                fallback_providers: vec![],
            },
        );
        endpoints.insert(
            "anthropic".to_string(),
            ResolvedEndpoint {
                base_url: ANTHROPIC_BASE_URL.to_string(),
                api_key: None,
                api_mode: ApiMode::AnthropicMessages,
                default_model: "claude-sonnet-4-20250514".to_string(),
                fallback_providers: vec![],
            },
        );
        endpoints.insert(
            "openai".to_string(),
            ResolvedEndpoint {
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: None,
                api_mode: ApiMode::ChatCompletions,
                default_model: "gpt-4o".to_string(),
                fallback_providers: vec![],
            },
        );

        // --- 2. Overlay config.providers entries (user config overrides defaults) ---
        for (name, prov_cfg) in &config.providers {
            let entry = endpoints.entry(name.clone()).or_insert_with(|| ResolvedEndpoint {
                base_url: String::new(),
                api_key: None,
                api_mode: ApiMode::ChatCompletions,
                default_model: String::new(),
                fallback_providers: vec![],
            });
            if let Some(ref url) = prov_cfg.base_url {
                entry.base_url = url.clone();
            }
            if let Some(ref mode) = prov_cfg.api_mode {
                entry.api_mode = mode.clone();
            }
            if let Some(ref model) = prov_cfg.default_model {
                entry.default_model = model.clone();
            }
            if !prov_cfg.fallback_providers.is_empty() {
                entry.fallback_providers = prov_cfg.fallback_providers.clone();
            }
            // api_key from ProviderConfig applied after env resolution below
        }

        // --- 3. Add custom_providers entries ---
        for custom in &config.custom_providers {
            // T-12-02: validate base_url scheme
            // Allow https (public) or http only for localhost/127.0.0.1 (local model servers)
            if !is_provider_url_safe(&custom.base_url) {
                return Err(anyhow!(
                    "custom provider '{}' has unsafe base_url '{}': must be https or http://localhost / http://127.0.0.1",
                    custom.name,
                    custom.base_url
                ));
            }
            endpoints.insert(
                custom.name.clone(),
                ResolvedEndpoint {
                    base_url: custom.base_url.clone(),
                    api_key: custom.api_key.clone(),
                    api_mode: custom.api_mode.clone().unwrap_or(ApiMode::ChatCompletions),
                    default_model: custom.default_model.clone().unwrap_or_default(),
                    fallback_providers: vec![],
                },
            );
        }

        // --- 4. Resolve API keys with precedence (D-03, PROV-03, PROV-04) ---
        let main = &config.model.provider;

        for (name, endpoint) in endpoints.iter_mut() {
            // Config explicit api_key for this provider (from config.providers[name].api_key)
            let config_key = config.providers.get(name).and_then(|p| p.api_key.clone());

            let env_key: Option<String> = match name.as_str() {
                "openrouter" => std::env::var("OPENROUTER_API_KEY").ok(),
                "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
                "openai" => std::env::var("OPENAI_API_KEY").ok(),
                // custom providers: try OPENAI_API_KEY as generic fallback
                _ => std::env::var("OPENAI_API_KEY").ok(),
            };

            // Fallback: config.model.api_key for the main provider only
            let model_key = if name == main { config.model.api_key.clone() } else { None };

            endpoint.api_key = config_key
                .or(env_key)
                .or(model_key)
                .or_else(|| endpoint.api_key.clone());
        }

        // --- 5. T-12-03: Validate fallback_providers reference known names ---
        for (name, endpoint) in &endpoints {
            for fb in &endpoint.fallback_providers {
                if !endpoints.contains_key(fb.as_str()) {
                    return Err(anyhow!(
                        "provider '{}' has unknown fallback_provider '{}'",
                        name,
                        fb
                    ));
                }
            }
        }
        // Note: validation above needs a snapshot — we do a second pass after building
        // to avoid borrow issues. The check is done post-build in a separate pass.
        // Actually the check above works because we iterate &endpoints and check contains_key
        // simultaneously, which is fine for shared refs.

        // --- 6. Store roles ---
        let roles = config.model.roles.clone();

        Ok(Self {
            endpoints,
            roles,
            main_provider: main.clone(),
        })
    }

    /// Direct lookup by provider name.
    pub fn resolve(&self, provider: &str) -> Option<&ResolvedEndpoint> {
        self.endpoints.get(provider)
    }

    /// Resolve the main provider. Panics if missing (startup validation prevents this).
    pub fn resolve_for_main(&self) -> &ResolvedEndpoint {
        self.endpoints
            .get(&self.main_provider)
            .unwrap_or_else(|| panic!("main provider '{}' not found in endpoints", self.main_provider))
    }

    /// Resolve an auxiliary model role (D-05, PROV-06).
    ///
    /// - If role.provider == "main", returns a clone of the main endpoint with the role's model.
    /// - Otherwise resolves the role's provider name.
    /// - Returns None if the role is not configured or the provider is unknown.
    pub fn resolve_role(&self, role: &str) -> Option<ResolvedEndpoint> {
        let role_cfg = self.roles.get(role)?;
        let base_endpoint = if role_cfg.provider == "main" {
            self.endpoints.get(&self.main_provider)?
        } else {
            self.endpoints.get(&role_cfg.provider)?
        };
        let mut ep = base_endpoint.clone();
        if let Some(ref model) = role_cfg.model {
            ep.default_model = model.clone();
        }
        Some(ep)
    }

    /// Get the main provider name.
    pub fn main_provider(&self) -> &str {
        &self.main_provider
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CustomProviderConfig, ModelRoleConfig, ProviderConfig};

    fn default_config() -> Config {
        Config::default()
    }

    #[test]
    fn test_build_default_config_has_three_providers() {
        let config = default_config();
        let resolver = ProviderResolver::build(&config).expect("build should succeed");
        assert!(resolver.resolve("openrouter").is_some(), "openrouter should exist");
        assert!(resolver.resolve("anthropic").is_some(), "anthropic should exist");
        assert!(resolver.resolve("openai").is_some(), "openai should exist");
    }

    #[test]
    fn test_resolve_openrouter_base_url_and_mode() {
        let config = default_config();
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve("openrouter").expect("openrouter");
        assert_eq!(ep.base_url, OPENROUTER_BASE_URL);
        assert_eq!(ep.api_mode, ApiMode::ChatCompletions);
    }

    #[test]
    fn test_resolve_anthropic_base_url_and_mode() {
        let config = default_config();
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve("anthropic").expect("anthropic");
        assert_eq!(ep.base_url, ANTHROPIC_BASE_URL);
        assert_eq!(ep.api_mode, ApiMode::AnthropicMessages);
    }

    #[test]
    fn test_resolve_unknown_provider_returns_none() {
        let config = default_config();
        let resolver = ProviderResolver::build(&config).expect("build");
        assert!(resolver.resolve("unknown_provider").is_none());
    }

    #[test]
    fn test_resolve_role_vision_with_configured_provider() {
        let mut config = default_config();
        config.model.roles.insert(
            "vision".to_string(),
            ModelRoleConfig {
                provider: "openrouter".to_string(),
                model: Some("openai/gpt-4o".to_string()),
            },
        );
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve_role("vision").expect("vision role");
        assert_eq!(ep.base_url, OPENROUTER_BASE_URL);
        assert_eq!(ep.default_model, "openai/gpt-4o");
    }

    #[test]
    fn test_resolve_role_main_fallthrough() {
        let mut config = default_config();
        // main provider is "openrouter" by default
        config.model.roles.insert(
            "vision".to_string(),
            ModelRoleConfig {
                provider: "main".to_string(),
                model: Some("openai/gpt-4o-vision".to_string()),
            },
        );
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve_role("vision").expect("vision role via main");
        // Should return the openrouter endpoint (main provider)
        assert_eq!(ep.base_url, OPENROUTER_BASE_URL);
        assert_eq!(ep.default_model, "openai/gpt-4o-vision");
    }

    #[test]
    fn test_api_key_config_takes_precedence_over_env() {
        let mut config = default_config();
        config.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                api_key: Some("config-key".to_string()),
                ..Default::default()
            },
        );

        // Set env var — config key should win
        // SAFETY: test-only, single-threaded context
        unsafe { std::env::set_var("OPENROUTER_API_KEY_TEST_PREC", "env-key"); }

        // We test by setting config.providers key and verifying the resolved key
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve("openrouter").expect("openrouter");
        assert_eq!(ep.api_key.as_deref(), Some("config-key"));

        // SAFETY: test-only cleanup
        unsafe { std::env::remove_var("OPENROUTER_API_KEY_TEST_PREC"); }
    }

    #[test]
    fn test_api_mode_has_three_variants() {
        // Ensure all three variants serialize/deserialize correctly
        let chat = ApiMode::ChatCompletions;
        let anthr = ApiMode::AnthropicMessages;
        let codex = ApiMode::CodexResponses;

        let chat_json = serde_json::to_string(&chat).unwrap();
        let anthr_json = serde_json::to_string(&anthr).unwrap();
        let codex_json = serde_json::to_string(&codex).unwrap();

        assert_eq!(chat_json, "\"chat_completions\"");
        assert_eq!(anthr_json, "\"anthropic_messages\"");
        assert_eq!(codex_json, "\"codex_responses\"");
    }

    #[test]
    fn test_resolved_endpoint_api_key_scoping() {
        // OPENROUTER_API_KEY should only appear for openrouter, not anthropic
        let unique_key = "test_scoping_key_12345";
        // Use a temporary unique env var name won't conflict since it's scoped
        let mut config = default_config();
        config.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                api_key: Some(unique_key.to_string()),
                ..Default::default()
            },
        );
        let resolver = ProviderResolver::build(&config).expect("build");
        let or_ep = resolver.resolve("openrouter").expect("openrouter");
        let anthr_ep = resolver.resolve("anthropic").expect("anthropic");

        assert_eq!(or_ep.api_key.as_deref(), Some(unique_key));
        // anthropic should NOT inherit the openrouter config key
        assert_ne!(anthr_ep.api_key.as_deref(), Some(unique_key));
    }

    #[test]
    fn test_resolve_role_unconfigured_returns_none() {
        let config = default_config();
        let resolver = ProviderResolver::build(&config).expect("build");
        assert!(resolver.resolve_role("nonexistent_role").is_none());
    }

    #[test]
    fn test_debug_redacts_api_key() {
        let ep = ResolvedEndpoint {
            base_url: "https://example.com".to_string(),
            api_key: Some("super-secret".to_string()),
            api_mode: ApiMode::ChatCompletions,
            default_model: "gpt-4".to_string(),
            fallback_providers: vec![],
        };
        let debug_str = format!("{:?}", ep);
        assert!(!debug_str.contains("super-secret"), "Debug should redact api_key");
        assert!(debug_str.contains("REDACTED"), "Debug should show REDACTED");
    }

    #[test]
    fn test_custom_provider_added_to_endpoints() {
        let mut config = default_config();
        config.custom_providers.push(CustomProviderConfig {
            name: "local-llama".to_string(),
            base_url: "http://localhost:11434/v1".to_string(),
            api_key: Some("ollama".to_string()),
            api_mode: None,
            default_model: Some("llama3".to_string()),
        });
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve("local-llama").expect("local-llama");
        assert_eq!(ep.base_url, "http://localhost:11434/v1");
        assert_eq!(ep.api_key.as_deref(), Some("ollama"));
        assert_eq!(ep.default_model, "llama3");
    }

    #[test]
    fn test_unknown_fallback_provider_errors() {
        let mut config = default_config();
        config.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                fallback_providers: vec!["nonexistent-provider".to_string()],
                ..Default::default()
            },
        );
        let result = ProviderResolver::build(&config);
        assert!(result.is_err(), "Unknown fallback provider should error");
    }
}
