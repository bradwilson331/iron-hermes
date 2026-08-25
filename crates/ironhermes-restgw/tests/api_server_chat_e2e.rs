//! `/v1/chat/completions` + `/v1/responses*` proof suite — Phase 36.7.1 Plan 08.
//!
//! Task 1 covers the OpenAI-compatible chat completions route (streaming and
//! non-streaming). Task 2 covers the three stateful Responses API routes, mounted
//! as an explicit, documented not-implemented gap.
//!
//! # Known Stub
//!
//! `POST /v1/chat/completions` runs a deterministic, non-`AgentLoop` stub turn
//! (see `routes::chat`'s own module doc) — these tests prove the wire shapes,
//! model-registry gating, streaming-bridge mechanics, and turn-registry
//! integration, not model-produced content.
//!
//! Every listener binds `127.0.0.1:0` (ephemeral) — never port 8642.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::concurrency::Surface;
use ironhermes_core::{MessageEvent, ModelMetadata, ModelRegistry};
use ironhermes_restgw::api_server::sse::RunEventRegistry;
use ironhermes_restgw::api_server::{
    ApiServerAdapter, ApiServerConfig, ApiServerHandles, serve_api_server_adapter,
};
use ironhermes_tools::ToolRegistry;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Shared fixtures (mirrors tests/api_server_runs_e2e.rs's own helpers)
// ---------------------------------------------------------------------------

struct UnusedHandler;

#[async_trait]
impl MessageHandler for UnusedHandler {
    async fn handle(
        &self,
        _event: &MessageEvent,
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        panic!("UnusedHandler::handle should never be called by this test suite's routes");
    }
}

/// The one model this suite's registry knows — everything else is "unknown".
const KNOWN_MODEL: &str = "distinctive-chat-model-xyz";

fn known_model_registry() -> ModelRegistry {
    let mut registry = ModelRegistry::new();
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        KNOWN_MODEL.to_string(),
        ModelMetadata {
            context_length: 8_192,
            max_output_tokens: Some(1_024),
            tokenizer: "cl100k_base".to_string(),
            capabilities: Default::default(),
        },
    );
    registry.merge_cache(entries);
    registry
}

fn test_handles() -> ApiServerHandles {
    let db_path = std::env::temp_dir().join(format!(
        "ih-restgw-chat-test-state-{}.db",
        uuid::Uuid::new_v4()
    ));
    ApiServerHandles {
        turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
        state_store: Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(&db_path).expect("open test state store"),
        )),
        job_store: None,
        model_registry: Arc::new(known_model_registry()),
        skill_registry: None,
        tool_registry: Arc::new(tokio::sync::RwLock::new(ToolRegistry::new())),
        approval_gate: None,
        run_events: Arc::new(RunEventRegistry::new()),
    }
}

fn unique_key(test_name: &str) -> String {
    format!("test-key-{test_name}-{}", uuid::Uuid::new_v4())
}

async fn spawn_adapter(
    adapter: Arc<ApiServerAdapter>,
) -> (std::net::SocketAddr, CancellationToken) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let addr = listener.local_addr().expect("local_addr");
    let cancel = CancellationToken::new();
    let handler: Arc<dyn MessageHandler> = Arc::new(UnusedHandler);
    let serve_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = serve_api_server_adapter(listener, adapter, handler, serve_cancel).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    (addr, cancel)
}

/// Parse an OpenAI-style SSE body (`data: <json or [DONE]>\n\n` blocks) into the
/// JSON chunk payloads plus whether the `[DONE]` sentinel was seen.
fn parse_openai_stream(body: &str) -> (Vec<serde_json::Value>, bool) {
    let mut chunks = Vec::new();
    let mut saw_done = false;
    for block in body.split("\n\n") {
        for line in block.lines() {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                saw_done = true;
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                chunks.push(v);
            }
        }
    }
    (chunks, saw_done)
}

// ---------------------------------------------------------------------------
// Task 1: POST /v1/chat/completions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_completion_returns_the_standard_response_shape() {
    let key = unique_key("standard-shape");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "model": KNOWN_MODEL,
            "messages": [{"role": "user", "content": "hello world"}],
        }))
        .send()
        .await
        .expect("completions request");
    assert!(resp.status().is_success(), "status: {}", resp.status());

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictMessage {
        role: String,
        content: String,
    }
    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictChoice {
        index: usize,
        message: StrictMessage,
        finish_reason: String,
    }
    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictUsage {
        prompt_tokens: usize,
        completion_tokens: usize,
        total_tokens: usize,
    }
    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictChatCompletionResponse {
        id: String,
        object: String,
        created: u64,
        model: String,
        choices: Vec<StrictChoice>,
        usage: StrictUsage,
    }

    let body: String = resp.text().await.expect("response body");
    let parsed: StrictChatCompletionResponse = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("response must deserialize strictly (rejecting unknown fields): {e}: {body}"));
    assert_eq!(parsed.object, "chat.completion");
    assert_eq!(parsed.model, KNOWN_MODEL);
    assert!(!parsed.id.is_empty());
    assert!(parsed.created > 0);
    assert_eq!(parsed.choices.len(), 1);
    assert_eq!(parsed.choices[0].index, 0);
    assert_eq!(parsed.choices[0].message.role, "assistant");
    assert_eq!(parsed.choices[0].finish_reason, "stop");
    assert!(parsed.choices[0].message.content.contains("hello"));
    assert_eq!(
        parsed.usage.total_tokens,
        parsed.usage.prompt_tokens + parsed.usage.completion_tokens
    );

    cancel.cancel();
}

#[tokio::test]
async fn multi_message_conversation_is_passed_through() {
    let key = unique_key("multi-message");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "model": KNOWN_MODEL,
            "messages": [
                {"role": "system", "content": "SYSMARKER"},
                {"role": "user", "content": "USERMARKER"},
                {"role": "assistant", "content": "ASSISTANTMARKER"},
            ],
        }))
        .send()
        .await
        .expect("completions request");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.expect("json body");
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .expect("content string");
    assert!(content.contains("SYSMARKER"), "reply must reflect the system message: {content}");
    assert!(content.contains("USERMARKER"), "reply must reflect the user message: {content}");
    assert!(
        content.contains("ASSISTANTMARKER"),
        "reply must reflect the prior assistant message: {content}"
    );
    // Order preserved: a client's history is not reordered on the way to the turn.
    let sys_pos = content.find("SYSMARKER").unwrap();
    let user_pos = content.find("USERMARKER").unwrap();
    let asst_pos = content.find("ASSISTANTMARKER").unwrap();
    assert!(sys_pos < user_pos && user_pos < asst_pos, "message order must be preserved: {content}");

    cancel.cancel();
}

#[tokio::test]
async fn named_model_is_honoured() {
    let key = unique_key("named-model");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "model": KNOWN_MODEL,
            "messages": [{"role": "user", "content": "hi"}],
        }))
        .send()
        .await
        .expect("completions request");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["model"], KNOWN_MODEL);

    cancel.cancel();
}

#[tokio::test]
async fn unknown_model_is_refused_not_substituted() {
    let key = unique_key("unknown-model");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (addr, cancel, registry) = {
        let handles = test_handles();
        let registry = handles.turn_registry.clone();
        let adapter = Arc::new(
            ApiServerAdapter::new(ApiServerConfig::default(), handles)
                .expect("construction must succeed"),
        );
        let (addr, cancel) = spawn_adapter(adapter).await;
        (addr, cancel, registry)
    };
    let client = reqwest::Client::new();

    let non_streaming = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "model": "totally-unknown-model-xyz",
            "messages": [{"role": "user", "content": "hi"}],
        }))
        .send()
        .await
        .expect("non-streaming request");
    assert!(
        non_streaming.status().is_client_error(),
        "unknown model must be a client error, got {}",
        non_streaming.status()
    );

    let streaming = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "model": "totally-unknown-model-xyz",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
        }))
        .send()
        .await
        .expect("streaming request");
    assert!(
        streaming.status().is_client_error(),
        "unknown model must be a client error even when streaming, got {}",
        streaming.status()
    );

    // No turn ran for either request.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        registry.list_all().await.is_empty(),
        "an unknown model must never reach the turn registry"
    );

    cancel.cancel();
}

#[tokio::test]
async fn streamed_chunks_concatenate_to_the_same_answer() {
    let key = unique_key("streamed-concat");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let request_body = serde_json::json!({
        "model": KNOWN_MODEL,
        "messages": [{"role": "user", "content": "alpha beta gamma"}],
    });

    let non_streaming = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&request_body)
        .send()
        .await
        .expect("non-streaming request")
        .json::<serde_json::Value>()
        .await
        .expect("non-streaming json");
    let non_streaming_answer = non_streaming["choices"][0]["message"]["content"]
        .as_str()
        .expect("non-streaming content")
        .to_string();

    let mut streaming_body = request_body.clone();
    streaming_body["stream"] = serde_json::Value::Bool(true);
    let streamed = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&streaming_body)
        .send()
        .await
        .expect("streaming request")
        .text()
        .await
        .expect("streaming body text");
    let (chunks, _saw_done) = parse_openai_stream(&streamed);
    let concatenated: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect::<Vec<_>>()
        .join(" ");

    assert_eq!(
        concatenated, non_streaming_answer,
        "streamed deltas must concatenate to the same answer as the non-streamed form"
    );

    cancel.cancel();
}

#[tokio::test]
async fn stream_terminates_with_the_expected_sentinel() {
    let key = unique_key("stream-sentinel");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let body = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "model": KNOWN_MODEL,
            "messages": [{"role": "user", "content": "hi there"}],
            "stream": true,
        }))
        .send()
        .await
        .expect("streaming request")
        .text()
        .await
        .expect("streaming body text");

    assert!(
        body.contains("data: [DONE]"),
        "streamed response must end with the OpenAI sentinel: {body}"
    );
    let (_chunks, saw_done) = parse_openai_stream(&body);
    assert!(saw_done, "parsed sentinel flag must be set");
    // The sentinel must be the LAST frame, not merely present somewhere mid-stream.
    let trimmed = body.trim_end();
    assert!(
        trimmed.ends_with("data: [DONE]"),
        "sentinel must terminate the stream, not appear mid-stream: {body}"
    );

    cancel.cancel();
}

#[tokio::test]
async fn streamed_turn_is_registered_in_the_registry() {
    let key = unique_key("streamed-registered");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let handles = test_handles();
    let registry = handles.turn_registry.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let req_handle = {
        let client = client.clone();
        let key = key.clone();
        tokio::spawn(async move {
            client
                .post(format!("http://{addr}/v1/chat/completions"))
                .bearer_auth(&key)
                .json(&serde_json::json!({
                    "model": KNOWN_MODEL,
                    "messages": [{"role": "user", "content": "still going"}],
                    "stream": true,
                }))
                .send()
                .await
        })
    };

    let found_surface = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let all = registry.list_all().await;
            if let Some(t) = all.first() {
                return t.surface;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("turn must appear in the registry while in flight");
    assert_eq!(found_surface, Surface::ApiServer);

    let resp = req_handle.await.expect("request task").expect("request");
    assert!(resp.status().is_success());

    cancel.cancel();
}

#[tokio::test]
async fn malformed_request_body_is_a_client_error() {
    let key = unique_key("malformed-body");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "model": KNOWN_MODEL }))
        .send()
        .await
        .expect("malformed request");
    assert!(
        resp.status().is_client_error(),
        "missing `messages` must be a client error, not a server error: {}",
        resp.status()
    );
    let body = resp.text().await.expect("body text");
    assert!(
        body.to_lowercase().contains("messages"),
        "the client error must name the missing field: {body}"
    );

    cancel.cancel();
}

#[tokio::test]
async fn chat_route_requires_the_bearer_key() {
    let key = unique_key("chat-requires-key");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": KNOWN_MODEL,
            "messages": [{"role": "user", "content": "hi"}],
        }))
        .send()
        .await
        .expect("unauthenticated request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 1: source assertions — the streaming path derives from the shared
// bridge and introduces no second `StreamEvent`-shaped vocabulary (API-11).
// ---------------------------------------------------------------------------

const CHAT_RS: &str = include_str!("../src/api_server/routes/chat.rs");

#[test]
fn chat_streaming_derives_from_the_shared_event_source() {
    assert!(
        CHAT_RS.contains("use ironhermes_agent::client::StreamEvent"),
        "chat.rs must import the one real StreamEvent source"
    );
    assert!(
        CHAT_RS.contains("SseFrame::from"),
        "chat.rs must derive its chunks by calling the shared bridge's canonical \
         StreamEvent -> SseFrame mapping"
    );
    assert!(
        !CHAT_RS.contains("enum SseFrame"),
        "chat.rs must not redefine the shared frame type — it reuses sse.rs's own"
    );
    assert!(
        !CHAT_RS.contains("tui_rata"),
        "chat.rs must not import the terminal UI's own StreamEvent-shaped enum"
    );
    assert!(
        !CHAT_RS.contains("iron_hermes_ui"),
        "chat.rs must not import the web UI's own protocol event enum"
    );
    assert!(
        !CHAT_RS.contains("ChatStreamEvent"),
        "chat.rs must not import the web UI's ChatStreamEvent type"
    );
}

// ---------------------------------------------------------------------------
// Task 2: POST/GET/DELETE /v1/responses(/{id})
// ---------------------------------------------------------------------------

const RESPONSES_PATHS: &[&str] = &["/v1/responses", "/v1/responses/00000000-0000-0000-0000-000000000000"];

#[tokio::test]
async fn responses_routes_return_not_implemented_with_a_pointer() {
    let key = unique_key("responses-not-implemented");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let fake_id = "00000000-0000-0000-0000-000000000000";

    let create = client
        .post(format!("http://{addr}/v1/responses"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request");
    assert_eq!(create.status(), reqwest::StatusCode::NOT_IMPLEMENTED);
    let create_body: serde_json::Value = create.json().await.expect("create json");
    assert!(create_body["alternatives"].as_array().is_some_and(|a| !a.is_empty()));
    assert!(
        create_body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("chat/completions"),
        "not-implemented body must point at chat completions: {create_body}"
    );

    let fetch = client
        .get(format!("http://{addr}/v1/responses/{fake_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch request");
    assert_eq!(fetch.status(), reqwest::StatusCode::NOT_IMPLEMENTED);

    let delete = client
        .delete(format!("http://{addr}/v1/responses/{fake_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("delete request");
    assert_eq!(delete.status(), reqwest::StatusCode::NOT_IMPLEMENTED);

    cancel.cancel();
}

#[tokio::test]
async fn responses_routes_are_mounted_not_absent() {
    let key = unique_key("responses-mounted");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    for path in RESPONSES_PATHS {
        let resp = client
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .expect("unauthenticated request");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "path {path} must be mounted (401), not absent (404)"
        );
    }

    cancel.cancel();
}

#[tokio::test]
async fn capabilities_and_responses_routes_agree() {
    let key = unique_key("capabilities-agree");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let caps: serde_json::Value = client
        .get(format!("http://{addr}/v1/capabilities"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("capabilities request")
        .json()
        .await
        .expect("capabilities json");
    assert_eq!(caps["features"]["responses_api"], false);
    assert!(
        caps["endpoints"]["responses"]
            .as_array()
            .is_some_and(|paths| paths.iter().any(|p| p == "/v1/responses")),
        "capabilities must list the responses family's own paths: {caps}"
    );

    let create = client
        .post(format!("http://{addr}/v1/responses"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request");
    assert_eq!(
        create.status(),
        reqwest::StatusCode::NOT_IMPLEMENTED,
        "the routes must refuse exactly as the capabilities map claims"
    );

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 2: source assertion — no handler touches the turn registry or the agent.
// ---------------------------------------------------------------------------

const RESPONSES_RS: &str = include_str!("../src/api_server/routes/responses.rs");

#[test]
fn responses_routes_run_no_turn() {
    assert!(
        !RESPONSES_RS.contains("turn_registry"),
        "responses.rs must never reach the turn registry: {RESPONSES_RS}"
    );
    assert!(
        !RESPONSES_RS.contains("State(adapter)"),
        "responses.rs's handlers must not even extract adapter state — they have nothing to \
         read or write"
    );
}
