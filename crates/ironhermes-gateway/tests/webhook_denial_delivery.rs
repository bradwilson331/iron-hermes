//! WR-05 (Phase 36.7.1 code review): the DELIVERED half of T-36.7.1-37.
//!
//! `ImmediateDenyApprovalGate` has two obligations, and the phase's own
//! SECURITY.md says both are required: log the denial, AND surface it in the
//! route's delivered output. Before this file, only the first had a test —
//! `handler.rs`'s two unit tests both stop at `denial_log.lock()` and assert
//! the gate RECORDED the text. Nothing asserted it was ever DELIVERED, so the
//! drain block in `run_agent` could be deleted outright and the whole suite
//! stayed green.
//!
//! This drives the real path end to end: a real `GatewayMessageHandler`, a
//! real `AgentRuntime::run_turn`, a real `AgentLoop`, a real guardrail chain.
//! Only the network is faked (a `wiremock::MockServer` serving a canned SSE
//! chat-completions stream) — the same seam `buzz_agent_turn.rs` uses, via
//! the same `AgentRuntime::for_tests_with_base_url` test-support constructor.
//!
//! Shape of the run:
//!   1. the canned first completion returns a tool call for `srv__delete_all`;
//!   2. `McpMutationGuardrail` (registered on the runtime's own registry)
//!      classifies the `__`-prefixed destructive name as `NeedsApproval`;
//!   3. `AgentLoop` consults `TurnConfig.approval_gate`, which for a
//!      `Platform::Webhook` event is the `ImmediateDenyApprovalGate`
//!      `approval_gate_for_event` installs — it denies and records the text;
//!   4. the canned second completion returns plain text, ending the turn;
//!   5. `run_agent`'s drain block must send that recorded text to the adapter.
//!
//! Step 5 is the assertion. Steps 1-4 are production code throughout.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_agent::AgentRuntime;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::{Config, MessageEvent, MessageResponse, Platform, ProviderResolver};
use ironhermes_gateway::handler::GatewayMessageHandler;
use ironhermes_gateway::session::SessionStore;
use ironhermes_hooks::guardrail::McpMutationGuardrail;
use ironhermes_tools::ToolRegistry;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A `__`-prefixed MCP-style tool name carrying a destructive verb — the
/// exact shape `McpMutationGuardrail` classifies as `NeedsApproval`.
const GATED_TOOL: &str = "srv__delete_all";

// ===========================================================================
// Canned chat-completions SSE fixtures
// ===========================================================================

fn sse(chunks: &[String]) -> String {
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// Completion #1: the model issues one tool call for [`GATED_TOOL`].
fn sse_tool_call() -> String {
    sse(&[
        format!(
            r#"{{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"call-1","type":"function","function":{{"name":"{GATED_TOOL}","arguments":"{{}}"}}}}]}}}}]}}"#
        ),
        r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#
            .to_string(),
    ])
}

/// Completion #2: having been told the tool did not execute, the model
/// answers in plain text and the turn ends.
fn sse_text(text: &str) -> String {
    sse(&[
        format!(
            r#"{{"id":"c2","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{{"index":0,"delta":{{"content":{}}}}}]}}"#,
            serde_json::to_string(text).unwrap()
        ),
        r#"{"id":"c2","object":"chat.completion.chunk","created":1,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#
            .to_string(),
    ])
}

/// Mount the tool-call response for the FIRST request only, then the text
/// response for every request after it. Without the `up_to_n_times(1)` bound
/// the model would re-issue the tool call forever and the turn would only end
/// at `max_iterations`.
async fn mount_tool_call_then_text(server: &MockServer, text: &str) {
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_tool_call(), "text/event-stream"),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_text(text), "text/event-stream"),
        )
        .with_priority(2)
        .mount(server)
        .await;
}

// ===========================================================================
// Recording adapter — the route's delivered output
// ===========================================================================

/// Records every `send_message`. `supports_in_place_edits` is `false`,
/// matching the real `WebhookAdapter`: an HTTP request that already received
/// its 202 has no message to edit in place, so every observable byte the
/// route delivers arrives through `send_message`.
#[derive(Default)]
struct RecordingAdapter {
    sent: StdMutex<Vec<String>>,
}

impl RecordingAdapter {
    fn sent(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait]
impl PlatformAdapter for RecordingAdapter {
    fn platform(&self) -> Platform {
        Platform::Webhook
    }
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.sent.lock().unwrap().push(content.to_string());
        Ok(MessageResponse {
            message_id: "webhook-out-1".to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Webhook,
        })
    }
    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.send_message(chat_id, content, thread_id).await
    }
    async fn edit_message(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn edit_message_markdown_v2(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn delete_message(&self, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn is_running(&self) -> bool {
        true
    }
    fn supports_in_place_edits(&self) -> bool {
        false
    }
}

// ===========================================================================
// Handler harness
// ===========================================================================

fn build_handler() -> GatewayMessageHandler {
    let config = Config::default();
    let resolver = ProviderResolver::build(&config).unwrap();
    let state_store = Arc::new(std::sync::Mutex::new(
        ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
    ));
    let session_store = Arc::new(RwLock::new(SessionStore::new(state_store)));
    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    GatewayMessageHandler::new(config, resolver, session_store, tool_registry)
}

fn webhook_event(route_name: &str, content: &str) -> MessageEvent {
    MessageEvent {
        platform: Platform::Webhook,
        message_id: "wh-msg-1".to_string(),
        // `webhook/mod.rs` sets `chat_id` to the ROUTE NAME — the denial
        // notice must be addressed to that same route.
        chat_id: route_name.to_string(),
        sender_id: "webhook-sender".to_string(),
        content: content.to_string(),
        attachments: Vec::new(),
        thread_id: None,
        chat_type: "webhook".to_string(),
        chat_name: None,
        sender_name: None,
        replied_to_id: None,
    }
}

/// Stand up the full composition and run one webhook turn in which the model
/// attempts a gated tool. Returns everything the adapter was sent.
async fn run_webhook_turn_that_attempts_a_gated_tool() -> Vec<String> {
    let server = MockServer::start().await;
    mount_tool_call_then_text(&server, "Here is my answer.").await;

    let runtime = Arc::new(AgentRuntime::for_tests_with_base_url(server.uri()));
    // The guardrail that turns the `__`-prefixed destructive tool name into
    // `NeedsApproval`, which is what makes `AgentLoop` consult the gate at
    // all. Registered on the RUNTIME's registry — the one `run_turn`'s
    // `AgentLoop` reads — not the handler's.
    runtime
        .registry()
        .write()
        .await
        .add_guardrail(Box::new(McpMutationGuardrail::new()));

    let mut handler = build_handler();
    handler.set_agent_runtime(runtime);

    let adapter = Arc::new(RecordingAdapter::default());
    let adapter_dyn: Arc<dyn PlatformAdapter> = adapter.clone();
    let event = webhook_event("my-route", "do the dangerous thing");

    tokio::time::timeout(
        Duration::from_secs(30),
        handler.handle(&event, adapter_dyn, CancellationToken::new()),
    )
    .await
    .expect("the webhook turn must not hang")
    .expect("handle failed");

    adapter.sent()
}

// ===========================================================================
// The assertions
// ===========================================================================

/// WR-05, the headline: the auto-denial must reach the ROUTE'S OUTPUT, not
/// only the process log.
///
/// `ApprovalGate::request_approval` can only return an `ApprovalOutcome`, and
/// what `AgentLoop` hands the model on refusal is the terse "Tool was not
/// approved and did not execute." — so whatever the operator eventually reads
/// is the model's paraphrase of that, and a model may not mention the refusal
/// at all. That is exactly the "why does my route never do X" blindness
/// Research Pitfall 4 describes. The drain block in `run_agent` is what closes
/// it, and this is the assertion that keeps it alive.
#[tokio::test]
async fn a_webhook_turn_that_denies_an_approval_delivers_the_notice() {
    let sent = run_webhook_turn_that_attempts_a_gated_tool().await;

    let notice = sent
        .iter()
        .find(|body| body.contains("Approval automatically denied"))
        .unwrap_or_else(|| {
            panic!(
                "the auto-denial must be DELIVERED to the route's output, not only logged. \
                 The gate recording the text is only half of T-36.7.1-37; run_agent's drain \
                 block is the other half. Adapter received: {sent:?}"
            )
        });

    assert!(
        notice.contains(GATED_TOOL),
        "the delivered notice must name the tool that was denied, or the operator cannot \
         tell which action their route silently skipped: {notice}"
    );
    assert!(
        !notice.contains('\n') || notice.lines().all(|l| !l.is_empty()),
        "delivered denial text must not carry injected blank lines: {notice:?}"
    );
}

/// The companion: the turn's own answer is still delivered. The denial is a
/// SUPPLEMENTARY message (mirroring the media-fallback notice immediately
/// above it in `run_agent`), never a replacement — a drain that swallowed the
/// answer would satisfy the test above while breaking the route.
#[tokio::test]
async fn the_denial_notice_supplements_the_answer_rather_than_replacing_it() {
    let sent = run_webhook_turn_that_attempts_a_gated_tool().await;

    assert!(
        sent.iter().any(|b| b.contains("Here is my answer.")),
        "the turn's own answer must still be delivered alongside the denial: {sent:?}"
    );
    assert!(
        sent.iter().any(|b| b.contains("Approval automatically denied")),
        "and the denial must still be delivered alongside the answer: {sent:?}"
    );
}
