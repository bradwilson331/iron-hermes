//! `/api/jobs*` proof suite — Phase 36.7.1 Plan 09.
//!
//! Task 1 covers job listing/create/fetch/update/delete over the real cron
//! job store. Task 2 covers pause/resume/trigger through the store's own
//! operations. Task 3 covers the capabilities-map-vs-router drift test, the
//! reachability sweep, the two deliberately-omitted endpoints, and the
//! general not-implemented agreement — landing in this same file per the
//! plan's own `<files>` list.
//!
//! Every listener binds `127.0.0.1:0` (ephemeral) — never port 8642.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::{MessageEvent, ModelRegistry};
use ironhermes_cron::JobStore;
use ironhermes_restgw::api_server::routes::FAMILIES;
use ironhermes_restgw::api_server::sse::RunEventRegistry;
use ironhermes_restgw::api_server::{
    ApiServerAdapter, ApiServerConfig, ApiServerHandles, serve_api_server_adapter,
};
use ironhermes_tools::ToolRegistry;
use reqwest::Method;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Shared fixtures (mirrors tests/api_server_chat_e2e.rs's own helpers)
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

/// Handles carrying a REAL, isolated `JobStore` (own temp `cron/` dir) — the
/// SAME handle a test keeps alongside, so tests can assert store state
/// directly (create/reload/get_due_jobs) rather than trusting the HTTP
/// response echo.
fn test_handles() -> (ApiServerHandles, Arc<Mutex<JobStore>>, std::path::PathBuf) {
    let db_path = std::env::temp_dir().join(format!(
        "ih-restgw-jobs-test-state-{}.db",
        uuid::Uuid::new_v4()
    ));
    let cron_dir = std::env::temp_dir().join(format!(
        "ih-restgw-jobs-test-cron-{}",
        uuid::Uuid::new_v4()
    ));
    let job_store = Arc::new(Mutex::new(
        JobStore::open(cron_dir.clone()).expect("open test job store"),
    ));
    let handles = ApiServerHandles {
        turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
        state_store: Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(&db_path).expect("open test state store"),
        )),
        job_store: Some(job_store.clone()),
        model_registry: Arc::new(ModelRegistry::new()),
        skill_registry: None,
        tool_registry: Arc::new(tokio::sync::RwLock::new(ToolRegistry::new())),
        approval_gate: None,
        run_events: Arc::new(RunEventRegistry::new()),
    };
    (handles, job_store, cron_dir)
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

// ---------------------------------------------------------------------------
// Task 1: list / create / fetch / update / delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn job_lifecycle_round_trips_through_the_job_store() {
    let key = unique_key("lifecycle");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    // create
    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/jobs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "name": "daily-report",
            "prompt": "summarize yesterday",
            "schedule": "0 9 * * *",
        }))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let job_id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["name"], "daily-report");

    // Assert directly against the real store — not the response echo.
    {
        let store = job_store.lock().unwrap();
        assert_eq!(store.list_jobs().len(), 1);
        assert_eq!(store.get_job(&job_id).unwrap().name, "daily-report");
    }

    // list
    let listed: serde_json::Value = client
        .get(format!("http://{addr}/api/jobs"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("list request")
        .json()
        .await
        .expect("list json");
    assert_eq!(listed["jobs"].as_array().unwrap().len(), 1);

    // fetch
    let fetched: serde_json::Value = client
        .get(format!("http://{addr}/api/jobs/{job_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch request")
        .json()
        .await
        .expect("fetch json");
    assert_eq!(fetched["id"], job_id);

    // update
    let updated: serde_json::Value = client
        .patch(format!("http://{addr}/api/jobs/{job_id}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "name": "daily-report-v2" }))
        .send()
        .await
        .expect("update request")
        .json()
        .await
        .expect("update json");
    assert_eq!(updated["name"], "daily-report-v2");
    {
        let store = job_store.lock().unwrap();
        assert_eq!(store.get_job(&job_id).unwrap().name, "daily-report-v2");
    }

    // delete
    let delete_resp = client
        .delete(format!("http://{addr}/api/jobs/{job_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("delete request");
    assert_eq!(delete_resp.status(), reqwest::StatusCode::NO_CONTENT);
    {
        let store = job_store.lock().unwrap();
        assert!(store.get_job(&job_id).is_none());
        assert!(store.list_jobs().is_empty());
    }

    cancel.cancel();
}

#[tokio::test]
async fn created_job_is_persisted_not_only_in_memory() {
    let key = unique_key("persisted");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/jobs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "name": "survives-reload",
            "prompt": "check status",
            "schedule": "every 30m",
        }))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let job_id = created["id"].as_str().expect("id").to_string();

    // Reload from disk through the SAME store handle — proves the mutation
    // was persisted through save(), not left only in the in-memory Vec.
    {
        let mut store = job_store.lock().unwrap();
        store.reload().expect("reload");
        assert_eq!(store.get_job(&job_id).unwrap().name, "survives-reload");
    }

    cancel.cancel();
}

#[tokio::test]
async fn fetch_resolves_by_identifier_and_by_name() {
    let key = unique_key("resolve-by-name");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/jobs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "name": "Named-Job",
            "prompt": "do a thing",
            "schedule": "every 45m",
        }))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let job_id = created["id"].as_str().expect("id").to_string();

    let by_id: serde_json::Value = client
        .get(format!("http://{addr}/api/jobs/{job_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch by id")
        .json()
        .await
        .expect("json");
    assert_eq!(by_id["id"], job_id);

    // Case-insensitive name resolution, matching JobStore::find_job.
    let by_name: serde_json::Value = client
        .get(format!("http://{addr}/api/jobs/named-job"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch by name")
        .json()
        .await
        .expect("json");
    assert_eq!(by_name["id"], job_id);

    cancel.cancel();
}

#[tokio::test]
async fn update_writes_only_the_supplied_fields() {
    let key = unique_key("partial-update");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/jobs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "name": "original-name",
            "prompt": "original prompt",
            "schedule": "every 60m",
            "deliver": "local",
        }))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let job_id = created["id"].as_str().expect("id").to_string();
    let original_schedule_display = created["schedule_display"].clone();

    let updated: serde_json::Value = client
        .patch(format!("http://{addr}/api/jobs/{job_id}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "name": "renamed-only" }))
        .send()
        .await
        .expect("update request")
        .json()
        .await
        .expect("update json");

    assert_eq!(updated["name"], "renamed-only");
    assert_eq!(updated["prompt"], "original prompt");
    assert_eq!(updated["deliver"], "local");
    assert_eq!(updated["schedule_display"], original_schedule_display);

    cancel.cancel();
}

#[tokio::test]
async fn delete_removes_the_job() {
    let key = unique_key("delete-removes");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/jobs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "name": "to-delete",
            "prompt": "gone soon",
            "schedule": "every 10m",
        }))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    let job_id = created["id"].as_str().expect("id").to_string();

    client
        .delete(format!("http://{addr}/api/jobs/{job_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("delete request");

    let fetch_after = client
        .get(format!("http://{addr}/api/jobs/{job_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch after delete");
    assert_eq!(fetch_after.status(), reqwest::StatusCode::NOT_FOUND);

    let listed: serde_json::Value = client
        .get(format!("http://{addr}/api/jobs"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("list request")
        .json()
        .await
        .expect("list json");
    assert!(listed["jobs"].as_array().unwrap().is_empty());

    cancel.cancel();
}

#[tokio::test]
async fn unknown_job_is_not_found_on_every_verb() {
    let key = unique_key("unknown-not-found");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let fake_id = "00000000-0000-0000-0000-000000000000";

    let fetch = client
        .get(format!("http://{addr}/api/jobs/{fake_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("fetch");
    assert_eq!(fetch.status(), reqwest::StatusCode::NOT_FOUND);

    let update = client
        .patch(format!("http://{addr}/api/jobs/{fake_id}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "name": "should-not-be-created" }))
        .send()
        .await
        .expect("update");
    assert_eq!(update.status(), reqwest::StatusCode::NOT_FOUND);

    let delete = client
        .delete(format!("http://{addr}/api/jobs/{fake_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("delete");
    assert_eq!(delete.status(), reqwest::StatusCode::NOT_FOUND);

    // None of the above implicitly created a job.
    {
        let store = job_store.lock().unwrap();
        assert!(store.list_jobs().is_empty());
    }

    cancel.cancel();
}

#[tokio::test]
async fn malformed_schedule_is_a_client_error() {
    let key = unique_key("malformed-schedule");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{addr}/api/jobs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "name": "bad-schedule",
            "prompt": "do something",
            "schedule": "not a real schedule at all",
        }))
        .send()
        .await
        .expect("create request");
    assert!(
        resp.status().is_client_error(),
        "malformed schedule must be a client error, got {}",
        resp.status()
    );
    let body = resp.text().await.expect("body text");
    assert!(
        body.to_lowercase().contains("schedule"),
        "the client error must name the problem: {body}"
    );

    {
        let store = job_store.lock().unwrap();
        assert!(
            store.list_jobs().is_empty(),
            "a malformed schedule must persist nothing"
        );
    }

    cancel.cancel();
}

#[tokio::test]
async fn job_routes_require_the_bearer_key() {
    let key = unique_key("jobs-require-key");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let fake_id = "00000000-0000-0000-0000-000000000000";

    let cases: &[(Method, String)] = &[
        (Method::GET, "/api/jobs".to_string()),
        (Method::POST, "/api/jobs".to_string()),
        (Method::GET, format!("/api/jobs/{fake_id}")),
        (Method::PATCH, format!("/api/jobs/{fake_id}")),
        (Method::DELETE, format!("/api/jobs/{fake_id}")),
        (Method::POST, format!("/api/jobs/{fake_id}/pause")),
        (Method::POST, format!("/api/jobs/{fake_id}/resume")),
        (Method::POST, format!("/api/jobs/{fake_id}/run")),
    ];

    for (method, path) in cases {
        let resp = client
            .request(method.clone(), format!("http://{addr}{path}"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap_or_else(|e| panic!("{method} {path} request failed: {e}"));
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{method} {path} must require the bearer key"
        );
    }

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 2: pause / resume / trigger
// ---------------------------------------------------------------------------

async fn create_job(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    key: &str,
    name: &str,
    schedule: &str,
) -> String {
    let created: serde_json::Value = client
        .post(format!("http://{addr}/api/jobs"))
        .bearer_auth(key)
        .json(&serde_json::json!({
            "name": name,
            "prompt": "do something",
            "schedule": schedule,
        }))
        .send()
        .await
        .expect("create request")
        .json()
        .await
        .expect("create json");
    created["id"].as_str().expect("id").to_string()
}

#[tokio::test]
async fn pause_and_resume_toggle_the_stored_enabled_state() {
    let key = unique_key("pause-resume-toggle");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let job_id = create_job(&client, addr, &key, "toggle-me", "every 60m").await;

    client
        .post(format!("http://{addr}/api/jobs/{job_id}/pause"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("pause request");
    {
        let store = job_store.lock().unwrap();
        let job = store.get_job(&job_id).unwrap();
        assert!(!job.enabled);
        assert_eq!(job.state, ironhermes_cron::JobState::Paused);
    }

    client
        .post(format!("http://{addr}/api/jobs/{job_id}/resume"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("resume request");
    {
        let store = job_store.lock().unwrap();
        let job = store.get_job(&job_id).unwrap();
        assert!(job.enabled);
        assert_eq!(job.state, ironhermes_cron::JobState::Scheduled);
    }

    cancel.cancel();
}

#[tokio::test]
async fn paused_job_is_not_due() {
    let key = unique_key("paused-not-due");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let job_id = create_job(&client, addr, &key, "due-then-paused", "every 60m").await;

    // Backdate next_run_at directly in the store so the job is due.
    {
        let mut store = job_store.lock().unwrap();
        let job = store.jobs_mut().iter_mut().find(|j| j.id == job_id).unwrap();
        job.next_run_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
        store.save().expect("save backdated next_run_at");
    }

    client
        .post(format!("http://{addr}/api/jobs/{job_id}/pause"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("pause request");

    let mut store = job_store.lock().unwrap();
    let due = store.get_due_jobs();
    assert!(
        due.iter().all(|j| j.id != job_id),
        "a paused job must be excluded from the store's due-jobs selection — the pause must \
         reach the scheduler, not only the HTTP response"
    );

    cancel.cancel();
}

#[tokio::test]
async fn pause_is_idempotent() {
    let key = unique_key("pause-idempotent");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let job_id = create_job(&client, addr, &key, "double-pause", "every 60m").await;

    let first = client
        .post(format!("http://{addr}/api/jobs/{job_id}/pause"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("first pause");
    assert!(first.status().is_success());

    let second = client
        .post(format!("http://{addr}/api/jobs/{job_id}/pause"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("second pause");
    assert!(
        second.status().is_success(),
        "pausing an already-paused job must succeed, got {}",
        second.status()
    );

    let store = job_store.lock().unwrap();
    let job = store.get_job(&job_id).unwrap();
    assert!(!job.enabled);
    assert_eq!(job.state, ironhermes_cron::JobState::Paused);

    cancel.cancel();
}

#[tokio::test]
async fn run_triggers_the_job_through_the_store() {
    let key = unique_key("run-triggers");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    // A schedule far in the future — next_run_at starts well beyond "now".
    let job_id = create_job(&client, addr, &key, "trigger-me", "every 1440m").await;
    let before = chrono::Utc::now() - chrono::Duration::seconds(5);

    client
        .post(format!("http://{addr}/api/jobs/{job_id}/run"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("run request");
    let after = chrono::Utc::now() + chrono::Duration::seconds(5);

    let store = job_store.lock().unwrap();
    let job = store.get_job(&job_id).unwrap();
    let next_run_at = job.next_run_at.expect("next_run_at set");
    assert!(
        next_run_at >= before && next_run_at <= after,
        "trigger must set next_run_at ~= now through the store's own trigger_job, got {next_run_at:?}"
    );

    cancel.cancel();
}

#[tokio::test]
async fn pause_resume_and_run_are_not_found_for_an_unknown_job() {
    let key = unique_key("lifecycle-not-found");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let fake_id = "00000000-0000-0000-0000-000000000000";

    for action in ["pause", "resume", "run"] {
        let resp = client
            .post(format!("http://{addr}/api/jobs/{fake_id}/{action}"))
            .bearer_auth(&key)
            .send()
            .await
            .unwrap_or_else(|e| panic!("{action} request failed: {e}"));
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::NOT_FOUND,
            "{action} on an unknown job must be not-found"
        );
    }

    cancel.cancel();
}

#[tokio::test]
async fn lifecycle_routes_require_the_bearer_key() {
    let key = unique_key("lifecycle-requires-key");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let fake_id = "00000000-0000-0000-0000-000000000000";

    for action in ["pause", "resume", "run"] {
        let resp = client
            .post(format!("http://{addr}/api/jobs/{fake_id}/{action}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("{action} request failed: {e}"));
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 3: capabilities-vs-router drift, reachability, omissions, agreement
// ---------------------------------------------------------------------------

const ROUTES_MOD_RS: &str = include_str!("../src/api_server/routes/mod.rs");

/// Parse the `name: "..."` entries out of `FAMILIES`'s source text.
fn family_names_in_source() -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    for line in ROUTES_MOD_RS.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name: \"")
            && let Some(end) = rest.find('"')
        {
            names.insert(rest[..end].to_string());
        }
    }
    names
}

/// Parse the `.merge(<module>::router())` calls out of `build_router`'s
/// source text.
fn merged_module_names_in_source() -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    for line in ROUTES_MOD_RS.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(".merge(")
            && let Some(idx) = rest.find("::router())")
        {
            names.insert(rest[..idx].to_string());
        }
    }
    names
}

/// D-03: the set of families `FAMILIES` declares and the set of modules
/// actually `.merge()`d into the router must be identical in both
/// directions — a family with no merge would mount nothing (caught by
/// `every_mounted_path_is_reachable` below as a 404), and a merge with no
/// FAMILIES entry would be invisible to the capabilities map and the
/// bearer-auth enumeration test alike. `axum` 0.8 exposes no runtime
/// router-introspection API (Plan 04's own key-decision), so this is a
/// source-level structural assertion rather than a live-router walk.
#[test]
fn capabilities_map_and_router_do_not_drift() {
    let declared = family_names_in_source();
    let merged = merged_module_names_in_source();
    assert!(!declared.is_empty(), "sanity: FAMILIES must not be empty");
    assert!(!merged.is_empty(), "sanity: build_router must merge something");
    assert_eq!(
        declared, merged,
        "every family FAMILIES declares must have a merged router, and every merged router \
         module must have a FAMILIES entry — drift in either direction is a lie about the \
         mounted surface (D-03)"
    );
}

/// Every path `FAMILIES` advertises must actually resolve on the live
/// router. Driven with `TRACE` — a method no handler in this crate
/// registers — rather than `GET`: several handlers (`routes::runs::status`,
/// e.g.) legitimately answer a well-formed-but-nonexistent identifier with
/// an APPLICATION-level `404` that is indistinguishable, by status code
/// alone, from axum's own ROUTING `404` for a path that matches nothing.
/// `TRACE` sidesteps that ambiguity: axum's `MethodRouter` answers `405
/// Method Not Allowed` for any method it has no handler for on a MOUNTED
/// path, and only falls through to `404` when the path pattern itself
/// matches no route at all — so `405` here is the proof the map's claim is
/// backed by a live route, independent of what any individual handler does
/// with a bad identifier.
#[tokio::test]
async fn every_mounted_path_is_reachable() {
    let key = unique_key("reachable");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    for family in FAMILIES {
        for raw_path in family.paths {
            let path = raw_path
                .replace("{id}", "dummy-id")
                .replace("{run_id}", "dummy-run-id");
            let resp = client
                .request(Method::TRACE, format!("http://{addr}{path}"))
                .bearer_auth(&key)
                .send()
                .await
                .unwrap_or_else(|e| panic!("TRACE {path} failed: {e}"));
            assert_eq!(
                resp.status(),
                reqwest::StatusCode::METHOD_NOT_ALLOWED,
                "path {path} (family '{}') must be mounted — TRACE (registered nowhere) must \
                 hit 405 (method not allowed on a real route), not 404 (no route matched), got \
                 {}",
                family.name,
                resp.status()
            );
        }
    }

    cancel.cancel();
}

/// D-03: the two port-target endpoints this phase deliberately does not
/// carry must be named in the capabilities map with a reason each, so full
/// parity is reported honestly rather than inferred from absence.
#[tokio::test]
async fn deliberately_omitted_endpoints_are_named_with_reasons() {
    let key = unique_key("omitted-named");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
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

    let omitted = caps["omitted_endpoints"]
        .as_array()
        .expect("omitted_endpoints must be an array");
    assert_eq!(omitted.len(), 2, "exactly two endpoints are deliberately omitted");
    for entry in omitted {
        let name = entry["name"].as_str().unwrap_or_default();
        let reason = entry["reason"].as_str().unwrap_or_default();
        assert!(!name.is_empty(), "omitted endpoint must be named: {entry}");
        assert!(!reason.is_empty(), "omitted endpoint must carry a reason: {entry}");
    }
    let names: Vec<&str> = omitted.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(
        names
            .iter()
            .any(|n| n.to_lowercase().contains("callback") || n.to_lowercase().contains("webhook")),
        "must name the generic per-platform callback ingress: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.to_lowercase().contains("cron")),
        "must name the managed-cron fire webhook: {names:?}"
    );

    cancel.cancel();
}

/// Generalises `capabilities_and_responses_routes_agree` (Plan 08) to every
/// feature the map reports, not only the Responses API: a feature reported
/// available must have no route returning `501`, and a feature reported
/// unavailable must have at least one that does.
#[tokio::test]
async fn not_implemented_features_are_reported_as_unavailable() {
    let key = unique_key("features-agree");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let (handles, _job_store, _cron_dir) = test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction"),
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
    let features = caps["features"].as_object().expect("features object");
    assert!(!features.is_empty(), "sanity: features must not be empty");

    for (feature_key, expected_available) in features {
        let family_name = feature_key.strip_suffix("_api").unwrap_or(feature_key);
        let family = FAMILIES
            .iter()
            .find(|f| f.name == family_name)
            .unwrap_or_else(|| {
                panic!("feature '{feature_key}' has no matching FAMILIES entry '{family_name}'")
            });
        let expected_available = expected_available.as_bool().expect("bool value");

        let mut saw_not_implemented = false;
        'paths: for raw_path in family.paths {
            let path = raw_path
                .replace("{id}", "dummy-id")
                .replace("{run_id}", "dummy-run-id");
            for method in [Method::GET, Method::POST, Method::PATCH, Method::DELETE] {
                let resp = client
                    .request(method.clone(), format!("http://{addr}{path}"))
                    .bearer_auth(&key)
                    .json(&serde_json::json!({}))
                    .send()
                    .await
                    .unwrap_or_else(|e| panic!("{method} {path} failed: {e}"));
                if resp.status() == reqwest::StatusCode::NOT_IMPLEMENTED {
                    saw_not_implemented = true;
                    break 'paths;
                }
            }
        }
        assert_eq!(
            !saw_not_implemented, expected_available,
            "feature '{feature_key}' claims available={expected_available} but observed \
             not-implemented={saw_not_implemented}"
        );
    }

    cancel.cancel();
}
