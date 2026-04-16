//! Integration tests for GitHubSource adapter.
//!
//! All HTTP calls are intercepted by wiremock — no live network in CI.

mod fixtures;

use std::collections::HashSet;

use ironhermes_core::SkillSource;
use ironhermes_hub::{GitHubAuth, GitHubSource, GitHubTap, HubError, HubErrorKind, HubSource};
use wiremock::{
    matchers::{header, method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

fn anon_auth() -> GitHubAuth {
    GitHubAuth::anonymous()
}

fn source_with(server: &MockServer, trusted: &[&str]) -> GitHubSource {
    let trusted_repos: HashSet<String> = trusted.iter().map(|s| s.to_string()).collect();
    GitHubSource::new(anon_auth(), trusted_repos, vec![]).with_api_base(server.uri())
}

// ── DEFAULT_TAPS constant ────────────────────────────────────────────────────

#[test]
fn github_default_taps_contents() {
    let taps = GitHubSource::DEFAULT_TAPS;
    let repos: Vec<&str> = taps.iter().map(|(r, _)| *r).collect();
    assert_eq!(
        repos,
        &[
            "openai/skills",
            "anthropics/skills",
            "VoltAgent/awesome-agent-skills",
            "garrytan/gstack",
        ],
        "DEFAULT_TAPS must match hermes-agent reference exactly (D-02)"
    );
}

// ── Trust resolution ─────────────────────────────────────────────────────────

#[test]
fn github_trust_resolution() {
    let trusted: HashSet<String> = ["anthropics/skills"].iter().map(|s| s.to_string()).collect();
    let auth = GitHubAuth::anonymous();
    let src = GitHubSource::new(auth, trusted, vec![]);

    assert_eq!(
        src.trust_level_for("anthropics/skills/tenor-gif"),
        SkillSource::Trusted,
        "repo in trusted_repos → Trusted"
    );
    assert_eq!(
        src.trust_level_for("openai/skills/some-skill"),
        SkillSource::Community,
        "repo not in trusted_repos → Community"
    );

    // With empty trusted_repos, same identifier → Community.
    let src2 = GitHubSource::new(GitHubAuth::anonymous(), HashSet::new(), vec![]);
    assert_eq!(
        src2.trust_level_for("anthropics/skills/tenor-gif"),
        SkillSource::Community
    );
}

// ── search happy path ────────────────────────────────────────────────────────

#[tokio::test]
async fn github_search_happy_path() {
    let server = MockServer::start().await;

    // Mock: GET /repos/openai/skills → default_branch = "main"
    let repo_info = serde_json::json!({"default_branch": "main", "full_name": "openai/skills"});
    Mock::given(method("GET"))
        .and(path("/repos/openai/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&repo_info))
        .mount(&server)
        .await;

    // Mock: GET /repos/openai/skills/git/trees/main?recursive=1
    // Tree has two SKILL.md entries: one matching "gif", one not.
    let tree = serde_json::json!({
        "truncated": false,
        "tree": [
            {"type": "blob", "path": "tenor-gif/SKILL.md"},
            {"type": "blob", "path": "tenor-gif/handler.py"},
            {"type": "blob", "path": "weather-tool/SKILL.md"},
            {"type": "blob", "path": "weather-tool/handler.py"},
        ]
    });
    Mock::given(method("GET"))
        .and(path("/repos/openai/skills/git/trees/main"))
        .and(query_param("recursive", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&tree))
        .mount(&server)
        .await;

    // Mock all other repos → 404 so they don't interfere
    for repo in &["anthropics/skills", "VoltAgent/awesome-agent-skills", "garrytan/gstack"] {
        Mock::given(method("GET"))
            .and(path(format!("/repos/{repo}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
    }

    let src = source_with(&server, &[]);
    let results = src.search("gif", 10).await.expect("search should succeed");

    assert_eq!(results.len(), 1, "only 'tenor-gif' matches 'gif'");
    assert_eq!(results[0].identifier, "openai/skills/tenor-gif");
    assert_eq!(results[0].source_id, "github");
}

// ── search rate-limited ──────────────────────────────────────────────────────

#[tokio::test]
async fn github_search_rate_limited() {
    let server = MockServer::start().await;

    let reset_ts = (chrono::Utc::now() + chrono::Duration::seconds(120))
        .timestamp()
        .to_string();

    Mock::given(method("GET"))
        .and(path("/repos/openai/skills"))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("X-RateLimit-Remaining", "0")
                .insert_header("X-RateLimit-Reset", reset_ts.as_str()),
        )
        .mount(&server)
        .await;

    // Other taps also 404
    for repo in &["anthropics/skills", "VoltAgent/awesome-agent-skills", "garrytan/gstack"] {
        Mock::given(method("GET"))
            .and(path(format!("/repos/{repo}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
    }

    let src = source_with(&server, &[]);
    let err = src.search("gif", 10).await.expect_err("should fail with rate-limit");

    match err {
        HubError::Typed { kind: HubErrorKind::RateLimited, retry_after_s, .. } => {
            assert!(retry_after_s.is_some(), "retry_after_s should be populated from X-RateLimit-Reset");
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

// ── search not found ─────────────────────────────────────────────────────────

#[tokio::test]
async fn github_search_not_found() {
    let server = MockServer::start().await;

    // All taps 404 → all fail → expect NotFound (or empty results, per design)
    for repo in &[
        "openai/skills",
        "anthropics/skills",
        "VoltAgent/awesome-agent-skills",
        "garrytan/gstack",
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/repos/{repo}")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
    }

    let src = source_with(&server, &[]);
    // When all taps 404, search returns empty (not an error) — adapters skip
    // per-tap failures and return the accumulated results.
    let results = src.search("gif", 10).await.expect("search skips 404 taps");
    assert!(results.is_empty(), "no taps available → empty results");
}

// ── fetch happy path ─────────────────────────────────────────────────────────

#[tokio::test]
async fn github_fetch_happy_path() {
    let server = MockServer::start().await;

    let tarball_bytes = fixtures::sample_skill_tarball();

    // Mock: GET /repos/anthropics/skills/tarball/HEAD
    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills/tarball/HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tarball_bytes)
                .insert_header("content-type", "application/x-gzip"),
        )
        .mount(&server)
        .await;

    let src = source_with(&server, &["anthropics/skills"]);
    let bundle = src
        .fetch("anthropics/skills/tenor-gif")
        .await
        .expect("fetch should succeed");

    assert_eq!(bundle.files.len(), 2, "SKILL.md + handler.py");
    assert!(
        bundle.skill_md.contains("name: tenor-gif"),
        "skill_md must contain frontmatter"
    );
    for f in &bundle.files {
        assert!(
            !f.path.starts_with('/'),
            "all BundleFile paths must be relative, got: {}",
            f.path
        );
        assert!(
            !f.path.contains(".."),
            "no parent-dir refs in paths, got: {}",
            f.path
        );
    }
    assert_eq!(bundle.source_id, "github");
    assert_eq!(bundle.identifier, "anthropics/skills/tenor-gif");
}

// ── fetch path traversal rejection ──────────────────────────────────────────

#[tokio::test]
async fn github_fetch_rejects_traversal() {
    let server = MockServer::start().await;

    let bad_tarball = fixtures::traversal_tarball();

    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills/tarball/HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(bad_tarball)
                .insert_header("content-type", "application/x-gzip"),
        )
        .mount(&server)
        .await;

    let src = source_with(&server, &[]);
    let err = src
        .fetch("anthropics/skills/tenor-gif")
        .await
        .expect_err("traversal tarball must be rejected");

    match &err {
        HubError::Typed { kind, .. } => {
            assert!(
                matches!(kind, HubErrorKind::Parse | HubErrorKind::TrustRejected),
                "expected Parse or TrustRejected, got {kind:?}"
            );
        }
        other => panic!("expected Typed error, got {other:?}"),
    }
}

// ── fetch size cap ────────────────────────────────────────────────────────────

#[tokio::test]
async fn github_fetch_size_cap() {
    let server = MockServer::start().await;

    let big_tarball = fixtures::oversized_tarball();

    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills/tarball/HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(big_tarball)
                .insert_header("content-type", "application/x-gzip"),
        )
        .mount(&server)
        .await;

    let src = source_with(&server, &[]);
    let err = src
        .fetch("anthropics/skills/tenor-gif")
        .await
        .expect_err("oversized tarball must be rejected");

    match &err {
        HubError::Typed { kind: HubErrorKind::Parse, message, .. } => {
            assert!(
                message.contains("MAX_EXTRACTED_BYTES"),
                "error must mention size cap: {message}"
            );
        }
        other => panic!("expected Parse error about size, got {other:?}"),
    }
}

// ── auth header propagation (compile-only / logic test) ──────────────────────

#[test]
fn github_auth_header_format() {
    let auth = GitHubAuth::from_token("test-token-xyz".to_string());
    let header_val = auth.authorization_header();
    assert_eq!(header_val, Some("Bearer test-token-xyz".to_string()));

    let anon = GitHubAuth::anonymous();
    assert_eq!(anon.authorization_header(), None);
}
