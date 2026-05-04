//! CLI-level end-to-end integration tests for `hermes skills` subcommands.
//!
//! Unlike `skills_cmd_test.rs` (which exercises pub impl helpers directly),
//! this file drives the compiled binary via subprocess invocations using
//! `CARGO_BIN_EXE_ironhermes` — the canonical way to exercise clap dispatch,
//! the process entry point, and the full `cmd_install` → `cmd_list` →
//! `cmd_remove` round-trip end-to-end.
//!
//! Tests:
//! - `parse_uninstall_alias_resolves_to_remove` — `--help` mentions `remove`
//! - `parse_install_skip_audit_flag` — `--help` mentions `--skip-audit`
//! - `cmd_list_reads_lock_file` — pre-seed lock; `list --format json` reads it
//! - `install_list_remove_round_trip` — full subprocess flow against wiremock
//!
//! Requires `wiremock` (workspace dev-dep) for the round-trip; uses
//! `CARGO_BIN_EXE_ironhermes` guaranteed to be set by Cargo's integration-test
//! runner.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_hermes_home<F: FnOnce(PathBuf)>(f: F) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var("HERMES_HOME").ok();
    unsafe {
        std::env::set_var("HERMES_HOME", tmp.path());
    }
    f(tmp.path().to_path_buf());
    unsafe {
        match prev {
            Some(v) => std::env::set_var("HERMES_HOME", v),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }
}

/// Path to the freshly-built `ironhermes` binary. Cargo guarantees this env
/// var is set when running integration tests in the same workspace.
fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ironhermes"))
}

fn any_file_named(root: &Path, file_name: &str) -> bool {
    if let Ok(dir) = std::fs::read_dir(root) {
        for entry in dir.flatten() {
            let path = entry.path();
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    if any_file_named(&path, file_name) {
                        return true;
                    }
                } else if ft.is_file() && entry.file_name() == file_name {
                    return true;
                }
            }
        }
    }
    false
}

// ────────────────────────────────────────────────────────────────────────────
// Test 1: `ironhermes skills --help` shows `remove` as canonical verb.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn parse_uninstall_alias_resolves_to_remove() {
    let out = std::process::Command::new(binary_path())
        .args(["skills", "--help"])
        .output()
        .expect("run ironhermes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // `remove` is the canonical verb; `uninstall` is an alias (D-04).
    assert!(
        stdout.to_lowercase().contains("remove"),
        "remove verb must appear in skills --help output: {stdout}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 2: `ironhermes skills install --help` documents `--skip-audit`.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn parse_install_skip_audit_flag() {
    let out = std::process::Command::new(binary_path())
        .args(["skills", "install", "--help"])
        .output()
        .expect("run ironhermes");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--skip-audit"),
        "--skip-audit flag must be documented in `skills install --help`: {stdout}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 3: `ironhermes skills list --format json` reads a pre-seeded lock file.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_list_reads_lock_file() {
    with_hermes_home(|home| {
        // Pre-seed skills-lock.json with one entry.
        let lock_path = home.join("skills-lock.json");
        let lock_json = r#"{"version":1,"skills":[
            {"name":"preseeded","source":"skills-sh","identifier":"pre",
             "repoPath":"skills/preseeded/SKILL.md",
             "snapshotHash":"00000000000000000000000000000000",
             "computedHash":"11111111111111111111111111111111",
             "installedAt":"2026-04-22T00:00:00Z"}
        ]}"#;
        std::fs::write(&lock_path, lock_json).unwrap();

        let out = std::process::Command::new(binary_path())
            .env("HERMES_HOME", home.as_path())
            .args(["skills", "list", "--format", "json"])
            .output()
            .expect("run ironhermes skills list");

        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("preseeded"),
            "list must read SkillLock and include preseeded entry; stdout: {stdout}"
        );
    });
}

// ────────────────────────────────────────────────────────────────────────────
// Test 4: Full CLI round-trip install → list → remove → uninstall-alias
// against a wiremock skills.sh backend (revision round 1 W5 option (a)).
// ────────────────────────────────────────────────────────────────────────────

/// Canonical SKILL.md frontmatter that the test serves on hop 2 AND places in
/// the /api/download blob response. Duplicating the small string avoids
/// pulling in the hub crate's test fixtures from the CLI crate.
const SKILL_MD: &str =
    "---\nname: ascii-art\ndescription: ASCII art skill\n---\n# ASCII Art\nBody.\n";
const HELPER_PY: &str = "print('hi')\n";

fn expected_folder_hash() -> String {
    use sha2::{Digest, Sha256};
    // Match D-13: sort by path, hash `path || content` (no separators).
    let mut files: Vec<(&str, &[u8])> = vec![
        ("SKILL.md", SKILL_MD.as_bytes()),
        ("helper.py", HELPER_PY.as_bytes()),
    ];
    files.sort_by(|a, b| a.0.cmp(b.0));
    let mut hasher = Sha256::new();
    for (p, c) in &files {
        hasher.update(p.as_bytes());
        hasher.update(c);
    }
    hex::encode(hasher.finalize())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_list_remove_round_trip() {
    // 1. Spin up wiremock for the 3-hop skills.sh pipeline.
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/repos/.+/.+/git/trees/.+$"))
        .and(query_param("recursive", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": "main-sha-abc",
            "tree": [
                {"path": "ascii-art/SKILL.md", "mode": "100644", "type": "blob",
                 "sha": "file-sha-xyz", "size": 128}
            ],
            "truncated": false
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/.+/.+/.+/ascii-art/SKILL\.md$"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SKILL_MD))
        .mount(&server)
        .await;

    let server_hash = expected_folder_hash();
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/download/.+/.+/.+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "files": [
                {"path": "SKILL.md", "contents": SKILL_MD},
                {"path": "helper.py", "contents": HELPER_PY}
            ],
            "hash": server_hash
        })))
        .mount(&server)
        .await;

    // 2. Set HERMES_HOME and point SKILLS_DOWNLOAD_URL + SKILLS_AUDIT_URL at
    // wiremock. This test bypasses ENV_LOCK because it spawns subprocesses
    // — subprocess env is copy-on-spawn and does NOT mutate the parent.
    let hermes_home = tempfile::tempdir().unwrap();
    let home_path = hermes_home.path().to_path_buf();
    let wiremock_uri = server.uri();

    let bin = binary_path();
    let run = |args: &[&str]| -> std::process::Output {
        std::process::Command::new(&bin)
            .env("HERMES_HOME", &home_path)
            // All three hops re-routed at the single wiremock origin. The
            // `http://` scheme triggers the `any_override_is_http()` relaxer
            // in SkillsShBlobSource::new — see blob.rs.
            .env("SKILLS_DOWNLOAD_URL", &wiremock_uri)
            .env("GITHUB_API_BASE", &wiremock_uri)
            .env("GITHUB_RAW_CONTENT_BASE", &wiremock_uri)
            .env("SKILLS_AUDIT_URL", &wiremock_uri)
            .args(args)
            .output()
            .expect("run ironhermes subprocess")
    };

    // 3a. Install (with --skip-audit so the subprocess does NOT need the
    // audit endpoint to be up — keeps the mock set small and deterministic).
    let install_out = run(&[
        "skills",
        "install",
        "skills-sh:foo/bar/ascii-art",
        "--skip-audit",
    ]);
    let install_stdout = String::from_utf8_lossy(&install_out.stdout);
    let install_stderr = String::from_utf8_lossy(&install_out.stderr);
    assert!(
        install_out.status.success(),
        "install failed: exit={:?} stdout={install_stdout} stderr={install_stderr}",
        install_out.status.code()
    );
    // D-21 progress shape + D-23 restart.
    assert!(
        install_stdout.contains("Resolving skills.sh/"),
        "missing D-21 line 1 (Resolving): {install_stdout}"
    );
    assert!(
        install_stdout.contains("Installed '"),
        "missing D-21 line 5 (Installed): {install_stdout}"
    );
    assert!(
        install_stdout.contains("Restart the agent"),
        "missing D-23 restart line: {install_stdout}"
    );
    // Filesystem: a SKILL.md appears under skills_root.
    assert!(
        any_file_named(&home_path.join("skills"), "SKILL.md"),
        "SKILL.md must be on disk after install"
    );
    // Lock file contains ascii-art.
    let lock_raw = std::fs::read_to_string(home_path.join("skills-lock.json"))
        .expect("skills-lock.json must exist after install");
    assert!(
        lock_raw.contains("ascii-art"),
        "lock must contain installed entry: {lock_raw}"
    );

    // 3b. List — JSON format must include the installed skill.
    let list_out = run(&["skills", "list", "--format", "json"]);
    assert!(list_out.status.success(), "list failed");
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        list_stdout.contains("ascii-art"),
        "list must report installed skill: {list_stdout}"
    );

    // 3c. Remove — canonical verb.
    let remove_out = run(&["skills", "remove", "ascii-art"]);
    assert!(remove_out.status.success(), "remove failed");
    // Filesystem: SKILL.md is gone.
    assert!(
        !any_file_named(&home_path.join("skills"), "SKILL.md"),
        "SKILL.md must be gone after remove"
    );
    // Lock file no longer contains ascii-art.
    if home_path.join("skills-lock.json").exists() {
        let after = std::fs::read_to_string(home_path.join("skills-lock.json")).unwrap();
        assert!(
            !after.contains("ascii-art"),
            "lock must not contain removed entry: {after}"
        );
    }

    // 3d. Uninstall alias — re-install, then remove via `uninstall`.
    let install2 = run(&[
        "skills",
        "install",
        "skills-sh:foo/bar/ascii-art",
        "--skip-audit",
    ]);
    assert!(
        install2.status.success(),
        "second install failed: stderr={}",
        String::from_utf8_lossy(&install2.stderr)
    );
    let uninstall_alias = run(&["skills", "uninstall", "ascii-art"]);
    assert!(
        uninstall_alias.status.success(),
        "uninstall alias must succeed (D-04); stderr={}",
        String::from_utf8_lossy(&uninstall_alias.stderr)
    );
}
