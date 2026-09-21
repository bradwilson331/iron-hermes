//! `ironhermes vault migrate-profile <slug>` — per-profile `.env` → vault migration
//! (Phase 51, D-04/D-10/D-14, Plan 08).
//!
//! Mirrors `vault_cmd.rs::cmd_migrate`'s proven safe-by-ordering flow (backup → vault-write →
//! scrub-only-written → print backup path), but destined for
//! [`ironhermes_vault::ProfileSecretStore`] at `secret/profiles/{slug}/{provider}` instead of
//! the root `secret/providers/` keyspace — `SecretStore::put_secret` rejects any `/` in its key
//! and prefixes `secret/providers/` unconditionally, so it cannot address a profile path at any
//! argument value (`crates/ironhermes-vault/src/rusty_vault_store.rs:241-245,474`).
//!
//! # A NEW subcommand, never a flag on the root `Migrate` (D-04)
//!
//! `VaultCommands::Migrate`'s own doc comment states its scope is root provider keys and
//! explicitly "never platform/gateway/telegram tokens or profile `.env`s" — a `--profile` flag
//! on it would make that sentence false. `MigrateProfile { slug, dry_run }` is a second,
//! independent verb: `ironhermes vault migrate-profile <slug> [--dry-run]`.
//!
//! # The substitution trap this module exists to avoid (CR-03 lineage)
//!
//! `dotenvy` resolves `${VAR}` references against the READING process's own environment, and
//! this CLI loads the root `.env` into its own environment at startup. A migration that read a
//! profile's `.env` through that parser would hand the operator's ROOT credential to whichever
//! profile's file happened to reference it, and then PERSIST that value into the profile's own
//! vault entry — the exact CR-03 exfiltration (Phase 47.4), arriving through the tool built to
//! retire it. [`find_raw_env_line`] never calls `dotenvy`; it reads the bytes between the first
//! `=` and the end of the physical line, verbatim, with no trimming and no quote-stripping. Every
//! value this project's own writer produces for a profile `.env` is single-line and strong-quoted
//! (`render_profile_env` / `quote_env_value`), so "one physical line" IS "one logical line" for
//! every value this migration will ever see in practice — which is what makes the two `dotenvy`
//! parser hazards (an unquoted value truncating at a space; a trailing backslash merging the next
//! line) structurally impossible here rather than merely avoided by convention.
//!
//! # CR-03: the writer's own quoting must be undone, not just tolerated
//!
//! The paragraph above (pre-Phase-51-Plan-15) reasoned about the writer's strong-quoting ONLY
//! for what it does to line-splitting — it never asked what happens to the two literal quote
//! characters `find_raw_env_line` faithfully preserves in the extracted bytes. Nothing undid
//! them: the quotes went into the vault, and `worker_bootstrap.rs` then installed a *quoted*
//! string into the provider's env var, producing `Bearer 'sk-or-v1-...'` instead of `Bearer
//! sk-or-v1-...` — a 401 with the plaintext already scrubbed (CR-03). `ironhermes_core::
//! dotenv_write::unquote_env_value` is the fix, and WHERE it runs is the whole safety argument:
//! it is applied to `find_raw_env_line`'s output — bytes already extracted byte-for-byte,
//! substitution-free — and produces a plain `String`, never re-entering any `dotenvy` parsing.
//! The `${VAR}` exfiltration path this module exists to close stays closed because the decode
//! step never looks at the process environment at all; it only rewrites quote characters already
//! sitting in memory.
//!
//! Deliberately does NOT reuse `find_provider_key_lines` (`vault_cmd.rs:412-450`): its value
//! extraction is `raw_value.trim().trim_matches('"').trim_matches('\'')`, a transformation that
//! would silently alter a value with a leading/trailing quote or space on its way into the vault
//! — the same bug class as the substitution trap, one step quieter (T-51-46b). Only its
//! `line_idx`-preserving match structure and `cmd_migrate`'s exclusion-based rewrite are reused
//! here; the value extraction is written fresh.
//!
//! # D-10: two distinct failure moments, two distinct outcomes
//!
//! **Preflight** (vault sealed/uninitialized/unreachable) aborts before anything is written — no
//! backup, no vault entry, no scrub. **Per-key** (the vault write itself fails, once preflight has
//! passed) leaves the complete `0600` timestamped backup in place and the source line untouched.
//! These are different moments with different correct outcomes, kept structurally distinct in
//! [`migrate_profile`] rather than folded into one code path.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use ironhermes_core::config::Config;
// Pre-existing Phase-51 gap: consumed only by `migrate_profile_inner`, which is
// `#[cfg(feature = "rusty-vault")]`-gated below — genuinely unused in a default (no-feature)
// build. `#[allow]` rather than `#[cfg]`-gating the import itself, matching the idiom used on
// the helper fns below.
#[allow(unused_imports)]
use ironhermes_core::constants::PROFILES_SUBDIR;
#[allow(unused_imports)]
use ironhermes_core::get_hermes_home;

/// Resolve the single env-var name a profile's configured provider's key would live under.
///
/// Phase 51 Plan 15 (CR-04): delegates to [`ironhermes_core::provider_env::provider_api_key_env_name`]
/// — the single shared resolver ALSO called by `worker_bootstrap::resolve_worker_api_key_env` —
/// rather than keeping this migration's own copy of the two-tier rule (explicit
/// `providers.<name>.api_key_env` first, falling back to a built-in legacy name for one of the
/// three bundled providers). Before this change, this crate answered the same question two
/// different ways in two different files: this migration's two-tier resolver, and
/// `worker_bootstrap`'s one-tier resolver (explicit override only, no built-in fallback) — so
/// migration would scrub a profile the worker could never actually consume. `None` means no
/// single env-var name could be determined — the caller reports that as nothing to migrate
/// rather than guessing. `pub(crate)` so `worker_bootstrap`'s own cross-path equality test can
/// assert this function and its own return the SAME answer for every case.
///
/// This and the three helper fns below (`find_raw_env_line`, `rewrite_excluding_line`,
/// `write_profile_env_backup`) are pure, reusable, independently unit-tested logic with no side
/// effects, but their only production callers (`migrate_profile_inner`) sit behind
/// `#[cfg(feature = "rusty-vault")]` — so a default (no-feature) build correctly reports them as
/// dead code. `#[allow(dead_code)]` rather than `#[cfg(feature = "rusty-vault")]`-gating the
/// functions themselves: `find_raw_env_line` and `rewrite_excluding_line` are ALSO exercised
/// directly by this file's own `#[cfg(test)] mod unit_tests` below, independent of the vault
/// feature, and feature-gating would either break those tests under a non-`--all-features` test
/// build or force gating the tests too — unnecessarily narrowing coverage of pure logic that has
/// nothing to do with `rusty_vault` itself.
#[allow(dead_code)]
pub(crate) fn resolve_provider_env_var_name(config: &Config, main: &str) -> Option<String> {
    ironhermes_core::provider_env::provider_api_key_env_name(config, main)
}

/// Find `env_var_name`'s value on its own physical, uncommented line — see the module doc for
/// why "one physical line" is the deliberately chosen unit and why that is safe for every value
/// this migration will ever see. Returns `(line_idx, raw_value)` for the FIRST match; `raw_value`
/// is everything after the first `=` to the end of the line, byte for byte.
#[allow(dead_code)]
fn find_raw_env_line(contents: &str, env_var_name: &str) -> Option<(usize, String)> {
    for (line_idx, line) in contents.lines().enumerate() {
        let trimmed_start = line.trim_start();
        if trimmed_start.is_empty() || trimmed_start.starts_with('#') {
            continue;
        }
        let Some(eq_idx) = line.find('=') else {
            continue;
        };
        let key = line[..eq_idx].trim();
        if key == env_var_name {
            return Some((line_idx, line[eq_idx + 1..].to_string()));
        }
    }
    None
}

/// Rewrite `original_contents` with physical line `line_idx` removed, preserving CRLF/LF and
/// trailing-newline state — the same exclusion-by-index technique `cmd_migrate`'s rewrite uses
/// (`vault_cmd.rs:598-623`), which exists because of a real CRLF-corruption bug; reimplementing
/// it differently would reintroduce that bug.
#[allow(dead_code)]
fn rewrite_excluding_line(original_contents: &str, line_idx: usize) -> String {
    let eol = if original_contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let had_trailing_newline = original_contents.ends_with('\n');
    let kept_lines: Vec<&str> = original_contents
        .lines()
        .enumerate()
        .filter(|(idx, _)| *idx != line_idx)
        .map(|(_, line)| line)
        .collect();
    let mut new_contents = kept_lines.join(eol);
    if had_trailing_newline && !new_contents.is_empty() {
        new_contents.push_str(eol);
    }
    new_contents
}

/// Write a `0600` timestamped backup of the FULL original profile `.env` at
/// `<profile_dir>/.env.pre-vault-<ts>.bak` — parameterizes `vault_cmd.rs::write_env_backup`'s
/// atomic-`0600` idiom by the profile's own directory instead of the home directory. MUST be
/// called AFTER preflight and BEFORE the first vault write (D-10).
#[allow(dead_code)]
fn write_profile_env_backup(profile_dir: &Path, original_contents: &str) -> Result<PathBuf> {
    use std::io::Write as _;

    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ");
    let backup_path = profile_dir.join(format!(".env.pre-vault-{ts}.bak"));

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

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&backup_path, std::fs::Permissions::from_mode(0o600))
            .context("failed to set 0600 permissions on .env backup")?;
    }

    Ok(backup_path)
}

/// Import one profile's provider credential from its `.env` into the vault (D-04/D-10/D-14).
///
/// Ordered flow, safe-by-ordering:
/// 1. Validate `slug`, load the profile's `config.yaml`, resolve its single provider + env-var
///    name.
/// 2. Read `.env` RAW (never through `dotenvy`). A missing file, or a file with no entry for the
///    resolved env-var name, is a true no-op — exits `Ok` having written nothing.
/// 3. `--dry-run` reports what would move and returns here — no backup, no vault entry, no file
///    change.
/// 4. **Preflight**: open the vault. Sealed/uninitialized/unreachable aborts here — no backup, no
///    vault entry, no scrub.
/// 5. Write the `0600` timestamped backup of the FULL original file.
/// 6. Write the value to `secret/profiles/{slug}/{provider}` through
///    [`ironhermes_vault::ProfileSecretStore::put_profile_secret`] — never `SecretStore::put_secret`,
///    which cannot address a profile path. On failure: the line stays untouched, the backup
///    remains as the safety net, and the error names both.
/// 7. On success, scrub only that line and rewrite, preserving line-ending/trailing-newline state
///    and every other line byte-for-byte. If the rewrite itself fails, the value is now in TWO
///    PLACES (vault + still-plaintext `.env`, backup also intact) — reported as such, never as a
///    silent loss.
#[cfg(feature = "rusty-vault")]
pub async fn migrate_profile(slug: String, dry_run: bool) -> Result<()> {
    let profiles_root = get_hermes_home().join(PROFILES_SUBDIR);
    match migrate_profile_inner(&profiles_root, &slug, dry_run).await {
        Ok(msg) => {
            println!("{msg}");
            Ok(())
        }
        Err(e) => {
            eprintln!("{e}");
            Err(e)
        }
    }
}

#[cfg(feature = "rusty-vault")]
async fn migrate_profile_inner(profiles_root: &Path, slug: &str, dry_run: bool) -> Result<String> {
    ironhermes_core::profile::validate_profile_name(slug)
        .map_err(|e| anyhow::anyhow!("\"{slug}\" is not a valid profile name: {e}"))?;

    let profile_dir = profiles_root.join(slug);
    if !profile_dir.is_dir() {
        anyhow::bail!(
            "profile \"{slug}\" has no directory at {}",
            profile_dir.display()
        );
    }

    let config_path = profile_dir.join("config.yaml");
    if !config_path.is_file() {
        anyhow::bail!("profile \"{slug}\" has no config.yaml");
    }
    let config = Config::load_from(&config_path)
        .with_context(|| format!("profile \"{slug}\" config.yaml did not parse"))?;

    let main = config.model.provider.clone();
    if main.is_empty() {
        anyhow::bail!("profile \"{slug}\" config.yaml sets no model.provider");
    }

    let env_var_name = resolve_provider_env_var_name(&config, &main).ok_or_else(|| {
        anyhow::anyhow!(
            "profile \"{slug}\" is configured for provider \"{main}\", but no single env-var \
             name could be resolved for it — nothing to migrate"
        )
    })?;

    let env_path = profile_dir.join(".env");
    let original_contents = match std::fs::read_to_string(&env_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(format!(
                "nothing to migrate for profile \"{slug}\" — no .env file found at {}",
                env_path.display()
            ));
        }
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", env_path.display()));
        }
    };

    let Some((line_idx, raw_value)) = find_raw_env_line(&original_contents, &env_var_name) else {
        return Ok(format!(
            "nothing to migrate for profile \"{slug}\" — no entry for {env_var_name} found in {}",
            env_path.display()
        ));
    };

    // CR-03: undo the writer's own quoting — applied to `find_raw_env_line`'s already-extracted,
    // substitution-free bytes, never re-entering `dotenvy` (see the module doc's "CR-03" section).
    let decoded_value = ironhermes_core::dotenv_write::unquote_env_value(&raw_value);

    // T-51-87 ruling (Task 3 <behavior>): an UNQUOTED value containing a space is ambiguous.
    // This project's own writer always strong-quotes (`quote_env_value`), so an unquoted value
    // with a space is necessarily hand-edited — and the REAL running process would load it via
    // `dotenvy`, which truncates an unquoted value at the first space. Storing the full,
    // untruncated text into the vault would silently persist MORE than the runtime ever actually
    // used as the credential. Re-deriving "what dotenvy would have resolved" would mean invoking
    // a substituting parser on this path — exactly what this module exists to avoid (it could
    // also expand a `${VAR}` reference for an unrelated reason). Refusing is the only option that
    // does not risk re-opening either hazard: fail closed, name the fix, leave everything
    // untouched (checked BEFORE dry-run's own early return, so a dry-run previews this refusal
    // too).
    let was_quote_wrapped = raw_value.len() >= 2
        && ((raw_value.starts_with('\'') && raw_value.ends_with('\''))
            || (raw_value.starts_with('"') && raw_value.ends_with('"')));
    if !was_quote_wrapped && decoded_value.contains(' ') {
        anyhow::bail!(
            "profile \"{slug}\"'s {env_var_name} value in {} is UNQUOTED and contains a space \
             — this project's own writer always strong-quotes values, so an unquoted value with \
             a space is almost certainly hand-edited, and the real running process would only \
             ever see dotenvy's own truncation of it at the first space, never the full text. \
             Refusing rather than guessing which substring was actually in use: wrap the value \
             in single quotes in the .env file, then re-run the migration. Nothing was written.",
            env_path.display()
        );
    }

    let vault_path = format!("secret/profiles/{slug}/{main}");

    if dry_run {
        let dry_run_msg = format!(
            "[dry-run] profile \"{slug}\": would move {env_var_name} from {} to {vault_path}; \
             nothing written.",
            env_path.display()
        );
        // Task 3 (G-51-5, Test 5): best-effort unreachable-leaf report, ONLY when the vault
        // is reachable — an unreachable/uninitialized vault is the normal case for a
        // not-yet-migrated profile and must not turn a dry-run into an error. `--dry-run`'s
        // existing no-op contract (no backup, no vault entry, no file change) is unaffected:
        // this only READS.
        let dry_run_vault_cfg = ironhermes_core::resolve_vault_config(&config);
        if let Ok(store) = ironhermes_vault::RustyVaultStore::open(&dry_run_vault_cfg.rusty_vault)
            && matches!(store.is_sealed(), Ok(false))
            && let Some(report) = unreachable_leaf_report(&store, slug, &config).await
        {
            return Ok(format!("{dry_run_msg}\n{report}"));
        }
        return Ok(dry_run_msg);
    }

    // Preflight — abort BEFORE touching anything (no backup, no vault entry, no scrub) if the
    // vault is sealed, uninitialized, or otherwise unreachable.
    let vault_cfg = ironhermes_core::resolve_vault_config(&config);
    let store = ironhermes_vault::RustyVaultStore::open(&vault_cfg.rusty_vault).with_context(
        || "preflight failed: vault could not be opened — aborting before any backup or write",
    )?;
    let sealed = store.is_sealed().with_context(|| {
        "preflight failed: could not read vault seal state — aborting before any backup or write"
    })?;
    if sealed {
        anyhow::bail!(
            "preflight failed: vault is sealed — run `ironhermes vault unlock` first; aborting \
             before any backup or write"
        );
    }

    // Backup — AFTER preflight, BEFORE the first vault write (D-10's whole undo story).
    let backup_path = write_profile_env_backup(&profile_dir, &original_contents)?;

    // Destination write goes through Plan 09's ProfileSecretStore — never `SecretStore::put_secret`,
    // which rejects any `/` in its key and prefixes `secret/providers/` unconditionally, and so
    // cannot address a profile path at any argument value (T-51-46c).
    let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(&store);
    let value = secrecy::SecretString::from(decoded_value);
    if let Err(e) = profile_store.put_profile_secret(slug, &main, value).await {
        anyhow::bail!(
            "vault write failed for profile \"{slug}\" provider \"{main}\" ({e}); the line for \
             {env_var_name} in {} was left untouched. A full backup of the original file is at \
             {} — the value never left {}.",
            env_path.display(),
            backup_path.display(),
            env_path.display()
        );
    }

    let new_contents = rewrite_excluding_line(&original_contents, line_idx);
    if let Err(e) = std::fs::write(&env_path, &new_contents) {
        anyhow::bail!(
            "profile \"{slug}\": the value for provider \"{main}\" is now in TWO PLACES — it \
             was written to the vault at {vault_path}, but rewriting {} to remove the plaintext \
             copy FAILED ({e}). Neither copy was destroyed: the vault entry is live and the \
             original .env is unchanged. A full pre-migration backup also remains at {}. This \
             leaves a still-exposed plaintext value in {} until you resolve it: (1) verify \
             {env_var_name} resolves correctly from the vault for this profile, then (2) remove \
             the {env_var_name} line from {} by hand.",
            env_path.display(),
            backup_path.display(),
            env_path.display(),
            env_path.display()
        );
    }

    let mut result_msg = format!(
        "migrated provider \"{main}\" (env var \"{env_var_name}\") for profile \"{slug}\" into \
         the vault at {vault_path}; the key was removed from {}; backup written to {} — delete \
         it once resolution from the vault is verified.",
        env_path.display(),
        backup_path.display()
    );

    // Task 3 (G-51-5): after the migration has completed, report any leaf ALREADY sitting in
    // this profile's own vault subtree that the CURRENT config.yaml cannot reach or cannot
    // install — reusing the SAME `store` this migration just wrote through, never a second
    // vault open. Never changes this function's `Ok`/`Err` outcome (see the helper's own doc).
    if let Some(report) = unreachable_leaf_report(&store, slug, &config).await {
        result_msg = format!("{result_msg}\n{report}");
    }

    Ok(result_msg)
}

/// Phase 51 Plan 18 (G-51-5, Task 3): build the operator-facing unreachable-leaf report for
/// `slug`, given an already-open `store`. Lists the profile's own vault leaf NAMES via
/// [`ironhermes_vault::ProfileSecretStore::list_profile_secret_names`] — never
/// [`ironhermes_vault::SecretStore::list_secrets`], which is rooted at `secret/providers/` and
/// can never address a profile path — and cross-checks each leaf against `config`'s resolvable
/// provider set ([`ironhermes_core::ProviderResolver::endpoint_names`], Task 2's accessor) and
/// against [`resolve_provider_env_var_name`].
///
/// Returns `None` when there is nothing to report (the common case: a clean profile, or a
/// vault subtree with no leaves at all). NEVER changes the caller's control flow or exit
/// status: any failure listing or resolving is folded into a one-line note instead of
/// propagated — "a diagnostic that can break the thing it diagnoses is worse than no
/// diagnostic." NEVER prints a secret VALUE — leaf and provider NAMES only (D-15).
///
/// Does NOT migrate anything it finds — see this plan's prohibitions on extending
/// `migrate-profile` to move additional providers into the vault (D-10's scrub stays
/// destructive and deliberately non-automatic).
///
/// Feature-gated alongside its only two call sites (`migrate_profile_inner`'s dry-run and
/// real-write paths, both `#[cfg(feature = "rusty-vault")]` above) — without this attribute the
/// function's `&ironhermes_vault::RustyVaultStore` parameter fails to resolve whenever this
/// crate is built without `--features rusty-vault` (the default), since `RustyVaultStore`
/// itself only exists behind that same feature (`ironhermes-vault/src/lib.rs`).
#[cfg(feature = "rusty-vault")]
async fn unreachable_leaf_report(
    store: &ironhermes_vault::RustyVaultStore,
    slug: &str,
    config: &Config,
) -> Option<String> {
    let profile_store = ironhermes_vault::ProfileSecretStore::from_rusty_vault_store(store);
    let leaves = match profile_store.list_profile_secret_names(slug).await {
        Ok(l) => l,
        Err(e) => {
            return Some(format!(
                "note: could not list profile \"{slug}\"'s vault secrets to check for \
                 unreachable leaves ({e}) — this does not affect the migration's own outcome."
            ));
        }
    };
    if leaves.is_empty() {
        return None;
    }

    let resolver = match ironhermes_core::ProviderResolver::build_with_env_overrides_strict(
        config,
        ironhermes_core::ModelsCache::default(),
        &std::collections::HashMap::new(),
    ) {
        Ok(r) => r,
        Err(e) => {
            return Some(format!(
                "note: could not build a resolver for profile \"{slug}\" to check for \
                 unreachable leaves ({e}) — this does not affect the migration's own outcome."
            ));
        }
    };
    let resolvable: std::collections::HashSet<String> =
        resolver.endpoint_names().into_iter().collect();

    let mut unreachable: Vec<String> = Vec::new();
    let mut uninstallable: Vec<String> = Vec::new();
    for leaf in &leaves {
        if !resolvable.contains(leaf) {
            unreachable.push(leaf.clone());
        } else if resolve_provider_env_var_name(config, leaf).is_none() {
            uninstallable.push(leaf.clone());
        }
    }

    if unreachable.is_empty() && uninstallable.is_empty() {
        return None;
    }

    let mut lines: Vec<String> = Vec::new();
    if !unreachable.is_empty() {
        lines.push(format!(
            "unreachable: profile \"{slug}\"'s vault subtree holds a secret for {} that no \
             resolver endpoint names — a worker for this profile can never read {}: {}",
            if unreachable.len() == 1 {
                "provider"
            } else {
                "providers"
            },
            if unreachable.len() == 1 { "it" } else { "them" },
            unreachable.join(", ")
        ));
    }
    if !uninstallable.is_empty() {
        lines.push(format!(
            "no env var: profile \"{slug}\"'s vault subtree holds a secret for {} with no \
             resolvable api-key env var name — add providers.<name>.api_key_env to \
             config.yaml (or use a built-in provider name) so a spawned worker knows which \
             variable to install {} into: {}",
            if uninstallable.len() == 1 {
                "provider"
            } else {
                "providers"
            },
            if uninstallable.len() == 1 {
                "it"
            } else {
                "them"
            },
            uninstallable.join(", ")
        ));
    }
    Some(lines.join("\n"))
}

#[cfg(not(feature = "rusty-vault"))]
pub async fn migrate_profile(_slug: String, _dry_run: bool) -> Result<()> {
    anyhow::bail!(
        "vault migrate-profile requires the `rusty-vault` feature — rebuild with `--features rusty-vault`"
    )
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn find_raw_env_line_reads_the_bytes_after_the_first_equals() {
        let contents = "# a comment\nOTHER=x\nOPENROUTER_API_KEY=sk-abc\n";
        let (idx, value) = find_raw_env_line(contents, "OPENROUTER_API_KEY").expect("found");
        assert_eq!(idx, 2);
        assert_eq!(value, "sk-abc");
    }

    /// Task 2 (T-51-46b): the extraction must NOT trim surrounding whitespace and must NOT
    /// strip a leading/trailing quote character — both are exactly what
    /// `find_provider_key_lines`'s `.trim().trim_matches('"').trim_matches('\'')` chain does,
    /// and doing so here would silently alter a value on its way into the vault.
    #[test]
    fn find_raw_env_line_never_trims_or_strips_quotes() {
        let contents = "KEY= sk with a leading space and a trailing one \nOTHER='literally-quoted'\n";
        let (_, value) = find_raw_env_line(contents, "KEY").expect("found");
        assert_eq!(value, " sk with a leading space and a trailing one ");
        let (_, value) = find_raw_env_line(contents, "OTHER").expect("found");
        assert_eq!(value, "'literally-quoted'");
    }

    /// Phase 51 Plan 15 (CR-03): the sibling to the test above — extraction still does not
    /// trim or strip (proven above, unchanged), but the DECODE now happens one layer up, on
    /// the already-extracted bytes, via `ironhermes_core::dotenv_write::unquote_env_value`.
    /// This is `migrate_profile_inner`'s actual composition, proven at the unit level rather
    /// than only through the slower real-subprocess fixtures in `tests/profile_vault_migrate.rs`.
    #[test]
    fn extraction_then_decode_composes_to_the_original_plaintext() {
        let contents = "KEY='sk-composed-value'\n";
        let (_, raw) = find_raw_env_line(contents, "KEY").expect("found");
        assert_eq!(raw, "'sk-composed-value'", "extraction itself must not strip the quotes");
        let decoded = ironhermes_core::dotenv_write::unquote_env_value(&raw);
        assert_eq!(decoded, "sk-composed-value", "decode, one layer up, must strip them");
    }

    /// Task 2: this migration never calls the substituting parser at all — a `${...}` in the
    /// raw text is returned as literal bytes, never resolved against this process's own
    /// environment (the CR-03 mechanism).
    #[test]
    fn find_raw_env_line_never_expands_a_reference() {
        // Prove it against a name that IS actually set in this test process's own environment —
        // if extraction ever called a substituting reader, the returned value would be that
        // sentinel's value instead of the literal reference text.
        // SAFETY: test-only, single-threaded within this crate's own `--test-threads=1` gate.
        unsafe {
            std::env::set_var(
                "PROFILE_MIGRATE_UNIT_TEST_SENTINEL",
                "this-must-never-be-returned",
            );
        }
        let contents = "KEY=${PROFILE_MIGRATE_UNIT_TEST_SENTINEL}\n";
        let (_, value) = find_raw_env_line(contents, "KEY").expect("found");
        assert_eq!(value, "${PROFILE_MIGRATE_UNIT_TEST_SENTINEL}");
        unsafe {
            std::env::remove_var("PROFILE_MIGRATE_UNIT_TEST_SENTINEL");
        }
    }

    #[test]
    fn find_raw_env_line_returns_none_when_absent() {
        assert!(find_raw_env_line("OTHER=x\n", "OPENROUTER_API_KEY").is_none());
    }

    #[test]
    fn rewrite_excluding_line_preserves_other_lines() {
        let original = "A=1\nB=2\nC=3\n";
        let rewritten = rewrite_excluding_line(original, 1);
        assert_eq!(rewritten, "A=1\nC=3\n");
    }
}
