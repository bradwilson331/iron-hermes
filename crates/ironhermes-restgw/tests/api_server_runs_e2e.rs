//! `/v1/runs*` proof suite — Phase 36.7.1 Plan 06.
//!
//! Task 1's registry coverage lives in `ironhermes-core`'s own
//! `concurrency_integration.rs`. This file covers Task 2 (submit/status/stop/
//! approval over the process-wide `TurnRegistry`) and Task 3 (the SSE run event
//! stream).
//!
//! Most behaviors here register a stub `TurnEntry` directly against the shared
//! `TurnRegistry` (mirroring `crates/ironhermes-gateway/tests/concurrent_turns_gateway.rs`'s
//! own `stub_turn` helper) rather than driving `POST /v1/runs` for every case — this lets
//! a test control exactly which surface a turn originated on, exactly when its
//! cancellation token fires, and exactly what `StreamEvent`s it emits, without needing a
//! real `AgentLoop` wired into this plan (that wiring is a later plan's concern; this
//! plan's own registry-integration mechanics are what is under test). Only the tests
//! whose names say "submitted"/"submitting" drive the real `POST /v1/runs` handler.
//!
//! Every listener binds `127.0.0.1:0` (ephemeral) — never port 8642.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ironhermes_agent::client::StreamEvent;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::concurrency::{Surface, TurnEntry, TurnId};
use ironhermes_core::{ApprovalGate, ApprovalOutcome, MessageEvent, ModelRegistry};
use ironhermes_restgw::api_server::{
    ApiServerAdapter, ApiServerConfig, ApiServerHandles, serve_api_server_adapter,
};
use ironhermes_restgw::api_server::sse::RunEventRegistry;
use ironhermes_tools::ToolRegistry;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared fixtures (mirrors tests/api_server_auth.rs's own helpers)
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

fn test_handles() -> ApiServerHandles {
    let db_path = std::env::temp_dir().join(format!(
        "ih-restgw-runs-test-state-{}.db",
        uuid::Uuid::new_v4()
    ));
    ApiServerHandles {
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

/// Mirrors `concurrent_turns_gateway.rs`'s own `stub_turn` — a `TurnEntry` this test
/// controls directly, parked until its `CancellationToken` fires.
fn stub_entry(session_id: &str, surface: Surface) -> (TurnId, CancellationToken, TurnEntry) {
    let turn_id = TurnId::new_v4();
    let cancel = CancellationToken::new();
    let entry = TurnEntry {
        turn_id,
        session_id: session_id.to_string(),
        surface,
        started_at: Instant::now(),
        cancel: cancel.clone(),
    };
    (turn_id, cancel, entry)
}

/// A minimal, self-contained `ApprovalGate` — deliberately NOT
/// `ironhermes_gateway::approval::ApprovalCoordinator` (this crate cannot depend on
/// `ironhermes-gateway`; see `ApiServerHandles::approval_gate`'s own doc comment for
/// why). Sufficient to exercise the REST route's `ApprovalGate::resolve` call site,
/// which is polymorphic over any implementation of the trait.
struct FakeApprovalGate {
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl FakeApprovalGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl ApprovalGate for FakeApprovalGate {
    async fn request_approval(
        &self,
        session_id: &str,
        _tool_name: &str,
        _reason: &str,
        _args: &serde_json::Value,
    ) -> ApprovalOutcome {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(session_id.to_string(), tx);
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(true)) => ApprovalOutcome::Approved,
            Ok(Ok(false)) => ApprovalOutcome::Denied,
            Ok(Err(_)) => ApprovalOutcome::Denied,
            Err(_) => ApprovalOutcome::TimedOut,
        }
    }

    async fn resolve(&self, session_id: &str, approved: bool) -> bool {
        if let Some(tx) = self.pending.lock().await.remove(session_id) {
            tx.send(approved).is_ok()
        } else {
            false
        }
    }
}

/// Parse an SSE response body (`data: <json>\n\n` blocks) into the JSON payloads.
fn parse_sse_frames(body: &str) -> Vec<serde_json::Value> {
    body.split("\n\n")
        .filter_map(|block| block.strip_prefix("data: ").or_else(|| block.strip_prefix("data:")))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| serde_json::from_str(s).unwrap_or_else(|e| panic!("frame is valid JSON: {e}: {s}")))
        .collect()
}

// ---------------------------------------------------------------------------
// Task 2: submit, status, stop, approval
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submitted_run_is_observable_before_it_completes() {
    let key = unique_key("submit-observable");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let submit_resp: serde_json::Value = client
        .post(format!("http://{addr}/v1/runs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "hello" }))
        .send()
        .await
        .expect("submit request")
        .json()
        .await
        .expect("submit json");
    let run_id = submit_resp["run_id"].as_str().expect("run_id string").to_string();

    // The stub turn sleeps 150ms before completing — poll immediately.
    let status_resp: serde_json::Value = client
        .get(format!("http://{addr}/v1/runs/{run_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("status json");
    assert_eq!(
        status_resp["status"], "in_flight",
        "status while the stub turn is still blocked must report in_flight: {status_resp:?}"
    );

    cancel.cancel();
}

#[tokio::test]
async fn run_is_registered_before_the_task_is_spawned() {
    let key = unique_key("register-before-spawn");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let submit_resp: serde_json::Value = client
        .post(format!("http://{addr}/v1/runs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "" }))
        .send()
        .await
        .expect("submit request")
        .json()
        .await
        .expect("submit json");
    let run_id = submit_resp["run_id"].as_str().expect("run_id string").to_string();

    let status_resp: serde_json::Value = client
        .get(format!("http://{addr}/v1/runs/{run_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("status request")
        .json()
        .await
        .expect("status json");
    assert_eq!(status_resp["status"], "in_flight");
    // The spawned stub turn sleeps 150ms before doing anything observable — an
    // elapsed_ms this far below that window proves the registry entry was
    // visible essentially immediately after `register()` returned, well
    // before the spawned work had any chance to run to completion.
    let elapsed = status_resp["elapsed_ms"].as_u64().expect("elapsed_ms number");
    assert!(
        elapsed < 100,
        "elapsed_ms ({elapsed}) should be small — entry was registered before the \
         150ms-sleeping spawned task could have completed"
    );

    cancel.cancel();
}

#[tokio::test]
async fn rest_runs_report_their_own_surface() {
    let key = unique_key("report-surface");
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

    // A REST-submitted run.
    let submit_resp: serde_json::Value = client
        .post(format!("http://{addr}/v1/runs"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "prompt": "" }))
        .send()
        .await
        .expect("submit request")
        .json()
        .await
        .expect("submit json");
    let rest_run_id = submit_resp["run_id"].as_str().expect("run_id string").to_string();

    // A gateway-originated turn registered directly in the SAME registry.
    let (gw_turn_id, _gw_cancel, gw_entry) = stub_entry("gw:chat:user", Surface::Gateway);
    registry.register(gw_entry).await;

    let rest_status: serde_json::Value = client
        .get(format!("http://{addr}/v1/runs/{rest_run_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("rest status request")
        .json()
        .await
        .expect("rest status json");
    assert_eq!(rest_status["surface"], "api_server");

    let gw_status: serde_json::Value = client
        .get(format!("http://{addr}/v1/runs/{gw_turn_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("gw status request")
        .json()
        .await
        .expect("gw status json");
    assert_eq!(gw_status["surface"], "gateway");
    assert_ne!(rest_status["surface"], gw_status["surface"]);

    cancel.cancel();
}

#[tokio::test]
async fn stop_cancels_the_in_flight_turn() {
    let key = unique_key("stop-cancels");
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

    let (turn_id, turn_cancel, entry) = stub_entry("stop-session", Surface::ApiServer);
    registry.register(entry).await;
    assert!(!turn_cancel.is_cancelled());

    let resp = client
        .post(format!("http://{addr}/v1/runs/{turn_id}/stop"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("stop request");
    assert!(resp.status().is_success());
    assert!(
        turn_cancel.is_cancelled(),
        "stopping a run must signal its cancellation token"
    );

    cancel.cancel();
}

#[tokio::test]
async fn stop_for_an_unknown_run_reports_not_found() {
    let key = unique_key("stop-unknown");
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
        .post(format!("http://{addr}/v1/runs/{}/stop", Uuid::new_v4()))
        .bearer_auth(&key)
        .send()
        .await
        .expect("stop request");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    cancel.cancel();
}

#[tokio::test]
async fn approval_resolution_unblocks_a_parked_turn() {
    let key = unique_key("approval-unblocks");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let mut handles = test_handles();
    let registry = handles.turn_registry.clone();
    let gate = FakeApprovalGate::new();
    handles.approval_gate = Some(gate.clone() as Arc<dyn ApprovalGate>);
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let (turn_id, _turn_cancel, entry) =
        stub_entry(&TurnId::new_v4().to_string(), Surface::ApiServer);
    // Mirror the real submit-time invariant: run_id (turn_id) IS the session
    // identifier the gate parks under.
    let session_id = turn_id.to_string();
    let entry = TurnEntry {
        session_id: session_id.clone(),
        ..entry
    };
    registry.register(entry).await;

    let outcome: Arc<Mutex<Option<ApprovalOutcome>>> = Arc::new(Mutex::new(None));
    let park_gate = gate.clone();
    let park_session = session_id.clone();
    let park_outcome = outcome.clone();
    tokio::spawn(async move {
        let result = park_gate
            .request_approval(&park_session, "some_tool", "because", &serde_json::json!({}))
            .await;
        *park_outcome.lock().await = Some(result);
    });
    // Give the parking task a moment to insert its pending entry.
    tokio::time::sleep(Duration::from_millis(30)).await;

    let resp = client
        .post(format!("http://{addr}/v1/runs/{turn_id}/approval"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "approved": true }))
        .send()
        .await
        .expect("approval request");
    assert!(resp.status().is_success());

    // Poll for the parked task to observe the resolution — well before the
    // gate's own 5s internal timeout.
    let mut resolved = None;
    for _ in 0..50 {
        if let Some(o) = *outcome.lock().await {
            resolved = Some(o);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(resolved, Some(ApprovalOutcome::Approved));

    cancel.cancel();
}

#[tokio::test]
async fn approval_for_an_unknown_run_reports_not_found() {
    let key = unique_key("approval-unknown");
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
        .post(format!("http://{addr}/v1/runs/{}/approval", Uuid::new_v4()))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "approved": true }))
        .send()
        .await
        .expect("approval request");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    cancel.cancel();
}

#[tokio::test]
async fn approval_for_a_run_from_another_surface_is_refused() {
    let key = unique_key("approval-cross-surface");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let mut handles = test_handles();
    let registry = handles.turn_registry.clone();
    let gate = FakeApprovalGate::new();
    handles.approval_gate = Some(gate.clone() as Arc<dyn ApprovalGate>);
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    // A turn registered under a NON-REST surface, parked on the SAME shared
    // coordinator — the exact T-36.7.1-48 scenario (e.g. a Telegram-originated
    // turn awaiting operator approval).
    let (turn_id, _turn_cancel, entry) = stub_entry("gw:chat:operator", Surface::Gateway);
    registry.register(entry).await;

    let park_gate = gate.clone();
    let outcome: Arc<Mutex<Option<ApprovalOutcome>>> = Arc::new(Mutex::new(None));
    let park_outcome = outcome.clone();
    tokio::spawn(async move {
        let result = park_gate
            .request_approval(
                "gw:chat:operator",
                "dangerous_tool",
                "because",
                &serde_json::json!({}),
            )
            .await;
        *park_outcome.lock().await = Some(result);
    });
    tokio::time::sleep(Duration::from_millis(30)).await;

    // The REST route must refuse — the run's origin surface is Gateway, not
    // ApiServer — BEFORE ever touching the coordinator.
    let resp = client
        .post(format!("http://{addr}/v1/runs/{turn_id}/approval"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "approved": true }))
        .send()
        .await
        .expect("approval request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "an approval parked by a non-REST-surface turn must be refused"
    );

    // The parked approval must STILL be pending — the REST call never
    // resolved it. Resolve it directly through the gate to prove it is still
    // there (a `resolve` on an already-consumed entry would return `false`).
    let directly_resolved = gate.resolve("gw:chat:operator", true).await;
    assert!(
        directly_resolved,
        "the pending approval must still exist after the refused REST call"
    );
    let mut resolved = None;
    for _ in 0..50 {
        if let Some(o) = *outcome.lock().await {
            resolved = Some(o);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        resolved,
        Some(ApprovalOutcome::Approved),
        "the parked turn must still be resolvable by the real mechanism afterward"
    );

    cancel.cancel();
}

/// T-36.7.1-48, second half (security audit AR-02).
///
/// The surface check alone is NOT sufficient, because `submit` is not the only
/// producer of `Surface::ApiServer` entries: `sessions::chat_stream` registers
/// one whose `session_id` is the caller-supplied path parameter. A REST caller
/// can therefore name ANY session in the shared store — including one owned by
/// Telegram, the TUI, the web UI or the CLI — mint an `ApiServer`-surface turn
/// against it, and then present that turn's id here. The surface check passes
/// (the surface really is `ApiServer`), and before this fix `gate.resolve` was
/// then called with that FOREIGN `session_id`.
///
/// The route must authorise on the same key it acts on.
#[tokio::test]
async fn approval_for_an_api_server_turn_on_a_foreign_session_is_refused() {
    let key = unique_key("approval-foreign-session");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let mut handles = test_handles();
    let registry = handles.turn_registry.clone();
    let gate = FakeApprovalGate::new();
    handles.approval_gate = Some(gate.clone() as Arc<dyn ApprovalGate>);
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    // Surface::ApiServer — as `chat_stream` registers it — but the session id
    // belongs to a Telegram conversation, not to this turn.
    const FOREIGN_SESSION: &str = "gw:chat:operator";
    let (turn_id, _turn_cancel, entry) = stub_entry(FOREIGN_SESSION, Surface::ApiServer);
    registry.register(entry).await;

    let park_gate = gate.clone();
    let outcome: Arc<Mutex<Option<ApprovalOutcome>>> = Arc::new(Mutex::new(None));
    let park_outcome = outcome.clone();
    tokio::spawn(async move {
        let result = park_gate
            .request_approval(
                FOREIGN_SESSION,
                "dangerous_tool",
                "because",
                &serde_json::json!({}),
            )
            .await;
        *park_outcome.lock().await = Some(result);
    });
    tokio::time::sleep(Duration::from_millis(30)).await;

    let resp = client
        .post(format!("http://{addr}/v1/runs/{turn_id}/approval"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "approved": true }))
        .send()
        .await
        .expect("approval request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "an ApiServer-surface turn whose session_id is not its own run id must be \
         refused — otherwise a REST caller resolves another surface's approval"
    );

    // Prove the foreign approval was never consumed: a `resolve` on an
    // already-consumed entry returns `false`.
    let directly_resolved = gate.resolve(FOREIGN_SESSION, true).await;
    assert!(
        directly_resolved,
        "the foreign pending approval must still exist after the refused REST call"
    );
    let mut resolved = None;
    for _ in 0..50 {
        if let Some(o) = *outcome.lock().await {
            resolved = Some(o);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        resolved,
        Some(ApprovalOutcome::Approved),
        "the foreign turn must still be resolvable by its own mechanism afterward"
    );

    cancel.cancel();
}

#[tokio::test]
async fn unknown_run_id_is_not_found_on_every_verb() {
    let key = unique_key("unknown-every-verb");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let unknown = Uuid::new_v4();

    let status = client
        .get(format!("http://{addr}/v1/runs/{unknown}"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("status request");
    assert_eq!(status.status(), reqwest::StatusCode::NOT_FOUND);

    let stop = client
        .post(format!("http://{addr}/v1/runs/{unknown}/stop"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("stop request");
    assert_eq!(stop.status(), reqwest::StatusCode::NOT_FOUND);

    let approval = client
        .post(format!("http://{addr}/v1/runs/{unknown}/approval"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "approved": true }))
        .send()
        .await
        .expect("approval request");
    assert_eq!(approval.status(), reqwest::StatusCode::NOT_FOUND);

    cancel.cancel();
}

#[tokio::test]
async fn run_routes_require_the_bearer_key() {
    let key = unique_key("run-routes-bearer");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let run_id = Uuid::new_v4();

    let submit = client
        .post(format!("http://{addr}/v1/runs"))
        .json(&serde_json::json!({ "prompt": "x" }))
        .send()
        .await
        .expect("submit request");
    assert_eq!(submit.status(), reqwest::StatusCode::UNAUTHORIZED);

    let status = client
        .get(format!("http://{addr}/v1/runs/{run_id}"))
        .send()
        .await
        .expect("status request");
    assert_eq!(status.status(), reqwest::StatusCode::UNAUTHORIZED);

    let events = client
        .get(format!("http://{addr}/v1/runs/{run_id}/events"))
        .send()
        .await
        .expect("events request");
    assert_eq!(events.status(), reqwest::StatusCode::UNAUTHORIZED);

    let approval = client
        .post(format!("http://{addr}/v1/runs/{run_id}/approval"))
        .json(&serde_json::json!({ "approved": true }))
        .send()
        .await
        .expect("approval request");
    assert_eq!(approval.status(), reqwest::StatusCode::UNAUTHORIZED);

    let stop = client
        .post(format!("http://{addr}/v1/runs/{run_id}/stop"))
        .send()
        .await
        .expect("stop request");
    assert_eq!(stop.status(), reqwest::StatusCode::UNAUTHORIZED);

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 3: SSE run event stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sse_frames_map_only_known_variants() {
    let key = unique_key("sse-known-variants");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let handles = test_handles();
    let registry = handles.turn_registry.clone();
    let events_registry = handles.run_events.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let (turn_id, _turn_cancel, entry) = stub_entry("sse-known-variants", Surface::ApiServer);
    registry.register(entry).await;
    let tx = events_registry.register(turn_id).await;
    tx.send(StreamEvent::ContentDelta("hi".to_string())).unwrap();
    tx.send(StreamEvent::ToolCallDelta {
        index: 0,
        id: Some("call-1".to_string()),
        name: Some("test_tool".to_string()),
        arguments: Some("{}".to_string()),
    })
    .unwrap();
    tx.send(StreamEvent::Usage(ironhermes_core::Usage {
        prompt_tokens: 1,
        completion_tokens: 1,
        total_tokens: 2,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    }))
    .unwrap();
    tx.send(StreamEvent::ProviderError("boom".to_string()))
        .unwrap();
    drop(tx);

    let body = client
        .get(format!("http://{addr}/v1/runs/{turn_id}/events"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("events request")
        .text()
        .await
        .expect("events body");
    let frames = parse_sse_frames(&body);
    let known: [&str; 5] = [
        "content_delta",
        "tool_call_delta",
        "usage",
        "done",
        "provider_error",
    ];
    assert_eq!(frames.len(), 4, "expected 4 frames: {frames:?}");
    for frame in &frames {
        let ty = frame["type"].as_str().expect("type field");
        assert!(known.contains(&ty), "unexpected frame type: {ty}");
    }

    cancel.cancel();
}

#[tokio::test]
async fn content_deltas_arrive_in_order() {
    let key = unique_key("content-order");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let handles = test_handles();
    let registry = handles.turn_registry.clone();
    let events_registry = handles.run_events.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let (turn_id, _turn_cancel, entry) = stub_entry("content-order", Surface::ApiServer);
    registry.register(entry).await;
    let tx = events_registry.register(turn_id).await;
    for word in ["alpha", "beta", "gamma"] {
        tx.send(StreamEvent::ContentDelta(word.to_string())).unwrap();
    }
    tx.send(StreamEvent::Done(None)).unwrap();
    drop(tx);

    let body = client
        .get(format!("http://{addr}/v1/runs/{turn_id}/events"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("events request")
        .text()
        .await
        .expect("events body");
    let frames = parse_sse_frames(&body);
    let deltas: Vec<&str> = frames
        .iter()
        .filter(|f| f["type"] == "content_delta")
        .map(|f| f["text"].as_str().expect("text field"))
        .collect();
    assert_eq!(deltas, vec!["alpha", "beta", "gamma"]);

    cancel.cancel();
}

#[tokio::test]
async fn tool_call_deltas_are_forwarded() {
    let key = unique_key("tool-call-forward");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let handles = test_handles();
    let registry = handles.turn_registry.clone();
    let events_registry = handles.run_events.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let (turn_id, _turn_cancel, entry) = stub_entry("tool-call-forward", Surface::ApiServer);
    registry.register(entry).await;
    let tx = events_registry.register(turn_id).await;
    tx.send(StreamEvent::ToolCallDelta {
        index: 2,
        id: Some("call-xyz".to_string()),
        name: Some("web_search".to_string()),
        arguments: Some("{\"q\":\"rust\"}".to_string()),
    })
    .unwrap();
    tx.send(StreamEvent::Done(None)).unwrap();
    drop(tx);

    let body = client
        .get(format!("http://{addr}/v1/runs/{turn_id}/events"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("events request")
        .text()
        .await
        .expect("events body");
    let frames = parse_sse_frames(&body);
    let tool_frame = frames
        .iter()
        .find(|f| f["type"] == "tool_call_delta")
        .expect("a tool_call_delta frame must be present");
    assert_eq!(tool_frame["index"], 2);
    assert_eq!(tool_frame["id"], "call-xyz");
    assert_eq!(tool_frame["name"], "web_search");
    assert_eq!(tool_frame["arguments"], "{\"q\":\"rust\"}");

    cancel.cancel();
}

#[tokio::test]
async fn stream_terminates_on_done() {
    let key = unique_key("stream-done");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let handles = test_handles();
    let registry = handles.turn_registry.clone();
    let events_registry = handles.run_events.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let (turn_id, _turn_cancel, entry) = stub_entry("stream-done", Surface::ApiServer);
    registry.register(entry).await;
    let tx = events_registry.register(turn_id).await;
    tx.send(StreamEvent::ContentDelta("last words".to_string()))
        .unwrap();
    tx.send(StreamEvent::Done(Some("stop".to_string()))).unwrap();
    drop(tx);

    // `.text()` blocks until the server closes the response body — completing
    // within the test's own timeout IS the "closes cleanly" assertion.
    let body = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .get(format!("http://{addr}/v1/runs/{turn_id}/events"))
            .bearer_auth(&key)
            .send(),
    )
    .await
    .expect("request must not hang")
    .expect("events request")
    .text()
    .await
    .expect("events body");
    let frames = parse_sse_frames(&body);
    assert_eq!(frames.last().unwrap()["type"], "done");
    assert_eq!(frames.last().unwrap()["reason"], "stop");

    cancel.cancel();
}

#[tokio::test]
async fn provider_error_is_forwarded_then_the_stream_closes() {
    let key = unique_key("stream-provider-error");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let handles = test_handles();
    let registry = handles.turn_registry.clone();
    let events_registry = handles.run_events.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let (turn_id, _turn_cancel, entry) = stub_entry("stream-provider-error", Surface::ApiServer);
    registry.register(entry).await;
    let tx = events_registry.register(turn_id).await;
    tx.send(StreamEvent::ContentDelta("partial".to_string()))
        .unwrap();
    tx.send(StreamEvent::ProviderError(
        "(429 Too Many Requests): rate limited".to_string(),
    ))
    .unwrap();
    // No further sends, and drop tx — the producer stops after forwarding the
    // error, exactly as a real turn would.
    drop(tx);

    let body = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .get(format!("http://{addr}/v1/runs/{turn_id}/events"))
            .bearer_auth(&key)
            .send(),
    )
    .await
    .expect("request must not hang")
    .expect("events request")
    .text()
    .await
    .expect("events body");
    let frames = parse_sse_frames(&body);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["type"], "content_delta");
    assert_eq!(frames[1]["type"], "provider_error");
    assert_eq!(frames[1]["message"], "(429 Too Many Requests): rate limited");

    cancel.cancel();
}

#[tokio::test]
async fn stream_for_an_unknown_run_is_not_found() {
    let key = unique_key("stream-unknown");
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
        .get(format!("http://{addr}/v1/runs/{}/events", Uuid::new_v4()))
        .bearer_auth(&key)
        .send()
        .await
        .expect("events request");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    cancel.cancel();
}

#[tokio::test]
async fn stream_route_requires_the_bearer_key() {
    let key = unique_key("stream-bearer");
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
        .get(format!("http://{addr}/v1/runs/{}/events", Uuid::new_v4()))
        .send()
        .await
        .expect("events request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    cancel.cancel();
}

#[tokio::test]
async fn cancelled_run_terminates_its_stream() {
    let key = unique_key("stream-cancelled");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let handles = test_handles();
    let registry = handles.turn_registry.clone();
    let events_registry = handles.run_events.clone();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let (turn_id, _turn_cancel, entry) = stub_entry("stream-cancelled", Surface::ApiServer);
    registry.register(entry).await;
    let tx = events_registry.register(turn_id).await;
    tx.send(StreamEvent::ContentDelta("still going".to_string()))
        .unwrap();
    // Deliberately do NOT drop `tx` — the producer has not finished. Only
    // `/stop`'s cancellation should end the stream.

    let stream_client = client.clone();
    let events_addr = addr;
    let bearer = key.clone();
    let fetch = tokio::spawn(async move {
        stream_client
            .get(format!("http://{events_addr}/v1/runs/{turn_id}/events"))
            .bearer_auth(&bearer)
            .send()
            .await
            .expect("events request")
            .text()
            .await
            .expect("events body")
    });

    // Give the SSE connection a moment to open and receive the first frame.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let stop_resp = client
        .post(format!("http://{addr}/v1/runs/{turn_id}/stop"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("stop request");
    assert!(stop_resp.status().is_success());

    let body = tokio::time::timeout(Duration::from_secs(5), fetch)
        .await
        .expect("stream must close within the timeout after /stop")
        .expect("fetch task");
    let frames = parse_sse_frames(&body);
    assert!(
        frames.iter().any(|f| f["type"] == "content_delta"),
        "the frame sent before cancellation must still have been forwarded: {frames:?}"
    );

    // `tx` is still alive here (never dropped) — proves the stream closed
    // because of cancellation, not because the channel closed.
    drop(tx);
    cancel.cancel();
}
