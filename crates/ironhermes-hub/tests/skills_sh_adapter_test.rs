//! Integration tests for SkillsShSource adapter.
//!
//! All HTTP calls are intercepted by wiremock — no live network in CI.

mod fixtures;

use std::{collections::HashSet, sync::Arc};

use ironhermes_core::SkillSource;
use ironhermes_hub::{
    GitHubAuth, GitHubSource, HubError, HubErrorKind, HubSource, SkillsShSource,
};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

/// Build a GitHubSource pointed at the mock server with the given trusted repos.
fn github_source(server: &MockServer, trusted: &[&str]) -> Arc<GitHubSource> {
    let trusted_repos: HashSet<String> = trusted.iter().map(|s| s.to_string()).collect();
    Arc::new(
        GitHubSource::new(GitHubAuth::anonymous(), trusted_repos, vec![])
            .with_api_base(server.uri()),
    )
}

// ── source_id ─────────────────────────────────────────────────────────────────

#[test]
fn skills_sh_source_id() {
    let github = github_source_no_server();
    let src = SkillsShSource::new(github);
    assert_eq!(src.source_id(), "skills-sh", "source_id must be 'skills-sh' (D-14)");
}

fn github_source_no_server() -> Arc<GitHubSource> {
    Arc::new(GitHubSource::new(GitHubAuth::anonymous(), HashSet::new(), vec![]))
}

// ── trust_resolution ─────────────────────────────────────────────────────────

#[test]
fn skills_sh_trust_resolution() {
    // skills-sh trust_level_for is synchronous and defers to Community (D-06/D-08)
    // The install pipeline re-resolves via async path for the actual trust label.
    let github = github_source_no_server();
    let src = SkillsShSource::new(github);
    // Always Community from the sync method
    assert_eq!(
        src.trust_level_for("skills-sh:tenor-gif"),
        SkillSource::Community,
        "sync trust_level_for must return Community (async resolution deferred to install pipeline)"
    );
    assert_eq!(src.trust_level_for("skills-sh:any-skill"), SkillSource::Community);
}

// ── search happy path ─────────────────────────────────────────────────────────

#[tokio::test]
async fn skills_sh_search_happy_path() {
    let server = MockServer::start().await;

    let registry = serde_json::json!({
        "tenor-gif": {
            "repo": "anthropics/skills",
            "path": "tenor-gif",
            "description": "Tenor GIF search skill"
        },
        "weather-tool": {
            "repo": "openai/skills",
            "path": "weather-tool",
            "description": "Weather forecast skill"
        }
    });

    Mock::given(method("GET"))
        .and(path("/registry.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&registry))
        .mount(&server)
        .await;

    let github = github_source(&server, &[]);
    let registry_url = format!("{}/registry.json", server.uri());
    let src = SkillsShSource::new(github).with_registry_url(registry_url);

    let results = src.search("tenor", 10).await.expect("search should succeed");

    assert_eq!(results.len(), 1, "only tenor-gif matches 'tenor'");
    assert_eq!(results[0].name, "tenor-gif");
    assert_eq!(results[0].source_id, "skills-sh");
    assert_eq!(
        results[0].identifier, "skills-sh:tenor-gif",
        "identifier must use 'skills-sh:' prefix"
    );
}

// ── fetch delegates to GitHub, re-stamps provenance ──────────────────────────

#[tokio::test]
async fn skills_sh_fetch_delegates_to_github() {
    let server = MockServer::start().await;

    // Registry maps tenor-gif → anthropics/skills/tenor-gif
    let registry = serde_json::json!({
        "tenor-gif": {
            "repo": "anthropics/skills",
            "path": "tenor-gif"
        }
    });

    Mock::given(method("GET"))
        .and(path("/registry.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&registry))
        .mount(&server)
        .await;

    // GitHub tarball mock
    let tarball_bytes = fixtures::sample_skill_tarball();
    Mock::given(method("GET"))
        .and(path("/repos/anthropics/skills/tarball/HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tarball_bytes)
                .insert_header("content-type", "application/x-gzip"),
        )
        .mount(&server)
        .await;

    let github = github_source(&server, &["anthropics/skills"]);
    let registry_url = format!("{}/registry.json", server.uri());
    let src = SkillsShSource::new(github).with_registry_url(registry_url);

    let bundle = src
        .fetch("skills-sh:tenor-gif")
        .await
        .expect("fetch should succeed");

    // source_id must be "skills-sh" NOT "github" — provenance re-stamped (D-06/D-09)
    assert_eq!(
        bundle.source_id, "skills-sh",
        "bundle.source_id must be 'skills-sh', not 'github'"
    );
    // identifier preserved as skills-sh form
    assert_eq!(bundle.identifier, "skills-sh:tenor-gif");

    // Files were actually extracted from the GitHub tarball
    assert_eq!(bundle.files.len(), 2, "SKILL.md + handler.py from tarball");
    assert!(bundle.skill_md.contains("tenor-gif"), "skill_md must contain frontmatter");
}
