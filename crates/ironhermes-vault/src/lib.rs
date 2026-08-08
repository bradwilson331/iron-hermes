//! `ironhermes-vault` — pluggable `SecretStore` adapter for provider API keys (Phase 46.8).
//!
//! # Zero-cycle leaf crate (D-01)
//!
//! This crate has ZERO dependency on `ironhermes-core` — it is the adapter, not the canonical
//! config/backend, and `ironhermes-core` depends on it (never the reverse) so `VaultConfig`
//! can be embedded into `ironhermes-core::Config` (Plan 04) without a dependency cycle.
//!
//! # Four-method trait, no rotation (D-09)
//!
//! [`SecretStore`] intentionally has exactly four methods — get/put/delete/list. RustyVault's
//! published KV v1 backend (`rusty_vault` 0.2.1) has no version-history API, so a
//! `rotate_secret`/`get_secret_version` method would be dishonest to implement; do not add one.
//!
//! # Secrets never touch `Debug`/`Display` (D-08)
//!
//! Every value that crosses the [`SecretStore`] boundary is wrapped in
//! [`secrecy::SecretString`], which does not implement plain `Debug`/`Display` — callers must
//! explicitly `.expose_secret()` at the one boundary that needs the raw value (Plan 04's
//! `apply_vault_fallback`), never log or print it beforehand.
//!
//! # Hard-error on sealed (D-07)
//!
//! A sealed or uninitialized backend returns [`error::VaultError::Sealed`] /
//! [`error::VaultError::NotInitialized`] rather than silently falling back — see
//! [`error::VaultError`].

pub mod config;
pub mod env_var_store;
pub mod error;
#[cfg(feature = "rusty-vault")]
pub mod rusty_vault_store;

pub use config::{RustyVaultConfig, VaultConfig};
pub use env_var_store::EnvVarStore;
pub use error::VaultError;
#[cfg(feature = "rusty-vault")]
pub use rusty_vault_store::RustyVaultStore;

/// Pluggable secret-storage backend for provider API keys (Phase 46.8 D-01/D-09).
///
/// Exactly four methods — no rotation or version-history API (D-09: KV v1 cannot honestly
/// implement one). Implementations MUST be `Send + Sync` so a `Box<dyn SecretStore>` can be
/// shared across the async runtime.
#[async_trait::async_trait]
pub trait SecretStore: Send + Sync {
    /// Fetch a secret by key. Returns `Ok(None)` if the key does not exist (not an error).
    async fn get_secret(&self, key: &str) -> anyhow::Result<Option<secrecy::SecretString>>;

    /// Store (create or overwrite) a secret value under `key`.
    async fn put_secret(&self, key: &str, value: secrecy::SecretString) -> anyhow::Result<()>;

    /// Delete a secret by key. Backends may treat deleting an absent key as a no-op success.
    async fn delete_secret(&self, key: &str) -> anyhow::Result<()>;

    /// List secret key names, optionally filtered by `prefix`. Never returns values (D-15).
    async fn list_secrets(&self, prefix: Option<&str>) -> anyhow::Result<Vec<String>>;
}

/// Select and construct the configured [`SecretStore`] backend (D-10).
///
/// `backend == "env-var"` always works (the always-on diagnostic backend, no cargo feature
/// required). `backend == "rusty-vault"` requires this crate's `rusty-vault` feature; without
/// it, this hard-errors with [`VaultError::BackendUnavailable`] naming the missing feature
/// rather than silently falling back to `EnvVarStore` — the same hard-error-don't-mask
/// posture D-07 uses for a sealed/uninitialized backend, extended to backend selection
/// itself.
pub fn open_store(config: &VaultConfig) -> anyhow::Result<Box<dyn SecretStore>> {
    match config.backend.as_str() {
        "env-var" => Ok(Box::new(EnvVarStore::new())),
        "rusty-vault" => {
            #[cfg(feature = "rusty-vault")]
            {
                Ok(Box::new(RustyVaultStore::open(&config.rusty_vault)?))
            }
            #[cfg(not(feature = "rusty-vault"))]
            {
                Err(VaultError::BackendUnavailable.into())
            }
        }
        other => Err(VaultError::Backend(format!(
            "unknown vault backend {other:?} — expected \"env-var\" or \"rusty-vault\""
        ))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_store_env_var_backend_works_on_default_features() {
        let cfg = VaultConfig {
            enabled: true,
            backend: "env-var".to_string(),
            ..VaultConfig::default()
        };
        let store = open_store(&cfg).expect("env-var backend is always available");
        // Diagnostic-only backend: a lookup for an absent key is Ok(None), not an error.
        let result = store
            .get_secret("DEFINITELY_UNSET_IRONHERMES_TEST_KEY_XYZ")
            .await;
        assert!(result.is_ok());
    }

    // Only meaningful (and only compiles as a true feature-off assertion) when the
    // `rusty-vault` feature is NOT enabled — this is exactly `cargo test -p ironhermes-vault`
    // with no `--features` flag, matching Task 2's acceptance criteria.
    #[cfg(not(feature = "rusty-vault"))]
    #[tokio::test]
    async fn open_store_rusty_vault_without_feature_hard_errors() {
        let cfg = VaultConfig {
            enabled: true,
            backend: "rusty-vault".to_string(),
            ..VaultConfig::default()
        };
        // `Box<dyn SecretStore>` is not `Debug`, so `expect_err`/`unwrap_err` (which require
        // `T: Debug` for their panic message) don't work here — match explicitly instead.
        let msg = match open_store(&cfg) {
            Ok(_) => panic!("expected an error when the rusty-vault feature is not compiled in"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("rusty-vault"),
            "error message must name the missing `rusty-vault` feature, got: {msg}"
        );
    }
}
