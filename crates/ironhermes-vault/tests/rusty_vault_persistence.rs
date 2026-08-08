//! Phase 46.8 Plan 03 — restart-persistence + sealed-hard-error integration tests for
//! `RustyVaultStore` (D-05/D-07/D-09).
//!
//! D-15: every value stored/read here is a throwaway placeholder (`"spike-placeholder-value"`,
//! `"first-value"`/`"second-value"`, `"<key>-placeholder"`) — no real credential. Each test
//! uses its own `tempfile::TempDir` data dir (no shared state, no env mutation — RESEARCH
//! Pitfall 5).
//!
//! Feature-gated: only compiles/runs with `--features rusty-vault` (default build never
//! touches this file or the `rusty_vault`/openssl dependency tree — D-10).
#![cfg(feature = "rusty-vault")]

use ironhermes_vault::{RustyVaultConfig, RustyVaultStore, SecretStore};
use secrecy::{ExposeSecret, SecretString};

fn config_for(data_dir: std::path::PathBuf, unseal_mode: &str) -> RustyVaultConfig {
    RustyVaultConfig {
        data_dir,
        unseal_mode: unseal_mode.to_string(),
    }
}

/// D-05 (backstop): the `file` backend persists a written secret across a full process
/// restart — a fresh `RustyVaultStore::open` from the same `data_dir` reads back the same
/// key. Named to match the VALIDATION map (`rusty_vault_restart_persistence`).
#[tokio::test]
async fn rusty_vault_restart_persistence() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let data_dir = tmp.path().join("vault");
    let cfg = config_for(data_dir, "keyfile");

    RustyVaultStore::init(&cfg).expect("vault init");

    {
        // First store instance: write, then drop (simulates process exit).
        let store = RustyVaultStore::open(&cfg).expect("vault open (keyfile auto-unseal)");
        store
            .put_secret(
                "anthropic",
                SecretString::from("spike-placeholder-value".to_string()),
            )
            .await
            .expect("put_secret");
    }

    // Fresh store instance, same data_dir — proves file-backend + AES-GCM barrier
    // persistence across a full "restart" (no shared in-memory state with the first store).
    let store2 = RustyVaultStore::open(&cfg).expect("fresh vault open from same data_dir");
    let value = store2
        .get_secret("anthropic")
        .await
        .expect("get_secret")
        .expect("secret must survive a fresh Core reload from the same data_dir");
    assert_eq!(value.expose_secret(), "spike-placeholder-value");
}

/// D-07 hard-error precondition, proven at the backend layer: a sealed store's `get_secret`
/// returns `Err`, never `Ok(None)` — a broken/sealed vault must never masquerade as "key
/// absent".
#[tokio::test]
async fn sealed_store_get_secret_hard_errors_not_ok_none() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let data_dir = tmp.path().join("vault");
    let cfg = config_for(data_dir, "passphrase");

    RustyVaultStore::init(&cfg).expect("vault init");

    // passphrase mode: open() deliberately leaves the store SEALED (no unlock() call yet).
    let store = RustyVaultStore::open(&cfg).expect("vault open (passphrase mode, sealed)");
    let result = store.get_secret("anthropic").await;
    assert!(
        result.is_err(),
        "a sealed vault must hard-error (D-07), never Ok(None)"
    );
}

/// D-09 empty edge: deleting an absent key is idempotent, not an error.
#[tokio::test]
async fn delete_secret_on_absent_key_is_idempotent() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let data_dir = tmp.path().join("vault");
    let cfg = config_for(data_dir, "keyfile");

    RustyVaultStore::init(&cfg).expect("vault init");
    let store = RustyVaultStore::open(&cfg).expect("vault open");

    store
        .delete_secret("never-existed")
        .await
        .expect("deleting an absent key must be Ok(()), not an error");
}

/// D-09 ordering + adjacency: `list_secrets` returns sorted names, and an optional prefix
/// filters without disturbing sort order.
#[tokio::test]
async fn list_secrets_returns_sorted_names() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let data_dir = tmp.path().join("vault");
    let cfg = config_for(data_dir, "keyfile");

    RustyVaultStore::init(&cfg).expect("vault init");
    let store = RustyVaultStore::open(&cfg).expect("vault open");

    for key in ["zeta", "alpha", "mid"] {
        store
            .put_secret(key, SecretString::from(format!("{key}-placeholder")))
            .await
            .expect("put_secret");
    }

    let names = store.list_secrets(None).await.expect("list_secrets");
    assert_eq!(names, vec!["alpha", "mid", "zeta"]);

    let filtered = store
        .list_secrets(Some("a"))
        .await
        .expect("list_secrets with prefix");
    assert_eq!(filtered, vec!["alpha"]);
}

/// D-09: `put_secret` on an existing key is a flat KV v1 overwrite — old value gone, no
/// version history, two distinct keys never collide.
#[tokio::test]
async fn put_secret_overwrite_leaves_only_latest_value() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let data_dir = tmp.path().join("vault");
    let cfg = config_for(data_dir, "keyfile");

    RustyVaultStore::init(&cfg).expect("vault init");
    let store = RustyVaultStore::open(&cfg).expect("vault open");

    store
        .put_secret("anthropic", SecretString::from("first-value".to_string()))
        .await
        .expect("put_secret (first)");
    store
        .put_secret("anthropic", SecretString::from("second-value".to_string()))
        .await
        .expect("put_secret (overwrite)");
    store
        .put_secret("openai", SecretString::from("distinct-value".to_string()))
        .await
        .expect("put_secret (distinct key)");

    let anthropic = store
        .get_secret("anthropic")
        .await
        .expect("get_secret")
        .expect("value present");
    assert_eq!(anthropic.expose_secret(), "second-value");

    let openai = store
        .get_secret("openai")
        .await
        .expect("get_secret")
        .expect("value present");
    assert_eq!(
        openai.expose_secret(),
        "distinct-value",
        "distinct keys must never collide"
    );
}

/// T-46.8-16 (46.8-gap): `secret_path` interpolates the key directly into
/// `secret/providers/<key>` with no filtering — a key containing `/` or `..`
/// could otherwise escape the fixed KV mount prefix. Every CRUD method must
/// reject a malicious key name with a `VaultError`, never panic, and never
/// reach the backend.
#[tokio::test]
async fn crud_methods_reject_path_traversal_keys() {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let data_dir = tmp.path().join("vault");
    let cfg = config_for(data_dir, "keyfile");

    RustyVaultStore::init(&cfg).expect("vault init");
    let store = RustyVaultStore::open(&cfg).expect("vault open");

    let malicious_keys = ["../root", "a/b", "..", "/etc/passwd", ""];

    for key in malicious_keys {
        let get_err = store.get_secret(key).await;
        assert!(
            get_err.is_err(),
            "get_secret must reject traversal-shaped key {key:?}, got {get_err:?}"
        );

        let put_err = store
            .put_secret(key, SecretString::from("placeholder".to_string()))
            .await;
        assert!(
            put_err.is_err(),
            "put_secret must reject traversal-shaped key {key:?}, got {put_err:?}"
        );

        let delete_err = store.delete_secret(key).await;
        assert!(
            delete_err.is_err(),
            "delete_secret must reject traversal-shaped key {key:?}, got {delete_err:?}"
        );
    }

    // Sanity check: a legitimate key is unaffected by the validation.
    store
        .put_secret("anthropic", SecretString::from("legit-value".to_string()))
        .await
        .expect("put_secret with a legitimate key must still succeed");
    let value = store
        .get_secret("anthropic")
        .await
        .expect("get_secret")
        .expect("value present");
    assert_eq!(value.expose_secret(), "legit-value");
}
