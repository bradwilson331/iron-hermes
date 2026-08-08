//! Phase 46.8 UAT gap G-46.8-1 — end-to-end regression test mirroring the SERVER path.
//!
//! Root cause: `RustyVaultConfig::data_dir` defaults to an empty `PathBuf` sentinel.
//! Before the fix, production `open_store` call sites (server, cron-runner) passed
//! `&config.vault` straight through with that sentinel unresolved — `RustyVaultStore::open`
//! then looked at data_dir `""`, which is never an initialized vault, and hard-errored with
//! `VaultError::NotInitialized`. This test proves the fix: init a REAL rusty-vault vault
//! under a `home.join("vault")` directory, then open it through
//! `resolve_vault_config_with_home` with `config.vault.rusty_vault.data_dir` left EMPTY
//! (the exact sentinel state every runtime call site starts from) — it must open `Ok`, not
//! `NotInitialized`.
//!
//! `#![cfg(feature = "rusty-vault")]`-gated (via Cargo.toml — see module doc below) because
//! it needs the concrete `RustyVaultStore` type, matching `ironhermes-vault`'s own
//! feature-gating posture (D-10: off by default, no behavioral change to non-feature builds).

#![cfg(feature = "rusty-vault")]

use ironhermes_core::config::Config;
use ironhermes_core::resolve_vault_config_with_home;
use ironhermes_vault::{RustyVaultStore, open_store};

/// Mirrors the SERVER path (`AppState::init` / `run_cron_job`) end-to-end:
/// 1. Real `RustyVaultStore::init` writes an actual vault to `home.join("vault")`
///    (the same on-disk location `resolve_vault_config_with_home` resolves the
///    empty sentinel to).
/// 2. Build a `Config` with `vault.enabled = true`, `vault.backend = "rusty-vault"`,
///    and — critically — `vault.rusty_vault.data_dir` left EMPTY (the sentinel state
///    every production call site starts from before this fix).
/// 3. Resolve via `resolve_vault_config_with_home(&config, home)` — NOT the real
///    `resolve_vault_config`, so this test never touches `std::env`/`IRONHERMES_HOME`
///    (avoiding the process-global-env test flake called out project-wide).
/// 4. `open_store(&resolved)` — pre-fix, `resolved.rusty_vault.data_dir` would still be
///    the empty sentinel and this would return `Err` (`VaultError::NotInitialized`).
///    Post-fix, it opens `Ok` against the real vault written in step 1.
#[tokio::test]
async fn resolver_opens_default_configured_vault_from_empty_sentinel() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let home = tmp.path();

    // Step 1: init a REAL vault at home.join("vault") — the exact location the
    // resolver fills the empty sentinel with.
    let init_data_dir = home.join("vault");
    let init_config = ironhermes_vault::RustyVaultConfig {
        data_dir: init_data_dir.clone(),
        unseal_mode: "keyfile".to_string(),
    };
    RustyVaultStore::init(&init_config).expect("vault init should succeed against a fresh dir");

    // Step 2: a Config whose vault.rusty_vault.data_dir is the EMPTY sentinel —
    // reproducing exactly what every pre-fix runtime call site passed to open_store.
    let mut config = Config::default();
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();
    assert!(
        config.vault.rusty_vault.data_dir.as_os_str().is_empty(),
        "precondition: data_dir sentinel must be empty before resolution"
    );

    // Step 3: resolve against `home` (pure — no env mutation).
    let resolved = resolve_vault_config_with_home(&config, home);
    assert_eq!(
        resolved.rusty_vault.data_dir, init_data_dir,
        "resolved data_dir must match where the vault was actually initialized"
    );

    // Step 4: open_store against the RESOLVED config — this is the exact call shape
    // every production site now uses. Must succeed, not NotInitialized.
    let store = open_store(&resolved)
        .expect("open_store must succeed: the sentinel was resolved to the real vault location");

    // Sanity: the opened store is actually usable (round-trip a secret), proving this
    // is a live, unsealed store — not just a non-erroring stub.
    store
        .put_secret(
            "regression-test-key",
            secrecy::SecretString::from("regression-test-value".to_string()),
        )
        .await
        .expect("put_secret should succeed on a freshly-opened, auto-unsealed keyfile vault");

    use secrecy::ExposeSecret;
    let fetched = store
        .get_secret("regression-test-key")
        .await
        .expect("get_secret should succeed")
        .expect("secret should be present after put_secret");
    assert_eq!(fetched.expose_secret(), "regression-test-value");
}

/// Negative control: opening `open_store` with the RAW (unresolved) empty-sentinel
/// config against the same real vault must fail with `NotInitialized` — proving this
/// test actually exercises the bug's failure mode and isn't vacuously passing.
#[tokio::test]
async fn unresolved_empty_sentinel_fails_not_initialized() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let home = tmp.path();

    let init_data_dir = home.join("vault");
    let init_config = ironhermes_vault::RustyVaultConfig {
        data_dir: init_data_dir,
        unseal_mode: "keyfile".to_string(),
    };
    RustyVaultStore::init(&init_config).expect("vault init should succeed against a fresh dir");

    let mut config = Config::default();
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();
    assert!(config.vault.rusty_vault.data_dir.as_os_str().is_empty());

    // Pre-fix behavior: pass the RAW config.vault straight through, unresolved.
    // `Box<dyn SecretStore>` isn't `Debug`, so use `.err()` (Option::expect has no
    // Debug bound) rather than `Result::expect_err`.
    let result = open_store(&config.vault);
    let err = result
        .err()
        .expect("opening the unresolved empty-sentinel config must fail");
    assert!(
        err.to_string().contains("not initialized") || err.to_string().contains("NotInitialized"),
        "expected a NotInitialized-flavored error, got: {err}"
    );
}
