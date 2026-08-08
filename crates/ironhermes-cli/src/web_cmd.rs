//! `ironhermes web` CLI subcommand — operator credential generation for the
//! login boundary (Phase 47.3 Plan 03, D-08).
//!
//! Provides one subcommand:
//! - `set-password [--vault]` — prompt twice (masked) for the operator
//!   password, hash it with argon2id at the OWASP baseline parameters, and
//!   either print the resulting PHC string (default) or store it via the
//!   existing `SecretStore` (`--vault`).
//!
//! # Print-only by default (D-08)
//!
//! This command NEVER mutates the operator's configuration file. The default
//! flow prints the PHC string and names the exact key it belongs under
//! (`web_ui.auth.password_hash`) so the operator can paste it in by hand —
//! this explicitly overrides WIRING.md §5's drafted "offers to write it in"
//! behavior. The only persistence path is the opt-in `--vault` flag, which
//! reuses `vault_cmd`'s existing store-opening path rather than constructing
//! a `SecretStore` directly.
//!
//! # No plaintext in argv (T-47.3-12)
//!
//! There is deliberately no positional or `--password` argument. The
//! plaintext password exists only in the masked/piped prompt read by
//! [`read_secret_value`] (copied verbatim from `vault_cmd.rs`) — never argv,
//! never a log line, never the printed output.

use anyhow::{Context as _, Result};
use clap::Subcommand;
use colored::Colorize;

/// Vault key this command writes to under `--vault`, and the same literal
/// `crates/iron_hermes_ui/src/server/auth.rs`'s `auth_config_from` reads.
/// Proven byte-identical by `tests::vault_key_literal_matches_server_auth_rs`
/// (Task 2) — the one place the two crates agree on a string with no
/// compiler-enforced link.
const PASSWORD_HASH_KEY: &str = "web_ui/auth/password_hash";

// ─────────────────────────────────────────────────────────────────────────────
// WebCommands — clap subcommand tree
// ─────────────────────────────────────────────────────────────────────────────

/// Web UI subcommands (Phase 47.3, D-08).
#[derive(Subcommand)]
pub enum WebCommands {
    /// Generate an argon2id password hash for the operator login boundary.
    ///
    /// Prompts twice (masked) for the password and prints the resulting PHC
    /// string, naming the exact config key `web_ui.auth.password_hash` to
    /// paste it under. Nothing is written to disk by default. Pass --vault
    /// to store the hash via the configured secret vault instead of
    /// printing it — exercises the same fallback layer the server's
    /// `auth_config_from` already consults.
    SetPassword {
        /// Store the hash via the operator's configured vault
        /// (`ironhermes vault`) instead of printing it to stdout.
        #[arg(long)]
        vault: bool,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch a `web` subcommand to the appropriate handler. Registered as
/// `Commands::Web { command }` in the dispatch match below.
pub async fn handle_web_command(cmd: WebCommands) -> Result<()> {
    match cmd {
        WebCommands::SetPassword { vault } => cmd_set_password(vault).await,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read a secret value: masked TTY prompt when interactive, otherwise a
/// single line from stdin (piped/non-interactive — CI, scripted usage, or
/// this crate's own integration tests). Never accepted as a bare argv token
/// either way. Copied verbatim from `vault_cmd.rs::read_secret_value` — do
/// not invent a second prompt idiom.
fn read_secret_value(prompt: &str) -> Result<String> {
    use std::io::IsTerminal as _;

    if std::io::stdin().is_terminal() {
        rpassword::prompt_password(prompt).context("failed to read secret value from terminal")
    } else {
        use std::io::BufRead as _;
        eprint!("{prompt}");
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("failed to read secret value from stdin")?;
        Ok(line.trim_end_matches(['\n', '\r']).to_string())
    }
}

/// Pure equality check between a password and its confirmation entry, kept
/// separate from stdin reading so it is unit-testable without a TTY (T-47.3-15).
fn confirm_match(password: &str, confirmation: &str) -> Result<()> {
    if password != confirmation {
        anyhow::bail!("passwords did not match — nothing was hashed or stored");
    }
    Ok(())
}

/// Hash a plaintext password with argon2id at the OWASP baseline parameters
/// (AUTH-DESIGN §3.2: m=19456 KiB, t=2, p=1). Rejects an empty input. Kept a
/// pure function so it is unit-testable without stdin (T-47.3-14).
///
/// Parameters are constructed explicitly rather than relying on
/// `Argon2::default()` — the OWASP baseline this command targets must stay
/// pinned even if a future argon2 release changes its own defaults.
pub fn hash_password(plain: &str) -> Result<String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    use argon2::{Algorithm, Argon2, Params, Version};

    if plain.is_empty() {
        anyhow::bail!("password must not be empty");
    }

    let params = Params::new(19456, 2, 1, None)
        .map_err(|e| anyhow::anyhow!("invalid argon2 parameters: {e}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("failed to hash password: {e}"))?
        .to_string();
    Ok(hash)
}

/// Store `hash` via the same vault store path `ironhermes vault set` uses —
/// `resolve_vault_config` → `open_store` → `put_secret`, so backend
/// selection, sealed-vault errors, and the `rusty-vault` feature gate all
/// behave identically to `ironhermes vault set` (Task 2). A sealed or
/// unavailable vault propagates via `?` rather than falling back to
/// printing, which would silently change where the operator believes the
/// credential lives.
async fn store_hash_in_vault(hash: String) -> Result<()> {
    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    let resolved = crate::vault_cmd::resolve_vault_config(&config);
    let store = ironhermes_vault::open_store(&resolved).context(
        "failed to open vault store — run `ironhermes vault init`/`unlock` first",
    )?;
    store
        .put_secret(PASSWORD_HASH_KEY, secrecy::SecretString::from(hash))
        .await
        .context("failed to store password hash in vault")?;
    println!(
        "{} stored the password hash under vault key {} — nothing was written to disk.",
        "✓".green().bold(),
        PASSWORD_HASH_KEY.bold()
    );
    Ok(())
}

async fn cmd_set_password(vault: bool) -> Result<()> {
    let password = read_secret_value("New operator password: ")?;
    let confirm = read_secret_value("Confirm password: ")?;
    confirm_match(&password, &confirm)?;
    let hash = hash_password(&password)?;

    if vault {
        store_hash_in_vault(hash).await
    } else {
        println!(
            "Paste this into the operator's configuration under {}:\n",
            "web_ui.auth.password_hash".bold()
        );
        println!("{hash}");
        println!("\nNothing was written to disk.");
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests (Task 1 RED — these must fail before the GREEN commit implements
// hash_password/confirm_match)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Hashing the same password twice must produce two different PHC
    /// strings (random salt), and both must verify against the original
    /// password and reject a wrong one — proven via the same
    /// `PasswordHash`/`Argon2::default().verify_password` verifier the
    /// server's `AuthState::verify_password` uses, not a re-derived
    /// assumption of our own.
    #[test]
    fn hash_password_round_trips_with_random_salt() {
        let h1 = hash_password("hunter2").unwrap();
        let h2 = hash_password("hunter2").unwrap();
        assert_ne!(
            h1, h2,
            "two hashes of the same password must differ (random salt)"
        );

        use argon2::password_hash::PasswordHash;
        use argon2::{Argon2, PasswordVerifier};
        for h in [&h1, &h2] {
            let parsed =
                PasswordHash::new(h).expect("produced string must be a valid PHC string");
            assert!(
                Argon2::default()
                    .verify_password(b"hunter2", &parsed)
                    .is_ok(),
                "hash must verify against the original password"
            );
            assert!(
                Argon2::default()
                    .verify_password(b"hunter3", &parsed)
                    .is_err(),
                "hash must NOT verify against a wrong password"
            );
        }
    }

    /// AuthState::new (iron_hermes_ui/src/server/auth.rs) validates a
    /// configured hash by calling exactly `PasswordHash::new(h)` and hard-
    /// erroring on failure. `iron_hermes_ui` is a bin-only crate and cannot
    /// be imported here, so this test proves the identical proxy condition:
    /// the produced string always parses as a valid PHC string, is
    /// `argon2id`, and encodes the explicit OWASP baseline params
    /// (m=19456,t=2,p=1) rather than relying on argon2's own defaults.
    #[test]
    fn hash_password_uses_argon2id_owasp_baseline_params() {
        let hash = hash_password("hunter2").unwrap();
        use argon2::password_hash::PasswordHash;
        let parsed = PasswordHash::new(&hash).expect("must parse as a valid PHC string");
        assert_eq!(parsed.algorithm.as_str(), "argon2id");
        assert!(
            hash.contains("m=19456,t=2,p=1"),
            "PHC string must encode the explicit OWASP baseline params: {hash}"
        );
    }

    /// An empty password must never be hashed.
    #[test]
    fn hash_password_rejects_empty_input() {
        assert!(hash_password("").is_err());
    }

    /// Two differing prompt entries must produce an error and no hash is
    /// ever computed from mismatched input.
    #[test]
    fn confirm_match_rejects_mismatched_entries() {
        assert!(confirm_match("hunter2", "hunter3").is_err());
        assert!(confirm_match("hunter2", "hunter2").is_ok());
    }

    /// The vault key literal `web_ui/auth/password_hash` is the ONE place
    /// this CLI crate and `iron_hermes_ui` agree on a string with no
    /// compiler-enforced link (`iron_hermes_ui` is bin-only and cannot be
    /// imported here). Proven by reading both files' raw source text and
    /// asserting they share the literal — the same source-string-assertion
    /// style as `crates/iron_hermes_ui/tests/kanban_server_fns.rs`, not a
    /// compile-time reference to `PASSWORD_HASH_KEY` on either side.
    #[test]
    fn vault_key_literal_matches_server_auth_rs() {
        const KEY: &str = "web_ui/auth/password_hash";

        let cli_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web_cmd.rs");
        let cli_src = std::fs::read_to_string(&cli_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", cli_path.display()));
        assert!(
            cli_src.contains(KEY),
            "web_cmd.rs must contain the vault key literal {KEY:?}"
        );

        let server_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../iron_hermes_ui/src/server/auth.rs");
        let server_src = std::fs::read_to_string(&server_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", server_path.display()));
        assert!(
            server_src.contains(KEY),
            "iron_hermes_ui/src/server/auth.rs must read the same vault key literal \
             ({KEY:?}) this CLI writes"
        );
    }
}
