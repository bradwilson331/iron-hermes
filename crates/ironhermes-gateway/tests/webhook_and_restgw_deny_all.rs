//! D-08 gaps 2 and 3 (Phase 49.1 Plan 05): the webhook route and restgw
//! independent-path proofs.
//!
//! Gap 2 — the webhook path (`PlatformGatewayConfig.routes: Vec<WebhookRoute>`)
//! authenticates by signature scheme, NOT by `PlatformGatewayConfig.whitelist`
//! — a second, independent route to the same agent regardless of whitelist
//! state. Tests 1-2 prove this by driving a REAL bound listener
//! (`ironhermes_restgw::webhook::serve_webhook_adapter`, the exact
//! production entry point `GatewayRunner::start()` calls) with real HTTP
//! requests over `reqwest`, carrying a real HMAC-SHA256 `generic_v2`
//! signature — never a stub or a reimplemented verifier.
//!
//! Gap 3 — restgw's `api_server_bind_auth_enabled` fails closed when
//! `api_key` is unset: `ApiServerAdapter::new` returns `Err` (test 5) AND
//! `GatewayRunner::start()` skips spawning the platform when construction
//! fails (test 6). Both halves are asserted separately per the plan's own
//! prohibition against inferring one from the other.
//!
//! Test 7 proves every currently-registered restgw route requires the
//! bearer key by driving `all_registered_paths()` (the router's own source
//! of truth) rather than a hand-written list, against a REAL bound listener
//! via `ironhermes_restgw::api_server::serve_api_server_adapter`.
//!
//! ## Test 6's proof shape (read before extending)
//!
//! `GatewayRunner::start()` cannot be practically constructed in a unit
//! test — this repository's own `tests/gateway_shutdown.rs` documents
//! exactly this limitation ("Uses Path B ... rather than constructing a
//! full GatewayRunner (which requires a live TG token)") and adopts a
//! source-grep structural proof instead of a live construction. Test 6
//! below follows that same established, in-repo convention: it asserts,
//! by inspecting `runner.rs`'s own source, that (a) `ApiServerAdapter::new`
//! is called inside a `match` whose `Ok` arm is the ONLY place
//! `api_server_adapter` is ever set to `Some(...)`, and (b) the
//! `run_api_server_adapter` spawn is gated behind
//! `if let Some(adapter) = api_server_adapter.clone()`, with the
//! construction match appearing BEFORE the spawn guard in source order.
//! Combined with test 5's live proof that construction genuinely returns
//! `Err` when the key is unset, this establishes both halves of the
//! fail-closed claim: construction fails (live), and a failed construction
//! structurally cannot reach the spawn (source-verified) — the same
//! "Option-gated spawn never sees an absent adapter" shape
//! `worker_join_set_drains_on_cancel` uses for its own structural
//! assertion in `gateway_shutdown.rs`.
//!
//! ## Env var races (D-16, `IRONHERMES_API_SERVER_KEY`)
//!
//! Per this crate's own established finding (`ironhermes-cli`/gateway env
//! races — see CLAUDE.md project memory), tests that mutate
//! `IRONHERMES_API_SERVER_KEY` are NOT safe under nextest's default
//! multi-threaded-per-binary execution if another test in the SAME binary
//! also touches it — there is exactly one other test here that does (test
//! 7, which sets it to a fixed value). Run this file with
//! `--test-threads=1`:
//!
//! ```text
//! cargo nextest run -p ironhermes-gateway --test webhook_and_restgw_deny_all --no-fail-fast --test-threads=1
//! ```
//!
//! matching this plan's own `<verify>` block.

use std::net::IpAddr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::MessageEvent;
use ironhermes_cron::DeliveryRegistry;
use ironhermes_restgw::api_server::routes::all_registered_paths;
use ironhermes_restgw::api_server::{
    ApiServerAdapter, ApiServerConfig, ApiServerHandles, serve_api_server_adapter,
};
use ironhermes_restgw::bind_guard::bind_guard_allows;
use ironhermes_restgw::webhook::route_config::{
    DeliverTarget, OutboundAuth, RouteRails, SessionMode, SignatureKind, WebhookRoute,
    WebhookRoutesConfig,
};
use ironhermes_restgw::webhook::{WebhookAdapter, serve_webhook_adapter};
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

type HmacSha256 = Hmac<Sha256>;

// ===========================================================================
// Shared test-only env guard (ScopedEnv — mirrors
// `ironhermes-kanban/src/paths.rs:449-475`, the crate's own established
// RAII pattern for scoped env mutation in tests, extended here with an
// explicit `unset` constructor since Task 2's D-06 proof needs the
// "variable absent" case, not just "variable set to something else").
// ===========================================================================

struct ScopedEnv {
    key: String,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: this file always runs `IRONHERMES_API_SERVER_KEY`-touching
        // tests under `--test-threads=1` (see module doc) — no concurrent
        // env access within this binary.
        unsafe { std::env::set_var(key, value) };
        Self {
            key: key.to_string(),
            prev,
        }
    }

    fn unset(key: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: see `set` above.
        unsafe { std::env::remove_var(key) };
        Self {
            key: key.to_string(),
            prev,
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // SAFETY: see `set` above.
        match &self.prev {
            Some(v) => unsafe { std::env::set_var(&self.key, v) },
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}

// ===========================================================================
// Recording MessageHandler — substitutes for the real GatewayMessageHandler
// at the exact seam `RouteState.handler: Arc<dyn MessageHandler>` (webhook)
// exposes, exactly as `run_buzz_adapter` and `handle_webhook_post` both
// already parameterize over the trait in production.
// ===========================================================================

struct RecordingHandler {
    events: StdMutex<Vec<MessageEvent>>,
}

impl RecordingHandler {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: StdMutex::new(Vec::new()),
        })
    }

    fn call_count(&self) -> usize {
        self.events.lock().unwrap().len()
    }
}

#[async_trait]
impl MessageHandler for RecordingHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

async fn wait_for_dispatch(handler: &RecordingHandler, at_least: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handler.call_count() >= at_least {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("event never reached the handler")
}

// ===========================================================================
// Route/config fixtures — mirrors `webhook/mod.rs`'s own `route_with_path`
// test fixture, extended with the `generic_v2` signature arm that fixture
// deliberately left as `SignatureKind::None`.
// ===========================================================================

fn generic_v2_route(name: &str, path: &str, secret_env: &str) -> WebhookRoute {
    WebhookRoute {
        name: name.to_string(),
        path: path.to_string(),
        signature: SignatureKind::GenericV2,
        secret_env: Some(secret_env.to_string()),
        auth_token_env: None,
        public_key_env: None,
        timestamp_skew_secs: 300,
        prompt_template: "{Body}".to_string(),
        deliver: DeliverTarget::Platform,
        deliver_url: None,
        deliver_platform: Some("teststub".to_string()),
        deliver_chat_id: None,
        deliver_only: false,
        outbound_auth: OutboundAuth::None,
        session: SessionMode::Ephemeral,
        rails: RouteRails::default(),
    }
}

fn none_signature_route(name: &str, path: &str) -> WebhookRoute {
    WebhookRoute {
        name: name.to_string(),
        path: path.to_string(),
        signature: SignatureKind::None,
        secret_env: None,
        auth_token_env: None,
        public_key_env: None,
        timestamp_skew_secs: 300,
        prompt_template: "{Body}".to_string(),
        deliver: DeliverTarget::Platform,
        deliver_url: None,
        deliver_platform: Some("teststub".to_string()),
        deliver_chat_id: None,
        deliver_only: false,
        outbound_auth: OutboundAuth::None,
        session: SessionMode::Ephemeral,
        rails: RouteRails::default(),
    }
}

/// Real HMAC-SHA256 `generic_v2` signature headers — mirrors
/// `webhook/verifier.rs`'s own `signed_headers` test helper exactly
/// (`{timestamp}.{raw_body}`, hex-encoded). A genuine signature computed
/// with the SAME secret the route resolves from its `secret_env`, not a
/// stub — the verifier this drives is production code.
fn signed_headers(secret: &str, ts: i64, body: &[u8]) -> (String, String) {
    let ts_str = ts.to_string();
    let mut signed_content = Vec::with_capacity(ts_str.len() + 1 + body.len());
    signed_content.extend_from_slice(ts_str.as_bytes());
    signed_content.push(b'.');
    signed_content.extend_from_slice(body);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&signed_content);
    let sig_hex = hex::encode(mac.finalize().into_bytes());
    (sig_hex, ts_str)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Bind an ephemeral local listener and serve the given webhook route via
/// the REAL `serve_webhook_adapter` — the same production entry point
/// `run_webhook_adapter`/`GatewayRunner::start()` calls, just supplied its
/// own pre-bound listener instead of binding the configured host:port
/// (this crate's own doc comment on `serve_webhook_adapter` names this as
/// the intended test seam).
async fn spawn_webhook_adapter(
    route: WebhookRoute,
    handler: Arc<dyn MessageHandler>,
) -> (
    std::net::SocketAddr,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let adapter = Arc::new(
        WebhookAdapter::new(config, Arc::new(RwLock::new(DeliveryRegistry::new())))
            .expect("webhook adapter construction must succeed for a loopback-bound generic_v2 route"),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let addr = listener.local_addr().expect("local_addr");
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let task = tokio::spawn(async move {
        let _ = serve_webhook_adapter(listener, adapter, handler, cancel_task).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, cancel, task)
}

// ===========================================================================
// Tests 1-2: the webhook route is signature-gated, not whitelist-gated.
// ===========================================================================

/// D-08 gap 2, test 1: an EMPTY whitelist is not even a concept the webhook
/// route consults — `WebhookRoutesConfig`/`WebhookRoute` carry no
/// `whitelist` field at all. A request carrying a VALID `generic_v2`
/// signature reaches the agent regardless, documenting that the webhook
/// path is a second, independent route to the same agent, exactly as D-08
/// gap 2 states.
#[tokio::test]
async fn webhook_route_is_signature_gated_not_whitelist_gated() {
    let secret = "t1-generic-v2-secret-49-1-05";
    let _env = ScopedEnv::set("WEBHOOK_T1_SECRET", secret);
    let route = generic_v2_route("t1", "/hooks/t1", "WEBHOOK_T1_SECRET");
    let handler = RecordingHandler::new();
    let (addr, cancel, _task) =
        spawn_webhook_adapter(route, handler.clone() as Arc<dyn MessageHandler>).await;

    let body = br#"{"hello":"webhook world"}"#;
    let ts = now_unix();
    let (sig_hex, ts_str) = signed_headers(secret, ts, body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/hooks/t1"))
        .header("X-Webhook-Signature-V2", sig_hex)
        .header("X-Webhook-Timestamp", ts_str)
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await
        .expect("request must complete");

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "a validly-signed request must be accepted (202) with no whitelist concept involved"
    );

    wait_for_dispatch(&handler, 1).await;
    assert_eq!(
        handler.call_count(),
        1,
        "the validly-signed request must reach the agent — proving the webhook route is a \
         second, independent path (D-08 gap 2)"
    );

    cancel.cancel();
}

/// D-08 gap 2, test 2 (the control): the SAME route, an INVALID signature —
/// rejected. Pairs with test 1 so that test 1's delivery is attributable to
/// the signature being valid, not to an absent gate.
#[tokio::test]
async fn webhook_route_rejects_invalid_signature() {
    let secret = "t2-generic-v2-secret-49-1-05";
    let _env = ScopedEnv::set("WEBHOOK_T2_SECRET", secret);
    let route = generic_v2_route("t2", "/hooks/t2", "WEBHOOK_T2_SECRET");
    let handler = RecordingHandler::new();
    let (addr, cancel, _task) =
        spawn_webhook_adapter(route, handler.clone() as Arc<dyn MessageHandler>).await;

    let body = br#"{"hello":"webhook world"}"#;
    let ts = now_unix();
    // Signed with the WRONG secret — a real HMAC, just over the wrong key,
    // so this exercises the verifier's actual comparison rather than a
    // malformed-header short-circuit.
    let (sig_hex, ts_str) = signed_headers("not-the-real-secret", ts, body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/hooks/t2"))
        .header("X-Webhook-Signature-V2", sig_hex)
        .header("X-Webhook-Timestamp", ts_str)
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await
        .expect("request must complete");

    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "an invalid signature must be rejected with 401"
    );

    // Settle window: the invalid request must never reach the handler.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        handler.call_count(),
        0,
        "an invalidly-signed request must never reach the agent"
    );

    cancel.cancel();
}

// ===========================================================================
// Tests 3-4: `signature: none` is blocked at TWO independent layers on a
// non-loopback bind (D-10).
// ===========================================================================

/// D-08 gap 3 context (D-10): construction-time layer. `WebhookAdapter::new`
/// refuses outright when any route selects `signature: none` and the
/// configured bind host is not loopback.
#[tokio::test]
async fn webhook_signature_none_rejected_at_construction_on_non_loopback_bind() {
    let route = none_signature_route("t3", "/hooks/t3");
    let config = WebhookRoutesConfig {
        host: "0.0.0.0".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let result = WebhookAdapter::new(config, Arc::new(RwLock::new(DeliveryRegistry::new())));
    let Err(err) = result else {
        panic!("a signature:none route on a non-loopback bind host must be refused at construction");
    };
    assert!(
        err.to_string().contains("D-10"),
        "the refusal must cite D-10, got: {err}"
    );
}

/// D-08 gap 3 context (D-10): the SECOND, independent layer —
/// `bind_guard_allows`, evaluated again at bind time (defense in depth,
/// distinct code path from `WebhookAdapter::new`'s construction check
/// above). Proven directly against the pure predicate, which is exactly
/// what `run_webhook_adapter` calls before `TcpListener::bind`.
#[tokio::test]
async fn webhook_signature_none_also_blocked_at_bind_guard() {
    let non_loopback: IpAddr = "0.0.0.0".parse().unwrap();
    // `run_webhook_adapter` passes `auth_enabled = !adapter.requires_loopback`;
    // a `signature: none` route sets `requires_loopback = true`, so
    // `auth_enabled` is `false` here.
    assert!(
        !bind_guard_allows(non_loopback, false),
        "a non-loopback bind with no signature-based auth enabled must be refused"
    );
    // The loopback case must still be allowed regardless — this is a
    // non-loopback rail, not a blanket refusal.
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    assert!(
        bind_guard_allows(loopback, false),
        "loopback must always be allowed, auth_enabled or not"
    );
}

// ===========================================================================
// Tests 5-6: restgw's `IRONHERMES_API_SERVER_KEY`-unset fail-closed claim,
// both halves.
// ===========================================================================

fn build_test_handles() -> ApiServerHandles {
    ApiServerHandles {
        turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
        state_store: Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
        )),
        job_store: None,
        model_registry: Arc::new(ironhermes_core::ModelRegistry::new()),
        skill_registry: None,
        tool_registry: Arc::new(tokio::sync::RwLock::new(ironhermes_tools::ToolRegistry::new())),
        approval_gate: None,
        run_events: Arc::new(ironhermes_restgw::api_server::sse::RunEventRegistry::new()),
    }
}

/// D-08 gap 3, test 5: `ApiServerAdapter::new` returns `Err` when
/// `IRONHERMES_API_SERVER_KEY` is unset, and separately when set to the
/// empty string — the construction-time half of the fail-closed claim.
#[tokio::test]
async fn api_server_adapter_new_errs_when_key_unset() {
    {
        let _guard = ScopedEnv::unset("IRONHERMES_API_SERVER_KEY");
        let result = ApiServerAdapter::new(ApiServerConfig::default(), build_test_handles());
        assert!(
            result.is_err(),
            "construction must fail closed when IRONHERMES_API_SERVER_KEY is unset"
        );
    }
    {
        let _guard = ScopedEnv::set("IRONHERMES_API_SERVER_KEY", "");
        let result = ApiServerAdapter::new(ApiServerConfig::default(), build_test_handles());
        assert!(
            result.is_err(),
            "construction must fail closed when IRONHERMES_API_SERVER_KEY is the empty string"
        );
    }
}

/// D-08 gap 3, test 6: the runner-spawn half. `GatewayRunner::start()`
/// itself cannot be practically constructed in a unit test — its
/// dispatch/setup path requires a live Telegram bot token, a PID lock,
/// channel plumbing and more, which is exactly what this repo's own
/// `tests/gateway_shutdown.rs` already documents as impractical ("Uses Path
/// B ... rather than constructing a full GatewayRunner (which requires a
/// live TG token)"). Rather than a pure source-grep (which a mutation of
/// `ApiServerAdapter::new`'s RUNTIME behaviour could never turn red), this
/// test reproduces `runner.rs`'s own two-step shape verbatim — construct-or-skip,
/// then an `Option`-gated spawn — driven by the REAL `ApiServerAdapter::new`
/// call. A source anchor pins that this reproduction has not silently
/// drifted from the actual code it mirrors.
///
/// This is the runner's OWN control-flow idiom (`match ... { Ok(x) =>
/// Some(x), Err(_) => {} }` then `if let Some(x) = ...`), not a
/// reimplementation of `ApiServerAdapter::new`'s decision — that decision is
/// the real function call, so a mutation of it (e.g. "always return Ok even
/// with an empty key") turns this test red exactly as it turns test 5 red,
/// without either being inferred from the other: test 5 asserts the
/// `Result` value; this test asserts the spawn-or-skip CONSEQUENCE of
/// whatever that `Result` is.
#[tokio::test]
async fn runner_skips_platform_whose_adapter_failed_to_construct() {
    // Source anchor: confirm runner.rs's actual construction site still
    // assigns `Some(...)` in exactly one place (the Ok arm) and gates the
    // spawn behind an `if let Some(...)` — so the reproduction below cannot
    // silently diverge from production without this test itself going red.
    let src = include_str!("../src/runner.rs");
    assert!(
        src.contains("ironhermes_restgw::api_server::ApiServerAdapter::new("),
        "runner.rs must call ApiServerAdapter::new — has the construction site moved?"
    );
    assert_eq!(
        src.match_indices("api_server_adapter = Some(").count(),
        1,
        "api_server_adapter must be set to Some(...) in exactly one place (the construction \
         match's Ok arm)"
    );
    assert!(
        src.contains("Ok(adapter) => api_server_adapter = Some(Arc::new(adapter))"),
        "the sole Some(...) assignment must be the construction match's Ok arm"
    );
    assert!(
        src.contains("if let Some(adapter) = api_server_adapter.clone()"),
        "the api_server_adapter spawn must be gated behind an Option check, matching the \
         reproduction below"
    );

    // The reproduction: real construction, runner.rs's exact gating shape.
    let _guard = ScopedEnv::unset("IRONHERMES_API_SERVER_KEY");
    let mut api_server_adapter: Option<Arc<ApiServerAdapter>> = None;
    match ApiServerAdapter::new(ApiServerConfig::default(), build_test_handles()) {
        Ok(adapter) => api_server_adapter = Some(Arc::new(adapter)),
        Err(e) => {
            // Mirrors runner.rs's own `tracing::error!(...); ` — fail
            // closed for THIS platform only.
            tracing::error!("API server adapter construction failed: {e:#}. Skipping (fail-closed).");
        }
    }

    let mut spawned = false;
    if let Some(adapter) = api_server_adapter.clone() {
        spawned = true;
        let _ = adapter; // in production this is where run_api_server_adapter is spawned
    }

    assert!(
        !spawned,
        "GatewayRunner::start() must not spawn the API server adapter when \
         ApiServerAdapter::new failed to construct (IRONHERMES_API_SERVER_KEY unset)"
    );
}

// ===========================================================================
// Test 7: every registered restgw route requires the bearer key.
// ===========================================================================

/// D-08 gap 3 corollary: every path `all_registered_paths()` reports —
/// the router's own source of truth, not a hand-written list, so a route
/// family merged into `build_router` later is covered automatically —
/// requires the bearer key. Driven against a REAL bound listener via
/// `serve_api_server_adapter`, the same production entry point
/// `run_api_server_adapter`/`GatewayRunner::start()` uses. The router-wide
/// `from_fn_with_state(adapter, require_bearer)` layer (`routes/mod.rs:88-102`)
/// is applied once over the FULLY MERGED router, so it intercepts every
/// request before any inner route (or 404/405) is even considered — a
/// per-path assertion here is a check on THAT structural invariant, not on
/// 28+ separate hand-maintained guards.
#[tokio::test]
async fn restgw_router_requires_bearer_on_every_registered_path() {
    let _env = ScopedEnv::set("IRONHERMES_API_SERVER_KEY", "t7-bearer-key-49-1-05");
    let handles = build_test_handles();
    let adapter = Arc::new(
        ApiServerAdapter::new(ApiServerConfig::default(), handles)
            .expect("construction must succeed with the key set"),
    );
    let handler: Arc<dyn MessageHandler> = RecordingHandler::new();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral listener");
    let addr = listener.local_addr().expect("local_addr");
    let cancel = CancellationToken::new();
    let cancel_task = cancel.clone();
    let _task = tokio::spawn(async move {
        let _ = serve_api_server_adapter(listener, adapter, handler, cancel_task).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let paths = all_registered_paths();
    assert!(
        paths.len() >= 28,
        "expected at least 28 registered routes, got {} — a truncated FAMILIES list must not \
         pass this bound silently",
        paths.len()
    );

    let client = reqwest::Client::new();
    let mut checked = 0usize;
    for path in &paths {
        let resp = client
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("request to {path} must complete: {e}"));
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "path {path} must require the bearer key (got {})",
            resp.status()
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        paths.len(),
        "every registered path must have been checked — none silently skipped"
    );

    cancel.cancel();
}
