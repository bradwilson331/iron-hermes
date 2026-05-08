//! End-to-end integration tests for `LocalDirSource` — the local filesystem
//! skill install adapter (Phase 21.8.1).
//!
//! Covers every row in VALIDATION.md's Decision Coverage Matrix (D-A1..D-D1)
//! and every Failure-Mode Test row.  No HTTP — no wiremock needed for most
//! tests (one test uses wiremock to assert the audit endpoint is never called).
//!
//! Pattern source: mirrors `skills_sh_blob_adapter.rs` for ENV_LOCK / EnvGuard,
//! folder_hash_over D-13 helper, and any_file_named recursive walker.

use ironhermes_hub::{
    AlwaysBlockedScanner, AlwaysCleanScanner, HubError, HubErrorKind, LocalDirSource, SkillLock,
    compute_folder_hash,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

// ────────────────────────────────────────────────────────────────────────────
// Env guard — HERMES_HOME and any other process-global env vars mutated by
// these tests must be serialized to avoid races under `cargo test --jobs N`.
// Pattern: skills_sh_blob_adapter.rs:33-71.
// ────────────────────────────────────────────────────────────────────────────

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    restore: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn new(pairs: &[(&str, Option<&str>)]) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut restore = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            restore.push(((*k).to_string(), std::env::var(k).ok()));
            unsafe {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        Self {
            _lock: lock,
            restore,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.restore {
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// D-13 hash helper — mirrors skills_sh_blob_adapter.rs:97-109.
// Sort by rel_path, hash `path_bytes || content_bytes` (NO separators).
// ────────────────────────────────────────────────────────────────────────────

fn folder_hash_over(files: &[(&str, &[u8])]) -> String {
    let mut sorted: Vec<(String, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.replace('\\', "/"), *b))
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, content) in &sorted {
        hasher.update(rel.as_bytes());
        hasher.update(content);
    }
    hex::encode(hasher.finalize())
}

// ────────────────────────────────────────────────────────────────────────────
// Recursive walker — mirrors skills_sh_blob_adapter.rs:119-138.
// No walkdir dep (RULE 1).
// ────────────────────────────────────────────────────────────────────────────

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
// Fixture helpers
// ────────────────────────────────────────────────────────────────────────────

/// Write multiple files relative to `dir`. Creates parent directories as needed.
fn write_skill_dir(dir: &Path, files: &[(&str, &[u8])]) -> std::io::Result<()> {
    for (rel, bytes) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
    }
    Ok(())
}

/// Minimal valid SKILL.md — name: my-skill.
const VALID_SKILL_MD: &[u8] =
    b"---\nname: my-skill\nversion: 0.1.0\ncategory: test\n---\n\n# My Skill\nDoes things.\n";

/// Build a hermes_home tempdir, set HERMES_HOME, create skills_root, return both.
fn make_hermes_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let hermes_home = tempfile::tempdir().unwrap();
    let skills_root = hermes_home.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();
    (hermes_home, skills_root)
}

// ────────────────────────────────────────────────────────────────────────────
// Test 1: Tilde expansion (D-A2)
// ────────────────────────────────────────────────────────────────────────────

/// D-A2: `local:~/my-skill` form — the CLI layer expands ~ via dirs::home_dir()
/// and canonicalizes before calling LocalDirSource::fetch().
/// This test exercises fetch() with the already-expanded canonical path (the
/// CLI expansion is tested end-to-end in skills_cmd_integration.rs).
///
/// Here: verify that the canonical identifier is stored in the lock entry
/// (NOT the raw ~/my-skill form — that would break cmd_update after CWD change).
#[tokio::test]
async fn local_dir_tilde_expansion() {
    let (hermes_home, skills_root) = make_hermes_home();
    let fake_home = tempfile::tempdir().unwrap();

    // Create the skill source at <fake_home>/my-skill
    let skill_src = fake_home.path().join("my-skill");
    std::fs::create_dir_all(&skill_src).unwrap();
    write_skill_dir(&skill_src, &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    // Simulate what the CLI does: canonicalize before passing to install
    let canonical = std::fs::canonicalize(&skill_src).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);
    let scanner = AlwaysCleanScanner;
    let src = LocalDirSource;

    let outcome = ironhermes_hub::install(&src, &canonical_str, &scanner, &skills_root, false)
        .await
        .expect("local install with canonical path must succeed");

    assert_eq!(outcome.name, "my-skill");

    let lock = SkillLock::load_or_default().unwrap();
    let entry = lock.get("my-skill").expect("lock entry must exist");

    // Canonical absolute path stored — NOT raw ~/my-skill
    assert!(
        entry.identifier.starts_with('/'),
        "identifier in lock must be an absolute path: {}",
        entry.identifier
    );
    assert_eq!(
        entry.identifier, canonical_str,
        "identifier must equal the canonicalized path"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 2: Relative path (D-A2)
// ────────────────────────────────────────────────────────────────────────────

/// D-A2: relative `./skill-subdir` form — CLI resolves against CWD and
/// canonicalizes. This test passes the already-resolved canonical path to verify
/// fetch() handles it correctly (CWD mutation is serialized via ENV_LOCK).
#[tokio::test]
async fn local_dir_relative_path() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    let skill_src = source_tmp.path().join("skill-subdir");
    std::fs::create_dir_all(&skill_src).unwrap();
    write_skill_dir(&skill_src, &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    let canonical = std::fs::canonicalize(&skill_src).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);
    let scanner = AlwaysCleanScanner;

    let outcome = ironhermes_hub::install(&LocalDirSource, &canonical_str, &scanner, &skills_root, false)
        .await
        .expect("relative-path install must succeed after CLI-side canonicalization");

    assert_eq!(outcome.name, "my-skill");
    assert!(outcome.install_path.exists());
}

// ────────────────────────────────────────────────────────────────────────────
// Test 3: Missing path — LocalSourceMissing (D-A2, RULE 5)
// ────────────────────────────────────────────────────────────────────────────

/// D-A2: non-existent path → hard-fail with HubErrorKind::LocalSourceMissing.
/// RULE 5: assert specific variant, not bare .is_err().
#[tokio::test]
async fn local_dir_missing_path_hard_fails() {
    let (hermes_home, skills_root) = make_hermes_home();
    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let err = ironhermes_hub::install(
        &LocalDirSource,
        "/nonexistent/path/12345/does/not/exist",
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect_err("missing path must fail");

    match err {
        HubError::Typed {
            kind: HubErrorKind::LocalSourceMissing,
            ..
        } => {} // correct
        other => panic!(
            "expected HubErrorKind::LocalSourceMissing, got {:?}",
            other
        ),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Test 4: All subfiles recursively copied (D-B1)
// ────────────────────────────────────────────────────────────────────────────

/// D-B1: install copies all files recursively — not just SKILL.md.
/// Source has SKILL.md + helpers/script.sh + references/note.md + assets/img.png.
/// After install, all 4 files must exist and content must match byte-for-byte.
#[tokio::test]
async fn local_dir_copies_all_subfiles() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();

    let files_to_copy: &[(&str, &[u8])] = &[
        ("SKILL.md", VALID_SKILL_MD),
        ("helpers/script.sh", b"#!/bin/sh\necho ok\n"),
        ("references/note.md", b"# Notes\nSome notes here.\n"),
        ("assets/img.png", b"\x89PNG\r\n\x1a\n"),
    ];
    write_skill_dir(source_tmp.path(), files_to_copy).unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical_str,
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("all-subfiles install must succeed");

    // All 4 files must be in the install dir
    for (rel, expected_content) in files_to_copy {
        let installed_path = outcome.install_path.join(rel);
        assert!(
            installed_path.exists(),
            "installed file must exist: {rel}; install_path={}",
            outcome.install_path.display()
        );
        let installed_content = std::fs::read(&installed_path).unwrap();
        assert_eq!(
            installed_content, *expected_content,
            "installed content must match source byte-for-byte: {rel}"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Test 5: WARN-BUT-LOAD on scan hit (D-B2 + D-15, RULE 5)
// ────────────────────────────────────────────────────────────────────────────

/// D-B2: local installs are Trusted tier → WARN-BUT-LOAD when scanner blocks.
/// RULE 5 aliasing-risk: assert the verdict summary contains "blocked" (proving
/// the scan actually ran), NOT just that install returned Ok.
#[tokio::test]
async fn local_dir_scan_hit_warns_but_loads() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    write_skill_dir(source_tmp.path(), &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    // AlwaysBlockedScanner returns "[BLOCKED: reason]" verdict for every file
    let scanner = AlwaysBlockedScanner::new("test-threat-pattern");

    let outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical_str,
        &scanner,
        &skills_root,
        false,
    )
    .await
    .expect("WARN-BUT-LOAD: Trusted tier install MUST succeed even on scan hit");

    // RULE 5: prove the scan actually ran by asserting the verdict string
    assert!(
        outcome.scan_verdict.contains("blocked"),
        "scan_verdict must contain 'blocked' proving the scanner ran; got: {:?}",
        outcome.scan_verdict
    );

    // Lock entry must still be written (WARN-BUT-LOAD = install proceeds)
    let lock = SkillLock::load_or_default().unwrap();
    assert!(
        lock.get("my-skill").is_some(),
        "WARN-BUT-LOAD: lock entry must be written despite scan hit"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 6: Audit endpoint NOT contacted for local installs (T-21.8.1-05)
// ────────────────────────────────────────────────────────────────────────────

/// T-21.8.1-05: calling the audit endpoint with a local path would leak the path
/// to a third party. Verify via wiremock expectation count = 0.
#[tokio::test]
async fn local_dir_audit_skipped() {
    // Spin up a wiremock server. Any request to it = audit was called = test fails.
    let mock_server = MockServer::start().await;
    let audit_url = format!("{}/audit", mock_server.uri());

    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    write_skill_dir(source_tmp.path(), &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();

    let _env = EnvGuard::new(&[
        ("HERMES_HOME", Some(hermes_home.path().to_str().unwrap())),
        ("SKILLS_AUDIT_URL", Some(&audit_url)),
    ]);

    // Mount a catch-all mock — if it fires, the test will fail on `received_len`.
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&mock_server)
        .await;

    let _outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical_str,
        &AlwaysCleanScanner,
        &skills_root,
        false, // skip_audit=false — must still skip automatically for local installs
    )
    .await
    .expect("local install must succeed");

    // Assert audit server received zero requests
    let received = mock_server.received_requests().await.unwrap_or_default();
    assert_eq!(
        received.len(),
        0,
        "audit endpoint must NOT be contacted for local installs (T-21.8.1-05); got {} requests",
        received.len()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 7: Lock entry identifier is canonical (D-C1)
// ────────────────────────────────────────────────────────────────────────────

/// D-C1: lock entry `identifier` must be the FULLY-RESOLVED canonical path.
/// On macOS, /tmp resolves to /private/tmp — this test exercises that landmine
/// by using whatever tempfile gives us and asserting the stored value equals
/// `std::fs::canonicalize()` of the source path.
#[tokio::test]
async fn local_dir_lock_entry_identifier_is_canonical() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    write_skill_dir(source_tmp.path(), &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let _outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical_str,
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("install must succeed");

    let lock = SkillLock::load_or_default().unwrap();
    let entry = lock.get("my-skill").expect("lock entry must exist");

    assert_eq!(
        entry.identifier, canonical_str,
        "D-C1: identifier must be the canonicalized absolute path (macOS /tmp → /private/tmp)"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 8: snapshotHash is empty string (D-C1)
// ────────────────────────────────────────────────────────────────────────────

/// D-C1: `snapshotHash` must be `""` for local installs — there is no remote
/// snapshot to mirror.
#[tokio::test]
async fn local_dir_snapshot_hash_is_empty() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    write_skill_dir(source_tmp.path(), &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let _outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical.to_string_lossy(),
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("install must succeed");

    let lock = SkillLock::load_or_default().unwrap();
    let entry = lock.get("my-skill").expect("lock entry must exist");

    assert_eq!(
        entry.snapshot_hash, "",
        "D-C1: snapshotHash must be empty string for local installs (no remote snapshot)"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 9: computedHash matches compute_folder_hash (D-13)
// ────────────────────────────────────────────────────────────────────────────

/// D-13: after install, `SkillLockEntry.computed_hash` must equal
/// `compute_folder_hash(&outcome.install_path)` byte-for-byte.
/// Also cross-checks against our in-test D-13 algorithm to prove the hash
/// is deterministic and content-driven.
#[tokio::test]
async fn local_dir_computed_hash_matches_folder() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    let files: &[(&str, &[u8])] = &[
        ("SKILL.md", VALID_SKILL_MD),
        ("helpers/script.sh", b"#!/bin/sh\necho hello\n"),
    ];
    write_skill_dir(source_tmp.path(), files).unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical.to_string_lossy(),
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("install must succeed");

    let lock = SkillLock::load_or_default().unwrap();
    let entry = lock.get("my-skill").expect("lock entry must exist");

    // Primary assertion: lock hash == on-disk hash (D-13 invariant)
    let on_disk = compute_folder_hash(&outcome.install_path)
        .expect("compute_folder_hash must succeed on install path");
    assert_eq!(
        on_disk, entry.computed_hash,
        "D-13 invariant: lock computed_hash must equal compute_folder_hash(install_path)"
    );

    // Cross-check: our in-test helper produces the same value
    let expected = folder_hash_over(files);
    assert_eq!(
        expected, entry.computed_hash,
        "D-13 cross-check: computed_hash must match test-local folder_hash_over()"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 10: Update re-copies all files (D-C2)
// ────────────────────────────────────────────────────────────────────────────

/// D-C2: `hermes skills update <name>` for a local-dir skill re-runs the full
/// pipeline against the source path stored in `identifier`.
/// After source modification, the installed copy must reflect the changes.
/// A newly-added file must appear after update.
#[tokio::test]
async fn local_dir_update_recopies_all_files() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();

    // Initial install: SKILL.md + helpers/script.sh
    write_skill_dir(
        source_tmp.path(),
        &[
            ("SKILL.md", VALID_SKILL_MD),
            ("helpers/script.sh", b"#!/bin/sh\necho v1\n"),
        ],
    )
    .unwrap();
    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical_str,
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("initial install must succeed");

    // Modify helpers/script.sh in source
    std::fs::write(
        source_tmp.path().join("helpers").join("script.sh"),
        b"#!/bin/sh\necho v2-modified\n",
    )
    .unwrap();

    // Add a new file
    std::fs::create_dir_all(source_tmp.path().join("references")).unwrap();
    std::fs::write(
        source_tmp.path().join("references").join("added.md"),
        b"# Added\n",
    )
    .unwrap();

    // Run update — update() looks up the identifier from the lock file using skill_name
    let update_outcome = ironhermes_hub::update(
        &LocalDirSource,
        &outcome.name,
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("update must succeed");

    // Verify modified script.sh content reflects v2
    let script = std::fs::read(update_outcome.install_path.join("helpers").join("script.sh"))
        .expect("helpers/script.sh must exist after update");
    assert!(
        std::str::from_utf8(&script)
            .unwrap()
            .contains("v2-modified"),
        "updated script must contain v2-modified content"
    );

    // Verify new references/added.md was copied
    assert!(
        update_outcome
            .install_path
            .join("references")
            .join("added.md")
            .exists(),
        "newly-added file must appear in install dir after update"
    );

    // Hash must have changed (different content)
    assert_ne!(
        update_outcome.old_hash, update_outcome.new_hash,
        "computed hash must change after source modification"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 11: Update missing source → LocalSourceMissing (D-C2, RULE 5)
// ────────────────────────────────────────────────────────────────────────────

/// D-C2: if the source directory no longer exists at update time, hard-fail
/// with HubErrorKind::LocalSourceMissing.
/// RULE 5: assert specific variant.
#[tokio::test]
async fn local_dir_update_missing_source_hard_fails() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    write_skill_dir(source_tmp.path(), &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let canonical_str = canonical.to_string_lossy().into_owned();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    // Install first
    ironhermes_hub::install(
        &LocalDirSource,
        &canonical_str,
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("initial install must succeed");

    // Delete the source dir
    std::fs::remove_dir_all(source_tmp.path()).unwrap();

    // Update must fail with LocalSourceMissing
    // update() uses the identifier stored in the lock file (loaded by skill_name)
    let err = ironhermes_hub::update(
        &LocalDirSource,
        "my-skill",
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect_err("update must fail when source dir is gone");

    match err {
        HubError::Typed {
            kind: HubErrorKind::LocalSourceMissing,
            ..
        } => {} // correct
        other => panic!(
            "expected HubErrorKind::LocalSourceMissing, got {:?}",
            other
        ),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Test 12: Symlink skipped in walk (T-21.8.1-02) — unix only
// ────────────────────────────────────────────────────────────────────────────

/// T-21.8.1-02: symlinks inside the source dir must be skipped (not followed).
/// A symlink to /etc/passwd must not appear in the install dir.
#[cfg(unix)]
#[tokio::test]
async fn local_dir_symlink_skipped_in_walk() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    write_skill_dir(source_tmp.path(), &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    // Create a symlink inside the source dir pointing to /etc/passwd
    std::os::unix::fs::symlink(
        "/etc/passwd",
        source_tmp.path().join("outside_link"),
    )
    .unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    // Install must succeed (symlink skipped, not an error)
    let outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical.to_string_lossy(),
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("install must succeed with symlink present (symlink is skipped)");

    // The symlink target (outside_link) must NOT exist in install dir
    assert!(
        !outcome.install_path.join("outside_link").exists(),
        "symlink must not be installed (T-21.8.1-02)"
    );

    // passwd file must not appear
    assert!(
        !any_file_named(&outcome.install_path, "passwd"),
        "symlink must not be followed — passwd must not appear under install_path"
    );

    // SKILL.md must still be installed
    assert!(
        outcome.install_path.join("SKILL.md").exists(),
        "SKILL.md must be installed even when symlinks are present"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 13: Source path is a file, not a dir (failure mode)
// ────────────────────────────────────────────────────────────────────────────

/// Source path points to a file, not a directory. LocalDirSource::fetch() checks
/// `!base.is_dir()` and returns LocalSourceMissing.
#[tokio::test]
async fn local_dir_file_not_dir_hard_fails() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();

    // Create a file (not a dir) at the source path
    let file_path = source_tmp.path().join("skill.md");
    std::fs::write(&file_path, VALID_SKILL_MD).unwrap();
    let canonical = std::fs::canonicalize(&file_path).unwrap();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let err = ironhermes_hub::install(
        &LocalDirSource,
        &canonical.to_string_lossy(),
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect_err("file-path (not dir) must fail");

    // The adapter rejects it with LocalSourceMissing (same code path as "missing")
    match err {
        HubError::Typed {
            kind: HubErrorKind::LocalSourceMissing,
            ..
        } => {} // correct — "not a directory" maps to LocalSourceMissing
        other => panic!(
            "expected HubErrorKind::LocalSourceMissing for file-not-dir, got {:?}",
            other
        ),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Test 14: No SKILL.md → Parse error (failure mode)
// ────────────────────────────────────────────────────────────────────────────

/// Source dir has no SKILL.md. Must hard-fail with HubErrorKind::Parse
/// and message containing "no SKILL.md found".
#[tokio::test]
async fn local_dir_no_skill_md_hard_fails() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();

    // Write a non-SKILL.md file only
    std::fs::write(source_tmp.path().join("README.md"), b"# Not a skill\n").unwrap();
    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let err = ironhermes_hub::install(
        &LocalDirSource,
        &canonical.to_string_lossy(),
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect_err("no-SKILL.md must fail");

    match err {
        HubError::Typed {
            kind: HubErrorKind::Parse,
            ref message,
            ..
        } => {
            assert!(
                message.contains("no SKILL.md found") || message.contains("SKILL.md"),
                "Parse error message must mention SKILL.md; got: {message}"
            );
        }
        other => panic!("expected HubErrorKind::Parse, got {:?}", other),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Test 15: JS frontmatter rejected (D-17)
// ────────────────────────────────────────────────────────────────────────────

/// D-17: `---js` frontmatter delimiter must be rejected by strict_yaml_delimiter.
/// Results in a HubErrorKind::Parse error.
#[tokio::test]
async fn local_dir_js_frontmatter_rejected() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();

    // SKILL.md with `---js` frontmatter delimiter (D-17 violation)
    std::fs::write(
        source_tmp.path().join("SKILL.md"),
        b"---js\nname: evil-skill\n---\n# Evil\n",
    )
    .unwrap();
    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();

    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let err = ironhermes_hub::install(
        &LocalDirSource,
        &canonical.to_string_lossy(),
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect_err("---js frontmatter must be rejected (D-17)");

    match err {
        HubError::Typed {
            kind: HubErrorKind::Parse,
            ..
        } => {} // correct — strict_yaml_delimiter returns Parse error
        other => panic!(
            "expected HubErrorKind::Parse from strict_yaml_delimiter, got {:?}",
            other
        ),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Test 16: Path traversal via symlink skipped (T-21.8.1-01) — unix only
// ────────────────────────────────────────────────────────────────────────────

/// T-21.8.1-01: a symlink inside the source dir pointing at ../../etc must not
/// be followed. The install must succeed (symlink is silently skipped), and no
/// traversed path must appear under install_path.
#[cfg(unix)]
#[tokio::test]
async fn local_dir_traversal_via_symlink_skipped() {
    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    write_skill_dir(source_tmp.path(), &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    // Symlink pointing at /etc (traversal attempt)
    std::os::unix::fs::symlink("/etc", source_tmp.path().join("evil")).unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    // Install must succeed — symlink is skipped, not an error
    let outcome = ironhermes_hub::install(
        &LocalDirSource,
        &canonical.to_string_lossy(),
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect("install must succeed; symlink must be skipped");

    // install_path must NOT contain anything under evil/ or any traversed path
    assert!(
        !outcome.install_path.join("evil").exists(),
        "evil symlink must not appear in install dir"
    );
    assert!(
        !any_file_named(&outcome.install_path, "passwd"),
        "traversal via symlink must not reach /etc/passwd"
    );
    // SKILL.md must be present
    assert!(
        outcome.install_path.join("SKILL.md").exists(),
        "SKILL.md must be installed; only the symlink is skipped"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Test 17: Permission denied on source dir (unix only, skip if root)
// ────────────────────────────────────────────────────────────────────────────

/// Failure mode: source dir with mode 000 (no read perms for owner).
/// Expected: error containing some I/O failure (HubErrorKind::Io).
/// Gated: unix only, skipped if running as root (root can read mode-000 dirs).
#[cfg(unix)]
#[tokio::test]
async fn local_dir_perms_denied() {
    // Skip if running as root — chmod 000 doesn't deny root access.
    // Use `id -u` to check UID without requiring the libc crate.
    let uid_output = std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1000);
    if uid_output == 0 {
        eprintln!("Skipping local_dir_perms_denied: running as root");
        return;
    }

    let (hermes_home, skills_root) = make_hermes_home();
    let source_tmp = tempfile::tempdir().unwrap();
    write_skill_dir(source_tmp.path(), &[("SKILL.md", VALID_SKILL_MD)]).unwrap();

    // Chmod source dir to 000 — no read/execute permissions
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        source_tmp.path(),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();

    let canonical = std::fs::canonicalize(source_tmp.path()).unwrap();
    let _env = EnvGuard::new(&[("HERMES_HOME", Some(hermes_home.path().to_str().unwrap()))]);

    let err = ironhermes_hub::install(
        &LocalDirSource,
        &canonical.to_string_lossy(),
        &AlwaysCleanScanner,
        &skills_root,
        false,
    )
    .await
    .expect_err("mode-000 dir must fail with I/O error");

    // Restore permissions so tempdir cleanup can succeed
    std::fs::set_permissions(
        source_tmp.path(),
        std::fs::Permissions::from_mode(0o755),
    )
    .ok();

    // Any I/O-related error is acceptable here
    match err {
        HubError::Typed {
            kind: HubErrorKind::Io,
            ..
        } => {} // expected
        HubError::Typed {
            kind: HubErrorKind::LocalSourceMissing,
            ..
        } => {} // also acceptable — is_dir() may return false on no-execute dir
        other => panic!("expected Io or LocalSourceMissing, got {:?}", other),
    }
}
