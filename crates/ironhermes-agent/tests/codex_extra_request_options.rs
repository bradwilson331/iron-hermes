//! Phase 46.2 Plan 01 Task 3 (D-02/D-05 acceptance proof), REWORKED 2026-07-04
//! for merge's real Anthropic-flavored `/responses` schema (D-07 UAT finding).
//!
//! Mounts a wiremock server, drives one `CodexClient::chat_completion` call with
//! an `extra` map of `{project_id, include_routing_metadata}` plus a tool call +
//! tool result in the messages, and asserts the captured request body has the
//! flattened extras, top-level `model`/`temperature`/`max_tokens`, NO `store`,
//! NO `instructions`, an assistant `message` item whose content is a block array
//! carrying a `tool_use` block `{id, name, input:{...}}`, and a TOP-LEVEL
//! `tool_result` item keyed by `tool_use_id`.

use ironhermes_agent::CodexClient;
use ironhermes_core::{ChatMessage, FunctionCall, Role, ToolCall};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Minimal valid merge (Anthropic-flavored) response body so chat_completion
/// returns Ok and we can inspect the captured request body.
fn minimal_codex_response() -> serde_json::Value {
    serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "text", "text": "72F sunny" }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 10, "output_tokens": 5 }
    })
}

/// Assert that the most-recently-received request body (parsed as JSON) contains
/// the value at `json_pointer` (RFC 6901 — e.g. "/project_id" or "/input/0/type").
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
    let body: serde_json::Value =
        serde_json::from_slice(&req.body).expect("request body must be valid JSON");
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

/// Assert a top-level request field is ABSENT (e.g. no `store`/`instructions`).
async fn assert_request_body_absent(server: &MockServer, json_pointer: &str) {
    let requests = server
        .received_requests()
        .await
        .expect("wiremock must have captured at least one request");
    let req = requests.last().expect("at least one request captured");
    let body: serde_json::Value =
        serde_json::from_slice(&req.body).expect("request body must be valid JSON");
    assert!(
        body.pointer(json_pointer).is_none(),
        "expected '{}' to be absent, but it was present in: {}",
        json_pointer,
        serde_json::to_string_pretty(&body).unwrap_or_default()
    );
}

#[tokio::test]
async fn codex_request_flattens_extras_and_round_trips_tool_call_id() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(minimal_codex_response()))
        .mount(&server)
        .await;

    let client = CodexClient::new(server.uri(), "test-key", "anthropic/claude-fable-5");

    let tool_calls = vec![ToolCall {
        id: "call_abc123".to_string(),
        call_type: "function".to_string(),
        function: FunctionCall {
            name: "get_weather".to_string(),
            arguments: r#"{"city":"SF"}"#.to_string(),
        },
    }];
    let messages = vec![
        ChatMessage::user("What's the weather in SF?"),
        ChatMessage {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        },
        ChatMessage::tool_result("call_abc123", "72F sunny"),
    ];

    let mut extra = std::collections::HashMap::new();
    extra.insert(
        "project_id".to_string(),
        serde_json::json!("85c3d7ba-project"),
    );
    extra.insert(
        "include_routing_metadata".to_string(),
        serde_json::json!(true),
    );

    client
        .chat_completion(&messages, None, None, None, Some(0.5), Some(extra))
        .await
        .expect("request should succeed");

    // D-02: top-level model/temperature/max_tokens/extras flatten; no store/instructions.
    assert_request_body_contains(
        &server,
        "/model",
        serde_json::json!("anthropic/claude-fable-5"),
    )
    .await;
    assert_request_body_contains(&server, "/temperature", serde_json::json!(0.5)).await;
    assert_request_body_contains(&server, "/max_tokens", serde_json::json!(4096)).await;
    assert_request_body_contains(
        &server,
        "/project_id",
        serde_json::json!("85c3d7ba-project"),
    )
    .await;
    assert_request_body_contains(
        &server,
        "/include_routing_metadata",
        serde_json::json!(true),
    )
    .await;
    assert_request_body_absent(&server, "/store").await;
    assert_request_body_absent(&server, "/instructions").await;
    assert_request_body_absent(&server, "/max_output_tokens").await;

    // D-05: the tool call_id round-trips through a tool_use block -> tool_result.
    // [0] user message, [1] assistant message (tool_use block array), [2] tool_result.
    assert_request_body_contains(&server, "/input/1/type", serde_json::json!("message")).await;
    assert_request_body_contains(&server, "/input/1/role", serde_json::json!("assistant")).await;
    assert_request_body_contains(
        &server,
        "/input/1/content/0/type",
        serde_json::json!("tool_use"),
    )
    .await;
    assert_request_body_contains(
        &server,
        "/input/1/content/0/id",
        serde_json::json!("call_abc123"),
    )
    .await;
    assert_request_body_contains(
        &server,
        "/input/1/content/0/name",
        serde_json::json!("get_weather"),
    )
    .await;
    // input must be a parsed object, not an arguments string.
    assert_request_body_contains(
        &server,
        "/input/1/content/0/input",
        serde_json::json!({"city": "SF"}),
    )
    .await;

    assert_request_body_contains(&server, "/input/2/type", serde_json::json!("tool_result")).await;
    assert_request_body_contains(
        &server,
        "/input/2/tool_use_id",
        serde_json::json!("call_abc123"),
    )
    .await;
    assert_request_body_contains(&server, "/input/2/content", serde_json::json!("72F sunny")).await;
}
