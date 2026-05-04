//! Phase 25.1 GAP-7 follow-up: regression test proving `AgentResult::appended`
//! tracks the round-trip output of a full agent run (assistant turn + matching
//! tool result + final assistant) WITHOUT the input slice or transient
//! pressure-tier system advisories.
//!
//! This guards the gateway-handler persistence path against a recurrence of
//! the role-filter bug at handler.rs:642-646, which dropped Role::Tool
//! messages and broke OpenAI's strict assistant↔tool pairing on the next turn.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_agent::AgentLoop;
use ironhermes_agent::any_client::AnyClient;
use ironhermes_agent::client::LlmClient;
use ironhermes_core::{ChatMessage, Role, ToolSchema, validate_tool_call_pairing};
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::RwLock;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn toolset(&self) -> &str {
        "test"
    }
    fn description(&self) -> &str {
        "echo for tests"
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "echo",
            "echo for tests",
            serde_json::json!({ "type": "object", "properties": {} }),
        )
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        Ok("echoed".to_string())
    }
}

/// Mock OpenAI server: first call returns an assistant tool_call response,
/// second call returns a final-text assistant response (no tool_calls).
async fn mount_two_turn_mock(server: &MockServer) {
    // First response: assistant requests `echo` tool
    let first = serde_json::json!({
        "id": "resp-1",
        "object": "chat.completion",
        "created": 0,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_abc",
                    "type": "function",
                    "function": { "name": "echo", "arguments": "{}" }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
    });
    let second = serde_json::json!({
        "id": "resp-2",
        "object": "chat.completion",
        "created": 1,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "done" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 20, "completion_tokens": 2, "total_tokens": 22 }
    });

    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(first))
        .up_to_n_times(1)
        .mount(server)
        .await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(second))
        .mount(server)
        .await;
}

#[tokio::test]
async fn agent_run_appended_excludes_input_includes_tool_pair() {
    let server = MockServer::start().await;
    mount_two_turn_mock(&server).await;

    let client = AnyClient::ChatCompletions(LlmClient::new(
        server.uri(),
        "test-key".to_string(),
        "gpt-4o-mini",
    ));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));
    let registry = Arc::new(RwLock::new(registry));

    // Input is just [user] — explicit, no system, mirroring agent_loop tests.
    let input = vec![ChatMessage::user("please echo")];
    let input_len = input.len();

    let mut agent = AgentLoop::new(client, registry, 4);
    let result = agent.run(input).await.expect("run must succeed");

    // appended must contain exactly: [assistant_tc, tool_result, assistant_final].
    assert_eq!(
        result.appended.len(),
        3,
        "appended must hold exactly the round-trip output (assistant + tool + final), got {}",
        result.appended.len()
    );

    // None of the appended entries are the original user input.
    assert!(
        result.appended.iter().all(|m| m.role != Role::User),
        "appended must NOT include the input user message"
    );

    // The first appended entry is the assistant with tool_calls.
    let first = &result.appended[0];
    assert_eq!(first.role, Role::Assistant, "appended[0] must be assistant");
    assert!(
        first.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()),
        "appended[0] must carry tool_calls"
    );
    let call_id = &first.tool_calls.as_ref().unwrap()[0].id;
    assert_eq!(call_id, "call_abc");

    // The second appended entry is the matching tool result with the SAME id.
    let second = &result.appended[1];
    assert_eq!(second.role, Role::Tool, "appended[1] must be Role::Tool");
    assert_eq!(
        second.tool_call_id.as_deref(),
        Some("call_abc"),
        "appended[1].tool_call_id must match the assistant's tool_call id"
    );

    // The third entry is the final assistant text response.
    let third = &result.appended[2];
    assert_eq!(third.role, Role::Assistant);
    assert!(
        third.tool_calls.is_none() || third.tool_calls.as_ref().unwrap().is_empty(),
        "final assistant must not carry tool_calls"
    );

    // result.messages still contains the full conversation: input + appended.
    assert_eq!(
        result.messages.len(),
        input_len + result.appended.len(),
        "messages must be input ({input_len}) + appended ({}); got {}",
        result.appended.len(),
        result.messages.len()
    );

    // The reconstructed-on-restore Vec<ChatMessage> the gateway will build for
    // turn N+1 is `[system?] + prior_session_messages + appended`. Simulate
    // that exact path and prove the strict tool-pair invariant holds:
    let mut next_turn_history = vec![
        ChatMessage::system("you are a helpful agent"),
        ChatMessage::user("please echo"),
    ];
    next_turn_history.extend(result.appended.iter().cloned());
    next_turn_history.push(ChatMessage::user("now do another thing"));

    validate_tool_call_pairing(&next_turn_history)
        .expect("reconstructed history must satisfy strict tool-pair invariant after appended-based persistence");
}

#[tokio::test]
async fn agent_run_appended_empty_when_cancelled_before_first_iteration() {
    use tokio_util::sync::CancellationToken;

    // No mock server needed — cancellation fires before any LLM call.
    let client = AnyClient::ChatCompletions(LlmClient::new(
        "http://localhost".to_string(),
        "".to_string(),
        "mock-model",
    ));
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let token = CancellationToken::new();
    token.cancel();

    let mut agent = AgentLoop::new(client, registry, 4).with_cancellation_token(token);
    let result = agent
        .run(vec![ChatMessage::user("hi")])
        .await
        .expect("cancelled run is Ok");

    assert!(
        result.appended.is_empty(),
        "appended must be empty when run cancels before any LLM call"
    );
    assert_eq!(result.turns_used, 0);
}
