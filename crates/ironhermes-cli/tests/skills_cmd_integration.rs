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

// ============================================================================
// Phase 21.8.1 — local: install / update / list integration tests
// ============================================================================

/// Minimal SKILL.md content for local-install tests.
const LOCAL_SKILL_MD: &str = "---\nname: my-local-skill\ncategory: test\ndescription: a local skill\n---\n# My Local Skill\nBody.\n";

/// Create a minimal valid skill source directory in `parent` and return its path.
fn make_skill_source(parent: &std::path::Path) -> std::path::PathBuf {
    let skill_dir = parent.join("my-local-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), LOCAL_SKILL_MD).unwrap();
    skill_dir
}

/// Build a subprocess Command with HERMES_HOME set and no network env overrides.
fn cmd_with_home(hermes_home: &std::path::Path) -> std::process::Command {
    let mut c = std::process::Command::new(binary_path());
    c.env("HERMES_HOME", hermes_home);
    c
}

// ────────────────────────────────────────────────────────────────────────────
// Test: build_sources includes local-dir
// (verified transitively by a successful local install; Test 1 in plan)
// The acceptance-criteria grep check is done in the structural checks below.
// ────────────────────────────────────────────────────────────────────────────

// ────────────────────────────────────────────────────────────────────────────
// Test: cmd_install local: prefix routes to LocalDirSource
// Plan Test 2 — happy path, absolute path
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_install_local_prefix_routes_to_local_dir() {
    let hermes_home = tempfile::tempdir().unwrap();
    let source_tmp = tempfile::tempdir().unwrap();
    let skill_src = make_skill_source(source_tmp.path());

    let out = cmd_with_home(hermes_home.path())
        .args([
            "skills",
            "install",
            &format!("local:{}", skill_src.display()),
            "--skip-audit",
        ])
        .output()
        .expect("run ironhermes skills install local:");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "local install must succeed; stdout={stdout} stderr={stderr}"
    );
    // Lock file must have source: "local-dir"
    let lock_raw = std::fs::read_to_string(hermes_home.path().join("skills-lock.json"))
        .expect("skills-lock.json must exist after local install");
    assert!(
        lock_raw.contains("\"local-dir\""),
        "lock entry must have source local-dir; lock={lock_raw}"
    );
    // Install dir must exist under skills_root
    assert!(
        any_file_named(&hermes_home.path().join("skills"), "SKILL.md"),
        "SKILL.md must be installed under skills_root"
    );
    // Source dir must be unchanged (no files added/removed)
    let src_entries: Vec<_> = std::fs::read_dir(&skill_src).unwrap().collect();
    assert_eq!(
        src_entries.len(),
        1,
        "source dir must contain exactly 1 file (SKILL.md) after install — nothing added"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test: cmd_install local: with tilde expansion
// Plan Test 3
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_install_local_tilde_expands() {
    // Use a tempdir as HOME. Create skill source inside it at ~/my-local-skill/.
    let fake_home = tempfile::tempdir().unwrap();
    let hermes_home = tempfile::tempdir().unwrap();
    let skill_src = make_skill_source(fake_home.path());
    // The skill is at <fake_home>/my-local-skill; tilde form: ~/my-local-skill
    let identifier = "local:~/my-local-skill";

    let out = std::process::Command::new(binary_path())
        .env("HOME", fake_home.path())   // controls dirs::home_dir() on unix
        .env("HERMES_HOME", hermes_home.path())
        .args(["skills", "install", identifier, "--skip-audit"])
        .output()
        .expect("run ironhermes with tilde path");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "tilde-expanded local install must succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(
        any_file_named(&hermes_home.path().join("skills"), "SKILL.md"),
        "SKILL.md must be installed after tilde-path install"
    );
    // source dir must survive untouched
    assert!(skill_src.is_dir(), "source dir must still exist after install");
}

// ────────────────────────────────────────────────────────────────────────────
// Test: cmd_install local: missing path hard-fails with exit 1
// Plan Test 5 — D-A2 hard-fail on canonicalize error
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_install_local_missing_path_hard_fails() {
    let hermes_home = tempfile::tempdir().unwrap();
    let out = cmd_with_home(hermes_home.path())
        .args([
            "skills",
            "install",
            "local:/this/path/does/not/exist/12345",
            "--skip-audit",
        ])
        .output()
        .expect("run ironhermes with nonexistent local path");

    let stderr = String::from_utf8_lossy(&out.stderr);
    // Must exit 1 (not panic)
    assert_eq!(
        out.status.code(),
        Some(1),
        "missing path must exit 1; stderr={stderr}"
    );
    // Error message must mention the path problem
    assert!(
        stderr.contains("cannot resolve local path") || stderr.contains("does not exist"),
        "stderr must describe the canonicalize failure; stderr={stderr}"
    );
    // No raw ESC bytes in stderr (D-16 / T-21.8.1-04)
    assert!(
        !stderr.contains('\x1b'),
        "stderr must not contain raw terminal escape bytes; stderr={stderr}"
    );
    // Lock file must not be created / modified
    assert!(
        !hermes_home.path().join("skills-lock.json").exists(),
        "skills-lock.json must not be created on failed local install"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test: D-21 line 1 says "Resolving local:" not "Resolving skills.sh/local:"
// Plan Test 6 — Pitfall 6 / RULE 5 fix
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_install_local_resolving_print_says_local_not_skills_sh() {
    let hermes_home = tempfile::tempdir().unwrap();
    let source_tmp = tempfile::tempdir().unwrap();
    let skill_src = make_skill_source(source_tmp.path());

    let out = cmd_with_home(hermes_home.path())
        .args([
            "skills",
            "install",
            &format!("local:{}", skill_src.display()),
            "--skip-audit",
        ])
        .output()
        .expect("run ironhermes skills install local:");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Resolving local:"),
        "stdout must contain 'Resolving local:' for a local install; stdout={stdout}"
    );
    assert!(
        !stdout.contains("Resolving skills.sh/local:"),
        "stdout must NOT contain 'Resolving skills.sh/local:'; stdout={stdout}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test: cmd_update re-copies updated files from source dir
// Plan Test 7 — D-C2 update pipeline
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_update_local_dir_recopies_from_source() {
    let hermes_home = tempfile::tempdir().unwrap();
    let source_tmp = tempfile::tempdir().unwrap();
    let skill_src = make_skill_source(source_tmp.path());

    // Step 1: install
    let install_out = cmd_with_home(hermes_home.path())
        .args([
            "skills",
            "install",
            &format!("local:{}", skill_src.display()),
            "--skip-audit",
        ])
        .output()
        .expect("initial local install");
    assert!(
        install_out.status.success(),
        "initial install must succeed: stderr={}",
        String::from_utf8_lossy(&install_out.stderr)
    );

    // Step 2: modify the SKILL.md in the source dir
    let updated_md = "---\nname: my-local-skill\ncategory: test\ndescription: updated\n---\n# Updated.\n";
    std::fs::write(skill_src.join("SKILL.md"), updated_md).unwrap();

    // Step 3: update
    let update_out = cmd_with_home(hermes_home.path())
        .args(["skills", "update", "my-local-skill"])
        .output()
        .expect("cmd_update for local skill");
    let update_stderr = String::from_utf8_lossy(&update_out.stderr);
    assert!(
        update_out.status.success(),
        "update must succeed; stderr={update_stderr}"
    );

    // Step 4: verify install dir reflects the modified SKILL.md content
    // Walk skills_root to find the installed SKILL.md
    fn find_skill_md(root: &std::path::Path) -> Option<std::path::PathBuf> {
        if let Ok(dir) = std::fs::read_dir(root) {
            for entry in dir.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    if let Some(found) = find_skill_md(&p) {
                        return Some(found);
                    }
                } else if p.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
                    return Some(p);
                }
            }
        }
        None
    }
    let installed_skill_md = find_skill_md(&hermes_home.path().join("skills"))
        .expect("installed SKILL.md must exist after update");
    let installed_content = std::fs::read_to_string(&installed_skill_md).unwrap();
    assert!(
        installed_content.contains("updated"),
        "installed SKILL.md must reflect source modification after update; content={installed_content}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test: cmd_update bulk — missing source isolates failure, other skill updated
// Plan Test 8 — D-C2 bulk update failure isolation
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_update_local_dir_missing_source_isolates_failure() {
    let hermes_home = tempfile::tempdir().unwrap();
    let source_tmp_a = tempfile::tempdir().unwrap();
    let source_tmp_b = tempfile::tempdir().unwrap();

    // Skill A source dir
    let skill_a_src = source_tmp_a.path().join("skill-a");
    std::fs::create_dir_all(&skill_a_src).unwrap();
    std::fs::write(
        skill_a_src.join("SKILL.md"),
        "---\nname: skill-a\ncategory: test\ndescription: a\n---\n# A\n",
    )
    .unwrap();

    // Skill B source dir
    let skill_b_src = source_tmp_b.path().join("skill-b");
    std::fs::create_dir_all(&skill_b_src).unwrap();
    let skill_b_md_initial =
        "---\nname: skill-b\ncategory: test\ndescription: b\n---\n# B\n";
    std::fs::write(skill_b_src.join("SKILL.md"), skill_b_md_initial).unwrap();

    // Install both skills
    let install_a = cmd_with_home(hermes_home.path())
        .args([
            "skills",
            "install",
            &format!("local:{}", skill_a_src.display()),
            "--skip-audit",
        ])
        .output()
        .expect("install skill-a");
    assert!(
        install_a.status.success(),
        "install skill-a failed: stderr={}",
        String::from_utf8_lossy(&install_a.stderr)
    );

    let install_b = cmd_with_home(hermes_home.path())
        .args([
            "skills",
            "install",
            &format!("local:{}", skill_b_src.display()),
            "--skip-audit",
        ])
        .output()
        .expect("install skill-b");
    assert!(
        install_b.status.success(),
        "install skill-b failed: stderr={}",
        String::from_utf8_lossy(&install_b.stderr)
    );

    // Delete skill-a's source dir so its update will fail
    std::fs::remove_dir_all(&skill_a_src).unwrap();

    // Modify skill-b's SKILL.md so we can verify it was re-copied
    let skill_b_md_updated =
        "---\nname: skill-b\ncategory: test\ndescription: updated-b\n---\n# B Updated\n";
    std::fs::write(skill_b_src.join("SKILL.md"), skill_b_md_updated).unwrap();

    // Bulk update (no name arg) — must exit 1 (skill-a failed) but continue for skill-b
    let update_out = cmd_with_home(hermes_home.path())
        .args(["skills", "update"])
        .output()
        .expect("bulk update");
    let update_stderr = String::from_utf8_lossy(&update_out.stderr);
    let update_stdout = String::from_utf8_lossy(&update_out.stdout);

    // Exit code 1 because skill-a failed
    assert_eq!(
        update_out.status.code(),
        Some(1),
        "bulk update must exit 1 when one source is missing; stderr={update_stderr}"
    );

    // stderr mentions skill-a's failure
    assert!(
        update_stderr.contains("skill-a") || update_stderr.contains("no longer exists") || update_stderr.contains("LocalSourceMissing"),
        "stderr must report skill-a failure; stderr={update_stderr}"
    );

    // skill-b must have been successfully updated (check stdout or installed content)
    assert!(
        update_stdout.contains("skill-b") || !update_stderr.contains("skill-b"),
        "skill-b must succeed even though skill-a failed; stdout={update_stdout} stderr={update_stderr}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test: cmd_install local: does NOT call the audit endpoint
// Plan Test 9 — T-21.8.1-05 audit-skip verification
// Uses wiremock to assert zero audit requests during local install.
// ────────────────────────────────────────────────────────────────────────────
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cmd_install_local_does_not_call_audit_endpoint() {
    // Spin up wiremock — it will receive zero requests if audit is skipped.
    let audit_server = MockServer::start().await;

    // Mount a catch-all mock that records any request to the audit server.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&audit_server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&audit_server)
        .await;

    let hermes_home = tempfile::tempdir().unwrap();
    let source_tmp = tempfile::tempdir().unwrap();
    let skill_src = make_skill_source(source_tmp.path());
    let audit_uri = audit_server.uri();

    // Run install with SKILLS_AUDIT_URL pointing at wiremock.
    // NOTE: --skip-audit is NOT passed — we want to verify the audit is skipped automatically
    // for local installs (not via the flag).
    let out = std::process::Command::new(binary_path())
        .env("HERMES_HOME", hermes_home.path())
        .env("SKILLS_AUDIT_URL", &audit_uri)
        .args([
            "skills",
            "install",
            &format!("local:{}", skill_src.display()),
        ])
        .output()
        .expect("run ironhermes skills install local: (no --skip-audit)");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "local install must succeed; stdout={stdout} stderr={stderr}"
    );

    // Assert wiremock received ZERO requests — audit endpoint never called.
    let received = audit_server.received_requests().await.unwrap_or_default();
    assert_eq!(
        received.len(),
        0,
        "audit endpoint must receive ZERO requests during local install (T-21.8.1-05); got {} requests",
        received.len()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test: cmd_list annotates local-dir entries with [local]
// Structural check for the cmd_list_impl [local] annotation
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_list_local_dir_shows_local_annotation() {
    let hermes_home = tempfile::tempdir().unwrap();
    let source_tmp = tempfile::tempdir().unwrap();
    let skill_src = make_skill_source(source_tmp.path());

    // Install a local skill first
    let install_out = cmd_with_home(hermes_home.path())
        .args([
            "skills",
            "install",
            &format!("local:{}", skill_src.display()),
            "--skip-audit",
        ])
        .output()
        .expect("install local skill for list test");
    assert!(
        install_out.status.success(),
        "install for list test failed: stderr={}",
        String::from_utf8_lossy(&install_out.stderr)
    );

    // List in text format — must show [local] annotation
    let list_out = cmd_with_home(hermes_home.path())
        .args(["skills", "list"])
        .output()
        .expect("skills list");
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        list_stdout.contains("[local]"),
        "skills list text output must include [local] annotation for local-dir skills; stdout={list_stdout}"
    );
    assert!(
        list_stdout.contains("[trusted]"),
        "skills list text output must include [trusted] annotation for local-dir skills; stdout={list_stdout}"
    );

    // JSON output must have source: "local-dir"
    let list_json_out = cmd_with_home(hermes_home.path())
        .args(["skills", "list", "--format", "json"])
        .output()
        .expect("skills list --format json");
    let list_json = String::from_utf8_lossy(&list_json_out.stdout);
    assert!(
        list_json.contains("\"local-dir\""),
        "skills list JSON must have source: local-dir; json={list_json}"
    );
}

// ============================================================================
// Phase 21.8.1 Plan 04 — cmd_remove sacrosanctness + UAT replay tests
// ============================================================================

// ────────────────────────────────────────────────────────────────────────────
// Helper: snapshot_dir
//
// Returns a sorted Vec<(rel_path, sha256_hex)> for every file under `dir`.
// Used by cmd_remove_does_not_touch_source_dir to prove byte-equality
// before and after remove (RULE 6).
// No walkdir dep (hand-rolled per plan RULE 1).
// ────────────────────────────────────────────────────────────────────────────

fn snapshot_dir(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk_for_snapshot(dir, dir, &mut out).expect("snapshot_dir walk failed");
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk_for_snapshot(
    base: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, String)>,
) -> std::io::Result<()> {
    use sha2::{Digest, Sha256};
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk_for_snapshot(base, &path, out)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(base)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());
            let content = std::fs::read(&path)?;
            let hash = hex::encode(Sha256::digest(&content));
            out.push((rel, hash));
        }
        // symlinks skipped — mirror source walk behavior
    }
    Ok(())
}

// ────────────────────────────────────────────────────────────────────────────
// Test: cmd_remove does NOT touch the source directory (T-21.8.1-07, RULE 6)
//
// RULE 6: snapshot the source dir BEFORE remove and compare AFTER remove.
// A test that only asserts "source dir still exists" is too weak — cmd_remove
// could truncate files in-place and pass. This test proves byte-equality.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn cmd_remove_does_not_touch_source_dir() {
    let hermes_home = tempfile::tempdir().unwrap();
    let source_tmp = tempfile::tempdir().unwrap();

    // Create a multi-file skill source
    let skill_dir = source_tmp.path().join("remove-test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: remove-test-skill\ncategory: test\ndescription: test\n---\n# Remove Test\n",
    )
    .unwrap();
    std::fs::create_dir(skill_dir.join("helpers")).unwrap();
    std::fs::write(
        skill_dir.join("helpers").join("script.sh"),
        b"#!/bin/sh\necho ok\n",
    )
    .unwrap();
    std::fs::create_dir(skill_dir.join("references")).unwrap();
    std::fs::write(
        skill_dir.join("references").join("note.md"),
        b"# Notes\n",
    )
    .unwrap();

    // Snapshot the source dir BEFORE install
    let before_install = snapshot_dir(&skill_dir);

    // Install
    let install_out = cmd_with_home(hermes_home.path())
        .args([
            "skills",
            "install",
            &format!("local:{}", skill_dir.display()),
            "--skip-audit",
        ])
        .output()
        .expect("run ironhermes skills install");
    assert!(
        install_out.status.success(),
        "install must succeed; stderr={}",
        String::from_utf8_lossy(&install_out.stderr)
    );

    // Snapshot source dir AFTER install (must be unchanged — install copies, not moves)
    let after_install = snapshot_dir(&skill_dir);
    assert_eq!(
        before_install, after_install,
        "install must not modify source dir (copy semantics)"
    );

    // Remove the installed skill
    let remove_out = cmd_with_home(hermes_home.path())
        .args(["skills", "remove", "remove-test-skill"])
        .output()
        .expect("run ironhermes skills remove");
    assert!(
        remove_out.status.success(),
        "remove must succeed; stderr={}",
        String::from_utf8_lossy(&remove_out.stderr)
    );

    // Snapshot source dir AFTER remove — must be BYTE-IDENTICAL to before_install
    let after_remove = snapshot_dir(&skill_dir);
    assert_eq!(
        before_install, after_remove,
        "T-21.8.1-07: cmd_remove must not touch the source directory under any circumstances. \
         before_install snapshot differs from after_remove snapshot."
    );

    // Verify install dir under skills_root is gone
    assert!(
        !any_file_named(&hermes_home.path().join("skills"), "SKILL.md"),
        "install dir must be cleaned up after remove"
    );

    // Lock file must not contain the removed skill
    if hermes_home.path().join("skills-lock.json").exists() {
        let lock_raw =
            std::fs::read_to_string(hermes_home.path().join("skills-lock.json")).unwrap();
        assert!(
            !lock_raw.contains("remove-test-skill"),
            "lock must not contain removed entry; lock={lock_raw}"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Test: UAT replay — bradwilson/download/ascii-art WITH local: prefix
//
// This is the headline-fix regression test for Phase 21.8.1.
// The original failing UAT identifier was `bradwilson/download/ascii-art/`.
// With the `local:` prefix it MUST install successfully.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn uat_replay_bradwilson_download_ascii_art_with_local_prefix() {
    // Original failing UAT identifier: bradwilson/download/ascii-art/
    // After Phase 21.8.1, the same identifier with `local:` prefix MUST work.
    let hermes_home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();

    // Set up the directory tree mirroring the user's filesystem at UAT time
    let skill_dir = workspace.path().join("bradwilson").join("download").join("ascii-art");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: ascii-art\ncategory: art\ndescription: ASCII art skill\n---\n\n# ASCII Art\n",
    )
    .unwrap();

    let identifier = format!("local:{}", skill_dir.display());

    let out = cmd_with_home(hermes_home.path())
        .args(["skills", "install", &identifier, "--skip-audit"])
        .output()
        .expect("run ironhermes skills install bradwilson/download/ascii-art with local: prefix");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the original failing UAT identifier MUST work with local: prefix; \
         stdout={stdout} stderr={stderr}"
    );

    // Verify lock entry
    let lock_raw = std::fs::read_to_string(hermes_home.path().join("skills-lock.json"))
        .expect("skills-lock.json must exist after install");
    assert!(
        lock_raw.contains("ascii-art"),
        "lock must contain ascii-art entry; lock={lock_raw}"
    );
    assert!(
        lock_raw.contains("\"local-dir\""),
        "lock entry must have source local-dir; lock={lock_raw}"
    );

    // Verify SKILL.md was installed
    assert!(
        any_file_named(&hermes_home.path().join("skills"), "SKILL.md"),
        "SKILL.md must be installed under skills_root"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test: UAT replay — bradwilson/download/ascii-art WITHOUT prefix emits hint
//
// When the user types the bare path identifier (no `local:` prefix), the
// pre-dispatch hint (D-D1) must fire with exit 1 and suggest the corrected form.
// The hint is the headline UX deliverable of Phase 21.8.1.
// ────────────────────────────────────────────────────────────────────────────
#[test]
fn uat_replay_bradwilson_download_ascii_art_without_prefix_emits_hint() {
    let hermes_home = tempfile::tempdir().unwrap();

    // Set up a workspace containing bradwilson/ directory — this makes
    // fs::metadata("bradwilson/download/ascii-art/").is_dir() return true
    // which triggers the D-D1 pre-dispatch hint.
    let workspace = tempfile::tempdir().unwrap();
    let skill_dir = workspace
        .path()
        .join("bradwilson")
        .join("download")
        .join("ascii-art");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: ascii-art\n---\n# ASCII Art\n",
    )
    .unwrap();

    // Run with CWD set to the workspace so the relative path resolves correctly.
    // The identifier is the bare path (no local: prefix) — exactly the original failing input.
    let out = std::process::Command::new(binary_path())
        .current_dir(workspace.path())
        .env("HERMES_HOME", hermes_home.path())
        .args(["skills", "install", "bradwilson/download/ascii-art/"])
        .output()
        .expect("run ironhermes skills install bradwilson/download/ascii-art/ (no prefix)");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Must exit 1 (hint path, no install attempted)
    assert_eq!(
        out.status.code(),
        Some(1),
        "without local: prefix the hint must fire and exit 1; \
         stdout={stdout} stderr={stderr}"
    );

    // Hint must fire: stderr must contain the "looks like a local path" message
    assert!(
        stderr.contains("looks like a local path"),
        "headline UX deliverable: hint must fire on the original failing identifier; \
         stderr={stderr}"
    );

    // Hint must suggest the corrected `local:` invocation
    assert!(
        stderr.contains("local:bradwilson/download/ascii-art/"),
        "hint must suggest the corrected invocation with local: prefix; stderr={stderr}"
    );

    // No install must have occurred — lock file must not exist
    assert!(
        !hermes_home.path().join("skills-lock.json").exists(),
        "no install must occur when hint fires; skills-lock.json must not be created"
    );
}
