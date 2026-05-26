//! Phase 36.15 Plan 04 (PROV-11): wire-body integration tests for
//! extra_request_options TOML knob.
//!
//! Each test uses a wiremock server to capture the exact POST body sent to
//! /chat/completions, then asserts that the flattened extra fields land at the
//! correct JSON path. These tests verify the LAST hop: HashMap-on-AgentLoop →
//! wire (via ChatRequest #[serde(flatten)] extra field in ironhermes-core).
//!
//! D-05 in-scope providers tested here:
//!   - Ollama    — num_ctx = 32768
//!   - vLLM      — top_k = 40
//!   - OpenRouter (non-Claude route) — provider.order = ["anthropic", "openai"]
//!
//! D-09 caller-wins, D-10 mid-session-switch, and reserved-key collision tests
//! ship in Plan 05 and will be appended to this same file.
//!
//! Wiremock harness mirrors crates/ironhermes-agent/tests/streaming_usage_capture.rs.

use ironhermes_agent::client::LlmClient;
use ironhermes_core::ChatMessage;
use std::collections::HashMap;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

// =============================================================================
// Helpers
// =============================================================================

/// Minimal valid ChatResponse JSON so chat_completion returns Ok and we can
/// inspect the captured request body.
fn minimal_chat_response() -> serde_json::Value {
    serde_json::json!({
        "id": "test-id",
        "object": "chat.completion",
        "created": 1,
        "model": "test-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 2,
            "total_tokens": 7
        }
    })
}

/// Assert that the most-recently-received request body (parsed as JSON) contains
/// the value at `json_pointer` (RFC 6901 — e.g. "/num_ctx" or "/provider/order").
async fn assert_request_body_contains(
    server: &MockServer,
    json_pointer: &str,
    expected: serde_json::Value,
) {
    let requests = server
        .received_requests()
        .await
        .expect("wiremock must have captured at least one request");
    let req = requests
        .last()
        .expect("at least one request must have been captured");
    let body: serde_json::Value = serde_json::from_slice(&req.body)
        .expect("request body must be valid JSON");
    let actual = body.pointer(json_pointer).unwrap_or_else(|| {
        panic!(
            "JSON pointer '{}' not found in request body: {}",
            json_pointer,
            serde_json::to_string_pretty(&body).unwrap_or_default()
        )
    });
    assert_eq!(
        actual, &expected,
        "request body field at '{}': expected {:?}, got {:?}",
        json_pointer, expected, actual
    );
}

// =============================================================================
// Tests
// =============================================================================

/// Test 1: Ollama num_ctx = 32768 appears as a top-level field in the POST body.
///
/// This verifies that `HashMap { "num_ctx": json!(32768) }` passed as `extra`
/// to `chat_completion` is flattened into the ChatRequest root via
/// `#[serde(flatten)]` in ironhermes-core's ChatRequest struct.
#[tokio::test]
async fn ollama_num_ctx_appears_in_request_body() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(minimal_chat_response()))
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "llama3.1:8b");
    let messages = vec![ChatMessage::user("hi")];
    let extra = HashMap::from([("num_ctx".to_string(), serde_json::json!(32768u32))]);

    client
        .chat_completion(&messages, None, None, None, None, Some(extra))
        .await
        .expect("request should succeed");

    assert_request_body_contains(&server, "/num_ctx", serde_json::json!(32768u32)).await;
}

/// Test 2: vLLM top_k = 40 appears as a top-level field in the POST body.
///
/// vLLM's OpenAI-compatible endpoint accepts `top_k` as a top-level key in the
/// request JSON alongside standard OpenAI fields.
#[tokio::test]
async fn vllm_top_k_appears_in_request_body() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(minimal_chat_response()))
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "meta-llama/Llama-3-8B");
    let messages = vec![ChatMessage::user("hi")];
    let extra = HashMap::from([("top_k".to_string(), serde_json::json!(40i32))]);

    client
        .chat_completion(&messages, None, None, None, None, Some(extra))
        .await
        .expect("request should succeed");

    assert_request_body_contains(&server, "/top_k", serde_json::json!(40i32)).await;
}

/// Test 3: OpenRouter provider.order = ["anthropic", "openai"] appears nested
/// under the "provider" key in the POST body.
///
/// OpenRouter accepts a `provider` object at the top level of the request body
/// with an `order` array specifying provider preference. The extra HashMap entry
/// `"provider" → json!({"order": ["anthropic", "openai"]})` must survive
/// flattening through ChatRequest.extra and appear as a nested object on the wire.
#[tokio::test]
async fn openrouter_provider_order_nested_in_request_body() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(minimal_chat_response()))
        .mount(&server)
        .await;

    // Use a non-Claude OpenRouter route so this stays in D-05 scope (not D-06).
    let client = LlmClient::new(
        server.uri(),
        "test-key",
        "meta-llama/llama-3.1-8b-instruct",
    );
    let messages = vec![ChatMessage::user("hi")];
    let extra = HashMap::from([(
        "provider".to_string(),
        serde_json::json!({"order": ["anthropic", "openai"]}),
    )]);

    client
        .chat_completion(&messages, None, None, None, None, Some(extra))
        .await
        .expect("request should succeed");

    // Assert the nested provider.order array landed correctly.
    assert_request_body_contains(
        &server,
        "/provider/order",
        serde_json::json!(["anthropic", "openai"]),
    )
    .await;
}
