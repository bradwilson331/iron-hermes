//! Phase 36.3.7.12 Plan 03 Task 2 — Three-tier judge_model resolution tests (D-05).
//!
//! Covers the four locked behaviors from `<task name="Task 2">`:
//!
//! 1. Tier 1 — `kanban_config.judge_model = "mistral-large"` wins.
//! 2. Tier 2 — empty judge_model + `model.roles["kanban_judge"]` set →
//!    role's endpoint + model.
//! 3. Tier 3 — empty judge_model + no kanban_judge role → main provider's
//!    default model.
//! 4. Actionable error — when no provider is resolvable, the error message
//!    names BOTH `kanban.judge_model` AND `auxiliary.kanban_judge` so the
//!    operator knows what to set.
//!
//! These tests exercise the `resolve_judge_model_and_endpoint` helper (the
//! production-side resolution fn called by `build_runtime_judge_fn`). They do
//! NOT execute the returned `JudgeFn` closure — that would require a live
//! provider. Closure execution lands in Plan 05 manual UAT.

use ironhermes_cli::kanban::commands::resolve_judge_model_and_endpoint;
use ironhermes_core::config::{Config, ModelRoleConfig, ProviderConfig};
use ironhermes_kanban::KanbanConfig;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Test fixture: minimal Config wired so ProviderResolver::build succeeds with
// an api key on the main provider. We use a hand-crafted `providers` map
// rather than `Config::load()` so the test never touches `$HOME/.ironhermes`.
// ---------------------------------------------------------------------------

fn config_with_main_provider_and_key() -> Config {
    let mut config = Config::default();
    // Set the main provider to "openrouter" with default_model = "main-default".
    config.model.provider = "openrouter".to_string();
    config.model.default = "main-default".to_string();
    // Register the provider with a literal api_key so ProviderResolver::build
    // hydrates the endpoint with a non-empty key.
    let mut providers: HashMap<String, ProviderConfig> = HashMap::new();
    providers.insert(
        "openrouter".to_string(),
        ProviderConfig {
            api_key: Some("test-api-key".to_string()),
            ..Default::default()
        },
    );
    config.providers = providers;
    config
}

// ---------------------------------------------------------------------------
// Tier 1 — explicit kanban_config.judge_model wins
// ---------------------------------------------------------------------------

#[test]
fn tier_1_judge_model_explicit() {
    let kanban = KanbanConfig {
        judge_model: "mistral-large".to_string(),
        ..KanbanConfig::default()
    };
    let main = config_with_main_provider_and_key();
    let (endpoint, resolved_model) =
        resolve_judge_model_and_endpoint(&kanban, &main).expect("tier-1 resolution");
    assert_eq!(
        resolved_model, "mistral-large",
        "tier-1: explicit kanban.judge_model must win"
    );
    // Endpoint is whatever the main provider points to.
    assert!(
        !endpoint.base_url.is_empty(),
        "tier-1: main endpoint must carry a base_url"
    );
}

// ---------------------------------------------------------------------------
// Tier 2 — model.roles["kanban_judge"] override
// ---------------------------------------------------------------------------

#[test]
fn tier_2_kanban_judge_role() {
    let kanban = KanbanConfig::default(); // judge_model = ""
    let mut main = config_with_main_provider_and_key();
    // Register a `kanban_judge` role pointing at the main provider with
    // a model override.
    let mut roles: HashMap<String, ModelRoleConfig> = HashMap::new();
    roles.insert(
        "kanban_judge".to_string(),
        ModelRoleConfig {
            provider: "main".to_string(),
            model: Some("claude-haiku".to_string()),
        },
    );
    main.model.roles = roles;

    let (_endpoint, resolved_model) =
        resolve_judge_model_and_endpoint(&kanban, &main).expect("tier-2 resolution");
    assert_eq!(
        resolved_model, "claude-haiku",
        "tier-2: kanban_judge role override must win when kanban.judge_model is empty"
    );
}

// ---------------------------------------------------------------------------
// Tier 3 — fallback to main provider's default model
// ---------------------------------------------------------------------------

#[test]
fn tier_3_fallback_to_main_default() {
    let kanban = KanbanConfig::default(); // judge_model = ""
    // No kanban_judge role.
    let main = config_with_main_provider_and_key();

    let (_endpoint, resolved_model) =
        resolve_judge_model_and_endpoint(&kanban, &main).expect("tier-3 resolution");
    assert_eq!(
        resolved_model, "main-default",
        "tier-3: when neither kanban.judge_model nor kanban_judge role is set, main default wins"
    );
}

// ---------------------------------------------------------------------------
// Error path — no API key available; message names both config keys
// ---------------------------------------------------------------------------

#[test]
fn error_message_names_both_config_keys() {
    // Pathological config: main provider has no api_key at all.
    let mut config = Config::default();
    config.model.provider = "openrouter".to_string();
    config.model.default = "".to_string();
    // No api_key resolved (no providers map, no env vars set for this run).
    // To make this deterministic across CI environments that have OPENROUTER_API_KEY
    // exported, we don't rely on the legacy env path here — instead we use a
    // disabled custom provider name so the resolver yields an empty key.
    config.providers = HashMap::new();

    // Clear OPENROUTER_API_KEY for the duration of this test so the legacy env
    // fallback doesn't accidentally provide a key. SAFETY: only this test
    // process mutates env; concurrent tests in other crates are isolated.
    // (env mutation behind a guard would be ideal but is overkill — the test
    // only fails closed if the message check is wrong, never with a false-pass.)
    let saved_or = std::env::var("OPENROUTER_API_KEY").ok();
    // SAFETY: tests in this binary are not run concurrently for env mutation
    // sensitivity; the kanban-tools `ENV_LOCK` is only required for tests that
    // mutate kanban-task env vars (HERMES_KANBAN_*). OPENROUTER_API_KEY has no
    // such cross-test owner in this binary.
    unsafe {
        std::env::remove_var("OPENROUTER_API_KEY");
    }

    let kanban = KanbanConfig::default();
    let result = resolve_judge_model_and_endpoint(&kanban, &config);

    // Restore env before any assertion can panic.
    if let Some(val) = saved_or {
        unsafe {
            std::env::set_var("OPENROUTER_API_KEY", val);
        }
    }

    let err = result.expect_err("expected Err when no provider key is resolvable");
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
// Compile anchor — build_runtime_judge_fn is reachable as pub
// (Plan 04's loop wrapper will import it from the same crate.) We can't call
// it without a runtime provider, but referencing the fn name in a no-op
// closure ensures the symbol exists at the expected path.
// ---------------------------------------------------------------------------

#[test]
fn build_runtime_judge_fn_symbol_exists() {
    // Just verify the symbol is reachable. The actual factory invocation
    // requires the same fixture as tier_1 above; we don't call it here to
    // avoid an extra LLM-shaped allocation in the unit-test path.
    fn _assert_callable(
        _kanban: &ironhermes_kanban::KanbanConfig,
        _main: &ironhermes_core::config::Config,
    ) -> anyhow::Result<ironhermes_kanban::JudgeFn> {
        ironhermes_cli::kanban::commands::build_runtime_judge_fn(_kanban, _main)
    }
}

// ===========================================================================
// Closure-execution tests — wiremock the LLM endpoint to verify the JudgeFn
// closure's JSON-parse + verdict-match semantics (T-36.3.7.12-03-T01 / T02 /
// I01 mitigation rails). These tests build a real JudgeFn against a fake
// provider, invoke it once with a synthetic JudgeRequest, and assert the
// closure's behavior on the three load-bearing response shapes:
//   - valid {"verdict":"met","reason":"..."} → Ok(Met, reason)
//   - verdict="MAYBE" (case-sensitive miss) → Err
//   - non-JSON body → Err with raw snippet
// ===========================================================================

use ironhermes_core::config::ProviderConfig as Provider2; // alias to keep tier_* using config::ProviderConfig naming
use ironhermes_kanban::{JudgeRequest, JudgeVerdict};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a Config where the main provider points at a wiremock server's
/// `/chat/completions` endpoint. Returns the (Config, server_uri) pair.
fn config_pointing_at_wiremock(server: &MockServer) -> Config {
    let mut config = Config::default();
    config.model.provider = "wiremock".to_string();
    config.model.default = "stub-model".to_string();
    let mut providers: HashMap<String, Provider2> = HashMap::new();
    providers.insert(
        "wiremock".to_string(),
        Provider2 {
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

/// LLM returns `{"verdict":"met","reason":"all criteria satisfied"}` →
/// closure returns Ok(Met, "all criteria satisfied").
#[tokio::test]
async fn closure_parses_met_verdict() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 0,
            "model": "stub-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"verdict\":\"met\",\"reason\":\"all criteria satisfied\"}"
                },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let kanban = KanbanConfig::default();
    let judge = ironhermes_cli::kanban::commands::build_runtime_judge_fn(&kanban, &config)
        .expect("build_runtime_judge_fn");
    let out = judge(synthetic_judge_request()).await.expect("Ok JudgeOutput");
    assert_eq!(out.verdict, JudgeVerdict::Met);
    assert_eq!(out.reason, "all criteria satisfied");
}

/// LLM returns `{"verdict":"not_met","reason":"missing criterion 3"}` →
/// closure returns Ok(NotMet, "missing criterion 3").
#[tokio::test]
async fn closure_parses_not_met_verdict() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 0,
            "model": "stub-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"verdict\":\"not_met\",\"reason\":\"missing criterion 3\"}"
                },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let kanban = KanbanConfig::default();
    let judge = ironhermes_cli::kanban::commands::build_runtime_judge_fn(&kanban, &config)
        .expect("build_runtime_judge_fn");
    let out = judge(synthetic_judge_request()).await.expect("Ok JudgeOutput");
    assert_eq!(out.verdict, JudgeVerdict::NotMet);
    assert_eq!(out.reason, "missing criterion 3");
}

/// T-36.3.7.12-03-T02 — verdict="MAYBE" (or any string outside the locked
/// {"met","not_met"} pair) MUST return Err. Case-sensitive match: even
/// "MET" or "yes" fail.
#[tokio::test]
async fn unknown_verdict_returns_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 0,
            "model": "stub-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"verdict\":\"MAYBE\",\"reason\":\"unclear\"}"
                },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let kanban = KanbanConfig::default();
    let judge = ironhermes_cli::kanban::commands::build_runtime_judge_fn(&kanban, &config)
        .expect("build_runtime_judge_fn");
    let err = judge(synthetic_judge_request())
        .await
        .expect_err("verdict='MAYBE' must Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not in {met,not_met}"),
        "error msg must name the locked verdict pair; got: {msg}"
    );
    assert!(
        msg.contains("MAYBE"),
        "error msg must include the offending verdict; got: {msg}"
    );
}

/// T-36.3.7.12-03-T01 — non-JSON LLM body MUST return Err. Raw snippet
/// truncated to 200 chars (T-36.3.7.12-03-I01 V8 mitigation).
#[tokio::test]
async fn non_json_body_returns_error_with_snippet() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 0,
            "model": "stub-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "this is not JSON at all"
                },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let kanban = KanbanConfig::default();
    let judge = ironhermes_cli::kanban::commands::build_runtime_judge_fn(&kanban, &config)
        .expect("build_runtime_judge_fn");
    let err = judge(synthetic_judge_request())
        .await
        .expect_err("non-JSON body must Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not JSON"),
        "error msg must indicate parse failure; got: {msg}"
    );
    assert!(
        msg.contains("this is not JSON at all"),
        "error msg must include the raw snippet for operator triage; got: {msg}"
    );
}

/// T-36.3.7.12-03-T01 — well-formed JSON missing the `verdict` field
/// MUST return Err.
#[tokio::test]
async fn missing_verdict_field_returns_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 0,
            "model": "stub-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"reason\":\"no verdict here\"}"
                },
                "finish_reason": "stop"
            }]
        })))
        .mount(&server)
        .await;

    let config = config_pointing_at_wiremock(&server);
    let kanban = KanbanConfig::default();
    let judge = ironhermes_cli::kanban::commands::build_runtime_judge_fn(&kanban, &config)
        .expect("build_runtime_judge_fn");
    let err = judge(synthetic_judge_request())
        .await
        .expect_err("missing verdict field must Err");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("missing 'verdict'"),
        "error msg must name the missing field; got: {msg}"
    );
}
