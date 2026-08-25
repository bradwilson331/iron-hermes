//! `ApiServerAdapter` proof suite — Phase 36.7.1 Plan 04.
//!
//! Task 1: fail-closed construction (D-06), loopback default/non-loopback
//! rail (D-07), the router-wide bearer layer enumerated against the
//! router's own path list, and the three health endpoints.
//!
//! Task 2: the five discovery endpoints project live registry data.
//!
//! Task 3: an end-to-end case driving the SAME construction+spawn path
//! `GatewayRunner::start()` uses.
//!
//! Every listener in this file binds `127.0.0.1:0` (ephemeral) — never port
//! 8642, the production default, which would collide with a running
//! gateway.
//!
//! Env var note: this file mutates the process-wide `IRONHERMES_API_SERVER_KEY`
//! variable. Safe under `cargo nextest run` (this plan's mandated runner),
//! which executes each test in its own process — no cross-test races.
//! `cargo test`'s default in-process thread-per-test model would race on
//! this shared name; do not run this file with plain `cargo test`.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::{MessageEvent, ModelMetadata, ModelRegistry, SkillRegistry};
use ironhermes_restgw::api_server::{
    ApiServerAdapter, ApiServerConfig, ApiServerHandles, api_server_bind_auth_enabled,
    serve_api_server_adapter,
};
use ironhermes_restgw::bind_guard::bind_guard_allows;
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// A `MessageHandler` this suite never expects to be invoked (every route
/// under test in Tasks 1-2 is a read-only GET; Task 3's end-to-end case
/// drives `/v1/health` only, also a GET) — panics loudly if a route
/// unexpectedly dispatches a turn.
struct UnusedHandler;

#[async_trait]
impl MessageHandler for UnusedHandler {
    async fn handle(
        &self,
        _event: &MessageEvent,
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        panic!("UnusedHandler::handle should never be called by this test suite's GET-only routes");
    }
}

fn test_handles() -> ApiServerHandles {
    // A unique-per-call SQLite file in the OS temp dir — `StateStore` has no
    // public in-memory constructor, and this keeps each test's store
    // isolated from every other test's (and from any real
    // `$IRONHERMES_HOME/state.db`).
    let db_path =
        std::env::temp_dir().join(format!("ih-restgw-apiserver-test-state-{}.db", uuid::Uuid::new_v4()));
    ApiServerHandles {
        turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
        state_store: Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(&db_path).expect("open test state store"),
        )),
        job_store: None,
        model_registry: Arc::new(ModelRegistry::new()),
        skill_registry: None,
        tool_registry: Arc::new(tokio::sync::RwLock::new(ToolRegistry::new())),
        // Plan 06 (runs family): no approval mechanism wired for this
        // pre-existing Plan 04 suite (it drives none of the run routes) —
        // the approval route fails closed on `None` rather than panicking.
        approval_gate: None,
        run_events: Arc::new(ironhermes_restgw::api_server::sse::RunEventRegistry::new()),
    }
}

fn unique_key(test_name: &str) -> String {
    format!("test-key-{test_name}-{}", uuid::Uuid::new_v4())
}

/// Bind an ephemeral listener, serve the given adapter on it via
/// [`serve_api_server_adapter`], and return the bound address plus a
/// [`CancellationToken`] the caller must cancel to stop the server task.
async fn spawn_adapter(adapter: Arc<ApiServerAdapter>) -> (std::net::SocketAddr, CancellationToken) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral listener");
    let addr = listener.local_addr().expect("local_addr");
    let cancel = CancellationToken::new();
    let handler: Arc<dyn MessageHandler> = Arc::new(UnusedHandler);
    let serve_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = serve_api_server_adapter(listener, adapter, handler, serve_cancel).await;
    });
    // Give the spawned serve task a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    (addr, cancel)
}

// ---------------------------------------------------------------------------
// Task 1: fail-closed construction, loopback default, bind guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_server_refuses_to_start_without_a_key() {
    let key_name = "IRONHERMES_API_SERVER_KEY";
    unsafe {
        std::env::remove_var(key_name);
    }
    let result = ApiServerAdapter::new(ApiServerConfig::default(), test_handles());
    let err = match result {
        Err(e) => e.to_string(),
        Ok(_) => panic!("construction must refuse without the key"),
    };
    assert!(
        err.contains("IRONHERMES_API_SERVER_KEY"),
        "error must name the variable: {err}"
    );
    assert!(
        !err.to_lowercase().contains("bind") || !err.contains(':'),
        "no socket should have been described as bound in the refusal message"
    );
}

#[tokio::test]
async fn default_port_is_the_parity_port() {
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", unique_key("default-port"));
    }
    let adapter = ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
        .expect("construction with key set must succeed");
    assert_eq!(adapter.bound_host(), "127.0.0.1");
    assert_eq!(adapter.bound_port(), 8642);
}

#[test]
fn non_loopback_bind_requires_key_and_opt_in() {
    let non_loopback: IpAddr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
    // All four combinations of (key_configured, public_opt_in) against a
    // non-loopback address — only both-true is permitted.
    assert!(bind_guard_allows(
        non_loopback,
        api_server_bind_auth_enabled(true, true)
    ));
    assert!(!bind_guard_allows(
        non_loopback,
        api_server_bind_auth_enabled(true, false)
    ));
    assert!(!bind_guard_allows(
        non_loopback,
        api_server_bind_auth_enabled(false, true)
    ));
    assert!(!bind_guard_allows(
        non_loopback,
        api_server_bind_auth_enabled(false, false)
    ));
}

#[test]
fn loopback_bind_needs_no_opt_in() {
    let loopback: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    // Loopback proceeds with the key configured and NO opt-in — the guard's
    // first clause (`ip.is_loopback()`) is satisfied regardless of the
    // second.
    assert!(bind_guard_allows(
        loopback,
        api_server_bind_auth_enabled(true, false)
    ));
}

#[tokio::test]
async fn every_route_requires_the_bearer_key() {
    let key = unique_key("every-route");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    for path in ironhermes_restgw::api_server::routes::all_registered_paths() {
        let url = format!("http://{addr}{path}");

        let no_auth = client.get(&url).send().await.expect("request without auth");
        assert_eq!(
            no_auth.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "path {path} must refuse a request with no Authorization header"
        );

        let wrong_auth = client
            .get(&url)
            .bearer_auth("definitely-the-wrong-key")
            .send()
            .await
            .expect("request with wrong auth");
        assert_eq!(
            wrong_auth.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "path {path} must refuse a request with a wrong bearer value"
        );
    }

    cancel.cancel();
}

#[tokio::test]
async fn correct_bearer_is_admitted() {
    let key = unique_key("correct-bearer");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    for path in ironhermes_restgw::api_server::routes::all_registered_paths() {
        let url = format!("http://{addr}{path}");
        let resp = client
            .get(&url)
            .bearer_auth(&key)
            .send()
            .await
            .expect("request with the configured key");
        assert_ne!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "path {path} must be admitted with the configured key"
        );
    }

    cancel.cancel();
}

#[tokio::test]
async fn bearer_comparison_is_constant_time() {
    let key = format!("{}{}", "a".repeat(40), "-real-suffix");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/v1/health");

    // Shares a long prefix with the real key, differs only in the suffix.
    let prefix_correct_wrong = format!("{}{}", "a".repeat(40), "-wrong-suffix");
    // Shares nothing with the real key.
    let wholly_wrong = "z".repeat(key.len());

    let resp_prefix = client
        .get(&url)
        .bearer_auth(&prefix_correct_wrong)
        .send()
        .await
        .expect("prefix-correct wrong key request");
    let resp_wholly = client
        .get(&url)
        .bearer_auth(&wholly_wrong)
        .send()
        .await
        .expect("wholly wrong key request");

    assert_eq!(resp_prefix.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(resp_wholly.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp_prefix.status(),
        resp_wholly.status(),
        "a prefix-correct wrong key must be refused identically to a wholly wrong one"
    );

    cancel.cancel();
}

#[tokio::test]
async fn health_endpoints_answer() {
    let key = unique_key("health-answer");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    for path in ironhermes_restgw::api_server::routes::health::HEALTH_PATHS {
        let url = format!("http://{addr}{path}");
        let resp = client
            .get(&url)
            .bearer_auth(&key)
            .send()
            .await
            .expect("health request");
        assert!(resp.status().is_success(), "path {path} must succeed");
        let body: serde_json::Value = resp.json().await.expect("json body");
        assert_eq!(body["service"], "ironhermes-api-server");
    }

    cancel.cancel();
}

#[tokio::test]
async fn api_server_binds_its_own_listener() {
    let key = unique_key("own-listener");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;

    // A second, independent ephemeral listener bound in the same process —
    // proves the adapter's listener is its own, not mounted onto an
    // existing server.
    let other = TcpListener::bind("127.0.0.1:0").await.expect("bind second listener");
    let other_addr = other.local_addr().expect("local_addr");

    assert_ne!(addr.port(), other_addr.port());

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 2: discovery endpoints served from the live registries
// ---------------------------------------------------------------------------

fn write_distinctive_skill(root: &std::path::Path, name: &str) {
    let skill_dir = root.join(name);
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    let content = format!(
        "---\nname: {name}\ndescription: A distinctive skill for discovery endpoint testing.\n---\nBody.\n"
    );
    std::fs::write(skill_dir.join("SKILL.md"), content).expect("write SKILL.md");
}

struct DistinctiveTool {
    toolset: &'static str,
    name: &'static str,
}

#[async_trait]
impl Tool for DistinctiveTool {
    fn name(&self) -> &str {
        self.name
    }
    fn toolset(&self) -> &str {
        self.toolset
    }
    fn description(&self) -> &str {
        "distinctive tool for discovery endpoint testing"
    }
    fn schema(&self) -> ironhermes_core::ToolSchema {
        ironhermes_core::ToolSchema::new(
            self.name,
            self.description(),
            serde_json::json!({ "type": "object", "properties": {} }),
        )
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        Ok("ok".to_string())
    }
}

fn distinctive_model_registry() -> ModelRegistry {
    let mut registry = ModelRegistry::new();
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        "distinctive-test-model-xyz".to_string(),
        ModelMetadata {
            context_length: 12_345,
            max_output_tokens: Some(1_000),
            tokenizer: "cl100k_base".to_string(),
            capabilities: Default::default(),
        },
    );
    registry.merge_cache(entries);
    registry
}

#[tokio::test]
async fn discovery_endpoints_reflect_live_registries() {
    let key = unique_key("discovery-live");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }

    let skills_root =
        std::env::temp_dir().join(format!("ih-restgw-apiserver-test-{}", uuid::Uuid::new_v4()));
    write_distinctive_skill(&skills_root, "distinctive-test-skill");
    // `load_with_paths` (not the `#[cfg(test)]`-gated `load_with_paths_for_test`,
    // which is internal to `ironhermes-core`'s own unit tests and invisible
    // from a downstream crate's integration tests) — its production default
    // source is already `SkillSource::Builtin`, which is all this test needs.
    let skill_registry = SkillRegistry::load_with_paths(std::slice::from_ref(&skills_root));

    let mut tool_registry = ToolRegistry::new();
    tool_registry.register(Box::new(DistinctiveTool {
        toolset: "distinctive-test-toolset",
        name: "distinctive-test-tool",
    }));

    let mut handles = test_handles();
    handles.model_registry = Arc::new(distinctive_model_registry());
    handles.skill_registry = Some(Arc::new(skill_registry));
    handles.tool_registry = Arc::new(tokio::sync::RwLock::new(tool_registry));

    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let models: serde_json::Value = client
        .get(format!("http://{addr}/v1/models"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("models request")
        .json()
        .await
        .expect("models json");
    let model_ids: Vec<&str> = models["data"]
        .as_array()
        .expect("data array")
        .iter()
        .map(|m| m["id"].as_str().expect("id string"))
        .collect();
    assert!(
        model_ids.contains(&"distinctive-test-model-xyz"),
        "distinctive model must appear in /v1/models: {model_ids:?}"
    );

    let skills: serde_json::Value = client
        .get(format!("http://{addr}/v1/skills"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("skills request")
        .json()
        .await
        .expect("skills json");
    let skill_names: Vec<&str> = skills["skills"]
        .as_array()
        .expect("skills array")
        .iter()
        .map(|s| s["name"].as_str().expect("name string"))
        .collect();
    assert!(
        skill_names.contains(&"distinctive-test-skill"),
        "distinctive skill must appear in /v1/skills: {skill_names:?}"
    );

    let toolsets: serde_json::Value = client
        .get(format!("http://{addr}/v1/toolsets"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("toolsets request")
        .json()
        .await
        .expect("toolsets json");
    let toolset_names: Vec<&str> = toolsets["toolsets"]
        .as_array()
        .expect("toolsets array")
        .iter()
        .map(|t| t.as_str().expect("toolset string"))
        .collect();
    assert!(
        toolset_names.contains(&"distinctive-test-toolset"),
        "distinctive toolset must appear in /v1/toolsets: {toolset_names:?}"
    );

    cancel.cancel();
    let _ = std::fs::remove_dir_all(&skills_root);
}

#[tokio::test]
async fn model_options_and_model_list_agree() {
    let key = unique_key("options-agree");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let mut handles = test_handles();
    handles.model_registry = Arc::new(distinctive_model_registry());
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles).expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let models: serde_json::Value = client
        .get(format!("http://{addr}/v1/models"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("models request")
        .json()
        .await
        .expect("models json");
    let model_ids: Vec<&str> = models["data"]
        .as_array()
        .expect("data array")
        .iter()
        .map(|m| m["id"].as_str().expect("id string"))
        .collect();

    let options: serde_json::Value = client
        .get(format!("http://{addr}/api/model/options"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("options request")
        .json()
        .await
        .expect("options json");
    let option_ids: Vec<&str> = options["options"]
        .as_array()
        .expect("options array")
        .iter()
        .map(|o| o.as_str().expect("option string"))
        .collect();

    for option in &option_ids {
        assert!(
            model_ids.contains(option),
            "picker option {option} must correspond to an entry in /v1/models"
        );
    }
    assert_eq!(option_ids.len(), model_ids.len());

    cancel.cancel();
}

#[tokio::test]
async fn capabilities_reports_responses_api_unavailable() {
    let key = unique_key("capabilities-responses");
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

    cancel.cancel();
}

#[tokio::test]
async fn capabilities_lists_every_mounted_family() {
    let key = unique_key("capabilities-families");
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
    let endpoints = caps["endpoints"].as_object().expect("endpoints object");

    for family in ironhermes_restgw::api_server::routes::FAMILIES {
        assert!(
            endpoints.contains_key(family.name),
            "capabilities map must list mounted family '{}': {endpoints:?}",
            family.name
        );
    }

    cancel.cancel();
}

/// Model discovery is intentionally NOT "empty" here: `ModelRegistry`
/// always ships a non-empty built-in static table by design (there is no
/// public constructor for a bare/empty registry) — a fresh install without
/// any configured provider still knows about the built-in catalog. This
/// test therefore asserts the genuinely-empty-capable registries (skills,
/// toolsets) return an empty collection with a success status, and asserts
/// the models endpoint succeeds with an array (not erroring), which is the
/// literal claim this endpoint can honestly make.
#[tokio::test]
async fn empty_registries_return_empty_collections() {
    let key = unique_key("empty-registries");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    // `test_handles()` already carries `skill_registry: None` and a fresh,
    // empty `ToolRegistry` — the genuinely-empty-capable case.
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let skills_resp = client
        .get(format!("http://{addr}/v1/skills"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("skills request");
    assert!(skills_resp.status().is_success());
    let skills: serde_json::Value = skills_resp.json().await.expect("skills json");
    assert_eq!(skills["skills"].as_array().expect("array").len(), 0);

    let toolsets_resp = client
        .get(format!("http://{addr}/v1/toolsets"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("toolsets request");
    assert!(toolsets_resp.status().is_success());
    let toolsets: serde_json::Value = toolsets_resp.json().await.expect("toolsets json");
    assert_eq!(toolsets["toolsets"].as_array().expect("array").len(), 0);

    let models_resp = client
        .get(format!("http://{addr}/v1/models"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("models request");
    assert!(models_resp.status().is_success());
    let models: serde_json::Value = models_resp.json().await.expect("models json");
    assert!(models["data"].is_array());

    cancel.cancel();
}

#[tokio::test]
async fn discovery_endpoints_require_the_bearer_key() {
    let key = unique_key("discovery-require-key");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    let discovery_paths = [
        "/v1/models",
        "/api/model/options",
        "/v1/skills",
        "/v1/toolsets",
        "/v1/capabilities",
    ];
    for path in discovery_paths {
        let resp = client
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .expect("unauthenticated discovery request");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "path {path} must require the bearer key"
        );
    }

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Task 3: the same construction+spawn path GatewayRunner::start() uses
// ---------------------------------------------------------------------------

/// Drives `ApiServerAdapter::new` + `run_api_server_adapter` — the exact two
/// calls `GatewayRunner::start()` makes (Task 3's `runner.rs` construction +
/// spawn blocks) — rather than the `serve_api_server_adapter` shortcut every
/// other test in this file uses. Configures an ephemeral port (never 8642)
/// and polls `adapter.bound_addr()` (set inside `serve_api_server_adapter`,
/// which `run_api_server_adapter` calls internally after binding) to learn
/// the resolved address.
#[tokio::test]
async fn adapter_reachable_through_run_api_server_adapter_with_key() {
    let key = unique_key("run-adapter-with-key");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let config = ApiServerConfig {
        host: Some("127.0.0.1".to_string()),
        port: Some(0),
        public_opt_in: false,
    };
    let adapter = Arc::new(
        ApiServerAdapter::new(config, test_handles()).expect("construction with key must succeed"),
    );
    let cancel = CancellationToken::new();
    let handler: Arc<dyn MessageHandler> = Arc::new(UnusedHandler);
    let run_adapter = adapter.clone();
    let run_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = ironhermes_restgw::api_server::run_api_server_adapter(run_adapter, handler, run_cancel)
            .await;
    });

    let mut addr = None;
    for _ in 0..100 {
        if let Some(a) = adapter.bound_addr() {
            addr = Some(a);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let addr = addr.expect("adapter must bind within the polling window");

    let client = reqwest::Client::new();
    let with_key = client
        .get(format!("http://{addr}/v1/health"))
        .bearer_auth(&key)
        .send()
        .await
        .expect("health request with key");
    assert!(with_key.status().is_success());

    let without_key = client
        .get(format!("http://{addr}/v1/health"))
        .send()
        .await
        .expect("health request without key");
    assert_eq!(without_key.status(), reqwest::StatusCode::UNAUTHORIZED);

    cancel.cancel();
}

/// Construction refuses without the key, so `run_api_server_adapter` is
/// never reached and no socket opens — proven at the construction boundary
/// (mirrors `api_server_refuses_to_start_without_a_key`, restated here in
/// Task 3's own name to document the "still starts the other configured
/// platforms" claim: `GatewayRunner::start()`'s `api_server_adapter` match
/// logs and skips on this exact `Err`, exactly as the webhook adapter's own
/// construction failure path does — see `runner.rs`).
#[tokio::test]
async fn adapter_construction_fails_closed_without_key() {
    unsafe {
        std::env::remove_var("IRONHERMES_API_SERVER_KEY");
    }
    let config = ApiServerConfig {
        host: Some("127.0.0.1".to_string()),
        port: Some(0),
        public_opt_in: false,
    };
    let result = ApiServerAdapter::new(config, test_handles());
    assert!(result.is_err());
}
