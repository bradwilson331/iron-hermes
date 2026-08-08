//! `ironhermes buzz` CLI subcommand — per-profile Nostr identity
//! provisioning (Phase 47.6 Plan 04, D-06/D-07).
//!
//! Provides three subcommands:
//! - `keygen [--force]` — generate a fresh Nostr keypair with
//!   `nostr_sdk::Keys::generate`, write the secret into the active profile's
//!   `.env` under `BUZZ_NSEC` (imported from `ironhermes_gateway::buzz_identity`
//!   so the read and write sides can never drift), and print the resulting
//!   npub together with the exact `.env` path written.
//! - `import [--force]` — read an externally generated secret through a
//!   masked prompt, accept both the bech32 `nsec1…` and 64-character hex
//!   forms, validate it parses as a keypair BEFORE writing anything, then
//!   take the same write path as keygen.
//! - `pubkey` — resolve the active profile's identity through
//!   `resolve_buzz_nsec` (the plan 01 seam) and print the npub and its hex
//!   form.
//!
//! # No secret in argv, stdout, logs, or errors (D-06)
//!
//! Neither `keygen` nor `import` accepts a positional argument or a
//! `--nsec` flag — argv is visible in the process table to every user on the
//! machine. `import`'s only input channel is [`read_secret_value`] (masked
//! TTY prompt, with a documented non-TTY plain-stdin fallback), copied
//! verbatim from `web_cmd.rs`/`vault_cmd.rs` — do not invent a second prompt
//! idiom. No handler in this file ever prints, logs, or interpolates the
//! secret value into an error; every report-building helper's signature is
//! structurally secret-blind (it never receives the nsec/hex value as an
//! argument at all).
//!
//! # Single write seam, single quoting implementation (D-06)
//!
//! [`write_nsec_to_profile_env`] is the ONE write path both `keygen` and
//! `import` use. It never hand-renders a line and never appends by string
//! concatenation — it hands the entry to
//! `ironhermes_core::dotenv_write::upsert_env_entries` (Phase 47.6 Plan 04
//! Task 1), whose single-quoting and round-trip self-check are what keep a
//! `${VAR}` inside a generated secret from being expanded out of the
//! operator's process environment on the next read, and what keep a
//! trailing backslash from swallowing the following provider key.

use anyhow::{Context as _, Result};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_gateway::buzz_identity::{BUZZ_NSEC_ENV, BuzzIdentityError, resolve_buzz_nsec_with};
use nostr_sdk::prelude::*;
use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────────
// BuzzCommands — clap subcommand tree
// ─────────────────────────────────────────────────────────────────────────────

/// Buzz (Nostr) identity subcommands (Phase 47.6 Plan 04, D-06/D-07).
#[derive(Subcommand)]
pub enum BuzzCommands {
    /// Generate a fresh Nostr keypair for the active profile and write the
    /// secret into its `.env` under `BUZZ_NSEC`. Prints the resulting npub —
    /// put THIS in the OTHER profile's whitelist for the D-04 two-profile
    /// UAT.
    Keygen {
        /// Replace an already-configured key. Without this flag, keygen
        /// refuses when a key is already present — replacing a live agent
        /// identity orphans every message it previously signed and drops it
        /// out of every peer's whitelist.
        #[arg(long)]
        force: bool,
    },
    /// Import an externally generated Nostr secret key through a masked
    /// prompt (bech32 `nsec1…` or 64-character hex). There is deliberately
    /// no positional argument and no `--nsec` flag — argv is world-readable
    /// via the process table.
    Import {
        /// Replace an already-configured key.
        #[arg(long)]
        force: bool,
    },
    /// Print the active profile's Buzz identity (npub + hex form).
    Pubkey,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch a `buzz` subcommand to the appropriate handler. Registered as
/// `Commands::Buzz { command }` in the dispatch match in `main.rs`.
pub async fn handle_buzz_command(cmd: BuzzCommands) -> Result<()> {
    match cmd {
        BuzzCommands::Keygen { force } => cmd_keygen(force),
        BuzzCommands::Import { force } => cmd_import(force),
        BuzzCommands::Pubkey => cmd_pubkey(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

/// Errors from this module's pure helpers. `Display` never embeds a secret
/// value, a raw `dotenvy::Error`, or the caller-supplied input — only a
/// key name or path, neither of which is sensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuzzCmdError {
    /// `BUZZ_NSEC` is already present in the profile `.env` and `--force`
    /// was not passed (T-47.6-04-06) — replacing a live agent identity
    /// orphans every message it previously signed.
    AlreadyConfigured { path: PathBuf },
    /// The shared writer's own round-trip self-check refused the write.
    WriteVerificationFailed,
    /// The supplied secret failed to parse as a Nostr keypair (bech32
    /// `nsec1…` or 64-character hex). Never embeds the supplied input.
    InvalidSecret,
    /// An I/O error reading or writing the profile `.env`. Only the path is
    /// named, never file content.
    Io { path: PathBuf },
    /// Wraps `resolve_buzz_nsec`'s own actionable message (it already names
    /// the environment variable and the profile `.env` path; never a
    /// secret).
    NotConfigured { message: String },
}

impl std::fmt::Display for BuzzCmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuzzCmdError::AlreadyConfigured { path } => write!(
                f,
                "{BUZZ_NSEC_ENV} is already configured in {} — pass --force to replace it",
                path.display()
            ),
            BuzzCmdError::WriteVerificationFailed => write!(
                f,
                "refusing to write: the rendered .env did not round-trip cleanly"
            ),
            BuzzCmdError::InvalidSecret => write!(
                f,
                "the supplied value is not a valid Nostr secret key (expected nsec1... or 64-character hex)"
            ),
            BuzzCmdError::Io { path } => write!(f, "failed to read/write {}", path.display()),
            BuzzCmdError::NotConfigured { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for BuzzCmdError {}

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read a secret value: masked TTY prompt when interactive, otherwise a
/// single line from stdin (piped/non-interactive — CI, scripted usage, or
/// this crate's own integration tests). Never accepted as a bare argv token
/// either way. Copied verbatim from `web_cmd.rs`/`vault_cmd.rs` — do not
/// invent a second prompt idiom.
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

/// True when `.env` body already declares `key` — a real dotenvy-parseable
/// check, not a naive substring search (a value merely CONTAINING the
/// literal `KEY=` text must not false-positive).
fn env_already_has_key(existing_body: &str, key: &str) -> bool {
    dotenvy::from_read_iter(std::io::Cursor::new(existing_body.as_bytes()))
        .filter_map(Result::ok)
        .any(|(k, _)| k == key)
}

/// Atomic 0600 write: create the file at mode 0600 BEFORE any byte is
/// written (never a plain write-then-chmod, which leaves a world-readable
/// window), flush, fsync, then rename into place. Mirrors this project's
/// existing idiom for secret-bearing files (`AuthStore::save_to_disk`,
/// `iron_hermes_ui::server::profile_api::write_env_atomic_0600`) — this
/// crate cannot import that fn directly (`iron_hermes_ui` is a bin-only
/// crate), so the idiom is reproduced here rather than referencing it.
fn write_env_atomic_0600(final_path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    let file_name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp_path = final_path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));

    {
        #[cfg(unix)]
        let mut f = {
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?
        };
        #[cfg(not(unix))]
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;

        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }

    std::fs::rename(&tmp_path, final_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(final_path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(final_path, perms)?;
    }

    Ok(())
}

/// Parse a supplied secret (bech32 `nsec1…` or 64-character hex) into
/// `Keys`. Never embeds the supplied input in the returned error.
fn parse_secret_keys(secret: &str) -> Result<Keys, BuzzCmdError> {
    Keys::parse(secret).map_err(|_| BuzzCmdError::InvalidSecret)
}

/// Write `nsec` into `home`'s `.env` under `BUZZ_NSEC_ENV`, preserving every
/// other entry (T-47.6-04-05) and refusing without `force` when the key is
/// already present (T-47.6-04-06). Returns the `.env` path written.
///
/// This is the ONE write path both `keygen` and `import` use — see this
/// module's doc comment for why it delegates to
/// `ironhermes_core::dotenv_write::upsert_env_entries` rather than
/// hand-rendering a line.
pub(crate) fn write_nsec_to_profile_env(
    home: &Path,
    nsec: &str,
    force: bool,
) -> Result<PathBuf, BuzzCmdError> {
    let env_path = home.join(".env");
    let existing_body = if env_path.exists() {
        std::fs::read_to_string(&env_path).map_err(|_| BuzzCmdError::Io {
            path: env_path.clone(),
        })?
    } else {
        String::new()
    };

    if !force && env_already_has_key(&existing_body, BUZZ_NSEC_ENV) {
        return Err(BuzzCmdError::AlreadyConfigured { path: env_path });
    }

    let entries = vec![(BUZZ_NSEC_ENV.to_string(), nsec.to_string())];
    let rendered = ironhermes_core::dotenv_write::upsert_env_entries(&existing_body, &entries)
        .map_err(|_| BuzzCmdError::WriteVerificationFailed)?;

    write_env_atomic_0600(&env_path, &rendered).map_err(|_| BuzzCmdError::Io {
        path: env_path.clone(),
    })?;

    Ok(env_path)
}

/// Pure keygen orchestration: generate a fresh keypair and write it into
/// `home`'s `.env`. Returns only the npub and the `.env` path written — the
/// secret itself never leaves this function, so nothing downstream (report
/// building, printing) can accidentally embed it.
pub(crate) fn keygen_into(home: &Path, force: bool) -> Result<(String, PathBuf), BuzzCmdError> {
    let keys = Keys::generate();
    let nsec = keys
        .secret_key()
        .to_bech32()
        .map_err(|_| BuzzCmdError::InvalidSecret)?;
    let npub = keys
        .public_key()
        .to_bech32()
        .map_err(|_| BuzzCmdError::InvalidSecret)?;
    let env_path = write_nsec_to_profile_env(home, &nsec, force)?;
    Ok((npub, env_path))
}

/// Pure import orchestration: parse the supplied secret BEFORE writing
/// anything, then take the same write path [`keygen_into`] uses. Returns
/// only the npub and the `.env` path written.
pub(crate) fn import_into(
    home: &Path,
    secret: &str,
    force: bool,
) -> Result<(String, PathBuf), BuzzCmdError> {
    let keys = parse_secret_keys(secret)?;
    let nsec = keys
        .secret_key()
        .to_bech32()
        .map_err(|_| BuzzCmdError::InvalidSecret)?;
    let npub = keys
        .public_key()
        .to_bech32()
        .map_err(|_| BuzzCmdError::InvalidSecret)?;
    let env_path = write_nsec_to_profile_env(home, &nsec, force)?;
    Ok((npub, env_path))
}

/// Pure pubkey orchestration: resolve the active identity through the
/// injectable seam `resolve_buzz_nsec_with` — the SAME resolution function
/// `resolve_buzz_nsec()` itself is a two-line wrapper around, so this never
/// forks the D-06 resolution order — and derive the npub/hex pair. Takes
/// `home` and the environment lookup as parameters so tests can exercise
/// every branch against a `tempfile`-backed home, never the operator's real
/// one.
pub(crate) fn resolve_pubkey_report(
    home: &Path,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> Result<(String, String), BuzzCmdError> {
    let env_path = home.join(".env");
    let secret =
        resolve_buzz_nsec_with(env_lookup, &env_path).map_err(|e: BuzzIdentityError| {
            BuzzCmdError::NotConfigured {
                message: e.to_string(),
            }
        })?;
    let keys = parse_secret_keys(&secret)?;
    let npub = keys
        .public_key()
        .to_bech32()
        .map_err(|_| BuzzCmdError::InvalidSecret)?;
    let hex = keys.public_key().to_hex();
    Ok((npub, hex))
}

// ─────────────────────────────────────────────────────────────────────────────
// Report builders — every signature below is structurally secret-blind: none
// of them accepts the nsec or its hex form as an argument, so no report can
// embed it (T-47.6-04-04).
// ─────────────────────────────────────────────────────────────────────────────

fn keygen_report(npub: &str, env_path: &Path) -> String {
    format!(
        "{} generated a new Buzz identity.\n  npub: {npub}\n  written to: {}\n\n\
         Put this npub in the OTHER profile's whitelist so the two-profile UAT can see each other.",
        "✓".green().bold(),
        env_path.display(),
    )
}

fn import_report(npub: &str, env_path: &Path) -> String {
    format!(
        "{} imported the Buzz identity.\n  npub: {npub}\n  written to: {}",
        "✓".green().bold(),
        env_path.display(),
    )
}

fn pubkey_report(npub: &str, hex: &str) -> String {
    format!("npub: {npub}\nhex:  {hex}")
}

// ─────────────────────────────────────────────────────────────────────────────
// Command handlers
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_keygen(force: bool) -> Result<()> {
    let home = ironhermes_core::get_hermes_home();
    let (npub, env_path) = keygen_into(&home, force).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{}", keygen_report(&npub, &env_path));
    Ok(())
}

fn cmd_import(force: bool) -> Result<()> {
    let secret = read_secret_value("Nostr secret key (nsec1... or hex): ")?;
    let home = ironhermes_core::get_hermes_home();
    let (npub, env_path) =
        import_into(&home, &secret, force).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{}", import_report(&npub, &env_path));
    Ok(())
}

fn cmd_pubkey() -> Result<()> {
    let home = ironhermes_core::get_hermes_home();
    let (npub, hex) = resolve_pubkey_report(&home, |name| std::env::var(name).ok())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{}", pubkey_report(&npub, &hex));
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests — unit-level over the pure helpers, with a tempfile-backed home
// directory. Never the operator's real home.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_name: &str) -> Option<String> {
        None
    }

    #[test]
    fn keygen_writes_nsec_and_returns_npub() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (npub, env_path) = keygen_into(dir.path(), false).expect("keygen should succeed");

        assert_eq!(env_path, dir.path().join(".env"));
        let body = std::fs::read_to_string(&env_path).expect("read .env");
        let parsed: std::collections::HashMap<String, String> =
            dotenvy::from_read_iter(std::io::Cursor::new(body.as_bytes()))
                .collect::<Result<Vec<_>, _>>()
                .expect("written .env must parse cleanly")
                .into_iter()
                .collect();
        let written_nsec = parsed
            .get(BUZZ_NSEC_ENV)
            .expect("BUZZ_NSEC must be present");
        let keys = Keys::parse(written_nsec).expect("written nsec must parse as a keypair");
        assert_eq!(
            keys.public_key().to_bech32().unwrap(),
            npub,
            "returned npub must match the written keypair's public key"
        );
    }

    #[test]
    fn keygen_preserves_existing_env_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        std::fs::write(&env_path, "ANTHROPIC_API_KEY='sk-provider-key'\n").expect("seed .env");

        let (_npub, _path) = keygen_into(dir.path(), false).expect("keygen should succeed");

        let body = std::fs::read_to_string(&env_path).expect("read .env");
        let parsed: std::collections::HashMap<String, String> =
            dotenvy::from_read_iter(std::io::Cursor::new(body.as_bytes()))
                .collect::<Result<Vec<_>, _>>()
                .expect("written .env must parse cleanly")
                .into_iter()
                .collect();
        assert_eq!(
            parsed.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-provider-key"),
            "provider key must survive byte-identical"
        );
        assert!(parsed.contains_key(BUZZ_NSEC_ENV));
    }

    #[test]
    fn keygen_refuses_to_overwrite_without_force() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");

        let (_npub, _path) = keygen_into(dir.path(), false).expect("first keygen should succeed");
        let body_before = std::fs::read_to_string(&env_path).expect("read .env");

        let err = keygen_into(dir.path(), false).unwrap_err();
        match err {
            BuzzCmdError::AlreadyConfigured { path } => assert_eq!(path, env_path),
            other => panic!("expected AlreadyConfigured, got {other:?}"),
        }

        let body_after = std::fs::read_to_string(&env_path).expect("read .env");
        assert_eq!(
            body_before, body_after,
            "file must not be modified when refusing to overwrite"
        );
    }

    #[test]
    fn keygen_with_force_replaces_the_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");

        let (first_npub, _path) =
            keygen_into(dir.path(), false).expect("first keygen should succeed");
        let (second_npub, _path) =
            keygen_into(dir.path(), true).expect("forced keygen should succeed");

        assert_ne!(
            first_npub, second_npub,
            "a forced keygen must generate a genuinely new keypair"
        );

        let body = std::fs::read_to_string(&env_path).expect("read .env");
        let occurrences = body
            .lines()
            .filter(|l| l.starts_with(&format!("{BUZZ_NSEC_ENV}=")))
            .count();
        assert_eq!(occurrences, 1, "force must replace, not duplicate, the key");
    }

    #[test]
    fn written_nsec_value_is_single_quoted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (_npub, env_path) = keygen_into(dir.path(), false).expect("keygen should succeed");

        let body = std::fs::read_to_string(&env_path).expect("read .env");
        let line = body
            .lines()
            .find(|l| l.starts_with(&format!("{BUZZ_NSEC_ENV}=")))
            .expect("BUZZ_NSEC line must be present");
        let value_part = line
            .strip_prefix(&format!("{BUZZ_NSEC_ENV}="))
            .expect("prefix must match");
        assert!(value_part.starts_with('\''), "value must start with '");
        assert!(value_part.ends_with('\''), "value must end with '");
    }

    #[test]
    fn written_nsec_round_trips_through_dotenvy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (npub, env_path) = keygen_into(dir.path(), false).expect("keygen should succeed");

        let parsed: std::collections::HashMap<String, String> =
            dotenvy::from_path_iter(&env_path)
                .expect("dotenvy must open the written .env")
                .collect::<Result<Vec<_>, _>>()
                .expect("dotenvy must parse the written .env cleanly")
                .into_iter()
                .collect();
        let nsec = parsed.get(BUZZ_NSEC_ENV).expect("BUZZ_NSEC must resolve");
        let keys = Keys::parse(nsec).expect("resolved value must parse as a keypair");
        assert_eq!(keys.public_key().to_bech32().unwrap(), npub);
    }

    #[test]
    fn import_accepts_bech32_and_hex() {
        let dir_bech32 = tempfile::tempdir().expect("tempdir");
        let dir_hex = tempfile::tempdir().expect("tempdir");

        let generated = Keys::generate();
        let nsec_bech32 = generated.secret_key().to_bech32().unwrap();
        let nsec_hex = generated.secret_key().to_secret_hex();

        let (npub_from_bech32, _path) =
            import_into(dir_bech32.path(), &nsec_bech32, false).expect("bech32 import");
        let (npub_from_hex, _path) =
            import_into(dir_hex.path(), &nsec_hex, false).expect("hex import");

        assert_eq!(npub_from_bech32, npub_from_hex);
        assert_eq!(npub_from_bech32, generated.public_key().to_bech32().unwrap());
    }

    #[test]
    fn import_rejects_malformed_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let malformed = "not-a-real-nsec-or-hex-value";
        let err = import_into(dir.path(), malformed, false).unwrap_err();
        assert_eq!(err, BuzzCmdError::InvalidSecret);
        let rendered = err.to_string();
        assert!(!rendered.contains(malformed));
    }

    #[test]
    fn pubkey_reports_the_configured_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_path = dir.path().join(".env");
        let keys = Keys::generate();
        let nsec = keys.secret_key().to_bech32().unwrap();
        std::fs::write(&env_path, format!("{BUZZ_NSEC_ENV}='{nsec}'\n")).expect("seed .env");

        let (npub, hex) = resolve_pubkey_report(dir.path(), no_env).expect("should resolve");
        assert_eq!(npub, keys.public_key().to_bech32().unwrap());
        assert_eq!(hex, keys.public_key().to_hex());
    }

    #[test]
    fn pubkey_reports_not_configured_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = resolve_pubkey_report(dir.path(), no_env).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains(BUZZ_NSEC_ENV));
        assert!(rendered.contains(&dir.path().join(".env").display().to_string()));
    }

    #[test]
    fn no_command_output_contains_the_secret() {
        // keygen
        let dir = tempfile::tempdir().expect("tempdir");
        let (npub, env_path) = keygen_into(dir.path(), false).expect("keygen should succeed");
        let body = std::fs::read_to_string(&env_path).expect("read .env");
        let parsed: std::collections::HashMap<String, String> =
            dotenvy::from_read_iter(std::io::Cursor::new(body.as_bytes()))
                .collect::<Result<Vec<_>, _>>()
                .expect("parse")
                .into_iter()
                .collect();
        let nsec = parsed.get(BUZZ_NSEC_ENV).unwrap().clone();
        let keys = Keys::parse(&nsec).unwrap();
        let hex = keys.secret_key().to_secret_hex();

        let report = keygen_report(&npub, &env_path);
        assert!(!report.contains(&nsec));
        assert!(!report.contains(&hex));

        // import
        let dir2 = tempfile::tempdir().expect("tempdir");
        let generated = Keys::generate();
        let import_nsec = generated.secret_key().to_bech32().unwrap();
        let import_hex = generated.secret_key().to_secret_hex();
        let (import_npub, import_env_path) =
            import_into(dir2.path(), &import_nsec, false).expect("import should succeed");
        let import_report_text = import_report(&import_npub, &import_env_path);
        assert!(!import_report_text.contains(&import_nsec));
        assert!(!import_report_text.contains(&import_hex));

        // pubkey
        let dir3 = tempfile::tempdir().expect("tempdir");
        let env_path3 = dir3.path().join(".env");
        let generated3 = Keys::generate();
        let nsec3 = generated3.secret_key().to_bech32().unwrap();
        let hex3 = generated3.secret_key().to_secret_hex();
        std::fs::write(&env_path3, format!("{BUZZ_NSEC_ENV}='{nsec3}'\n")).expect("seed .env");
        let (pk_npub, pk_hex) =
            resolve_pubkey_report(dir3.path(), no_env).expect("should resolve");
        let pubkey_report_text = pubkey_report(&pk_npub, &pk_hex);
        assert!(!pubkey_report_text.contains(&nsec3));
        assert!(!pubkey_report_text.contains(&hex3));
    }
}
