//! [`EnvVarStore`] — the always-on, diagnostic-only `SecretStore` backend (Phase 46.8 D-01).
//!
//! This backend is intentionally NOT a full credential provider: it exists so a
//! `SecretStore` is always available even with the `rusty-vault` feature disabled, and so
//! callers can be exercised end-to-end without an external vault. It does NOT invent an
//! `IRONHERMES_<KEY>` naming scheme (RESEARCH Anti-Pattern) — the real dynamic env-var
//! resolution for provider API keys lives in `ironhermes_core::provider` and is never
//! duplicated here.
//!
//! Writes hard-error (`VaultError::Backend`) rather than silently no-op, so a caller can
//! never be misled into thinking a `put_secret`/`delete_secret` call actually persisted.

use std::collections::HashMap;
use std::sync::Arc;

use secrecy::SecretString;

use crate::SecretStore;
use crate::error::VaultError;

const READ_ONLY_MESSAGE: &str = "EnvVarStore is read-only diagnostic";

/// Boxed injectable key-lookup closure (factored out per `clippy::type_complexity`).
type LookupFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Diagnostic-only `SecretStore` backed by an injectable key lookup.
///
/// The production constructor ([`EnvVarStore::new`]) reads from `std::env::var`; the
/// test-support constructor ([`EnvVarStore::with_lookup`]) injects an in-memory
/// `HashMap` so tests never mutate real process environment state (no
/// `std::env::set_var`/`remove_var` — those race under parallel test execution). Not gated
/// behind `#[cfg(test)]` because `tests/integration_tests.rs` compiles this crate as an
/// external dependency, where `cfg(test)` items of the library crate are not visible.
pub struct EnvVarStore {
    lookup: LookupFn,
    /// Enumerable key names for `list_secrets`. Empty for the real env-backed store
    /// (`std::env` has no portable "list all vars we care about" API and this backend is
    /// diagnostic-only) — populated only by the [`EnvVarStore::with_lookup`] constructor
    /// from the injected map's keys.
    keys: Vec<String>,
}

impl EnvVarStore {
    /// Construct the real, process-environment-backed store.
    pub fn new() -> Self {
        Self {
            lookup: Box::new(|k| std::env::var(k).ok()),
            keys: Vec::new(),
        }
    }

    /// Construct a store backed by an in-memory map, for tests/diagnostics.
    pub fn with_lookup(map: HashMap<String, String>) -> Self {
        let keys: Vec<String> = map.keys().cloned().collect();
        let shared = Arc::new(map);
        let lookup_map = Arc::clone(&shared);
        Self {
            lookup: Box::new(move |k| lookup_map.get(k).cloned()),
            keys,
        }
    }
}

impl Default for EnvVarStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SecretStore for EnvVarStore {
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<SecretString>> {
        Ok((self.lookup)(key).map(SecretString::from))
    }

    async fn put_secret(&self, _key: &str, _value: SecretString) -> anyhow::Result<()> {
        Err(VaultError::Backend(READ_ONLY_MESSAGE.to_string()).into())
    }

    async fn delete_secret(&self, _key: &str) -> anyhow::Result<()> {
        Err(VaultError::Backend(READ_ONLY_MESSAGE.to_string()).into())
    }

    async fn list_secrets(&self, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        Ok(match prefix {
            Some(p) => self
                .keys
                .iter()
                .filter(|k| k.starts_with(p))
                .cloned()
                .collect(),
            None => self.keys.clone(),
        })
    }
}
