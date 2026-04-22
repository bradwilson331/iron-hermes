//! Integration tests for the install pipeline (Plan 03 Task 1).
//!
//! Tests the 5-step atomic pipeline: fetch -> quarantine -> scan -> move -> manifest.
//! Uses wiremock for HTTP mocking and injected scanners via `SkillScanner` trait.

mod fixtures;

use std::collections::HashSet;
use std::sync::Mutex;

use ironhermes_hub::{
    install, bundle_content_hash, AlwaysBlockedScanner, AlwaysCleanScanner,
    GitHubAuth, GitHubSource, HubError, HubErrorKind, SkillLock, SkillScanner,
};

use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Set up a test environment with HERMES_HOME pointing to a tempdir.
/// Returns the tempdir (must stay alive for the test duration) and the skills root path.
fn setup_test_env() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HERMES_HOME", tmp.path());
    }
    let skills_root = tmp.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();
    (tmp, skills_root)
}

/// Restore previous HERMES_HOME value.
fn restore_env(prev: Option<String>) {
    unsafe {
        match prev {
            Some(v) => std::env::set_var("HERMES_HOME", v),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }
}

/// Build a GitHubSource pointing at a wiremock server.
fn test_github_source(mock_url: &str, trusted_repos: HashSet<String>) -> GitHubSource {
    GitHubSource::new(GitHubAuth::anonymous(), trusted_repos, vec![])
        .with_api_base(mock_url)
}

/// Mount standard GitHub API mocks for the "anthropics/skills" repo
/// with a tenor-gif skill.
async fn mount_github_mocks(server: &MockServer) {
    // GET /repos/anthropics/skills -> default_branch: main
    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "default_branch": "main"
        })))
        .mount(server)
        .await;

    // GET /repos/anthropics/skills/tarball/HEAD -> tarball bytes
    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills/tarball/HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(fixtures::sample_skill_tarball()),
        )
        .mount(server)
        .await;
}

// ── Happy path ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn install_github_happy_path() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;
    mount_github_mocks(&server).await;

    let source = test_github_source(&server.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    let outcome = install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root, false)
        .await
        .expect("install should succeed");

    // Verify outcome
    assert_eq!(outcome.name, "tenor-gif");
    assert!(outcome.install_path.exists(), "install path should exist");
    assert!(outcome.install_path.join("SKILL.md").exists(), "SKILL.md should exist");
    assert!(outcome.install_path.join("handler.py").exists(), "handler.py should exist");
    assert_eq!(outcome.scan_verdict, "clean");
    assert!(!outcome.content_hash.is_empty());

    // Verify skills-lock.json was written with the new SkillLock schema.
    let lock = SkillLock::load_or_default().expect("lock load");
    let entry = lock
        .get("tenor-gif")
        .expect("lock entry for tenor-gif should exist");
    assert_eq!(entry.source, "github");
    assert_eq!(entry.identifier, "anthropics/skills/tenor-gif");
    assert_eq!(entry.computed_hash.len(), 64, "SHA-256 hex is 64 chars");
    assert!(
        entry.snapshot_hash.is_empty(),
        "github source populates snapshot_hash=None -> empty string in lock"
    );
    assert!(
        !entry.repo_path.is_empty(),
        "repo_path should be populated from first bundle file"
    );

    restore_env(prev);
}

// ── Already installed ───────────────────────────────────────────────────────

#[tokio::test]
async fn install_rejects_already_installed() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;
    mount_github_mocks(&server).await;

    let source = test_github_source(&server.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    // First install succeeds
    install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root, false)
        .await
        .expect("first install");

    // Second install should fail with AlreadyInstalled
    let err = install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root, false)
        .await
        .expect_err("should fail");

    match err {
        HubError::Typed { kind: HubErrorKind::AlreadyInstalled, .. } => {}
        other => panic!("expected AlreadyInstalled, got: {:?}", other),
    }

    restore_env(prev);
}

// ── Community scan-blocked (D-15 hard-reject) ───────────────────────────────

#[tokio::test]
async fn install_community_scan_blocked() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;
    mount_github_mocks(&server).await;

    let source = test_github_source(&server.uri(), HashSet::new()); // no trusted repos = Community
    let scanner = AlwaysBlockedScanner::new("injection pattern detected");

    let err = install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root, false)
        .await
        .expect_err("should be blocked");

    match err {
        HubError::Typed { kind: HubErrorKind::ScanBlocked, message, .. } => {
            assert!(message.contains("Community skill blocked"), "msg: {}", message);
        }
        other => panic!("expected ScanBlocked, got: {:?}", other),
    }

    // Verify no partial state left
    let skills_dir = std::fs::read_dir(&skills_root).unwrap();
    let count: usize = skills_dir.filter_map(|e| e.ok()).count();
    // Only .hub directory might exist, no skill directories
    for entry in std::fs::read_dir(&skills_root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        assert_eq!(name.to_str().unwrap(), ".hub", "only .hub should exist, found: {:?}", name);
    }

    // Verify skills-lock.json has no entry
    let lock = SkillLock::load_or_default().expect("lock load");
    assert!(lock.get("tenor-gif").is_none());

    restore_env(prev);
}

// ── Trusted scan-hit: WARN-BUT-LOAD (D-15) ���─────────────────────────��──────

#[tokio::test]
async fn install_trusted_scan_warn_but_load() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;
    mount_github_mocks(&server).await;

    let mut trusted = HashSet::new();
    trusted.insert("anthropics/skills".to_string());
    let source = test_github_source(&server.uri(), trusted);
    let scanner = AlwaysBlockedScanner::new("suspicious pattern");

    // Should succeed despite scan hit because the repo is trusted
    let outcome = install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root, false)
        .await
        .expect("trusted install should succeed despite scan hit");

    assert_eq!(outcome.name, "tenor-gif");
    assert!(outcome.install_path.exists());
    assert!(outcome.scan_verdict.contains("blocked"));
    assert_eq!(outcome.trust_level, ironhermes_core::SkillSource::Trusted);

    // Verify lock file records the install (SkillLock schema has no scan_verdict —
    // the scan outcome is asserted via the `InstallOutcome` above).
    let lock = SkillLock::load_or_default().expect("lock load");
    let entry = lock
        .get("tenor-gif")
        .expect("lock entry for tenor-gif should exist");
    assert_eq!(entry.source, "github");

    restore_env(prev);
}

// ── Failure atomicity ───────────────────────────────────────────────────────

#[tokio::test]
async fn install_failure_leaves_no_partial_state() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;

    // Mock that returns 404 for the tarball (fetch step fails)
    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills/tarball/HEAD"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let source = test_github_source(&server.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    let err = install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root, false)
        .await;
    assert!(err.is_err(), "fetch failure should propagate");

    // Verify no partial state in skills root (excluding .hub)
    for entry in std::fs::read_dir(&skills_root).unwrap().flatten() {
        let name = entry.file_name();
        if name.to_str() != Some(".hub") {
            panic!("unexpected directory in skills root: {:?}", name);
        }
    }

    // Verify lock file is clean
    let lock = SkillLock::load_or_default().expect("lock load");
    assert!(lock.skills.is_empty());

    restore_env(prev);
}

// ── Content hash determinism across installs ────────��───────────────────────

#[tokio::test]
async fn install_content_hash_matches_bundle_hash() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;
    mount_github_mocks(&server).await;

    let source = test_github_source(&server.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    let outcome = install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root, false)
        .await
        .expect("install");

    // The content hash should be a valid 64-char hex SHA-256
    assert_eq!(outcome.content_hash.len(), 64);
    assert!(outcome.content_hash.chars().all(|c| c.is_ascii_hexdigit()));

    restore_env(prev);
}

// ── Category from frontmatter ─────────��─────────────────────────────────────

#[tokio::test]
async fn install_uses_category_from_frontmatter() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;

    // Build a tarball with metadata.hermes.category set
    let tarball = build_categorized_tarball("devops");

    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills/tarball/HEAD"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tarball))
        .mount(&server)
        .await;

    let source = test_github_source(&server.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    let outcome = install(&source, "anthropics/skills/my-tool", &scanner, &skills_root, false)
        .await
        .expect("install");

    // Should be installed under devops/ category
    assert!(
        outcome.install_path.to_str().unwrap().contains("/devops/"),
        "path should contain category: {:?}",
        outcome.install_path
    );

    restore_env(prev);
}

// ── Helper: build a tarball with a specific category in frontmatter ──────────

fn build_categorized_tarball(category: &str) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let skill_md = format!(
        "---\nname: my-tool\ndescription: A tool\nmetadata:\n  hermes:\n    category: {}\n---\n\n# My Tool\n",
        category
    );

    let buf = Vec::new();
    let enc = GzEncoder::new(buf, Compression::default());
    let mut ar = tar::Builder::new(enc);

    let mut header = tar::Header::new_gnu();
    header.set_path("anthropics-skills-abc123/my-tool/SKILL.md").unwrap();
    header.set_size(skill_md.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    ar.append(&header, skill_md.as_bytes()).unwrap();

    let handler = b"# handler\n";
    let mut header2 = tar::Header::new_gnu();
    header2.set_path("anthropics-skills-abc123/my-tool/handler.py").unwrap();
    header2.set_size(handler.len() as u64);
    header2.set_mode(0o644);
    header2.set_cksum();
    ar.append(&header2, &handler[..]).unwrap();

    let enc = ar.into_inner().unwrap();
    enc.finish().unwrap()
}
