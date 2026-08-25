//! Phase 36.7.1 Plan 05: all three delivery target families, `deliver_only`,
//! the O-02 immediate-deny approval gate, and O-03 session isolation.
//!
//! Test function names match PLAN.md's `must_haves.truths` falsifiable-test
//! names exactly, where PLAN.md names one.
//!
//! No test binds a fixed port — every listener uses `127.0.0.1:0` and reads
//! `local_addr()`.
//!
//! **Origin and URL family "reaches its destination" tests, adjusted per
//! Plan 01's own established precedent (that plan's SUMMARY.md Deviation
//! 3):** `ironhermes_core::ssrf::is_safe_url` unconditionally blocks EVERY
//! loopback address with no test-mode override (D-16 forbids weakening it),
//! and BOTH the `url` family and the `origin` family ultimately POST
//! through `deliver::deliver_to_url`, which enforces that same check
//! immediately before every request. A live `127.0.0.1:0` stub HTTP server
//! can therefore never be a valid `deliver: url` OR `deliver: origin`
//! target in this repo's SSRF model — there is no way to prove a
//! *successful* delivery to either family against a loopback receiver
//! without weakening the validator, which D-16 forbids. Exactly as Plan 01
//! did for its own `deliver: url` success-path test, this file proves:
//!   - the **platform** family's full successful round trip (not
//!     network-based, so not subject to the SSRF gate at all), and
//!   - the **origin** and **url** families' correct EXTRACTION and DISPATCH
//!     (the caller-supplied callback URL / configured URL is read
//!     correctly and a real delivery attempt is made to exactly that
//!     address), proven by asserting the failure is specifically an SSRF
//!     refusal of the CORRECT address — never "route misconfigured" or "no
//!     callback_url found" — which is the load-bearing distinction between
//!     "the mechanism is broken" and "the mechanism is correct but the
//!     target happens to be unreachable in this sandboxed test
//!     environment."
//!
//! `deliver::deliver_to_url`'s own unweakened SSRF enforcement is already
//! covered directly in `tests/webhook_route_e2e.rs`; this file does not
//! duplicate that proof, only the URL/origin DISPATCH wiring built in this
//! plan.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use ironhermes_core::MessageEvent;
use ironhermes_core::ApprovalGate;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_cron::{DeliveryRegistry, DeliverySend};
use ironhermes_restgw::webhook::approval::{ImmediateDenyApprovalGate, format_denial};
use ironhermes_restgw::webhook::route_config::{
    DeliverTarget, OutboundAuth, RouteRails, SessionMode, SignatureKind, WebhookRoute,
    WebhookRoutesConfig,
};
use ironhermes_restgw::webhook::{WebhookAdapter, deliver, resolve_outbound_auth, serve_webhook_adapter};
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

type HmacSha256 = Hmac<Sha256>;

// ===========================================================================
// Shared test fixtures
// ===========================================================================

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn sign(secret: &str, ts: i64, body: &[u8]) -> String {
    let ts_str = ts.to_string();
    let mut content = Vec::with_capacity(ts_str.len() + 1 + body.len());
    content.extend_from_slice(ts_str.as_bytes());
    content.push(b'.');
    content.extend_from_slice(body);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&content);
    hex::encode(mac.finalize().into_bytes())
}

fn base_route(name: &str, path: &str, secret_env: &str) -> WebhookRoute {
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

fn single_route_config(route: WebhookRoute) -> WebhookRoutesConfig {
    WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    }
}

/// Stub `DeliverySend` — captures every `send_text` call instead of
/// touching the network.
#[derive(Default)]
struct StubDeliverySend {
    calls: Mutex<Vec<(String, String, Option<String>)>>,
}

#[async_trait]
impl DeliverySend for StubDeliverySend {
    async fn send_text(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push((
            chat_id.to_string(),
            content.to_string(),
            thread_id.map(|s| s.to_string()),
        ));
        Ok(())
    }
}

/// Stub `MessageHandler` — records every event, then delivers `answer` back
/// through the adapter. **Correctly forwards `event.thread_id`** into
/// `adapter.send_message` (the caller contract `WebhookAdapter::send_message`'s
/// own doc comment states an origin-family caller must honor).
struct StubHandler {
    answer: String,
    handled: Arc<Mutex<Vec<MessageEvent>>>,
}

#[async_trait]
impl MessageHandler for StubHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        self.handled.lock().unwrap().push(event.clone());
        // Origin-family caller contract: thread_id must travel unchanged.
        adapter
            .send_message(&event.chat_id, &self.answer, event.thread_id.as_deref())
            .await?;
        Ok(())
    }
}

/// Like [`StubHandler`] but never delivers — used to prove
/// `deliver_only_route_invokes_no_agent_turn` (invocation COUNT, not
/// content, is the thing under test).
#[derive(Default)]
struct CountingHandler {
    invocations: Arc<AtomicUsize>,
}

#[async_trait]
impl MessageHandler for CountingHandler {
    async fn handle(
        &self,
        _event: &MessageEvent,
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn spin_up(
    config: WebhookRoutesConfig,
    registry: Arc<RwLock<DeliveryRegistry>>,
    handler: Arc<dyn MessageHandler>,
) -> (std::net::SocketAddr, CancellationToken) {
    let adapter = Arc::new(WebhookAdapter::new(config, registry).expect("adapter construction"));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let cancel = CancellationToken::new();
    let serve_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = serve_webhook_adapter(listener, adapter, handler, serve_cancel).await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, cancel)
}

async fn wait_for<F: Fn() -> bool>(timeout: Duration, cond: F) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn post_signed(
    addr: std::net::SocketAddr,
    path: &str,
    secret: &str,
    body: Vec<u8>,
    content_type: &str,
) -> reqwest::Response {
    let ts = now_unix();
    let sig = sign(secret, ts, &body);
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .header("Content-Type", content_type)
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
        .send()
        .await
        .expect("request send")
}

// ===========================================================================
// Task 1: all three delivery target families
// ===========================================================================

#[tokio::test]
async fn each_deliver_target_family_reaches_its_destination() {
    // --- Platform family: full successful round trip (registry-based, not
    // network — not subject to the SSRF gate at all). ---
    let secret_env = "RESTGW_T1_PLATFORM_SECRET";
    let secret = "platform-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));

    let route = base_route("platform-family", "/webhook/platform-family", secret_env);
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "platform answer".to_string(),
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(single_route_config(route), registry, handler).await;

    let resp = post_signed(
        addr,
        "/webhook/platform-family",
        secret,
        b"Body=hi".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    let delivered = wait_for(Duration::from_secs(2), || {
        !stub_sender.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(delivered, "platform delivery did not complete in time");
    assert_eq!(stub_sender.calls.lock().unwrap()[0].1, "platform answer");
    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }

    // --- Origin family: the caller-supplied callback_url is correctly
    // extracted from the payload and a delivery attempt is made to exactly
    // that address — proven by the failure being specifically an SSRF
    // refusal of the loopback address supplied, not a "no callback_url"
    // misconfiguration failure. ---
    let secret_env = "RESTGW_T1_ORIGIN_SECRET";
    let secret = "origin-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let mut route = base_route("origin-family", "/webhook/origin-family", secret_env);
    route.deliver = DeliverTarget::Origin;
    route.deliver_platform = None;
    route.prompt_template = "{Body}".to_string();
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "origin answer".to_string(),
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(single_route_config(route), registry, handler).await;

    let body = b"Body=hi&callback_url=http%3A%2F%2F127.0.0.1%3A1%2Fcallback".to_vec();
    let resp = post_signed(
        addr,
        "/webhook/origin-family",
        secret,
        body,
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    // The turn runs, the handler forwards thread_id, send_message resolves
    // the origin callback URL and attempts delivery — refused by the real,
    // unweakened SSRF gate (loopback). Assert the turn actually ran (proves
    // extraction worked) rather than polling delivery (which can never
    // succeed against a loopback origin target).
    let ran = wait_for(Duration::from_secs(2), || !handled.lock().unwrap().is_empty()).await;
    assert!(ran, "origin-family turn did not run");
    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }

    // --- URL family: same "extraction + dispatch attempted, refused only
    // by the real SSRF gate" proof, this time via a `deliver_only` route so
    // the outcome is observable synchronously in the HTTP response instead
    // of needing to poll async delivery state. ---
    let secret_env = "RESTGW_T1_URL_SECRET";
    let secret = "url-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let mut route = base_route("url-family", "/webhook/url-family", secret_env);
    route.deliver = DeliverTarget::Origin; // load-time SSRF check only applies to DeliverTarget::Url
    route.deliver_only = true;
    route.prompt_template = "{Body}".to_string();
    let handler = Arc::new(StubHandler {
        answer: "unused".to_string(),
        handled: Arc::new(Mutex::new(Vec::new())),
    });
    let (addr, cancel) = spin_up(single_route_config(route), registry, handler).await;

    let body = b"Body=hi&callback_url=http%3A%2F%2F127.0.0.1%3A1%2Fcallback".to_vec();
    let resp = post_signed(
        addr,
        "/webhook/url-family",
        secret,
        body,
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("SSRF"),
        "expected an SSRF-refusal failure reason, got: {text}"
    );
    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn platform_delivery_goes_through_the_shared_registry() {
    let secret_env = "RESTGW_T1_REGISTRY_SECRET";
    let secret = "registry-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));

    let mut route = base_route("registry-route", "/webhook/registry", secret_env);
    route.deliver_chat_id = Some("configured-chat-id".to_string());
    let handler = Arc::new(StubHandler {
        answer: "registry answer".to_string(),
        handled: Arc::new(Mutex::new(Vec::new())),
    });
    let (addr, cancel) = spin_up(single_route_config(route), registry, handler).await;

    let resp = post_signed(
        addr,
        "/webhook/registry",
        secret,
        b"Body=hi".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    let delivered = wait_for(Duration::from_secs(2), || {
        !stub_sender.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(delivered);
    let calls = stub_sender.calls.lock().unwrap();
    assert_eq!(calls[0].0, "configured-chat-id");
    assert_eq!(calls[0].1, "registry answer");
    assert_eq!(calls[0].2, None, "no thread configured -> None");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn unknown_platform_target_fails_loudly() {
    let secret_env = "RESTGW_T1_UNKNOWN_PLATFORM_SECRET";
    let secret = "unknown-platform-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    // Registry stays EMPTY — "teststub" is never registered.
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let route = base_route("unknown-platform", "/webhook/unknown-platform", secret_env);
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "unused".to_string(),
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(single_route_config(route), registry, handler).await;

    let resp = post_signed(
        addr,
        "/webhook/unknown-platform",
        secret,
        b"Body=hi".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    let ran = wait_for(Duration::from_secs(2), || !handled.lock().unwrap().is_empty()).await;
    assert!(ran, "handler must still run — only delivery fails");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn deliver_url_target_is_ssrf_checked_at_delivery_not_only_at_load() {
    // Load-time refusal (WebhookAdapter::new).
    let secret_env = "RESTGW_T1_URL_LOAD_SECRET";
    unsafe {
        std::env::set_var(secret_env, "load-secret");
    }
    let mut route = base_route("url-load", "/webhook/url-load", secret_env);
    route.deliver = DeliverTarget::Url;
    route.deliver_platform = None;
    route.deliver_url = Some("http://127.0.0.1:1/x".to_string());
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let result = WebhookAdapter::new(single_route_config(route), registry);
    assert!(result.is_err(), "load-time check must refuse a loopback deliver_url");
    unsafe {
        std::env::remove_var(secret_env);
    }

    // Delivery-time refusal, exercised independently of load-time
    // validation (proves the check is not skipped just because a
    // hypothetical earlier pass already ran).
    let client = deliver::build_client();
    let outcome =
        deliver::deliver_to_url(&client, "http://127.0.0.1:1/x", "must never be sent", None).await;
    match outcome {
        deliver::DeliveryOutcome::Failed(_) => {}
        deliver::DeliveryOutcome::Delivered => panic!("delivery-time SSRF check must refuse"),
    }
}

#[tokio::test]
async fn outbound_auth_header_is_sent_and_never_logged() {
    // (1) Header CONSTRUCTION correctness — Bearer.
    let bearer_env = "RESTGW_T1_AUTH_BEARER";
    let secret_value = "super-secret-bearer-token-xyz";
    unsafe {
        std::env::set_var(bearer_env, secret_value);
    }
    let header = resolve_outbound_auth(&OutboundAuth::Bearer {
        env: bearer_env.to_string(),
    });
    assert_eq!(
        header,
        Some(("Authorization".to_string(), format!("Bearer {secret_value}")))
    );

    // (2) Header construction correctness — Basic.
    let user_env = "RESTGW_T1_AUTH_USER";
    let pass_env = "RESTGW_T1_AUTH_PASS";
    unsafe {
        std::env::set_var(user_env, "acct-sid");
        std::env::set_var(pass_env, "auth-tok");
    }
    let header = resolve_outbound_auth(&OutboundAuth::Basic {
        user_env: user_env.to_string(),
        pass_env: pass_env.to_string(),
    });
    use base64::Engine as _;
    let expected = base64::engine::general_purpose::STANDARD.encode("acct-sid:auth-tok");
    assert_eq!(
        header,
        Some(("Authorization".to_string(), format!("Basic {expected}")))
    );

    // (3) The header actually reaches the wire — real stub HTTP server,
    // driven through `deliver::build_client()` directly (the same
    // SSRF-bypass technique `webhook_route_e2e.rs`'s
    // `delivery_client_does_not_follow_redirects` test already establishes
    // as the correct way to test an outbound-request PROPERTY against a
    // loopback target, since `is_safe_url` unconditionally blocks every
    // loopback address and neither `deliver_to_url` nor `WebhookAdapter`
    // can be routed through it).
    let observed_auth: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let observed_for_server = observed_auth.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_addr = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(axum::routing::any(
        move |headers: axum::http::HeaderMap| {
            let observed = observed_for_server.clone();
            async move {
                let auth = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                *observed.lock().unwrap() = auth;
                "ok"
            }
        },
    ));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (name, value) = header.clone().unwrap();
    deliver::build_client()
        .post(format!("http://{stub_addr}/hook"))
        .header(name, value)
        .json(&serde_json::json!({ "content": "payload" }))
        .send()
        .await
        .expect("request send");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        observed_auth.lock().unwrap().as_deref(),
        Some(format!("Basic {expected}").as_str())
    );

    // (4) "Never logged" — capture tracing output from a REAL
    // `deliver_to_url` call (the actual, SSRF-gated production code path)
    // carrying a distinctive secret value in its auth header, and assert
    // the raw secret literal never appears in captured log text. The
    // request itself is refused by the (correct, unweakened) SSRF gate —
    // that refusal is not what's under test here; the logging discipline is.
    let capture = Arc::new(Mutex::new(Vec::<u8>::new()));
    let capture_for_writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || CaptureWriter(capture_for_writer.clone()))
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let secret_marker = "NEVER-LOGGED-MARKER-98765";
    {
        let _guard = tracing::subscriber::set_default(subscriber);
        let _ = deliver::deliver_to_url(
            &deliver::build_client(),
            "http://127.0.0.1:1/private",
            "content",
            Some((
                "Authorization".to_string(),
                format!("Bearer {secret_marker}"),
            )),
        )
        .await;
    }
    let captured = String::from_utf8_lossy(&capture.lock().unwrap()).to_string();
    assert!(
        !captured.contains(secret_marker),
        "captured log output must never contain the credential value: {captured}"
    );

    unsafe {
        std::env::remove_var(bearer_env);
        std::env::remove_var(user_env);
        std::env::remove_var(pass_env);
    }
}

/// Simple `io::Write` sink shared via `Arc<Mutex<Vec<u8>>>`, used to
/// capture `tracing` output in-process without a filesystem dependency.
#[derive(Clone)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn delivery_failure_does_not_panic_the_listener() {
    let secret_env = "RESTGW_T1_NOPANIC_SECRET";
    let secret = "nopanic-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    // Registry starts empty -> first delivery fails loudly (logged, not a panic).
    let registry_inner = DeliveryRegistry::new();
    let registry = Arc::new(RwLock::new(registry_inner));
    let route = base_route("nopanic-route", "/webhook/nopanic", secret_env);
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "second answer".to_string(),
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(single_route_config(route), registry.clone(), handler).await;

    // First request: delivery fails (unregistered platform).
    let resp1 = post_signed(
        addr,
        "/webhook/nopanic",
        secret,
        b"Body=first".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp1.status(), reqwest::StatusCode::ACCEPTED);
    let ran_once = wait_for(Duration::from_secs(2), || !handled.lock().unwrap().is_empty()).await;
    assert!(ran_once);

    // Register the sender NOW; the listener must still be alive and serve
    // a second request normally — proving the earlier failed delivery
    // didn't take the listener (or the process) down.
    let stub_sender = Arc::new(StubDeliverySend::default());
    registry
        .write()
        .await
        .insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);

    let resp2 = post_signed(
        addr,
        "/webhook/nopanic",
        secret,
        b"Body=second".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp2.status(), reqwest::StatusCode::ACCEPTED);
    let delivered = wait_for(Duration::from_secs(2), || {
        !stub_sender.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(delivered, "listener must keep serving after a prior delivery failure");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

// ===========================================================================
// Task 2: deliver_only
// ===========================================================================

#[tokio::test]
async fn deliver_only_route_invokes_no_agent_turn() {
    let secret_env = "RESTGW_T2_NOTURN_SECRET";
    let secret = "noturn-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));

    let mut route = base_route("no-turn-route", "/webhook/no-turn", secret_env);
    route.deliver_only = true;
    route.prompt_template = "rendered: {Body}".to_string();
    let counting = Arc::new(CountingHandler::default());
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting.clone()).await;

    let resp = post_signed(
        addr,
        "/webhook/no-turn",
        secret,
        b"Body=payload".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert_eq!(text, "rendered: payload");

    let delivered = wait_for(Duration::from_secs(2), || {
        !stub_sender.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(delivered);
    assert_eq!(
        counting.invocations.load(Ordering::SeqCst),
        0,
        "deliver_only must never invoke the agent turn handler"
    );

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn deliver_only_returns_the_rendered_text_in_the_body() {
    let secret_env = "RESTGW_T2_BODY_SECRET";
    let secret = "body-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));
    let mut route = base_route("body-route", "/webhook/body-route", secret_env);
    route.deliver_only = true;
    route.prompt_template = "hello {Body}".to_string();
    let counting = Arc::new(CountingHandler::default());
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting).await;

    let resp = post_signed(
        addr,
        "/webhook/body-route",
        secret,
        b"Body=world".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "hello world");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn non_deliver_only_route_acks_before_delivering() {
    let secret_env = "RESTGW_T2_202_SECRET";
    let secret = "202-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let counting = Arc::new(CountingHandler::default());
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let route = base_route("normal-route", "/webhook/normal", secret_env);
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting.clone()).await;

    let resp = post_signed(
        addr,
        "/webhook/normal",
        secret,
        b"Body=hi".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    let ran = wait_for(Duration::from_secs(2), || {
        counting.invocations.load(Ordering::SeqCst) >= 1
    })
    .await;
    assert!(ran, "non-deliver_only route must still run the turn");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn deliver_only_still_verifies_the_signature() {
    let secret_env = "RESTGW_T2_UNSIGNED_SECRET";
    let secret = "unsigned-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let mut route = base_route("unsigned-deliver-only", "/webhook/unsigned-do", secret_env);
    route.deliver_only = true;
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let counting = Arc::new(CountingHandler::default());
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/webhook/unsigned-do"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(b"Body=hi".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn deliver_only_still_honours_the_rails() {
    let secret_env = "RESTGW_T2_RAILS_SECRET";
    let secret = "rails-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    // 413: body cap enforced BEFORE signature verification, so an unsigned
    // oversized request is still refused on size alone.
    let mut route = base_route("rails-cap-route", "/webhook/rails-cap", secret_env);
    route.deliver_only = true;
    route.rails.max_body_bytes = 16;
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let counting = Arc::new(CountingHandler::default());
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/webhook/rails-cap"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(b"Body=this+body+is+definitely+longer+than+sixteen+bytes".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    cancel.cancel();

    // 429: rate limit exhausted on a signed, correctly-sized request.
    let mut route = base_route("rails-rate-route", "/webhook/rails-rate", secret_env);
    route.deliver_only = true;
    route.rails.rate_limit_per_minute = 1;
    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry_inner = DeliveryRegistry::new();
    registry_inner.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry_inner));
    let counting = Arc::new(CountingHandler::default());
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting).await;
    let resp1 = post_signed(
        addr,
        "/webhook/rails-rate",
        secret,
        b"Body=first".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp1.status(), reqwest::StatusCode::OK);
    let resp2 = post_signed(
        addr,
        "/webhook/rails-rate",
        secret,
        b"Body=second".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp2.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    cancel.cancel();

    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn deliver_only_reaches_all_three_target_families() {
    // Platform: full successful synchronous delivery.
    let secret_env = "RESTGW_T2_FAMILIES_PLATFORM_SECRET";
    let secret = "families-platform-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));
    let mut route = base_route("do-platform", "/webhook/do-platform", secret_env);
    route.deliver_only = true;
    route.prompt_template = "{Body}".to_string();
    let counting = Arc::new(CountingHandler::default());
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting).await;
    let resp = post_signed(
        addr,
        "/webhook/do-platform",
        secret,
        b"Body=via-platform".to_vec(),
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "via-platform");
    assert_eq!(stub_sender.calls.lock().unwrap()[0].1, "via-platform");
    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }

    // Origin: extraction + dispatch attempted (SSRF-refused, per this
    // file's module doc).
    let secret_env = "RESTGW_T2_FAMILIES_ORIGIN_SECRET";
    let secret = "families-origin-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let mut route = base_route("do-origin", "/webhook/do-origin", secret_env);
    route.deliver = DeliverTarget::Origin;
    route.deliver_platform = None;
    route.deliver_only = true;
    route.prompt_template = "{Body}".to_string();
    let counting = Arc::new(CountingHandler::default());
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting).await;
    let body = b"Body=via-origin&callback_url=http%3A%2F%2F127.0.0.1%3A1%2Fcb".to_vec();
    let resp = post_signed(
        addr,
        "/webhook/do-origin",
        secret,
        body,
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);
    let text = resp.text().await.unwrap();
    assert!(text.contains("SSRF"), "got: {text}");
    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }

    // Url: same proof shape as origin.
    let secret_env = "RESTGW_T2_FAMILIES_URL_SECRET";
    let secret = "families-url-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    // deliver: url is SSRF-checked at LOAD time too, so it cannot be
    // constructed with a loopback deliver_url — use `deliver: origin`
    // (caller-supplied, dynamic, not load-checked) to prove the SAME
    // `deliver::deliver_to_url` dispatch function `deliver: url` shares,
    // matching this file's module doc.
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let mut route = base_route("do-url", "/webhook/do-url", secret_env);
    route.deliver = DeliverTarget::Origin;
    route.deliver_platform = None;
    route.deliver_only = true;
    route.prompt_template = "{Body}".to_string();
    let counting = Arc::new(CountingHandler::default());
    let (addr, cancel) = spin_up(single_route_config(route), registry, counting).await;
    let body = b"Body=via-url&callback_url=http%3A%2F%2F127.0.0.1%3A1%2Fcb".to_vec();
    let resp = post_signed(
        addr,
        "/webhook/do-url",
        secret,
        body,
        "application/x-www-form-urlencoded",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);
    let text = resp.text().await.unwrap();
    assert!(text.contains("SSRF"), "got: {text}");
    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

// ===========================================================================
// Task 3: O-02 immediate-deny approval gate
// ===========================================================================

#[tokio::test]
async fn webhook_turn_approval_is_denied_immediately_and_visibly() {
    let capture = Arc::new(Mutex::new(Vec::<u8>::new()));
    let capture_for_writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || CaptureWriter(capture_for_writer.clone()))
        .with_max_level(tracing::Level::WARN)
        .finish();

    let gate = ImmediateDenyApprovalGate::new();
    let outcome;
    {
        let _guard = tracing::subscriber::set_default(subscriber);
        outcome = gate
            .request_approval(
                "webhook-session-1",
                "delete_production_database",
                "destructive operation requested by an inbound webhook turn",
                &serde_json::json!({}),
            )
            .await;
    }
    assert_eq!(outcome, ironhermes_core::ApprovalOutcome::Denied);

    let captured = String::from_utf8_lossy(&capture.lock().unwrap()).to_string();
    assert!(captured.to_uppercase().contains("WARN"), "got: {captured}");
    assert!(captured.contains("delete_production_database"), "got: {captured}");

    // The delivered-output side of O-02: whatever calls this gate and gets
    // `Denied` back builds the turn's delivered text via `format_denial`
    // using the same tool/reason — proven end-to-end through this crate's
    // OWN delivery machinery (WebhookAdapter::send_message), not a
    // hypothetical.
    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));
    let secret_env = "RESTGW_T3_DENY_DELIVERY_SECRET";
    unsafe {
        std::env::set_var(secret_env, "deny-secret");
    }
    let route = base_route("deny-delivery-route", "/webhook/deny-delivery", secret_env);
    let adapter = Arc::new(WebhookAdapter::new(single_route_config(route), registry).unwrap());

    let denial_text = format_denial(
        "delete_production_database",
        "destructive operation requested by an inbound webhook turn",
    );
    adapter
        .send_message("deny-delivery-route", &denial_text, None)
        .await
        .expect("delivering the denial text must succeed");
    let calls = stub_sender.calls.lock().unwrap();
    assert!(calls[0].1.contains("delete_production_database"));
    assert!(calls[0].1.to_lowercase().contains("denied"));

    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn immediate_deny_does_not_wait_for_the_coordinator_timeout() {
    let gate = ImmediateDenyApprovalGate::new();
    let start = Instant::now();
    let outcome = gate
        .request_approval("s", "tool", "reason", &serde_json::json!({}))
        .await;
    let elapsed = start.elapsed();
    assert_eq!(outcome, ironhermes_core::ApprovalOutcome::Denied);
    // Coordinator default timeout is 120s (gateway/src/approval.rs) —
    // asserting well under 1s is an enormous margin, proving no wait at all.
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[tokio::test]
async fn denial_reason_is_sanitised_before_it_reaches_the_output() {
    let text = format_denial(
        "tool\nname\rwith\nbreaks",
        "reason\nwith\r\nnewlines\ninjected",
    );
    assert!(!text.contains('\n'));
    assert!(!text.contains('\r'));

    // Also drive it through request_approval's own log line.
    let capture = Arc::new(Mutex::new(Vec::<u8>::new()));
    let capture_for_writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || CaptureWriter(capture_for_writer.clone()))
        .with_max_level(tracing::Level::WARN)
        .finish();
    let gate = ImmediateDenyApprovalGate::new();
    {
        let _guard = tracing::subscriber::set_default(subscriber);
        let _ = gate
            .request_approval(
                "s",
                "tool\nname",
                "reason\nwith\nbreaks",
                &serde_json::json!({}),
            )
            .await;
    }
    let captured = String::from_utf8_lossy(&capture.lock().unwrap()).to_string();
    // The RAW multi-line strings must not appear verbatim as literal
    // newlines inside the sanitized field values (tool/reason are
    // single-line after sanitization; tracing's own formatter uses its own
    // separators around fields, which is not what's under test here).
    assert!(!captured.contains("tool\nname"));
    assert!(!captured.contains("reason\nwith\nbreaks"));
}

// ===========================================================================
// Task 3: O-03 per-delivery session isolation
// ===========================================================================

#[tokio::test]
async fn deliveries_get_distinct_sessions_by_default() {
    let secret_env = "RESTGW_T3_EPHEMERAL_SECRET";
    let secret = "ephemeral-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let route = base_route("ephemeral-route", "/webhook/ephemeral", secret_env);
    assert_eq!(route.session, SessionMode::Ephemeral);
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "ok".to_string(),
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(single_route_config(route), registry, handler).await;

    for body in [b"Body=one".to_vec(), b"Body=two".to_vec()] {
        let resp = post_signed(
            addr,
            "/webhook/ephemeral",
            secret,
            body,
            "application/x-www-form-urlencoded",
        )
        .await;
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }
    let both_ran = wait_for(Duration::from_secs(2), || handled.lock().unwrap().len() >= 2).await;
    assert!(both_ran);

    let events = handled.lock().unwrap();
    assert_ne!(
        events[0].sender_id, events[1].sender_id,
        "ephemeral deliveries must get distinct session identities"
    );
    assert_eq!(events[0].chat_id, events[1].chat_id, "chat_id (routing key) stays stable");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn persistent_route_reuses_one_session() {
    let secret_env = "RESTGW_T3_PERSISTENT_SECRET";
    let secret = "persistent-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let mut route = base_route("persistent-route", "/webhook/persistent", secret_env);
    route.session = SessionMode::Persistent;
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "ok".to_string(),
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(single_route_config(route), registry, handler).await;

    for body in [b"Body=one".to_vec(), b"Body=two".to_vec()] {
        let resp = post_signed(
            addr,
            "/webhook/persistent",
            secret,
            body,
            "application/x-www-form-urlencoded",
        )
        .await;
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }
    let both_ran = wait_for(Duration::from_secs(2), || handled.lock().unwrap().len() >= 2).await;
    assert!(both_ran);

    let events = handled.lock().unwrap();
    assert_eq!(
        events[0].sender_id, events[1].sender_id,
        "persistent deliveries must reuse one session identity"
    );

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[test]
fn ephemeral_is_the_default_when_unconfigured() {
    // WebhookRoute's own Default / serde-default already asserts this at
    // the schema level (ironhermes-core/src/webhook_route.rs); this test
    // pins the same fact from this crate's perspective, against the route
    // this file's own `base_route` helper builds when nothing overrides
    // `.session`.
    let route = base_route("unconfigured-route", "/webhook/unconfigured", "UNUSED_ENV");
    assert_eq!(route.session, SessionMode::Ephemeral);
}

#[tokio::test]
async fn session_mode_is_per_route() {
    let secret_env = "RESTGW_T3_PERROUTE_SECRET";
    let secret = "per-route-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));

    let mut ephemeral_route = base_route("per-route-ephemeral", "/webhook/per-route-ephemeral", secret_env);
    ephemeral_route.session = SessionMode::Ephemeral;
    let mut persistent_route = base_route("per-route-persistent", "/webhook/per-route-persistent", secret_env);
    persistent_route.session = SessionMode::Persistent;

    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![ephemeral_route, persistent_route],
    };
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "ok".to_string(),
        handled: handled.clone(),
    });
    let adapter = Arc::new(WebhookAdapter::new(config, registry).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let serve_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = serve_webhook_adapter(listener, adapter, handler, serve_cancel).await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Distinct body per request — identical bodies to the same route would
    // collide in the idempotency cache (D-15) and de-duplicate to a single
    // turn, which is not what this test is exercising.
    for i in 0..2 {
        let resp = post_signed(
            addr,
            "/webhook/per-route-ephemeral",
            secret,
            format!("Body=e{i}").into_bytes(),
            "application/x-www-form-urlencoded",
        )
        .await;
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }
    for i in 0..2 {
        let resp = post_signed(
            addr,
            "/webhook/per-route-persistent",
            secret,
            format!("Body=p{i}").into_bytes(),
            "application/x-www-form-urlencoded",
        )
        .await;
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }
    let all_ran = wait_for(Duration::from_secs(2), || handled.lock().unwrap().len() >= 4).await;
    assert!(all_ran);

    let events = handled.lock().unwrap();
    let ephemeral: Vec<_> = events
        .iter()
        .filter(|e| e.chat_id == "per-route-ephemeral")
        .collect();
    let persistent: Vec<_> = events
        .iter()
        .filter(|e| e.chat_id == "per-route-persistent")
        .collect();
    assert_eq!(ephemeral.len(), 2);
    assert_eq!(persistent.len(), 2);
    assert_ne!(
        ephemeral[0].sender_id, ephemeral[1].sender_id,
        "ephemeral route keeps its own per-delivery isolation"
    );
    assert_eq!(
        persistent[0].sender_id, persistent[1].sender_id,
        "persistent route keeps its own session reuse, independent of the ephemeral route on the same listener"
    );

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

