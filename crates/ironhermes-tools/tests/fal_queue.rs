//! Phase 01 Plan 02 — stateful `wiremock` test matrix for the hardened
//! `FalClient` queue state machine (TEST-01).
//!
//! These tests point a `FalClient` at a `wiremock` `MockServer` via the
//! injectable base URL and prove:
//!   - the full IN_QUEUE → IN_PROGRESS → COMPLETED → result flow (happy path),
//!   - timeout fires a `PUT .../requests/{id}/cancel` (GEN-02),
//!   - a 422 `content_policy_violation` is a clear, non-retried error and the
//!     branch keys on `detail[].type`, never `msg` (SAFE-04),
//!   - `X-Fal-Retryable` governs retry: a retryable 503 is retried, a
//!     non-retryable 422 is not (SAFE-04),
//!   - an SSRF result URL (loopback) is rejected with no file written (SAFE-03),
//!   - an oversized CDN result yields `DownloadOutcome::OversizedDocument`
//!     (client layer) and drives `ImageGenTool::execute()` to emit the
//!     document/text-link fallback rather than a photo `<MEDIA:>` tag (DLV-04).
//!
//! Entirely within `ironhermes-tools` — no `ironhermes-gateway` dependency and
//! no call to the private gateway `check_size_cap`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ironhermes_core::Config;
use ironhermes_tools::fal::{DownloadOutcome, FalClient};
use ironhermes_tools::image_gen::ImageGenTool;
use ironhermes_tools::registry::Tool;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const MODEL: &str = "fal-ai/flux/schnell";
/// fal collapses the sub-model for the queue status/result/cancel routes: a
/// submit to `fal-ai/flux/schnell` returns status/response/cancel URLs under the
/// parent app `fal-ai/flux`. The client must follow those returned URLs, never
/// reconstruct `{model}/requests/...` (which 405s on the real API). These mocks
/// mirror that collapse so the suite would catch a regression to reconstruction.
const PARENT: &str = "fal-ai/flux";

/// A `Respond` that returns a different template on each successive call,
/// reusing the last template once the script is exhausted. Lets us model a
/// status endpoint that walks IN_QUEUE → IN_PROGRESS → COMPLETED.
struct Sequence {
    calls: AtomicUsize,
    templates: Vec<ResponseTemplate>,
}

impl Sequence {
    fn new(templates: Vec<ResponseTemplate>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            templates,
        }
    }
}

impl Respond for Sequence {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let idx = n.min(self.templates.len() - 1);
        self.templates[idx].clone()
    }
}

fn status_url(id: &str) -> String {
    format!("/{PARENT}/requests/{id}/status")
}

fn result_path(id: &str) -> String {
    format!("/{PARENT}/requests/{id}")
}

fn cancel_path(id: &str) -> String {
    format!("/{PARENT}/requests/{id}/cancel")
}

/// Mount the submit endpoint returning a fixed request id plus the
/// status/response/cancel URLs fal hands back — pointed at the parent-app path
/// so a client that wrongly reconstructs `{model}/requests/...` would miss them.
async fn mount_submit(server: &MockServer, request_id: &str) {
    let base = server.uri();
    Mock::given(method("POST"))
        .and(path(format!("/{MODEL}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": request_id,
            "status_url": format!("{base}{}", status_url(request_id)),
            "response_url": format!("{base}{}", result_path(request_id)),
            "cancel_url": format!("{base}{}", cancel_path(request_id)),
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn test_full_queue_flow() {
    let server = MockServer::start().await;
    let id = "req-flow";
    mount_submit(&server, id).await;

    // status: IN_QUEUE → IN_PROGRESS → COMPLETED (then sticky COMPLETED).
    Mock::given(method("GET"))
        .and(path(status_url(id)))
        .respond_with(Sequence::new(vec![
            ResponseTemplate::new(200).set_body_json(json!({"status": "IN_QUEUE"})),
            ResponseTemplate::new(200).set_body_json(json!({"status": "IN_PROGRESS"})),
            ResponseTemplate::new(200).set_body_json(json!({"status": "COMPLETED"})),
        ]))
        .mount(&server)
        .await;

    // result: one image whose URL points back at the mock (a public-looking host
    // would fail SSRF; download is exercised in the oversized test, not here).
    Mock::given(method("GET"))
        .and(path(result_path(id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "images": [{
                "url": format!("{}/cdn/img.png", server.uri()),
                "width": 512,
                "height": 512,
                "content_type": "image/png"
            }],
            "seed": 42
        })))
        .mount(&server)
        .await;

    let client = FalClient::with_base_url(server.uri());
    let submit = client
        .submit("test-key", MODEL, "a cat")
        .await
        .expect("submit ok");
    client
        .poll("test-key", &submit, std::time::Duration::from_secs(10))
        .await
        .expect("poll drives to COMPLETED");
    let result = client.fetch("test-key", &submit).await.expect("fetch ok");
    assert_eq!(result.images.len(), 1);

    // submit → (>=1 status) → result, in order.
    let reqs = server.received_requests().await.unwrap_or_default();
    let paths: Vec<String> = reqs.iter().map(|r| r.url.path().to_string()).collect();
    assert_eq!(
        paths.first().map(String::as_str),
        Some(format!("/{MODEL}").as_str())
    );
    assert!(
        paths.iter().any(|p| p == &status_url(id)),
        "a status call must occur, got {paths:?}"
    );
    assert_eq!(
        paths.last().map(String::as_str),
        Some(result_path(id).as_str())
    );
}

#[tokio::test]
async fn test_timeout_cancels_job() {
    let server = MockServer::start().await;
    let id = "req-timeout";
    mount_submit(&server, id).await;

    // status NEVER completes.
    Mock::given(method("GET"))
        .and(path(status_url(id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "IN_PROGRESS"})))
        .mount(&server)
        .await;

    // cancel endpoint — assert it is hit (at the returned cancel_url path).
    Mock::given(method("PUT"))
        .and(path(cancel_path(id)))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let client = FalClient::with_base_url(server.uri());
    let submit = client
        .submit("test-key", MODEL, "never finishes")
        .await
        .expect("submit ok");
    let err = client
        .poll("test-key", &submit, std::time::Duration::from_secs(1))
        .await
        .expect_err("poll must time out");
    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("timed out") || msg.to_lowercase().contains("timeout"),
        "error must mention timeout, got: {msg}"
    );

    let reqs = server.received_requests().await.unwrap_or_default();
    assert!(
        reqs.iter()
            .any(|r| r.method == wiremock::http::Method::PUT && r.url.path() == cancel_path(id)),
        "a cancel PUT must fire on timeout (GEN-02)"
    );
}

#[tokio::test]
async fn test_422_content_policy_non_retried() {
    let server = MockServer::start().await;

    // submit returns 422 content_policy_violation with X-Fal-Retryable: false.
    Mock::given(method("POST"))
        .and(path(format!("/{MODEL}")))
        .respond_with(
            ResponseTemplate::new(422)
                .insert_header("X-Fal-Retryable", "false")
                .set_body_json(json!({
                    "detail": [{
                        "loc": ["body", "prompt"],
                        "msg": "this could change at any time",
                        "type": "content_policy_violation"
                    }]
                })),
        )
        .mount(&server)
        .await;

    let client = FalClient::with_base_url(server.uri());
    let err = client
        .submit("test-key", MODEL, "blocked prompt")
        .await
        .expect_err("422 content policy must error");
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("content")
            && (msg.contains("policy") || msg.contains("rejected") || msg.contains("blocked")),
        "error must be marked a content-policy rejection, got: {msg}"
    );

    // No status/retry call must follow the 422 — only the single submit POST.
    let reqs = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        reqs.len(),
        1,
        "no retry/status after a 422 content policy (SAFE-04)"
    );
    assert_eq!(reqs[0].method, wiremock::http::Method::POST);
}

#[tokio::test]
async fn test_retryable_header_governs_retry() {
    // (a) Retryable 503 → submit retries (then succeeds).
    let server = MockServer::start().await;
    let id = "req-retry";
    Mock::given(method("POST"))
        .and(path(format!("/{MODEL}")))
        .respond_with(Sequence::new(vec![
            ResponseTemplate::new(503).insert_header("X-Fal-Retryable", "true"),
            ResponseTemplate::new(200).set_body_json(json!({
                "request_id": id, "status_url": "", "response_url": "", "cancel_url": ""
            })),
        ]))
        .mount(&server)
        .await;

    let client = FalClient::with_base_url(server.uri());
    let resp = client
        .submit("test-key", MODEL, "ok prompt")
        .await
        .expect("retryable 503 must be retried then succeed");
    assert_eq!(resp.request_id, id);
    let reqs = server.received_requests().await.unwrap_or_default();
    assert!(
        reqs.len() >= 2,
        "retryable error must produce >=2 submit attempts"
    );

    // (b) Non-retryable 422 (no content policy) → NOT retried.
    let server2 = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/{MODEL}")))
        .respond_with(
            ResponseTemplate::new(422)
                .insert_header("X-Fal-Retryable", "false")
                .set_body_json(json!({
                    "detail": [{"loc": [], "msg": "bad", "type": "value_error"}]
                })),
        )
        .mount(&server2)
        .await;
    let client2 = FalClient::with_base_url(server2.uri());
    let _ = client2
        .submit("test-key", MODEL, "bad")
        .await
        .expect_err("422 errors");
    let reqs2 = server2.received_requests().await.unwrap_or_default();
    assert_eq!(
        reqs2.len(),
        1,
        "non-retryable 422 must NOT be retried (SAFE-04)"
    );
}

#[tokio::test]
async fn test_ssrf_result_url_rejected() {
    // No mock needed for the CDN: the SSRF gate must reject before any GET.
    let cache_dir = ironhermes_core::constants::get_hermes_home()
        .join("cache")
        .join("images");
    let before = count_files(&cache_dir);

    let client = FalClient::with_base_url("http://unused.invalid");
    let err = client
        .download_to_cache("http://127.0.0.1/x.png", "image/png")
        .await
        .expect_err("loopback URL must be rejected");
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("ssrf") || msg.contains("rejected"),
        "error must name SSRF, got: {msg}"
    );

    let after = count_files(&cache_dir);
    assert_eq!(
        before, after,
        "no file may be written when SSRF rejects (SAFE-03)"
    );
}

#[tokio::test]
async fn test_oversized_client_returns_outcome() {
    let server = MockServer::start().await;

    // A CDN response just over the in-test photo cap. We use a small payload and
    // a tiny test cap via the test-only download entry point.
    let body = vec![0u8; 64 * 1024]; // 64 KiB
    Mock::given(method("GET"))
        .and(path("/cdn/big.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&server)
        .await;

    // The mock CDN binds to loopback; permit it (the SSRF gate itself is proven
    // by `test_ssrf_result_url_rejected`).
    let client = FalClient::with_base_url_allow_loopback(server.uri());
    let url = format!("{}/cdn/big.png", server.uri());
    // 32 KiB cap → 64 KiB body is oversized.
    let outcome = client
        .download_to_cache_with_cap(&url, "image/png", 32 * 1024)
        .await
        .expect("download succeeds even when oversized");
    match outcome {
        DownloadOutcome::OversizedDocument(p) => {
            assert!(p.is_absolute(), "path must be absolute");
            assert!(p.exists(), "oversized file is still written to cache");
            let _ = std::fs::remove_file(&p);
        }
        DownloadOutcome::Photo(p) => panic!("expected OversizedDocument, got Photo({p:?})"),
        DownloadOutcome::Video(p) => panic!("expected OversizedDocument, got Video({p:?})"),
    }
}

#[tokio::test]
async fn test_oversized_tool_emits_document_fallback() {
    let server = MockServer::start().await;
    let id = "req-big";
    mount_submit(&server, id).await;

    Mock::given(method("GET"))
        .and(path(status_url(id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "COMPLETED"})))
        .mount(&server)
        .await;

    let cdn_url = format!("{}/cdn/huge.png", server.uri());
    Mock::given(method("GET"))
        .and(path(result_path(id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "images": [{
                "url": cdn_url,
                "width": 4096,
                "height": 4096,
                "content_type": "image/png"
            }]
        })))
        .mount(&server)
        .await;

    // Oversized CDN body: 21 MB > 20 MB photo cap.
    let body = vec![0u8; 21 * 1024 * 1024];
    Mock::given(method("GET"))
        .and(path("/cdn/huge.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(&server)
        .await;

    // Build a tool whose client points at the mock server.
    // Phase 47 Plan 06: execute() now resolves the effective model from
    // `image_gen.t2i.model` (D-01/D-03), not the legacy `default_model` flat
    // key — set t2i explicitly (with an explicit `fal` provider so it can
    // never be misrouted to venice) so this test keeps exercising the
    // existing fal-routed path (GEN-01 back-compat).
    let mut config = Config::default();
    config.image_gen.default_model = MODEL.to_string();
    config.image_gen.t2i.provider = Some("fal".to_string());
    config.image_gen.t2i.model = MODEL.to_string();
    config.image_gen.timeout_secs = 10;
    let tool = ImageGenTool::new(
        Arc::new(config),
        FalClient::with_base_url_allow_loopback(server.uri()),
    );

    let prior = std::env::var("FAL_KEY").ok();
    unsafe {
        std::env::set_var("FAL_KEY", "test-key-not-real");
    }
    let out = tool.execute(json!({"prompt": "a huge poster"})).await;
    unsafe {
        match prior {
            Some(v) => std::env::set_var("FAL_KEY", v),
            None => std::env::remove_var("FAL_KEY"),
        }
    }

    let out = out.expect("tool must succeed with a fallback, not error");
    assert!(
        !out.contains("<MEDIA:"),
        "oversized result must NOT emit a photo <MEDIA:> tag, got: {out}"
    );
    assert!(
        out.to_lowercase().contains("document")
            || out.to_lowercase().contains("too large")
            || out.to_lowercase().contains("link"),
        "oversized result must emit a document/text-link fallback, got: {out}"
    );
}

/// Count regular files directly under `dir` (0 if the dir does not exist).
fn count_files(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .count()
        })
        .unwrap_or(0)
}
