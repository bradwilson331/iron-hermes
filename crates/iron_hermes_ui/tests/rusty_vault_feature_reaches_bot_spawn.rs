//! Phase 51 Plan 10 UAT gap-closure: proves the `rusty-vault` cargo feature actually
//! REACHES `ironhermes_core::profile_credentials::decide_spawn_credential` — the bot
//! spawn path's credential decision — THROUGH `iron_hermes_ui`'s own feature-forwarding
//! chain, not merely that the decision compiles when `ironhermes-core` is tested
//! directly with `--all-features`.
//!
//! # Why this mirrors, but does not reuse, `rusty_vault_feature_reaches_dispatch_gate.rs`
//!
//! That sibling file proves the SAME class of forward for `evaluate_profile_dispatch_at`
//! (the gate alone). The bot path needs `ironhermes-core/rusty-vault` for the mint and
//! host halves too, not only for the gate's vault branch — `--all-features` on a leaf
//! crate cannot substitute for that, because it turns on that crate's own feature
//! directly, with no forwarding involved at all, and says nothing about whether a
//! DOWNSTREAM crate's feature declaration reaches it. This file closes that blind spot
//! for the bot spawn path specifically.
//!
//! This is a REACHABILITY proof, not a functional proof of the vault-backed socket
//! read — that proof lives in `ironhermes-core`'s own `spawn_credential_decision.rs`
//! (currently `#[ignore]`d as a documented repro of an upstream `rusty_vault` defect;
//! see `51-10-SUMMARY.md`). This file only asks: does a `Vault` decision come back at
//! all, through THIS crate's own build, when the vault is genuinely reachable and
//! seeded? It does not read the minted token back over any socket.
//!
//! Compiled and run ONLY under `--features rusty-vault` — on default features this file
//! has zero tests, which is expected and not a false negative for THIS specific
//! regression (a default-features build never claims to support the vault backend).
//!
//! # Mutation-test finding (Phase 51 Plan 10)
//!
//! Severing `iron_hermes_ui`'s OWN direct `"ironhermes-core/rusty-vault"` line from its
//! `rusty-vault` feature list, alone, does NOT turn this test red — `ironhermes-kanban`'s
//! and `ironhermes-cli`'s own `rusty-vault` features each independently re-forward to
//! `ironhermes-core/rusty-vault`, and this crate still activates both of those. All
//! THREE paths had to be severed together (this crate's direct line, plus
//! `ironhermes-kanban/Cargo.toml`'s `rusty-vault = ["ironhermes-core/rusty-vault"]`,
//! plus the same entry in `ironhermes-cli/Cargo.toml`) before reachability actually
//! broke. When it did break, the failure was a COMPILE-TIME error
//! (`E0599: no method named \`shutdown\` found for struct \`ProfileCredentialHost\``),
//! not a runtime refuse-reason string — severing the forward swaps the type alias
//! `ProfileCredentialHost = ironhermes_vault::profile_endpoint::ProfileCredentialEndpointHandle`
//! for the crate's deliberately-uninhabited feature-off stand-in struct (see
//! `ironhermes-core/src/profile_credentials.rs`'s doc comment on that struct), which has
//! no such method. That is a STRONGER, earlier-failing signal than a runtime message
//! would have been — the build itself cannot produce a binary that lies about vault
//! support. Restoring all three lines returned this test to green. See
//! `51-10-SUMMARY.md` for the full mutation-test log.

#![cfg(feature = "rusty-vault")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ironhermes_core::config::{Config, ProviderConfig};
use ironhermes_core::profile_credentials::{ProfileTokenAudit, SpawnCredentialDecision};
use secrecy::SecretString;
use tempfile::TempDir;

const PROFILE: &str = "ui-bot-spawn-feature-gate-check";
const PROVIDER: &str = "uibotspawnfeaturegateprovider";
const SECRET_VALUE: &str = "sk-ui-bot-spawn-feature-forward-check";

struct NoopAudit;
impl ProfileTokenAudit for NoopAudit {
    fn record_mint(&self, _slug: &str, _accessor: &str, _ttl: Duration) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Mirrors `dispatch_gate_loop.rs`'s own `write_profile` helper exactly —
/// `profiles_root.join(name)/config.yaml` (no `.env` — vault-only, matching the
/// operator's own UAT profile shape).
fn write_profile(root: &Path, name: &str, config: &Config) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("mkdir profile dir");
    config.save_to(&dir.join("config.yaml")).expect("save_to config.yaml");
}

/// Real vault, real endpoint semantics — mirrors
/// `rusty_vault_feature_reaches_dispatch_gate.rs`'s `init_vault_with_secret` exactly.
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

/// THE regression test: a profile with NO `.env`, a genuinely reachable and seeded
/// vault, must resolve to `decide_spawn_credential`'s `Vault` arm — not a `Refuse`
/// naming the feature as absent — when driven from a test binary built with
/// `cargo test -p iron_hermes_ui --features rusty-vault,server`, the same
/// feature-resolution a real UI server build gets.
///
/// `flavor = "multi_thread"` required — `RustyVaultStore::init`/`open` bridge via
/// `spawn_blocking` + a blocking `rx.recv()` on the calling thread, which deadlocks
/// under `#[tokio::test]`'s default `current_thread` flavor.
#[tokio::test(flavor = "multi_thread")]
async fn rusty_vault_feature_reaches_the_bot_spawn_credential_decision() {
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
            api_key_env: Some("UI_BOT_SPAWN_FEATURE_GATE_CHECK_API_KEY".to_string()),
            ..Default::default()
        },
    );
    write_profile(&profiles_root, PROFILE, &config);

    let host = ironhermes_core::profile_credentials::host_profile_credentials(&config)
        .expect("a genuinely reachable, unsealed vault must yield a hosted credential endpoint");
    let sink: Arc<dyn ProfileTokenAudit> = Arc::new(NoopAudit);

    let decision = ironhermes_core::profile_credentials::decide_spawn_credential(
        &config,
        &profiles_root,
        PROFILE,
        Some(&host),
        Some(sink.as_ref()),
    )
    .await;

    match decision {
        SpawnCredentialDecision::Vault(_) => {
            // Correct — the vault arm resolved, minted, and ledgered.
        }
        SpawnCredentialDecision::Refuse { reason, .. } => {
            assert!(
                !reason.contains("rusty-vault` feature was not compiled in")
                    && !reason.to_lowercase().contains("feature")
                    && !reason.to_lowercase().contains("not compiled"),
                "REGRESSION (the exact incident this file exists to prevent): the bot \
                 spawn credential decision reports the vault as unavailable even though \
                 this test binary was built with --features rusty-vault and a genuinely \
                 reachable, seeded vault. This means iron_hermes_ui/Cargo.toml's \
                 `rusty-vault` feature forward is broken again for the decision function's \
                 mint/host halves (not merely the gate). Full reason: {reason}"
            );
            panic!(
                "expected the Vault arm for a genuinely reachable, seeded, .env-less \
                 profile; got Refuse instead (not the feature-absent shape, but still \
                 wrong): {reason}"
            );
        }
        SpawnCredentialDecision::Dotenv => {
            panic!(
                "expected the Vault arm (this profile has no .env at all) — got Dotenv, \
                 meaning the vault branch may not have been reached at all"
            );
        }
    }

    if let Ok(host) = Arc::try_unwrap(host) {
        host.shutdown().await;
    }
}
