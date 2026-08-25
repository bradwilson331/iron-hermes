//! `/api/sessions*` proof suite — Phase 36.7.1 Plan 07.
//!
//! Task 1 covers CRUD + messages over the existing `StateStore`. Task 2 adds fork and
//! the model lock. Task 3 adds session-scoped chat (single-shot and streamed).
//!
//! Mutating assertions are checked directly against the store (not only the response
//! body) per the plan's own instruction — a test that only checks the response passes
//! against a handler that returns a plausible echo and writes nothing.
//!
//! Every listener binds `127.0.0.1:0` (ephemeral) — never port 8642.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::{MessageEvent, ModelRegistry};
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

/// Returns the handles AND the temp db path, so a test can open a second, independent
/// `StateStore` handle against the SAME file to assert store state directly — the
/// route's own `state_store` field is behind a `Mutex` shared with the running server,
/// so a second short-lived connection is how a test reads store state without racing
/// the server's own lock.
fn test_handles() -> (ApiServerHandles, std::path::PathBuf) {
    let db_path = std::env::temp_dir().join(format!(
        "ih-restgw-sessions-test-state-{}.db",
        uuid::Uuid::new_v4()
    ));
    let handles = ApiServerHandles {
        turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
        state_store: Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(&db_path).expect("open test state store"),
        )),
        job_store: None,
        model_registry: Arc::new(ModelRegistry::new()),
        skill_registry: None,
        tool_registry: Arc::new(tokio::sync::RwLock::new(ToolRegistry::new())),
        approval_gate: None,
        run_events: Arc::new(RunEventRegistry::new()),
    };
    (handles, db_path)
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

/// Open a second, short-lived connection to the same on-disk store a running test
/// server is using, so assertions can read store state directly rather than trusting
/// the HTTP response.
fn open_store(db_path: &std::path::Path) -> ironhermes_state::StateStore {
    ironhermes_state::StateStore::new(db_path).expect("open direct store handle for assertions")
}

/// Create a session directly in the store, standing in for one this surface
/// created through `POST /api/sessions`.
///
/// Uses the real `api_server` source rather than an arbitrary literal: since
/// the N-04 fix, `source` decides whether the chat routes will author into a
/// session at all, so a stand-in for an API-owned session must actually claim
/// to be one. Use [`create_foreign_session_directly`] for the opposite case.
fn create_session_directly(db_path: &std::path::Path) -> String {
    create_session_with_source(db_path, "api_server")
}

/// Create a session owned by a DIFFERENT surface — what a Telegram, Discord,
/// Slack or Buzz conversation looks like in the shared store.
fn create_foreign_session_directly(db_path: &std::path::Path) -> String {
    create_session_with_source(db_path, "telegram")
}

fn create_session_with_source(db_path: &std::path::Path, source: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let mut store = open_store(db_path);
    store
        .create_session(&id, source, None, None, None, None)
        .expect("create session directly");
    id
}

/// Parse an SSE response body (`data: <json>\n\n` blocks) into the JSON payloads —
/// mirrors `tests/api_server_runs_e2e.rs`'s own helper.
fn parse_sse_frames(body: &str) -> Vec<serde_json::Value> {
    body.split("\n\n")
        .filter_map(|block| block.strip_prefix("data: ").or_else(|| block.strip_prefix("data:")))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::from_str(s).unwrap_or_else(|e| panic!("frame is valid JSON: {e}: {s}")))
        .collect()
}

// ---------------------------------------------------------------------------
// Task 1: CRUD + messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_lifecycle_round_trips_through_the_store() {
    let key = unique_key("lifecycle");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    // Create.
    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_str().expect("id string").to_string();

    let store = open_store(&db_path);
    assert!(
        store.get_session(&id).unwrap().is_some(),
        "created session must be visible directly in the store"
    );

    // List.
    let listed: serde_json::Value = client
        .get(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("list request")
        .json()
        .await
        .expect("list json");
    let ids: Vec<&str> = listed["sessions"]
        .as_array()
        .expect("sessions array")
        .iter()
        .map(|s| s["id"].as_str().expect("id string"))
        .collect();
    assert!(ids.contains(&id.as_str()), "list must reflect the created session");

    // Fetch.
    let fetched: serde_json::Value = client
        .get(format!("http://{addr}/api/sessions/{id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch request")
        .json()
        .await
        .expect("fetch json");
    assert_eq!(fetched["id"], id);

    // Retitle.
    let resp = client
        .patch(format!("http://{addr}/api/sessions/{id}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "title": "renamed" }))
        .send()
        .await
        .expect("retitle request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let store = open_store(&db_path);
    assert_eq!(
        store.get_session(&id).unwrap().unwrap().title.as_deref(),
        Some("renamed"),
        "retitle must be visible directly in the store"
    );

    // Delete.
    let resp = client
        .delete(format!("http://{addr}/api/sessions/{id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("delete request");
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    let store = open_store(&db_path);
    assert!(
        store.get_session(&id).unwrap().is_none(),
        "deleted session must be gone directly in the store"
    );

    cancel.cancel();
}

#[tokio::test]
async fn list_reflects_created_sessions() {
    let key = unique_key("list-reflects");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_str().expect("id string").to_string();

    let listed: serde_json::Value = client
        .get(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("list request")
        .json()
        .await
        .expect("list json");
    let ids: Vec<&str> = listed["sessions"]
        .as_array()
        .expect("sessions array")
        .iter()
        .map(|s| s["id"].as_str().expect("id string"))
        .collect();
    assert!(ids.contains(&id.as_str()));

    cancel.cancel();
}

#[tokio::test]
async fn retitle_persists() {
    let key = unique_key("retitle-persists");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_str().expect("id string").to_string();

    client
        .patch(format!("http://{addr}/api/sessions/{id}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "title": "my title" }))
        .send()
        .await
        .expect("retitle request");

    let fetched: serde_json::Value = client
        .get(format!("http://{addr}/api/sessions/{id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch request")
        .json()
        .await
        .expect("fetch json");
    assert_eq!(fetched["title"], "my title");

    cancel.cancel();
}

#[tokio::test]
async fn delete_removes_the_session_and_its_messages() {
    let key = unique_key("delete-orphans");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_str().expect("id string").to_string();

    {
        let mut store = open_store(&db_path);
        store
            .add_message(&id, &ironhermes_core::ChatMessage::user("hi"))
            .expect("add message");
    }

    let resp = client
        .delete(format!("http://{addr}/api/sessions/{id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("delete request");
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);

    let store = open_store(&db_path);
    assert!(store.get_session(&id).unwrap().is_none());
    assert!(
        store.get_messages(&id).unwrap().is_empty(),
        "delete must leave no orphaned message rows behind"
    );

    cancel.cancel();
}

#[tokio::test]
async fn messages_endpoint_returns_persisted_messages_in_order() {
    let key = unique_key("messages-order");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_str().expect("id string").to_string();

    {
        let mut store = open_store(&db_path);
        store
            .add_message(&id, &ironhermes_core::ChatMessage::user("first"))
            .unwrap();
        store
            .add_message(&id, &ironhermes_core::ChatMessage::assistant("second"))
            .unwrap();
        store
            .add_message(&id, &ironhermes_core::ChatMessage::user("third"))
            .unwrap();
    }

    let resp: serde_json::Value = client
        .get(format!("http://{addr}/api/sessions/{id}/messages"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("messages request")
        .json()
        .await
        .expect("messages json");
    let contents: Vec<&str> = resp["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .map(|m| m["content"].as_str().expect("content string"))
        .collect();
    assert_eq!(contents, vec!["first", "second", "third"]);

    cancel.cancel();
}

#[tokio::test]
async fn empty_session_returns_an_empty_message_list() {
    let key = unique_key("messages-empty");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_str().expect("id string").to_string();

    let resp = client
        .get(format!("http://{addr}/api/sessions/{id}/messages"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("messages request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("messages json");
    assert_eq!(body["messages"].as_array().unwrap().len(), 0);

    cancel.cancel();
}

#[tokio::test]
async fn unknown_session_is_not_found_on_every_verb() {
    let key = unique_key("unknown-every-verb");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let unknown = uuid::Uuid::new_v4().to_string();

    let fetch_resp = client
        .get(format!("http://{addr}/api/sessions/{unknown}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch request");
    assert_eq!(fetch_resp.status(), reqwest::StatusCode::NOT_FOUND);

    let patch_resp = client
        .patch(format!("http://{addr}/api/sessions/{unknown}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "title": "x" }))
        .send()
        .await
        .expect("patch request");
    assert_eq!(patch_resp.status(), reqwest::StatusCode::NOT_FOUND);

    let delete_resp = client
        .delete(format!("http://{addr}/api/sessions/{unknown}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("delete request");
    assert_eq!(delete_resp.status(), reqwest::StatusCode::NOT_FOUND);

    let messages_resp = client
        .get(format!("http://{addr}/api/sessions/{unknown}/messages"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("messages request");
    assert_eq!(messages_resp.status(), reqwest::StatusCode::NOT_FOUND);

    let store = open_store(&db_path);
    assert!(
        store.get_session(&unknown).unwrap().is_none(),
        "none of the above verbs may implicitly create the session"
    );

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 2: fork + model lock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fork_sets_parent_and_leaves_the_original_untouched() {
    let key = unique_key("fork-parent");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let original_id = created["id"].as_str().expect("id string").to_string();

    client
        .patch(format!("http://{addr}/api/sessions/{original_id}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "title": "original title" }))
        .send()
        .await
        .expect("retitle request");
    {
        let mut store = open_store(&db_path);
        store
            .add_message(&original_id, &ironhermes_core::ChatMessage::user("hello"))
            .unwrap();
    }
    let before = open_store(&db_path).get_session(&original_id).unwrap().unwrap();

    let forked: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions/{original_id}/fork"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fork request")
        .json()
        .await
        .expect("fork json");
    assert_eq!(forked["parent_session_id"], original_id);

    let after = open_store(&db_path).get_session(&original_id).unwrap().unwrap();
    assert_eq!(before.title, after.title);
    assert_eq!(before.message_count, after.message_count);
    assert_eq!(before.started_at, after.started_at);

    cancel.cancel();
}

#[tokio::test]
async fn fork_copies_history_up_to_the_fork_point() {
    let key = unique_key("fork-copies-history");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let original_id = created["id"].as_str().expect("id string").to_string();
    {
        let mut store = open_store(&db_path);
        store
            .add_message(&original_id, &ironhermes_core::ChatMessage::user("one"))
            .unwrap();
        store
            .add_message(&original_id, &ironhermes_core::ChatMessage::assistant("two"))
            .unwrap();
    }

    let forked: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions/{original_id}/fork"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fork request")
        .json()
        .await
        .expect("fork json");
    let fork_id = forked["id"].as_str().expect("id string").to_string();

    let store = open_store(&db_path);
    let original_contents: Vec<Option<String>> = store
        .get_messages(&original_id)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    let fork_contents: Vec<Option<String>> = store
        .get_messages(&fork_id)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert_eq!(original_contents, fork_contents);
    assert_eq!(fork_contents, vec![Some("one".to_string()), Some("two".to_string())]);

    cancel.cancel();
}

#[tokio::test]
async fn fork_of_an_unknown_session_is_not_found() {
    let key = unique_key("fork-unknown");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let unknown = uuid::Uuid::new_v4().to_string();

    let resp = client
        .post(format!("http://{addr}/api/sessions/{unknown}/fork"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fork request");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    let store = open_store(&db_path);
    let sessions = store.list_sessions(None, 100).unwrap();
    assert!(sessions.is_empty(), "forking an unknown session must create nothing");

    cancel.cancel();
}

#[tokio::test]
async fn forked_session_is_independently_writable() {
    let key = unique_key("fork-independent");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let original_id = created["id"].as_str().expect("id string").to_string();

    let forked: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions/{original_id}/fork"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fork request")
        .json()
        .await
        .expect("fork json");
    let fork_id = forked["id"].as_str().expect("id string").to_string();

    {
        let mut store = open_store(&db_path);
        store
            .add_message(&fork_id, &ironhermes_core::ChatMessage::user("only in the fork"))
            .unwrap();
    }

    let store = open_store(&db_path);
    assert!(store.get_messages(&original_id).unwrap().is_empty());
    assert_eq!(store.get_messages(&fork_id).unwrap().len(), 1);

    cancel.cancel();
}

#[tokio::test]
async fn model_lock_persists_and_is_readable() {
    let key = unique_key("model-lock-persists");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_str().expect("id string").to_string();

    let resp = client
        .patch(format!("http://{addr}/api/sessions/{id}/model"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "model": "claude-sonnet-4.5" }))
        .send()
        .await
        .expect("model lock request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let fetched: serde_json::Value = client
        .get(format!("http://{addr}/api/sessions/{id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch request")
        .json()
        .await
        .expect("fetch json");
    assert_eq!(fetched["model"], "claude-sonnet-4.5");

    cancel.cancel();
}

#[tokio::test]
async fn model_lock_rejects_an_unknown_model() {
    let key = unique_key("model-lock-rejects");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions"))
        .bearer_auth(&key)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let id = created["id"].as_str().expect("id string").to_string();

    let resp = client
        .patch(format!("http://{addr}/api/sessions/{id}/model"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "model": "definitely-not-a-real-model" }))
        .send()
        .await
        .expect("model lock request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);

    let store = open_store(&db_path);
    assert_eq!(store.get_session(&id).unwrap().unwrap().model, None);

    cancel.cancel();
}

#[tokio::test]
async fn model_lock_on_an_unknown_session_is_not_found() {
    let key = unique_key("model-lock-unknown-session");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let unknown = uuid::Uuid::new_v4().to_string();

    let resp = client
        .patch(format!("http://{addr}/api/sessions/{unknown}/model"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "model": "claude-sonnet-4.5" }))
        .send()
        .await
        .expect("model lock request");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    cancel.cancel();
}

#[tokio::test]
async fn fork_and_model_routes_require_the_bearer_key() {
    let key = unique_key("fork-model-bearer");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let placeholder = uuid::Uuid::new_v4().to_string();

    let fork_resp = client
        .post(format!("http://{addr}/api/sessions/{placeholder}/fork"))
        .send()
        .await
        .expect("fork request");
    assert_eq!(fork_resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    let model_resp = client
        .patch(format!("http://{addr}/api/sessions/{placeholder}/model"))
        .send()
        .await
        .expect("model lock request");
    assert_eq!(model_resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    cancel.cancel();
}

#[tokio::test]
async fn session_routes_require_the_bearer_key() {
    let key = unique_key("session-bearer");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    for path in ironhermes_restgw::api_server::routes::sessions::SESSION_PATHS {
        let url = format!("http://{addr}{path}");
        let resp = client.get(&url).send().await.expect("unauthenticated request");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "path {path} must refuse a request with no Authorization header"
        );
    }

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 3: session-scoped chat, single-shot and streamed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chat_turn_is_persisted_to_the_named_session() {
    let key = unique_key("chat-persisted");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let id = create_session_directly(&db_path);

    let resp: serde_json::Value = client
        .post(format!("http://{addr}/api/sessions/{id}/chat"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "hello there" }))
        .send()
        .await
        .expect("chat request")
        .json()
        .await
        .expect("chat json");
    assert_eq!(resp["session_id"], id);
    assert_eq!(resp["reply"], "hello there");

    let store = open_store(&db_path);
    let contents: Vec<Option<String>> = store
        .get_messages(&id)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert_eq!(
        contents,
        vec![Some("hello there".to_string()), Some("hello there".to_string())],
        "both the prompt and the answer must be visible on a subsequent messages read"
    );

    cancel.cancel();
}

#[tokio::test]
async fn chat_on_an_unknown_session_is_not_found() {
    let key = unique_key("chat-unknown");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let unknown = uuid::Uuid::new_v4().to_string();

    let resp = client
        .post(format!("http://{addr}/api/sessions/{unknown}/chat"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "hello" }))
        .send()
        .await
        .expect("chat request");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    let store = open_store(&db_path);
    assert!(store.get_session(&unknown).unwrap().is_none());
    assert!(store.get_messages(&unknown).unwrap().is_empty(), "no turn must have run");

    cancel.cancel();
}

#[tokio::test]
async fn chat_stream_uses_the_shared_sse_frame_set() {
    let key = unique_key("chat-stream-frame-set");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let id = create_session_directly(&db_path);

    let body = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .post(format!("http://{addr}/api/sessions/{id}/chat/stream"))
            .bearer_auth(&key)
            .json(&serde_json::json!({ "prompt": "alpha beta" }))
            .send(),
    )
    .await
    .expect("request must not hang")
    .expect("chat stream request")
    .text()
    .await
    .expect("chat stream body");

    let frames = parse_sse_frames(&body);
    let known: [&str; 5] = [
        "content_delta",
        "tool_call_delta",
        "usage",
        "done",
        "provider_error",
    ];
    assert!(!frames.is_empty(), "expected at least one frame: {frames:?}");
    for frame in &frames {
        let ty = frame["type"].as_str().expect("type field");
        assert!(known.contains(&ty), "unexpected frame type: {ty}");
    }

    cancel.cancel();
}

#[tokio::test]
async fn chat_stream_deltas_arrive_in_order() {
    let key = unique_key("chat-stream-order");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let id = create_session_directly(&db_path);

    let body = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .post(format!("http://{addr}/api/sessions/{id}/chat/stream"))
            .bearer_auth(&key)
            .json(&serde_json::json!({ "prompt": "alpha beta gamma" }))
            .send(),
    )
    .await
    .expect("request must not hang")
    .expect("chat stream request")
    .text()
    .await
    .expect("chat stream body");

    let frames = parse_sse_frames(&body);
    let deltas: Vec<&str> = frames
        .iter()
        .filter(|f| f["type"] == "content_delta")
        .map(|f| f["text"].as_str().expect("text field"))
        .collect();
    assert_eq!(deltas, vec!["alpha", "beta", "gamma"]);
    assert_eq!(frames.last().unwrap()["type"], "done");

    let store = open_store(&db_path);
    let contents: Vec<Option<String>> = store
        .get_messages(&id)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert_eq!(
        contents,
        vec![
            Some("alpha beta gamma".to_string()),
            Some("alpha beta gamma".to_string())
        ],
        "the streamed exchange must be persisted, same as the single-shot route"
    );

    cancel.cancel();
}

#[tokio::test]
async fn chat_stream_provider_error_is_forwarded_then_closes() {
    let key = unique_key("chat-stream-provider-error");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let id = create_session_directly(&db_path);

    let body = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .post(format!("http://{addr}/api/sessions/{id}/chat/stream"))
            .bearer_auth(&key)
            .json(&serde_json::json!({ "prompt": "__simulate_provider_error__" }))
            .send(),
    )
    .await
    .expect("request must not hang")
    .expect("chat stream request")
    .text()
    .await
    .expect("chat stream body");

    let frames = parse_sse_frames(&body);
    assert_eq!(frames.len(), 1, "expected exactly one frame: {frames:?}");
    assert_eq!(frames[0]["type"], "provider_error");

    let store = open_store(&db_path);
    let contents: Vec<Option<String>> = store
        .get_messages(&id)
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    // WR-06: this assertion previously read `contents.len() == 1` — "only the
    // prompt is persisted". That codified the defect. A turn that produced no
    // reply leaves NOTHING behind: a session must never carry a trailing user
    // message with no assistant message, because the gateway's resume path
    // replays persisted rows as model context, and a client that received this
    // `ProviderError` frame reasonably retries — appending a second copy of the
    // same prompt. Repeated failures otherwise accumulate duplicate user turns.
    assert!(
        contents.is_empty(),
        "a turn with no reply must persist nothing at all — an orphaned user \
         message would be replayed as context and duplicated on every retry: \
         {contents:?}"
    );

    cancel.cancel();
}

/// WR-06, single-shot twin. `chat` returns 502 on the provider-error path and
/// must leave the session byte-for-byte untouched, exactly as the N-04
/// ownership-gate fix already required of the refusal path in this same
/// function.
#[tokio::test]
async fn chat_provider_error_leaves_no_orphaned_user_message() {
    let key = unique_key("chat-provider-error-orphan");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let id = create_session_directly(&db_path);

    let resp = client
        .post(format!("http://{addr}/api/sessions/{id}/chat"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "__simulate_provider_error__" }))
        .send()
        .await
        .expect("chat request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::BAD_GATEWAY,
        "the provider-error path must still report 502"
    );

    let store = open_store(&db_path);
    let messages = store.get_messages(&id).expect("read messages");
    assert!(
        messages.is_empty(),
        "a failed turn must persist neither half of the exchange, found {} message(s)",
        messages.len()
    );

    cancel.cancel();
}

/// The retry half, which is what makes WR-06 accumulate rather than merely
/// leave one stray row: a client that failed and retries must not build up a
/// pile of unanswered prompts. Three failures then one success must leave
/// exactly the successful exchange.
#[tokio::test]
async fn repeated_provider_errors_do_not_accumulate_duplicate_prompts() {
    let key = unique_key("chat-provider-error-retry");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let id = create_session_directly(&db_path);

    for _ in 0..3 {
        let resp = client
            .post(format!("http://{addr}/api/sessions/{id}/chat"))
            .bearer_auth(&key)
            .json(&serde_json::json!({ "prompt": "__simulate_provider_error__" }))
            .send()
            .await
            .expect("chat request");
        assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);
    }

    let resp = client
        .post(format!("http://{addr}/api/sessions/{id}/chat"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "finally works" }))
        .send()
        .await
        .expect("chat request");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let store = open_store(&db_path);
    let contents: Vec<Option<String>> = store
        .get_messages(&id)
        .expect("read messages")
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert_eq!(
        contents.len(),
        2,
        "three failed retries plus one success must leave exactly one user turn \
         and its reply — every failure that persisted its prompt would add another \
         unanswered row to the model's replayed context: {contents:?}"
    );
    assert_eq!(contents[0].as_deref(), Some("finally works"));
    assert_eq!(contents[1].as_deref(), Some("finally works"));

    cancel.cancel();
}

#[tokio::test]
async fn chat_stream_registers_its_turn_in_the_registry() {
    let key = unique_key("chat-stream-registers");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let registry = handles.turn_registry.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let id = create_session_directly(&db_path);

    // Spawned as a background task so it is actually driven (sends the request and
    // awaits the response) concurrently with the registry-polling loop below — an
    // unpolled future does nothing at all, it never sends the request.
    let req_handle = {
        let client = client.clone();
        let id = id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            client
                .post(format!("http://{addr}/api/sessions/{id}/chat/stream"))
                .bearer_auth(&key)
                .json(&serde_json::json!({ "prompt": "still going" }))
                .send()
                .await
        })
    };

    let found = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let all = registry.list_all().await;
            if let Some(t) = all.iter().find(|t| t.session_id == id) {
                return t.surface;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("turn must be registered before the stub turn's 150ms window elapses");
    assert_eq!(found, ironhermes_core::concurrency::Surface::ApiServer);

    let _ = req_handle.await;
    cancel.cancel();
}

#[tokio::test]
async fn chat_stream_terminates_when_its_turn_is_cancelled() {
    let key = unique_key("chat-stream-cancelled");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let registry = handles.turn_registry.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let id = create_session_directly(&db_path);

    // Spawned as a background task so it is actually driven concurrently with the
    // registry-polling loop below — see the identical note in
    // `chat_stream_registers_its_turn_in_the_registry`.
    let req_handle = {
        let client = client.clone();
        let id = id.clone();
        let key = key.clone();
        tokio::spawn(async move {
            client
                .post(format!("http://{addr}/api/sessions/{id}/chat/stream"))
                .bearer_auth(&key)
                .json(&serde_json::json!({ "prompt": "still going" }))
                .send()
                .await
        })
    };

    let turn_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let all = registry.list_all().await;
            if let Some(t) = all.iter().find(|t| t.session_id == id) {
                return t.turn_id;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("turn must be registered before the stub turn's 150ms window elapses");
    registry.cancel_one(turn_id).await;

    let body = tokio::time::timeout(Duration::from_secs(5), async {
        req_handle
            .await
            .expect("task join")
            .expect("chat stream request")
            .text()
            .await
            .expect("body")
    })
    .await
    .expect("the stream must terminate promptly after cancellation");
    let frames = parse_sse_frames(&body);
    assert!(
        frames.iter().all(|f| f["type"] != "done"),
        "a cancelled turn must never reach a done frame: {frames:?}"
    );

    cancel.cancel();
}

#[tokio::test]
async fn chat_routes_require_the_bearer_key() {
    let key = unique_key("chat-bearer");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let placeholder = uuid::Uuid::new_v4().to_string();

    let chat_resp = client
        .post(format!("http://{addr}/api/sessions/{placeholder}/chat"))
        .send()
        .await
        .expect("chat request");
    assert_eq!(chat_resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    let stream_resp = client
        .post(format!("http://{addr}/api/sessions/{placeholder}/chat/stream"))
        .send()
        .await
        .expect("chat stream request");
    assert_eq!(stream_resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    cancel.cancel();
}

// ===========================================================================
// Security audit N-04: REST turns may not author into another surface's session
// ===========================================================================

/// `ApiServerHandles.state_store` is the SAME store the gateway persists
/// Telegram, Discord, Slack and Buzz conversations into, and the session resume
/// path replays persisted rows back to the model as context.
///
/// Before this fix, `chat` and `chat_stream` accepted any id `get_session`
/// resolved and appended an assistant message to it. A REST caller could
/// therefore write a fabricated assistant turn into a live Telegram
/// conversation, where it would later be replayed to the model as though the
/// agent had said it. WINDOWS ledger 19 records the stub as this surface's own
/// fabricated response content; the cross-surface persistence is the part that
/// was not recorded anywhere.
#[tokio::test]
async fn chat_refuses_a_session_owned_by_another_surface() {
    let key = unique_key("chat-foreign-source");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let foreign_id = create_foreign_session_directly(&db_path);

    let resp = client
        .post(format!("http://{addr}/api/sessions/{foreign_id}/chat"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "inject" }))
        .send()
        .await
        .expect("chat request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "a REST turn against a Telegram-owned session must be refused"
    );

    // Nothing may have been written — not the prompt, not a reply. The prompt
    // is persisted BEFORE the turn runs, so a gate placed after it would leave
    // a stray user message behind even on the refusal path.
    let store = open_store(&db_path);
    let messages = store
        .get_messages(&foreign_id)
        .expect("read foreign session messages");
    assert!(
        messages.is_empty(),
        "the foreign session must be untouched, found {} message(s)",
        messages.len()
    );

    cancel.cancel();
}

/// The streaming twin of the same route family — a long-lived connection is the
/// worst place to miss a gate, which is why it is asserted separately.
#[tokio::test]
async fn chat_stream_refuses_a_session_owned_by_another_surface() {
    let key = unique_key("chat-stream-foreign-source");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let foreign_id = create_foreign_session_directly(&db_path);

    let resp = client
        .post(format!("http://{addr}/api/sessions/{foreign_id}/chat/stream"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "inject" }))
        .send()
        .await
        .expect("chat stream request");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    let store = open_store(&db_path);
    let messages = store
        .get_messages(&foreign_id)
        .expect("read foreign session messages");
    assert!(messages.is_empty(), "the foreign session must be untouched");

    cancel.cancel();
}

/// The gate must not over-reject: a session this surface owns still works.
#[tokio::test]
async fn chat_still_works_on_a_session_this_surface_owns() {
    let key = unique_key("chat-own-source");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, db_path) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let own_id = create_session_directly(&db_path);

    let resp = client
        .post(format!("http://{addr}/api/sessions/{own_id}/chat"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "hello" }))
        .send()
        .await
        .expect("chat request");
    assert!(resp.status().is_success());

    cancel.cancel();
}
