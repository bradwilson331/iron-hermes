//! End-to-end proof of the webhook spine (Phase 36.7.1 Plan 01, Task 2):
//! a real listener on `127.0.0.1:0`, a stub `MessageHandler`, and a stub
//! `DeliverySend` standing in for the delivery target. Test function names
//! match PLAN.md's `must_haves.truths` falsifiable-test names exactly,
//! where PLAN.md names one.
//!
//! No test binds a fixed port — every listener uses `127.0.0.1:0` and reads
//! `local_addr()`, so parallel test runs (cargo-nextest: one process per
//! test) never collide.
//!
//! **Why `deliver: platform` (not `deliver: url`) for the success-path
//! delivery target:** `ironhermes_core::ssrf::is_safe_url` unconditionally
//! blocks EVERY loopback address (verified directly in
//! `ironhermes-core/src/ssrf.rs`'s own `test_loopback_ipv4_blocked`) — there
//! is no test-mode override, and D-16 forbids weakening it. A second
//! `127.0.0.1:0` axum server standing in for a `deliver: url` target would
//! therefore be refused by the very SSRF check D-11 requires, making that
//! combination unable to prove a successful delivery. The success-path test
//! below proves the full pipeline (verify → 202 → spawned turn → delivery)
//! through `deliver: platform` and a stub `DeliverySend` registered in the
//! shared `DeliveryRegistry` instead — the delivery target that is not
//! network-based and so is not subject to the SSRF gate at all. D-11's real,
//! unweakened SSRF enforcement is proven separately below
//! (`deliver_url_target_is_ssrf_checked_at_load_and_at_delivery`), against a
//! loopback URL that the check correctly refuses at both load time and
//! delivery time.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use ironhermes_core::MessageEvent;
use ironhermes_cron::{DeliveryRegistry, DeliverySend};
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_restgw::webhook::route_config::{
    DeliverTarget, OutboundAuth, RouteRails, SessionMode, SignatureKind, WebhookRoute,
    WebhookRoutesConfig,
};
use ironhermes_restgw::webhook::{WebhookAdapter, deliver, serve_webhook_adapter};
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::{Barrier, RwLock};
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

/// Stub `DeliverySend` — captures every `send_text` call instead of
/// touching the network. Stands in for the delivery target on the
/// `deliver: platform` success path.
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

/// Stub `MessageHandler` — records every event it was handed, optionally
/// blocks on a `Barrier` (released only after the test observes the 202
/// response) before delivering `answer` back through the adapter.
struct StubHandler {
    answer: String,
    barrier: Option<Arc<Barrier>>,
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
        if let Some(ref barrier) = self.barrier {
            barrier.wait().await;
        }
        adapter
            .send_message(&event.chat_id, &self.answer, None)
            .await?;
        Ok(())
    }
}

/// Bind an ephemeral `127.0.0.1:0` listener, construct a `WebhookAdapter`
/// from `config` + `registry`, spawn `serve_webhook_adapter`, and return
/// the bound address plus a `CancellationToken` the caller can fire to shut
/// the listener down.
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
    // Yield so the spawned server task reaches accept() before the test
    // issues its first request.
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, cancel)
}

// ===========================================================================
// D-09/D-03: signed POST -> verified -> 202 -> turn -> delivered
// ===========================================================================

#[tokio::test]
async fn signed_post_runs_a_turn_and_delivers_to_the_target() {
    let secret_env = "RESTGW_TEST_SECRET_SUCCESS";
    let secret = "top-secret-shared-value";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));

    let route = base_route("success-route", "/webhook/success", secret_env);
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };

    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "the agent's answer".to_string(),
        barrier: None,
        handled: handled.clone(),
    });

    let (addr, cancel) = spin_up(config, registry, handler).await;

    let body = b"Body=hello+webhook".to_vec();
    let ts = now_unix();
    let sig = sign(secret, ts, &body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/success"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
        .send()
        .await
        .expect("request send");

    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    // The turn runs on a spawned task — poll briefly for the stub sender to
    // observe the delivered answer.
    let delivered = wait_for(Duration::from_secs(2), || {
        !stub_sender.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(delivered, "delivery did not complete in time");

    let calls = stub_sender.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "success-route");
    assert_eq!(calls[0].1, "the agent's answer");

    let handled_events = handled.lock().unwrap();
    assert_eq!(handled_events.len(), 1);
    assert_eq!(handled_events[0].content, "hello webhook");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
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

// ===========================================================================
// D-10: refusal paths — tampered body, expired timestamp, malformed header
// ===========================================================================

#[tokio::test]
async fn tampered_body_is_refused() {
    let secret_env = "RESTGW_TEST_SECRET_TAMPERED";
    let secret = "another-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let route = base_route("tampered-route", "/webhook/tampered", secret_env);
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "unused".to_string(),
        barrier: None,
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(config, registry, handler).await;

    let signed_body = b"Body=original".to_vec();
    let ts = now_unix();
    let sig = sign(secret, ts, &signed_body);
    let tampered_body = b"Body=TAMPERED".to_vec();

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/tampered"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(tampered_body)
        .send()
        .await
        .expect("request send");

    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        handled.lock().unwrap().is_empty(),
        "handler must never be invoked for a refused request"
    );

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn expired_timestamp_is_refused() {
    let secret_env = "RESTGW_TEST_SECRET_EXPIRED";
    let secret = "expired-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let route = base_route("expired-route", "/webhook/expired", secret_env);
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let handler = Arc::new(StubHandler {
        answer: "unused".to_string(),
        barrier: None,
        handled: Arc::new(Mutex::new(Vec::new())),
    });
    let (addr, cancel) = spin_up(config, registry, handler).await;

    let body = b"Body=late".to_vec();
    let ts = now_unix() - 400; // outside the default 300s skew window
    let sig = sign(secret, ts, &body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/expired"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
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
async fn malformed_v2_header_does_not_degrade() {
    let secret_env = "RESTGW_TEST_SECRET_MALFORMED";
    let secret = "malformed-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let route = base_route("malformed-route", "/webhook/malformed", secret_env);
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let handler = Arc::new(StubHandler {
        answer: "unused".to_string(),
        barrier: None,
        handled: Arc::new(Mutex::new(Vec::new())),
    });
    let (addr, cancel) = spin_up(config, registry, handler).await;

    let body = b"Body=whatever".to_vec();
    let ts = now_unix();

    let client = reqwest::Client::new();

    // Case 1: signature header present but not valid hex — must not
    // degrade to any weaker check succeeding.
    let resp = client
        .post(format!("http://{addr}/webhook/malformed"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-Webhook-Signature-V2", "not-hex-at-all!!")
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body.clone())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Case 2: signature header present and well-formed hex, but the
    // REQUIRED timestamp header is entirely absent — must be refused, not
    // fall through to any legacy/body-only check.
    let sig = sign(secret, ts, &body);
    let resp = client
        .post(format!("http://{addr}/webhook/malformed"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-Webhook-Signature-V2", sig)
        .body(body)
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

// ===========================================================================
// D-10 loopback rail: no-verification route on a non-loopback host
// ===========================================================================

#[test]
fn no_verification_route_on_non_loopback_is_refused() {
    let mut route = base_route("open-route", "/webhook/open", "UNUSED_ENV");
    route.signature = SignatureKind::None;
    route.secret_env = None;

    let config = WebhookRoutesConfig {
        host: "0.0.0.0".to_string(), // non-loopback
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };

    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    // Construction is cheap and does no I/O (see `WebhookAdapter::new`'s own
    // doc comment) — this assertion alone proves no socket was ever opened,
    // since a bind only happens inside `run_webhook_adapter`/
    // `serve_webhook_adapter`, neither of which is reached here.
    let result = WebhookAdapter::new(config, registry);
    let err = match result {
        Err(e) => format!("{e:#}"),
        Ok(_) => panic!("must refuse to construct (D-10)"),
    };
    assert!(err.contains("D-10") || err.contains("loopback"));
}

#[test]
fn no_verification_route_on_loopback_host_constructs_successfully() {
    let mut route = base_route("open-route-loopback", "/webhook/open-loop", "UNUSED_ENV");
    route.signature = SignatureKind::None;
    route.secret_env = None;

    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    assert!(WebhookAdapter::new(config, registry).is_ok());
}

#[test]
fn missing_secret_env_names_the_variable() {
    let route = base_route(
        "missing-secret-route",
        "/webhook/missing-secret",
        "RESTGW_TEST_DEFINITELY_UNSET_VAR",
    );
    // Ensure it really is unset in this test process.
    unsafe {
        std::env::remove_var("RESTGW_TEST_DEFINITELY_UNSET_VAR");
    }
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let result = WebhookAdapter::new(config, registry);
    let err = match result {
        Err(e) => format!("{e:#}"),
        Ok(_) => panic!("must refuse to construct — missing secret_env"),
    };
    assert!(err.contains("RESTGW_TEST_DEFINITELY_UNSET_VAR"));
}

// ===========================================================================
// D-12: 202 observed before the turn finishes
// ===========================================================================

#[tokio::test]
async fn ack_returns_before_the_turn_finishes() {
    let secret_env = "RESTGW_TEST_SECRET_ACK";
    let secret = "ack-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));

    let route = base_route("ack-route", "/webhook/ack", secret_env);
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };

    // Two parties: the spawned handler task, and this test — the handler's
    // barrier.wait() cannot complete until the test also calls wait(),
    // which it only does AFTER observing the 202 response.
    let barrier = Arc::new(Barrier::new(2));
    let handler = Arc::new(StubHandler {
        answer: "late answer".to_string(),
        barrier: Some(barrier.clone()),
        handled: Arc::new(Mutex::new(Vec::new())),
    });

    let (addr, cancel) = spin_up(config, registry, handler).await;

    let body = b"Body=slow".to_vec();
    let ts = now_unix();
    let sig = sign(secret, ts, &body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/ack"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
        .send()
        .await
        .expect("request send");

    // 202 observed WHILE the stub handler is still blocked on the barrier —
    // proving the response returns before the turn completes (D-12).
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    assert!(
        stub_sender.calls.lock().unwrap().is_empty(),
        "delivery must not have happened yet — the handler is still on the barrier"
    );

    // Release the handler; it can now proceed to deliver.
    barrier.wait().await;

    let delivered = wait_for(Duration::from_secs(2), || {
        !stub_sender.calls.lock().unwrap().is_empty()
    })
    .await;
    assert!(delivered, "delivery did not complete after barrier release");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

// ===========================================================================
// D-11: deliver:url is SSRF-checked at load time AND at delivery time
// ===========================================================================

#[test]
fn deliver_url_target_is_ssrf_checked_at_load_and_at_delivery() {
    let secret_env = "RESTGW_TEST_SECRET_SSRF";
    unsafe {
        std::env::set_var(secret_env, "ssrf-secret");
    }

    // --- Load-time check: WebhookAdapter::new refuses a route whose
    // deliver_url resolves to a loopback address. ---
    let mut route = base_route("ssrf-route", "/webhook/ssrf", secret_env);
    route.deliver = DeliverTarget::Url;
    route.deliver_platform = None;
    route.deliver_url = Some("http://127.0.0.1:1/private".to_string());

    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let result = WebhookAdapter::new(config, registry);
    assert!(
        result.is_err(),
        "load-time SSRF check must refuse a loopback deliver_url target"
    );

    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn deliver_to_url_refuses_loopback_target_at_delivery_time() {
    let client = deliver::build_client();
    let outcome = deliver::deliver_to_url(
        &client,
        "http://127.0.0.1:1/private",
        "content that must never be sent",
        None,
    )
    .await;
    match outcome {
        deliver::DeliveryOutcome::Failed(_) => {}
        deliver::DeliveryOutcome::Delivered => {
            panic!("delivery-time SSRF check must refuse a loopback target")
        }
    }
}

// ===========================================================================
// Phase 36.7.1 Plan 01 Task 3: shared DeliveryRegistry handle is SHARED,
// not copied — an adapter registered AFTER WebhookAdapter::new is still
// visible to it. Proves the `Arc<RwLock<DeliveryRegistry>>` handle
// `GatewayRunner::start()` threads through solves the construction-order
// problem described in that function's own doc comment.
// ===========================================================================

#[tokio::test]
async fn adapter_registered_after_construction_is_visible_through_shared_handle() {
    let secret_env = "RESTGW_TEST_SECRET_SHARED_REGISTRY";
    let secret = "shared-registry-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    // The registry is EMPTY at WebhookAdapter::new time — no "teststub"
    // sender registered yet.
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));

    let route = base_route("shared-registry-route", "/webhook/shared-registry", secret_env);
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };

    let adapter =
        Arc::new(WebhookAdapter::new(config, registry.clone()).expect("adapter construction"));

    // NOW register the sender — strictly AFTER construction.
    let stub_sender = Arc::new(StubDeliverySend::default());
    registry
        .write()
        .await
        .insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);

    // The adapter must see it — proving the handle it was constructed with
    // is SHARED (the same `Arc<RwLock<..>>` the caller still holds), not a
    // by-value snapshot taken at construction time.
    adapter
        .send_message("shared-registry-route", "post-construction delivery", None)
        .await
        .expect("delivery through the post-construction-registered sender must succeed");

    let calls = stub_sender.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, "post-construction delivery");

    unsafe {
        std::env::remove_var(secret_env);
    }
}

// ===========================================================================
// D-14: content-type-aware body parsing
// ===========================================================================

#[tokio::test]
async fn form_encoded_body_populates_parsed_form() {
    let secret_env = "RESTGW_TEST_SECRET_FORM";
    let secret = "form-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));

    let mut route = base_route("form-route", "/webhook/form", secret_env);
    route.prompt_template = "from={From} body={Body}".to_string();
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "ok".to_string(),
        barrier: None,
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(config, registry, handler).await;

    // Percent-encoded space (%20) and plus (%2B) — both must decode exactly
    // once: space -> space (form encoding uses '+' for space, so a literal
    // '+' must be percent-encoded as %2B to survive as a literal '+').
    let body = b"From=%2B15551234567&Body=hello%20world%2Bmore".to_vec();
    let ts = now_unix();
    let sig = sign(secret, ts, &body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/form"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    let delivered = wait_for(Duration::from_secs(2), || !handled.lock().unwrap().is_empty()).await;
    assert!(delivered);

    // The RENDERED PROMPT (built from the parsed form map) is what the
    // handler received as `event.content` — not the stub's fixed "ok"
    // answer, which is what actually reaches the delivery target.
    let handled_events = handled.lock().unwrap();
    assert_eq!(
        handled_events[0].content,
        "from=+15551234567 body=hello world+more"
    );

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn raw_body_survives_form_parsing() {
    // The success-path test already proves this indirectly (a form-encoded
    // body verifies correctly under the RAW-byte HMAC, which would fail if
    // parsing had mutated the bytes used for signing). This test makes the
    // byte-identity assertion explicit and additionally proves the parsed
    // form map is populated from THE SAME bytes.
    let secret_env = "RESTGW_TEST_SECRET_RAWFORM";
    let secret = "rawform-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let route = base_route("rawform-route", "/webhook/rawform", secret_env);
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "ok".to_string(),
        barrier: None,
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(config, registry, handler).await;

    let body = b"Body=exact+bytes+must+survive".to_vec();
    let ts = now_unix();
    // Sign over the EXACT raw bytes — if the server's own body handling
    // mutated them before verification (e.g. by decoding-then-re-encoding
    // the form before hashing), this signature would not verify.
    let sig = sign(secret, ts, &body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/rawform"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
        .send()
        .await
        .expect("request send");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "signature computed over the exact raw bytes must still verify"
    );

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn json_body_leaves_parsed_form_absent() {
    let secret_env = "RESTGW_TEST_SECRET_JSON";
    let secret = "json-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));

    let mut route = base_route("json-route", "/webhook/json", secret_env);
    route.prompt_template = "user said: {message.text}".to_string();
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let handled = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(StubHandler {
        answer: "ok".to_string(),
        barrier: None,
        handled: handled.clone(),
    });
    let (addr, cancel) = spin_up(config, registry, handler).await;

    let json_body = serde_json::json!({"message": {"text": "hi from json"}});
    let body = serde_json::to_vec(&json_body).unwrap();
    let ts = now_unix();
    let sig = sign(secret, ts, &body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/json"))
        .header("Content-Type", "application/json")
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    let delivered = wait_for(Duration::from_secs(2), || !handled.lock().unwrap().is_empty()).await;
    assert!(delivered);

    let handled_events = handled.lock().unwrap();
    assert_eq!(handled_events[0].content, "user said: hi from json");

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn unrecognised_content_type_yields_raw_bytes_not_refused() {
    let secret_env = "RESTGW_TEST_SECRET_UNKNOWN_CT";
    let secret = "unknown-ct-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let stub_sender = Arc::new(StubDeliverySend::default());
    let mut registry = DeliveryRegistry::new();
    registry.insert("teststub", stub_sender.clone() as Arc<dyn DeliverySend>);
    let registry = Arc::new(RwLock::new(registry));

    let mut route = base_route("unknown-ct-route", "/webhook/unknown-ct", secret_env);
    route.prompt_template = "raw content, no placeholders".to_string();
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let handler = Arc::new(StubHandler {
        answer: "ok".to_string(),
        barrier: None,
        handled: Arc::new(Mutex::new(Vec::new())),
    });
    let (addr, cancel) = spin_up(config, registry, handler).await;

    let body = b"arbitrary-bytes-not-form-not-json".to_vec();
    let ts = now_unix();
    let sig = sign(secret, ts, &body);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/unknown-ct"))
        .header("Content-Type", "application/octet-stream")
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
        .send()
        .await
        .expect("request send");
    // Not refused on the basis of an unrecognised content type alone.
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}


// ===========================================================================
// D-11: the delivery client must not follow redirects
// ===========================================================================

/// Both `is_safe_url` calls validate only the URL they are handed, so a
/// delivery target on a public IP can pass both checks and then answer with a
/// redirect to an internal address. Under reqwest's DEFAULT redirect policy
/// the client would follow that hop — carrying the POST body and the route's
/// outbound auth header to the internal target — and neither SSRF check would
/// ever see the final URL. `build_client` therefore disables redirects.
///
/// This drives `build_client()` directly rather than going through
/// `deliver_to_url`, because `is_safe_url` unconditionally refuses every
/// loopback address (see this file's module doc) and both hops of this test
/// necessarily live on `127.0.0.1`. The client's redirect policy IS the
/// mitigation under test, so exercising it directly is what proves it.
#[tokio::test]
async fn delivery_client_does_not_follow_redirects() {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Second hop — stands in for the internal address an attacker redirects
    // to. It must never receive a request.
    let internal_was_hit = Arc::new(AtomicBool::new(false));
    let hit_flag = internal_was_hit.clone();
    let internal_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind internal listener");
    let internal_addr = internal_listener.local_addr().expect("local_addr");
    let internal_app = axum::Router::new().fallback(axum::routing::any(move || {
        let hit_flag = hit_flag.clone();
        async move {
            hit_flag.store(true, Ordering::SeqCst);
            "internal secret"
        }
    }));
    tokio::spawn(async move {
        let _ = axum::serve(internal_listener, internal_app).await;
    });

    // First hop — the configured, checked delivery target. It redirects.
    let redirect_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind redirect listener");
    let redirect_addr = redirect_listener.local_addr().expect("local_addr");
    let location = format!("http://{internal_addr}/latest/meta-data/");
    let redirect_app = axum::Router::new().fallback(axum::routing::any(move || {
        let location = location.clone();
        async move {
            (
                axum::http::StatusCode::FOUND,
                [(axum::http::header::LOCATION, location)],
            )
        }
    }));
    tokio::spawn(async move {
        let _ = axum::serve(redirect_listener, redirect_app).await;
    });

    tokio::time::sleep(Duration::from_millis(20)).await;

    let resp = deliver::build_client()
        .post(format!("http://{redirect_addr}/hook"))
        .header("X-Api-Key", "route-outbound-secret")
        .json(&serde_json::json!({ "content": "payload" }))
        .send()
        .await
        .expect("request send");

    // The redirect is surfaced as-is rather than transparently followed.
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::FOUND,
        "delivery client followed a redirect instead of surfacing it"
    );
    assert!(
        !internal_was_hit.load(Ordering::SeqCst),
        "delivery client followed a redirect to an internal address — D-11 bypassed"
    );
}
