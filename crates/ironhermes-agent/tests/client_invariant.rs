//! Phase 25.1 GAP-7: integration tests proving the pre-send invariant
//! short-circuits BEFORE any HTTP request reaches the wire. Both
//! `chat_completion` and `chat_completion_stream` must reject orphan-pair
//! histories without the wiremock server seeing a single inbound request.

use ironhermes_agent::client::LlmClient;
use ironhermes_core::{ChatMessage, FunctionCall, ToolCall};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build an orphan-pair history (per OpenAI strict semantics):
/// `[user, asst.tool_calls(["x"]), user("next")]` — `x` is never answered
/// before the next user message arrives.
fn orphan_history() -> Vec<ChatMessage> {
    vec![
        ChatMessage::user("hi"),
        ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "x".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "tool_x".to_string(),
                arguments: "{}".to_string(),
            },
        }]),
        ChatMessage::user("next"),
    ]
}

#[tokio::test]
async fn chat_completion_stream_rejects_orphan_history_before_send() {
    // Server would 200-OK any request — but the invariant should short-circuit
    // BEFORE we ever post.
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "gpt-4o-mini");
    let messages = orphan_history();

    let result = client
        .chat_completion_stream(&messages, None, None, None, None, None)
        .await;

    assert!(result.is_err(), "must err on orphan history");
    let err_text = format!("{:#}", result.err().unwrap());
    assert!(
        err_text.contains("tool-call pairing invariant"),
        "diagnostic must surface invariant string; got: {err_text}"
    );
    assert!(
        err_text.contains('x'),
        "diagnostic must reference orphan id 'x'; got: {err_text}"
    );

    // Critical: wiremock saw ZERO requests (short-circuit before network).
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        0,
        "invariant check must short-circuit BEFORE any network call (saw {} requests)",
        requests.len()
    );
}

#[tokio::test]
async fn chat_completion_rejects_orphan_history_before_send() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "gpt-4o-mini");
    let messages = orphan_history();

    let result = client
        .chat_completion(&messages, None, None, None, None, None)
        .await;

    assert!(result.is_err(), "must err on orphan history");
    let err_text = format!("{:#}", result.err().unwrap());
    assert!(
        err_text.contains("tool-call pairing invariant"),
        "diagnostic must surface invariant string; got: {err_text}"
    );
    assert!(
        err_text.contains('x'),
        "diagnostic must reference orphan id 'x'; got: {err_text}"
    );

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        requests.len(),
        0,
        "invariant check must short-circuit BEFORE any network call (saw {} requests)",
        requests.len()
    );
}
