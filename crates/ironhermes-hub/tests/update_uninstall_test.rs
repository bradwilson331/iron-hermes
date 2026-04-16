//! Integration tests for update and uninstall (Plan 03 Task 2).
//!
//! Tests:
//! - update() with hash-drift detection (re-fetch + compare)
//! - update() no-op when content hash matches
//! - update() re-scans and rejects if community + scan-hit
//! - uninstall() removes directory + manifest entry atomically
//! - uninstall() cleans up empty parent category directory
//! - uninstall() errors for unknown skill

mod fixtures;

use std::collections::HashSet;
use std::sync::Mutex;

use ironhermes_hub::{
    install, uninstall, update, AlwaysBlockedScanner, AlwaysCleanScanner, GitHubAuth,
    GitHubSource, HubError, HubErrorKind, HubManifest,
};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup_test_env() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HERMES_HOME", tmp.path());
    }
    let skills_root = tmp.path().join("skills");
    std::fs::create_dir_all(&skills_root).unwrap();
    (tmp, skills_root)
}

fn restore_env(prev: Option<String>) {
    unsafe {
        match prev {
            Some(v) => std::env::set_var("HERMES_HOME", v),
            None => std::env::remove_var("HERMES_HOME"),
        }
    }
}

fn test_github_source(mock_url: &str, trusted_repos: HashSet<String>) -> GitHubSource {
    GitHubSource::new(GitHubAuth::anonymous(), trusted_repos, vec![]).with_api_base(mock_url)
}

async fn mount_github_mocks(server: &MockServer, tarball_bytes: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"default_branch": "main"})),
        )
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills/tarball/HEAD"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(tarball_bytes))
        .mount(server)
        .await;
}

/// Build a tarball with customisable SKILL.md body so we can produce different
/// content hashes on successive fetches.
fn build_skill_tarball(extra_content: &str) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let skill_md = format!(
        "---\nname: tenor-gif\ndescription: Tenor GIF search\nversion: 1.0.0\n---\n\n# Tenor GIF\n\n{}\n",
        extra_content
    );

    let buf = Vec::new();
    let enc = GzEncoder::new(buf, Compression::default());
    let mut ar = tar::Builder::new(enc);

    let mut header = tar::Header::new_gnu();
    header
        .set_path("anthropics-skills-abc123/tenor-gif/SKILL.md")
        .unwrap();
    header.set_size(skill_md.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    ar.append(&header, skill_md.as_bytes()).unwrap();

    let handler = b"# handler stub\n";
    let mut h2 = tar::Header::new_gnu();
    h2.set_path("anthropics-skills-abc123/tenor-gif/handler.py")
        .unwrap();
    h2.set_size(handler.len() as u64);
    h2.set_mode(0o644);
    h2.set_cksum();
    ar.append(&h2, &handler[..]).unwrap();

    let enc = ar.into_inner().unwrap();
    enc.finish().unwrap()
}

// ── Update: hash-drift detected ─────────────────────────────────────────────

#[tokio::test]
async fn update_detects_hash_drift_and_replaces() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    // Install v1
    let server1 = MockServer::start().await;
    mount_github_mocks(&server1, build_skill_tarball("v1 content")).await;
    let source1 = test_github_source(&server1.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    let outcome1 = install(
        &source1,
        "anthropics/skills/tenor-gif",
        &scanner,
        &skills_root,
    )
    .await
    .expect("install v1");
    let old_hash = outcome1.content_hash.clone();

    // Update with v2 (different content -> different hash)
    let server2 = MockServer::start().await;
    mount_github_mocks(&server2, build_skill_tarball("v2 updated content")).await;
    let source2 = test_github_source(&server2.uri(), HashSet::new());

    let update_outcome = update(
        &source2,
        "tenor-gif",
        &scanner,
        &skills_root,
    )
    .await
    .expect("update should succeed");

    assert_eq!(update_outcome.name, "tenor-gif");
    assert_eq!(update_outcome.old_hash, old_hash);
    assert_ne!(update_outcome.old_hash, update_outcome.new_hash, "hashes should differ");
    assert_eq!(update_outcome.scan_verdict, "clean");

    // Verify manifest was updated
    let manifest = HubManifest::load_or_default().expect("manifest");
    let entry = &manifest.installed["tenor-gif"];
    assert_eq!(entry.content_hash, update_outcome.new_hash);
    assert!(entry.updated_at.is_some(), "updated_at should be set");

    // Verify files exist
    assert!(update_outcome.install_path.join("SKILL.md").exists());

    // Verify SKILL.md contains the updated content
    let skill_content =
        std::fs::read_to_string(update_outcome.install_path.join("SKILL.md")).unwrap();
    assert!(skill_content.contains("v2 updated content"));

    restore_env(prev);
}

// ── Update: no drift (already up to date) ───────────────────────────────────

#[tokio::test]
async fn update_noop_when_hash_matches() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let tarball = build_skill_tarball("same content");

    // Install
    let server1 = MockServer::start().await;
    mount_github_mocks(&server1, tarball.clone()).await;
    let source1 = test_github_source(&server1.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    install(&source1, "anthropics/skills/tenor-gif", &scanner, &skills_root)
        .await
        .expect("install");

    // Update with same content
    let server2 = MockServer::start().await;
    mount_github_mocks(&server2, tarball).await;
    let source2 = test_github_source(&server2.uri(), HashSet::new());

    let err = update(&source2, "tenor-gif", &scanner, &skills_root)
        .await
        .expect_err("should report already up to date");

    match err {
        HubError::Typed {
            kind: HubErrorKind::AlreadyInstalled,
            message,
            ..
        } => {
            assert!(message.contains("already up to date"), "msg: {}", message);
        }
        other => panic!("expected AlreadyInstalled, got: {:?}", other),
    }

    restore_env(prev);
}

// ── Update: not installed ──────────────────────────────────────���────────────

#[tokio::test]
async fn update_errors_for_unknown_skill() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;
    let source = test_github_source(&server.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    let err = update(&source, "nonexistent-skill", &scanner, &skills_root)
        .await
        .expect_err("should fail");

    match err {
        HubError::Typed {
            kind: HubErrorKind::NotFound,
            ..
        } => {}
        other => panic!("expected NotFound, got: {:?}", other),
    }

    restore_env(prev);
}

// ── Update: community scan-blocked on new version ───────────────────────────

#[tokio::test]
async fn update_rejects_community_scan_blocked() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    // Install v1 with clean scanner
    let server1 = MockServer::start().await;
    mount_github_mocks(&server1, build_skill_tarball("v1 clean")).await;
    let source1 = test_github_source(&server1.uri(), HashSet::new());
    let clean_scanner = AlwaysCleanScanner;

    install(
        &source1,
        "anthropics/skills/tenor-gif",
        &clean_scanner,
        &skills_root,
    )
    .await
    .expect("install v1");

    // Update v2 with blocked scanner (simulates new version having injection)
    let server2 = MockServer::start().await;
    mount_github_mocks(&server2, build_skill_tarball("v2 with injection")).await;
    let source2 = test_github_source(&server2.uri(), HashSet::new());
    let blocked_scanner = AlwaysBlockedScanner::new("injection detected");

    let err = update(
        &source2,
        "tenor-gif",
        &blocked_scanner,
        &skills_root,
    )
    .await
    .expect_err("should be blocked");

    match err {
        HubError::Typed {
            kind: HubErrorKind::ScanBlocked,
            ..
        } => {}
        other => panic!("expected ScanBlocked, got: {:?}", other),
    }

    // Verify the old version is still intact (atomic: scan failure = no replace)
    let manifest = HubManifest::load_or_default().expect("manifest");
    let entry = &manifest.installed["tenor-gif"];
    assert_eq!(entry.scan_verdict, "clean", "old scan verdict preserved");

    restore_env(prev);
}

// ── Uninstall: happy path ───────────────────────────────────────────────────

#[tokio::test]
async fn uninstall_removes_dir_and_manifest() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    // Install first
    let server = MockServer::start().await;
    mount_github_mocks(&server, fixtures::sample_skill_tarball()).await;
    let source = test_github_source(&server.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    let outcome = install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root)
        .await
        .expect("install");
    assert!(outcome.install_path.exists());

    // Uninstall
    let un_outcome = uninstall("tenor-gif").expect("uninstall should succeed");
    assert_eq!(un_outcome.name, "tenor-gif");
    assert!(!un_outcome.removed_path.exists(), "directory should be removed");

    // Verify manifest is clean
    let manifest = HubManifest::load_or_default().expect("manifest");
    assert!(!manifest.installed.contains_key("tenor-gif"));

    restore_env(prev);
}

// ── Uninstall: unknown skill ──────────────────────────────────────���─────────

#[tokio::test]
async fn uninstall_errors_for_unknown_skill() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, _skills_root) = setup_test_env();

    let err = uninstall("nonexistent").expect_err("should fail");
    match err {
        HubError::Typed {
            kind: HubErrorKind::NotFound,
            ..
        } => {}
        other => panic!("expected NotFound, got: {:?}", other),
    }

    restore_env(prev);
}

// ── Uninstall: cleans up empty category dir ─────────────────────────────────

#[tokio::test]
async fn uninstall_cleans_empty_parent_category() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    let server = MockServer::start().await;
    mount_github_mocks(&server, fixtures::sample_skill_tarball()).await;
    let source = test_github_source(&server.uri(), HashSet::new());
    let scanner = AlwaysCleanScanner;

    let outcome = install(&source, "anthropics/skills/tenor-gif", &scanner, &skills_root)
        .await
        .expect("install");

    // The parent dir is the category directory (e.g. "general")
    let category_dir = outcome.install_path.parent().unwrap().to_path_buf();
    assert!(category_dir.exists(), "category dir should exist before uninstall");

    uninstall("tenor-gif").expect("uninstall");

    // Category dir should be cleaned up since it's now empty
    assert!(
        !category_dir.exists(),
        "empty category dir should be removed: {:?}",
        category_dir
    );

    restore_env(prev);
}
