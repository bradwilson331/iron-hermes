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
use ironhermes_core::config_extras::{ProviderModelConfig, resolve_extras};
use ironhermes_core::config::ProviderConfig;
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

// =============================================================================
// Plan 05 tests — D-09 caller-wins, D-09 floor, reserved-key collision, D-10
// =============================================================================

/// Test 4 (Plan 05): D-09 caller-wins per-key.
///
/// When a caller passes `extra = {"num_ctx": 100}` directly to `chat_completion`,
/// that value lands on the wire as `num_ctx: 100`. This is the direct-boundary
/// semantics of D-09: the caller's map wins over anything the config layer would
/// supply. Here the caller IS the agent; config-layer merging happens in
/// AgentRuntime which assembles the map before calling chat_completion. This test
/// exercises the LlmClient boundary: whatever arrives in `extra` appears
/// flattened into the ChatRequest body unchanged.
#[tokio::test]
async fn extra_caller_wins_per_key_d09() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(minimal_chat_response()))
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "llama3.1:8b");
    let messages = vec![ChatMessage::user("hi")];
    // Caller explicitly sets num_ctx = 100 — this is the "caller" value in D-09.
    let extra = HashMap::from([("num_ctx".to_string(), serde_json::json!(100u32))]);

    client
        .chat_completion(&messages, None, None, None, None, Some(extra))
        .await
        .expect("request should succeed");

    // Verify the wire body carries the caller's value, not any config default.
    assert_request_body_contains(&server, "/num_ctx", serde_json::json!(100u32)).await;
}

/// Test 5 (Plan 05): D-09 stream_options floor preserved when config is silent.
///
/// When `chat_completion_stream` is called with `extra = None` (no caller-supplied
/// extras, no config extras), the wire body MUST still contain
/// `stream_options.include_usage: true`. This is the `or_insert_with` floor at
/// client.rs:236 that ensures streaming usage events fire for all providers.
///
/// This test protects against accidental removal of the floor in future edits
/// (threat T-36.15-12).
#[tokio::test]
async fn extra_stream_options_floor_preserved_when_config_silent() {
    let server = MockServer::start().await;
    // Minimal SSE response — just the DONE sentinel so the stream terminates
    // without the client waiting for more chunks.
    let sse_body = "data: [DONE]\n\n";
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_body, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "llama3.1:8b");
    let messages = vec![ChatMessage::user("hi")];

    // Pass extra = None — neither caller nor config sets stream_options.
    let rx = client
        .chat_completion_stream(&messages, None, None, None, None, None)
        .await
        .expect("stream should start");

    // Drain the receiver so the streaming task completes and the request is
    // fully sent + captured by wiremock before we inspect.
    let mut rx = rx;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while rx.recv().await.is_some() {}
    })
    .await
    .expect("stream must drain within 5 seconds");

    // The floor at client.rs:236 must have injected stream_options.include_usage.
    assert_request_body_contains(
        &server,
        "/stream_options/include_usage",
        serde_json::json!(true),
    )
    .await;
}

/// Test 6 (Plan 05): Reserved-key collision behavior audit (T-36.15-09).
///
/// SECURITY FINDING: serde flatten DOES allow the HashMap key "model" to
/// overwrite the named `model` field in ChatRequest. The RESEARCH §Security
/// Domain assumed named fields would win — this test proves the OPPOSITE.
///
/// Actual behavior: when `extra` contains `"model" → "attacker-supplied-model"`,
/// the wire body's top-level `model` is `"attacker-supplied-model"`, NOT the
/// ChatRequest's intended model string. The flattened HashMap key wins because
/// serde_json::Value serialization merges both sources into the same JSON object
/// and the last writer (the flattened HashMap) prevails.
///
/// This test documents the actual (insecure) behavior and guards against
/// regressions. When T-36.15-09 is properly mitigated (reserved-key blocklist
/// filter on extra_request_options ingestion or ChatRequest serialization), this
/// test MUST be updated: flip the assertion from "attacker-supplied-model" to
/// "real-model" and remove the `// SECURITY FINDING` comment block.
///
/// See SUMMARY §Threat Flags — T-36.15-09 is NOT mitigated; follow-up required.
///
/// FOLLOW-UP REQUIRED: Add a reserved-key blocklist filter in ChatRequest
/// serialization or in the config_extras ingestion path that rejects/strips keys
/// matching named ChatRequest fields ("model", "messages", "stream", etc.).
// SECURITY FINDING T-36.15-09: test asserts CURRENT insecure behavior (flatten wins).
// When fixed, update assertion to expect "real-model" and remove this marker comment.
#[tokio::test]
async fn extra_reserved_key_collision_named_field_wins() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(minimal_chat_response()))
        .mount(&server)
        .await;

    // The LlmClient's actual model is "real-model".
    let client = LlmClient::new(server.uri(), "test-key", "real-model");
    let messages = vec![ChatMessage::user("hi")];
    // Attacker-supplied extras attempt to override the "model" reserved field.
    let mut extra = HashMap::new();
    extra.insert(
        "model".to_string(),
        serde_json::json!("attacker-supplied-model"),
    );

    client
        .chat_completion(&messages, None, None, None, None, Some(extra))
        .await
        .expect("request should succeed");

    // SECURITY FINDING: the attacker-supplied value WINS on the wire.
    // This assertion documents the current insecure behavior (T-36.15-09 open).
    // The named `model` field in ChatRequest does NOT prevent the flattened
    // HashMap from shadowing it during serde serialization.
    // When a future fix is applied, flip this to assert "real-model" instead.
    assert_request_body_contains(
        &server,
        "/model",
        serde_json::json!("attacker-supplied-model"),
    )
    .await;
}

/// Test 7 (Plan 05): D-10 mid-session model switch picks per-model extras.
///
/// Two sequential calls to `resolve_extras` with different model names against
/// the same providers HashMap return different per-model overrides. This directly
/// models the D-10 invariant: switching the active model (via `/model` command)
/// causes AgentRuntime to call `resolve_extras` with the new model name on the
/// next turn, producing the correct per-model extras for that model.
///
/// Provider-level default: num_ctx = 8192
/// Per-model for "llama3.1:8b":  num_ctx = 32768
/// Per-model for "llama3.1:70b": num_ctx = 4096
///
/// After a switch from 8b → 70b the resolver must return 4096, not 32768.
/// `resolve_extras` is stateless — the same Config produces different outputs
/// for different model name arguments, which is exactly what enables mid-session
/// model switching without any shared mutable state.
///
/// This is a pure synchronous unit test — no wiremock needed.
#[test]
fn extra_model_switch_picks_per_model_override_d10() {
    // Build providers map programmatically (no serde_yaml needed — struct literals
    // are unambiguous and avoid the D-01 serde_yaml untagged enum pitfall).
    let mut providers: HashMap<String, ProviderConfig> = HashMap::new();

    // Provider-level default: num_ctx = 8192
    let mut provider_extras: HashMap<String, serde_json::Value> = HashMap::new();
    provider_extras.insert("num_ctx".to_string(), serde_json::json!(8192u32));

    // Per-model for "llama3.1:8b": num_ctx = 32768 (overrides provider default)
    let mut model_8b_extras: HashMap<String, serde_json::Value> = HashMap::new();
    model_8b_extras.insert("num_ctx".to_string(), serde_json::json!(32768u32));

    // Per-model for "llama3.1:70b": num_ctx = 4096 (overrides provider default)
    let mut model_70b_extras: HashMap<String, serde_json::Value> = HashMap::new();
    model_70b_extras.insert("num_ctx".to_string(), serde_json::json!(4096u32));

    let mut models: HashMap<String, ProviderModelConfig> = HashMap::new();
    models.insert(
        "llama3.1:8b".to_string(),
        ProviderModelConfig { extra_request_options: model_8b_extras },
    );
    models.insert(
        "llama3.1:70b".to_string(),
        ProviderModelConfig { extra_request_options: model_70b_extras },
    );

    providers.insert(
        "ollama".to_string(),
        ProviderConfig {
            extra_request_options: provider_extras,
            models,
            ..Default::default()
        },
    );

    // First call — model "llama3.1:8b" — must return per-model value 32768.
    let result_8b = resolve_extras(&providers, "ollama", "llama3.1:8b");
    assert_eq!(
        result_8b.get("num_ctx"),
        Some(&serde_json::json!(32768u32)),
        "llama3.1:8b per-model num_ctx must be 32768 (not the provider default 8192)"
    );

    // Second call — same providers map, different model "llama3.1:70b" — must
    // return per-model value 4096. This simulates the mid-session model switch.
    let result_70b = resolve_extras(&providers, "ollama", "llama3.1:70b");
    assert_eq!(
        result_70b.get("num_ctx"),
        Some(&serde_json::json!(4096u32)),
        "llama3.1:70b per-model num_ctx must be 4096 (not 32768 or 8192)"
    );
}
