//! Phase 46.9 Plan 06 (D-03/D-04): Provider secret API-key set/rotate/clear,
//! vault-backed. This is the phase's one deliberately-isolated
//! security-sensitive surface — a browser write reaches
//! `ironhermes-vault::SecretStore` for the first time — kept in its own
//! file so it can be audited in isolation and so the `/gsd-secure-phase
//! 46.9` pass (D-04) has a tight, single-file boundary to review.
//!
//! # Double gate (never auto-enable)
//!
//! Every write (`set`/`rotate`/`clear`) runs through [`check_double_gate`]:
//! 1. fail-closed unless `security.web_config_write_enabled` ("Config
//!    writes are disabled" — the same gate `provider_config_api.rs` and
//!    every other write-side `#[server]` fn in this phase already uses).
//! 2. HARD-BLOCK unless `ironhermes_core::resolve_vault_config(&config).enabled`
//!    ("Vault is not enabled." two-line copy). This is a genuine hard
//!    block, never a silent auto-enable — Research Open Question 2's
//!    explicit-only stance.
//!
//! # Key convention
//!
//! `put_secret`/`delete_secret`/`get_secret` are keyed by **provider
//! name**, matching `ProviderResolver::apply_vault_fallback`'s convention
//! (`crates/ironhermes-core/src/provider.rs:511-530`) and the CLI's
//! `vault_cmd.rs::cmd_set`/`cmd_migrate` — the same key a stored secret
//! will later be looked up by when a chat completion resolves credentials.
//!
//! # Secrets never touch `Debug`/`Display`
//!
//! The raw `String` key is wrapped in `secrecy::SecretString` the moment it
//! is available (mirrors `vault_cmd.rs::cmd_set`'s
//! `secrecy::SecretString::from(value)` precedent) and crosses straight
//! into `put_secret` — it is never logged, printed, or placed on any
//! `Debug`/`Display`-deriving struct first (RustyVault master-key log-leak
//! precedent, Phase 46.8 NF-1).
//!
//! # Backend writability gate (UAT fix, checkpoint round 2)
//!
//! `vault.enabled: true` alone is NOT sufficient for secret writes — the
//! default `vault.backend` is `"env-var"`, whose [`SecretStore`] impl is
//! read-only diagnostic (`put_secret`/`delete_secret` hard-error at call
//! time; the trait itself exposes no writability probe). Gating only on
//! `enabled` let the UI present `SET KEY` for a store that can never
//! persist, then surface the raw backend diagnostic. [`check_write_gates`]
//! therefore adds a third fail-closed check after the double gate:
//! `backend == "env-var"` hard-blocks with actionable copy, and
//! [`get_provider_secret_status`] reports the same block up front so the
//! UI renders the row blocked BEFORE the user types a key. Like the vault
//! gate, this never auto-switches the backend.
//!
//! # No readback, ever
//!
//! No function in this file returns a raw secret value. `set`/`rotate`
//! return `Result<(), ServerFnError>`; `clear` returns
//! `Result<(), ServerFnError>`; the one read, [`get_provider_secret_status`],
//! returns presence + blocked-reason only via
//! `SecretStore::get_secret(..).is_some()` — never the value.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// UI-SPEC Copywriting Contract — vault-disabled hard-block copy.
#[cfg(not(target_arch = "wasm32"))]
const VAULT_DISABLED_LINE1: &str = "Vault is not enabled.";
#[cfg(not(target_arch = "wasm32"))]
const VAULT_DISABLED_LINE2: &str =
    "Set vault.enabled: true in config.yaml to store provider secrets, then retry.";

/// Read-only-backend hard-block copy (UAT fix). Actionable and secret-free:
/// names the config key to change AND the required build feature — never
/// the raw `EnvVarStore` backend diagnostic.
#[cfg(not(target_arch = "wasm32"))]
const BACKEND_READ_ONLY_LINE1: &str = "Vault backend is read-only (env-var).";
#[cfg(not(target_arch = "wasm32"))]
const BACKEND_READ_ONLY_LINE2: &str = "Set vault.backend: \"rusty-vault\" in config.yaml and \
     serve a rusty-vault build (--features rusty-vault), then retry.";

/// Presence + writability snapshot for one provider's vault secret.
/// `blocked` carries the two-line hard-block copy when secret writes
/// cannot work in the current config/build (vault disabled, read-only
/// backend, backend unavailable) so the UI can render the row blocked
/// BEFORE the user types a key. Never carries a secret value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProviderSecretStatus {
    /// Presence-only — never the credential value itself.
    pub has_secret: bool,
    /// `Some((line1, line2))` when secret writes are hard-blocked.
    pub blocked: Option<(String, String)>,
}

/// Pure, disk-I/O-free double-gate check (unit-testable without
/// `Config::load()` touching the real filesystem). Returns the resolved
/// `VaultConfig` on success, or one of the two exact hard-block error
/// strings the UI-SPEC Copywriting Contract specifies.
#[cfg(not(target_arch = "wasm32"))]
fn check_double_gate(
    config: &ironhermes_core::config::Config,
) -> Result<ironhermes_vault::VaultConfig, ServerFnError> {
    if !config.security.web_config_write_enabled {
        return Err(ServerFnError::new("Config writes are disabled"));
    }

    let vault_config = ironhermes_core::resolve_vault_config(config);
    if !vault_config.enabled {
        // Hard block — never auto-enable (Research Open Question 2).
        return Err(ServerFnError::new(format!(
            "{VAULT_DISABLED_LINE1} {VAULT_DISABLED_LINE2}"
        )));
    }

    Ok(vault_config)
}

/// Pure writability check (UAT fix): the default `env-var` backend is
/// read-only diagnostic — its `put_secret`/`delete_secret` hard-error at
/// call time — so secret writes must hard-block on it BEFORE any store is
/// opened, with actionable copy instead of the raw backend diagnostic.
/// Never auto-switches the backend (same explicit-only stance as the
/// vault-enabled gate).
#[cfg(not(target_arch = "wasm32"))]
fn check_backend_writable(
    vault_config: &ironhermes_vault::VaultConfig,
) -> Result<(), ServerFnError> {
    if vault_config.backend == "env-var" {
        return Err(ServerFnError::new(format!(
            "{BACKEND_READ_ONLY_LINE1} {BACKEND_READ_ONLY_LINE2}"
        )));
    }
    Ok(())
}

/// Composed write-path gate: double gate (write-enabled + vault-enabled)
/// THEN backend writability. Pure and unit-testable — the full fail-closed
/// chain every secret write must pass.
#[cfg(not(target_arch = "wasm32"))]
fn check_write_gates(
    config: &ironhermes_core::config::Config,
) -> Result<ironhermes_vault::VaultConfig, ServerFnError> {
    let vault_config = check_double_gate(config)?;
    check_backend_writable(&vault_config)?;
    Ok(vault_config)
}

/// Pure, disk-I/O-free empty-secret validation (T-46.9-19 backstop): rejects
/// an empty key server-side even if a client bypassed its own
/// disable-until-non-empty submit guard.
#[cfg(not(target_arch = "wasm32"))]
fn validate_secret_value(api_key: &str) -> Result<(), ServerFnError> {
    if api_key.is_empty() {
        return Err(ServerFnError::new("API key must not be empty"));
    }
    Ok(())
}

/// Open the fully-gated (double gate + backend writability) vault store
/// for `config`. Shared by `set`/`rotate`/`clear` — never by the
/// presence-only status read (which deliberately does not error on a
/// closed WRITE gate; see [`get_provider_secret_status`]).
#[cfg(not(target_arch = "wasm32"))]
async fn open_gated_store(
    config: &ironhermes_core::config::Config,
) -> Result<Box<dyn ironhermes_vault::SecretStore>, ServerFnError> {
    let vault_config = check_write_gates(config)?;
    ironhermes_vault::open_store(&vault_config)
        .map_err(|e| ServerFnError::new(format!("Vault store open failed: {e}")))
}

/// Shared set/rotate implementation — both map to `put_secret` (create or
/// overwrite; RustyVault's KV v1 backend has no version-history API, so
/// there is no separate "rotate" primitive to call — D-09 precedent from
/// `ironhermes-vault`'s own module doc).
#[cfg(not(target_arch = "wasm32"))]
async fn write_provider_secret(
    provider_name: String,
    api_key: String,
) -> Result<(), ServerFnError> {
    if provider_name.trim().is_empty() {
        return Err(ServerFnError::new("provider name must not be empty"));
    }
    validate_secret_value(&api_key)?;

    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let store = open_gated_store(&config).await?;

    // Wrap the raw key in SecretString the moment it is available — before
    // it touches any Debug/Display-deriving struct.
    let secret = secrecy::SecretString::from(api_key);
    store
        .put_secret(&provider_name, secret)
        .await
        .map_err(|e| ServerFnError::new(format!("Vault write failed: {e}")))?;

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
async fn clear_provider_secret_impl(provider_name: String) -> Result<(), ServerFnError> {
    if provider_name.trim().is_empty() {
        return Err(ServerFnError::new("provider name must not be empty"));
    }

    let config = ironhermes_core::config::Config::load()
        .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
    let store = open_gated_store(&config).await?;

    store
        .delete_secret(&provider_name)
        .await
        .map_err(|e| ServerFnError::new(format!("Vault delete failed: {e}")))?;

    Ok(())
}

/// Pure blocked-state computation for the status read (UAT fix): reports
/// WHY secret writes cannot work in the current config — vault disabled,
/// or read-only `env-var` backend — so the UI can render the row blocked
/// with actionable copy BEFORE the user types a key. Returns `None` when
/// the configured backend should be writable. Deliberately does NOT
/// consult `web_config_write_enabled` — this is a read-side helper, and
/// the write gate is enforced (fail-closed) by every write fn separately.
#[cfg(not(target_arch = "wasm32"))]
fn secret_backend_block(config: &ironhermes_core::config::Config) -> Option<(String, String)> {
    let vault_config = ironhermes_core::resolve_vault_config(config);
    if !vault_config.enabled {
        return Some((
            VAULT_DISABLED_LINE1.to_string(),
            VAULT_DISABLED_LINE2.to_string(),
        ));
    }
    if vault_config.backend == "env-var" {
        return Some((
            BACKEND_READ_ONLY_LINE1.to_string(),
            BACKEND_READ_ONLY_LINE2.to_string(),
        ));
    }
    None
}

/// Presence + blocked-state snapshot for one provider — never returns the
/// value. Fail-closed and actionable: any state in which a secret write
/// cannot succeed (config unreadable, vault disabled, read-only backend,
/// backend unavailable in this build) is reported as `blocked` so the UI
/// never offers `SET KEY` for a store that cannot persist.
#[cfg(not(target_arch = "wasm32"))]
async fn provider_secret_status_impl(provider_name: &str) -> ProviderSecretStatus {
    let Ok(config) = ironhermes_core::config::Config::load() else {
        return ProviderSecretStatus {
            has_secret: false,
            blocked: Some((
                "Vault status unavailable.".to_string(),
                "Could not read config.yaml on the server.".to_string(),
            )),
        };
    };
    if let Some(blocked) = secret_backend_block(&config) {
        return ProviderSecretStatus {
            has_secret: false,
            blocked: Some(blocked),
        };
    }
    let vault_config = ironhermes_core::resolve_vault_config(&config);
    let store = match ironhermes_vault::open_store(&vault_config) {
        Ok(store) => store,
        // e.g. backend "rusty-vault" without the compiled `rusty-vault`
        // cargo feature — VaultError::BackendUnavailable names the missing
        // feature (secret-free), pass it through as the actionable line.
        Err(e) => {
            return ProviderSecretStatus {
                has_secret: false,
                blocked: Some((
                    "Vault backend is unavailable in this build.".to_string(),
                    format!("{e}"),
                )),
            };
        }
    };
    let has_secret = matches!(store.get_secret(provider_name).await, Ok(Some(_)));
    ProviderSecretStatus {
        has_secret,
        blocked: None,
    }
}

/// Store a NEW provider API key in the vault (D-03). Double-gated
/// (write-enabled + vault-enabled hard block); the raw key is wrapped in
/// `SecretString` before any write; never returns the value.
#[server]
pub async fn set_provider_secret(
    provider_name: String,
    api_key: String,
) -> Result<(), ServerFnError> {
    write_provider_secret(provider_name, api_key).await
}

/// Overwrite an existing provider API key in the vault (D-03). Same
/// double-gated write path as [`set_provider_secret`] — RustyVault's KV v1
/// backend has no rotation/version-history primitive, so "rotate" is a
/// `put_secret` overwrite, identical to "set".
#[server]
pub async fn rotate_provider_secret(
    provider_name: String,
    api_key: String,
) -> Result<(), ServerFnError> {
    write_provider_secret(provider_name, api_key).await
}

/// Remove a provider's stored API key from the vault (D-03). Double-gated
/// like set/rotate; the provider stops resolving credentials from the
/// vault immediately (the UI's confirmation copy states this explicitly).
#[server]
pub async fn clear_provider_secret(provider_name: String) -> Result<(), ServerFnError> {
    clear_provider_secret_impl(provider_name).await
}

/// Presence + blocked-state read used by the provider editor to paint the
/// secret row's initial state (`NOT CONFIGURED` + `SET KEY` vs `••••••••`
/// + `ROTATE`/`CLEAR` vs a blocked row with actionable copy) without ever
/// exposing the value. Never errors.
#[server]
pub async fn get_provider_secret_status(
    provider_name: String,
) -> Result<ProviderSecretStatus, ServerFnError> {
    Ok(provider_secret_status_impl(&provider_name).await)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod provider_secrets_tests {
    use super::{
        check_backend_writable, check_double_gate, check_write_gates, secret_backend_block,
        validate_secret_value,
    };
    use ironhermes_core::config::Config;

    /// T-46.9-16: the write gate must fail closed when
    /// security.web_config_write_enabled is false (the default) —
    /// verified BEFORE the vault gate is even consulted.
    #[test]
    fn gate_fails_closed_by_default() {
        let config = Config::default();
        assert!(
            !config.security.web_config_write_enabled,
            "web_config_write_enabled must default to false (gate closed)"
        );
        let err = check_double_gate(&config)
            .expect_err("write-gate-closed config must be rejected before any vault access");
        assert!(
            err.to_string().contains("Config writes are disabled"),
            "got: {err}"
        );
    }

    /// T-46.9-16: even with the write gate open, a disabled vault
    /// hard-blocks — never auto-enabled as a side effect of a secret
    /// write (Research Open Question 2).
    #[test]
    fn vault_disabled_hard_blocks_even_with_write_enabled() {
        let mut config = Config::default();
        config.security.web_config_write_enabled = true;
        assert!(
            !config.vault.enabled,
            "precondition: vault.enabled defaults to false"
        );
        let err = check_double_gate(&config)
            .expect_err("a disabled vault must hard-block even when write-enabled is true");
        assert!(
            err.to_string().contains("Vault is not enabled"),
            "got: {err}"
        );
        assert!(
            err.to_string().contains("vault.enabled: true"),
            "error must name the config key to flip, got: {err}"
        );
    }

    /// The double gate passes only when BOTH flags are explicitly true.
    #[test]
    fn gate_passes_when_both_enabled() {
        let mut config = Config::default();
        config.security.web_config_write_enabled = true;
        config.vault.enabled = true;
        assert!(check_double_gate(&config).is_ok());
    }

    /// UAT fix (checkpoint round 2): the DEFAULT backend is the read-only
    /// diagnostic `env-var` store — with both flags enabled, a secret
    /// write must still hard-block with actionable copy (config key +
    /// build feature), never the raw "EnvVarStore is read-only diagnostic"
    /// backend error the live UAT hit.
    #[test]
    fn read_only_backend_hard_blocks_writes_with_actionable_copy() {
        let mut config = Config::default();
        config.security.web_config_write_enabled = true;
        config.vault.enabled = true;
        assert_eq!(
            config.vault.backend, "env-var",
            "precondition: default backend is the read-only env-var store"
        );
        let err = check_write_gates(&config)
            .expect_err("the read-only env-var backend must hard-block secret writes");
        let msg = err.to_string();
        assert!(msg.contains("read-only (env-var)"), "got: {msg}");
        assert!(
            msg.contains("vault.backend: \"rusty-vault\""),
            "error must name the config key to change, got: {msg}"
        );
        assert!(
            msg.contains("--features rusty-vault"),
            "error must name the required build feature, got: {msg}"
        );
        assert!(
            !msg.contains("EnvVarStore"),
            "raw backend diagnostic must never surface to the UI, got: {msg}"
        );
    }

    /// A writable backend passes the writability check (the gate is on
    /// the read-only diagnostic backend only — never a blanket block).
    #[test]
    fn rusty_vault_backend_passes_writability_check() {
        let mut config = Config::default();
        config.security.web_config_write_enabled = true;
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        let vault_config =
            check_write_gates(&config).expect("rusty-vault backend must pass the write gates");
        assert_eq!(vault_config.backend, "rusty-vault");
        assert!(check_backend_writable(&vault_config).is_ok());
    }

    /// UAT fix: the status read reports the read-only backend as BLOCKED
    /// (with the same actionable copy) so the UI never offers SET KEY for
    /// a store that cannot persist.
    #[test]
    fn status_reports_read_only_backend_as_blocked() {
        let mut config = Config::default();
        config.vault.enabled = true;
        assert_eq!(config.vault.backend, "env-var");
        let (line1, line2) =
            secret_backend_block(&config).expect("env-var backend must report blocked");
        assert!(line1.contains("read-only (env-var)"), "got: {line1}");
        assert!(
            line2.contains("vault.backend: \"rusty-vault\""),
            "got: {line2}"
        );
        assert!(!line1.contains("EnvVarStore") && !line2.contains("EnvVarStore"));
    }

    /// The status read also reports a disabled vault as blocked (same
    /// hard-block copy the write path uses).
    #[test]
    fn status_reports_disabled_vault_as_blocked() {
        let config = Config::default();
        let (line1, line2) =
            secret_backend_block(&config).expect("disabled vault must report blocked");
        assert!(line1.contains("Vault is not enabled"), "got: {line1}");
        assert!(line2.contains("vault.enabled: true"), "got: {line2}");
    }

    /// A writable, enabled backend reports NOT blocked.
    #[test]
    fn status_reports_writable_backend_as_unblocked() {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        assert!(secret_backend_block(&config).is_none());
    }

    /// T-46.9-19 backstop: an empty secret is rejected server-side even if
    /// a client somehow bypassed its own disable-until-non-empty guard.
    #[test]
    fn empty_secret_rejected_server_side() {
        let err = validate_secret_value("").expect_err("empty secret must be rejected");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn non_empty_secret_accepted() {
        assert!(validate_secret_value("sk-abc123").is_ok());
    }
}
