//! Integration tests for `ironhermes vault migrate-profile <slug> [--dry-run]` (Phase 51 Plan
//! 08, D-04/D-10/D-14).
//!
//! Gated `#![cfg(feature = "rusty-vault")]` — every test exercises a real `RustyVaultStore` /
//! `ProfileSecretStore`. The CLI invocation under test is spawned as a real subprocess (mirrors
//! `vault_migrate.rs`, Plan 06) with `IRONHERMES_HOME` set per-subprocess (never
//! `std::env::set_var` in this process — that races under parallel `cargo test`). The vault
//! itself is opened DIRECTLY by this test file (a `[dependencies]` entry of `ironhermes-cli`,
//! so visible to its own integration tests) using an absolute `data_dir` embedded straight into
//! each fixture profile's `config.yaml` — this sidesteps `resolve_vault_config`'s env-var-based
//! sentinel resolution entirely, so no test here needs `IRONHERMES_HOME` set in ITS OWN process
//! to verify vault content or run the dispatch gate in-process.
//!
//! `--test-threads=1` is mandatory (this crate races on env `set_var` / shared filesystem
//! state; see the plan's own `<verification>` block).

#![cfg(feature = "rusty-vault")]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use ironhermes_vault::{ProfileSecretStore, RustyVaultConfig, RustyVaultStore};
use secrecy::{ExposeSecret as _, SecretString};
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// Fixtures
// ─────────────────────────────────────────────────────────────────────────────

/// A fresh, initialized, unsealed vault — mirrors `ironhermes-vault/tests/profile_store.rs`'s
/// own helper.
fn open_fresh_vault() -> (TempDir, RustyVaultConfig) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let rv_config = RustyVaultConfig {
        data_dir: tmp.path().join("vault"),
        unseal_mode: "keyfile".to_string(),
    };
    RustyVaultStore::init(&rv_config).expect("vault init");
    (tmp, rv_config)
}

/// Same shape, but never opened this session — `unseal_mode: "passphrase"` means
/// `RustyVaultStore::open` returns a genuinely SEALED store (Task 1's preflight-failure case).
fn open_sealed_vault() -> (TempDir, RustyVaultConfig) {
    let tmp = tempfile::tempdir().expect("create temp vault data dir");
    let rv_config = RustyVaultConfig {
        data_dir: tmp.path().join("vault"),
        unseal_mode: "passphrase".to_string(),
    };
    RustyVaultStore::init(&rv_config).expect("vault init");
    let store = RustyVaultStore::open(&rv_config).expect("vault open (left sealed)");
    assert!(
        store.is_sealed().expect("read seal state"),
        "fixture must be left sealed for the preflight-failure test"
    );
    (tmp, rv_config)
}

/// A profile `config.yaml` naming `provider` as `model.provider`, with `extra_providers_yaml`
/// spliced in verbatim (e.g. a `providers:` block for a custom leaf), and an EXPLICIT `vault:`
/// section pointing at `rv_config`'s own absolute `data_dir` — this is what lets every
/// in-process assertion (direct `ProfileSecretStore` reads, the dispatch gate) resolve the exact
/// same vault the subprocess wrote to, without any env-var-based resolution or `IRONHERMES_HOME`
/// mutation in this test's own process (mirrors
/// `ironhermes-core/tests/dispatch_gate_vault_backed.rs`'s `enable_vault` helper).
fn profile_config_yaml(provider: &str, extra_providers_yaml: &str, rv_config: &RustyVaultConfig) -> String {
    format!(
        "model:\n  provider: {provider}\n{extra_providers_yaml}vault:\n  enabled: true\n  backend: rusty-vault\n  rusty_vault:\n    data_dir: \"{}\"\n    unseal_mode: {}\n",
        rv_config.data_dir.display(),
        rv_config.unseal_mode
    )
}

/// Write a profile directory at `<profiles_root>/<slug>/` with the given `config.yaml` text and
/// optional `.env` contents (`None` = no `.env` file at all). Returns the profile directory.
fn write_profile(profiles_root: &Path, slug: &str, config_yaml: &str, env_contents: Option<&str>) -> PathBuf {
    let dir = profiles_root.join(slug);
    std::fs::create_dir_all(&dir).expect("mkdir profile dir");
    std::fs::write(dir.join("config.yaml"), config_yaml).expect("write config.yaml");
    if let Some(contents) = env_contents {
        std::fs::write(dir.join(".env"), contents).expect("write .env");
    }
    dir
}

/// Spawn the real built binary: `ironhermes vault migrate-profile <slug> [--dry-run]` with
/// `IRONHERMES_HOME=home` (subprocess-local — never a race with other tests).
fn run_migrate_profile(home: &Path, slug: &str, dry_run: bool) -> std::process::Output {
    let mut cmd = Command::cargo_bin("ironhermes").unwrap();
    cmd.env("IRONHERMES_HOME", home)
        .args(["vault", "migrate-profile", slug]);
    if dry_run {
        cmd.arg("--dry-run");
    }
    cmd.output().expect("run ironhermes vault migrate-profile")
}

/// Every `.env.pre-vault-*.bak` file directly under `profile_dir`, sorted (the timestamp prefix
/// sorts chronologically) — mirrors `vault_migrate.rs`'s own `backup_files` helper.
fn backup_files(profile_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(profile_dir)
        .expect("read profile dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.starts_with(".env.pre-vault-") && name.ends_with(".bak"))
        })
        .collect();
    files.sort();
    files
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 1 — the tracer: migrate one real profile end to end, prove it still dispatches
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn migrated_profile_still_dispatches_from_the_vault() {
    use ironhermes_core::dispatch_gate::{DispatchDecision, evaluate_profile_dispatch_at};

    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "alpha";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=${ROOT_SENTINEL}\n"),
    );

    let mut cmd = Command::cargo_bin("ironhermes").unwrap();
    let output = cmd
        .env("IRONHERMES_HOME", home.path())
        .env("ROOT_SENTINEL", "root-secret-must-never-appear")
        .args(["vault", "migrate-profile", slug])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "migrate-profile must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let new_env = std::fs::read_to_string(profiles_root.join(slug).join(".env")).unwrap();
    assert!(
        !new_env.contains("OPENROUTER_API_KEY"),
        "migrated key must be scrubbed from .env; got: {new_env:?}"
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap()
        .expect("secret must exist after migration");
    assert_eq!(
        value.expose_secret(),
        "${ROOT_SENTINEL}",
        "the vault must hold the LITERAL reference text, never an expanded value"
    );

    let decision = evaluate_profile_dispatch_at(&profiles_root, slug).await;
    assert_eq!(
        decision,
        DispatchDecision::AllowFromVault,
        "a migrated profile must dispatch via the vault branch specifically, got {decision:?}"
    );
}

/// Shared fixture for the two "per-key write failure" tests below: preflight PASSES (a real,
/// unsealed vault), but the per-key `put_profile_secret` call itself is forced to fail by
/// giving the provider an invalid VAULT LEAF name (containing `/`) — a genuine failure through
/// the real validator (`validate_profile_leaf`), not a test-only seam.
fn per_key_write_failure_fixture() -> (TempDir, RustyVaultConfig, TempDir, PathBuf, &'static str) {
    let (vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "beta";
    let providers_yaml = "providers:\n  \"weird/name\":\n    api_key_env: WEIRD_KEY\n";
    let config_yaml = profile_config_yaml("weird/name", providers_yaml, &rv_config);
    let original_env = "WEIRD_KEY=sk-should-stay-put\n";
    let profile_dir = write_profile(&profiles_root, slug, &config_yaml, Some(original_env));
    (vault_tmp, rv_config, home, profile_dir, original_env)
}

#[tokio::test]
async fn backup_is_written_before_the_vault_write() {
    let (_vault_tmp, _rv_config, home, profile_dir, original_env) = per_key_write_failure_fixture();
    let slug = "beta";

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        !output.status.success(),
        "an invalid vault leaf must make the per-key write fail; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let backups = backup_files(&profile_dir);
    assert_eq!(
        backups.len(),
        1,
        "a per-key write failure (AFTER preflight passed) must still leave the complete backup; got {backups:?}"
    );
    let backup_contents = std::fs::read_to_string(&backups[0]).unwrap();
    assert_eq!(
        backup_contents, original_env,
        "the backup must contain the FULL original file"
    );
}

#[tokio::test]
async fn failed_vault_write_leaves_the_line_untouched() {
    let (_vault_tmp, _rv_config, home, profile_dir, original_env) = per_key_write_failure_fixture();
    let slug = "beta";

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(!output.status.success());

    let new_env = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
    assert_eq!(
        new_env, original_env,
        "a failed per-key write must leave the source line byte-identical"
    );
}

#[tokio::test]
async fn sealed_vault_aborts_before_touching_anything() {
    let (_vault_tmp, rv_config) = open_sealed_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "gamma";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    let original_env = "OPENROUTER_API_KEY=sk-should-stay-put\n";
    let profile_dir = write_profile(&profiles_root, slug, &config_yaml, Some(original_env));

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        !output.status.success(),
        "a sealed vault must abort at preflight"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("sealed"),
        "expected a sealed-vault preflight error, got: {stderr}"
    );

    let backups = backup_files(&profile_dir);
    assert!(
        backups.is_empty(),
        "preflight failure must leave NO backup — distinct from a per-key write failure; got {backups:?}"
    );

    let new_env = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
    assert_eq!(
        new_env, original_env,
        "preflight failure must leave .env completely untouched"
    );
}

#[tokio::test]
async fn missing_profile_env_is_a_clean_no_op() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "delta";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    let profile_dir = write_profile(&profiles_root, slug, &config_yaml, None);

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "a profile with no .env must be a clean no-op; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        backup_files(&profile_dir).is_empty(),
        "no .env means nothing to back up"
    );
    assert!(
        !profile_dir.join(".env").exists(),
        "must not create a .env file where none existed"
    );
}

#[tokio::test]
async fn migration_writes_through_the_profile_store() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "epsilon";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=sk-epsilon-fixture\n"),
    );

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap()
        .expect("value must be written through ProfileSecretStore at the profile path");
    assert_eq!(value.expose_secret(), "sk-epsilon-fixture");
}

#[test]
fn migrate_profile_subcommand_parses_and_root_migrate_is_unchanged() {
    Command::cargo_bin("ironhermes")
        .unwrap()
        .args(["vault", "migrate-profile", "--help"])
        .assert()
        .success();

    let home = tempfile::tempdir().unwrap();
    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", home.path())
        .args(["vault", "migrate-profile", "no-such-profile", "--dry-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no-such-profile") && stderr.contains("no directory"),
        "expected a business-logic refusal naming the missing profile (proving the CLI \
         parsed successfully and ran, rather than a clap parse error), got: {stderr}"
    );

    // The EXISTING root `Migrate` variant still takes NO arguments — a clap PARSE error
    // (not a business-logic refusal) proves no `--profile`/other flag was added to it.
    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", home.path())
        .args(["vault", "migrate", "--dry-run"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "root `vault migrate` must still take no arguments"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 2 — read without substitution, scrub without reshaping, dry run
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn migration_never_expands_variable_references() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "zeta";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    let profile_dir = write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=${ROOT_SENTINEL}\n"),
    );

    let sentinel_value = "root-secret-must-never-appear-9f21ab";
    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", home.path())
        .env("ROOT_SENTINEL", sentinel_value)
        .args(["vault", "migrate-profile", slug])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains(sentinel_value),
        "stdout must never carry the sentinel's value"
    );
    assert!(
        !stderr.contains(sentinel_value),
        "stderr must never carry the sentinel's value"
    );

    let rewritten = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
    assert!(
        !rewritten.contains(sentinel_value),
        "the rewritten file must never carry the sentinel's value"
    );

    let backups = backup_files(&profile_dir);
    assert_eq!(backups.len(), 1);
    let backup_contents = std::fs::read_to_string(&backups[0]).unwrap();
    assert!(
        !backup_contents.contains(sentinel_value),
        "the backup must never carry the sentinel's value"
    );
    assert!(
        backup_contents.contains("${ROOT_SENTINEL}"),
        "the backup is the untouched original — it DOES contain the literal reference text"
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value.expose_secret(), "${ROOT_SENTINEL}");
    assert_ne!(value.expose_secret(), sentinel_value);
}

#[tokio::test]
async fn scrub_preserves_line_endings_and_trailing_newline_state() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "crlf",
            "FIRST=1\r\nOPENROUTER_API_KEY=sk-crlf\r\nLAST=2\r\n",
            "FIRST=1\r\nLAST=2\r\n",
        ),
        (
            "lf",
            "FIRST=1\nOPENROUTER_API_KEY=sk-lf\nLAST=2\n",
            "FIRST=1\nLAST=2\n",
        ),
        (
            "no-trailing-newline",
            "FIRST=1\nOPENROUTER_API_KEY=sk-notrail\nLAST=2",
            "FIRST=1\nLAST=2",
        ),
    ];
    for (name, original, expected) in cases {
        let (_vault_tmp, rv_config) = open_fresh_vault();
        let home = tempfile::tempdir().unwrap();
        let profiles_root = home.path().join("profiles");
        let slug = format!("shape-{name}");
        let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
        let profile_dir = write_profile(&profiles_root, &slug, &config_yaml, Some(original));

        let output = run_migrate_profile(home.path(), &slug, false);
        assert!(
            output.status.success(),
            "case {name}: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );

        let rewritten = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
        assert_eq!(
            rewritten, *expected,
            "case {name}: line-ending/trailing-newline shape must survive the scrub"
        );
    }
}

#[tokio::test]
async fn scrub_preserves_unrelated_lines_byte_for_byte() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "unrelated";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    let original =
        "# a comment\n\nFIRST=1\nOPENROUTER_API_KEY=sk-unrelated\nLAST=2\n# trailing comment\n";
    let profile_dir = write_profile(&profiles_root, slug, &config_yaml, Some(original));

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let rewritten = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
    assert_eq!(
        rewritten,
        "# a comment\n\nFIRST=1\nLAST=2\n# trailing comment\n"
    );
}

#[tokio::test]
async fn value_ending_in_a_backslash_does_not_merge_the_next_line() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "backslashy";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    let profile_dir = write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=sk-ends-in-backslash\\\nNEXT_LINE=untouched\n"),
    );

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value.expose_secret(), "sk-ends-in-backslash\\");

    let rewritten = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
    assert_eq!(
        rewritten, "NEXT_LINE=untouched\n",
        "the following line must survive intact, never merged into the migrated value"
    );
}

/// Phase 51 Plan 15 (T-51-87 ruling): an UNQUOTED value containing a space is
/// ambiguous — this project's own writer always strong-quotes, so an unquoted
/// value with a space is necessarily hand-edited, and the REAL running process
/// would have loaded it via `dotenvy`, which truncates an unquoted value at the
/// first space. Storing the full untruncated text would silently persist MORE
/// than the runtime ever actually used as the credential. This migration
/// REFUSES rather than guessing (the alternative, truncate-to-match-dotenvy,
/// would mean invoking a substituting parser on this path — exactly what
/// `find_raw_env_line` exists to avoid). Renamed from
/// `value_containing_spaces_is_read_whole`, which asserted the OLD behavior
/// (the full untruncated value was stored) as correct.
#[tokio::test]
async fn unquoted_value_with_a_space_is_refused_not_silently_over_stored() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "spacey";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    let original = "OPENROUTER_API_KEY=sk with spaces inside\n";
    let profile_dir = write_profile(&profiles_root, slug, &config_yaml, Some(original));

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        !output.status.success(),
        "an unquoted value containing a space must refuse rather than silently over-store"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("space"),
        "the refusal must explain why (ambiguous unquoted space); got: {stderr}"
    );

    // Nothing was written: no backup, .env unchanged, no vault entry.
    assert!(
        backup_files(&profile_dir).is_empty(),
        "a refused migration must write no backup"
    );
    let after = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
    assert_eq!(after, original, "a refused migration must leave .env byte-identical");

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap();
    assert!(value.is_none(), "a refused migration must create no vault entry");
}

/// Phase 51 Plan 15 (CR-03): before this plan, this test's OWN name and assertion pinned
/// the corrupted behavior — "quote-bearing value stored verbatim" was CR-03 itself
/// (the migration's third recorded instance of a test verifying its own assumption; see
/// `sibling `find_raw_env_line_never_trims_or_strips_quotes` in `profile_migrate.rs`,
/// which is still correct because EXTRACTION must not strip — the strip now happens one
/// layer up, in the decode step this test proves). Renamed to describe what it actually
/// checks now: a double-quoted value is DECODED (quotes removed), not stored with them.
#[tokio::test]
async fn double_quoted_value_is_decoded_not_stored_with_its_quotes() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "quoty";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=\"sk-quoted\"\n"),
    );

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        value.expose_secret(),
        "sk-quoted",
        "surrounding double-quote characters must be DECODED away, never stored"
    );
}

/// Phase 51 Plan 15 (CR-03, the acceptance criterion's own grep target): a value
/// STRONG-QUOTED the way this project's OWN writer (`quote_env_value`, via
/// `render_profile_env_with_stamp`) actually renders it round-trips through the migration
/// to the ORIGINAL plaintext — not the writer's surrounding quotes. Before this plan, the
/// suite's fixtures never once exercised this literal format
/// (`grep -c "OPENROUTER_API_KEY='" ` over this file was 0) — every fixture was hand-written
/// unquoted, which is exactly how CR-03 shipped behind a fully green suite.
#[tokio::test]
async fn writer_quoted_value_round_trips_to_the_original_plaintext() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "writerquoted";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);

    let original_plaintext = "sk-or-v1-writer-produced-9f21ab";
    // The exact byte-encoding `render_profile_env_with_stamp` produces for this value —
    // `quote_env_value` is the single shared primitive both that function and this
    // assertion call; `render_profile_env_with_stamp` itself is `pub(crate)` inside
    // `iron_hermes_ui` and unreachable from this crate's tests, so this reproduces its
    // value encoding byte for byte rather than imitating it by hand.
    let rendered_line = format!(
        "OPENROUTER_API_KEY={}\n",
        ironhermes_core::dotenv_write::quote_env_value(original_plaintext)
    );
    assert!(
        rendered_line.starts_with("OPENROUTER_API_KEY='"),
        "sanity: the writer's own encoding must be single-quoted; got: {rendered_line:?}"
    );
    write_profile(&profiles_root, slug, &config_yaml, Some(&rendered_line));

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap()
        .expect("value must exist after migration");
    assert_eq!(
        value.expose_secret(),
        original_plaintext,
        "a writer-produced, strong-quoted value must round-trip to the ORIGINAL plaintext, \
         not the writer's surrounding quotes (CR-03)"
    );
}

/// Phase 51 Plan 15 (CR-03): a value containing BOTH a quote and a backslash — the case a
/// `replace`-based decoder gets wrong (see `unquote_env_value`'s own doc) — survives the
/// real writer's quoting through migration to the original plaintext.
#[tokio::test]
async fn writer_quoted_value_with_mixed_quote_and_backslash_round_trips() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "writermixed";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);

    let original_plaintext = r"sk-mix-a'b\c'd\e";
    let rendered_line = format!(
        "OPENROUTER_API_KEY={}\n",
        ironhermes_core::dotenv_write::quote_env_value(original_plaintext)
    );
    write_profile(&profiles_root, slug, &config_yaml, Some(&rendered_line));

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap()
        .expect("value must exist after migration");
    assert_eq!(value.expose_secret(), original_plaintext);
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "dryrun";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    let original = "OPENROUTER_API_KEY=sk-dryrun\n";
    let profile_dir = write_profile(&profiles_root, slug, &config_yaml, Some(original));
    let before_mtime = std::fs::metadata(profile_dir.join(".env"))
        .unwrap()
        .modified()
        .unwrap();

    let output = run_migrate_profile(home.path(), slug, true);
    assert!(
        output.status.success(),
        "dry-run must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dry-run"),
        "expected a dry-run report naming the move, got: {stdout}"
    );
    assert!(
        stdout.contains("OPENROUTER_API_KEY"),
        "the report must name the key that would move"
    );

    assert!(
        backup_files(&profile_dir).is_empty(),
        "dry-run must write no backup"
    );
    let after = std::fs::read_to_string(profile_dir.join(".env")).unwrap();
    assert_eq!(after, original, "dry-run must leave .env byte-identical");
    let after_mtime = std::fs::metadata(profile_dir.join(".env"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        before_mtime, after_mtime,
        "dry-run must not touch the file at all"
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap();
    assert!(value.is_none(), "dry-run must create no vault entry");
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 3 — the wrote-but-could-not-scrub story
// ─────────────────────────────────────────────────────────────────────────────

/// Forces a REAL rewrite failure (not a test-only seam): the vault write succeeds, then the
/// final `.env` rewrite fails because the file itself is `0444` (read-only) while its parent
/// directory stays writable (so backup creation, a NEW file, is unaffected).
#[cfg(unix)]
#[tokio::test]
async fn rewrite_failure_reports_key_in_two_places() {
    use std::os::unix::fs::PermissionsExt as _;

    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "tworeaded";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    let original = "OPENROUTER_API_KEY=sk-two-places\n";
    let profile_dir = write_profile(&profiles_root, slug, &config_yaml, Some(original));
    let env_path = profile_dir.join(".env");
    std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o444)).unwrap();

    let output = run_migrate_profile(home.path(), slug, false);
    // Restore write permission immediately so any later cleanup of the TempDir succeeds.
    std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        !output.status.success(),
        "a rewrite failure must be reported as a command failure, not a silent success"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("TWO PLACES"),
        "must state the two-places framing, got: {stderr}"
    );
    assert!(
        stderr.contains(".env.pre-vault-"),
        "must name the backup path, got: {stderr}"
    );

    let backups = backup_files(&profile_dir);
    assert_eq!(backups.len(), 1, "the backup must still exist");
    let backup_contents = std::fs::read_to_string(&backups[0]).unwrap();
    assert_eq!(
        backup_contents, original,
        "the backup must be intact (neither copy destroyed)"
    );

    let env_after = std::fs::read_to_string(&env_path).unwrap();
    assert_eq!(
        env_after, original,
        "the original .env must be UNCHANGED when the rewrite fails — the plaintext copy \
         is still exactly where it was"
    );

    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    let value = profile_store
        .get_profile_secret_as_root(slug, "openrouter")
        .await
        .unwrap()
        .expect("the vault write itself must have SUCCEEDED before the rewrite failed");
    assert_eq!(value.expose_secret(), "sk-two-places");
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 51 UAT gap (2026-09-12): `vault list` could not verify a profile migration
// ─────────────────────────────────────────────────────────────────────────────

/// The operator-facing verification path for `migrate-profile`.
///
/// `migrate-profile` tells the operator to delete the plaintext backup "once resolution from
/// the vault is verified", and D-15 makes a NAMES-only listing the only verification available
/// (values are never printed). Before this test, no CLI surface could list
/// `secret/profiles/<slug>/<provider>`: `vault list` is rooted at `secret/providers/` BY
/// DESIGN (see `rusty_vault_store.rs`'s `list_request` doc — that root "cannot reach
/// `secret/profiles/` at any `path` argument"), and `ProfileSecretStore::list_profile_secret_names`
/// existed but was unreachable from the command line. A successful migration was therefore
/// indistinguishable from a failed one, and an operator following the tool's own instruction
/// could delete the only plaintext copy.
///
/// This locks BOTH halves: the profile listing shows the migrated leaf, AND the root listing
/// still does not — so a future change cannot "fix" the first by breaching the D-08 namespace
/// boundary the second protects.
#[test]
fn vault_list_for_profile_shows_a_migrated_secret_while_root_list_still_cannot() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = TempDir::new().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "listable";

    write_profile(
        &profiles_root,
        slug,
        &profile_config_yaml("openrouter", "", &rv_config),
        Some("OPENROUTER_API_KEY=sk-not-a-real-key-listable\n"),
    );

    let migrated = run_migrate_profile(home.path(), slug, false);
    assert!(
        migrated.status.success(),
        "migrate-profile must succeed before its listing can be verified: {}",
        String::from_utf8_lossy(&migrated.stderr)
    );

    // The claim under test: the operator can now SEE the leaf that was written.
    let listed = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", home.path())
        .args(["vault", "list", "--for-profile", slug])
        .output()
        .expect("run ironhermes vault list --for-profile");
    assert!(
        listed.status.success(),
        "vault list --for-profile must succeed: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let listed_stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed_stdout.lines().any(|line| line.trim() == "openrouter"),
        "vault list --for-profile {slug} must name the migrated leaf 'openrouter'; got: \
         {listed_stdout:?}"
    );

    // D-15: names only — the value must never reach stdout.
    assert!(
        !listed_stdout.contains("sk-not-a-real-key-listable"),
        "vault list must never print a secret VALUE; got: {listed_stdout:?}"
    );

    // The boundary stays shut: the root provider listing still cannot see profile secrets.
    let root_listed = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", home.path())
        .args(["vault", "list"])
        .output()
        .expect("run ironhermes vault list");
    assert!(root_listed.status.success());
    let root_stdout = String::from_utf8_lossy(&root_listed.stdout);
    assert!(
        !root_stdout.contains("openrouter"),
        "the root `secret/providers/` listing must NOT reach `secret/profiles/` — that \
         namespace separation is the D-08 boundary, not a bug to fix; got: {root_stdout:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 51 Plan 18 (G-51-5, Task 3): `migrate-profile` names every leaf it leaves
// unreachable, and every leaf with nowhere to install, without touching exit status.
// ─────────────────────────────────────────────────────────────────────────────

/// Directly write `leaf`/`value` into `slug`'s own vault subtree via `ProfileSecretStore` —
/// bypassing the CLI entirely, so a leaf can exist in the vault that no migration ever put
/// there (the "orphan" shape this task's report exists to surface).
async fn seed_profile_secret(rv_config: &RustyVaultConfig, slug: &str, leaf: &str, value: &str) {
    let store = RustyVaultStore::open(rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    profile_store
        .put_profile_secret(slug, leaf, SecretString::from(value.to_string()))
        .await
        .expect("seed profile secret directly");
}

/// Test 1: a profile whose vault subtree holds a leaf ("orphanprovider") that the CURRENT
/// `config.yaml` does not reference anywhere — not the main provider, not in any
/// `fallback_providers`, not a `providers`/`custom_providers` entry. The migration's own
/// output must name that leaf and say a worker for this profile can never read it, and the
/// migration itself must still succeed.
#[tokio::test]
async fn migration_names_a_leaf_unreachable_by_any_resolver_endpoint() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "orphantest";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=sk-orphantest-main\n"),
    );

    // Seeded BEFORE the migration runs — config.yaml never names "orphanprovider" anywhere,
    // so no resolver built from this config can ever hold that endpoint.
    seed_profile_secret(&rv_config, slug, "orphanprovider", "sk-orphan-secret-value").await;

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "the migration itself must still succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("orphanprovider"),
        "the report must name the unreachable leaf; stdout={stdout:?}"
    );
    assert!(
        !stdout.contains("sk-orphan-secret-value"),
        "the report must never print the secret VALUE (D-15); stdout={stdout:?}"
    );
}

/// Test 2: a profile whose vault subtree holds a leaf ("noenvprovider") that the config DOES
/// reference (a real `providers:` entry, so it IS one of the resolver's endpoints) but for
/// which no api-key env var name resolves (no `api_key_env`, not one of the three built-ins).
/// The migration's output must name that leaf and say there is nowhere to install it, and the
/// migration itself must still succeed.
#[tokio::test]
async fn migration_names_a_leaf_with_no_resolvable_env_var() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "noenvtest";
    let providers_yaml = "providers:\n  noenvprovider:\n    base_url: https://example.invalid\n";
    let config_yaml = profile_config_yaml("openrouter", providers_yaml, &rv_config);
    write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=sk-noenvtest-main\n"),
    );

    seed_profile_secret(&rv_config, slug, "noenvprovider", "sk-noenv-secret-value").await;

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "the migration itself must still succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("noenvprovider"),
        "the report must name the uninstallable leaf; stdout={stdout:?}"
    );
    assert!(
        !stdout.contains("sk-noenv-secret-value"),
        "the report must never print the secret VALUE (D-15); stdout={stdout:?}"
    );
}

/// Test 3: a profile with no unreachable and no uninstallable leaf produces NO such report
/// line — the report is a signal, not constant noise, on the common single-provider case.
#[tokio::test]
async fn clean_profile_produces_no_unreachable_leaf_report() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "cleantest";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=sk-cleantest-main\n"),
    );

    let output = run_migrate_profile(home.path(), slug, false);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.to_lowercase().contains("unreachable") && !stdout.to_lowercase().contains("no env var"),
        "a clean profile (only the migrated main provider in its vault subtree) must produce \
         no unreachable/uninstallable report line; stdout={stdout:?}"
    );
}

/// Test 5: the report also runs for a `--dry-run` invocation when the vault is reachable, so
/// an operator can see the state before committing to a destructive scrub. Seeds an orphan
/// leaf directly (no migration has run yet) and confirms `--dry-run` alone surfaces it.
#[tokio::test]
async fn dry_run_also_reports_an_unreachable_leaf_when_the_vault_is_reachable() {
    let (_vault_tmp, rv_config) = open_fresh_vault();
    let home = tempfile::tempdir().unwrap();
    let profiles_root = home.path().join("profiles");
    let slug = "dryruntest";
    let config_yaml = profile_config_yaml("openrouter", "", &rv_config);
    write_profile(
        &profiles_root,
        slug,
        &config_yaml,
        Some("OPENROUTER_API_KEY=sk-dryruntest-main\n"),
    );

    seed_profile_secret(&rv_config, slug, "dryrunorphan", "sk-dryrun-orphan-value").await;

    let output = run_migrate_profile(home.path(), slug, true);
    assert!(
        output.status.success(),
        "a dry-run must still succeed; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dry-run"),
        "must still be the dry-run message; stdout={stdout:?}"
    );
    assert!(
        stdout.contains("dryrunorphan"),
        "the dry-run report must ALSO name the unreachable leaf when the vault is reachable; \
         stdout={stdout:?}"
    );
    assert!(
        !stdout.contains("sk-dryrun-orphan-value"),
        "the report must never print the secret VALUE (D-15); stdout={stdout:?}"
    );

    // Nothing was written — dry-run's existing no-op contract is unaffected by this report.
    let store = RustyVaultStore::open(&rv_config).unwrap();
    let profile_store = ProfileSecretStore::from_rusty_vault_store(&store);
    assert!(
        profile_store
            .get_profile_secret_as_root(slug, "openrouter")
            .await
            .unwrap()
            .is_none(),
        "dry-run must not have written the main provider's secret"
    );
}
