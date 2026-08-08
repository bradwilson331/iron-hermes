//! `ironhermes vault` CLI subcommand — operator secret-store lifecycle (Phase 46.8, D-03).
//!
//! Provides five subcommands:
//! - `init` — Create a fresh encrypted vault (rusty-vault backend only).
//! - `unlock` — Unseal an initialized vault (no-op status confirmation in
//!   `unseal_mode = "keyfile"`; prompts for the unseal key in
//!   `unseal_mode = "passphrase"`).
//! - `set` — Store (create/overwrite) a secret value under a key name.
//! - `list` — List secret key NAMES only, optionally prefix-filtered.
//! - `migrate` — Import provider API keys from `~/.ironhermes/.env` into the vault
//!   (Plan 06, D-04): backup → vault-write → scrub-only-written, see [`cmd_migrate`].
//!
//! Mirrors `auth_cmd.rs`'s structural shape (clap `Subcommand` enum + async
//! dispatcher + store-open-before-anything handlers), extended with:
//!
//! # Masked value entry (D-03)
//!
//! `vault set`/`vault unlock` (passphrase mode) NEVER accept the secret as a bare
//! argv token — [`read_secret_value`] always prompts. On a real TTY it uses
//! `rpassword::prompt_password` (masked, no echo). When stdin is not a TTY (e.g.
//! piped input, this crate's own integration tests, or scripted/CI usage) it falls
//! back to a plain line read from stdin — never argv, and consistent with common
//! CLI convention (masking a redirected pipe is meaningless anyway).
//!
//! # Names only, never values (D-15)
//!
//! `vault list` prints key NAMES only, sorted. No handler in this file ever calls
//! `.expose_secret()` — the one place a raw value is materialized is
//! `secrecy::SecretString::from(value)` immediately after [`read_secret_value`]
//! returns, and that `SecretString` crosses straight into `put_secret` without
//! ever being logged, printed, or audited.
//!
//! # Audit trail (D-06)
//!
//! Every handler appends exactly one [`ironhermes_core::audit::AuditLog`] entry
//! via [`record_audit`] — `kind` is one of `vault_init`/`vault_unlock`/`vault_set`/
//! `vault_list`, `tool` is the key NAME (or `"-"` for init/unlock/list-all), and
//! `args` carries `{"success": <bool>}` only, never a value. This runs even when
//! the underlying op fails (an empty/no-op list, a sealed vault, etc.) — no vault
//! op silently skips the audit trail.
//!
//! # Operator-CLI only (D-14)
//!
//! This module is the ONLY entry point into the vault subsystem. No
//! `SecretStore`/vault reference exists anywhere in a `ToolRegistry`/MCP source
//! file — there is no chat/agent path to a secret value.
//!
//! # Data-dir sentinel resolution
//!
//! `ironhermes-vault` is a zero-cycle leaf crate (D-01) and cannot call
//! `get_hermes_home()` itself, so `RustyVaultConfig::data_dir` defaults to an
//! empty `PathBuf` sentinel (see `ironhermes_vault::config` module doc). Every
//! handler here routes through [`resolve_vault_config`] to fill that sentinel
//! with `get_hermes_home().join("vault")` BEFORE calling `RustyVaultStore::init`/
//! `open`/`ironhermes_vault::open_store` — so `vault init`'s data dir and
//! `vault set`/`list`'s `open_store` call always agree on the same on-disk
//! location.

use anyhow::{Context as _, Result};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_core::config::Config;
use ironhermes_core::get_hermes_home;

// ─────────────────────────────────────────────────────────────────────────────
// VaultCommands — clap subcommand tree
// ─────────────────────────────────────────────────────────────────────────────

/// Vault subcommands for managing the operator secret store (D-03).
#[derive(Subcommand)]
pub enum VaultCommands {
    /// Initialize a new encrypted vault (rusty-vault backend only).
    ///
    /// Writes a fresh, 1-of-1 Shamir-sealed store to the configured data
    /// directory (`get_hermes_home().join("vault")` when unset) and a `0600`
    /// keyfile holding the unseal key + root token beside it. Key material is
    /// NEVER printed (D-15).
    Init,
    /// Unseal an initialized vault so subsequent `set`/`list` calls succeed.
    ///
    /// `unseal_mode = "keyfile"` (default) auto-unseals on every open — this
    /// command is then a status confirmation. `unseal_mode = "passphrase"`
    /// prompts for the unseal key (masked, never argv).
    Unlock,
    /// Store (create or overwrite) a secret value under `key`.
    ///
    /// The value is ALWAYS read from a masked/piped prompt — never accepted
    /// as a bare argv token (D-03: shell-history / `ps` leak).
    Set {
        /// Secret key name (e.g. a provider name consulted by
        /// `ProviderResolver::apply_vault_fallback`).
        key: String,
    },
    /// List secret key NAMES only, optionally prefix-filtered — never values (D-15).
    List {
        /// Only list keys starting with this prefix.
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Import provider API keys from `~/.ironhermes/.env` into the vault (D-04).
    ///
    /// Ordered flow, safe-by-ordering: (1) write a `0600` timestamped backup of the
    /// FULL original `.env` BEFORE any edit, (2) write each provider key into the
    /// vault, (3) scrub from `.env` ONLY the keys whose vault-write succeeded, (4)
    /// print a reminder naming the backup path. A failed vault-write for a key
    /// leaves that key's line in `.env` untouched. Scope is provider API keys only
    /// (D-02/D-13) — never platform/gateway/telegram tokens or profile `.env`s.
    Migrate,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch a `vault` subcommand to the appropriate handler.
pub async fn handle_vault_command(cmd: VaultCommands) -> Result<()> {
    match cmd {
        VaultCommands::Init => cmd_init().await,
        VaultCommands::Unlock => cmd_unlock().await,
        VaultCommands::Set { key } => cmd_set(key).await,
        VaultCommands::List { prefix } => cmd_list(prefix).await,
        VaultCommands::Migrate => cmd_migrate().await,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve `config.vault`, filling the empty-`PathBuf` `data_dir` sentinel with
/// the runtime default `get_hermes_home().join("vault")` (see module doc). Every
/// vault CLI handler routes through this so `init`'s data dir and `set`/`list`'s
/// `open_store` call agree on the same on-disk location.
///
/// UAT gap G-46.8-1 fix: delegates to [`ironhermes_core::resolve_vault_config`] — the
/// same shared resolver every runtime `open_store` call site (server, cron-runner) now
/// uses, so there is exactly ONE resolution point instead of a CLI-only duplicate.
/// Behavior is unchanged (byte-for-byte the same `get_hermes_home().join("vault")`
/// fill-when-empty logic, now centralized).
pub(crate) fn resolve_vault_config(config: &Config) -> ironhermes_vault::VaultConfig {
    ironhermes_core::resolve_vault_config(config)
}

/// Read a secret value: masked TTY prompt when interactive, otherwise a single
/// line from stdin (piped/non-interactive — CI, `echo secret | ironhermes vault
/// set key`, or this crate's own integration tests). Never accepted as a bare
/// argv token either way (D-03).
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

/// Append exactly ONE audit entry for a vault CLI operation (D-06).
///
/// `tool_name` is the key NAME (or `"-"` for init/unlock/list-all); `args`
/// carries `{"success": <bool>}` only — a value never crosses into the audit
/// sink. Called even on failure (sealed vault, no-op list, etc.) — no vault op
/// silently skips the audit trail.
async fn record_audit(kind: &str, tool_name: &str, success: bool) -> Result<()> {
    let audit =
        ironhermes_core::audit::AuditLog::load(ironhermes_core::audit::AuditConfig::default());
    let entry = audit.make_entry(
        kind,
        tool_name,
        None,
        "cli",
        "cli",
        "n/a",
        "operator vault CLI operation",
        if success { "approved" } else { "denied" },
        "operator",
        &serde_json::json!({ "success": success }),
    );
    audit
        .append(&entry)
        .await
        .context("failed to append vault audit entry")?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// cmd_init (D-05, feature-gated — `init`/`unlock` need the concrete
// `RustyVaultStore` type, not just the `SecretStore` trait object `open_store`
// returns)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "rusty-vault")]
async fn cmd_init() -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let resolved = resolve_vault_config(&config);
    let data_dir = resolved.rusty_vault.data_dir.clone();

    let init_result = ironhermes_vault::RustyVaultStore::init(&resolved.rusty_vault)
        .context("failed to initialize vault — is it already initialized at this data dir?");

    record_audit("vault_init", "-", init_result.is_ok()).await?;
    init_result?;

    println!(
        "{} Vault initialized at {}",
        "✓".green().bold(),
        data_dir.display()
    );
    println!(
        "  Unseal keyfile written next to the data dir with {} permissions — \
         back it up securely; key material is never printed.",
        "0600".bold()
    );
    if resolved.rusty_vault.unseal_mode == "passphrase" {
        println!(
            "  unseal_mode = \"passphrase\" — run `{}` before `set`/`list`.",
            "ironhermes vault unlock".bold()
        );
    } else {
        println!(
            "  unseal_mode = \"keyfile\" — subsequent commands auto-unseal, no `unlock` needed."
        );
    }

    Ok(())
}

#[cfg(not(feature = "rusty-vault"))]
async fn cmd_init() -> Result<()> {
    record_audit("vault_init", "-", false).await?;
    anyhow::bail!(
        "vault init requires the `rusty-vault` feature — rebuild with `--features rusty-vault`"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// cmd_unlock (D-05)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "rusty-vault")]
async fn cmd_unlock() -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let resolved = resolve_vault_config(&config);
    let passphrase_mode = resolved.rusty_vault.unseal_mode == "passphrase";

    let unlock_result: Result<()> = (|| {
        let store = ironhermes_vault::RustyVaultStore::open(&resolved.rusty_vault).context(
            "failed to open vault — run `ironhermes vault init` first if it doesn't exist yet",
        )?;
        if passphrase_mode {
            let key = read_secret_value("unseal key: ").context("failed to read unseal key")?;
            store.unlock(&key).context("failed to unseal vault")?;
        }
        Ok(())
    })();

    record_audit("vault_unlock", "-", unlock_result.is_ok()).await?;
    unlock_result?;

    if passphrase_mode {
        println!("{} Vault unsealed", "✓".green().bold());
    } else {
        println!(
            "{} Vault already unsealed (unseal_mode = \"keyfile\" auto-unseals on open)",
            "✓".green().bold()
        );
    }

    Ok(())
}

#[cfg(not(feature = "rusty-vault"))]
async fn cmd_unlock() -> Result<()> {
    record_audit("vault_unlock", "-", false).await?;
    anyhow::bail!(
        "vault unlock requires the `rusty-vault` feature — rebuild with `--features rusty-vault`"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// cmd_set (D-03, D-08, D-15)
// ─────────────────────────────────────────────────────────────────────────────

/// Store (create or overwrite) a secret value under `key` (D-03).
///
/// The value is ALWAYS read via [`read_secret_value`] — never a bare argv
/// token. `open_store` selects the configured backend (`env-var` is read-only
/// diagnostic and will hard-error here by design; use `backend: rusty-vault`
/// to actually persist a value).
async fn cmd_set(key: String) -> Result<()> {
    if key.trim().is_empty() {
        anyhow::bail!("vault set: key name must not be empty");
    }

    // D-03: never accept the secret value as a bare argv token — always prompt.
    let value = read_secret_value("secret value: ")?;

    let config = Config::load().unwrap_or_default();
    let resolved = resolve_vault_config(&config);

    let put_result: Result<()> = async {
        let store = ironhermes_vault::open_store(&resolved)
            .context("failed to open vault store — run `ironhermes vault init`/`unlock` first")?;
        store
            .put_secret(&key, secrecy::SecretString::from(value))
            .await
            .with_context(|| {
                format!(
                    "failed to store secret {key:?} — is the vault unsealed? run \
                     `ironhermes vault unlock`"
                )
            })
    }
    .await;

    record_audit("vault_set", &key, put_result.is_ok()).await?;
    put_result?;

    println!("{} stored {}", "✓".green().bold(), key.bold());
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// cmd_list (D-15: names only, never values)
// ─────────────────────────────────────────────────────────────────────────────

/// List secret key NAMES only, optionally prefix-filtered, sorted (D-03/D-15).
///
/// An empty vault prints nothing and exits `Ok(())` (not an error).
async fn cmd_list(prefix: Option<String>) -> Result<()> {
    let config = Config::load().unwrap_or_default();
    let resolved = resolve_vault_config(&config);

    let list_result: Result<Vec<String>> = async {
        let store = ironhermes_vault::open_store(&resolved)
            .context("failed to open vault store — run `ironhermes vault init`/`unlock` first")?;
        let mut names = store
            .list_secrets(prefix.as_deref())
            .await
            .with_context(|| {
                "failed to list vault secrets — is the vault unsealed? run \
                 `ironhermes vault unlock`"
            })?;
        // D-03 ordering: sort explicitly — never rely on a backend's own
        // ordering guarantee (EnvVarStore does not sort its injected keys).
        names.sort();
        Ok(names)
    }
    .await;

    // tool = "-" for list regardless of --prefix (this is a list-all-matching
    // op, not a single-key op).
    record_audit("vault_list", "-", list_result.is_ok()).await?;
    let names = list_result?;

    for name in &names {
        println!("{name}");
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// cmd_migrate (D-04, D-06, D-13, D-15 — Plan 06)
// ─────────────────────────────────────────────────────────────────────────────

/// A single provider-API-key line found in `.env`, matched against the known
/// env-var → provider-name map (see [`provider_env_var_map`]).
struct MigrateMatch {
    /// Line index into the ORIGINAL `.env` (split on `\n`) — used to rewrite the
    /// file by exclusion rather than by re-serializing the whole file, so every
    /// non-matching line survives byte-for-byte.
    line_idx: usize,
    /// Provider NAME (matches `ProviderResolver::apply_vault_fallback`'s lookup
    /// key — Plan 04) — this, not the env-var name, is what the vault is keyed by.
    provider: String,
    /// The secret value — wrapped in [`secrecy::SecretString`] the moment it is
    /// extracted from the plaintext line, and never printed/logged/audited.
    value: secrecy::SecretString,
}

/// Build the env-var-name → provider-name map used to identify provider API key
/// lines in `.env` (D-02/D-13 scope fence: provider API keys only).
///
/// Combines the three legacy built-in names `ProviderResolver` recognizes
/// (`provider.rs` priority 3) with every configured `providers.<name>.api_key_env`
/// (`provider.rs` priority 1) — the same two sources `ProviderResolver` itself
/// consults, so `migrate` never invents a parallel naming scheme.
fn provider_env_var_map(config: &Config) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    map.insert("OPENROUTER_API_KEY".to_string(), "openrouter".to_string());
    map.insert("ANTHROPIC_API_KEY".to_string(), "anthropic".to_string());
    map.insert("OPENAI_API_KEY".to_string(), "openai".to_string());
    for (provider_name, provider_cfg) in &config.providers {
        if let Some(env_name) = &provider_cfg.api_key_env {
            map.insert(env_name.clone(), provider_name.clone());
        }
    }
    map
}

/// Parse `.env` contents for provider-API-key lines, using `env_map` (env-var
/// name → provider name) to identify matches. Non-matching lines (comments,
/// blanks, platform/gateway/telegram tokens, unrelated config) are left alone —
/// this function only READS; the caller decides what to scrub.
fn find_provider_key_lines(
    contents: &str,
    env_map: &std::collections::HashMap<String, String>,
) -> Vec<MigrateMatch> {
    let mut matches = Vec::new();
    for (line_idx, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // WR-02 (46.8-gap): strip an optional leading `export ` token before the
        // key/value split, mirroring what the project's own `dotenvy`-based
        // loader (`main.rs`'s `dotenvy::from_path`) already accepts. Without
        // this, `export ANTHROPIC_API_KEY=...` silently fails to match any
        // provider key (the whole "export ANTHROPIC_API_KEY" string never
        // equals a bare env-var name) and is left untouched with no warning.
        let trimmed = trimmed
            .strip_prefix("export ")
            .map(str::trim_start)
            .unwrap_or(trimmed);
        let Some((raw_key, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let Some(provider) = env_map.get(key) else {
            continue;
        };
        let value = raw_value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        matches.push(MigrateMatch {
            line_idx,
            provider: provider.clone(),
            value: secrecy::SecretString::from(value),
        });
    }
    matches
}

/// Write a `0600` timestamped backup of the FULL original `.env` contents at
/// `~/.ironhermes/.env.pre-vault-<ts>.bak`. MUST be called before any scrub
/// (D-04 safe-by-ordering) — mirrors `audit.rs`'s `OpenOptions::mode(0o600)` +
/// redundant `set_permissions(0o600)` idiom (belt-and-suspenders against a
/// permissive umask).
fn write_env_backup(original_contents: &str) -> Result<std::path::PathBuf> {
    use std::io::Write as _;

    // Nanosecond precision (not just seconds) so two `migrate` invocations in
    // quick succession (e.g. an idempotency test running migrate twice in a
    // row) never collide on the same backup filename — D-04 (idempotency):
    // "never overwrites a prior backup".
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ");
    let backup_path = get_hermes_home().join(format!(".env.pre-vault-{ts}.bak"));

    if let Some(parent) = backup_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).context("failed to create home dir for .env backup")?;
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&backup_path)
        .with_context(|| format!("failed to create .env backup at {}", backup_path.display()))?;
    f.write_all(original_contents.as_bytes())
        .context("failed to write .env backup contents")?;
    f.flush().context("failed to flush .env backup")?;

    // Redundant chmod 0600 — safety net mirroring audit.rs (PITFALLS §A-1).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&backup_path, std::fs::Permissions::from_mode(0o600))
            .context("failed to set 0600 permissions on .env backup")?;
    }

    Ok(backup_path)
}

/// Import provider API keys from `~/.ironhermes/.env` into the vault (D-04).
///
/// Ordered flow (safe-by-ordering — an interruption between any two steps never
/// loses a key):
/// 1. Resolve + read `.env`. A missing file is a true no-op (nothing to back up,
///    nothing to migrate) — exits `Ok(())` immediately.
/// 2. Identify provider-API-key lines via [`provider_env_var_map`] /
///    [`find_provider_key_lines`] — strictly provider API keys (D-02/D-13); every
///    other line (comments, blanks, platform/gateway/telegram tokens, profile
///    env vars) is left untouched.
/// 3. Write the `0600` timestamped backup of the FULL original `.env` —
///    unconditionally, whenever `.env` exists, even if this particular run finds
///    zero NEW keys to migrate (D-04 idempotency: a second run still writes a
///    fresh, distinct backup). This happens BEFORE any scrub.
/// 4. For each matched key: `open_store(&resolved).put_secret(provider, value)`.
///    On success, mark the line for scrubbing and audit `{success:true}`
///    (`kind:"vault_migrate"`, `tool`: provider name). On failure, leave the
///    line untouched and audit `{success:false}` — a failed vault-write for a
///    key NEVER causes that key to be scrubbed (D-04 prohibition).
/// 5. Rewrite `.env` removing ONLY the successfully-migrated lines, byte-for-byte
///    identical otherwise. If nothing was migrated this run, `.env` is never
///    rewritten at all (no destructive write for a no-op).
/// 6. Print a reminder naming the backup PATH only — never a value (D-15).
async fn cmd_migrate() -> Result<()> {
    let env_path = Config::env_path();

    let original_contents = match std::fs::read_to_string(&env_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "{} nothing to migrate — no .env file found at {}",
                "✓".green().bold(),
                env_path.display()
            );
            return Ok(());
        }
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", env_path.display()));
        }
    };

    let config = Config::load().unwrap_or_default();
    let env_map = provider_env_var_map(&config);
    let matches = find_provider_key_lines(&original_contents, &env_map);

    // D-04: backup precedes any edit, unconditionally, whenever `.env` exists —
    // even a run that finds zero NEW keys to migrate (idempotency: still writes
    // a fresh, distinct backup). Only the `.env` REWRITE below is skipped when
    // there is nothing to scrub — the backup itself is never skipped.
    let backup_path = write_env_backup(&original_contents)?;

    if matches.is_empty() {
        println!(
            "{} nothing to migrate — no provider API keys found in {}",
            "✓".green().bold(),
            env_path.display()
        );
        println!(
            "  (a precautionary backup was written to {} — safe to delete)",
            backup_path.display().to_string().bold()
        );
        return Ok(());
    }

    let resolved = resolve_vault_config(&config);
    let store = ironhermes_vault::open_store(&resolved)
        .context("failed to open vault store — run `ironhermes vault init`/`unlock` first")?;

    let mut scrub_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut migrated = 0usize;
    let mut failed: Vec<String> = Vec::new();

    for m in matches {
        let MigrateMatch {
            line_idx,
            provider,
            value,
        } = m;

        // D-08: `value` moves straight from the parsed `.env` line into
        // `put_secret` — never exposed, logged, printed, or audited.
        let put_result = store
            .put_secret(&provider, value)
            .await
            .with_context(|| format!("failed to migrate provider key {provider:?} into vault"));
        let success = put_result.is_ok();

        // D-06: one audit entry per migrated key — provider NAME + {success}
        // only, never a value.
        record_audit("vault_migrate", &provider, success).await?;

        if success {
            scrub_lines.insert(line_idx);
            migrated += 1;
        } else {
            // D-04 (safe-by-ordering): a failed vault-write leaves the key's
            // line intact in `.env` — never scrubbed.
            failed.push(provider);
        }
    }

    if !scrub_lines.is_empty() {
        // WR-03 (46.8-gap): `str::lines()` splits on both `"\n"` and `"\r\n"`,
        // stripping the trailing `\r` in the CRLF case — so unconditionally
        // rejoining with a bare `"\n"` silently converts every KEPT line's
        // ending too, not just the scrubbed one, contradicting this
        // function's own "byte-for-byte identical otherwise" doc contract.
        // Detect the original file's dominant line ending and reuse it.
        let eol = if original_contents.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let had_trailing_newline = original_contents.ends_with('\n');
        let kept_lines: Vec<&str> = original_contents
            .lines()
            .enumerate()
            .filter(|(idx, _)| !scrub_lines.contains(idx))
            .map(|(_, line)| line)
            .collect();
        let mut new_contents = kept_lines.join(eol);
        if had_trailing_newline && !new_contents.is_empty() {
            new_contents.push_str(eol);
        }
        std::fs::write(&env_path, new_contents)
            .with_context(|| format!("failed to rewrite {} after migrate", env_path.display()))?;
    }

    println!(
        "{} migrated {} provider key(s) into the vault",
        "✓".green().bold(),
        migrated
    );
    if !failed.is_empty() {
        println!(
            "{} {} key(s) failed to migrate and were left in {} untouched: {}",
            "!".yellow().bold(),
            failed.len(),
            env_path.display(),
            failed.join(", ")
        );
    }
    println!(
        "  backup written to {} — delete it once resolution from the vault is verified.",
        backup_path.display().to_string().bold()
    );

    Ok(())
}
