//! Finding 7 (D-20) — proving the CORS ABSENCE claim by test, over the real
//! serve stack, rather than trusting "we checked and there is nothing there".
//!
//! There are zero cross-origin references anywhere under
//! `crates/ironhermes-restgw/src` today. This suite proves three things:
//!
//! 1. No cross-origin response header is ever emitted, on a normal request,
//!    on a request that sends an `Origin` header (the echo-the-origin case),
//!    or on a preflight-shaped request — across more than one real route,
//!    driven over a real ephemeral `TcpListener`, through the real
//!    [`ironhermes_restgw::api_server::serve_api_server_adapter`] serve
//!    path. This crate has no `tower` dependency (confirmed absent from
//!    `Cargo.toml` in Plan 01's Wave 0), so the harness drives real HTTP
//!    requests with `reqwest` rather than an in-process service call.
//! 2. The real preflight status is asserted explicitly, not assumed — a
//!    future accidental cross-origin layer that changes the status (without
//!    necessarily adding the headers this suite already checks) is caught.
//! 3. The absence assertion is not vacuously true: a permissive cross-origin
//!    middleware, hand-written and applied ONLY inside this test's own
//!    harness (never under `src/`), is proven to flip the SAME header check
//!    the absence tests use.
//!
//! Every listener binds `127.0.0.1:0` (ephemeral) — never port 8642.

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::{MessageEvent, ModelRegistry};
use ironhermes_restgw::api_server::{
    ApiServerAdapter, ApiServerConfig, ApiServerHandles, serve_api_server_adapter,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

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
        panic!("UnusedHandler::handle should never be called by this suite's GET-only routes");
    }
}

fn test_handles() -> ApiServerHandles {
    let db_path = std::env::temp_dir().join(format!(
        "ih-restgw-cors-test-state-{}.db",
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
        tool_registry: Arc::new(tokio::sync::RwLock::new(ironhermes_tools::ToolRegistry::new())),
        approval_gate: None,
        run_events: Arc::new(ironhermes_restgw::api_server::sse::RunEventRegistry::new()),
    }
}

fn unique_key(test_name: &str) -> String {
    format!("test-key-{test_name}-{}", uuid::Uuid::new_v4())
}

/// Bind an ephemeral listener, serve the REAL adapter on it via
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
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    (addr, cancel)
}

// ---------------------------------------------------------------------------
// The shared header check — used IDENTICALLY by the real-router absence
// tests below and by `cors_absence_assertion_detects_an_injected_permissive_layer`.
// Without this being the SAME function on both sides, the whole absence
// suite would pass equally well if the header lookup were simply misspelled.
// ---------------------------------------------------------------------------

/// The three response headers a cross-origin ("CORS") layer would emit.
/// Named as literal lowercase strings (HTTP header lookups are
/// case-insensitive, but writing them lowercase here keeps the source
/// grep-able for exactly what this suite checks).
const CROSS_ORIGIN_RESPONSE_HEADERS: [&str; 3] = [
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-allow-headers",
];

/// Returns the subset of [`CROSS_ORIGIN_RESPONSE_HEADERS`] present on
/// `headers`. Empty means none of the three cross-origin headers were
/// emitted. This is the ONE function both the real-router absence tests and
/// the non-vacuity test call — proving they exercise the same check.
fn present_cross_origin_headers(headers: &HeaderMap) -> Vec<&'static str> {
    CROSS_ORIGIN_RESPONSE_HEADERS
        .iter()
        .copied()
        .filter(|name| headers.contains_key(*name))
        .collect()
}

// ---------------------------------------------------------------------------
// Test 1: no cross-origin header on a normal request, across two real routes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_cors_headers_on_normal_request() {
    let key = unique_key("no-cors-normal");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    for path in ["/health", "/v1/models"] {
        let resp = client
            .get(format!("http://{addr}{path}"))
            .bearer_auth(&key)
            .send()
            .await
            .unwrap_or_else(|e| panic!("request to {path} failed: {e}"));
        let found = present_cross_origin_headers(resp.headers());
        assert!(
            found.is_empty(),
            "a normal request to {path} must emit none of the cross-origin headers, found: {found:?}"
        );
    }

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Test 2: sending an `Origin` header does not cause an echo.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_cors_headers_when_origin_header_sent() {
    let key = unique_key("no-cors-origin-echo");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    for path in ["/health", "/v1/models"] {
        let resp = client
            .get(format!("http://{addr}{path}"))
            .bearer_auth(&key)
            .header("Origin", "https://evil.example")
            .send()
            .await
            .unwrap_or_else(|e| panic!("request to {path} failed: {e}"));
        let found = present_cross_origin_headers(resp.headers());
        assert!(
            found.is_empty(),
            "a request carrying a foreign Origin header to {path} must not be echoed back as a \
             cross-origin allow header, found: {found:?}"
        );
    }

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Test 3: a preflight-shaped request — assert the REAL status, not an
// assumed one, plus the absence of the three headers.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn preflight_request_returns_expected_status_and_no_cors_headers() {
    let key = unique_key("preflight-status");
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), test_handles())
            .expect("construction must succeed"),
    );
    let (addr, cancel) = spawn_adapter(adapter).await;
    let client = reqwest::Client::new();

    // `/v1/models` registers only `GET` (see
    // `crates/ironhermes-restgw/src/api_server/routes/models.rs`). No route
    // in this crate registers an OPTIONS handler, so axum's own default
    // routing answers a method it does not recognise for a path it does
    // know with `405 Method Not Allowed` — 54-RESEARCH.md Pitfall 5 predicts
    // this exact status; this test asserts it as observed fact.
    let resp = client
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/v1/models"))
        .bearer_auth(&key)
        .header("Origin", "https://evil.example")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await
        .expect("preflight-shaped request");

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::METHOD_NOT_ALLOWED,
        "observed status for an OPTIONS request against /v1/models (a GET-only route) — a \
         future accidental cross-origin layer that answers 200 instead must be caught here even \
         if a narrower header-only check would not catch it"
    );
    let found = present_cross_origin_headers(resp.headers());
    assert!(
        found.is_empty(),
        "a preflight-shaped request must emit none of the cross-origin headers, found: {found:?}"
    );

    cancel.cancel();
}

// ---------------------------------------------------------------------------
// Test 4 (SC-8 non-vacuity proof): a permissive cross-origin middleware,
// hand-written and applied ONLY inside this test's own harness, must make
// the SAME header check (`present_cross_origin_headers`) find a header.
// Without this test, the absence suite above would pass equally well if
// `present_cross_origin_headers` looked up the wrong header name.
//
// This middleware — and every cross-origin construction in this file —
// exists ONLY here, inside the test harness. None appears under
// `crates/ironhermes-restgw/src/`.
// ---------------------------------------------------------------------------

async fn inject_permissive_cross_origin_header(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    response.headers_mut().insert(
        HeaderName::from_static("access-control-allow-origin"),
        HeaderValue::from_static("*"),
    );
    response
}

async fn harness_only_handler() -> impl IntoResponse {
    "harness-only ok"
}

#[tokio::test]
async fn cors_absence_assertion_detects_an_injected_permissive_layer() {
    // A tiny router, built entirely inside this test — not the real
    // `ApiServerAdapter` router — with a hand-written middleware that
    // inserts a permissive cross-origin header. No CORS crate, no `tower`,
    // no dependency on production route code.
    let router: Router = Router::new()
        .route("/harness-only", get(harness_only_handler))
        .layer(middleware::from_fn(inject_permissive_cross_origin_header));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral listener");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/harness-only"))
        .send()
        .await
        .expect("harness-only request");

    let found = present_cross_origin_headers(resp.headers());
    assert!(
        !found.is_empty(),
        "the SAME header check used by the absence tests above must detect the injected \
         permissive layer's header — an empty result here would mean the absence suite is \
         vacuously true"
    );
    assert!(
        found.contains(&"access-control-allow-origin"),
        "expected specifically access-control-allow-origin to be detected, found: {found:?}"
    );
}
