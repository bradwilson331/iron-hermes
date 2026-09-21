//! Phase 51 Plan 17 (CR-05 / T-51-67, second half): proves the vault-aware
//! post-write gate re-check `profile_api::create_profile_impl` performs does
//! not block a current-thread tokio runtime — the acceptance criterion
//! `51-VERIFICATION.md` names as the missing assertion.
//!
//! A SEPARATE file, not a `#[cfg(test)]` mod inside `profile_api.rs` itself:
//! `profile_api_never_mentions_vault_storage`
//! (`tests/profile_key_masking.rs` / `tests/profile_scaffold.rs`) is a
//! literal-source shape lock asserting `profile_api.rs` never contains the
//! strings "ironhermes_vault"/"RustyVault" (D-06 — no vault storage call
//! site in that file), and this test needs a real `RustyVaultStore` fixture.
//! `create_profile_impl` is `pub(crate)`, so it is reachable from this
//! sibling module without any visibility widening.
//!
//! Gated on `rusty-vault` (mirrors `dispatch_gate_vault_backed.rs`) — every
//! test here needs a real `RustyVaultStore`/`ProfileSecretStore`.

use super::profile_api::create_profile_impl;
use crate::protocol::{KeyMode, SecretSource};
use ironhermes_core::config::{Config, ProviderConfig};
use ironhermes_vault::{ProfileSecretStore, RustyVaultConfig, RustyVaultStore};
use secrecy::SecretString;

/// RAII guard that sets an env var and restores the previous value on drop.
/// Duplicated verbatim per this codebase's own established "each test module
/// is its own namespace" precedent (see `profile_api.rs`'s
/// `profile_scaffold_tests::ScopedEnv` doc comment).
struct ScopedEnv {
    key: String,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: single-threaded test context; no concurrent env access.
        unsafe { std::env::set_var(key, value) };
        Self {
            key: key.to_string(),
            prev,
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // SAFETY: single-threaded test context; no concurrent env access.
        match &self.prev {
            Some(v) => unsafe { std::env::set_var(&self.key, v) },
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}

/// The credential decision `create_profile_impl`'s Step 9 post-write
/// re-check makes — `ironhermes_core::dispatch_gate::evaluate_profile_dispatch`,
/// which for a vault-backed profile now opens the vault via
/// `RustyVaultStore::open_async` (Plan 17, Task 1) — must not block a
/// current-thread runtime's only worker thread. Driven under
/// `#[tokio::test]`'s DEFAULT (current-thread) flavor deliberately: before
/// Task 1's `open_async` fix, this exact call reached `RustyVaultStore::open`'s
/// blocking `rx.recv()` bridge directly from this file's own vault-aware
/// gate re-check (CR-05's finding).
///
/// Bounded by `tokio::time::timeout` rather than
/// `dispatch_gate_vault_backed.rs`'s external `std::thread` + `recv_timeout`
/// watchdog: after the fix there is no raw OS-level blocking call left on
/// this thread at all, only a proper `.await` on `spawn_blocking`, which an
/// in-runtime timeout can legitimately race against and does not require an
/// external thread to detect a hang that can no longer occur by
/// construction.
#[tokio::test]
async fn create_profile_vault_gate_recheck_does_not_block_current_thread_runtime() {
    let vault_tmp = tempfile::tempdir().expect("tempdir");
    let rv_config = RustyVaultConfig {
        data_dir: vault_tmp.path().join("vault"),
        unseal_mode: "keyfile".to_string(),
    };
    RustyVaultStore::init(&rv_config).expect("vault init");
    let store = RustyVaultStore::open(&rv_config).expect("vault open");
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    profile_store
        .put_profile_secret(
            "vault-thread-safety-profile",
            "openrouter",
            SecretString::from("sk-fixture-vault-thread-safety-9f21ab".to_string()),
        )
        .await
        .expect("put profile secret");

    let dir = tempfile::tempdir().expect("tempdir");
    let _guard = ScopedEnv::set(
        "IRONHERMES_HOME",
        dir.path().to_str().expect("tempdir path must be utf8"),
    );

    // Root config carries the vault settings — create_profile_impl byte-
    // copies this into the new profile's own config.yaml, which is what the
    // gate re-check actually reads (never the caller's in-memory `Config`).
    // No root `.env`: the whole point is that the key comes from the vault,
    // not a scrubbed `.env`.
    let mut root_config = Config::default();
    root_config.model.provider = "openrouter".to_string();
    root_config.providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key_env: Some("OPENROUTER_API_KEY".to_string()),
            ..Default::default()
        },
    );
    root_config.vault.enabled = true;
    root_config.vault.backend = "rusty-vault".to_string();
    root_config.vault.rusty_vault = rv_config;
    root_config
        .save_to(&dir.path().join("config.yaml"))
        .expect("save root config.yaml");

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        create_profile_impl(
            "vault-thread-safety-profile",
            &KeyMode::LlmOnly,
            false,
            Vec::new(),
            SecretSource::RootEnv,
            &root_config,
        ),
    )
    .await;

    match result {
        Ok(Ok(_rows)) => {
            // Correct — AllowFromVault is not a Refuse, so
            // create_profile_impl returns Ok even with an empty .env.
        }
        Ok(Err(e)) => panic!(
            "create_profile_impl returned an unexpected refusal for a genuinely reachable, \
             seeded vault: {e}"
        ),
        Err(_) => panic!(
            "create_profile_impl's vault-aware post-write gate re-check deadlocked under a \
             current-thread tokio runtime within 15s — this is precisely the CR-05 / T-51-67 \
             hazard open_async exists to make unreachable"
        ),
    }
}
