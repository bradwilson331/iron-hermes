//! Phase 41.3 Plan 08 Task 3 — wiremock-backed integration tests for the
//! multi-provider `web_answer` chain (D-07/D-08/D-09/D-13/D-19).
//!
//! Every header name, endpoint path, and body field asserted below is
//! derived from the verified provider contracts in `41.3-CONTEXT.md` §
//! canonical_refs / `41.3-RESEARCH.md` § Sources and from a direct
//! documentation pull for Brave's LLM-context endpoint (this plan's Task 1,
//! since RESEARCH.md Open Question 2 left its exact path/envelope
//! unverified) — NOT copied from a sibling test file's fixtures, which
//! target different endpoints with different auth conventions (see
//! `41.3-VALIDATION.md` § Anti-Self-Verification Guards item 2).
//!
//! All tests mutate process environment (provider keys,
//! `*_ENDPOINT_OVERRIDE`, `IRONHERMES_HOME`) and take the shared
//! `env_lock()` — this binary MUST run with `--test-threads=1` (crate-wide
//! convention for env-mutating tests).

use std::sync::{Arc, OnceLock};

use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{any, bearer_token, body_partial_json, header, header_exists, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

use ironhermes_core::Config;
use ironhermes_tools::Tool;
use ironhermes_tools::credentials::ToolCredentials;
use ironhermes_tools::web_answer::WebAnswerTool;
use ironhermes_tools::web_answer::backends::{brave, ddg, exa, perplexity};

// =============================================================================
// Shared test infrastructure — env_lock + EnvGuard. Each integration test
// binary in this crate compiles as its own independent crate, so this
// cannot be imported from a sibling test file; it is re-derived here
// (mechanics only, per this file's own doc comment above).
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

/// Writes an empty `config.yaml` (no `tools.web_answer.chain` key at all) to
/// a fresh tempdir and points `IRONHERMES_HOME` at it — isolates the test
/// from the real operator config while leaving `tools.web_answer.chain`
/// fully unconfigured, so `ToolsConfig`'s serde default (Perplexity > Exa >
/// Brave > DDG) applies exactly as it would for an operator who edits
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

/// Writes a `config.yaml` with an explicit `tools.web_answer.chain` and,
/// optionally, `tools.credentials` entries (the D-19 config tier), to a
/// fresh tempdir, and points `IRONHERMES_HOME` at it.
fn write_web_answer_config(
    chain: &[&str],
    credentials: &[(&str, &str)],
) -> (tempfile::TempDir, EnvGuard) {
    let cfg_tmp = tempfile::tempdir().expect("tempdir");
    let mut yaml = String::from("tools:\n  web_answer:\n    chain:\n");
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

/// Resolves a `WebAnswerTool` from `cfg` with no vault store — mirrors
/// `build_app_runtime_bundle`'s resolve-once-ahead-of-time shape (Plan 11).
async fn build_tool(cfg: &Config) -> WebAnswerTool {
    let creds = ToolCredentials::resolve(cfg, None)
        .await
        .expect("ToolCredentials::resolve must succeed with no vault store");
    WebAnswerTool::new(Arc::new(creds))
}

// =============================================================================
// Per-provider request-shape assertions (direct backend calls — no
// WebAnswerTool/registry involved; these prove one backend's wire format).
// =============================================================================

/// Asserts the Perplexity request carries an `Authorization` bearer header
/// and a body with `input` and the current preset value, and that `stream`
/// is never sent as `true` — a mock that only checked the path would pass
/// either way, so this asserts on the header and the body together.
#[tokio::test]
async fn perplexity_request_matches_the_documented_contract() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(bearer_token("plan08-perplexity-key"))
        .and(body_partial_json(json!({
            "input": "what is rust",
            "preset": "low",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"output":[{"id":"msg_1","type":"message","content":[{"type":"output_text","text":"Rust is a systems language."}]}]}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    // Negative control: no request may ever carry `stream: true`.
    let no_stream_guard = Mock::given(body_partial_json(json!({ "stream": true })))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    let _pe = EnvGuard::set("PERPLEXITY_ENDPOINT_OVERRIDE", &server.uri());

    let api_key = SecretString::from("plan08-perplexity-key".to_string());
    let outcome = perplexity::answer("what is rust", &api_key)
        .await
        .expect("perplexity answer must succeed against the bearer+body-matched mock");
    assert_eq!(outcome.provider, "perplexity");

    drop(no_stream_guard);
}

/// D-13, executable: a mocked multi-part response (two `message`-typed
/// output items, each carrying a `content` part) is returned as ONE
/// finished string containing every part — proving collection, not
/// truncation.
#[tokio::test]
async fn perplexity_answer_is_returned_complete() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"output":[
                {"id":"msg_1","type":"message","content":[{"type":"output_text","text":"Part one of the answer."}]},
                {"id":"msg_2","type":"message","content":[{"type":"output_text","text":"Part two of the answer."}]}
            ]}"#,
        ))
        .mount(&server)
        .await;

    let _pe = EnvGuard::set("PERPLEXITY_ENDPOINT_OVERRIDE", &server.uri());

    let api_key = SecretString::from("plan08-perplexity-complete-key".to_string());
    let outcome = perplexity::answer("multi-part probe", &api_key)
        .await
        .expect("perplexity answer must succeed");

    assert!(
        outcome.text.contains("Part one of the answer."),
        "got: {}",
        outcome.text
    );
    assert!(
        outcome.text.contains("Part two of the answer."),
        "got: {}",
        outcome.text
    );
}

/// A mocked response carrying `output.grounding[].citations[]` and
/// `costDollars.total` yields populated `citations` AND `cost_dollars ==
/// Some(...)` (D-14).
#[tokio::test]
async fn exa_answer_captures_grounding_and_cost() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"output":{"content":"Rust is a systems programming language.","grounding":[{"citations":["https://rust-lang.org/"]}]},"costDollars":{"total":0.01}}"#,
        ))
        .mount(&server)
        .await;

    let _ee = EnvGuard::set("EXA_ANSWER_ENDPOINT_OVERRIDE", &server.uri());

    let api_key = SecretString::from("plan08-exa-answer-key".to_string());
    let outcome = exa::answer("rust", &api_key)
        .await
        .expect("exa answer must succeed");

    assert_eq!(outcome.citations, vec!["https://rust-lang.org/".to_string()]);
    assert_eq!(outcome.cost_dollars, Some(0.01));
}

/// Asserts the Brave-specific `X-Subscription-Token` header is present AND
/// that no `Authorization` header is ever sent — the non-uniform-auth
/// guard, mirroring `web_search`'s identical Brave assertion.
#[tokio::test]
async fn brave_answer_sends_the_subscription_token_header() {
    let _g = env_lock().lock().await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("X-Subscription-Token", "plan08-brave-answer-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"grounding":{"generic":[{"url":"https://example.com","title":"T","snippets":["A grounded snippet."]}]}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    let no_auth_guard = Mock::given(header_exists("Authorization"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    let _be = EnvGuard::set("BRAVE_ANSWER_ENDPOINT_OVERRIDE", &server.uri());

    let api_key = SecretString::from("plan08-brave-answer-key".to_string());
    brave::answer("rust", &api_key)
        .await
        .expect("brave answer must succeed against the subscription-token-matched mock");

    drop(no_auth_guard);
}

/// A mocked JSON response carrying `AbstractText` yields that text as the
/// answer; a response whose only populated field is the disambiguation
/// list (never named or modelled by this backend at all) yields a MISS —
/// an `Err`, not a fabricated empty-string answer dressed as success.
#[tokio::test]
async fn ddg_answer_uses_the_instant_answer_json() {
    let _g = env_lock().lock().await;

    let hit_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"AbstractText":"DuckDuckGo is a search engine.","Answer":"","Definition":""}"#,
        ))
        .mount(&hit_server)
        .await;
    {
        let _de = EnvGuard::set("DDG_API_ENDPOINT_OVERRIDE", &hit_server.uri());
        let outcome = ddg::answer("duckduckgo")
            .await
            .expect("ddg answer must succeed when AbstractText is populated");
        assert_eq!(outcome.text, "DuckDuckGo is a search engine.");
    }

    let miss_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"AbstractText":"","Answer":"","Definition":"","RelatedTopics":[{"Text":"a disambiguation link","FirstURL":"https://example.com/x"}]}"#,
        ))
        .mount(&miss_server)
        .await;
    {
        let _de = EnvGuard::set("DDG_API_ENDPOINT_OVERRIDE", &miss_server.uri());
        let result = ddg::answer("ambiguous query").await;
        assert!(
            result.is_err(),
            "a RelatedTopics-only response must be a clean miss (Err), not a fabricated answer; \
             got: {result:?}"
        );
    }
}

// =============================================================================
// Whole-chain behavior — exercised through WebAnswerTool::execute(), the
// real production entry point, not a lower-level seam.
// =============================================================================

/// The first chain entry's mock returns 500; the second returns an answer.
/// The tool returns the second's answer, and the second mock records
/// exactly one request.
#[tokio::test]
async fn chain_falls_through_from_an_erroring_provider_to_the_next() {
    let _g = env_lock().lock().await;
    let _bk = EnvGuard::unset("BRAVE_API_KEY");

    let perplexity_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&perplexity_server)
        .await;

    let exa_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"output":{"content":"Exa Fallback Answer.","grounding":[]}}"#,
        ))
        .expect(1)
        .mount(&exa_server)
        .await;

    let _pe = EnvGuard::set("PERPLEXITY_ENDPOINT_OVERRIDE", &perplexity_server.uri());
    let _ee = EnvGuard::set("EXA_ANSWER_ENDPOINT_OVERRIDE", &exa_server.uri());
    let _pk = EnvGuard::set("PERPLEXITY_API_KEY", "plan08-chain-fallthrough-perplexity-key");
    let _ek = EnvGuard::set("EXA_API_KEY", "plan08-chain-fallthrough-exa-key");

    let (_cfg_tmp, _home) = write_web_answer_config(&["perplexity", "exa"], &[]);
    let cfg = Config::load().unwrap_or_default();
    let tool = build_tool(&cfg).await;

    let result = tool
        .execute(json!({ "query": "rust" }))
        .await
        .expect("web_answer must succeed via exa after perplexity's 500");

    assert!(
        result.contains("Exa Fallback Answer."),
        "expected exa's answer content; got: {result}"
    );
}

/// D-09, asserted end to end: with every provider key cleared for the
/// duration of the test and restored afterwards (never leaking an ambient
/// developer key), the default (unconfigured) chain reaches DDG and the
/// tool returns `Ok`.
#[tokio::test]
async fn no_keys_configured_reaches_ddg() {
    let _g = env_lock().lock().await;
    let _pk = EnvGuard::unset("PERPLEXITY_API_KEY");
    let _ek = EnvGuard::unset("EXA_API_KEY");
    let _bk = EnvGuard::unset("BRAVE_API_KEY");

    let ddg_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"AbstractText":"Example Target Answer.","Answer":"","Definition":""}"#,
        ))
        .expect(1)
        .mount(&ddg_server)
        .await;
    let _de = EnvGuard::set("DDG_API_ENDPOINT_OVERRIDE", &ddg_server.uri());

    let (_cfg_tmp, _home) = write_isolated_home();
    let cfg = Config::load().unwrap_or_default();
    let tool = build_tool(&cfg).await;

    let result = tool
        .execute(json!({ "query": "rust" }))
        .await
        .expect("web_answer must succeed via ddg with zero provider keys configured");

    assert!(result.contains("Example Target Answer."), "got: {result}");
}

/// D-19: the first chain entry's endpoint is mocked with a catch-all
/// matcher expecting ZERO requests, and its key is absent from env, config,
/// AND vault. The second entry is configured and answers. The tool returns
/// the second provider's answer, and the first mock's zero-request
/// expectation verifies on drop — the proof is that no socket was touched,
/// not that a function returned `false`.
#[tokio::test]
async fn an_unconfigured_provider_receives_no_request() {
    let _g = env_lock().lock().await;
    let _pk = EnvGuard::unset("PERPLEXITY_API_KEY");

    let perplexity_server = MockServer::start().await;
    let perplexity_guard = Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&perplexity_server)
        .await;

    let exa_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"output":{"content":"Exa Only Answer.","grounding":[]}}"#,
        ))
        .expect(1)
        .mount(&exa_server)
        .await;

    let _pe = EnvGuard::set("PERPLEXITY_ENDPOINT_OVERRIDE", &perplexity_server.uri());
    let _ee = EnvGuard::set("EXA_ANSWER_ENDPOINT_OVERRIDE", &exa_server.uri());
    let _ek = EnvGuard::set("EXA_API_KEY", "plan08-unconfigured-provider-exa-key");

    // PERPLEXITY_API_KEY: absent from env (unset above), absent from config
    // credentials (empty list below), and no vault store is supplied to
    // build_tool — absent from all three D-19 tiers.
    let (_cfg_tmp, _home) = write_web_answer_config(&["perplexity", "exa"], &[]);
    let cfg = Config::load().unwrap_or_default();
    let tool = build_tool(&cfg).await;

    let result = tool
        .execute(json!({ "query": "rust" }))
        .await
        .expect("web_answer must succeed via exa with perplexity fully unconfigured");
    assert!(result.contains("Exa Only Answer."), "got: {result}");

    // Verifies on drop — panics if the perplexity mock ever received any request.
    drop(perplexity_guard);
}

/// D-19 positive control (without which the previous test would also pass
/// against a tool that can never call anything): the same first entry with
/// its key supplied ONLY through the snapshot's config tier
/// (`tools.credentials`, never process env) receives exactly one request.
#[tokio::test]
async fn a_provider_configured_only_in_the_config_tier_is_called() {
    let _g = env_lock().lock().await;
    let _pk = EnvGuard::unset("PERPLEXITY_API_KEY");

    let perplexity_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(bearer_token("plan08-config-tier-perplexity-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"output":[{"id":"msg_1","type":"message","content":[{"type":"output_text","text":"Config Tier Perplexity Answer."}]}]}"#,
        ))
        .expect(1)
        .mount(&perplexity_server)
        .await;

    let _pe = EnvGuard::set("PERPLEXITY_ENDPOINT_OVERRIDE", &perplexity_server.uri());

    let (_cfg_tmp, _home) = write_web_answer_config(
        &["perplexity"],
        &[("PERPLEXITY_API_KEY", "plan08-config-tier-perplexity-key")],
    );
    let cfg = Config::load().unwrap_or_default();
    let tool = build_tool(&cfg).await;

    let result = tool
        .execute(json!({ "query": "rust" }))
        .await
        .expect("web_answer must succeed using the config-tier-only credential");
    assert!(result.contains("Config Tier Perplexity Answer."), "got: {result}");
}
