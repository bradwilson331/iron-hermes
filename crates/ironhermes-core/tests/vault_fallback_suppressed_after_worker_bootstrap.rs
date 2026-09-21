//! Phase 51 T17/WR-08 fix — `ProviderResolver::apply_vault_fallback`'s suppression
//! after a worker bootstraps a credential over the Phase 51 vault socket
//! (`worker_bootstrap::bootstrap_worker_credential` in `ironhermes-cli`,
//! `BootstrapOutcome::Installed`) is now PER-PROVIDER, not per-process.
//!
//! This narrows a regression introduced by the Phase 51 UAT F-04 fix, commit 2:
//! that fix made the WHOLE resolver stop consulting the vault once ANY provider
//! bootstrapped, on the false premise that "the process has no legitimate need
//! to ever consult a vault again". A resolver commonly holds MANY endpoints
//! (`roles.vision`, `roles.kanban_judge`, a fallback model, ...) and a socket
//! bootstrap resolves exactly ONE of them (`bootstrap_worker_credential` at
//! `worker_bootstrap.rs:141-147`) — so the broad form silently left every OTHER
//! vault-backed provider keyless, surfacing only as an opaque later 401. This is
//! `51-CONTEXT.md`'s `apply_vault_fallback` must-not-regress constraint,
//! regressed and now restored.
//!
//! One test function (not several) — the process-global marker under test has
//! no public "unset" by design (a worker bootstraps exactly one provider, once,
//! for its whole life), so every sub-case is ordered inside a single async fn's
//! sequential control flow, with the ONE call that sets the marker placed last.
//! This guarantees the "before any bootstrap" assertions (a never-bootstrapped
//! process, and the priorities-1-4/sealed-vault invariants that are independent
//! of bootstrap state) run against a still-unset marker, regardless of how the
//! test harness schedules test FUNCTIONS across files/processes — there is only
//! one function here, so there is nothing to race against.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use ironhermes_core::config::{Config, CustomProviderConfig, ProviderConfig};
use ironhermes_core::models_cache::ModelsCache;
use ironhermes_core::provider::{ProviderResolver, mark_worker_bootstrapped_over_socket};
use ironhermes_vault::SecretStore;
use secrecy::SecretString;

/// A `SecretStore` double that:
/// - returns a fixed secret for any key present in `secrets`, `Ok(None)` otherwise;
/// - PANICS if asked for a key in `forbidden` — the strongest available proof that
///   `apply_vault_fallback` never even calls `get_secret` for a name it must skip,
///   matching the existing "priorities 1-4 win" skip's own never-consult-the-store
///   behavior.
struct FixtureStore {
    secrets: HashMap<String, String>,
    forbidden: Vec<String>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl FixtureStore {
    fn new() -> Self {
        Self {
            secrets: HashMap::new(),
            forbidden: Vec::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_secret(mut self, key: &str, value: &str) -> Self {
        self.secrets.insert(key.to_string(), value.to_string());
        self
    }

    fn forbid(mut self, key: &str) -> Self {
        self.forbidden.push(key.to_string());
        self
    }
}

#[async_trait::async_trait]
impl SecretStore for FixtureStore {
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
        if self.forbidden.iter().any(|f| f == key) {
            panic!(
                "apply_vault_fallback must never consult the store for {key:?} — this \
                 provider either already has a key from priorities 1-4, or IS the provider \
                 this process bootstrapped over the socket and must not have its \
                 socket-installed credential overwritten (T17/WR-08)"
            );
        }
        self.calls.lock().unwrap().push(key.to_string());
        Ok(self.secrets.get(key).map(|v| SecretString::from(v.clone())))
    }

    async fn put_secret(&self, _key: &str, _value: SecretString) -> anyhow::Result<()> {
        unimplemented!("not exercised by this test")
    }

    async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
        unimplemented!("not exercised by this test")
    }

    async fn list_secrets(&self, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        unimplemented!("not exercised by this test")
    }
}

/// A `SecretStore` double that always errors — proves the sealed-vault hard
/// error (D-07/D-14) is untouched by this plan's per-provider rework.
struct ErroringStore;

#[async_trait::async_trait]
impl SecretStore for ErroringStore {
    async fn get_secret(&self, _key: &str) -> anyhow::Result<Option<SecretString>> {
        anyhow::bail!("sealed vault (fixture)")
    }

    async fn put_secret(&self, _key: &str, _value: SecretString) -> anyhow::Result<()> {
        unimplemented!("not exercised by this test")
    }

    async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
        unimplemented!("not exercised by this test")
    }

    async fn list_secrets(&self, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        unimplemented!("not exercised by this test")
    }
}

/// Builds a `Config` whose only addressable providers are the given custom
/// providers — the three built-ins are disabled so a lookup can never
/// accidentally succeed against one of them instead of the fixture.
fn config_with_custom_providers(main: &str, providers: &[(&str, Option<&str>)]) -> Config {
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
    config.model.provider = main.to_string();
    for (name, api_key) in providers {
        config.custom_providers.push(CustomProviderConfig {
            name: name.to_string(),
            base_url: "https://f04-fixture.example/v1".to_string(),
            api_key: api_key.map(str::to_string),
            api_mode: None,
            default_model: None,
        });
    }
    config
}

#[tokio::test]
async fn apply_vault_fallback_suppresses_only_the_bootstrapped_providers_endpoint() {
    // --- Case 1: before ANY bootstrap in this process, behavior is unchanged
    // from before the Phase 51 UAT F-04 fix ever existed (byte-identical): a
    // keyless endpoint fills from the vault, and an endpoint that already has a
    // key from priorities 1-4 never even reaches the store. ---
    let config = config_with_custom_providers(
        "unbootstrapped-main",
        &[
            ("unbootstrapped-main", None),
            ("already-keyed", Some("pre-existing-key")),
        ],
    );
    let mut resolver = ProviderResolver::build_with_cache(&config, ModelsCache::default())
        .expect("build_with_cache should succeed for the fixture config");
    let store = FixtureStore::new()
        .with_secret("unbootstrapped-main", "vault-value-for-main")
        .forbid("already-keyed"); // priorities-1-4 already won — must not be consulted

    resolver
        .apply_vault_fallback(&store)
        .await
        .expect("a never-bootstrapped process must resolve the vault normally");

    assert_eq!(
        resolver
            .resolve("unbootstrapped-main")
            .unwrap()
            .api_key
            .as_deref(),
        Some("vault-value-for-main"),
        "a never-bootstrapped process must still fill a keyless endpoint from the vault"
    );
    assert_eq!(
        resolver.resolve("already-keyed").unwrap().api_key.as_deref(),
        Some("pre-existing-key"),
        "priorities 1-4 must still win without the vault being consulted at all"
    );

    // --- Case 2: a sealed/unreachable vault still hard-errors (D-07/D-14),
    // independent of bootstrap state. ---
    let config2 = config_with_custom_providers("sealed-case", &[("sealed-case", None)]);
    let mut resolver2 = ProviderResolver::build_with_cache(&config2, ModelsCache::default())
        .expect("build_with_cache should succeed for the fixture config");
    let err = resolver2
        .apply_vault_fallback(&ErroringStore)
        .await
        .expect_err("a sealed/erroring vault must propagate loudly, not degrade to keyless");
    assert!(
        err.to_string().contains("sealed vault"),
        "unexpected error message: {err}"
    );

    // --- Case 3 (the headline fix, T17/WR-08): after THIS process bootstraps
    // ONE provider over the socket, a SECOND vault-backed provider must still
    // resolve from the root vault, and the bootstrapped provider's own
    // socket-installed key must NOT be overwritten. This is the exact case
    // 51-VERIFICATION.md names as missing. The single `mark_...` call for this
    // whole test file happens here, last, on purpose. ---
    let config3 = config_with_custom_providers(
        "bootstrapped-main",
        &[
            ("bootstrapped-main", Some("socket-installed-value")),
            ("sibling-provider", None),
        ],
    );
    let mut resolver3 = ProviderResolver::build_with_cache(&config3, ModelsCache::default())
        .expect("build_with_cache should succeed for the fixture config");
    let store3 = FixtureStore::new()
        .with_secret("sibling-provider", "vault-value-for-sibling")
        .forbid("bootstrapped-main"); // must never be consulted — socket value must win

    mark_worker_bootstrapped_over_socket("bootstrapped-main");

    resolver3
        .apply_vault_fallback(&store3)
        .await
        .expect("apply_vault_fallback must still succeed after a bootstrap mark");

    assert_eq!(
        resolver3
            .resolve("sibling-provider")
            .unwrap()
            .api_key
            .as_deref(),
        Some("vault-value-for-sibling"),
        "a SECOND vault-backed provider must resolve from the root vault even after this \
         process bootstrapped a DIFFERENT provider over the socket (T17/WR-08)"
    );
    assert_eq!(
        resolver3
            .resolve("bootstrapped-main")
            .unwrap()
            .api_key
            .as_deref(),
        Some("socket-installed-value"),
        "the bootstrapped provider's own socket-installed credential must never be \
         overwritten from the vault"
    );
}
