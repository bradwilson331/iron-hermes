//! Phase 46.8 Plan 06 — integration tests for `ironhermes vault migrate` (D-04).
//!
//! Gated `#![cfg(feature = "rusty-vault")]` — exercises the real RustyVault backend
//! end-to-end via the built CLI binary. Each test uses its own `tempfile::TempDir` as
//! `IRONHERMES_HOME`, passed as a per-subprocess env var (never `std::env::set_var` —
//! that races under parallel `cargo test` execution; mirrors `vault_cli.rs`, Plan 05).
//!
//! Covers:
//! - `vault_migrate_backup_and_scrub` (D-04/D-06/D-15): the primary flow — 0600
//!   timestamped backup written before any edit, the migrated key's line scrubbed
//!   from `.env`, the value round-trips through the vault, and neither the audit
//!   sink nor stdout ever carries the secret value.
//! - `vault_migrate_idempotent_second_run` (D-04 idempotency): re-running `migrate`
//!   after a successful run is a no-op for the already-scrubbed key, writes a
//!   fresh/distinct backup, and never errors.
//! - `vault_migrate_empty_env_is_noop` (D-04 empty edge): a `.env` with zero
//!   provider API keys is left byte-for-byte unchanged and `migrate` still exits 0.

#![cfg(feature = "rusty-vault")]

use assert_cmd::Command;
use tempfile::TempDir;

/// A placeholder secret value used across tests — never a real secret. Tests assert
/// this literal string is ABSENT from CLI output and the audit sink.
const PLACEHOLDER_VALUE: &str = "PLACEHOLDER_SECRET_DO_NOT_LEAK_9f3a";

fn write_rusty_vault_config(dir: &TempDir) {
    std::fs::write(
        dir.path().join("config.yaml"),
        "vault:\n  enabled: true\n  backend: rusty-vault\n",
    )
    .expect("write config.yaml");
}

fn vault_init(dir: &TempDir) {
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["vault", "init"])
        .assert()
        .success();
}

fn write_env_file(dir: &TempDir, contents: &str) {
    std::fs::write(dir.path().join(".env"), contents).expect("write .env");
}

fn read_env_file(dir: &TempDir) -> String {
    std::fs::read_to_string(dir.path().join(".env")).expect("read .env")
}

fn run_migrate(dir: &TempDir) -> std::process::Output {
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["vault", "migrate"])
        .output()
        .unwrap()
}

/// Every `.env.pre-vault-*.bak` file directly under `dir`, sorted by filename
/// (which sorts chronologically — the timestamp is the zero-padded prefix).
fn backup_files(dir: &TempDir) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
        .expect("read dir")
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

#[cfg(unix)]
fn assert_mode_0600(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode,
        0o600,
        "{} must be 0600, got {:o}",
        path.display(),
        mode
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (a) Primary flow: 0600 backup before scrub, correct scrub, value round-trips,
//     no secret value in audit sink or stdout (D-04/D-06/D-15)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn vault_migrate_backup_and_scrub() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    let original_env = format!("ANTHROPIC_API_KEY={PLACEHOLDER_VALUE}\n");
    write_env_file(&dir, &original_env);

    let output = run_migrate(&dir);
    assert!(
        output.status.success(),
        "vault migrate must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Exactly one backup, written BEFORE the scrub, mode 0600, containing the
    // FULL original .env (D-04 ordering + T-46.8-17).
    let backups = backup_files(&dir);
    assert_eq!(
        backups.len(),
        1,
        "expected exactly 1 backup after the first migrate run; got {backups:?}"
    );
    assert_mode_0600(&backups[0]);
    let backup_contents = std::fs::read_to_string(&backups[0]).expect("read backup");
    assert_eq!(
        backup_contents, original_env,
        "backup must contain the FULL original .env byte-for-byte"
    );

    // The migrated key's line is gone from .env (scrub-only-written, D-04).
    let new_env = read_env_file(&dir);
    assert!(
        !new_env.contains("ANTHROPIC_API_KEY"),
        "migrated key must be scrubbed from .env; got: {new_env:?}"
    );

    // D-15: neither stdout nor stderr ever carries the secret value — path and
    // provider-name-only reminder.
    assert!(
        !stdout.contains(PLACEHOLDER_VALUE),
        "stdout must never contain the secret value; got: {stdout}"
    );
    assert!(
        !stderr.contains(PLACEHOLDER_VALUE),
        "stderr must never contain the secret value; got: {stderr}"
    );
    assert!(
        stdout.contains(backups[0].display().to_string().as_str())
            || stdout.contains(
                backups[0]
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
            ),
        "stdout must name the backup path in the delete reminder; got: {stdout}"
    );

    // D-06/D-15: audit sink carries exactly the provider NAME + {success:true},
    // never the value — vault_init also appends one entry (2 total).
    let audit_contents = std::fs::read_to_string(dir.path().join("audit.jsonl"))
        .expect("vault migrate must append to audit.jsonl");
    assert!(
        !audit_contents.contains(PLACEHOLDER_VALUE),
        "audit.jsonl must never contain the secret value; got: {audit_contents}"
    );
    assert!(
        audit_contents.contains("vault_migrate"),
        "audit.jsonl must contain the vault_migrate kind; got: {audit_contents}"
    );
    assert!(
        audit_contents.contains("anthropic"),
        "audit.jsonl must contain the provider NAME; got: {audit_contents}"
    );
    assert!(
        audit_contents.contains(r#"\"success\":true"#),
        "audit.jsonl args must carry {{success:true}}; got: {audit_contents}"
    );

    // The value round-trips through the vault under the provider NAME (matching
    // `apply_vault_fallback`'s lookup key, Plan 04) — open the SAME on-disk vault
    // directly (in-process; no CLI surface exists to read a value back, D-09/D-15).
    use secrecy::ExposeSecret as _;
    let vault_cfg = ironhermes_vault::VaultConfig {
        enabled: true,
        backend: "rusty-vault".to_string(),
        rusty_vault: ironhermes_vault::RustyVaultConfig {
            data_dir: dir.path().join("vault"),
            unseal_mode: "keyfile".to_string(),
        },
    };
    let store = ironhermes_vault::open_store(&vault_cfg).expect("open vault store");
    let secret = store
        .get_secret("anthropic")
        .await
        .expect("get_secret must not error")
        .expect("anthropic secret must exist after migrate");
    assert_eq!(
        secret.expose_secret(),
        PLACEHOLDER_VALUE,
        "migrated value must round-trip through the vault unchanged"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (a2) WR-02 (46.8-gap): `export KEY=value` lines are recognized and migrated
// ─────────────────────────────────────────────────────────────────────────────

/// The project's own `.env` loader (`dotenvy::from_path`, `main.rs`) accepts
/// the standard `export KEY=value` syntax. `find_provider_key_lines` must
/// match it too — before the fix, `export ANTHROPIC_API_KEY=...` was silently
/// left untouched (undercounting migratable keys with no warning).
#[tokio::test]
async fn vault_migrate_export_prefixed_key_is_migrated() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    let original_env = format!("export ANTHROPIC_API_KEY={PLACEHOLDER_VALUE}\n");
    write_env_file(&dir, &original_env);

    let output = run_migrate(&dir);
    assert!(
        output.status.success(),
        "vault migrate must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let new_env = read_env_file(&dir);
    assert!(
        !new_env.contains("ANTHROPIC_API_KEY"),
        "WR-02: an `export`-prefixed provider key must be recognized and \
         scrubbed, not silently left untouched; got: {new_env:?}"
    );

    use secrecy::ExposeSecret as _;
    let vault_cfg = ironhermes_vault::VaultConfig {
        enabled: true,
        backend: "rusty-vault".to_string(),
        rusty_vault: ironhermes_vault::RustyVaultConfig {
            data_dir: dir.path().join("vault"),
            unseal_mode: "keyfile".to_string(),
        },
    };
    let store = ironhermes_vault::open_store(&vault_cfg).expect("open vault store");
    let secret = store
        .get_secret("anthropic")
        .await
        .expect("get_secret must not error")
        .expect("anthropic secret must exist after migrate");
    assert_eq!(
        secret.expose_secret(),
        PLACEHOLDER_VALUE,
        "WR-02: the export-prefixed key's value must migrate unchanged"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (a3) WR-03 + IN-01 (46.8-gap): a realistic mixed-content, CRLF `.env` —
// comments/blanks/export/quoted-value/duplicate/distractor all survive
// byte-for-byte (incl. line-ending fidelity) while ONLY provider keys scrub
// ─────────────────────────────────────────────────────────────────────────────

/// A realistic multi-line, CRLF-terminated `.env`: a comment, an
/// `export`-prefixed provider key (WR-02), a blank line, a non-provider
/// distractor (`TELEGRAM_BOT_TOKEN`, D-13 scope fence), a quoted-value
/// provider key, and a duplicate non-provider key pair. Before the WR-03 fix,
/// any successful scrub would have silently rewritten every KEPT line's CRLF
/// ending to bare LF — this test asserts the full post-migrate `.env` is
/// byte-for-byte identical to the original for every non-scrubbed line,
/// including line endings.
#[tokio::test]
async fn vault_migrate_realistic_mixed_env_preserves_crlf_and_distractors() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    let original_env = format!(
        "# personal notes\r\n\
         export ANTHROPIC_API_KEY={PLACEHOLDER_VALUE}\r\n\
         \r\n\
         TELEGRAM_BOT_TOKEN=unrelated-platform-token\r\n\
         OPENAI_API_KEY=\"quoted-openai-value\"\r\n\
         DUPLICATE_KEY=first\r\n\
         DUPLICATE_KEY=second\r\n"
    );
    write_env_file(&dir, &original_env);

    let output = run_migrate(&dir);
    assert!(
        output.status.success(),
        "vault migrate must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let new_env = read_env_file(&dir);

    let expected_env = "# personal notes\r\n\
         \r\n\
         TELEGRAM_BOT_TOKEN=unrelated-platform-token\r\n\
         DUPLICATE_KEY=first\r\n\
         DUPLICATE_KEY=second\r\n";
    assert_eq!(
        new_env, expected_env,
        "WR-02/WR-03/IN-01: only the two provider-key lines must be removed; \
         every other line (comment, blank, distractor, duplicates) must \
         survive byte-for-byte INCLUDING its original CRLF ending"
    );

    // Explicit CRLF-preservation check (a substring `contains` check alone
    // cannot distinguish CRLF from LF — assert the literal CRLF bytes).
    assert!(
        new_env.contains("TELEGRAM_BOT_TOKEN=unrelated-platform-token\r\n"),
        "WR-03: the distractor line's original CRLF ending must be preserved, \
         not silently converted to LF; got: {new_env:?}"
    );
    assert!(
        !new_env.contains("ANTHROPIC_API_KEY") && !new_env.contains("OPENAI_API_KEY"),
        "both provider key lines (incl. the export-prefixed one) must be \
         scrubbed; got: {new_env:?}"
    );

    // Both provider keys round-trip through the vault under their provider names.
    use secrecy::ExposeSecret as _;
    let vault_cfg = ironhermes_vault::VaultConfig {
        enabled: true,
        backend: "rusty-vault".to_string(),
        rusty_vault: ironhermes_vault::RustyVaultConfig {
            data_dir: dir.path().join("vault"),
            unseal_mode: "keyfile".to_string(),
        },
    };
    let store = ironhermes_vault::open_store(&vault_cfg).expect("open vault store");
    let anthropic = store
        .get_secret("anthropic")
        .await
        .expect("get_secret must not error")
        .expect("anthropic secret must exist after migrate");
    assert_eq!(anthropic.expose_secret(), PLACEHOLDER_VALUE);
    let openai = store
        .get_secret("openai")
        .await
        .expect("get_secret must not error")
        .expect("openai secret must exist after migrate");
    assert_eq!(
        openai.expose_secret(),
        "quoted-openai-value",
        "quoted value must be unquoted before storage, matching the existing \
         (untouched by this fix) quote-stripping behavior"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (b) Idempotency: second run no-ops on the scrubbed key, writes a fresh backup
//     (D-04 idempotency)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn vault_migrate_idempotent_second_run() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    write_env_file(&dir, &format!("ANTHROPIC_API_KEY={PLACEHOLDER_VALUE}\n"));

    let first = run_migrate(&dir);
    assert!(
        first.status.success(),
        "first migrate must exit 0; stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    let backups_after_first = backup_files(&dir);
    assert_eq!(backups_after_first.len(), 1);
    let env_after_first = read_env_file(&dir);
    assert!(!env_after_first.contains("ANTHROPIC_API_KEY"));

    // Second run: the key is already scrubbed — must be a no-op for it, must not
    // error, and must write a NEW, DISTINCT backup (never overwrite a prior one).
    let second = run_migrate(&dir);
    assert!(
        second.status.success(),
        "second migrate must exit 0 (idempotent); stderr={}",
        String::from_utf8_lossy(&second.stderr)
    );

    let backups_after_second = backup_files(&dir);
    assert_eq!(
        backups_after_second.len(),
        2,
        "second run must write a fresh, distinct backup; got {backups_after_second:?}"
    );
    assert_ne!(
        backups_after_first[0], backups_after_second[1],
        "the two backups must have distinct filenames (never overwrite a prior backup)"
    );
    for backup in &backups_after_second {
        assert_mode_0600(backup);
    }

    // .env is unchanged by the second run — nothing left to scrub.
    let env_after_second = read_env_file(&dir);
    assert_eq!(
        env_after_first, env_after_second,
        "second run must not touch .env further — nothing left to scrub"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (c) Empty edge: a .env with zero provider API keys is left untouched, exit 0
//     (D-04 empty)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn vault_migrate_empty_env_is_noop() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    // Deliberately no provider API keys — only an unrelated platform token, which
    // migrate must NEVER touch (D-13 scope fence).
    let original_env = "TELEGRAM_BOT_TOKEN=unrelated-platform-token\n";
    write_env_file(&dir, original_env);

    let output = run_migrate(&dir);
    assert!(
        output.status.success(),
        "migrate on a provider-key-free .env must still exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let new_env = read_env_file(&dir);
    assert_eq!(
        new_env, original_env,
        "a .env with zero provider API keys must be left byte-for-byte unchanged"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(PLACEHOLDER_VALUE),
        "stdout must never contain a secret value; got: {stdout}"
    );
}
