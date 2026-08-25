//! D-15 defensive rails tests (Phase 36.7.1 Plan 03): the body cap, the
//! self-pruning idempotency cache, the per-route rate limiter, and the rail
//! ordering that makes them safe together. Test names match PLAN.md's
//! `must_haves.truths` falsifiable-test names where PLAN.md names one.
//!
//! No test binds a fixed port — every listener uses `127.0.0.1:0` and reads
//! `local_addr()`. No test sleeps in real time to exercise TTL/window
//! behavior — this platform has no GNU `timeout` and fresh test binaries can
//! stall in the dynamic loader (source_facts #7), so any time-dependent rail
//! state is driven by an injected clock instead.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use ironhermes_core::MessageEvent;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_cron::DeliveryRegistry;
use ironhermes_restgw::webhook::idempotency::{Clock, FakeClock, HEADER_IDEMPOTENCY_KEY};
use ironhermes_restgw::webhook::route_config::{
    DeliverTarget, OutboundAuth, RouteRails, SessionMode, SignatureKind, WebhookRoute,
    WebhookRoutesConfig,
};
use ironhermes_restgw::webhook::{
    ORIGIN_CALLBACK_MAX_ENTRIES, WebhookAdapter, serve_webhook_adapter,
};
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

type HmacSha256 = Hmac<Sha256>;

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// HMAC-SHA256 over `{timestamp}.{raw_body}`, matching
/// `verifier::verify_generic_v2`'s signed-content construction exactly.
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

/// A `generic_v2`-signed route — needed for Task 3's ordering/no-consume
/// tests, which must distinguish "refused by verification" from "refused by
/// a rail", something `open_route`'s always-accepting `signature: none`
/// cannot exercise.
fn signed_route(name: &str, path: &str, secret_env: &str) -> WebhookRoute {
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

// ===========================================================================
// Shared test fixtures
// ===========================================================================

/// An always-accepting route (`signature: none`) — these rail tests are
/// about the cap/limiter/cache, not the signature scheme, so every test
/// request here is a plain unsigned POST. `WebhookAdapter` only permits
/// `signature: none` on a loopback bind host (D-10), which every test in
/// this file already uses.
fn open_route(name: &str, path: &str) -> WebhookRoute {
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

/// Stub `MessageHandler` — records every event handed to it. The
/// invocation COUNT (not just presence) is what later tasks in this file
/// (retried-delivery de-duplication, rate-limit admission counts) assert
/// against.
struct CountingHandler {
    handled: Arc<Mutex<Vec<MessageEvent>>>,
}

impl CountingHandler {
    fn new() -> (Arc<Self>, Arc<Mutex<Vec<MessageEvent>>>) {
        let handled = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Self {
                handled: handled.clone(),
            }),
            handled,
        )
    }
}

#[async_trait]
impl MessageHandler for CountingHandler {
    async fn handle(
        &self,
        event: &MessageEvent,
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        self.handled.lock().unwrap().push(event.clone());
        Ok(())
    }
}

fn config_for(routes: Vec<WebhookRoute>) -> WebhookRoutesConfig {
    WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes,
    }
}

/// Bind an ephemeral `127.0.0.1:0` listener, construct a `WebhookAdapter`
/// from `config`, spawn `serve_webhook_adapter`, and return the bound
/// address plus a `CancellationToken` the caller can fire to shut the
/// listener down. Mirrors `webhook_route_e2e.rs`'s `spin_up` helper.
async fn spin_up(
    config: WebhookRoutesConfig,
    handler: Arc<dyn MessageHandler>,
) -> (std::net::SocketAddr, CancellationToken) {
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
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

/// Same as [`spin_up`], but constructs the `WebhookAdapter` through
/// `new_with_clock` so TTL/window-dependent rail state (Task 2's idempotency
/// cache, Task 3's rate limiter) can be advanced deterministically by the
/// test instead of sleeping in real time.
async fn spin_up_with_clock(
    config: WebhookRoutesConfig,
    handler: Arc<dyn MessageHandler>,
    clock: Arc<dyn Clock>,
) -> (std::net::SocketAddr, CancellationToken) {
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let adapter = Arc::new(
        WebhookAdapter::new_with_clock(config, registry, clock).expect("adapter construction"),
    );
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

/// Poll `cond` until it is true or `timeout` elapses. Used to observe
/// asynchronously-spawned handler invocations without a fixed sleep.
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
// Task 1 (D-15): body cap enforced as a router layer, before the body is read
// ===========================================================================

#[tokio::test]
async fn oversized_body_is_refused_before_it_is_read() {
    let mut route = open_route("cap-route", "/webhook/cap");
    route.rails.max_body_bytes = 16;
    let (handler, _handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let oversized = vec![b'a'; 1024];
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/cap"))
        .body(oversized)
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

    cancel.cancel();
}

#[tokio::test]
async fn body_at_the_cap_is_accepted() {
    let mut route = open_route("at-cap-route", "/webhook/at-cap");
    route.rails.max_body_bytes = 16;
    let (handler, _handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let exactly_at_cap = vec![b'a'; 16];
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/at-cap"))
        .body(exactly_at_cap)
        .send()
        .await
        .expect("request send");
    // `signature: none` accepts unconditionally, so reaching 202 proves the
    // request proceeded past the body cap and through verification — the
    // boundary is inclusive, not off by one.
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    cancel.cancel();
}

#[tokio::test]
async fn chunked_body_without_content_length_is_still_capped() {
    let mut route = open_route("chunked-route", "/webhook/chunked");
    route.rails.max_body_bytes = 16;
    let (handler, _handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    // `Body::wrap_stream` never sets `Content-Length` — reqwest sends this
    // as a chunked-transfer request, exactly the case a manual header check
    // cannot catch (D-15 prohibition).
    let stream = tokio_stream::iter(vec![Ok::<_, std::io::Error>(vec![b'a'; 1024])]);
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/chunked"))
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

    cancel.cancel();
}

#[tokio::test]
async fn cap_is_per_route() {
    let mut strict = open_route("strict-route", "/webhook/strict");
    strict.rails.max_body_bytes = 16;
    let mut permissive = open_route("permissive-route", "/webhook/permissive");
    permissive.rails.max_body_bytes = 4096;

    let (handler, _handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![strict, permissive]), handler).await;

    let body = vec![b'a'; 100]; // over the strict cap, under the permissive one

    let client = reqwest::Client::new();
    let strict_resp = client
        .post(format!("http://{addr}/webhook/strict"))
        .body(body.clone())
        .send()
        .await
        .expect("request send");
    assert_eq!(strict_resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

    let permissive_resp = client
        .post(format!("http://{addr}/webhook/permissive"))
        .body(body)
        .send()
        .await
        .expect("request send");
    assert_eq!(
        permissive_resp.status(),
        reqwest::StatusCode::ACCEPTED,
        "a permissive route's own cap must not be lowered by a stricter sibling route"
    );

    cancel.cancel();
}

// ===========================================================================
// Task 2 (D-15): self-pruning idempotency cache so a retried delivery runs
// exactly one agent turn
// ===========================================================================

#[tokio::test]
async fn retried_delivery_runs_exactly_one_turn() {
    let route = open_route("idem-route", "/webhook/idem");
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let body = b"same body every time".to_vec();
    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{addr}/webhook/idem"))
        .header(HEADER_IDEMPOTENCY_KEY, "retry-key-1")
        .body(body.clone())
        .send()
        .await
        .expect("first request send");
    assert_eq!(resp1.status(), reqwest::StatusCode::ACCEPTED);

    let resp2 = client
        .post(format!("http://{addr}/webhook/idem"))
        .header(HEADER_IDEMPOTENCY_KEY, "retry-key-1")
        .body(body)
        .send()
        .await
        .expect("second (retried) request send");
    assert_eq!(resp2.status(), reqwest::StatusCode::ACCEPTED);

    wait_for(Duration::from_millis(200), || !handled.lock().unwrap().is_empty()).await;
    // Give a would-be second spawn a moment to have shown up too, if the
    // de-duplication were broken.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        handled.lock().unwrap().len(),
        1,
        "a retried delivery with the same idempotency key must invoke the handler exactly once"
    );

    cancel.cancel();
}

#[tokio::test]
async fn distinct_deliveries_run_distinct_turns() {
    // On an unsigned route the key falls back to a digest of route + body, so
    // two genuinely different deliveries still key apart and each runs.
    let route = open_route("idem-distinct-route", "/webhook/idem-distinct");
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let client = reqwest::Client::new();
    for body in ["body-a", "body-b"] {
        let resp = client
            .post(format!("http://{addr}/webhook/idem-distinct"))
            .body(body.as_bytes().to_vec())
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }

    wait_for(Duration::from_millis(300), || handled.lock().unwrap().len() >= 2).await;
    assert_eq!(handled.lock().unwrap().len(), 2);

    cancel.cancel();
}

#[tokio::test]
async fn sender_supplied_key_cannot_multiply_turns() {
    // CR-02 regression. `X-Webhook-Idempotency-Key` is unsigned under every
    // scheme. When it was the cache key, replaying ONE captured delivery with
    // a fresh key each time claimed a fresh entry and ran a fresh agent turn —
    // turn amplification bounded only by the rate limit. The key is now
    // derived from authenticated material, so varying this header changes
    // nothing: the identical delivery still runs exactly one turn.
    let route = open_route("idem-amplify-route", "/webhook/idem-amplify");
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let client = reqwest::Client::new();
    for key in ["attacker-key-1", "attacker-key-2", "attacker-key-3"] {
        let resp = client
            .post(format!("http://{addr}/webhook/idem-amplify"))
            .header(HEADER_IDEMPOTENCY_KEY, key)
            .body(b"one captured delivery".to_vec())
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }

    // Give any extra turns a chance to appear before asserting they did not.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        handled.lock().unwrap().len(),
        1,
        "replaying one delivery with varying idempotency keys must still run exactly one turn"
    );

    cancel.cancel();
}

#[tokio::test]
async fn sender_supplied_key_cannot_preclaim_a_future_delivery() {
    // The other half of CR-02: an attacker who guessed the key the genuine
    // sender would use could claim it first, and the real delivery would then
    // be deduped away with a 202 and silently dropped. With the key bound to
    // authenticated material, a different delivery is never pre-claimable.
    let route = open_route("idem-preclaim-route", "/webhook/idem-preclaim");
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let client = reqwest::Client::new();
    // Attacker pre-claims, naming the key the real sender will use.
    let resp = client
        .post(format!("http://{addr}/webhook/idem-preclaim"))
        .header(HEADER_IDEMPOTENCY_KEY, "delivery-42")
        .body(b"attacker body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    // The genuine delivery arrives under the same declared key.
    let resp = client
        .post(format!("http://{addr}/webhook/idem-preclaim"))
        .header(HEADER_IDEMPOTENCY_KEY, "delivery-42")
        .body(b"genuine body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);

    wait_for(Duration::from_millis(300), || handled.lock().unwrap().len() >= 2).await;
    assert_eq!(
        handled.lock().unwrap().len(),
        2,
        "the genuine delivery must still run despite the attacker's pre-claim"
    );

    cancel.cancel();
}

#[tokio::test]
async fn same_key_after_ttl_runs_again() {
    let mut route = open_route("idem-ttl-route", "/webhook/idem-ttl");
    route.rails.idempotency_ttl_secs = 60;
    let (handler, handled) = CountingHandler::new();
    let clock = Arc::new(FakeClock::new());
    let (addr, cancel) =
        spin_up_with_clock(config_for(vec![route]), handler, clock.clone()).await;

    let client = reqwest::Client::new();
    let send = || {
        let client = client.clone();
        async move {
            client
                .post(format!("http://{addr}/webhook/idem-ttl"))
                .header(HEADER_IDEMPOTENCY_KEY, "ttl-key")
                .body(b"body".to_vec())
                .send()
                .await
                .expect("request send")
        }
    };

    let resp1 = send().await;
    assert_eq!(resp1.status(), reqwest::StatusCode::ACCEPTED);
    wait_for(Duration::from_millis(200), || !handled.lock().unwrap().is_empty()).await;
    assert_eq!(handled.lock().unwrap().len(), 1);

    // Still inside the TTL: a duplicate must not run a second turn.
    let resp2 = send().await;
    assert_eq!(resp2.status(), reqwest::StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(handled.lock().unwrap().len(), 1);

    // Advance the injected clock past the TTL — the same key must claim
    // again, running a second turn. Never a real sleep (source_facts #7).
    clock.advance(Duration::from_secs(61));

    let resp3 = send().await;
    assert_eq!(resp3.status(), reqwest::StatusCode::ACCEPTED);
    wait_for(Duration::from_millis(200), || handled.lock().unwrap().len() >= 2).await;
    assert_eq!(handled.lock().unwrap().len(), 2);

    cancel.cancel();
}

#[test]
fn expired_entries_are_pruned_without_an_external_sweeper() {
    use ironhermes_restgw::webhook::idempotency::IdempotencyCache;

    let clock = Arc::new(FakeClock::new());
    let cache = IdempotencyCache::with_clock(60, clock.clone());

    assert!(cache.claim("k1"));
    assert_eq!(cache.len(), 1);

    clock.advance(Duration::from_secs(61));
    // One further access — no external sweeper, no background task, just
    // this call — is what prunes the expired entry.
    assert!(cache.claim("k1"));
    assert_eq!(
        cache.len(),
        1,
        "the expired entry was pruned, not accumulated alongside the new claim"
    );
}

#[tokio::test]
async fn keys_are_scoped_per_route() {
    let route_a = open_route("idem-route-a", "/webhook/idem-a");
    let route_b = open_route("idem-route-b", "/webhook/idem-b");
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route_a, route_b]), handler).await;

    let client = reqwest::Client::new();
    let resp_a = client
        .post(format!("http://{addr}/webhook/idem-a"))
        .header(HEADER_IDEMPOTENCY_KEY, "shared-key")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp_a.status(), reqwest::StatusCode::ACCEPTED);

    let resp_b = client
        .post(format!("http://{addr}/webhook/idem-b"))
        .header(HEADER_IDEMPOTENCY_KEY, "shared-key")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp_b.status(), reqwest::StatusCode::ACCEPTED);

    wait_for(Duration::from_millis(300), || handled.lock().unwrap().len() >= 2).await;
    assert_eq!(
        handled.lock().unwrap().len(),
        2,
        "the same key value on two different routes must claim independently"
    );

    cancel.cancel();
}

#[tokio::test]
async fn absent_key_falls_back_to_a_derived_digest() {
    let route = open_route("idem-nokey-route", "/webhook/idem-nokey");
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let body = b"identical body, no idempotency header at all".to_vec();
    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{addr}/webhook/idem-nokey"))
        .body(body.clone())
        .send()
        .await
        .expect("first request send");
    assert_eq!(resp1.status(), reqwest::StatusCode::ACCEPTED);

    let resp2 = client
        .post(format!("http://{addr}/webhook/idem-nokey"))
        .body(body)
        .send()
        .await
        .expect("second (retried) request send");
    assert_eq!(resp2.status(), reqwest::StatusCode::ACCEPTED);

    wait_for(Duration::from_millis(200), || !handled.lock().unwrap().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        handled.lock().unwrap().len(),
        1,
        "a sender that never sends an idempotency key must still be de-duplicated by a \
         digest of the route name and the raw body"
    );

    cancel.cancel();
}

// ===========================================================================
// CR-02 (second pass): the idempotency key must come from the header THIS
// route's own verifier checked, never from whichever of the three signature
// headers happens to be present.
// ===========================================================================

/// Twilio's canonical signed string: the request URL, then every form
/// parameter as key-immediately-followed-by-value, sorted by key, with no
/// separators — HMAC-SHA1 with the auth token, base64. Mirrors
/// `verifier::verify_twilio` exactly.
fn twilio_sign(auth_token: &str, request_url: &str, params: &[(&str, &str)]) -> String {
    use base64::Engine as _;
    type HmacSha1 = Hmac<sha1::Sha1>;
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut canonical = String::from(request_url);
    for (k, v) in &sorted {
        canonical.push_str(k);
        canonical.push_str(v);
    }
    let mut mac = HmacSha1::new_from_slice(auth_token.as_bytes()).unwrap();
    mac.update(canonical.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// A `twilio`-signed route. `external_base_url` is pinned by the caller so
/// the canonical signed URL does not depend on the ephemeral port.
fn twilio_route(name: &str, path: &str, auth_token_env: &str) -> WebhookRoute {
    WebhookRoute {
        signature: SignatureKind::Twilio,
        secret_env: None,
        auth_token_env: Some(auth_token_env.to_string()),
        ..open_route(name, path)
    }
}

fn config_with_base(routes: Vec<WebhookRoute>, base: &str) -> WebhookRoutesConfig {
    WebhookRoutesConfig {
        external_base_url: Some(base.to_string()),
        ..config_for(routes)
    }
}

/// CR-02, reopened. The first fix keyed the idempotency cache on "the
/// signature", but selected it by scanning
/// `[V2, TWILIO, TELNYX]` for whichever header was PRESENT — a scan
/// independent of the route's configured scheme, with `X-Webhook-Signature-V2`
/// first.
///
/// `verify_twilio` reads only `X-Twilio-Signature`, and rejects nothing for
/// carrying extra headers. So one captured, validly-signed Twilio delivery,
/// replayed verbatim with only a fresh `X-Webhook-Signature-V2` nonce added
/// each time, derived a distinct key per replay, claimed each one, and
/// spawned a full agent turn per replay — bounded only by the rate limit, and
/// by nothing at all in time (Twilio's scheme carries no timestamp).
///
/// The Twilio signature is byte-identical across all three requests here: it
/// covers the URL and the form params, neither of which changes. Only the
/// unverified header varies.
#[tokio::test]
async fn an_unverified_signature_header_cannot_multiply_turns_on_a_twilio_route() {
    const TOKEN: &str = "cr02-twilio-auth-token";
    const BASE: &str = "http://cr02-twilio.local";
    const PATH: &str = "/webhook/cr02-twilio";
    unsafe { std::env::set_var("IH_TEST_CR02_TWILIO_TOKEN", TOKEN) };

    let route = twilio_route("cr02-twilio-route", PATH, "IH_TEST_CR02_TWILIO_TOKEN");
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_with_base(vec![route], BASE), handler).await;

    let params = [("Body", "one captured delivery")];
    let signature = twilio_sign(TOKEN, &format!("{BASE}{PATH}"), &params);
    let form_body = "Body=one+captured+delivery";

    let client = reqwest::Client::new();
    for nonce in ["nonce-1", "nonce-2", "nonce-3"] {
        let resp = client
            .post(format!("http://{addr}{PATH}"))
            .header("X-Twilio-Signature", &signature)
            // The header nothing on this route verifies. Attacker-chosen,
            // fresh per replay.
            .header("X-Webhook-Signature-V2", nonce)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body)
            .send()
            .await
            .expect("request send");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::ACCEPTED,
            "the Twilio signature is unchanged and still valid — verification must pass, so \
             this test is exercising the idempotency key and not a verification refusal"
        );
    }

    // Give any extra turns a chance to appear before asserting they did not.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        handled.lock().unwrap().len(),
        1,
        "one captured Twilio delivery replayed N times must run exactly one turn — the \
         idempotency key must be derived from X-Twilio-Signature (the header this route's \
         verifier actually checked), never from an unverified X-Webhook-Signature-V2 the \
         caller chose"
    );

    cancel.cancel();
}

/// The `SignatureKind::None` arm of the same defect. An unsigned (loopback)
/// route's ONLY de-duplication is `derive_key`'s route+body digest fallback.
/// Under the scanning selection, supplying any `X-Webhook-Signature-V2` value
/// took `derive_key`'s `Some(sig)` branch and skipped that fallback entirely,
/// so identical bodies keyed apart and each ran a turn.
#[tokio::test]
async fn an_unverified_signature_header_cannot_bypass_the_body_digest_on_an_unsigned_route() {
    let route = open_route("cr02-none-route", "/webhook/cr02-none");
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let client = reqwest::Client::new();
    for nonce in ["nonce-a", "nonce-b", "nonce-c"] {
        let resp = client
            .post(format!("http://{addr}/webhook/cr02-none"))
            .header("X-Webhook-Signature-V2", nonce)
            .body(b"identical body".to_vec())
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        handled.lock().unwrap().len(),
        1,
        "an unsigned route has no authenticated material to key on, so it must fall back to \
         the route+body digest — a caller-supplied signature header must not be able to \
         switch the key derivation onto a value the caller controls"
    );

    cancel.cancel();
}

/// The other side of the same selection: a `generic_v2` route must still key
/// on its own verified `X-Webhook-Signature-V2`, so two SEPARATELY signed
/// deliveries (distinct timestamps, hence distinct signatures) with identical
/// bodies still run two turns. Without this, "select by scheme" could be
/// satisfied by always returning `None` and de-duplicating everything by body
/// — which would silently swallow legitimate repeat deliveries.
#[tokio::test]
async fn a_generic_v2_route_still_keys_on_its_own_verified_signature() {
    const SECRET: &str = "cr02-generic-secret";
    unsafe { std::env::set_var("IH_TEST_CR02_GENERIC_SECRET", SECRET) };

    let route = signed_route(
        "cr02-generic-route",
        "/webhook/cr02-generic",
        "IH_TEST_CR02_GENERIC_SECRET",
    );
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let body = b"identical body".to_vec();
    let client = reqwest::Client::new();
    // Two distinct timestamps inside the 300s skew window → two distinct
    // valid signatures over the same body.
    for ts in [now_unix(), now_unix() - 5] {
        let resp = client
            .post(format!("http://{addr}/webhook/cr02-generic"))
            .header("X-Webhook-Signature-V2", sign(SECRET, ts, &body))
            .header("X-Webhook-Timestamp", ts.to_string())
            .body(body.clone())
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }

    wait_for(Duration::from_millis(500), || {
        handled.lock().unwrap().len() >= 2
    })
    .await;
    assert_eq!(
        handled.lock().unwrap().len(),
        2,
        "two separately signed generic_v2 deliveries must key apart — this route's own \
         verified signature is still the key"
    );

    cancel.cancel();
}

// ===========================================================================
// Task 3 (D-15): per-route fixed-window rate limit, and the rail ordering
// that makes it worth having
// ===========================================================================

#[tokio::test]
async fn excess_requests_in_one_window_are_refused() {
    let mut route = open_route("ratelimit-route", "/webhook/ratelimit");
    route.rails.rate_limit_per_minute = 3;
    let (handler, _handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let client = reqwest::Client::new();
    for i in 0..3 {
        let resp = client
            .post(format!("http://{addr}/webhook/ratelimit"))
            .header(HEADER_IDEMPOTENCY_KEY, format!("key-{i}"))
            .body(b"body".to_vec())
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }

    let resp = client
        .post(format!("http://{addr}/webhook/ratelimit"))
        .header(HEADER_IDEMPOTENCY_KEY, "key-overflow")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    cancel.cancel();
}

#[tokio::test]
async fn limiter_admits_again_after_the_window_rolls() {
    let mut route = open_route("ratelimit-roll-route", "/webhook/ratelimit-roll");
    route.rails.rate_limit_per_minute = 1;
    let (handler, _handled) = CountingHandler::new();
    let clock = Arc::new(FakeClock::new());
    let (addr, cancel) =
        spin_up_with_clock(config_for(vec![route]), handler, clock.clone()).await;

    let client = reqwest::Client::new();
    let resp1 = client
        .post(format!("http://{addr}/webhook/ratelimit-roll"))
        .header(HEADER_IDEMPOTENCY_KEY, "k1")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp1.status(), reqwest::StatusCode::ACCEPTED);

    let resp2 = client
        .post(format!("http://{addr}/webhook/ratelimit-roll"))
        .header(HEADER_IDEMPOTENCY_KEY, "k2")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp2.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    // Advance the injected clock past the 60s window — never a real sleep
    // (source_facts #7).
    clock.advance(Duration::from_secs(61));

    let resp3 = client
        .post(format!("http://{addr}/webhook/ratelimit-roll"))
        .header(HEADER_IDEMPOTENCY_KEY, "k3")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp3.status(), reqwest::StatusCode::ACCEPTED);

    cancel.cancel();
}

#[tokio::test]
async fn rate_limit_is_scoped_per_route() {
    let mut strict = open_route("ratelimit-strict", "/webhook/ratelimit-strict");
    strict.rails.rate_limit_per_minute = 1;
    let mut generous = open_route("ratelimit-generous", "/webhook/ratelimit-generous");
    generous.rails.rate_limit_per_minute = 30;

    let (handler, _handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![strict, generous]), handler).await;

    let client = reqwest::Client::new();
    let resp1 = client
        .post(format!("http://{addr}/webhook/ratelimit-strict"))
        .header(HEADER_IDEMPOTENCY_KEY, "s1")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp1.status(), reqwest::StatusCode::ACCEPTED);

    let resp2 = client
        .post(format!("http://{addr}/webhook/ratelimit-strict"))
        .header(HEADER_IDEMPOTENCY_KEY, "s2")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp2.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    // The generous route's own budget is unaffected by the strict route's
    // exhaustion.
    let resp3 = client
        .post(format!("http://{addr}/webhook/ratelimit-generous"))
        .header(HEADER_IDEMPOTENCY_KEY, "g1")
        .body(b"body".to_vec())
        .send()
        .await
        .expect("request send");
    assert_eq!(resp3.status(), reqwest::StatusCode::ACCEPTED);

    cancel.cancel();
}

#[tokio::test]
async fn unsigned_request_consumes_no_rail_state() {
    let secret_env = "RESTGW_TEST_SECRET_UNSIGNED_NOCONSUME";
    let secret = "unsigned-noconsume-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let mut route = signed_route("unsigned-noconsume", "/webhook/unsigned-noconsume", secret_env);
    route.rails.rate_limit_per_minute = 3;
    let (handler, handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let client = reqwest::Client::new();
    let shared_body = b"shared-body-used-by-unsigned-attempts".to_vec();

    // Several unsigned requests carrying the SAME body an eventual valid
    // request will use — all refused with 401, none touching rate-limit
    // budget or idempotency state.
    for _ in 0..5 {
        let resp = client
            .post(format!("http://{addr}/webhook/unsigned-noconsume"))
            .body(shared_body.clone())
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    }

    // A correctly-signed request with the SAME body as the unsigned
    // attempts must still run the handler — if an unsigned attempt had
    // wrongly created an idempotency entry, this would be silently
    // de-duplicated (still 202, but the handler would never run).
    let ts = now_unix();
    let sig = sign(secret, ts, &shared_body);
    let resp = client
        .post(format!("http://{addr}/webhook/unsigned-noconsume"))
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(shared_body)
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    wait_for(Duration::from_millis(200), || !handled.lock().unwrap().is_empty()).await;
    assert_eq!(
        handled.lock().unwrap().len(),
        1,
        "the idempotency claim must not have been taken by any unsigned attempt"
    );

    // Two more DISTINCT-body valid requests fill the rate-limit window to
    // its configured limit of 3 -- proving none of the five unsigned
    // attempts consumed any of that budget.
    for i in 0..2 {
        let body = format!("distinct-valid-body-{i}").into_bytes();
        let ts = now_unix();
        let sig = sign(secret, ts, &body);
        let resp = client
            .post(format!("http://{addr}/webhook/unsigned-noconsume"))
            .header("X-Webhook-Signature-V2", sig)
            .header("X-Webhook-Timestamp", ts.to_string())
            .body(body)
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }
    wait_for(Duration::from_millis(200), || handled.lock().unwrap().len() >= 3).await;
    assert_eq!(handled.lock().unwrap().len(), 3);

    // The window is now genuinely exhausted by the three VALID requests —
    // proving the limiter is not simply a no-op.
    let body = b"fourth-valid-body".to_vec();
    let ts = now_unix();
    let sig = sign(secret, ts, &body);
    let resp = client
        .post(format!("http://{addr}/webhook/unsigned-noconsume"))
        .header("X-Webhook-Signature-V2", sig)
        .header("X-Webhook-Timestamp", ts.to_string())
        .body(body)
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

#[tokio::test]
async fn oversized_body_consumes_no_rail_state() {
    let mut route = open_route("oversized-noconsume", "/webhook/oversized-noconsume");
    route.rails.max_body_bytes = 16;
    route.rails.rate_limit_per_minute = 2;
    let (handler, _handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    let client = reqwest::Client::new();
    let oversized = vec![b'a'; 1024];
    for _ in 0..5 {
        let resp = client
            .post(format!("http://{addr}/webhook/oversized-noconsume"))
            .body(oversized.clone())
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    }

    // A full window of valid, correctly-sized requests must still be
    // admitted, proving none of the oversized attempts consumed budget.
    for i in 0..2 {
        let resp = client
            .post(format!("http://{addr}/webhook/oversized-noconsume"))
            .header(HEADER_IDEMPOTENCY_KEY, format!("k{i}"))
            .body(vec![b'a'; 8])
            .send()
            .await
            .expect("request send");
        assert_eq!(resp.status(), reqwest::StatusCode::ACCEPTED);
    }

    let resp = client
        .post(format!("http://{addr}/webhook/oversized-noconsume"))
        .header(HEADER_IDEMPOTENCY_KEY, "k-overflow")
        .body(vec![b'a'; 8])
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);

    cancel.cancel();
}

#[tokio::test]
async fn rails_run_in_the_documented_order() {
    let secret_env = "RESTGW_TEST_SECRET_ORDER";
    let secret = "order-test-secret";
    unsafe {
        std::env::set_var(secret_env, secret);
    }

    let mut route = signed_route("order-route", "/webhook/order", secret_env);
    route.rails.max_body_bytes = 16;
    let (handler, _handled) = CountingHandler::new();
    let (addr, cancel) = spin_up(config_for(vec![route]), handler).await;

    // Oversized AND unsigned -- the body cap runs first (a router layer,
    // before the handler even sees headers), so this must report 413, not
    // 401.
    let oversized_unsigned = vec![b'a'; 1024];
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/webhook/order"))
        .body(oversized_unsigned)
        .send()
        .await
        .expect("request send");
    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

    cancel.cancel();
    unsafe {
        std::env::remove_var(secret_env);
    }
}

// ===========================================================================
// Security audit N-03: duplicate route name / path refused at construction
// ===========================================================================

/// Two routes sharing a `name` silently cross-wire. The adapter's verifier,
/// idempotency and rate-limit maps are keyed on `route.name` (last write
/// wins), while `deliver_for` resolves the delivery target by scanning the
/// route list with `.find()` (first match). So a request arriving on the
/// shared name would be verified with the SECOND route's secret and its
/// answer delivered to the FIRST route's target — defeating the per-route
/// secret isolation T-36.7.1-01 exists to provide, with no error raised
/// anywhere. Construction must refuse instead.
#[test]
fn duplicate_route_name_is_refused_at_construction() {
    let config = config_for(vec![
        open_route("shared-name", "/hooks/a"),
        open_route("shared-name", "/hooks/b"),
    ]);
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let err = WebhookAdapter::new(config, registry)
        .map(|_| ())
        .expect_err("duplicate route name must refuse construction");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("duplicate route name") && msg.contains("shared-name"),
        "the error must name the offending route: {msg}"
    );
}

/// Duplicate `path` is worse-behaved than it looks: `axum::Router::route`
/// panics on a repeated path, and that panic happens inside the spawned serve
/// task — AFTER `running.store(true)`. The adapter would report itself started
/// and then die, escaping the fail-closed-at-construction contract.
#[test]
fn duplicate_route_path_is_refused_at_construction() {
    let config = config_for(vec![
        open_route("route-a", "/hooks/shared"),
        open_route("route-b", "/hooks/shared"),
    ]);
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let err = WebhookAdapter::new(config, registry)
        .map(|_| ())
        .expect_err("duplicate route path must refuse construction");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("duplicate route path") && msg.contains("/hooks/shared"),
        "the error must name the offending path: {msg}"
    );
}

/// The check must not over-reject: distinct names AND distinct paths still
/// construct, including names that merely share a prefix.
#[test]
fn distinct_routes_still_construct() {
    let config = config_for(vec![
        open_route("route", "/hooks/a"),
        open_route("route-2", "/hooks/b"),
    ]);
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    assert!(WebhookAdapter::new(config, registry).is_ok());
}

// ===========================================================================
// Security audit N-02: origin_callbacks is bounded, not a growth path
// ===========================================================================

/// Builds a `deliver: origin` route — the only shape that records callback
/// URLs at all.
fn origin_route(name: &str, path: &str) -> WebhookRoute {
    WebhookRoute {
        deliver: DeliverTarget::Origin,
        deliver_platform: None,
        ..open_route(name, path)
    }
}

/// The field's original comment claimed the unclaimed-entry case was "a
/// bounded, documented leak, not a growth path". It was not: one entry is
/// inserted per REQUEST, and the only removal lives in `send_message`, which
/// per WINDOWS ledger 17 is not reached for these turns at all. Entries
/// therefore accumulated with lifetime request count, each holding a
/// caller-supplied URL.
///
/// Asserted through the adapter's own prune-on-insert path with a `FakeClock`,
/// so the TTL is exercised deterministically rather than by sleeping.
#[test]
fn origin_callbacks_prune_themselves_without_an_external_sweeper() {
    let clock = Arc::new(FakeClock::new());
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let adapter = WebhookAdapter::new_with_clock(
        config_for(vec![origin_route("origin-route", "/hooks/origin")]),
        registry,
        clock.clone() as Arc<dyn Clock>,
    )
    .expect("adapter construction");

    for i in 0..50 {
        assert!(adapter.record_origin_callback_for_test(
            format!("delivery-{i}"),
            "https://caller.test/callback".to_string(),
        ));
    }
    assert_eq!(
        adapter.origin_callback_entry_count(),
        50,
        "entries are retained while live — a turn still running must be able to deliver"
    );

    // Past the TTL, the next insert sweeps every stale entry.
    clock.advance(Duration::from_secs(3601));
    assert!(adapter.record_origin_callback_for_test(
        "delivery-fresh".to_string(),
        "https://caller.test/callback".to_string(),
    ));
    assert_eq!(
        adapter.origin_callback_entry_count(),
        1,
        "expired entries must be pruned on access, leaving only the fresh one"
    );
}

/// WR-03. The test above inserts 50 entries and advances the clock — it
/// exercises `retain` only. Nothing ever drove `map.len() >=
/// ORIGIN_CALLBACK_MAX_ENTRIES`, so neither the `false` return nor its
/// consumer in `handle_webhook_post` (which flips `thread_id` to `None` and
/// changes the whole downstream delivery outcome) had ever executed under
/// test. The cap is the backstop for the one case the TTL cannot cover —
/// sustained arrival faster than entries expire — so it is exactly the arm
/// that runs under attack.
///
/// No clock advance here, deliberately: every entry stays live, so `retain`
/// evicts nothing and the cap is the only thing standing.
#[test]
fn the_origin_callback_cap_refuses_the_entry_past_the_limit() {
    let clock = Arc::new(FakeClock::new());
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));
    let adapter = WebhookAdapter::new_with_clock(
        config_for(vec![origin_route("origin-cap-route", "/hooks/origin-cap")]),
        registry,
        clock.clone() as Arc<dyn Clock>,
    )
    .expect("adapter construction");

    for i in 0..ORIGIN_CALLBACK_MAX_ENTRIES {
        assert!(
            adapter.record_origin_callback_for_test(
                format!("delivery-{i}"),
                "https://caller.test/callback".to_string(),
            ),
            "entry {i} is within the cap and must be recorded"
        );
    }
    assert_eq!(
        adapter.origin_callback_entry_count(),
        ORIGIN_CALLBACK_MAX_ENTRIES,
        "the table must fill to exactly the cap"
    );

    assert!(
        !adapter.record_origin_callback_for_test(
            "delivery-over-the-cap".to_string(),
            "https://caller.test/callback".to_string(),
        ),
        "at the cap with nothing evictable, recording must be REFUSED — carrying a \
         thread_id whose entry does not exist would make send_message report \
         'already consumed', the same symptom as a genuine double-delivery"
    );
    assert_eq!(
        adapter.origin_callback_entry_count(),
        ORIGIN_CALLBACK_MAX_ENTRIES,
        "the refused entry must not have been inserted — the cap bounds the table, \
         it does not merely log about it"
    );

    // The cap is not a wedge: an EXISTING key must still be updatable, and
    // once the TTL sweeps the table, recording resumes.
    assert!(
        adapter.record_origin_callback_for_test(
            "delivery-0".to_string(),
            "https://caller.test/updated".to_string(),
        ),
        "a key already in the table is not a new entry and must still be accepted"
    );
    clock.advance(Duration::from_secs(3601));
    assert!(
        adapter.record_origin_callback_for_test(
            "delivery-after-ttl".to_string(),
            "https://caller.test/callback".to_string(),
        ),
        "once the TTL sweeps the table the cap must stop refusing — otherwise one \
         burst wedges the route's origin delivery for the process lifetime"
    );
}
