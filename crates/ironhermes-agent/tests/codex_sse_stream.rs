//! Phase 46.2 — merge `/responses` streaming acceptance proof.
//!
//! REWRITTEN 2026-07-05 for merge's REAL wire format (captured live against
//! `api-gateway.merge.dev`): `data:`-only SSE frames (NO `event:` line), each a
//! CUMULATIVE snapshot of `{object, output:[{finish_reason, content:[...]}], usage}`
//! discriminated by `object` = "response.stream" | "response.done". The prior
//! version mounted an Anthropic-style named-event stream that merge never sends —
//! which is exactly why the reworked parser dropped every frame and delivered
//! empty turns (turn-ended-empty). This test now asserts the emitted StreamEvent
//! sequence for a text + tool_use turn is [ToolCallDelta, ContentDelta, Usage, Done].

use ironhermes_agent::client::StreamEvent;
use ironhermes_agent::codex_client::CodexClient;
use ironhermes_core::ChatMessage;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Frame a sequence of JSON snapshots as merge `data:`-only SSE frames.
fn codex_sse_body(frames: &[&str]) -> String {
    let mut body = String::new();
    for data in frames {
        body.push_str(&format!("data: {data}\n\n"));
    }
    body
}

async fn drain(rx: &mut tokio::sync::mpsc::Receiver<StreamEvent>) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

fn event_label(ev: &StreamEvent) -> &'static str {
    match ev {
        StreamEvent::ContentDelta(_) => "ContentDelta",
        StreamEvent::ToolCallDelta { .. } => "ToolCallDelta",
        StreamEvent::Usage(_) => "Usage",
        StreamEvent::Done(_) => "Done",
        StreamEvent::ProviderError(_) => "ProviderError",
    }
}

#[tokio::test]
async fn codex_sse_stream_emits_expected_event_sequence() {
    let server = MockServer::start().await;

    // merge cumulative snapshots: an initial empty frame, then a tool_use block,
    // then text appended alongside it, then a terminal response.done frame with
    // finish_reason + usage. Each frame repeats the full content-so-far.
    let body = codex_sse_body(&[
        r#"{"object":"response.stream","output":[{"finish_reason":null,"content":[]}],"usage":null}"#,
        r#"{"object":"response.stream","output":[{"finish_reason":null,"content":[{"type":"tool_use","id":"call_xyz","name":"get_weather","input":{"city":"SF"}}]}],"usage":null}"#,
        r#"{"object":"response.stream","output":[{"finish_reason":null,"content":[{"type":"tool_use","id":"call_xyz","name":"get_weather","input":{"city":"SF"}},{"type":"text","text":"Sunny and 72F"}]}],"usage":null}"#,
        r#"{"object":"response.done","output":[{"finish_reason":"tool_use","content":[{"type":"tool_use","id":"call_xyz","name":"get_weather","input":{"city":"SF"}},{"type":"text","text":"Sunny and 72F"}]}],"usage":{"input_tokens":10,"output_tokens":6}}"#,
    ]);

    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(body, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let client = CodexClient::new(server.uri(), "test-key", "anthropic/claude-fable-5");
    let messages = vec![ChatMessage::user("what's the weather in SF?")];

    let mut rx = client
        .chat_completion_stream(&messages, None, None, None, None, None)
        .await
        .expect("stream should start");

    let events = drain(&mut rx).await;
    let labels: Vec<&'static str> = events.iter().map(event_label).collect();

    assert_eq!(
        labels,
        vec!["ContentDelta", "ToolCallDelta", "Usage", "Done"],
        "merge cumulative frames must emit: the text suffix as it streams, then \
         (at finalization) the tool call with COMPLETE args, then usage + done — \
         got: {events:?}"
    );

    // Text IS delivered as it streams (the whole point — pre-fix this was
    // silently dropped, producing turn-ended-empty).
    match &events[0] {
        StreamEvent::ContentDelta(t) => assert_eq!(t, "Sunny and 72F"),
        other => panic!("expected ContentDelta, got {other:?}"),
    }

    // The tool call is emitted ONCE at finalization, fully (id + name + COMPLETE
    // args — merge builds `input` incrementally across frames, so emitting on
    // first sight would carry empty `{}`).
    match &events[1] {
        StreamEvent::ToolCallDelta {
            index,
            id,
            name,
            arguments,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(id.as_deref(), Some("call_xyz"));
            assert_eq!(name.as_deref(), Some("get_weather"));
            assert_eq!(arguments.as_deref(), Some(r#"{"city":"SF"}"#));
        }
        other => panic!("expected full ToolCallDelta, got {other:?}"),
    }

    // Done normalizes merge's finish_reason "tool_use" -> "tool_calls".
    match events.last().expect("non-empty") {
        StreamEvent::Done(reason) => assert_eq!(reason.as_deref(), Some("tool_calls")),
        other => panic!("expected Done, got {other:?}"),
    }
}
