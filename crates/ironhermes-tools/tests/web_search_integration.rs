//! Phase 41.3 Plan 07 Task 3 — wiremock-backed integration tests for the
//! multi-provider `web_search` chain (D-07/D-08/D-09/D-10/D-19).
//!
//! Every header name, endpoint path, and body field asserted below is
//! derived from the verified provider contracts in `41.3-CONTEXT.md` §
//! canonical_refs and `41.3-RESEARCH.md` § Sources — NOT copied from
//! `web_extract`'s existing Exa/Tavily wiremock fixtures, which target
//! different endpoints with different auth conventions (see
//! `41.3-VALIDATION.md` § Anti-Self-Verification Guards item 2, and
//! `41.3-RESEARCH.md` § Pitfall 5). The Exa case in particular carries an
//! explicit NEGATIVE match proving the wrong (content-retrieval-endpoint)
//! auth header is never sent — a mock that only checked the path would pass
//! either way.
//!
//! All tests mutate process environment (provider keys, `*_ENDPOINT_OVERRIDE`,
//! `IRONHERMES_HOME`) and take the shared `env_lock()` — this binary MUST run
//! with `--test-threads=1` (crate-wide convention for env-mutating tests).

use std::sync::{Arc, OnceLock};

use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{
    any, bearer_token, body_partial_json, header, header_exists, method, path, query_param,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

use ironhermes_core::Config;
use ironhermes_tools::Tool;
use ironhermes_tools::credentials::ToolCredentials;
use ironhermes_tools::web_search::WebSearchTool;
use ironhermes_tools::web_search::backends::{SearchFilters, brave, ddg, exa, tavily};

// =============================================================================
// Shared test infrastructure — env_lock + EnvGuard. Each integration test
// binary in this crate compiles as its own independent crate, so this
// cannot be imported from a sibling test file; it is re-derived here
// (mechanics only, per this file's own doc comment above and the plan's
// read_first note).
// =============================================================================

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, val: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: tests serialised by env_lock().
        unsafe { std::env::set_var(key, val) };
        Self { key, prev }
    }

    fn unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: tests serialised by env_lock().
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: serialised by env_lock(); restoring prior state.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Writes an empty `config.yaml` (no `tools.web_search.chain` key at all) to
/// a fresh tempdir and points `IRONHERMES_HOME` at it — isolates the test
/// from the real operator config while leaving `tools.web_search.chain`
/// fully unconfigured, so `ToolsConfig`'s serde default (Exa > Brave >
/// Tavily > DDG) applies exactly as it would for an operator who edits
/// nothing.
fn write_isolated_home() -> (tempfile::TempDir, EnvGuard) {
    let cfg_tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(cfg_tmp.path().join("config.yaml"), "").expect("write empty config.yaml");
    let home = EnvGuard::set(
        "IRONHERMES_HOME",
        cfg_tmp.path().to_str().expect("tmp path is valid utf8"),
    );
    (cfg_tmp, home)
}

/// Writes a `config.yaml` with an explicit `tools.web_search.chain` and,
/// optionally, `tools.credentials` entries (the D-19 config tier), to a
/// fresh tempdir, and points `IRONHERMES_HOME` at it.
fn write_web_search_config(
    chain: &[&str],
    credentials: &[(&str, &str)],
) -> (tempfile::TempDir, EnvGuard) {
    let cfg_tmp = tempfile::tempdir().expect("tempdir");
    let mut yaml = String::from("tools:\n  web_search:\n    chain:\n");
    for provider in chain {
        yaml.push_str(&format!("      - {provider}\n"));
    }
    if !credentials.is_empty() {
        yaml.push_str("  credentials:\n");
        for (key, value) in credentials {
            yaml.push_str(&format!("    {key}: \"{value}\"\n"));
        }
    }
    std::fs::write(cfg_tmp.path().join("config.yaml"), yaml).expect("write test config.yaml");
    let home = EnvGuard::set(
        "IRONHERMES_HOME",
        cfg_tmp.path().to_str().expect("tmp path is valid utf8"),
    );
    (cfg_tmp, home)
}

/// Resolves a `WebSearchTool` from `cfg` with no vault store — mirrors
/// `build_app_runtime_bundle`'s resolve-once-ahead-of-time shape (Plan 11).
async fn build_tool(cfg: &Config) -> WebSearchTool {
    let creds = ToolCredentials::resolve(cfg, None)
        .await
        .expect("ToolCredentials::resolve must succeed with no vault store");
    WebSearchTool::new(Arc::new(creds))
}

/// A representative fixture of DDG's `html.duckduckgo.com/html/` markup —
/// one `.web-result` block, real class names verified against a live fetch.
const DDG_FIXTURE_HTML: &str = r#"
<html><body><div class="results">
  <div class="result results_links results_links_deep web-result ">
    <div class="links_main links_deep result__body">
      <h2 class="result__title">
        <a rel="nofollow" class="result__a" href="https://example.com/target">Example Target</a>
      </h2>
      <a class="result__snippet" href="https://example.com/target">An example search result.</a>
    </div>
  </div>
</div></body></html>
"#;

// =============================================================================
// Per-provider request-shape assertions (direct backend calls — no
// WebSearchTool/registry involved; these prove one backend's wire format).
// =============================================================================

/// The load-bearing case: a mock that merely matched on path would pass with
/// EITHER header. This asserts BOTH that the correct (`Authorization: Bearer`)
/// header matches AND that a request carrying the existing `web_extract`
/// Exa backend's `x-api-key` header is never sent — a hard failure if the
/// new Exa search backend regresses onto the wrong (content-retrieval
/// endpoint's) auth convention.
#[tokio::test]
async fn exa_search_sends_bearer_auth_to_the_search_path() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(bearer_token("plan07-exa-search-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"results":[]}"#))
        .expect(1)
        .mount(&server)
        .await;
    // Negative control: mounted BEFORE the call, verified on drop.
    let wrong_header_guard = Mock::given(header_exists("x-api-key"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    let _ee = EnvGuard::set(
        "EXA_SEARCH_ENDPOINT_OVERRIDE",
        &format!("{}/search", server.uri()),
    );

    let api_key = SecretString::from("plan07-exa-search-key".to_string());
    let outcome = exa::search("rust programming", 10, &SearchFilters::default(), &api_key)
        .await
        .expect("exa search must succeed against the bearer-matched mock");
    assert_eq!(outcome.provider, "exa");

    drop(wrong_header_guard);
}

/// Asserts the JSON body contains `query`, `type: "auto"`, `numResults`, and
/// nested `contents.highlights: true` — derived from CONTEXT.md's verified
/// Exa contract, not from this backend's own implementation.
#[tokio::test]
async fn exa_search_body_matches_the_documented_request() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(body_partial_json(json!({
            "query": "rust programming",
            "type": "auto",
            "numResults": 5,
            "contents": { "highlights": true },
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"results":[]}"#))
        .expect(1)
        .mount(&server)
        .await;

    let _ee = EnvGuard::set(
        "EXA_SEARCH_ENDPOINT_OVERRIDE",
        &format!("{}/search", server.uri()),
    );

    let api_key = SecretString::from("plan07-exa-body-key".to_string());
    exa::search("rust programming", 5, &SearchFilters::default(), &api_key)
        .await
        .expect("exa search must match the documented request body");
}

/// A mocked response carrying `costDollars.total` results in
/// `SearchOutcome.cost_dollars == Some(...)` (D-14).
#[tokio::test]
async fn exa_cost_dollars_is_captured() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"results":[{"url":"https://example.com","title":"T","text":"body text"}],"costDollars":{"total":0.005}}"#,
        ))
        .mount(&server)
        .await;

    let _ee = EnvGuard::set(
        "EXA_SEARCH_ENDPOINT_OVERRIDE",
        &format!("{}/search", server.uri()),
    );

    let api_key = SecretString::from("plan07-exa-cost-key".to_string());
    let outcome = exa::search("rust", 10, &SearchFilters::default(), &api_key)
        .await
        .expect("exa search must succeed");

    assert_eq!(outcome.cost_dollars, Some(0.005));
}

/// Asserts Tavily's bearer auth AND its documented body fields (`query`,
/// `max_results`, `search_depth`, `include_usage`) in one case, mirroring
/// the plan's bundled behavior spec for this provider.
#[tokio::test]
async fn tavily_search_sends_bearer_auth_to_the_search_path() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/search"))
        .and(bearer_token("plan07-tavily-key"))
        .and(body_partial_json(json!({
            "query": "rust programming",
            "max_results": 7,
            "search_depth": "basic",
            "include_usage": true,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"results":[]}"#))
        .expect(1)
        .mount(&server)
        .await;

    let _te = EnvGuard::set(
        "TAVILY_SEARCH_ENDPOINT_OVERRIDE",
        &format!("{}/search", server.uri()),
    );

    let api_key = SecretString::from("plan07-tavily-key".to_string());
    tavily::search("rust programming", 7, &SearchFilters::default(), &api_key)
        .await
        .expect("tavily search must match bearer auth + the documented request body");
}

/// Asserts the Brave-specific `X-Subscription-Token` header is present AND
/// that no `Authorization` header is ever sent — the non-uniform-auth guard
/// (the backend trait deliberately assumes no shared auth scheme).
#[tokio::test]
async fn brave_search_sends_the_subscription_token_header() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("X-Subscription-Token", "plan07-brave-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"web":{"results":[]}}"#))
        .expect(1)
        .mount(&server)
        .await;
    let no_auth_guard = Mock::given(header_exists("Authorization"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    let _be = EnvGuard::set("BRAVE_SEARCH_ENDPOINT_OVERRIDE", &server.uri());

    let api_key = SecretString::from("plan07-brave-key".to_string());
    brave::search("rust", 10, &SearchFilters::default(), &api_key)
        .await
        .expect("brave search must succeed against the subscription-token-matched mock");

    drop(no_auth_guard);
}

/// A `limit` above Brave's documented maximum (20) produces a request whose
/// `count` parameter equals the maximum, not the raw requested limit.
#[tokio::test]
async fn brave_count_is_clamped_to_the_documented_maximum() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("count", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"web":{"results":[]}}"#))
        .expect(1)
        .mount(&server)
        .await;

    let _be = EnvGuard::set("BRAVE_SEARCH_ENDPOINT_OVERRIDE", &server.uri());

    let api_key = SecretString::from("plan07-brave-clamp-key".to_string());
    brave::search("rust", 50, &SearchFilters::default(), &api_key)
        .await
        .expect("brave search must clamp count to Brave's documented maximum of 20");
}

/// A mocked HTML body yields ordered `SearchHit`s via the real fetch + parse
/// path (not just the pure `parse_results` unit tests in `ddg.rs`).
#[tokio::test]
async fn ddg_results_are_parsed_from_the_html_endpoint() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DDG_FIXTURE_HTML))
        .expect(1)
        .mount(&server)
        .await;

    let _de = EnvGuard::set("DDG_HTML_ENDPOINT_OVERRIDE", &server.uri());

    let outcome = ddg::search("rust", 10).await.expect("ddg search must succeed");
    assert_eq!(outcome.provider, "ddg");
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].title, "Example Target");
    assert_eq!(outcome.hits[0].url, "https://example.com/target");
}

// =============================================================================
// Whole-chain behavior — exercised through WebSearchTool::execute(), the
// real production entry point, not a lower-level seam.
// =============================================================================

/// The first chain entry's mock returns 500; the second returns results.
/// The tool returns the second's results, and the second mock records
/// exactly one request.
#[tokio::test]
async fn chain_falls_through_from_an_erroring_provider_to_the_next() {
    let _g = env_lock().lock().await;
    let _tk = EnvGuard::unset("TAVILY_API_KEY");

    let exa_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&exa_server)
        .await;

    let brave_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"web":{"results":[{"title":"Brave Fallback","url":"https://example.com/b","description":"D"}]}}"#,
        ))
        .expect(1)
        .mount(&brave_server)
        .await;

    let _ee = EnvGuard::set(
        "EXA_SEARCH_ENDPOINT_OVERRIDE",
        &format!("{}/search", exa_server.uri()),
    );
    let _be = EnvGuard::set("BRAVE_SEARCH_ENDPOINT_OVERRIDE", &brave_server.uri());
    let _ek = EnvGuard::set("EXA_API_KEY", "plan07-chain-fallthrough-exa-key");
    let _bk = EnvGuard::set("BRAVE_API_KEY", "plan07-chain-fallthrough-brave-key");

    let (_cfg_tmp, _home) = write_web_search_config(&["exa", "brave"], &[]);
    let cfg = Config::load().unwrap_or_default();
    let tool = build_tool(&cfg).await;

    let result = tool
        .execute(json!({ "query": "rust" }))
        .await
        .expect("web_search must succeed via brave after exa's 500");

    assert!(
        result.contains("Brave Fallback"),
        "expected brave's result content; got: {result}"
    );
}

/// D-09, asserted end to end: with every provider key cleared for the
/// duration of the test and restored afterwards (never leaking an ambient
/// developer key), the default (unconfigured) chain reaches DDG and the
/// tool returns `Ok`.
#[tokio::test]
async fn no_keys_configured_reaches_ddg() {
    let _g = env_lock().lock().await;
    let _ek = EnvGuard::unset("EXA_API_KEY");
    let _bk = EnvGuard::unset("BRAVE_API_KEY");
    let _tk = EnvGuard::unset("TAVILY_API_KEY");

    let ddg_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DDG_FIXTURE_HTML))
        .expect(1)
        .mount(&ddg_server)
        .await;
    let _de = EnvGuard::set("DDG_HTML_ENDPOINT_OVERRIDE", &ddg_server.uri());

    let (_cfg_tmp, _home) = write_isolated_home();
    let cfg = Config::load().unwrap_or_default();
    let tool = build_tool(&cfg).await;

    let result = tool
        .execute(json!({ "query": "rust" }))
        .await
        .expect("web_search must succeed via ddg with zero provider keys configured");

    assert!(result.contains("Example Target"), "got: {result}");
}

/// D-19: the first chain entry's endpoint is mocked with a catch-all
/// matcher expecting ZERO requests, and its key is absent from env, config,
/// AND vault. The second entry is configured and answers. The tool returns
/// the second provider's results, and the first mock's zero-request
/// expectation verifies on drop — the proof is that no socket was touched,
/// not that a function returned `false`.
#[tokio::test]
async fn an_unconfigured_provider_receives_no_request() {
    let _g = env_lock().lock().await;
    let _ek = EnvGuard::unset("EXA_API_KEY");

    let exa_server = MockServer::start().await;
    let exa_guard = Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&exa_server)
        .await;

    let brave_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"web":{"results":[{"title":"Brave Only","url":"https://example.com/b","description":"D"}]}}"#,
        ))
        .expect(1)
        .mount(&brave_server)
        .await;

    let _ee = EnvGuard::set(
        "EXA_SEARCH_ENDPOINT_OVERRIDE",
        &format!("{}/search", exa_server.uri()),
    );
    let _be = EnvGuard::set("BRAVE_SEARCH_ENDPOINT_OVERRIDE", &brave_server.uri());
    let _bk = EnvGuard::set("BRAVE_API_KEY", "plan07-unconfigured-provider-brave-key");

    // EXA_API_KEY: absent from env (unset above), absent from config
    // credentials (empty list below), and no vault store is supplied to
    // build_tool — absent from all three D-19 tiers.
    let (_cfg_tmp, _home) = write_web_search_config(&["exa", "brave"], &[]);
    let cfg = Config::load().unwrap_or_default();
    let tool = build_tool(&cfg).await;

    let result = tool
        .execute(json!({ "query": "rust" }))
        .await
        .expect("web_search must succeed via brave with exa fully unconfigured");
    assert!(result.contains("Brave Only"), "got: {result}");

    // Verifies on drop — panics if the exa mock ever received any request.
    drop(exa_guard);
}

/// D-19 positive control (without which the previous test would also pass
/// against a tool that can never call anything): the same first entry with
/// its key supplied ONLY through the snapshot's config tier
/// (`tools.credentials`, never process env) receives exactly one request.
#[tokio::test]
async fn a_provider_configured_only_in_the_config_tier_is_called() {
    let _g = env_lock().lock().await;
    let _ek = EnvGuard::unset("EXA_API_KEY");

    let exa_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(bearer_token("plan07-config-tier-exa-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"results":[{"url":"https://example.com/x","title":"Config Tier Exa","text":"body"}]}"#,
        ))
        .expect(1)
        .mount(&exa_server)
        .await;

    let _ee = EnvGuard::set(
        "EXA_SEARCH_ENDPOINT_OVERRIDE",
        &format!("{}/search", exa_server.uri()),
    );

    let (_cfg_tmp, _home) =
        write_web_search_config(&["exa"], &[("EXA_API_KEY", "plan07-config-tier-exa-key")]);
    let cfg = Config::load().unwrap_or_default();
    let tool = build_tool(&cfg).await;

    let result = tool
        .execute(json!({ "query": "rust" }))
        .await
        .expect("web_search must succeed using the config-tier-only credential");
    assert!(result.contains("Config Tier Exa"), "got: {result}");
}
