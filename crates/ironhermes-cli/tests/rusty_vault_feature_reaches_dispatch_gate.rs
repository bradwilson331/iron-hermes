//! Phase 51 UAT gap-closure (post-07): regression gate proving the
//! `rusty-vault` cargo feature actually REACHES `ironhermes_core::dispatch_gate`'s
//! vault branch THROUGH `ironhermes-cli`'s own feature-forwarding chain — not
//! merely that the branch compiles when `ironhermes-core`/`ironhermes-kanban`
//! are tested directly with `--all-features`.
//!
//! # The incident this file exists to prevent from recurring
//!
//! `ironhermes-cli/Cargo.toml`'s `rusty-vault` feature forwarded ONLY to
//! `ironhermes-vault/rusty-vault` for ~6 plans (Phase 46.8 Plan 05 through
//! Phase 51 Plan 07). `ironhermes-core::dispatch_gate`'s vault branch AND
//! `ironhermes-kanban`'s dispatcher-side wiring are gated behind THEIR OWN,
//! SEPARATE `rusty-vault` features — so `cargo build -p ironhermes-cli
//! --features rusty-vault` produced a binary where `ironhermes vault
//! init/unlock/set/list` worked (they call `ironhermes-vault` directly) while
//! the REAL production dispatch path (the gateway-hosted dispatcher, which
//! calls `ironhermes_core::dispatch_gate::evaluate_profile_dispatch`) silently
//! compiled the feature-ABSENT arm. A live operator UAT against a genuinely
//! reachable, unsealed, seeded vault produced:
//!
//! ```text
//! reason=profile "uat-vault" is configured for provider "openrouter" but its
//! vault-backed credential resolution is unreachable (vault backend
//! unavailable — the `rusty-vault` feature was not compiled in)
//! ```
//!
//! Every gate this phase's plans ran before that UAT was green:
//! `cargo nextest -p ironhermes-core --all-features` and
//! `cargo nextest -p ironhermes-kanban --all-features` both genuinely exercise
//! the vault branch WITHIN those crates — because `--all-features` on a LEAF
//! crate turns on that crate's OWN feature directly, with no forwarding
//! involved at all. Neither run says anything about whether a DOWNSTREAM
//! crate's feature declaration actually reaches them. This file closes that
//! blind spot by building `ironhermes-cli`'s OWN test binary with
//! `--features rusty-vault` and proving the SAME vault branch is reachable
//! from THIS crate's build — the one the real `ironhermes` binary (and the
//! gateway process it hosts) actually ships.
//!
//! Compiled and run ONLY under `--features rusty-vault` (the same invocation
//! this plan's other real-vault tests use) — on default features this file
//! has zero tests, which is expected and not a false negative for THIS
//! specific regression (a default-features build never claims to support the
//! vault backend at all).

#![cfg(feature = "rusty-vault")]

use std::path::Path;

use ironhermes_core::config::{Config, ProviderConfig};
use secrecy::SecretString;
use tempfile::TempDir;

const PROFILE: &str = "cli-feature-gate-check";
const PROVIDER: &str = "clifeaturegateprovider";
const SECRET_VALUE: &str = "sk-cli-feature-forward-check";

/// Mirrors `dispatch_gate_loop.rs`'s own `write_profile` helper exactly —
/// `profiles_root.join(name)/config.yaml` (+ optional `.env`).
fn write_profile(root: &Path, name: &str, config: &Config) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("mkdir profile dir");
    config
        .save_to(&dir.join("config.yaml"))
        .expect("save_to config.yaml");
    // No .env at all — mirrors the operator's own UAT profile shape exactly
    // (vault-only, no plaintext fallback).
}

/// Real vault, real endpoint semantics: init at `data_dir`, then write the
/// profile's own secret via `ProfileSecretStore`, EXACTLY as
/// `ironhermes-cli/tests/worker_vault_bootstrap.rs`'s `real_vault` module and
/// `ironhermes-kanban/src/dispatcher.rs`'s `vault_mint_tests` module already do
/// for the rest of this plan's coverage — the same proven pattern, applied one
/// layer higher (through the gate, not the endpoint).
async fn init_vault_with_secret(data_dir: &Path) {
    let rv_config = ironhermes_vault::RustyVaultConfig {
        data_dir: data_dir.to_path_buf(),
        unseal_mode: "keyfile".to_string(),
    };
    ironhermes_vault::RustyVaultStore::init(&rv_config).expect("vault init");
    let store = ironhermes_vault::RustyVaultStore::open(&rv_config).expect("vault open");
    let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(&store);
    profile_store
        .put_profile_secret(PROFILE, PROVIDER, SecretString::from(SECRET_VALUE.to_string()))
        .await
        .expect("write profile secret");
}

/// THE regression test: a profile with NO `.env`, a genuinely reachable and
/// seeded vault, must resolve to `AllowFromVault` — not a `Refuse` naming the
/// feature as absent — when the dispatch gate is exercised from a test binary
/// built with `cargo test -p ironhermes-cli --features rusty-vault`, which is
/// the same feature-resolution the real `ironhermes` binary gets.
///
/// `flavor = "multi_thread"` required — `RustyVaultStore::init`/`open` bridge
/// via `spawn_blocking` + a blocking `rx.recv()` on the calling thread, a
/// pattern that deadlocks under `#[tokio::test]`'s default `current_thread`
/// flavor (see `ironhermes-kanban/src/dispatcher.rs`'s `vault_mint_tests`
/// module doc for the full mechanism).
#[tokio::test(flavor = "multi_thread")]
async fn rusty_vault_feature_reaches_the_dispatch_gate_through_ironhermes_cli() {
    let tmp = TempDir::new().unwrap();
    let profiles_root = tmp.path().join("profiles");
    std::fs::create_dir_all(&profiles_root).unwrap();
    let vault_dir = tmp.path().join("vault");

    init_vault_with_secret(&vault_dir).await;

    let mut config = Config::default();
    config.vault.enabled = true;
    config.vault.backend = "rusty-vault".to_string();
    config.vault.rusty_vault.data_dir = vault_dir;
    config.vault.rusty_vault.unseal_mode = "keyfile".to_string();
    config.model.provider = PROVIDER.to_string();
    config.providers.insert(
        PROVIDER.to_string(),
        ProviderConfig {
            api_key_env: Some("CLI_FEATURE_GATE_CHECK_API_KEY".to_string()),
            ..Default::default()
        },
    );
    write_profile(&profiles_root, PROFILE, &config);

    let decision =
        ironhermes_core::dispatch_gate::evaluate_profile_dispatch_at(&profiles_root, PROFILE)
            .await;

    match decision {
        ironhermes_core::dispatch_gate::DispatchDecision::AllowFromVault => {
            // Correct — the vault branch resolved the seeded secret.
        }
        ironhermes_core::dispatch_gate::DispatchDecision::Refuse { reason } => {
            assert!(
                !reason.contains("rusty-vault` feature was not compiled in"),
                "REGRESSION (the exact incident this file exists to catch): the \
                 dispatch gate reports the rusty-vault feature as absent even \
                 though this test binary was built with --features rusty-vault \
                 and a genuinely reachable, seeded vault. This means \
                 ironhermes-cli/Cargo.toml's `rusty-vault` feature forward is \
                 broken again — it must enable ironhermes-core/rusty-vault (and \
                 ironhermes-kanban/rusty-vault), not only ironhermes-vault/rusty-vault. \
                 Full reason: {reason}"
            );
            panic!(
                "expected AllowFromVault for a genuinely reachable, seeded, \
                 .env-less profile; got Refuse instead (not the feature-absent \
                 case, but still wrong): {reason}"
            );
        }
        ironhermes_core::dispatch_gate::DispatchDecision::Allow => {
            panic!(
                "expected AllowFromVault (this profile has no .env at all), got \
                 the plain Allow variant — the vault branch may not have been \
                 reached at all"
            );
        }
    }
}
