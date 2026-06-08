//! Phase 36.14 real stream-path integration tests — PROV-07 SSE-body error fallback.
//!
//! Mocks an HTTP 200 SSE response with an error envelope inside the stream body
//! (the OpenRouter / Qwen pattern documented in phase 36.14 CONTEXT.md) and asserts
//! that LlmClient::chat_completion_stream emits StreamEvent::ProviderError with the
//! correct (NNN Reason) status token. Belt-and-suspenders coverage for the full
//! PROV-07 surface: 400, 429, 500, 502, 503, unknown-code, and string-shaped code.
//!
//! Pattern mirrors tests/streaming_usage_capture.rs. wiremock is already a
//! workspace dev-dependency (crates/ironhermes-agent/Cargo.toml line 57).

use ironhermes_agent::client::{LlmClient, StreamEvent};
use ironhermes_core::ChatMessage;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn drain(rx: &mut tokio::sync::mpsc::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

fn sse_error_body(error_payload: &str) -> String {
    let mut body = String::new();
    body.push_str("data: ");
    body.push_str(error_payload);
    body.push_str("\n\n");
    body.push_str("data: [DONE]\n\n");
    body
}

async fn run_and_drain(error_payload: &str) -> Vec<StreamEvent> {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_error_body(error_payload), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "gpt-4o-mini");
    let messages = vec![ChatMessage::user("hi")];
    let mut rx = client
        .chat_completion_stream(&messages, None, None, None, None, None)
        .await
        .expect("stream should start");
    drain(&mut rx).await
}

fn extract_provider_error(events: &[StreamEvent]) -> Option<&str> {
    events.iter().find_map(|ev| {
        if let StreamEvent::ProviderError(msg) = ev {
            Some(msg.as_str())
        } else {
            None
        }
    })
}

#[tokio::test]
async fn sse_error_400_produces_provider_error_event() {
    let events = run_and_drain(
        r#"{"error":{"message":"qwen/qwen3.7-max is not a valid model ID","code":400}}"#,
    )
    .await;
    let msg = extract_provider_error(&events).expect(
        "phase 36.14 (PROV-07): chat_completion_stream must emit StreamEvent::ProviderError for SSE error envelope",
    );
    assert!(
        msg.contains("(400 Bad Request)"),
        "phase 36.14 (PROV-07): error message must contain '(400 Bad Request)' — got: {}",
        msg
    );
    assert!(
        msg.contains("qwen/qwen3.7-max is not a valid model ID"),
        "phase 36.14 (PROV-07): error message must contain the provider error message — got: {}",
        msg
    );
    assert!(
        events
            .iter()
            .all(|ev| !matches!(ev, StreamEvent::ContentDelta(_))),
        "phase 36.14 (PROV-07): no ContentDelta should be emitted when SSE error is the first event"
    );
}

#[tokio::test]
async fn sse_error_string_code_400_produces_provider_error_event() {
    // Codex MEDIUM #2: string-shaped "code" must work through the real stream path.
    let events = run_and_drain(
        r#"{"error":{"message":"qwen/qwen3.7-max is not a valid model ID","code":"400"}}"#,
    )
    .await;
    let msg = extract_provider_error(&events).expect(
        "phase 36.14 (PROV-07): chat_completion_stream must emit StreamEvent::ProviderError for string-code SSE error envelope",
    );
    assert!(
        msg.contains("(400 Bad Request)"),
        "phase 36.14 (PROV-07): string-code SSE error must contain '(400 Bad Request)' — got: {}",
        msg
    );
}

#[tokio::test]
async fn sse_error_429_produces_provider_error_event() {
    let events = run_and_drain(r#"{"error":{"message":"rate limit exceeded","code":429}}"#).await;
    let msg = extract_provider_error(&events).expect(
        "phase 36.14 (PROV-07): chat_completion_stream must emit StreamEvent::ProviderError for SSE error envelope",
    );
    assert!(
        msg.contains("(429 Too Many Requests)"),
        "phase 36.14 (PROV-07): error message must contain '(429 Too Many Requests)' — got: {}",
        msg
    );
}

#[tokio::test]
async fn sse_error_500_produces_provider_error_event() {
    let events = run_and_drain(r#"{"error":{"message":"internal server error","code":500}}"#).await;
    let msg = extract_provider_error(&events).expect(
        "phase 36.14 (PROV-07): chat_completion_stream must emit StreamEvent::ProviderError for SSE error envelope",
    );
    assert!(
        msg.contains("(500 Internal Server Error)"),
        "phase 36.14 (PROV-07): error message must contain '(500 Internal Server Error)' — got: {}",
        msg
    );
}

#[tokio::test]
async fn sse_error_502_produces_provider_error_event() {
    let events = run_and_drain(r#"{"error":{"message":"bad gateway","code":502}}"#).await;
    let msg = extract_provider_error(&events).expect(
        "phase 36.14 (PROV-07): chat_completion_stream must emit StreamEvent::ProviderError for SSE error envelope",
    );
    assert!(
        msg.contains("(502 Bad Gateway)"),
        "phase 36.14 (PROV-07): error message must contain '(502 Bad Gateway)' — got: {}",
        msg
    );
}

#[tokio::test]
async fn sse_error_503_produces_provider_error_event() {
    let events = run_and_drain(r#"{"error":{"message":"service unavailable","code":503}}"#).await;
    let msg = extract_provider_error(&events).expect(
        "phase 36.14 (PROV-07): chat_completion_stream must emit StreamEvent::ProviderError for SSE error envelope",
    );
    assert!(
        msg.contains("(503 Service Unavailable)"),
        "phase 36.14 (PROV-07): error message must contain '(503 Service Unavailable)' — got: {}",
        msg
    );
}

#[tokio::test]
async fn sse_error_unknown_code_produces_provider_error_event() {
    let events = run_and_drain(r#"{"error":{"message":"upstream failure"}}"#).await;
    let msg = extract_provider_error(&events).expect(
        "phase 36.14 (PROV-07): chat_completion_stream must emit StreamEvent::ProviderError even for unknown-code SSE error envelopes",
    );
    assert!(
        msg.contains("(SSE error)"),
        "phase 36.14 (PROV-07): unknown-code error must contain '(SSE error)' token — got: {}",
        msg
    );
}
