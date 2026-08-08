//! Phase 41.3 Plan 10 — `ToolCredentials` env → config → vault precedence + hygiene tests.
//!
//! D-19: the vault is priority 3, consulted ONLY after env (priority 1) and
//! `tools.credentials` (priority 2) both miss. An earlier tier's hit means the
//! vault is never even consulted for that key (proven via a call-recording store,
//! not merely via the returned value). A sealed/corrupt vault (`get_secret` returns
//! `Err`) propagates LOUDLY — mirrors `provider_vault_fallback.rs`'s D-07
//! conventions exactly.
//!
//! Every test that mutates process environment variables takes the shared
//! `env_lock()` — run this file with `--test-threads=1` (crate-wide convention for
//! env-mutating tests, see `registry.rs`'s own `env_lock()`).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

use ironhermes_core::Config;
use ironhermes_tools::credentials::{CredentialSource, ToolCredentials};
use ironhermes_vault::SecretStore;
use secrecy::{ExposeSecret, SecretString};

// =============================================================================
// env_lock — serialise tests that mutate environment variables.
//
// An async-aware `tokio::sync::Mutex`, not `std::sync::Mutex` — every test here is
// `#[tokio::test]` and holds the guard across `ToolCredentials::resolve(...).await`;
// a std Mutex guard held across an await point is `clippy::await_holding_lock`
// (denied under this crate's `-D warnings` gate). tokio's Mutex has no poisoning,
// so callers just `.await` the lock, no `unwrap_or_else(|p| p.into_inner())` needed.
// =============================================================================

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Remove every `TOOL_CREDENTIAL_KEYS` member from the process env, so a
/// developer's real `.env`/shell exports never leak into these tests.
fn clear_all_credential_env_vars() {
    for key in ironhermes_tools::credentials::TOOL_CREDENTIAL_KEYS {
        // SAFETY: caller holds env_lock(); single-threaded test binary (--test-threads=1).
        unsafe { std::env::remove_var(key) };
    }
}

// =============================================================================
// MockStore — test-only SecretStore backend, mirrors provider_vault_fallback.rs
// =============================================================================

#[derive(Clone)]
enum MockResponse {
    Found(String),
    // Kept for parity with provider_vault_fallback.rs's MockResponse shape even
    // though no test in this file currently exercises "healthy vault, key absent".
    #[allow(dead_code)]
    NotFound,
    Sealed,
}

struct MockStore {
    responses: HashMap<String, MockResponse>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl MockStore {
    fn new(responses: HashMap<String, MockResponse>) -> Self {
        Self {
            responses,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

#[async_trait::async_trait]
impl SecretStore for MockStore {
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
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
        unimplemented!("ToolCredentials::resolve never calls put_secret")
    }

    async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
        unimplemented!("ToolCredentials::resolve never calls delete_secret")
    }

    async fn list_secrets(&self, _prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        unimplemented!("ToolCredentials::resolve never calls list_secrets")
    }
}

fn config_with_credential(key: &str, value: &str) -> Config {
    let mut config = Config::default();
    config
        .tools
        .credentials
        .insert(key.to_string(), value.to_string());
    config
}

// =============================================================================
// Single-source cases
// =============================================================================

#[tokio::test]
async fn env_only_source_resolves() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::set_var("EXA_API_KEY", "env-value") };

    let creds = ToolCredentials::resolve(&Config::default(), None)
        .await
        .expect("resolve must succeed with no store");

    assert_eq!(
        creds
            .credential("EXA_API_KEY")
            .map(|s| s.expose_secret().to_string()),
        Some("env-value".to_string())
    );
    assert_eq!(creds.source_of("EXA_API_KEY"), Some(CredentialSource::Env));

    unsafe { std::env::remove_var("EXA_API_KEY") };
}

#[tokio::test]
async fn config_only_source_resolves() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    let config = config_with_credential("EXA_API_KEY", "config-value");
    let creds = ToolCredentials::resolve(&config, None)
        .await
        .expect("resolve must succeed with no store");

    assert_eq!(
        creds
            .credential("EXA_API_KEY")
            .map(|s| s.expose_secret().to_string()),
        Some("config-value".to_string())
    );
    assert_eq!(
        creds.source_of("EXA_API_KEY"),
        Some(CredentialSource::Config)
    );
}

#[tokio::test]
async fn vault_only_source_resolves() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    let mut responses = HashMap::new();
    responses.insert(
        "EXA_API_KEY".to_string(),
        MockResponse::Found("vault-value".to_string()),
    );
    let store = MockStore::new(responses);

    let creds = ToolCredentials::resolve(&Config::default(), Some(&store))
        .await
        .expect("resolve must succeed");

    assert_eq!(
        creds
            .credential("EXA_API_KEY")
            .map(|s| s.expose_secret().to_string()),
        Some("vault-value".to_string())
    );
    assert_eq!(
        creds.source_of("EXA_API_KEY"),
        Some(CredentialSource::Vault)
    );
    assert!(store.calls().contains(&"EXA_API_KEY".to_string()));
}

// =============================================================================
// Conflict cases — the earlier source wins
// =============================================================================

#[tokio::test]
async fn env_beats_config() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::set_var("EXA_API_KEY", "env-wins") };

    let config = config_with_credential("EXA_API_KEY", "config-loses");
    let creds = ToolCredentials::resolve(&config, None)
        .await
        .expect("resolve must succeed");

    assert_eq!(
        creds
            .credential("EXA_API_KEY")
            .map(|s| s.expose_secret().to_string()),
        Some("env-wins".to_string())
    );

    unsafe { std::env::remove_var("EXA_API_KEY") };
}

#[tokio::test]
async fn env_beats_vault() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::set_var("EXA_API_KEY", "env-wins") };

    let mut responses = HashMap::new();
    responses.insert(
        "EXA_API_KEY".to_string(),
        MockResponse::Found("vault-loses".to_string()),
    );
    let store = MockStore::new(responses);

    let creds = ToolCredentials::resolve(&Config::default(), Some(&store))
        .await
        .expect("resolve must succeed");

    assert_eq!(
        creds
            .credential("EXA_API_KEY")
            .map(|s| s.expose_secret().to_string()),
        Some("env-wins".to_string())
    );

    unsafe { std::env::remove_var("EXA_API_KEY") };
}

#[tokio::test]
async fn config_beats_vault() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    let config = config_with_credential("EXA_API_KEY", "config-wins");
    let mut responses = HashMap::new();
    responses.insert(
        "EXA_API_KEY".to_string(),
        MockResponse::Found("vault-loses".to_string()),
    );
    let store = MockStore::new(responses);

    let creds = ToolCredentials::resolve(&config, Some(&store))
        .await
        .expect("resolve must succeed");

    assert_eq!(
        creds
            .credential("EXA_API_KEY")
            .map(|s| s.expose_secret().to_string()),
        Some("config-wins".to_string())
    );
}

// =============================================================================
// "Never even consulted" cases — a call-recording store proves the vault was
// skipped, not merely that the returned value was correct.
// =============================================================================

#[tokio::test]
async fn vault_is_not_queried_for_a_key_env_already_satisfied() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::set_var("EXA_API_KEY", "env-value") };

    let store = MockStore::new(HashMap::new());
    let _creds = ToolCredentials::resolve(&Config::default(), Some(&store))
        .await
        .expect("resolve must succeed");

    assert!(
        !store.calls().contains(&"EXA_API_KEY".to_string()),
        "vault get_secret must never be called for a key env already satisfied"
    );

    unsafe { std::env::remove_var("EXA_API_KEY") };
}

#[tokio::test]
async fn vault_is_not_queried_for_a_key_config_already_satisfied() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    let config = config_with_credential("EXA_API_KEY", "config-value");
    let store = MockStore::new(HashMap::new());
    let _creds = ToolCredentials::resolve(&config, Some(&store))
        .await
        .expect("resolve must succeed");

    assert!(
        !store.calls().contains(&"EXA_API_KEY".to_string()),
        "vault get_secret must never be called for a key config already satisfied"
    );
}

// =============================================================================
// Hard-error and hygiene
// =============================================================================

#[tokio::test]
async fn sealed_vault_is_a_hard_error() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    let mut responses = HashMap::new();
    responses.insert("TAVILY_API_KEY".to_string(), MockResponse::Sealed);
    let store = MockStore::new(responses);

    let result = ToolCredentials::resolve(&Config::default(), Some(&store)).await;
    result.expect_err("a sealed/corrupt vault must return Err, never a partial snapshot");
}

#[tokio::test]
async fn sealed_vault_error_carries_no_secret_material() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();
    // A configured value elsewhere in the same resolve() call — must never leak
    // into the sealed-vault error string.
    let config = config_with_credential("BRAVE_API_KEY", "totally-secret-brave-value");
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::set_var("PERPLEXITY_API_KEY", "totally-secret-perplexity-value") };

    let mut responses = HashMap::new();
    responses.insert("TAVILY_API_KEY".to_string(), MockResponse::Sealed);
    let store = MockStore::new(responses);

    let err = ToolCredentials::resolve(&config, Some(&store))
        .await
        .expect_err("sealed vault must hard-error");
    let message = format!("{err:#}");

    assert!(
        !message.contains("totally-secret-brave-value"),
        "sealed-vault error must not leak a config-tier value, got: {message}"
    );
    assert!(
        !message.contains("totally-secret-perplexity-value"),
        "sealed-vault error must not leak an env-tier value, got: {message}"
    );

    unsafe { std::env::remove_var("PERPLEXITY_API_KEY") };
}

#[tokio::test]
async fn disabled_vault_is_never_opened() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    // No store at all — the caller only passes Some(store) when the operator
    // enabled the vault (vault.enabled: true). An operator who never turned the
    // vault on cannot be stopped from booting by a sealed vault (T-41.3-53).
    let result = ToolCredentials::resolve(&Config::default(), None).await;
    result.expect("resolve must succeed with no store, regardless of any vault state");
}

#[tokio::test]
async fn debug_impl_prints_names_and_counts_but_no_values() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    let mut responses = HashMap::new();
    responses.insert(
        "EXA_API_KEY".to_string(),
        MockResponse::Found("super-secret-vault-value-xyz".to_string()),
    );
    let store = MockStore::new(responses);
    let creds = ToolCredentials::resolve(&Config::default(), Some(&store))
        .await
        .expect("resolve must succeed");

    let debug_output = format!("{creds:?}");

    assert!(
        debug_output.contains("EXA_API_KEY"),
        "Debug output must name the credential key, got: {debug_output}"
    );
    assert!(
        debug_output.contains('1'),
        "Debug output must contain a count, got: {debug_output}"
    );
    assert!(
        !debug_output.contains("super-secret-vault-value-xyz"),
        "Debug output must never contain a resolved value, got: {debug_output}"
    );
}

#[tokio::test]
async fn env_only_snapshot_reproduces_todays_behavior() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    let creds = ToolCredentials::env_only();

    // Set the env var AFTER construction — proving env lookups stay live and are
    // not snapshotted at construction time.
    // SAFETY: env_lock held, --test-threads=1.
    unsafe { std::env::set_var("FIRECRAWL_API_KEY", "set-after-construction") };

    assert_eq!(
        creds.has_credential("FIRECRAWL_API_KEY"),
        std::env::var("FIRECRAWL_API_KEY").is_ok(),
        "env_only() must answer has_credential identically to a bare std::env::var check"
    );
    assert!(creds.has_credential("FIRECRAWL_API_KEY"));

    unsafe { std::env::remove_var("FIRECRAWL_API_KEY") };

    assert!(
        !creds.has_credential("FIRECRAWL_API_KEY"),
        "env_only() must reflect env var removal too — no caching at construction"
    );
}

#[tokio::test]
async fn config_field_present_reads_a_dotted_path() {
    let _g = env_lock().lock().await;
    clear_all_credential_env_vars();

    let mut config = Config::default();
    config
        .tools
        .credentials
        .insert("SOME_DOTTED_PATH_KEY".to_string(), "present".to_string());
    config
        .tools
        .credentials
        .insert("EMPTY_VALUE_KEY".to_string(), String::new());

    let creds = ToolCredentials::resolve(&config, None)
        .await
        .expect("resolve must succeed");

    assert!(
        creds.config_field_present("tools.credentials.SOME_DOTTED_PATH_KEY"),
        "a path set to a non-empty value must report present"
    );
    assert!(
        !creds.config_field_present("tools.credentials.NEVER_SET_KEY"),
        "an absent path must report absent"
    );
    assert!(
        !creds.config_field_present("tools.credentials.EMPTY_VALUE_KEY"),
        "a path whose value is an empty string must report absent"
    );
}
