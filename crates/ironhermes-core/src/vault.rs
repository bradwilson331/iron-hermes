//! Shared vault `data_dir` sentinel resolution (Phase 46.8 UAT gap G-46.8-1).
//!
//! # Root cause this module fixes
//!
//! `RustyVaultConfig::data_dir` defaults to an empty `PathBuf` sentinel (the leaf crate
//! `ironhermes-vault` cannot call [`crate::get_hermes_home`] — D-01 zero-cycle direction).
//! Before this module existed, only the CLI's `vault_cmd::resolve_vault_config` filled that
//! sentinel with `get_hermes_home().join("vault")` before opening a store — every *runtime*
//! `open_store`/`RustyVaultStore::open` call site (the web server's `AppState::init`, the
//! cron runner, the subagent test helper) passed `&config.vault` straight through with the
//! sentinel UNRESOLVED. `RustyVaultStore::open` then looked at path `""`, which is never an
//! initialized vault, and hard-errored with `VaultError::NotInitialized` — the server panics
//! at startup against a DEFAULT-configured (empty `data_dir`) vault even though `vault init`
//! (via the CLI) succeeded and wrote real data under `$HOME/.ironhermes/vault`.
//!
//! This module is the ONE shared resolution point every production call site now routes
//! through (`ironhermes_core::resolve_vault_config`), plus a pure `_with_home` variant for
//! unit/integration testing without mutating `std::env`.

use std::path::Path;

use crate::config::Config;
use crate::constants::get_hermes_home;

/// Resolve `config.vault`, filling the empty-`PathBuf` `rusty_vault.data_dir` sentinel with
/// `home.join("vault")`. Pure — takes `home` as a parameter instead of reading
/// `IRONHERMES_HOME`/`dirs::home_dir()` itself, so it is unit-testable without any
/// `std::env` mutation (avoiding the process-global-env test flake noted across this
/// codebase's `env_lock` precedent).
///
/// An explicitly-set (non-empty) `data_dir` is passed through UNCHANGED — this only ever
/// fills the sentinel, never overrides an operator's explicit configuration. Matches the
/// CLI's `vault_cmd::resolve_vault_config` semantics exactly (Phase 46.8 D-03).
pub fn resolve_vault_config_with_home(
    config: &Config,
    home: &Path,
) -> ironhermes_vault::VaultConfig {
    let mut vault_cfg = config.vault.clone();
    if vault_cfg.rusty_vault.data_dir.as_os_str().is_empty() {
        vault_cfg.rusty_vault.data_dir = home.join("vault");
    }
    vault_cfg
}

/// Resolve `config.vault` against the real runtime home directory
/// ([`crate::get_hermes_home`] — `IRONHERMES_HOME` or `~/.ironhermes`). Every production
/// `open_store`/`RustyVaultStore::open` call site (server, cron-runner, CLI) should route
/// through this so they all agree on the same on-disk vault location as `vault init`.
pub fn resolve_vault_config(config: &Config) -> ironhermes_vault::VaultConfig {
    resolve_vault_config_with_home(config, &get_hermes_home())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn empty_data_dir_resolves_to_home_join_vault() {
        let config = Config::default();
        assert!(
            config.vault.rusty_vault.data_dir.as_os_str().is_empty(),
            "precondition: default config data_dir sentinel is empty"
        );

        let home = PathBuf::from("/tmp/fake-home-for-test");
        let resolved = resolve_vault_config_with_home(&config, &home);

        assert_eq!(
            resolved.rusty_vault.data_dir,
            home.join("vault"),
            "empty sentinel must resolve to home.join(\"vault\")"
        );
    }

    #[test]
    fn explicit_data_dir_passes_through_unchanged() {
        let mut config = Config::default();
        let explicit = PathBuf::from("/opt/custom/vault-dir");
        config.vault.rusty_vault.data_dir = explicit.clone();

        let home = PathBuf::from("/tmp/fake-home-for-test");
        let resolved = resolve_vault_config_with_home(&config, &home);

        assert_eq!(
            resolved.rusty_vault.data_dir, explicit,
            "an explicitly-set data_dir must never be overwritten"
        );
    }

    #[test]
    fn preserves_other_vault_fields() {
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.backend = "rusty-vault".to_string();
        config.vault.rusty_vault.unseal_mode = "passphrase".to_string();

        let home = PathBuf::from("/tmp/fake-home-for-test");
        let resolved = resolve_vault_config_with_home(&config, &home);

        assert!(resolved.enabled);
        assert_eq!(resolved.backend, "rusty-vault");
        assert_eq!(resolved.rusty_vault.unseal_mode, "passphrase");
    }
}
