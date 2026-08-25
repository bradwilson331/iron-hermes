//! `ironhermes web` CLI subcommand — operator credential generation for the
//! login boundary (Phase 47.3 Plan 03, D-08).
//!
//! Provides two subcommands:
//! - `set-password [--vault]` — prompt twice (masked) for the operator
//!   password, hash it with argon2id at the OWASP baseline parameters, and
//!   either print the resulting PHC string (default) or store it via the
//!   existing `SecretStore` (`--vault`).
//! - `init-password` — non-interactive first-run provisioning (quick task
//!   260820-8h5). Generates a random password and persists ONLY its
//!   argon2id hash, but only when no hash is resolvable from config.yaml,
//!   `IRONHERMES_WEB_PASSWORD_HASH`, or the vault. Prints the plaintext
//!   exactly once, then never again. Invoked by `docker/web-entrypoint.sh`
//!   before the web server starts; never reads stdin, never accepts a
//!   password in argv.
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

    /// Non-interactive first-run credential provisioning (quick task
    /// 260820-8h5).
    ///
    /// Generates a random password and persists ONLY its argon2id hash —
    /// but only when no hash is resolvable from any of the three existing
    /// sources: config.yaml `web_ui.auth.password_hash`,
    /// `IRONHERMES_WEB_PASSWORD_HASH`, or the vault key
    /// `web_ui/auth/password_hash`. Also declines to generate when the `IP`
    /// env var names a non-loopback address, so the fail-closed bind guard
    /// in `iron_hermes_ui` still hard-refuses that case exactly as
    /// documented. Prints the plaintext exactly once to stdout, then never
    /// again — a second invocation with a hash already configured is a
    /// silent no-op. Never reads stdin. Never accepts a password in argv —
    /// there is deliberately no flag surface here.
    InitPassword,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch a `web` subcommand to the appropriate handler. Registered as
/// `Commands::Web { command }` in the dispatch match below.
pub async fn handle_web_command(cmd: WebCommands) -> Result<()> {
    match cmd {
        WebCommands::SetPassword { vault } => cmd_set_password(vault).await,
        WebCommands::InitPassword => cmd_init_password().await.map(|_outcome| ()),
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
// init-password — first-run credential generation (quick task 260820-8h5)
// ─────────────────────────────────────────────────────────────────────────────

/// Alphabet for [`generate_password`] (D7): 57 symbols excluding the
/// visually ambiguous characters `0`/`O` and `1`/`l`/`I` — chosen so a
/// password read off `podman logs` is never misdictated. Digits `2`-`9` (8)
/// + uppercase `A`-`Z` minus `I`,`O` (24) + lowercase `a`-`z` minus `l` (25)
/// = 57 symbols.
///
/// Entropy: [`PASSWORD_GROUP_COUNT`] * [`PASSWORD_GROUP_LEN`] = 16 symbols
/// drawn from this alphabet gives `16 * log2(57) = 93.3` bits, against the
/// 80-bit floor (T-8h5-02). `tests::entropy_floor_is_met` proves this
/// arithmetic against the live constants so shrinking either one below the
/// floor fails the build.
const PASSWORD_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Number of hyphen-separated groups [`generate_password`] emits.
const PASSWORD_GROUP_COUNT: usize = 4;

/// Number of alphabet symbols per group.
const PASSWORD_GROUP_LEN: usize = 4;

/// Generate a random password: [`PASSWORD_GROUP_COUNT`] groups of
/// [`PASSWORD_GROUP_LEN`] symbols from [`PASSWORD_ALPHABET`], hyphen-joined
/// (e.g. `k7mQ-2xVn-8pLd-Rw3f`, the shape published in `docs/CONTAINER.md`).
///
/// Uses `OsRng` — the same CSPRNG source [`hash_password`] already uses for
/// the argon2 salt (via `argon2::password_hash::rand_core`), so this adds no
/// new dependency. Symbols are selected by rejection sampling: any byte at
/// or above the largest multiple of the alphabet length that fits in a byte
/// (`57 * 4 = 228`) is discarded rather than reduced with `byte % 57`, which
/// would otherwise draw the 28 symbols below index `256 % 57` (`= 25`) with
/// slightly higher probability than the rest — a real, if small, modulo
/// bias. Never logs or stores the result; the caller owns disposal.
pub fn generate_password() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};

    const ALPHABET_LEN: usize = PASSWORD_ALPHABET.len();
    let reject_threshold = ((256 / ALPHABET_LEN) * ALPHABET_LEN) as u8;
    let total_symbols = PASSWORD_GROUP_COUNT * PASSWORD_GROUP_LEN;

    let mut symbols: Vec<u8> = Vec::with_capacity(total_symbols);
    let mut buf = [0u8; 64];
    let mut pos = buf.len();

    while symbols.len() < total_symbols {
        if pos == buf.len() {
            OsRng.fill_bytes(&mut buf);
            pos = 0;
        }
        let byte = buf[pos];
        pos += 1;
        if byte >= reject_threshold {
            continue;
        }
        symbols.push(PASSWORD_ALPHABET[(byte as usize) % ALPHABET_LEN]);
    }

    symbols
        .chunks(PASSWORD_GROUP_LEN)
        .map(|chunk| std::str::from_utf8(chunk).expect("alphabet is ASCII"))
        .collect::<Vec<_>>()
        .join("-")
}

/// Outcome of [`splice_web_ui_password_hash`].
#[derive(Debug, PartialEq, Eq)]
enum SpliceOutcome {
    /// The new full file text, with the hash block appended.
    Appended(String),
    /// A top-level `web_ui:` key already exists — nothing was changed.
    DeclinedKeyPresent,
}

/// Append a `web_ui: / auth: / password_hash:` block to `existing`
/// config.yaml text via a targeted raw-text splice — pure, no I/O.
///
/// Follows the raw-text-splice precedent at
/// `crates/ironhermes-cli/src/setup.rs:1292-1354` (the kanban section
/// write). `Config::save_to` (`config.rs:3688-3697`) and
/// `config_setter::config_set` are both rejected here because both
/// round-trip the file through `serde_yaml::Value`, which has no
/// representation for comments and would destroy the seeded file's
/// 1175-line documentation body. This function only ever appends bytes, so
/// `new.starts_with(old)` is the mechanical proof that comments survive.
///
/// Declines (`DeclinedKeyPresent`, `existing` returned untouched by the
/// caller) when any line of `existing`, with trailing whitespace stripped,
/// begins at column zero with `web_ui:` — appending a second top-level key
/// would produce a duplicate that `serde_yaml` rejects, turning a
/// convenience feature into a config-corrupting one. That state cannot
/// arise from the shipped seed (`cli-config.yaml.example` has no `web_ui:`
/// key at all — its only `web` match is the unrelated web-*browsing* block,
/// `config.rs:2895-2900`); it means an operator has hand-authored
/// `web_ui:`, and `ironhermes web set-password` is the correct path for
/// them.
///
/// The PHC string is single-quoted YAML: argon2 PHC strings contain `$`,
/// `+`, `/` and base64 but never an apostrophe, so single-quoting needs no
/// escaping and cannot be misread as a YAML escape sequence (double quotes
/// would require one).
fn splice_web_ui_password_hash(existing: &str, phc: &str) -> SpliceOutcome {
    let key_present = existing
        .lines()
        .any(|line| line.trim_end().starts_with("web_ui:"));
    if key_present {
        return SpliceOutcome::DeclinedKeyPresent;
    }

    let mut new_text = existing.to_string();
    if !new_text.is_empty() && !new_text.ends_with('\n') {
        new_text.push('\n');
    }
    new_text.push('\n');
    new_text.push_str("# Written by `ironhermes web init-password` on first container start.\n");
    new_text.push_str("# Replace with `ironhermes web set-password`.\n");
    new_text.push_str("web_ui:\n");
    new_text.push_str("  auth:\n");
    new_text.push_str(&format!("    password_hash: '{phc}'\n"));

    SpliceOutcome::Appended(new_text)
}

/// Write `content` to `path` atomically: write to a sibling temp file, copy
/// `path`'s existing permissions onto the temp file (if `path` already
/// exists) before the rename so the seeded file's mode survives, then
/// `std::fs::rename` — the same temp-then-rename shape `Config::save_to`
/// uses (`config.rs:3688-3697`).
fn write_config_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(metadata) = std::fs::metadata(path) {
        std::fs::set_permissions(&tmp, metadata.permissions())
            .with_context(|| format!("copying permissions onto {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Outcome of [`cmd_init_password`], returned so tests assert on it
/// directly. All four variants map to a successful process exit (`0`) —
/// deliberately no distinguishing exit codes. `docker/web-entrypoint.sh`
/// does not branch on the result: the image's bind address is a fixed
/// loopback default rather than something computed from whether a hash
/// pre-existed. A non-zero exit is reserved for genuine I/O, config, or
/// vault errors — and even those must not be fatal to the container (the
/// entrypoint wraps the invocation non-fatally; see Task 3).
#[derive(Debug, PartialEq, Eq)]
enum InitPasswordOutcome {
    /// A hash was already resolvable from config.yaml, the env var, or the
    /// vault — nothing generated, nothing printed.
    AlreadyConfigured,
    /// A password was generated, hashed, spliced into config.yaml, and
    /// printed exactly once.
    Generated,
    /// A top-level `web_ui:` key already existed in config.yaml — declined
    /// to splice a duplicate. Nothing printed.
    Declined,
    /// `IP` names a non-loopback address and no hash is configured —
    /// generation skipped so the unmodified bind guard still hard-refuses.
    /// Nothing generated, written, or printed to stdout.
    SkippedNonLoopbackBind,
}

/// True when any of the three credential sources carries a non-blank value.
/// Pure; mirrors the "pure predicate plus thin caller" shape established by
/// `bind_guard_allows` / `provider_key_guard_allows`
/// (`crates/iron_hermes_ui/src/main.rs:24-27`, quick task 260818-t3y).
fn hash_already_configured(env: Option<&str>, cfg: Option<&str>, vault: Option<&str>) -> bool {
    [env, cfg, vault]
        .into_iter()
        .any(|v| v.is_some_and(|s| !s.trim().is_empty()))
}

/// Whether generating a password is safe given the `IP` env var that will
/// determine the server's bind address. Pure. Returns `false` only when
/// `ip_env` parses as a non-loopback [`std::net::IpAddr`]; an unset or
/// unparseable value, and any loopback address, all return `true`.
///
/// Deliberately mirrors `dioxus_cli_config::server_ip`
/// (`dioxus-cli-config-0.7.7/src/lib.rs:141-145`) exactly, including its
/// `.and_then(parse.ok())` fallback to loopback at `:185`, so this command
/// and the server always agree on which address is about to be bound.
///
/// This gate exists because generation runs in a process that completes
/// BEFORE the server starts (D3): without it, minting a hash here would
/// satisfy the server's bind guard and silently publish a brand-new
/// credential before the operator has read it — breaking the hard-refusal
/// contract published in `docs/CONTAINER.md` for `-e IP=0.0.0.0` with no
/// hash configured.
fn generation_allowed_for_bind(ip_env: Option<&str>) -> bool {
    match ip_env.and_then(|s| s.parse::<std::net::IpAddr>().ok()) {
        Some(ip) => ip.is_loopback(),
        None => true,
    }
}

/// Reproduce the first-run banner published at `docs/CONTAINER.md:156-164`
/// byte-for-byte, with `config_path` substituted. Pure — no I/O — so it is
/// directly testable and pinned against the published doc by
/// `tests::first_run_banner_matches_published_docs`.
///
/// Geometry (all leading spaces significant): a rule of exactly 44 `=`
/// characters; one space + the title; a blank line; three spaces + the
/// password; a blank line; one space + the first half of the storage
/// sentence; the config path on its own line (so `Config::config_path()`
/// can be substituted without disturbing the layout) + the second half;
/// three spaces + the remediation command; the same rule. Nothing else is
/// emitted — no trailing advisory line, since `docs/CONTAINER.md`
/// reproduces this block as the container's literal output.
fn first_run_banner(password: &str, config_path: &std::path::Path) -> String {
    let rule = "=".repeat(44);
    format!(
        "{rule}\n FIRST-RUN WEB PASSWORD (shown once)\n\n   {password}\n\n Stored as an argon2id hash in\n {path}. Change it with:\n   ironhermes web set-password\n{rule}",
        path = config_path.display(),
    )
}

/// Read the operator's vault-held password hash, if any, via the same
/// `resolve_vault_config` -> `open_store` -> secret-access path
/// [`store_hash_in_vault`] uses for writing, and the same
/// [`PASSWORD_HASH_KEY`] literal — do not introduce a second literal.
async fn read_vault_password_hash(
    config: &ironhermes_core::config::Config,
) -> Result<Option<String>> {
    use secrecy::ExposeSecret as _;

    let resolved = crate::vault_cmd::resolve_vault_config(config);
    let store = ironhermes_vault::open_store(&resolved)?;
    let secret = store.get_secret(PASSWORD_HASH_KEY).await?;
    Ok(secret.map(|s| s.expose_secret().to_string()))
}

/// First-run provisioning (quick task 260820-8h5). Resolves the three
/// existing credential sources FIRST, in the same order and with the same
/// semantics as `crates/iron_hermes_ui/src/server/auth.rs:101-119` so the
/// two binaries cannot disagree about what "configured" means; only then
/// applies the bind-address generation gate ([`generation_allowed_for_bind`],
/// D5.5); only then generates, hashes, and splices.
///
/// Checking configuration before the bind gate is load-bearing: it makes
/// the documented case "`IRONHERMES_WEB_PASSWORD_HASH` set, `-e
/// IP=0.0.0.0` on the very first run" resolve silently as
/// `AlreadyConfigured` rather than emitting a misleading skip notice.
async fn cmd_init_password() -> Result<InitPasswordOutcome> {
    let config = ironhermes_core::config::Config::load()?;

    // 1. env — crates/ironhermes-cli/src/main.rs:1786-1788 has already
    //    loaded $IRONHERMES_HOME/.env by dispatch time, so a value living in
    //    the container's .env is visible here exactly as it is to the web
    //    binary at its own main.rs:79-82.
    let env_hash = std::env::var("IRONHERMES_WEB_PASSWORD_HASH").ok();

    // 2. config.yaml
    let cfg_hash = config.web_ui.auth.password_hash.clone();

    // 3. vault — only when enabled. A vault error (sealed/corrupt) is
    //    treated as "configured" here: it generates nothing, and the web
    //    binary will surface the canonical sealed-vault failure a moment
    //    later via its own `?` at auth.rs:110 — generating here would write
    //    a config.yaml value that shadows the vault-held credential.
    let vault_hash = if config.vault.enabled {
        match read_vault_password_hash(&config).await {
            Ok(h) => h,
            Err(e) => {
                eprintln!(
                    "ironhermes web init-password: vault error while checking for an \
                     existing password hash ({e}) — generating nothing; the server will \
                     report this error itself at startup."
                );
                return Ok(InitPasswordOutcome::AlreadyConfigured);
            }
        }
    } else {
        None
    };

    if hash_already_configured(
        env_hash.as_deref(),
        cfg_hash.as_deref(),
        vault_hash.as_deref(),
    ) {
        return Ok(InitPasswordOutcome::AlreadyConfigured);
    }

    // Bind gate (D5.5): skip generation entirely when IP names a
    // non-loopback address, so minting a hash here can never be what
    // satisfies the bind guard and silently publishes a brand-new
    // credential.
    let ip_env = std::env::var("IP").ok();
    if !generation_allowed_for_bind(ip_env.as_deref()) {
        eprintln!(
            "ironhermes web init-password: IP={ip} is a non-loopback address and no web \
             password hash is configured — not generating one, since the server is about \
             to refuse this bind. Mint a hash first with `podman run --rm -it --entrypoint \
             ironhermes ironhermes web set-password`.",
            ip = ip_env.as_deref().unwrap_or("<unset>")
        );
        return Ok(InitPasswordOutcome::SkippedNonLoopbackBind);
    }

    let config_path = ironhermes_core::config::Config::config_path();
    let existing = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;

    let password = generate_password();
    let hash = hash_password(&password)?;

    match splice_web_ui_password_hash(&existing, &hash) {
        SpliceOutcome::Appended(new_text) => {
            // Write BEFORE printing: a password that failed to persist must
            // never be shown, since the operator would then believe a hash
            // exists that the server cannot actually resolve.
            write_config_atomic(&config_path, &new_text)?;

            // println!, not tracing: the CLI installs no file-backed
            // subscriber for `web` subcommands (unlike iron_hermes_ui,
            // which fans a tracing:: call out to a rolling web.log), and
            // web_cmd.rs:151-155/:168-174 already use println! for
            // operator command output. Deliberately NOT wrapped in
            // secrecy::SecretString: it is printed by design, so in-memory
            // hygiene is not the threat model here, and wrapping it would
            // only obscure that.
            println!("{}", first_run_banner(&password, &config_path));
            use std::io::Write as _;
            std::io::stdout().flush().ok();

            Ok(InitPasswordOutcome::Generated)
        }
        SpliceOutcome::DeclinedKeyPresent => {
            eprintln!(
                "ironhermes web init-password: web_ui: already present in {} — leaving it \
                 untouched. Use `ironhermes web set-password` to change it.",
                config_path.display()
            );
            Ok(InitPasswordOutcome::Declined)
        }
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

    // ─────────────────────────────────────────────────────────────────────
    // init-password (quick task 260820-8h5, Task 1)
    // ─────────────────────────────────────────────────────────────────────

    /// Read the repo-root example config text, matching the sibling-path
    /// idiom already used by `vault_key_literal_matches_server_auth_rs`
    /// above (`../iron_hermes_ui/...`).
    fn example_config_text() -> String {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli-config.yaml.example");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
    }

    #[test]
    fn generate_password_has_the_published_shape() {
        let pw = generate_password();
        let groups: Vec<&str> = pw.split('-').collect();
        assert_eq!(
            groups.len(),
            PASSWORD_GROUP_COUNT,
            "expected {PASSWORD_GROUP_COUNT} hyphen-separated groups, got {pw:?}"
        );
        for group in &groups {
            assert_eq!(
                group.len(),
                PASSWORD_GROUP_LEN,
                "group {group:?} wrong length"
            );
        }
        assert_eq!(pw.len(), 19, "19 chars total, matching k7mQ-2xVn-8pLd-Rw3f");
    }

    #[test]
    fn generate_password_alphabet_excludes_ambiguous_chars() {
        for _ in 0..200 {
            let pw = generate_password();
            for ch in pw.chars().filter(|c| *c != '-') {
                assert!(
                    !matches!(ch, '0' | 'O' | '1' | 'l' | 'I'),
                    "ambiguous character {ch:?} must never appear in a generated password"
                );
                assert!(
                    PASSWORD_ALPHABET.contains(&(ch as u8)),
                    "character {ch:?} is not in PASSWORD_ALPHABET"
                );
            }
        }
    }

    /// CSPRNG smoke test, not a distribution test: 1000 successive calls
    /// must not collide.
    #[test]
    fn generate_password_produces_distinct_values() {
        use std::collections::HashSet;
        let values: HashSet<String> = (0..1000).map(|_| generate_password()).collect();
        assert_eq!(
            values.len(),
            1000,
            "1000 calls must yield 1000 distinct values"
        );
    }

    /// Static invariant: shrinking either the alphabet or the group count
    /// below the 80-bit entropy floor must fail the build.
    #[test]
    fn entropy_floor_is_met() {
        let bits = (PASSWORD_GROUP_COUNT * PASSWORD_GROUP_LEN) as f64
            * (PASSWORD_ALPHABET.len() as f64).log2();
        assert!(
            bits >= 80.0,
            "entropy {bits} bits is below the 80-bit floor"
        );
    }

    #[test]
    fn splice_appends_and_preserves_the_original_as_a_byte_prefix() {
        let existing = example_config_text();
        let hash = hash_password("hunter2").unwrap();

        let spliced = match splice_web_ui_password_hash(&existing, &hash) {
            SpliceOutcome::Appended(text) => text,
            SpliceOutcome::DeclinedKeyPresent => {
                panic!("the shipped seed must not carry a web_ui: key")
            }
        };

        assert!(
            spliced.starts_with(&existing),
            "spliced text must have the original as a byte prefix"
        );

        let config: ironhermes_core::config::Config =
            serde_yaml::from_str(&spliced).expect("spliced text must deserialize as Config");
        assert_eq!(
            config.web_ui.auth.password_hash.as_deref(),
            Some(hash.as_str())
        );
    }

    #[test]
    fn splice_declines_when_web_ui_key_already_present() {
        let existing = "web_ui:\n  auth:\n    password_hash: 'existing-hash'\n";
        let outcome = splice_web_ui_password_hash(existing, "new-hash");
        assert_eq!(outcome, SpliceOutcome::DeclinedKeyPresent);
    }

    /// Full round trip: generate -> hash_password -> splice into the real
    /// 1175-line example config -> serde_yaml::from_str::<Config> -> pull
    /// the hash back out -> PasswordHash::new -> Argon2::default()
    /// .verify_password — the identical sequence AuthState::verify_password
    /// runs (auth.rs:179-189), proving the generated plaintext actually
    /// authenticates and a different plaintext does not.
    #[test]
    fn full_round_trip_generate_hash_splice_verify() {
        let existing = example_config_text();
        let password = generate_password();
        let hash = hash_password(&password).unwrap();

        let spliced = match splice_web_ui_password_hash(&existing, &hash) {
            SpliceOutcome::Appended(text) => text,
            SpliceOutcome::DeclinedKeyPresent => {
                panic!("the shipped seed must not carry a web_ui: key")
            }
        };

        let config: ironhermes_core::config::Config =
            serde_yaml::from_str(&spliced).expect("spliced text must deserialize as Config");
        let parsed_hash = config
            .web_ui
            .auth
            .password_hash
            .expect("password_hash must round-trip through YAML");
        assert_eq!(parsed_hash, hash);

        use argon2::password_hash::PasswordHash;
        use argon2::{Argon2, PasswordVerifier};
        let parsed = PasswordHash::new(&parsed_hash).expect("must parse as a valid PHC string");
        assert!(
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok(),
            "the generated plaintext must authenticate against the real verifier"
        );
        assert!(
            Argon2::default()
                .verify_password(b"a-completely-different-password", &parsed)
                .is_err(),
            "a different plaintext must NOT authenticate"
        );

        assert!(
            !spliced.contains(&password),
            "the generated plaintext must never appear in the bytes written to config.yaml"
        );
    }

    /// Cross-crate pin (D8): `iron_hermes_ui` is bin-only and cannot be
    /// imported here, so this proves the identical proxy condition —
    /// `AuthState::verify_password`'s call fragments still exist in the
    /// server's source text, in the same style as
    /// `vault_key_literal_matches_server_auth_rs` above.
    #[test]
    fn verify_password_proxy_fragments_still_present_in_server_auth_rs() {
        let server_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../iron_hermes_ui/src/server/auth.rs");
        let server_src = std::fs::read_to_string(&server_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", server_path.display()));
        assert!(
            server_src.contains("PasswordHash::new(hash_str)"),
            "auth.rs must still call PasswordHash::new(hash_str)"
        );
        assert!(
            server_src.contains("Argon2::default()")
                && server_src.contains(".verify_password(candidate.as_bytes(), &parsed)"),
            "auth.rs must still call Argon2::default().verify_password(candidate.as_bytes(), &parsed)"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // init-password (quick task 260820-8h5, Task 2)
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn hash_already_configured_truth_table() {
        assert!(!hash_already_configured(None, None, None));
        assert!(hash_already_configured(Some("abc"), None, None));
        assert!(hash_already_configured(None, Some("abc"), None));
        assert!(hash_already_configured(None, None, Some("abc")));
        assert!(
            !hash_already_configured(Some(""), None, None),
            "an empty string must not count as configured"
        );
        assert!(
            !hash_already_configured(Some("   "), None, None),
            "a blank (whitespace-only) string must not count as configured"
        );
        assert!(!hash_already_configured(None, Some(""), Some("   ")));
        assert!(hash_already_configured(Some("  x  "), None, None));
    }

    #[test]
    fn generation_allowed_for_bind_matrix() {
        assert!(
            generation_allowed_for_bind(None),
            "unset IP falls back to loopback"
        );
        assert!(
            generation_allowed_for_bind(Some("not-an-ip-address")),
            "unparseable IP falls back to loopback, mirroring dioxus_cli_config::server_ip"
        );
        assert!(generation_allowed_for_bind(Some("127.0.0.1")));
        assert!(generation_allowed_for_bind(Some("::1")));
        assert!(
            !generation_allowed_for_bind(Some("0.0.0.0")),
            "wildcard bind is not loopback"
        );
        assert!(
            !generation_allowed_for_bind(Some("127.0.0.1")),
            "explicit non-loopback bind must skip generation"
        );
    }

    #[test]
    fn splice_decline_leaves_input_unmodified_and_plaintext_absent() {
        let existing = "web_ui:\n  auth:\n    password_hash: 'existing-hash'\nextra: true\n";
        let password = generate_password();
        let hash = hash_password(&password).unwrap();
        let outcome = splice_web_ui_password_hash(existing, &hash);
        assert_eq!(outcome, SpliceOutcome::DeclinedKeyPresent);
        // The caller never applies a DeclinedKeyPresent outcome, so the
        // input text itself — not some transformed copy — is what remains
        // on disk. Confirm it is untouched and the plaintext is absent.
        assert_eq!(
            existing,
            "web_ui:\n  auth:\n    password_hash: 'existing-hash'\nextra: true\n"
        );
        assert!(!existing.contains(&password));
    }

    /// Docs-pin (T-8h5-06): every structural line `first_run_banner`
    /// produces for the doc's own sample password and path must appear
    /// verbatim in the committed `docs/CONTAINER.md`, so code and the
    /// already-published documentation cannot silently diverge.
    #[test]
    fn first_run_banner_matches_published_docs() {
        let docs_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/CONTAINER.md");
        let docs_src = std::fs::read_to_string(&docs_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", docs_path.display()));

        let banner = first_run_banner(
            "k7mQ-2xVn-8pLd-Rw3f",
            std::path::Path::new("/opt/data/config.yaml"),
        );
        for line in banner.lines() {
            assert!(
                docs_src.contains(line),
                "banner line {line:?} not found verbatim in docs/CONTAINER.md"
            );
        }
    }

    #[test]
    fn first_run_banner_geometry() {
        let banner = first_run_banner(
            "k7mQ-2xVn-8pLd-Rw3f",
            std::path::Path::new("/opt/data/config.yaml"),
        );
        let lines: Vec<&str> = banner.lines().collect();
        assert_eq!(lines.len(), 9);
        let rule = "=".repeat(44);
        assert_eq!(lines[0], rule);
        assert_eq!(lines[8], rule);
        assert_eq!(lines[1], " FIRST-RUN WEB PASSWORD (shown once)");
        assert_eq!(lines[2], "");
        assert_eq!(lines[3], "   k7mQ-2xVn-8pLd-Rw3f");
        assert_eq!(lines[4], "");
        assert_eq!(lines[5], " Stored as an argon2id hash in");
        assert_eq!(lines[6], " /opt/data/config.yaml. Change it with:");
        assert_eq!(lines[7], "   ironhermes web set-password");
    }
}
