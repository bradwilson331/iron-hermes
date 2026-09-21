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
// Phase 51 Plan 07 (D-11): the worker-side client. Deliberately UNCONDITIONAL — see
// profile_client.rs's own module doc — it has no dependency on the `rusty_vault`
// crate at all, only tokio/serde_json/secrecy.
pub mod profile_client;
#[cfg(feature = "rusty-vault")]
pub mod profile_endpoint;
#[cfg(feature = "rusty-vault")]
pub mod profile_guard;
pub mod profile_paths;
#[cfg(feature = "rusty-vault")]
pub mod profile_policy;
#[cfg(feature = "rusty-vault")]
pub mod profile_store;
#[cfg(feature = "rusty-vault")]
pub mod profile_token;
#[cfg(feature = "rusty-vault")]
pub mod rusty_vault_store;

pub use config::{RustyVaultConfig, VaultConfig};
pub use env_var_store::EnvVarStore;
pub use error::VaultError;
pub use profile_client::{ProfileClientError, read_profile_credential, read_profile_credentials};
pub use profile_paths::{profile_policy_name, profile_secret_path, profile_secret_prefix};
#[cfg(feature = "rusty-vault")]
pub use profile_endpoint::{
    ProfileCredentialEndpointHandle, TracingProfileGuardAudit, host_profile_credential_endpoint,
    socket_path as profile_credential_socket_path, spawn_profile_credential_endpoint,
};
#[cfg(feature = "rusty-vault")]
pub use profile_guard::{
    ProfileGuard, ProfileGuardAudit, register_profile_guard, unregister_profile_guard,
};
#[cfg(feature = "rusty-vault")]
pub use profile_policy::{ensure_profile_policy, render_profile_policy};
#[cfg(feature = "rusty-vault")]
pub use profile_store::ProfileSecretStore;
#[cfg(feature = "rusty-vault")]
pub use profile_token::{
    MintedProfileToken, ProfileTokenAudit, mint_profile_token, profile_token_ttl_for_bootstrap,
    read_profile_secret_with_minted_token,
};
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

/// Open a fresh [`RustyVaultStore`] from `rv_config` and mint a profile-scoped token
/// against it (Phase 51 Plan 07, D-07). This is for a caller that holds **no** hosted
/// [`profile_endpoint::ProfileCredentialEndpointHandle`] — it opens its OWN, independent
/// `RustyVaultStore` for the mint, the same "open a store per call" pattern
/// `ironhermes-core::dispatch_gate`'s vault branch already uses. `core_handle`/
/// `root_token_secret` are `pub(crate)` on [`RustyVaultStore`] — reachable from here
/// because this function lives INSIDE the crate.
///
/// A caller that DOES hold a hosted handle must use [`mint_profile_token_for_host`]
/// instead (Phase 51 Plan 11, CR-06 fix). The earlier version of this doc comment claimed
/// such a caller "cannot reach that handle's own `Core`+root-token — deliberately not
/// exposed past this crate"; that was false in-crate (the fields were always plain-private,
/// not actually unreachable to code in this crate) and is the exact premise that produced
/// the defect. At the pinned `rusty_vault` rev, `TokenStore::new()` performs an
/// unsynchronized check-then-write on a process-scoped token salt, so a token minted
/// against a SECOND, independent `Core` disagrees with a DIFFERENT `Core`'s salt and fails
/// to validate there. Minting through the hosting endpoint's own `Core` instead makes that
/// defect unreachable by construction — one `Core`, one `TokenStore`, one salt.
#[cfg(feature = "rusty-vault")]
pub async fn mint_profile_token_via_config(
    rv_config: &RustyVaultConfig,
    slug: &str,
    ttl: std::time::Duration,
    sink: &dyn ProfileTokenAudit,
) -> Result<MintedProfileToken, VaultError> {
    let store = RustyVaultStore::open(rv_config)?;
    let core = store.core_handle();
    let root_token = store.root_token_secret();
    mint_profile_token(&core, &root_token, slug, ttl, sink).await
}

/// Mint a profile-scoped token through the HOSTED endpoint's own `Core` + root token
/// (Phase 51 Plan 11, CR-06 fix — closes T12). Prefer this over
/// [`mint_profile_token_via_config`] whenever the caller already holds a hosted
/// [`profile_endpoint::ProfileCredentialEndpointHandle`]: minting and validating through
/// the SAME `Core` means the same `TokenStore`, the same token salt, by construction —
/// the upstream `rusty_vault` defect where two independent `Core`s disagree on that salt
/// (see [`mint_profile_token_via_config`]'s doc) cannot arise on this path.
#[cfg(feature = "rusty-vault")]
pub async fn mint_profile_token_for_host(
    host: &ProfileCredentialEndpointHandle,
    slug: &str,
    ttl: std::time::Duration,
    sink: &dyn ProfileTokenAudit,
) -> Result<MintedProfileToken, VaultError> {
    mint_profile_token(host.core(), host.root_token(), slug, ttl, sink).await
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
