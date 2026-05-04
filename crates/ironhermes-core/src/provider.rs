use anyhow::{Result, anyhow};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::{ApiMode, Config, ModelRoleConfig};
use crate::constants::{ANTHROPIC_BASE_URL, DEFAULT_CONTEXT_LENGTH, OPENROUTER_BASE_URL};
use crate::model_metadata::{ModelMetadata, ModelRegistry};
use crate::models_cache::ModelsCache;

// =============================================================================
// Deprecation banner once-only emission (D-12, D-13, Phase 26)
// =============================================================================

/// Process-wide set of provider names that have already emitted the legacy
/// env-var deprecation banner (D-12). Insert returns false if already present,
/// so the banner fires exactly once per provider name per process.
///
/// Keyed by provider name for D-12 and by a special key `"__model_api_key__"`
/// for the D-13 `config.model.api_key` banner.
fn legacy_warned() -> &'static Mutex<HashSet<String>> {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Emit the D-12 / D-13 deprecation banner exactly once per key.
/// `banner_key` uniquely identifies this warning so it is never repeated.
fn emit_deprecation_once(banner_key: &str, message: &str) {
    let mut warned = legacy_warned().lock().unwrap_or_else(|p| p.into_inner());
    if warned.insert(banner_key.to_string()) {
        eprintln!("{}", message);
    }
}

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
    pub model_metadata: Option<ModelMetadata>, // Phase 21.3 D-14
    pub config_context_length: Option<usize>,  // Phase 21.3 D-06
}

impl ResolvedEndpoint {
    /// Returns the context_length with D-06 precedence:
    /// 1. User config.yaml context_length (if set) — always wins
    /// 2. Model metadata context_length (from cache or static table)
    /// 3. DEFAULT_CONTEXT_LENGTH (128K) as last resort
    pub fn context_length(&self) -> usize {
        // D-06: user config always wins
        if let Some(config_len) = self.config_context_length {
            return config_len;
        }
        // Then model metadata (cache > static, handled by ModelRegistry lookup order)
        self.model_metadata
            .as_ref()
            .map(|m| m.context_length)
            .unwrap_or(DEFAULT_CONTEXT_LENGTH)
    }
}

impl fmt::Debug for ResolvedEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedEndpoint")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_deref().map(|_| "[REDACTED]"))
            .field("api_mode", &self.api_mode)
            .field("default_model", &self.default_model)
            .field("fallback_providers", &self.fallback_providers)
            .field("model_metadata", &self.model_metadata)
            .field("config_context_length", &self.config_context_length)
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
    model_registry: ModelRegistry, // Phase 21.3
    /// Resolved auxiliary endpoint (D-05/D-06, Phase 26).
    /// `None` when no `auxiliary:` block is configured — callers fall through to main.
    auxiliary_endpoint: Option<ResolvedEndpoint>,
}

impl ProviderResolver {
    /// Build the resolver from the application config.
    ///
    /// # Errors
    /// - Unknown `fallback_providers` entries
    /// - Custom provider with non-https base_url (unless localhost/127.0.0.1)
    /// - `auxiliary.provider` references an unknown provider name (D-10)
    /// - Main provider is disabled (D-14)
    pub fn build(config: &Config) -> Result<Self> {
        let mut endpoints: HashMap<String, ResolvedEndpoint> = HashMap::new();
        let mut model_registry = ModelRegistry::new();
        let disk_cache = ModelsCache::load();
        model_registry.merge_cache(disk_cache.into_metadata_map());
        let config_context_length = config.model.context_length;

        // --- 1. Pre-populate three built-in providers with defaults ---
        endpoints.insert(
            "openrouter".to_string(),
            ResolvedEndpoint {
                base_url: OPENROUTER_BASE_URL.to_string(),
                api_key: None,
                api_mode: ApiMode::ChatCompletions,
                default_model: config.model.default.clone(),
                fallback_providers: vec![],
                model_metadata: None,
                config_context_length: None,
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
                model_metadata: None,
                config_context_length: None,
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
                model_metadata: None,
                config_context_length: None,
            },
        );

        // --- 2. Overlay config.providers entries (user config overrides defaults) ---
        // D-14: Skip providers with `disabled: true` — they are excluded from the
        // resolver entry map entirely. A disabled main provider errors at step 2b.
        for (name, prov_cfg) in &config.providers {
            // D-14: skip disabled providers
            if prov_cfg.disabled == Some(true) {
                // Remove any pre-populated built-in entry for this name
                endpoints.remove(name.as_str());
                continue;
            }
            let entry = endpoints
                .entry(name.clone())
                .or_insert_with(|| ResolvedEndpoint {
                    base_url: String::new(),
                    api_key: None,
                    api_mode: ApiMode::ChatCompletions,
                    default_model: String::new(),
                    fallback_providers: vec![],
                    model_metadata: None,
                    config_context_length: None,
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

        // --- 2b. D-14: Validate main provider is not disabled ---
        let main = &config.model.provider;
        if !endpoints.contains_key(main.as_str()) {
            // Check if explicitly disabled vs genuinely unknown
            let is_disabled = config
                .providers
                .get(main.as_str())
                .and_then(|p| p.disabled)
                .unwrap_or(false);
            if is_disabled {
                return Err(anyhow!(
                    "main provider '{}' is disabled — re-enable it with `hermes provider enable {}` or change model.provider in config.yaml",
                    main,
                    main
                ));
            }
            // Unknown main provider will be caught at resolve_for_main() time; allow build to succeed
            // so operators can introspect with `hermes provider list` even if main is misconfigured.
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
                    model_metadata: None,
                    config_context_length: None,
                },
            );
        }

        // --- 4. Resolve API keys with precedence (D-03, PROV-03, PROV-04, D-11, D-12, D-13) ---
        for (name, endpoint) in endpoints.iter_mut() {
            // Priority 1: api_key_env from config.providers[name] (D-01 / D-04)
            let api_key_env_key: Option<String> = config
                .providers
                .get(name.as_str())
                .and_then(|p| p.api_key_env.as_deref())
                .and_then(|env_name| std::env::var(env_name).ok());

            // Priority 2 (deprecated): api_key literal from config.providers[name] (D-01 / Pitfall 5)
            let config_literal_key: Option<String> = config
                .providers
                .get(name.as_str())
                .and_then(|p| p.api_key.clone());
            if let Some(ref _key) = config_literal_key {
                emit_deprecation_once(
                    &format!("__config_api_key__{}", name),
                    &format!(
                        "[provider:{}] config.providers.{}.api_key is deprecated — use api_key_env instead",
                        name, name
                    ),
                );
            }

            // Priority 3: legacy built-in env vars (D-12)
            // Only for the three canonical built-in provider names; custom providers get None (D-11).
            let legacy_env_key: Option<String> = match name.as_str() {
                "openrouter" => {
                    let val = std::env::var("OPENROUTER_API_KEY").ok();
                    if val.is_some() {
                        let prov_cfg = config.providers.get("openrouter");
                        let has_explicit = prov_cfg.and_then(|p| p.api_key_env.as_ref()).is_some()
                            || prov_cfg.and_then(|p| p.api_key.as_ref()).is_some();
                        if !has_explicit {
                            emit_deprecation_once(
                                "legacy_env_openrouter",
                                "[provider:openrouter] using deprecated env var OPENROUTER_API_KEY — set providers.openrouter.api_key_env in config.yaml to silence this warning",
                            );
                        }
                    }
                    val
                }
                "anthropic" => {
                    let val = std::env::var("ANTHROPIC_API_KEY").ok();
                    if val.is_some() {
                        let prov_cfg = config.providers.get("anthropic");
                        let has_explicit = prov_cfg.and_then(|p| p.api_key_env.as_ref()).is_some()
                            || prov_cfg.and_then(|p| p.api_key.as_ref()).is_some();
                        if !has_explicit {
                            emit_deprecation_once(
                                "legacy_env_anthropic",
                                "[provider:anthropic] using deprecated env var ANTHROPIC_API_KEY — set providers.anthropic.api_key_env in config.yaml to silence this warning",
                            );
                        }
                    }
                    val
                }
                "openai" => {
                    let val = std::env::var("OPENAI_API_KEY").ok();
                    if val.is_some() {
                        let prov_cfg = config.providers.get("openai");
                        let has_explicit = prov_cfg.and_then(|p| p.api_key_env.as_ref()).is_some()
                            || prov_cfg.and_then(|p| p.api_key.as_ref()).is_some();
                        if !has_explicit {
                            emit_deprecation_once(
                                "legacy_env_openai",
                                "[provider:openai] using deprecated env var OPENAI_API_KEY — set providers.openai.api_key_env in config.yaml to silence this warning",
                            );
                        }
                    }
                    val
                }
                // D-11: custom providers get None — no implicit cross-provider fallback
                _ => None,
            };

            // Priority 4 (deprecated): config.model.api_key for the main provider only (D-13)
            let model_key: Option<String> = if name == main {
                let key = config.model.api_key.clone();
                if key.is_some() {
                    emit_deprecation_once(
                        "__model_api_key__",
                        "[config:model.api_key] deprecated — set providers.<main-provider>.api_key_env instead",
                    );
                }
                key
            } else {
                None
            };

            endpoint.api_key = api_key_env_key
                .or(config_literal_key)
                .or(legacy_env_key)
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

        // --- 6. Populate model_metadata and config_context_length (Phase 21.3) ---
        for endpoint in endpoints.values_mut() {
            endpoint.model_metadata = model_registry.lookup(&endpoint.default_model).cloned();
            endpoint.config_context_length = config_context_length;
        }

        // --- 7. Store roles ---
        let roles = config.model.roles.clone();

        // --- 8. Build auxiliary endpoint (D-05/D-06/D-10, Phase 26) ---
        // If config.auxiliary is set, resolve the named provider and apply the auxiliary model.
        // Fail fast if auxiliary.provider references an unknown name (D-10 / Pitfall 3).
        let auxiliary_endpoint: Option<ResolvedEndpoint> = if config.auxiliary.is_set() {
            let aux_provider_name = &config.auxiliary.provider;
            let base = endpoints.get(aux_provider_name.as_str()).ok_or_else(|| {
                anyhow!(
                    "auxiliary.provider '{}' is not a known provider — define it in providers: first",
                    aux_provider_name
                )
            })?;
            let mut aux_ep = base.clone();
            if !config.auxiliary.model.is_empty() {
                aux_ep.default_model = config.auxiliary.model.clone();
            }
            Some(aux_ep)
        } else {
            None
        };

        Ok(Self {
            endpoints,
            roles,
            main_provider: main.clone(),
            model_registry,
            auxiliary_endpoint,
        })
    }

    /// Direct lookup by provider name.
    pub fn resolve(&self, provider: &str) -> Option<&ResolvedEndpoint> {
        self.endpoints.get(provider)
    }

    /// Resolve the main provider. Panics if missing (startup validation prevents this).
    pub fn resolve_for_main(&self) -> &ResolvedEndpoint {
        self.endpoints.get(&self.main_provider).unwrap_or_else(|| {
            panic!(
                "main provider '{}' not found in endpoints",
                self.main_provider
            )
        })
    }

    /// Resolve an auxiliary model role (D-05, D-07, PROV-06, Phase 26).
    ///
    /// Three-level cascade (D-05):
    /// 1. If `config.model.roles[role]` is set → use that per-task override.
    /// 2. Else if `config.auxiliary` is set → use the auxiliary block.
    /// 3. Else → return `None` (caller falls through to `resolve_for_main()`).
    ///
    /// When `role_cfg.provider == "main"` the main provider's endpoint is used
    /// with the role's optional model override.
    pub fn resolve_role(&self, role: &str) -> Option<ResolvedEndpoint> {
        // Level 1: per-task override from config.model.roles
        if let Some(role_cfg) = self.roles.get(role) {
            let base_endpoint = if role_cfg.provider == "main" {
                self.endpoints.get(&self.main_provider)?
            } else {
                self.endpoints.get(&role_cfg.provider)?
            };
            let mut ep = base_endpoint.clone();
            if let Some(ref model) = role_cfg.model {
                ep.default_model = model.clone();
            }
            return Some(ep);
        }

        // Level 2: auxiliary block fallback (D-06: optional, may be None)
        if let Some(ref aux) = self.auxiliary_endpoint {
            return Some(aux.clone());
        }

        // Level 3: not configured — caller uses resolve_for_main()
        None
    }

    /// Get the main provider name.
    pub fn main_provider(&self) -> &str {
        &self.main_provider
    }

    /// Get a reference to the model registry (Phase 21.3).
    pub fn model_registry(&self) -> &ModelRegistry {
        &self.model_registry
    }
}

// =============================================================================
// SummarizationClientHandle — dependency-inversion trait (Phase 25.2 D-13)
// =============================================================================

/// Phase 25.2 D-13: dependency-inversion handle for the summarization aux-LLM call path.
///
/// Mirrors Phase 25.1's `VisionClientHandle` (crates/ironhermes-tools/src/browser_vision.rs:51).
///
/// # Why this trait lives in `ironhermes-core`
///
/// `WebExtractTool` (in `ironhermes-tools`) needs to invoke an LLM via the Phase 26
/// `resolve_role("summarization")` cascade implemented in `ironhermes-agent`. A direct
/// `tools → agent` import would create a circular dependency (agent already depends on tools).
/// By defining the contract in `core` — which both `tools` and `agent` already depend on —
/// the consumer (`WebExtractTool`) holds an `Arc<dyn SummarizationClientHandle>` and the
/// implementation lives in `ironhermes-agent`, wired in at tool-registration time.
///
/// # Cascade contract (implementer's responsibility)
///
/// The implementation MUST call [`ProviderResolver::resolve_role`] with `"summarization"` first;
/// on `None`, it MUST fall back to [`ProviderResolver::resolve_for_main`]. The `WebExtractTool`
/// consumer never sees `None` — it always receives a usable response or an `Err`.
///
/// # Parameters
/// * `system_prompt` — system message for the summarization role.
/// * `user_prompt` — the content (or chunk) to summarize, prefixed with any per-call context.
/// * `max_tokens` — output token cap (caller chooses based on tier: tier 2 ≈ 20_000, tier 3 chunk ≈ 5_000, tier 3 synthesis ≈ 20_000).
///
/// # Returns
/// Assistant text content on success; `Err` on any failure (transport, model, parsing).
/// Errors propagate up to `WebExtractTool::execute` and become per-URL `ExtractionResult.error` entries.
#[async_trait::async_trait]
pub trait SummarizationClientHandle: Send + Sync {
    async fn summarize_call(
        &self,
        system_prompt: String,
        user_prompt: String,
        max_tokens: u32,
    ) -> anyhow::Result<String>;
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuxiliaryConfig, CustomProviderConfig, ModelRoleConfig, ProviderConfig};

    // =========================================================================
    // env_lock: process-wide mutex for tests that mutate environment variables.
    // Required for all tests using std::env::set_var / remove_var (Rust 2024
    // edition: unsafe), per RESEARCH.md §"Sampling Strategy for env-var Sensitive Tests".
    // =========================================================================
    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    fn default_config() -> Config {
        Config::default()
    }

    #[test]
    fn test_build_default_config_has_three_providers() {
        let config = default_config();
        let resolver = ProviderResolver::build(&config).expect("build should succeed");
        assert!(
            resolver.resolve("openrouter").is_some(),
            "openrouter should exist"
        );
        assert!(
            resolver.resolve("anthropic").is_some(),
            "anthropic should exist"
        );
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
        let ep = resolver
            .resolve_role("vision")
            .expect("vision role via main");
        // Should return the openrouter endpoint (main provider)
        assert_eq!(ep.base_url, OPENROUTER_BASE_URL);
        assert_eq!(ep.default_model, "openai/gpt-4o-vision");
    }

    #[test]
    fn test_api_key_config_takes_precedence_over_env() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let mut config = default_config();
        config.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                api_key: Some("config-key".to_string()),
                ..Default::default()
            },
        );

        // Set env var — config key should win
        // SAFETY: test-only env var mutation, held behind env_lock
        unsafe {
            std::env::set_var("OPENROUTER_API_KEY", "env-key");
        }

        // We test by setting config.providers key and verifying the resolved key
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve("openrouter").expect("openrouter");
        assert_eq!(ep.api_key.as_deref(), Some("config-key"));

        // SAFETY: test-only cleanup
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
        }
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
            model_metadata: None,
            config_context_length: None,
        };
        let debug_str = format!("{:?}", ep);
        assert!(
            !debug_str.contains("super-secret"),
            "Debug should redact api_key"
        );
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

    // =========================================================================
    // Phase 21.3: model_metadata and context_length tests
    // =========================================================================

    fn default_endpoint() -> ResolvedEndpoint {
        ResolvedEndpoint {
            base_url: "https://example.com".to_string(),
            api_key: None,
            api_mode: ApiMode::ChatCompletions,
            default_model: "test-model".to_string(),
            fallback_providers: vec![],
            model_metadata: None,
            config_context_length: None,
        }
    }

    #[test]
    fn test_resolved_endpoint_context_length_from_metadata() {
        use crate::model_metadata::{ModelCapabilities, ModelMetadata};

        // With metadata, no config override
        let ep = ResolvedEndpoint {
            model_metadata: Some(ModelMetadata {
                context_length: 200_000,
                max_output_tokens: Some(64_000),
                tokenizer: "cl100k_base".to_string(),
                capabilities: ModelCapabilities::default(),
            }),
            config_context_length: None,
            ..default_endpoint()
        };
        assert_eq!(ep.context_length(), 200_000);

        // Without metadata — falls back to DEFAULT_CONTEXT_LENGTH
        let ep2 = ResolvedEndpoint {
            model_metadata: None,
            config_context_length: None,
            ..default_endpoint()
        };
        assert_eq!(
            ep2.context_length(),
            crate::constants::DEFAULT_CONTEXT_LENGTH
        );
    }

    #[test]
    fn test_user_config_context_length_overrides_metadata() {
        use crate::model_metadata::{ModelCapabilities, ModelMetadata};

        // D-06: config.yaml context_length > metadata context_length
        let ep = ResolvedEndpoint {
            model_metadata: Some(ModelMetadata {
                context_length: 200_000,
                max_output_tokens: Some(64_000),
                tokenizer: "cl100k_base".to_string(),
                capabilities: ModelCapabilities::default(),
            }),
            config_context_length: Some(1_000_000), // User set 1M in config.yaml
            ..default_endpoint()
        };
        assert_eq!(
            ep.context_length(),
            1_000_000,
            "D-06: user config must override metadata"
        );
    }

    #[test]
    fn test_provider_resolver_populates_model_metadata() {
        let config = default_config();
        let resolver = ProviderResolver::build(&config).unwrap();
        let ep = resolver.resolve_for_main();
        // Default model is "anthropic/claude-sonnet-4" — should resolve to claude-sonnet-4 metadata
        assert!(
            ep.model_metadata.is_some(),
            "main endpoint should have model_metadata"
        );
        assert_eq!(ep.context_length(), 1_000_000);
    }

    #[test]
    fn test_provider_resolver_has_model_registry() {
        let config = default_config();
        let resolver = ProviderResolver::build(&config).unwrap();
        // model_registry accessor should work
        let reg = resolver.model_registry();
        assert!(reg.lookup("claude-sonnet-4").is_some());
    }

    // =========================================================================
    // Phase 21.3 Plan 05: Disk cache auto-load regression tests
    // (env_lock added in Plan 26-02 to fix pre-existing parallelism flake)
    // =========================================================================

    #[test]
    fn provider_resolver_loads_disk_cache_at_build() {
        use crate::model_metadata::{ModelCapabilities, ModelMetadata};
        use crate::models_cache::{ModelsCache, ModelsCacheEntry};
        use chrono::Utc;

        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

        let tmp = tempfile::tempdir().unwrap();
        let mut cache = ModelsCache::default();
        cache.entries.insert(
            "test-cache-only-model".to_string(),
            ModelsCacheEntry {
                metadata: ModelMetadata {
                    context_length: 500_000,
                    max_output_tokens: Some(8_000),
                    tokenizer: "cl100k_base".to_string(),
                    capabilities: ModelCapabilities::default(),
                },
                fetched_at: Utc::now(),
            },
        );
        cache
            .save_to(&tmp.path().join("models-cache.json"))
            .unwrap();

        // SAFETY: test-only env var mutation, held behind env_lock
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }

        let mut config = default_config();
        config.model.default = "test-cache-only-model".to_string();
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve_for_main();

        assert!(
            ep.model_metadata.is_some(),
            "cache-only model should have metadata"
        );
        assert_eq!(ep.model_metadata.as_ref().unwrap().context_length, 500_000);

        // SAFETY: test-only cleanup
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn provider_resolver_cache_overrides_static_for_same_model() {
        use crate::model_metadata::{ModelCapabilities, ModelMetadata};
        use crate::models_cache::{ModelsCache, ModelsCacheEntry};
        use chrono::Utc;

        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

        let tmp = tempfile::tempdir().unwrap();
        let mut cache = ModelsCache::default();
        cache.entries.insert(
            "claude-sonnet-4".to_string(),
            ModelsCacheEntry {
                metadata: ModelMetadata {
                    context_length: 999_999,
                    max_output_tokens: Some(100_000),
                    tokenizer: "cl100k_base".to_string(),
                    capabilities: ModelCapabilities::default(),
                },
                fetched_at: Utc::now(),
            },
        );
        cache
            .save_to(&tmp.path().join("models-cache.json"))
            .unwrap();

        // SAFETY: test-only env var mutation, held behind env_lock
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }

        let mut config = default_config();
        config.model.default = "claude-sonnet-4".to_string();
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver.resolve_for_main();

        assert_eq!(
            ep.context_length(),
            999_999,
            "cache entry should override static table (200K) for same model"
        );

        // SAFETY: test-only cleanup
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    // =========================================================================
    // Phase 26 Plan 02: PROV-04 leak fix (D-11), D-21 unit test
    // =========================================================================

    /// D-21: PROV-04 lib-level confirmation — OPENAI_API_KEY must NOT leak to
    /// a custom provider that has no api_key_env configured.
    #[test]
    fn legacy_openai_key_does_not_leak_to_unknown_provider() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

        // SAFETY: test-only env var mutation, held behind env_lock
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-leaked");
        }

        let mut config = default_config();
        config.providers.insert(
            "my-local-llm".to_string(),
            ProviderConfig {
                base_url: Some("http://localhost:8080/v1".to_string()),
                api_key_env: None, // explicitly unset
                ..Default::default()
            },
        );
        let resolver = ProviderResolver::build(&config).expect("build");
        let endpoint = resolver
            .resolve("my-local-llm")
            .expect("my-local-llm should exist");
        assert_eq!(
            endpoint.api_key, None,
            "OPENAI_API_KEY MUST NOT leak to my-local-llm — D-11 PROV-04 regression"
        );

        // SAFETY: test-only cleanup
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        }
    }

    /// D-11 variant: api_key_env on a custom provider resolves its own env var,
    /// not OPENAI_API_KEY.
    #[test]
    fn custom_provider_api_key_env_resolves_own_var() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());

        // SAFETY: test-only env var mutation, held behind env_lock
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-should-not-leak");
            std::env::set_var("MY_LLM_KEY", "my-custom-key");
        }

        let mut config = default_config();
        config.providers.insert(
            "my-local-llm".to_string(),
            ProviderConfig {
                base_url: Some("http://localhost:8080/v1".to_string()),
                api_key_env: Some("MY_LLM_KEY".to_string()),
                ..Default::default()
            },
        );
        let resolver = ProviderResolver::build(&config).expect("build");
        let endpoint = resolver
            .resolve("my-local-llm")
            .expect("my-local-llm should exist");
        assert_eq!(
            endpoint.api_key.as_deref(),
            Some("my-custom-key"),
            "api_key_env should resolve MY_LLM_KEY, not OPENAI_API_KEY"
        );

        // SAFETY: test-only cleanup
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("MY_LLM_KEY");
        }
    }

    // =========================================================================
    // Phase 26 Plan 02: D-14 disabled gate
    // =========================================================================

    #[test]
    fn disabled_provider_excluded_from_resolver() {
        let mut config = default_config();
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                disabled: Some(true),
                ..Default::default()
            },
        );
        let resolver = ProviderResolver::build(&config).expect("build");
        assert!(
            resolver.resolve("openai").is_none(),
            "disabled provider must not appear in resolver"
        );
        // Other providers still present
        assert!(resolver.resolve("anthropic").is_some());
        assert!(resolver.resolve("openrouter").is_some());
    }

    #[test]
    fn disabled_main_provider_errors_at_build() {
        let mut config = default_config();
        // Default main provider is "openrouter"
        config.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                disabled: Some(true),
                ..Default::default()
            },
        );
        let result = ProviderResolver::build(&config);
        assert!(
            result.is_err(),
            "Disabled main provider must error at build"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("disabled"),
            "Error message must mention 'disabled'"
        );
    }

    // =========================================================================
    // Phase 26 Plan 02: D-05/D-07 resolve_role 3-level cascade
    // =========================================================================

    /// D-05 cascade level 1: per-task override in model.roles wins over auxiliary.
    #[test]
    fn resolve_role_per_task_override_wins() {
        let mut config = default_config();
        // Per-task override for "vision" → openai
        config.model.roles.insert(
            "vision".to_string(),
            ModelRoleConfig {
                provider: "openai".to_string(),
                model: Some("gpt-4o-vision".to_string()),
            },
        );
        // auxiliary set to openrouter (should NOT be used)
        config.auxiliary = AuxiliaryConfig {
            provider: "openrouter".to_string(),
            model: "openrouter/aux-model".to_string(),
        };
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver
            .resolve_role("vision")
            .expect("vision must resolve");
        assert_eq!(
            ep.base_url, "https://api.openai.com/v1",
            "per-task override must win"
        );
        assert_eq!(ep.default_model, "gpt-4o-vision");
    }

    /// D-05 cascade level 2: no per-task override → falls through to auxiliary.
    #[test]
    fn resolve_role_falls_through_to_auxiliary() {
        let mut config = default_config();
        // No per-task role set for "compression"
        // auxiliary set to openai
        config.auxiliary = AuxiliaryConfig {
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
        };
        let resolver = ProviderResolver::build(&config).expect("build");
        let ep = resolver
            .resolve_role("compression")
            .expect("compression must fall through to aux");
        assert_eq!(
            ep.base_url, "https://api.openai.com/v1",
            "auxiliary must be used"
        );
        assert_eq!(ep.default_model, "gpt-4o-mini");
    }

    /// D-05 cascade level 3: no per-task, no auxiliary → returns None.
    #[test]
    fn resolve_role_returns_none_when_no_role_set() {
        let config = default_config();
        // No roles, no auxiliary
        let resolver = ProviderResolver::build(&config).expect("build");
        let result = resolver.resolve_role("compression");
        assert!(
            result.is_none(),
            "must return None when neither per-task nor auxiliary is configured"
        );
    }

    // =========================================================================
    // Phase 26 Plan 02: D-10 auxiliary.provider validation
    // =========================================================================

    #[test]
    fn auxiliary_provider_unknown_name_fails_build() {
        let mut config = default_config();
        config.auxiliary = AuxiliaryConfig {
            provider: "nonexistent-provider".to_string(),
            model: "some-model".to_string(),
        };
        let result = ProviderResolver::build(&config);
        assert!(
            result.is_err(),
            "Unknown auxiliary.provider must fail build"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("nonexistent-provider"),
            "Error message must identify the unknown provider"
        );
    }

    #[test]
    fn auxiliary_provider_known_name_builds_successfully() {
        let mut config = default_config();
        config.auxiliary = AuxiliaryConfig {
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
        };
        let resolver =
            ProviderResolver::build(&config).expect("Known auxiliary.provider must build");
        // Verify the auxiliary endpoint is used for an unconfigured role
        let ep = resolver
            .resolve_role("session_search")
            .expect("should fall through to auxiliary");
        assert_eq!(ep.base_url, "https://api.openai.com/v1");
        assert_eq!(ep.default_model, "gpt-4o-mini");
    }

    // =========================================================================
    // Phase 25.2 Plan 03 (D-13): SummarizationClientHandle dyn-compatibility lock
    // =========================================================================

    #[test]
    fn summarization_client_handle_is_dyn_compatible() {
        // Compile-only: ensures the trait can be made into a trait object.
        // Without Send + Sync, Arc<dyn SummarizationClientHandle> would not be valid.
        fn _accepts(_: std::sync::Arc<dyn super::SummarizationClientHandle>) {}
    }
}
