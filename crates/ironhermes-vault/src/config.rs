//! [`VaultConfig`] / [`RustyVaultConfig`] — serde config structs for the vault subsystem
//! (Phase 46.8 D-10).
//!
//! D-10: `VaultConfig::default()` is a zero-behavioral-change default (`enabled: false`,
//! `backend: "env-var"`) so pre-46.8 `config.yaml` files parse cleanly via `#[serde(default)]`,
//! mirroring `AuditConfig` / `AutonomousConfig` in `ironhermes-core::config`.
//!
//! This crate has no dependency on `ironhermes-core` (D-01 zero-cycle direction), so
//! `RustyVaultConfig::data_dir` cannot resolve `get_hermes_home()` here — it stays an empty
//! `PathBuf` sentinel; the consumer (Plan 03's `RustyVaultStore` construction, in
//! `ironhermes-core`) resolves the runtime default of `get_hermes_home().join("vault")` when
//! the field is empty.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

fn default_backend() -> String {
    "env-var".to_string()
}

fn default_unseal_mode() -> String {
    "keyfile".to_string()
}

/// Top-level vault configuration (Phase 46.8 D-10).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    /// When true, the configured `backend` is consulted as an API-key fallback after the
    /// existing 4-priority resolution chain (D-07: priorities 1-4 always win). Default
    /// `false` — zero behavioral change for pre-46.8 configs.
    pub enabled: bool,
    /// Backend selector. Default `"env-var"` (the always-on diagnostic-only backend).
    /// `"rusty-vault"` requires the crate's `rusty-vault` feature to be compiled in.
    #[serde(default = "default_backend")]
    pub backend: String,
    /// RustyVault-specific settings, consulted only when `backend == "rusty-vault"`.
    pub rusty_vault: RustyVaultConfig,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_backend(),
            rusty_vault: RustyVaultConfig::default(),
        }
    }
}

/// RustyVault backend settings (Phase 46.8 D-05).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RustyVaultConfig {
    /// On-disk data directory for the embedded RustyVault store. Empty by default — this
    /// leaf crate cannot call `get_hermes_home()` (D-01 zero-cycle direction), so an empty
    /// `PathBuf` is a sentinel meaning "unset"; the consumer resolves
    /// `get_hermes_home().join("vault")` at store-construction time (Plan 03).
    pub data_dir: PathBuf,
    /// Unseal mode: `"keyfile"` (default) or `"passphrase"` (D-05).
    #[serde(default = "default_unseal_mode")]
    pub unseal_mode: String,
}

impl Default for RustyVaultConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::new(),
            unseal_mode: default_unseal_mode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_env_var_backend() {
        let cfg = VaultConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.backend, "env-var");
        assert_eq!(cfg.rusty_vault.data_dir, PathBuf::new());
        assert_eq!(cfg.rusty_vault.unseal_mode, "keyfile");
    }
}
