//! Phase 46.8 Plan 05 — CLI integration tests for `ironhermes vault` subcommand.
//!
//! Gated `#![cfg(feature = "rusty-vault")]` — exercises the real RustyVault backend
//! end-to-end via the built CLI binary. Each test uses its own `tempfile::TempDir` as
//! `IRONHERMES_HOME`, passed as a per-subprocess env var (never `std::env::set_var` —
//! that races under parallel `cargo test` execution, PITFALLS §5).
//!
//! `vault set`'s masked prompt reads from `/dev/tty` on a real terminal but falls back
//! to a plain stdin line when stdin is not a TTY (`vault_cmd.rs::read_secret_value`) —
//! `assert_cmd`'s piped subprocess stdin is never a TTY, so `.write_stdin(...)` reaches
//! the handler exactly like a scripted/CI invocation would.
//!
//! Covers:
//! - `vault_audit_no_secret_values` (D-06/D-15): a `set` with a known placeholder value
//!   never leaks that value into `audit.jsonl`; the key NAME is present.
//! - `vault_list_*` (D-03): sorted, names-only output; empty vault → no output, exit 0;
//!   two prefix-sharing keys list as distinct entries.
//! - `vault_set_empty_key_rejected` (D-03 empty edge): an empty key name is rejected.
//! - `doctor_reports_vault_status_without_erroring`: `ironhermes doctor` always exits 0
//!   and reports vault enabled/backend/data-dir/sealed-state, even absent/sealed.

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

/// WR-01 (46.8-gap): a `passphrase`-mode config — `RustyVaultStore::open()`
/// deliberately leaves the store sealed in this mode (see the module-level
/// "Tiered unseal" doc in `rusty_vault_store.rs`).
fn write_rusty_vault_config_passphrase(dir: &TempDir) {
    std::fs::write(
        dir.path().join("config.yaml"),
        "vault:\n  enabled: true\n  backend: rusty-vault\n  rusty_vault:\n    unseal_mode: passphrase\n",
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

fn vault_set(dir: &TempDir, key: &str, value: &str) {
    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["vault", "set", key])
        .write_stdin(format!("{value}\n"))
        .assert()
        .success();
}

// ─────────────────────────────────────────────────────────────────────────────
// (a) D-06/D-15: audit sink never carries a secret value
// ─────────────────────────────────────────────────────────────────────────────

/// `vault set` must append exactly one audit entry carrying the key NAME + a
/// success flag — the placeholder VALUE must appear zero times in `audit.jsonl`.
#[test]
fn vault_audit_no_secret_values() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    vault_set(&dir, "openrouter", PLACEHOLDER_VALUE);

    let audit_path = dir.path().join("audit.jsonl");
    let audit_contents =
        std::fs::read_to_string(&audit_path).expect("vault set must append to audit.jsonl");

    assert!(
        !audit_contents.contains(PLACEHOLDER_VALUE),
        "audit.jsonl must never contain the secret value; got: {audit_contents}"
    );
    assert!(
        audit_contents.contains("openrouter"),
        "audit.jsonl must contain the key NAME; got: {audit_contents}"
    );
    assert!(
        audit_contents.contains("vault_set"),
        "audit.jsonl must contain the vault_set kind; got: {audit_contents}"
    );
    // `args` is a serialized-JSON *string* field (already redacted/truncated by
    // AuditEntry::new), so the embedded quotes are backslash-escaped in the raw
    // JSONL bytes — assert against that literal escaped form.
    assert!(
        audit_contents.contains(r#"\"success\":true"#),
        "audit.jsonl args must carry {{success:true}}; got: {audit_contents}"
    );

    // vault init also appends exactly one vault_init entry — two lines total.
    let line_count = audit_contents
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    assert_eq!(
        line_count, 2,
        "expected exactly 2 audit entries (vault_init + vault_set), got {line_count}: {audit_contents}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (b) vault list: sorted, names-only, empty-vault silent success
// ─────────────────────────────────────────────────────────────────────────────

/// An empty (but initialized + unsealed) vault's `list` prints nothing and exits 0 —
/// not an error (D-03 empty edge).
#[test]
fn vault_list_empty_is_silent_success() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["vault", "list"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "empty vault list must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "empty vault list must print nothing"
    );
}

/// `vault list` output is deterministically sorted and contains key NAMES only —
/// never the stored value (D-15).
#[test]
fn vault_list_names_only_sorted() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    vault_set(&dir, "zeta", PLACEHOLDER_VALUE);
    vault_set(&dir, "alpha", PLACEHOLDER_VALUE);

    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["vault", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["alpha", "zeta"],
        "vault list must be sorted, names-only; got: {stdout}"
    );
    assert!(
        !stdout.contains(PLACEHOLDER_VALUE),
        "vault list must never print a value; got: {stdout}"
    );
}

/// (c) Two keys sharing a name prefix list as distinct entries — `--prefix`
/// filtering does not merge them (D-03 adjacency).
#[test]
fn vault_list_prefix_sharing_keys_are_distinct() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    vault_set(&dir, "openrouter", PLACEHOLDER_VALUE);
    vault_set(&dir, "openrouter-backup", PLACEHOLDER_VALUE);

    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["vault", "list", "--prefix", "openrouter"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["openrouter", "openrouter-backup"],
        "prefix-sharing keys must list as distinct entries; got: {stdout}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// D-03 empty edge: empty key name rejected
// ─────────────────────────────────────────────────────────────────────────────

/// `vault set` with an empty key name is rejected with a clear message, not a
/// vault round-trip.
#[test]
fn vault_set_empty_key_rejected() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["vault", "set", ""])
        .write_stdin(format!("{PLACEHOLDER_VALUE}\n"))
        .assert()
        .failure()
        .stderr(predicates::str::contains("key name must not be empty"));
}

// ─────────────────────────────────────────────────────────────────────────────
// doctor: vault status block never errors
// ─────────────────────────────────────────────────────────────────────────────

/// `ironhermes doctor` must always exit 0 and report vault
/// enabled/backend/data-dir/sealed-state, even when the vault is absent
/// (never initialized).
#[test]
fn doctor_reports_vault_status_without_erroring() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    // No init — vault absent/not-initialized.

    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["doctor"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "doctor must always exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Vault enabled"),
        "doctor must report vault enabled; got: {stdout}"
    );
    assert!(
        stdout.contains("Vault backend"),
        "doctor must report vault backend; got: {stdout}"
    );
    assert!(
        stdout.contains("data-dir"),
        "doctor must report vault data-dir; got: {stdout}"
    );
    assert!(
        stdout.contains("sealed-state"),
        "doctor must report vault sealed-state; got: {stdout}"
    );
}

/// `ironhermes doctor` must still exit 0 (and show a sensible sealed-state) once
/// the vault has been initialized.
#[test]
fn doctor_reports_vault_status_when_initialized() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config(&dir);
    vault_init(&dir);

    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["doctor"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "doctor must always exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sealed-state: unsealed"),
        "doctor must report unsealed after init (keyfile mode auto-unseals); got: {stdout}"
    );
}

/// WR-01 (46.8-gap): in `unseal_mode = "passphrase"`, `RustyVaultStore::open()`
/// returns `Ok` while deliberately leaving the underlying `Core` sealed — before
/// the fix, `doctor` inferred "unsealed" purely from `open()`'s `Ok`/`Err`, so
/// this exact scenario produced a false-positive green "unsealed" checkmark for
/// a vault that genuinely requires `ironhermes vault unlock`. Doctor must now
/// report the ACTUAL seal state via `RustyVaultStore::is_sealed()`.
#[test]
fn doctor_reports_sealed_in_passphrase_mode_after_init() {
    let dir = TempDir::new().unwrap();
    write_rusty_vault_config_passphrase(&dir);
    vault_init(&dir);

    let output = Command::cargo_bin("ironhermes")
        .unwrap()
        .env("IRONHERMES_HOME", dir.path())
        .args(["doctor"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "doctor must always exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sealed-state: sealed"),
        "WR-01: doctor must report the ACTUAL sealed state (passphrase mode \
         leaves the store sealed after open()), not a false-positive \
         'unsealed'; got: {stdout}"
    );
}
