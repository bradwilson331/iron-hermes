//! The single shared "which env var does this provider's API key live in"
//! resolver (Phase 51 Plan 15, CR-04).
//!
//! Before this module existed, this exact question was answered independently
//! in THREE places: `ironhermes-cli::profile_migrate` (two-tier: an explicit
//! `providers.<name>.api_key_env` override, falling back to a local
//! `BUILTIN_PROVIDER_ENV_VARS` table for the three bundled provider names),
//! `ironhermes-cli::worker_bootstrap` (one-tier: the explicit override ONLY,
//! with no built-in fallback), and `ironhermes-core::dispatch_gate` (its own
//! private `BUILTIN_PROVIDERS` name-only list, used just to decide whether a
//! provider is "known"). For the very common shape `model.provider: openrouter`
//! with no `providers:` block at all, the migration was willing to scrub a
//! profile's `.env` that the worker bootstrap could never actually consume —
//! the migration migrated a strictly larger set of profiles than the worker
//! could dispatch. This module is the one place that question is answered now;
//! every caller above delegates to [`provider_api_key_env_name`] instead of
//! keeping its own copy of the rule or the table.
//!
//! The two-tier rule mirrors `provider.rs`'s own priority-3 legacy fallback
//! (`ProviderResolver::build_with_env_overrides_strict`'s priority chain) —
//! this function and that resolver must agree, or the dispatch gate and the
//! runtime credential bootstrap will disagree again, just one layer down.

use crate::config::Config;

/// The three provider names that get a built-in legacy env-var name even with
/// no explicit `providers:` entry — the single source of truth `dispatch_gate`
/// now reads from instead of keeping its own copy.
pub const BUILTIN_PROVIDER_ENV_VARS: &[(&str, &str)] = &[
    ("openrouter", "OPENROUTER_API_KEY"),
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
];

/// Resolve the single env-var name `provider`'s API key would live under.
///
/// - An explicit `providers.<provider>.api_key_env` wins, for ANY provider
///   name (including one of the three built-ins, if the operator overrode
///   it).
/// - With no explicit `providers:` entry, one of the three built-in legacy
///   provider names resolves to its legacy variable.
/// - An unknown provider — one with no `providers:` entry, not a built-in
///   name, and not (or not usefully, since [`crate::config::CustomProviderConfig`]
///   has no `api_key_env` field at all) a `custom_providers` entry — resolves
///   to `None`. The caller decides what "nothing to migrate" / "nothing to
///   install the credential into" means for its own context; this function
///   only ever answers the naming question.
pub fn provider_api_key_env_name(config: &Config, provider: &str) -> Option<String> {
    if let Some(env_name) = config
        .providers
        .get(provider)
        .and_then(|c| c.api_key_env.as_ref())
    {
        return Some(env_name.clone());
    }
    BUILTIN_PROVIDER_ENV_VARS
        .iter()
        .find(|(name, _)| *name == provider)
        .map(|(_, env_var)| env_var.to_string())
}

/// `true` when `provider` is one of the three bundled legacy names — the
/// single source [`crate::dispatch_gate`]'s "is this a known provider" checks
/// now read, replacing its own private `BUILTIN_PROVIDERS` array.
pub fn is_builtin_provider(provider: &str) -> bool {
    BUILTIN_PROVIDER_ENV_VARS
        .iter()
        .any(|(name, _)| *name == provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CustomProviderConfig, ProviderConfig};

    #[test]
    fn explicit_override_wins_for_any_provider_name() {
        let mut config = Config::default();
        config.providers.insert(
            "openrouter".to_string(),
            ProviderConfig {
                api_key_env: Some("CUSTOM_OPENROUTER_KEY".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            provider_api_key_env_name(&config, "openrouter"),
            Some("CUSTOM_OPENROUTER_KEY".to_string())
        );

        // Also wins for a name that is NOT one of the three built-ins.
        let mut config = Config::default();
        config.providers.insert(
            "moonshot".to_string(),
            ProviderConfig {
                api_key_env: Some("MOONSHOT_API_KEY".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            provider_api_key_env_name(&config, "moonshot"),
            Some("MOONSHOT_API_KEY".to_string())
        );
    }

    #[test]
    fn builtin_names_resolve_with_no_providers_entry() {
        let config = Config::default();
        assert_eq!(
            provider_api_key_env_name(&config, "openrouter"),
            Some("OPENROUTER_API_KEY".to_string())
        );
        assert_eq!(
            provider_api_key_env_name(&config, "anthropic"),
            Some("ANTHROPIC_API_KEY".to_string())
        );
        assert_eq!(
            provider_api_key_env_name(&config, "openai"),
            Some("OPENAI_API_KEY".to_string())
        );
    }

    #[test]
    fn unknown_provider_resolves_to_none() {
        let config = Config::default();
        assert_eq!(
            provider_api_key_env_name(&config, "totally-unknown-provider"),
            None
        );
    }

    #[test]
    fn custom_provider_with_no_api_key_env_field_resolves_to_none() {
        let mut config = Config::default();
        config.custom_providers.push(CustomProviderConfig {
            name: "mycustom".to_string(),
            base_url: "https://example.invalid".to_string(),
            api_key: None,
            api_mode: None,
            default_model: None,
        });
        assert_eq!(provider_api_key_env_name(&config, "mycustom"), None);
    }

    #[test]
    fn is_builtin_provider_matches_the_shared_table() {
        assert!(is_builtin_provider("openrouter"));
        assert!(is_builtin_provider("anthropic"));
        assert!(is_builtin_provider("openai"));
        assert!(!is_builtin_provider("mycustom"));
    }
}
