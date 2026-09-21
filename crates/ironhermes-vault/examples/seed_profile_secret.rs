//! Operator scaffolding for seeding and removing a profile-scoped vault entry. NOT a
//! migration tool — see "Why this SEEDS rather than MIGRATES" below. Committed and tracked;
//! retained until the profile-vault CLI that F-03 identifies as missing exists, at which
//! point this file and its `[[example]]` declaration in `Cargo.toml` are removed together in
//! one commit.
//!
//! # Why this exists
//!
//! Seeding `secret/profiles/{slug}/{leaf}` has no CLI surface yet: `ironhermes vault set` writes
//! through `SECRET_MOUNT_PREFIX` (`secret/providers/`) and is structurally unable to reach the
//! profiles mount. The per-profile operator command (F-03) has not shipped. Its `--delete`
//! branch is, as of Phase 51 Plan 12, the ONLY way to remove a profile-scoped vault entry —
//! UAT check-10 cleanup on the `uat-vault` fixture still depends on it.
//!
//! # Why this SEEDS rather than MIGRATES
//!
//! A real migration must read a profile `.env`. `dotenvy` resolves `${VAR}` references against the
//! reading process's own environment, and the CLI loads the ROOT `.env` into that environment at
//! startup — so a naive profile-`.env` read writes the operator's root credential into a profile's
//! vault entry (the CR-03 exfiltration that plan 51-08 exists to avoid). This example never opens a
//! profile `.env` at all: the value arrives on stdin from the operator. The trap is unreachable
//! rather than merely avoided.
//!
//! It is also purely additive — it writes one secret and destroys nothing, so it cannot strand a
//! profile the way an early migration could.
//!
//! # Usage
//!
//! ```text
//! printf '%s' "$KEY" | cargo run -p ironhermes-vault --features rusty-vault \
//!     --example seed_profile_secret -- <slug> <leaf> [data_dir]
//! ```
//!
//! `leaf` is the provider name (e.g. `openrouter`). `data_dir` defaults to
//! `$IRONHERMES_HOME/vault`, falling back to `~/.ironhermes/vault` — matching what
//! `RustyVaultStore` resolves when `vault.rusty_vault.data_dir` is left empty in `config.yaml`.
//!
//! The value is read from stdin, never from argv — argv is visible in `ps` and shell history.

use std::io::Read;
use std::path::PathBuf;

use ironhermes_vault::{ProfileSecretStore, RustyVaultConfig, RustyVaultStore};
use secrecy::SecretString;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 3 {
        eprintln!(
            "usage: printf '%s' \"$KEY\" | seed_profile_secret <slug> <leaf> [data_dir]\n\
             \n\
             Seeds secret/profiles/<slug>/<leaf> from stdin. UAT scaffolding only."
        );
        std::process::exit(2);
    }
    let slug = &args[0];
    let leaf = &args[1];

    let data_dir: PathBuf = match args.get(2) {
        Some(d) => PathBuf::from(d),
        None => default_vault_dir()?,
    };
    if !data_dir.exists() {
        anyhow::bail!(
            "vault data dir does not exist: {}\n\
             Run `ironhermes vault init` first (CLI must be built with --features rusty-vault).",
            data_dir.display()
        );
    }

    // `--delete` as the value on stdin removes the entry instead of writing one. UAT step 10
    // ("delete the throwaway profile and its vault entries") has no CLI surface either.
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    if value.trim() == "--delete" {
        let config = RustyVaultConfig {
            data_dir: data_dir.clone(),
            unseal_mode: "keyfile".to_string(),
        };
        let store = RustyVaultStore::open(&config)?;
        let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
        profile_store.delete_profile_secret(slug, leaf).await?;
        println!("deleted secret/profiles/{slug}/{leaf}");
        return Ok(());
    }
    // A trailing newline from `echo` would be stored as part of the credential.
    let value = value.trim_end_matches(['\n', '\r']).to_string();
    if value.is_empty() {
        anyhow::bail!("refusing to seed an empty value — pipe the key on stdin");
    }

    let config = RustyVaultConfig {
        data_dir: data_dir.clone(),
        unseal_mode: "keyfile".to_string(),
    };
    let store = RustyVaultStore::open(&config)?;
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);

    // Slug and leaf are validated independently inside put_profile_secret; a bad one is refused
    // here rather than producing a subtly wrong address.
    profile_store
        .put_profile_secret(slug, leaf, SecretString::from(value))
        .await?;

    // The value is never echoed. The address is not a secret and is what the operator must verify.
    println!("seeded secret/profiles/{slug}/{leaf}  (data_dir: {})", data_dir.display());
    println!(
        "note: the profile-{slug} ACL policy is created on first dispatch by mint_profile_token, \
         not here."
    );
    Ok(())
}

fn default_vault_dir() -> anyhow::Result<PathBuf> {
    if let Ok(home) = std::env::var("IRONHERMES_HOME")
        && !home.is_empty()
    {
        return Ok(PathBuf::from(home).join("vault"));
    }
    let home = std::env::var("HOME")
        .map_err(|_| anyhow::anyhow!("neither IRONHERMES_HOME nor HOME is set"))?;
    Ok(PathBuf::from(home).join(".ironhermes").join("vault"))
}
