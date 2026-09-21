//! Proves [`ironhermes_core::profile_credentials::host_profile_credentials`] yields no
//! endpoint whenever the vault is disabled/unreachable/sealed, AND — compiled and run on the
//! DEFAULT feature set specifically — when the `rusty-vault` feature is not compiled in at all
//! (Phase 51 D-14, `51-06-PLAN.md`).
//!
//! # WIDENED PROTOCOL note
//!
//! This file only exercises the fail-closed "no endpoint" arms and the positive "endpoint
//! actually hosted" arm of the facade — the wire-protocol behaviors (the WIDENED `profile`
//! field, the mismatch check, etc.) are exercised in
//! `crates/ironhermes-vault/tests/profile_endpoint.rs`. See that file's module doc and
//! `51-06-SUMMARY.md` for the full record of the user's checkpoint reversal.

use ironhermes_core::config::Config;
use ironhermes_core::profile_credentials::host_profile_credentials;

/// Default config (vault disabled, the install default) must yield no endpoint, on EITHER
/// feature arm.
#[test]
fn host_yields_no_endpoint_for_default_disabled_vault_config() {
    let config = Config::default();
    assert!(
        !config.vault.enabled,
        "precondition: default Config has vault disabled"
    );
    let handle = host_profile_credentials(&config);
    assert!(
        handle.is_none(),
        "a default (vault-disabled) config must yield no credential endpoint"
    );
}

#[cfg(feature = "rusty-vault")]
mod rusty_vault_feature_on {
    use std::sync::Arc;

    use ironhermes_core::config::Config;
    use ironhermes_core::profile_credentials::host_profile_credentials;

    /// `config.vault.enabled == true` but `backend != "rusty-vault"` must still yield no
    /// endpoint — the facade only ever tries the `rusty-vault` backend.
    #[test]
    fn host_yields_no_endpoint_for_non_rusty_vault_backend() {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "env-var".to_string();
        assert!(
            host_profile_credentials(&config).is_none(),
            "a non-rusty-vault backend must never be hosted by this facade"
        );
    }

    /// The vault is enabled + backend correct, but the data directory was never initialized
    /// (`RustyVaultStore::open` returns `VaultError::NotInitialized`) — unreachable, so no
    /// endpoint (D-14).
    #[test]
    fn host_yields_no_endpoint_when_vault_unavailable() {
        let tmp = tempfile::tempdir().expect("create temp home dir");
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        config.vault.rusty_vault.data_dir = tmp.path().join("vault");
        config.vault.rusty_vault.unseal_mode = "keyfile".to_string();

        let handle = host_profile_credentials(&config);
        assert!(
            handle.is_none(),
            "an uninitialized (never `vault init`-ed) vault data dir must yield no endpoint, \
             not an Err or a panic"
        );
    }

    /// The vault is enabled, backend correct, and genuinely initialized+unsealed (keyfile mode
    /// auto-unseals on open) — the facade must actually host the endpoint. Proves the facade's
    /// full wiring end to end, not only its fail-closed arms.
    #[tokio::test]
    async fn host_yields_an_endpoint_when_vault_is_genuinely_reachable() {
        let tmp = tempfile::tempdir().expect("create temp home dir");
        let data_dir = tmp.path().join("vault");
        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: data_dir.clone(),
            unseal_mode: "keyfile".to_string(),
        };
        ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");

        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        config.vault.rusty_vault.data_dir = data_dir;
        config.vault.rusty_vault.unseal_mode = "keyfile".to_string();

        let handle = host_profile_credentials(&config);
        assert!(
            handle.is_some(),
            "a genuinely reachable, unsealed vault must yield a hosted endpoint"
        );

        // Clean up: shutdown is only reachable if we can unwrap the Arc (sole owner here).
        if let Some(handle) = handle {
            match Arc::try_unwrap(handle) {
                Ok(h) => h.shutdown().await,
                Err(_) => { /* other refs outstanding — Drop will still clean up */ }
            }
        }
    }

    /// Phase 51 Plan 19 (G-51-6) Task 2 control: the long-lived variant is behaviorally
    /// identical to today's `host_profile_credentials` — asserted through the new
    /// lifetime-generalized entry point directly, with `CredentialHostLifetime::OutlivesWorkers`
    /// explicit, so a future edit to the shared constructor cannot silently demote the daemon
    /// and gateway to the declining path without this test catching it. Unlike the sibling test
    /// above, this one checks the bound socket path EXISTS ON DISK — not merely that the hosting
    /// call returned `Some` — because `Some`-without-a-socket is exactly the failure class G-51-6
    /// is about.
    #[tokio::test]
    async fn long_lived_dispatcher_still_hosts_the_credential_endpoint() {
        use ironhermes_core::profile_credentials::{
            CredentialHostLifetime, host_profile_credentials_with_lifetime,
        };

        let tmp = tempfile::tempdir().expect("create temp home dir");
        let data_dir = tmp.path().join("vault");
        let rv_config = ironhermes_vault::RustyVaultConfig {
            data_dir: data_dir.clone(),
            unseal_mode: "keyfile".to_string(),
        };
        ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");

        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        config.vault.rusty_vault.data_dir = data_dir;
        config.vault.rusty_vault.unseal_mode = "keyfile".to_string();

        let handle = host_profile_credentials_with_lifetime(
            &config,
            CredentialHostLifetime::OutlivesWorkers,
        );
        let handle = handle.expect(
            "a genuinely reachable, unsealed vault with OutlivesWorkers must yield a hosted \
             endpoint",
        );
        assert!(
            handle.socket_path().exists(),
            "the long-lived variant's bound socket path must exist on disk while the handle is \
             alive: {}",
            handle.socket_path().display()
        );

        match Arc::try_unwrap(handle) {
            Ok(h) => h.shutdown().await,
            Err(_) => { /* other refs outstanding — Drop will still clean up */ }
        }
    }
}

/// Compiled and RUN specifically WITHOUT the `rusty-vault` feature (the default feature set for
/// `cargo nextest run -p ironhermes-core`, no `--features`/`--all-features` flag) — this is the
/// arm the plan's pre-review draft asserted only as a described behavior, never actually
/// compiled. `open_store` hard-errors with `VaultError::BackendUnavailable` under this same
/// `#[cfg]` in `ironhermes-vault/src/lib.rs`; this proves the FACADE's fail-closed behavior on
/// top of that, independent of any config content.
#[cfg(not(feature = "rusty-vault"))]
#[test]
fn host_yields_no_endpoint_when_feature_absent() {
    use ironhermes_core::config::Config;
    use ironhermes_core::profile_credentials::host_profile_credentials;

    // Deliberately construct a config that WOULD be reachable if the feature were compiled in
    // (vault enabled, backend correct) — proving the feature-absence check fires independent of
    // config content, not merely because this particular config would fail anyway.
    let mut config = Config::default();
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();

    let handle = host_profile_credentials(&config);
    assert!(
        handle.is_none(),
        "with the rusty-vault feature not compiled in, the facade must yield no endpoint \
         regardless of config content"
    );
}
