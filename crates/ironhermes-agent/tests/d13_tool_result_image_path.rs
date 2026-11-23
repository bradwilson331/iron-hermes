//! Phase 49.8 D-13 spike evidence — RESEARCH.md Pitfall 2.
//!
//! Pins what a `data:image/...;base64,` **tool result** string actually becomes in the
//! outgoing provider request, for both the Anthropic adapter and the OpenAI-compatible
//! `ChatMessage` serialization, plus the fact that `agent_loop.rs` wraps every tool's
//! `String` result as a plain-text tool result (never an image content part).
//!
//! RESEARCH.md found that `hexapod_video`'s "vision path" — the precedent the ros2 camera
//! tools were originally modeled on — is NOT wired downstream: `Tool::execute()` can only
//! return a `String`, `anthropic_client.rs::convert_image_url_to_block` exists but is only
//! reached for `ContentPart::ImageUrl` parts in System/User messages, never for a tool
//! result's `Role::Tool` arm. This test turns that research finding into a regression-pinned
//! fact backing the phase's recorded D-13 mechanism decision (see 49.8-03-PLAN.md's "D-13
//! mechanism decision" section): images reach the model via the `browser_vision` pattern (an
//! internal vision-model call returning text), never as a raw data-URI tool result.
//!
//! **If any assertion below fails because an image content block IS now produced for a tool
//! result, STOP** — the 49.8 D-13 decision is invalid and must go back to the orchestrator
//! before the image-path plans (49.8-06 conversion, 49.8-08 wiring) execute. Do not edit these
//! assertions to make them pass; a failure here means the mechanism this phase decided on no
//! longer matches reality.

use ironhermes_agent::anthropic_client::{build_anthropic_request_for_test, serialize_request_for_test};
use ironhermes_core::config::PromptCachingConfig;
use ironhermes_core::types::{ChatMessage, FunctionCall, ToolCall};

const DATA_URI: &str = "data:image/png;base64,iVBORw0KGgo=";

#[test]
fn data_uri_tool_result_reaches_anthropic_as_text() {
    let tool_call_id = "toolu_01abc123";
    let messages = vec![
        ChatMessage::user("look"),
        ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: tool_call_id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "ros2".to_string(),
                arguments: "{}".to_string(),
            },
        }]),
        ChatMessage::tool_result(tool_call_id, DATA_URI),
    ];

    let request = build_anthropic_request_for_test(&messages, "claude-test", &PromptCachingConfig::default());
    let json = serialize_request_for_test(&request).expect("serialize anthropic request");

    assert!(
        json.contains("\"type\":\"tool_result\""),
        "expected a tool_result content block in the serialized request, got: {json}"
    );
    assert!(
        json.contains(DATA_URI),
        "expected the literal data URI to appear as a plain string in the tool_result content, got: {json}"
    );
    assert!(
        !json.contains("\"type\":\"image\""),
        "expected NO image content block to be produced for a tool result — if this fails, the D-13 \
         mechanism decision (browser_vision pattern) is invalid and must be revisited: {json}"
    );
}

#[test]
fn data_uri_tool_result_serializes_as_text_for_openai_compat() {
    let tool_call_id = "call_xyz789";
    let msg = ChatMessage::tool_result(tool_call_id, DATA_URI);

    let value = serde_json::to_value(&msg).expect("serialize ChatMessage");
    let content = value
        .get("content")
        .expect("tool_result ChatMessage must serialize a content field");

    assert_eq!(
        content,
        &serde_json::Value::String(DATA_URI.to_string()),
        "expected content to be the plain JSON string of the data URI, not an array or an \
         image_url part: {value}"
    );
    assert!(
        !content.is_array(),
        "content must not be an array (which would indicate a multi-part/image_url shape): {value}"
    );
}

#[test]
fn agent_loop_wraps_tool_results_as_text() {
    // Static-grep-style pin, mirroring the invariants_*.rs pattern used elsewhere in this
    // crate: confirms agent_loop.rs still constructs every tool result via
    // ChatMessage::tool_result (plain text), never via an image-bearing constructor.
    let source = include_str!("../src/agent_loop.rs");
    assert!(
        source.contains("ChatMessage::tool_result(&tool_call.id, result)"),
        "agent_loop.rs must wrap tool call results via ChatMessage::tool_result(&tool_call.id, \
         result) — if this string is no longer present, verify what changed and re-derive the \
         D-13 mechanism decision from the new code path"
    );
}
