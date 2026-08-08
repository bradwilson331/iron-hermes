//! Phase 46.8 Plan 04 — `ProviderResolver::apply_vault_fallback` precedence + hard-error tests.
//!
//! D-07: the vault is resolution priority 5, consulted ONLY after the existing 4-priority
//! chain (api_key_env → deprecated literal → legacy env vars → deprecated model.api_key) all
//! miss. It never overrides an endpoint that already has an api_key (precedence proof), and a
//! sealed/corrupt vault (`get_secret` returns `Err`) must propagate LOUDLY — never silently
//! degrade to "no api key configured" (D-07 hard-error).
//!
//! `MockStore` is a test-only `SecretStore` backend — configurable per-key to return
//! `Ok(Some(secret))`, `Ok(None)`, or `Err(sealed)`. Resolvers are built via the REAL
//! `ProviderResolver::build_with_cache` using `custom_providers` (whose `api_key` is set
//! directly from config, D-01) so these tests never touch process-global environment
//! variables and need no `env_lock` (contrast with `provider.rs`'s own env-var tests).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ironhermes_core::config::{Config, CustomProviderConfig, ProviderConfig};
use ironhermes_core::models_cache::ModelsCache;
use ironhermes_core::provider::ProviderResolver;
use ironhermes_vault::SecretStore;
use secrecy::SecretString;

// =============================================================================
// MockStore — test-only SecretStore backend
// =============================================================================

#[derive(Clone)]
enum MockResponse {
    Found(String),
    NotFound,
    Sealed,
}

struct MockStore {
    responses: HashMap<String, MockResponse>,
    /// Tracks how many times `get_secret` was called, and with which keys.
    calls: Arc<std::sync::Mutex<Vec<String>>>,
    call_count: Arc<AtomicUsize>,
}

impl MockStore {
    fn new(responses: HashMap<String, MockResponse>) -> Self {
        Self {
            responses,
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

#[async_trait::async_trait]
impl SecretStore for MockStore {
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(key.to_string());
        match self.responses.get(key) {
            Some(MockResponse::Found(v)) => Ok(Some(SecretString::from(v.clone()))),
            Some(MockResponse::NotFound) | None => Ok(None),
            Some(MockResponse::Sealed) => Err(anyhow::anyhow!(
                "vault is sealed — run `ironhermes vault unlock` to restore access"
            )),
        }
    }

    async fn put_secret(&self, _key: &str, _value: SecretString) -> anyhow::Result<()> {
        unimplemented!("apply_vault_fallback never calls put_secret")
    }

    async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
        unimplemented!("apply_vault_fallback never calls delete_secret")
    }

    async fn list_secrets(&self, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        unimplemented!("apply_vault_fallback never calls list_secrets")
    }
}

// =============================================================================
// Test fixtures — resolvers built via custom_providers (no env var mutation)
// =============================================================================

/// Build a `Config` with the three built-in providers disabled and one or more
/// `custom_providers` entries, so `ProviderResolver::build_with_cache` produces
/// endpoints ONLY from `entries`, each with the requested initial `api_key`.
/// This avoids any dependency on process-global environment variables.
fn config_with_custom_providers(entries: &[(&str, Option<&str>)]) -> Config {
    let mut config = Config::default();
    for name in ["openrouter", "anthropic", "openai"] {
        config.providers.insert(
            name.to_string(),
            ProviderConfig {
                disabled: Some(true),
                ..Default::default()
            },
        );
    }
    config.model.provider = entries
        .first()
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| "vaulttest-unused-main".to_string());
    for (name, key) in entries {
        config.custom_providers.push(CustomProviderConfig {
            name: name.to_string(),
            base_url: "https://vaulttest.example/v1".to_string(),
            api_key: key.map(|k| k.to_string()),
            api_mode: None,
            default_model: None,
        });
    }
    config
}

/// A config with zero providers at all — the three built-ins disabled, no
/// custom_providers, main provider pointed at an unknown name (allowed to
/// build per D-14's "unknown main caught at resolve_for_main() time" posture).
fn config_with_zero_providers() -> Config {
    config_with_custom_providers(&[])
}

fn build_resolver(config: &Config) -> ProviderResolver {
    ProviderResolver::build_with_cache(config, ModelsCache::default())
        .expect("build_with_cache should succeed for test fixture config")
}

// =============================================================================
// Behavior: precedence — priorities 1-4 always win over the vault
// =============================================================================

#[tokio::test]
async fn provider_resolver_vault_precedence() {
    // "already-present" has api_key=Some(...) from config (simulating priorities 1-4
    // having already won); "vault-only" has api_key=None so the vault should fill it.
    let config = config_with_custom_providers(&[
        ("already-present", Some("existing-key-from-config")),
        ("vault-only", None),
    ]);
    let mut resolver = build_resolver(&config);

    let mut responses = HashMap::new();
    responses.insert(
        "already-present".to_string(),
        MockResponse::Found("vault-key-should-never-be-used".to_string()),
    );
    responses.insert(
        "vault-only".to_string(),
        MockResponse::Found("vault-supplied-key".to_string()),
    );
    let store = MockStore::new(responses);

    resolver
        .apply_vault_fallback(&store)
        .await
        .expect("apply_vault_fallback should succeed");

    // Precedence proof: the already-set key is untouched, and the vault was never
    // consulted for that provider at all.
    assert_eq!(
        resolver
            .resolve("already-present")
            .unwrap()
            .api_key
            .as_deref(),
        Some("existing-key-from-config"),
        "an already-set api_key must never be overwritten by the vault (D-07)"
    );
    assert!(
        !store.calls().contains(&"already-present".to_string()),
        "vault get_secret must never be called for a provider that already has a key"
    );

    // The None endpoint gets filled from the vault (D-08 SecretString -> String boundary).
    assert_eq!(
        resolver.resolve("vault-only").unwrap().api_key.as_deref(),
        Some("vault-supplied-key"),
        "an endpoint with api_key=None must be filled from the vault"
    );
    assert!(store.calls().contains(&"vault-only".to_string()));
}

// =============================================================================
// Behavior: sealed/corrupt vault hard-errors, never silently degrades
// =============================================================================

#[tokio::test]
async fn provider_resolver_vault_sealed_hard_error() {
    let config = config_with_custom_providers(&[("sealed-provider", None)]);
    let mut resolver = build_resolver(&config);

    let mut responses = HashMap::new();
    responses.insert("sealed-provider".to_string(), MockResponse::Sealed);
    let store = MockStore::new(responses);

    let result = resolver.apply_vault_fallback(&store).await;

    let err = result.expect_err("a sealed/corrupt vault must return Err, never Ok");
    let message = format!("{err:#}");
    assert!(
        message.contains("vault unlock") || message.to_lowercase().contains("sealed"),
        "sealed-vault error message must be actionable (name `ironhermes vault unlock` or \
         mention 'sealed'), got: {message}"
    );
}

// =============================================================================
// Behavior: a healthy vault that simply lacks the key is NOT an error
// =============================================================================

#[tokio::test]
async fn provider_resolver_vault_ok_none_is_not_error() {
    let config = config_with_custom_providers(&[("absent-in-vault", None)]);
    let mut resolver = build_resolver(&config);

    let mut responses = HashMap::new();
    responses.insert("absent-in-vault".to_string(), MockResponse::NotFound);
    let store = MockStore::new(responses);

    resolver
        .apply_vault_fallback(&store)
        .await
        .expect("Ok(None) from the store must not be treated as an error");

    assert_eq!(
        resolver.resolve("absent-in-vault").unwrap().api_key,
        None,
        "endpoint must stay None when the vault healthily reports the key absent"
    );
}

// =============================================================================
// Behavior: zero endpoints is a true no-op — zero vault calls
// =============================================================================

#[tokio::test]
async fn provider_resolver_vault_zero_endpoints_is_noop() {
    let config = config_with_zero_providers();
    let mut resolver = build_resolver(&config);

    let store = MockStore::new(HashMap::new());

    resolver
        .apply_vault_fallback(&store)
        .await
        .expect("zero endpoints must be a no-op Ok(())");

    assert_eq!(
        store.call_count(),
        0,
        "a resolver with zero endpoints must make zero vault calls"
    );
}

// =============================================================================
// Behavior: all-keys-already-present makes zero vault calls (D-07 empty case)
// =============================================================================

#[tokio::test]
async fn provider_resolver_vault_all_keys_present_makes_zero_calls() {
    let config =
        config_with_custom_providers(&[("already-a", Some("key-a")), ("already-b", Some("key-b"))]);
    let mut resolver = build_resolver(&config);

    let store = MockStore::new(HashMap::new());

    resolver
        .apply_vault_fallback(&store)
        .await
        .expect("all-keys-present must succeed without any vault calls");

    assert_eq!(
        store.call_count(),
        0,
        "when every endpoint already has a key, apply_vault_fallback must make zero vault calls"
    );
}
