//! Phase 49.7 Plan 03 Task 3 — behavioral contract tests for the lifted
//! judge builder (`ironhermes_agent::judge_builder`).
//!
//! This is ADDITIVE coverage, not a port: `ironhermes-cli/tests/
//! judge_model_resolution.rs` keeps exercising the same behaviors through
//! the `ironhermes-cli` delegating wrappers (Task 2), so the two files
//! together prove both the wrapper hop and the lifted implementation.
//!
//! Mirrors `judge_model_resolution.rs`'s wiremock server setup and `Config`
//! fixture construction (same shapes, `judge_model: &str` instead of
//! `&KanbanConfig`).

use ironhermes_agent::judge_builder::build_runtime_judge_fn;
use ironhermes_core::config::{Config, ProviderConfig};
use ironhermes_core::judge::{JudgeRequest, JudgeVerdict};
use std::collections::HashMap;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a `Config` where the main provider points at a wiremock server's
/// `/chat/completions` endpoint. Mirrors
/// `judge_model_resolution.rs::config_pointing_at_wiremock`.
fn config_pointing_at_wiremock(server: &MockServer) -> Config {
    let mut config = Config::default();
    config.model.provider = "wiremock".to_string();
    config.model.default = "stub-model".to_string();
    let mut providers: HashMap<String, ProviderConfig> = HashMap::new();
    providers.insert(
        "wiremock".to_string(),
        ProviderConfig {
            base_url: Some(server.uri()),
            api_key: Some("test-api-key".to_string()),
            ..Default::default()
        },
    );
    config.providers = providers;
    config
}

fn synthetic_judge_request() -> JudgeRequest {
    JudgeRequest {
        task_id: "t_abc1234".to_string(),
        title: "Translate docs".to_string(),
        body: "Acceptance: every page translated".to_string(),
        worker_turn_output: "I translated 3 of 12 pages so far.".to_string(),
        turn: 1,
    }
}

fn mock_chat_completion_body(content: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 0,
        "model": "stub-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": content,
            },
            "finish_reason": "stop"
        }]
    })
}

// ---------------------------------------------------------------------------
// Behavior 1 — fail-closed empty-api-key bail names both config keys.
// ---------------------------------------------------------------------------

/// Calling `build_runtime_judge_fn` with an empty `judge_model` and a
/// `Config` whose resolved endpoint has no API key returns `Err`, and the
/// message names both `kanban.judge_model` and `auxiliary.kanban_judge` —
/// the fail-closed bail, unchanged by the lift.
#[test]
fn fail_closed_no_api_key_names_both_config_keys() {
    let mut config = Config::default();
    config.model.provider = "openrouter".to_string();
    config.model.default = "".to_string();
    config.providers = HashMap::new();

    // Clear OPENROUTER_API_KEY for the duration of this test so the legacy
    // env fallback doesn't accidentally provide a key — mirrors
    // judge_model_resolution.rs::error_message_names_both_config_keys.
    let saved_or = std::env::var("OPENROUTER_API_KEY").ok();
    // SAFETY: this binary's tests don't share ownership of OPENROUTER_API_KEY
    // mutation with any other test in this crate.
    unsafe {
        std::env::remove_var("OPENROUTER_API_KEY");
    }

    let result = build_runtime_judge_fn("", &config);

    if let Some(val) = saved_or {
        unsafe {
            std::env::set_var("OPENROUTER_API_KEY", val);
        }
    }

    // `JudgeFn` (the Ok type) is `Arc<dyn Fn(...) + Send + Sync>`, which does
    // not implement `Debug`, so `expect_err`/`unwrap_err` (both require `T:
    // Debug`) cannot be used here — match instead.
    let err = match result {
        Ok(_) => panic!("expected Err when no provider key is resolvable"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("kanban.judge_model"),
        "error must name `kanban.judge_model` config key; got: {msg}"
    );
    assert!(
        msg.contains("auxiliary.kanban_judge"),
        "error must name `auxiliary.kanban_judge` config key; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Behavior 2 — Met verdict parses cleanly.
// ---------------------------------------------------------------------------

/// The built closure, pointed at a wiremock endpoint returning
/// `{"verdict":"met","reason":"ok"}`, yields `JudgeOutput { verdict:
/// JudgeVerdict::Met, reason: "ok" }`.
#[tokio::test]
async fn closure_parses_met_verdict() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(mock_chat_completion_body(
            "{\"verdict\":\"met\",\"reason\":\"ok\"}",
        )))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let judge = build_runtime_judge_fn("", &config).expect("build_runtime_judge_fn");
    let out = judge(synthetic_judge_request())
        .await
        .expect("Ok JudgeOutput");
    assert_eq!(out.verdict, JudgeVerdict::Met);
    assert_eq!(out.reason, "ok");
}

// ---------------------------------------------------------------------------
// Behavior 3 — bounded preview on non-JSON body (T-49.7-03-01 mitigation).
// ---------------------------------------------------------------------------

/// The closure pointed at an endpoint returning non-JSON text yields `Err`
/// whose Display contains a preview of that text and whose length is
/// bounded — a 5000-character body does not produce a 5000-character error.
///
/// Mutation check (recorded in the SUMMARY): temporarily replacing the
/// `chars().take(200)` preview in `judge_builder.rs` with the full
/// `raw_text` must make this test FAIL.
#[tokio::test]
async fn non_json_body_bounded_preview() {
    let huge_body = "x".repeat(5000);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(mock_chat_completion_body(&huge_body)),
        )
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let judge = build_runtime_judge_fn("", &config).expect("build_runtime_judge_fn");
    let err = judge(synthetic_judge_request())
        .await
        .expect_err("non-JSON 5000-char body must Err");
    let msg = format!("{err:#}");
    assert!(
        msg.len() < 400,
        "error string must be bounded by the 200-char preview, not proportional \
         to the 5000-char response body; got length {}: {msg:.80}...",
        msg.len()
    );
}

// ---------------------------------------------------------------------------
// Behavior 4 — missing `verdict` field never defaults to a verdict.
// ---------------------------------------------------------------------------

/// The closure pointed at an endpoint returning `{"reason":"x"}` (no
/// `verdict` field) yields `Err`, not a defaulted verdict.
#[tokio::test]
async fn missing_verdict_field_returns_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(mock_chat_completion_body(
            "{\"reason\":\"x\"}",
        )))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let judge = build_runtime_judge_fn("", &config).expect("build_runtime_judge_fn");
    let result = judge(synthetic_judge_request()).await;
    assert!(
        result.is_err(),
        "missing 'verdict' field must yield Err, not a defaulted JudgeOutput"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("missing 'verdict'"),
        "error must name the missing field; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Behavior 5 — verdict literal match is case-sensitive.
// ---------------------------------------------------------------------------

/// The closure pointed at an endpoint returning `{"verdict":"MET"}` (wrong
/// case) yields `Err` — the literal match is case-sensitive.
#[tokio::test]
async fn wrong_case_verdict_returns_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(mock_chat_completion_body(
            "{\"verdict\":\"MET\",\"reason\":\"wrong case\"}",
        )))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let judge = build_runtime_judge_fn("", &config).expect("build_runtime_judge_fn");
    let result = judge(synthetic_judge_request()).await;
    assert!(
        result.is_err(),
        "verdict='MET' (wrong case) must yield Err, not Ok(Met)"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("not in {met,not_met}"),
        "error must name the locked verdict pair; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Behavior 6 — non-empty judge_model selects tier 1.
// ---------------------------------------------------------------------------

/// A non-empty `judge_model` string selects tier 1: the request the
/// wiremock server receives names that model.
#[tokio::test]
async fn tier1_model_selection_names_the_configured_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(serde_json::json!({ "model": "tier1-judge-model" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(mock_chat_completion_body(
            "{\"verdict\":\"met\",\"reason\":\"tier1 model matched\"}",
        )))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let judge =
        build_runtime_judge_fn("tier1-judge-model", &config).expect("build_runtime_judge_fn");
    let out = judge(synthetic_judge_request())
        .await
        .expect("Ok JudgeOutput — proves wiremock matched the tier-1 model in the request body");
    assert_eq!(out.verdict, JudgeVerdict::Met);
}
