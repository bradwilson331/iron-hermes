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
use std::sync::Arc;
use std::sync::Mutex;

use ironhermes_hub::{
    AlwaysBlockedScanner, AlwaysCleanScanner, GitHubAuth, GitHubSource, HubError, HubErrorKind,
    SkillLock, SkillsShBlobSource, install, uninstall, update,
};

use fixtures::{sample_blob_response_json, sample_skill_md_frontmatter, sample_tree_json};
use wiremock::matchers::{method, path, path_regex, query_param};
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
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"default_branch": "main"})),
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
    use flate2::Compression;
    use flate2::write::GzEncoder;

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

    let _outcome1 = install(
        &source1,
        "anthropics/skills/tenor-gif",
        &scanner,
        &skills_root,
        false,
    )
    .await
    .expect("install v1");
    // The pre-update hash is the SkillLockEntry.computed_hash (D-13 folder hash),
    // NOT InstallOutcome.content_hash (the pre-21.8 bundle_content_hash). The
    // UpdateOutcome surfaces computed_hash so old_hash here must come from the lock.
    let old_hash = SkillLock::load_or_default()
        .expect("lock")
        .get("tenor-gif")
        .expect("lock entry")
        .computed_hash
        .clone();

    // Update with v2 (different content -> different hash)
    let server2 = MockServer::start().await;
    mount_github_mocks(&server2, build_skill_tarball("v2 updated content")).await;
    let source2 = test_github_source(&server2.uri(), HashSet::new());

    let update_outcome = update(&source2, "tenor-gif", &scanner, &skills_root, false)
        .await
        .expect("update should succeed");

    assert_eq!(update_outcome.name, "tenor-gif");
    assert_eq!(update_outcome.old_hash, old_hash);
    assert_ne!(
        update_outcome.old_hash, update_outcome.new_hash,
        "hashes should differ"
    );
    assert_eq!(update_outcome.scan_verdict, "clean");

    // Verify skills-lock.json was updated (SkillLockEntry.computed_hash is
    // the post-rename folder hash matching UpdateOutcome.new_hash).
    let lock = SkillLock::load_or_default().expect("lock");
    let entry = lock
        .get("tenor-gif")
        .expect("lock entry for tenor-gif should exist");
    assert_eq!(entry.computed_hash, update_outcome.new_hash);
    assert_eq!(entry.source, "github");
    assert_eq!(entry.identifier, "anthropics/skills/tenor-gif");

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

    install(
        &source1,
        "anthropics/skills/tenor-gif",
        &scanner,
        &skills_root,
        false,
    )
    .await
    .expect("install");

    // Update with same content
    let server2 = MockServer::start().await;
    mount_github_mocks(&server2, tarball).await;
    let source2 = test_github_source(&server2.uri(), HashSet::new());

    let err = update(&source2, "tenor-gif", &scanner, &skills_root, false)
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

    let err = update(&source, "nonexistent-skill", &scanner, &skills_root, false)
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
        false,
    )
    .await
    .expect("install v1");

    // Update v2 with blocked scanner (simulates new version having injection)
    let server2 = MockServer::start().await;
    mount_github_mocks(&server2, build_skill_tarball("v2 with injection")).await;
    let source2 = test_github_source(&server2.uri(), HashSet::new());
    let blocked_scanner = AlwaysBlockedScanner::new("injection detected");

    let err = update(&source2, "tenor-gif", &blocked_scanner, &skills_root, false)
        .await
        .expect_err("should be blocked");

    match err {
        HubError::Typed {
            kind: HubErrorKind::ScanBlocked,
            ..
        } => {}
        other => panic!("expected ScanBlocked, got: {:?}", other),
    }

    // Verify the old version is still intact (atomic: scan failure = no replace).
    // SkillLock has no scan_verdict column (removed in 21.8) — we assert the
    // original entry is still registered in the lock.
    let lock = SkillLock::load_or_default().expect("lock");
    assert!(
        lock.get("tenor-gif").is_some(),
        "old lock entry must remain after failed update (atomicity)"
    );

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

    let outcome = install(
        &source,
        "anthropics/skills/tenor-gif",
        &scanner,
        &skills_root,
        false,
    )
    .await
    .expect("install");
    assert!(outcome.install_path.exists());

    // Uninstall
    let un_outcome = uninstall("tenor-gif").expect("uninstall should succeed");
    assert_eq!(un_outcome.name, "tenor-gif");
    assert!(
        !un_outcome.removed_path.exists(),
        "directory should be removed"
    );

    // Verify skills-lock.json is clean
    let lock = SkillLock::load_or_default().expect("lock");
    assert!(lock.get("tenor-gif").is_none());

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

    let outcome = install(
        &source,
        "anthropics/skills/tenor-gif",
        &scanner,
        &skills_root,
        false,
    )
    .await
    .expect("install");

    // The parent dir is the category directory (e.g. "general")
    let category_dir = outcome.install_path.parent().unwrap().to_path_buf();
    assert!(
        category_dir.exists(),
        "category dir should exist before uninstall"
    );

    uninstall("tenor-gif").expect("uninstall");

    // Category dir should be cleaned up since it's now empty
    assert!(
        !category_dir.exists(),
        "empty category dir should be removed: {:?}",
        category_dir
    );

    restore_env(prev);
}

// ── Update: server/client hash divergence is advisory (UAT gap 21.8-06) ────

/// Build a SkillsShBlobSource pointing at `server` for all three upstream hops.
/// Mirrors the helper in skills_sh_blob_adapter.rs (not pub, so reproduced here).
fn build_blob_source_for_update_test(server: &MockServer) -> SkillsShBlobSource {
    let gh = Arc::new(GitHubSource::new(
        GitHubAuth::anonymous(),
        HashSet::new(),
        vec![],
    ));
    SkillsShBlobSource::new_http_for_tests(gh, server.uri())
        .with_upstream_bases(server.uri(), server.uri())
}

/// Mount the standard three-hop wiremock for ascii-art with a caller-supplied hash.
async fn mount_three_hop_mocks(server: &MockServer, server_hash: &str) {
    Mock::given(method("GET"))
        .and(path_regex(r"^/repos/.+/.+/git/trees/.+$"))
        .and(query_param("recursive", "1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(sample_tree_json("ascii-art/SKILL.md")),
        )
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/.+/.+/.+/ascii-art/SKILL\.md$"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sample_skill_md_frontmatter()))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/api/download/.+/.+/.+$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(sample_blob_response_json(server_hash)),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn update_tolerates_server_client_hash_divergence() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("HERMES_HOME").ok();
    let (_tmp, skills_root) = setup_test_env();

    // Step 1 — prime: install ascii-art with a matching server hash so the
    // pre-state is a clean happy-path lock entry (no advisory trigger on install).
    // Use the same D-13 algorithm to derive the expected hash from the fixture files.
    let skill_md = sample_skill_md_frontmatter().as_bytes();
    let helper_py = b"print('hi')\n" as &[u8];
    use sha2::{Digest, Sha256};
    let server_hash_v1 = {
        let mut files: Vec<(String, &[u8])> = vec![
            ("SKILL.md".to_string(), skill_md),
            ("helper.py".to_string(), helper_py),
        ];
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut h = Sha256::new();
        for (path, content) in &files {
            h.update(path.as_bytes());
            h.update(content);
        }
        hex::encode(h.finalize())
    };

    let server1 = MockServer::start().await;
    mount_three_hop_mocks(&server1, &server_hash_v1).await;

    unsafe {
        std::env::set_var("SKILLS_DOWNLOAD_URL", server1.uri());
    }
    let src1 = build_blob_source_for_update_test(&server1);
    ironhermes_hub::install(
        &src1,
        "foo/bar/ascii-art",
        &AlwaysCleanScanner,
        &skills_root,
        true,
    )
    .await
    .expect("prime install for update divergence test");

    // Step 2 — remount a new server with a divergent hash AND content-changed blob.
    // The content delta ensures bundle_folder_hash != old snapshot -> drift detected.
    let server_hash_v2 = "divergent-update-server-hash-21-8-06".to_string();
    let server2 = MockServer::start().await;

    // Three-hop: Trees + raw frontmatter unchanged; /api/download returns new hash + extra file.
    Mock::given(method("GET"))
        .and(path_regex(r"^/repos/.+/.+/git/trees/.+$"))
        .and(query_param("recursive", "1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(sample_tree_json("ascii-art/SKILL.md")),
        )
        .mount(&server2)
        .await;

    Mock::given(method("GET"))
        .and(path_regex(r"^/.+/.+/.+/ascii-art/SKILL\.md$"))
        .respond_with(ResponseTemplate::new(200).set_body_string(sample_skill_md_frontmatter()))
        .mount(&server2)
        .await;

    // Blob with content delta: helper.py has one extra byte so D-13 folder hash changes.
    let updated_blob = serde_json::json!({
        "files": [
            {"path": "SKILL.md", "contents": sample_skill_md_frontmatter()},
            {"path": "helper.py", "contents": "print('hi')\nX"}
        ],
        "hash": server_hash_v2
    });
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/download/.+/.+/.+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&updated_blob))
        .mount(&server2)
        .await;

    unsafe {
        std::env::set_var("SKILLS_DOWNLOAD_URL", server2.uri());
    }
    let src2 = build_blob_source_for_update_test(&server2);

    // Step 3 — update: must succeed despite server hash != D-13 folder hash.
    let outcome = update(&src2, "ascii-art", &AlwaysCleanScanner, &skills_root, false)
        .await
        .expect("update must succeed on divergence (UAT gap 21.8-06)");

    // Step 4 — assert advisory posture preserved all invariants.
    let lock = SkillLock::load_or_default().unwrap();
    let entry = lock.get("ascii-art").expect("updated entry");

    // D-14: round-trip on update path.
    assert_eq!(
        entry.snapshot_hash, server_hash_v2,
        "update() MUST round-trip the new server snapshotHash verbatim even on divergence"
    );

    // D-13: refreshed computed_hash on update path.
    let disk_hash = ironhermes_hub::compute_folder_hash(&outcome.install_path).unwrap();
    assert_eq!(
        disk_hash, entry.computed_hash,
        "update() MUST refresh entry.computed_hash to match compute_folder_hash(resolved_final)"
    );

    // Advisory: directory survived.
    assert!(
        outcome.install_path.exists(),
        "update advisory branch MUST NOT cleanup resolved_final on divergence"
    );

    restore_env(prev);
}
