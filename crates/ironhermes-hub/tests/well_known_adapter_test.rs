//! Integration tests for WellKnownSkillSource adapter.
//!
//! All HTTP calls are intercepted by wiremock — no live network in CI.

mod fixtures;

use ironhermes_core::SkillSource;
use ironhermes_hub::{HubError, HubErrorKind, HubSource, WellKnownSkillSource};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

/// Build a WellKnownSkillSource restricted to a single mock host.
fn source_for(host: &str) -> WellKnownSkillSource {
    WellKnownSkillSource::new(vec![host.to_string()])
}

// ── Trust always Community ────────────────────────────────────────────────────

#[test]
fn well_known_always_community() {
    let src = WellKnownSkillSource::new(vec![]);
    assert_eq!(src.trust_level_for("anything"), SkillSource::Community);
    assert_eq!(src.trust_level_for("self.example.com/foo"), SkillSource::Community);
    assert_eq!(src.trust_level_for("well-known:example.com/skill"), SkillSource::Community);
    // Even if someone passes what looks like a trusted repo
    assert_eq!(src.trust_level_for("anthropics/skills/tenor-gif"), SkillSource::Community);
}

// ── HTTPS-only enforcement ────────────────────────────────────────────────────

#[tokio::test]
async fn well_known_https_only() {
    let src = WellKnownSkillSource::new(vec![]);
    // Plain HTTP must be rejected with InvalidIdentifier mentioning "HTTPS required"
    let err = src
        .fetch("http://example.com/foo")
        .await
        .expect_err("HTTP must be rejected");
    match err {
        HubError::Typed { kind: HubErrorKind::InvalidIdentifier, message, .. } => {
            assert!(
                message.to_lowercase().contains("https"),
                "error must mention HTTPS: {message}"
            );
        }
        other => panic!("expected InvalidIdentifier, got {other:?}"),
    }
}

// ── search happy path ─────────────────────────────────────────────────────────

#[tokio::test]
async fn well_known_search_happy_path() {
    let server = MockServer::start().await;
    let host = server.uri().replace("http://", "").replace("https://", "");

    // Serve the index over the mock server (wiremock is HTTP, we override https_only later)
    let index_json = serde_json::json!([
        {
            "name": "foo-skill",
            "description": "Example skill",
            "version": "1.0",
            "identifier": format!("well-known:{host}/foo-skill"),
            "tarball_url": format!("{}/foo-skill.tar.gz", server.uri())
        }
    ]);

    Mock::given(method("GET"))
        .and(path("/.well-known/skills/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&index_json))
        .mount(&server)
        .await;

    // Use a non-https-only source for the mock (wiremock listens on HTTP)
    let src = WellKnownSkillSource::new_http_for_tests(vec![host.clone()]);
    let results = src.search("foo", 10).await.expect("search should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "foo-skill");
    assert_eq!(results[0].source_id, "well-known");
    // Identifier must round-trip through install
    assert!(
        results[0].identifier.contains("foo-skill"),
        "identifier must contain skill name: {}",
        results[0].identifier
    );
}

// ── fetch happy path ──────────────────────────────────────────────────────────

#[tokio::test]
async fn well_known_fetch_happy_path() {
    let server = MockServer::start().await;
    let host = server.uri().replace("http://", "").replace("https://", "");

    let tarball_bytes = fixtures::well_known_skill_tarball();

    let index_json = serde_json::json!([
        {
            "name": "foo-skill",
            "description": "Example skill",
            "version": "1.0",
            "identifier": format!("well-known:{host}/foo-skill"),
            "tarball_url": format!("{}/foo-skill.tar.gz", server.uri())
        }
    ]);

    Mock::given(method("GET"))
        .and(path("/.well-known/skills/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&index_json))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/foo-skill.tar.gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tarball_bytes)
                .insert_header("content-type", "application/x-gzip"),
        )
        .mount(&server)
        .await;

    let src = WellKnownSkillSource::new_http_for_tests(vec![host.clone()]);
    let identifier = format!("well-known:{host}/foo-skill");
    let bundle = src.fetch(&identifier).await.expect("fetch should succeed");

    assert_eq!(bundle.source_id, "well-known");
    assert!(
        bundle.skill_md.contains("foo-skill"),
        "skill_md must contain skill name: {}",
        bundle.skill_md
    );
    assert!(!bundle.files.is_empty(), "bundle must have files");
}

// ── SSRF guard: reject loopback ───────────────────────────────────────────────

#[tokio::test]
async fn well_known_rejects_loopback() {
    let src = WellKnownSkillSource::new(vec![]);

    let err = src.fetch("https://127.0.0.1/foo").await.expect_err("loopback must be rejected");
    match &err {
        HubError::Typed { kind: HubErrorKind::InvalidIdentifier, message, .. } => {
            assert!(
                message.contains("loopback") || message.contains("private") || message.contains("127.0.0.1"),
                "error must mention loopback/private: {message}"
            );
        }
        other => panic!("expected InvalidIdentifier (SSRF guard), got {other:?}"),
    }

    let err2 = src.fetch("https://10.0.0.1/foo").await.expect_err("private IP must be rejected");
    assert!(matches!(err2, HubError::Typed { kind: HubErrorKind::InvalidIdentifier, .. }));
}

// ── Allowlist enforcement ─────────────────────────────────────────────────────

#[tokio::test]
async fn well_known_origin_allowlist_respected() {
    // Source only allows "example.com"
    let src = WellKnownSkillSource::new(vec!["example.com".to_string()]);

    // Fetch from a different host must fail with NotFound (not in allowlist)
    let err = src
        .fetch("well-known:other.example.org/foo")
        .await
        .expect_err("non-allowlisted host must be rejected");

    match &err {
        HubError::Typed { kind: HubErrorKind::NotFound, message, .. } => {
            assert!(
                message.contains("allowlist") || message.contains("other.example.org"),
                "error must mention allowlist: {message}"
            );
        }
        other => panic!("expected NotFound (allowlist), got {other:?}"),
    }
}
