//! Phase 36.2 Plan 07 regression net — streaming usage capture.
//!
//! Locks the ordering contract between SSE chunks in OpenAI-spec streaming:
//! when `stream_options.include_usage: true` is honored, providers deliver
//! the per-stream `usage` payload in a SEPARATE chunk that arrives AFTER
//! the chunk carrying `finish_reason`. The producer in `client.rs` must
//! defer `StreamEvent::Done` emission until either the trailing `[DONE]`
//! sentinel or natural stream end so the trailing usage chunk is forwarded
//! to the consumer BEFORE Done breaks the consumer loop.
//!
//! Pre-fix behavior: `Done` was emitted immediately on `finish_reason`
//! followed by an early `return` — the trailing usage chunk was dropped on
//! the floor, every `usage_events` row was written with all-zero token
//! counts, and the TUI / web token pills stayed stranded at 0/128k even
//! when the provider returned real numbers (confirmed against OpenRouter
//! generation traces showing `tokens_prompt: 65921 / tokens_completion: 17`
//! being lost).

use ironhermes_agent::client::{LlmClient, StreamEvent};
use ironhermes_core::ChatMessage;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Drain the receiver into a Vec — the test then asserts on the sequence.
async fn drain(rx: &mut tokio::sync::mpsc::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

/// SSE response that interleaves content deltas, a finish_reason chunk
/// (no usage), a SEPARATE usage chunk (no choices), and the `[DONE]`
/// sentinel — exactly the shape OpenAI and OpenRouter emit when
/// `stream_options.include_usage: true` is set.
fn sse_with_separated_usage_chunk() -> String {
    // Each `data: ...\n\n` is one SSE event. Order matters: deltas → finish
    // chunk → usage chunk → [DONE].
    let chunks = [
        // 1. Content delta
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":"Hello"}}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":" world"}}]}"#,
        // 2. finish_reason chunk — choices populated, NO usage on this one.
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        // 3. Separate trailing usage chunk — empty choices, usage populated.
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120}}"#,
    ];

    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// SSE response that bundles `finish_reason` AND `usage` into the SAME
/// chunk (Anthropic's gateway-translated shape; some providers do this).
/// The producer must still forward both Usage and Done.
fn sse_with_combined_finish_and_usage_chunk() -> String {
    let chunks = [
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":"Hi"}}]}"#,
        // Combined: choices carries finish_reason, top-level carries usage.
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"total_tokens":60}}"#,
    ];

    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// SSE response that omits `[DONE]` — natural stream end via byte-stream
/// completion. The producer must emit `Done(pending_finish_reason)` before
/// dropping `tx` so the consumer's Done arm fires once.
fn sse_without_done_sentinel() -> String {
    let chunks = [
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{"content":"X"}}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"gpt-4o-mini","choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}"#,
    ];

    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    // No [DONE] sentinel — wiremock will close the connection naturally.
    body
}

#[tokio::test]
async fn separated_usage_chunk_is_forwarded_before_done() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_with_separated_usage_chunk(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "gpt-4o-mini");
    let messages = vec![ChatMessage::user("hi")];
    let mut rx = client
        .chat_completion_stream(&messages, None, None, None, None, None)
        .await
        .expect("stream should start");

    let events = drain(&mut rx).await;

    // Find Usage and Done positions; assert Usage came first.
    let usage_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::Usage(_)))
        .expect(
            "Phase 36.2 Plan 07 regression: producer dropped the trailing usage chunk \
             after finish_reason — Usage event missing from stream",
        );
    let done_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::Done(_)))
        .expect("Done event missing from stream");

    assert!(
        usage_pos < done_pos,
        "Phase 36.2 Plan 07 regression: Usage must arrive BEFORE Done so the consumer \
         (`agent_loop::call_llm_streaming`) captures it before its `Done => break` fires. \
         got Usage at index {usage_pos}, Done at index {done_pos}: {events:?}"
    );

    // Verify the usage payload survived intact.
    if let StreamEvent::Usage(u) = &events[usage_pos] {
        assert_eq!(u.prompt_tokens, 100, "prompt_tokens lost in transit");
        assert_eq!(u.completion_tokens, 20, "completion_tokens lost in transit");
        assert_eq!(u.total_tokens, 120, "total_tokens lost in transit");
    }

    // Verify the finish_reason survived the deferral.
    if let StreamEvent::Done(reason) = &events[done_pos] {
        assert_eq!(
            reason.as_deref(),
            Some("stop"),
            "stashed finish_reason must be forwarded with the deferred Done event"
        );
    }
}

#[tokio::test]
async fn combined_finish_and_usage_chunk_emits_both_events() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(
                    sse_with_combined_finish_and_usage_chunk(),
                    "text/event-stream",
                ),
        )
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "gpt-4o-mini");
    let messages = vec![ChatMessage::user("hi")];
    let mut rx = client
        .chat_completion_stream(&messages, None, None, None, None, None)
        .await
        .expect("stream should start");

    let events = drain(&mut rx).await;

    let usage_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::Usage(_)))
        .expect("combined chunk must still emit Usage");
    let done_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::Done(_)))
        .expect("combined chunk must still emit Done");

    // Per producer ordering: Usage is emitted first because we check
    // `chunk.usage` before iterating `chunk.choices`.
    assert!(
        usage_pos < done_pos,
        "Usage must precede Done even when bundled in the same chunk"
    );

    if let StreamEvent::Usage(u) = &events[usage_pos] {
        assert_eq!(u.total_tokens, 60, "combined-chunk usage payload corrupted");
    }
}

#[tokio::test]
async fn natural_stream_end_without_done_sentinel_still_emits_done() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_without_done_sentinel(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-key", "gpt-4o-mini");
    let messages = vec![ChatMessage::user("hi")];
    let mut rx = client
        .chat_completion_stream(&messages, None, None, None, None, None)
        .await
        .expect("stream should start");

    let events = drain(&mut rx).await;

    let usage_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::Usage(_)))
        .expect("usage chunk before stream-end must be forwarded");
    let done_pos = events
        .iter()
        .position(|e| matches!(e, StreamEvent::Done(_)))
        .expect(
            "producer must emit Done at natural stream end so the consumer's \
             `Done => break` arm fires (the channel-close fallback used to work too \
             but Done is the explicit contract)",
        );

    assert!(
        usage_pos < done_pos,
        "Usage must precede Done at stream end"
    );

    if let StreamEvent::Done(reason) = &events[done_pos] {
        assert_eq!(
            reason.as_deref(),
            Some("tool_calls"),
            "stashed finish_reason must survive the natural stream end"
        );
    }
}
