//! Phase 36.3.3 Plan 01 — Wave-0 test scaffold for the fal video queue flow.
//!
//! Implements the behavior tests specified in Plan 01 Task 2:
//!   - `video_result_deserialize`: `VideoResult` deserializes `{ "video": { "url": ... } }`.
//!   - `video_gen_submit_body` (wiremock): `submit_with_body` POSTs the exact JSON body
//!     with `Authorization: Key <key>` and returns the parsed `SubmitResponse`.
//!   - `video_download_oversized` (wiremock): a CDN response exceeding a 1-byte cap returns
//!     `DownloadOutcome::OversizedDocument`; under the cap returns `DownloadOutcome::Video`.
//!   - `video_result_decode_failure`: an `{"images":[...]}` response fed to `fetch_video`
//!     yields a decode error (proves `fetch_video` is NOT reusing `ImageResult`).
//!
//! Plan 02 placeholder tests are stubbed with `#[ignore]` so this scaffold compiles
//! immediately and Plan 02 can fill in the bodies without merge conflicts.

use ironhermes_tools::fal::{DownloadOutcome, FalClient};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const VIDEO_MODEL: &str = "fal-ai/ltx-2.3/text-to-video";
const PARENT: &str = "fal-ai/ltx-2.3";

/// Serializes `FAL_KEY` env-var mutation across this file's tests
/// (parallel-safe). `cargo test` runs tests in parallel threads by default;
/// several tests below set/unset the process-global `FAL_KEY` env var around
/// an `execute()` call, which races when two such tests run concurrently.
/// Mirrors the `HOME_LOCK` pattern in `gen_backend.rs`'s cache-write tests.
/// An async-aware `tokio::sync::Mutex` is used (not `std::sync::Mutex`) since
/// the guard is held across `.await` points (clippy::await_holding_lock).
static FAL_KEY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn result_path(id: &str) -> String {
    format!("/{PARENT}/requests/{id}")
}

fn status_url_path(id: &str) -> String {
    format!("/{PARENT}/requests/{id}/status")
}

fn cancel_path(id: &str) -> String {
    format!("/{PARENT}/requests/{id}/cancel")
}

// =============================================================================
// Wave-0 behavioral tests (Plan 01)
// =============================================================================

/// `VideoResult` deserializes `{ "video": { "url": ... } }` correctly.
/// Proves the struct is NOT reusing `ImageResult { images: [] }` (Pitfall 2).
#[test]
fn video_result_deserialize() {
    let json_str = r#"{"video":{"url":"https://cdn/out.mp4","content_type":"video/mp4","duration":6.0},"seed":42}"#;
    let result: ironhermes_tools::fal::VideoResult =
        serde_json::from_str(json_str).expect("VideoResult must deserialize");
    assert_eq!(result.video.url, "https://cdn/out.mp4");
    assert_eq!(result.video.content_type, "video/mp4");
    assert_eq!(result.video.duration, Some(6.0));
    assert_eq!(result.seed, Some(42));
}

/// `submit_with_body` sends the exact JSON body to `/{model}` with `Authorization: Key <key>`.
#[tokio::test]
async fn video_gen_submit_body() {
    let server = MockServer::start().await;
    let request_id = "req-video-body";
    let base = server.uri();

    Mock::given(method("POST"))
        .and(path(format!("/{VIDEO_MODEL}")))
        .and(header("Authorization", "Key test-fal-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": request_id,
            "status_url": format!("{base}{}", status_url_path(request_id)),
            "response_url": format!("{base}{}", result_path(request_id)),
            "cancel_url": format!("{base}{}", cancel_path(request_id)),
        })))
        .mount(&server)
        .await;

    let client = FalClient::with_base_url(server.uri());
    let body = json!({ "prompt": "x", "duration": 6 });
    let submit = client
        .submit_with_body("test-fal-key", VIDEO_MODEL, body)
        .await
        .expect("submit_with_body must succeed");

    assert_eq!(submit.request_id, request_id);

    // Verify the exact body was POSTed by inspecting the received request.
    let reqs = server.received_requests().await.unwrap_or_default();
    assert_eq!(reqs.len(), 1, "exactly one submit POST must be made");
    let req_body: serde_json::Value =
        serde_json::from_slice(&reqs[0].body).expect("request body must be valid JSON");
    assert_eq!(req_body["prompt"], "x");
    assert_eq!(req_body["duration"], 6);
}

/// A CDN response exceeding the cap returns `OversizedDocument`; under the cap returns `Video`.
#[tokio::test]
async fn video_download_oversized() {
    let server = MockServer::start().await;

    // Under-cap body: 32 bytes.
    let small_body = vec![0u8; 32];
    Mock::given(method("GET"))
        .and(path("/cdn/small.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(small_body))
        .mount(&server)
        .await;

    // Over-cap body: 64 bytes — exceeds the 1-byte cap used in this test.
    let large_body = vec![0u8; 64];
    Mock::given(method("GET"))
        .and(path("/cdn/big.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(large_body))
        .mount(&server)
        .await;

    // allow_loopback_cdn bypasses the SSRF gate so the wiremock server (loopback) acts as CDN.
    let client = FalClient::with_base_url_allow_loopback(server.uri());

    // Over the 1-byte cap → OversizedDocument.
    let big_url = format!("{}/cdn/big.mp4", server.uri());
    let outcome = client
        .download_video_to_cache_with_cap(&big_url, "video/mp4", 1)
        .await
        .expect("download must succeed even when oversized");
    match outcome {
        DownloadOutcome::OversizedDocument(p) => {
            assert!(p.is_absolute(), "oversized path must be absolute");
            assert!(p.exists(), "oversized file must still be written to cache");
            assert!(
                p.to_string_lossy().contains("videos"),
                "path must be under cache/videos/, got: {}",
                p.display()
            );
            let _ = std::fs::remove_file(&p);
        }
        other => panic!("expected OversizedDocument for big file, got: {other:?}"),
    }

    // Under the 32-byte cap → Video.
    let small_url = format!("{}/cdn/small.mp4", server.uri());
    let outcome = client
        .download_video_to_cache_with_cap(&small_url, "video/mp4", 32)
        .await
        .expect("download must succeed");
    match outcome {
        DownloadOutcome::Video(p) => {
            assert!(p.is_absolute(), "video path must be absolute");
            assert!(p.exists(), "video file must be written to cache");
            assert!(
                p.to_string_lossy().contains("videos"),
                "path must be under cache/videos/, got: {}",
                p.display()
            );
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            assert_eq!(ext, "mp4", "video/mp4 content-type must yield .mp4 file");
            let _ = std::fs::remove_file(&p);
        }
        other => panic!("expected Video for small file, got: {other:?}"),
    }
}

/// Feeding an `{"images":[...]}` response to `fetch_video` yields a decode error.
/// Proves `fetch_video` is NOT reusing `ImageResult` (Pitfall 2 / plan must_haves).
#[tokio::test]
async fn video_result_decode_failure() {
    let server = MockServer::start().await;
    let request_id = "req-image-not-video";
    let base = server.uri();

    // Submit returns URLs so same_origin_url check passes.
    Mock::given(method("POST"))
        .and(path(format!("/{VIDEO_MODEL}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": request_id,
            "status_url": format!("{base}{}", status_url_path(request_id)),
            "response_url": format!("{base}{}", result_path(request_id)),
            "cancel_url": format!("{base}{}", cancel_path(request_id)),
        })))
        .mount(&server)
        .await;

    // The result endpoint returns an image-style response (no "video" key).
    Mock::given(method("GET"))
        .and(path(result_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "images": [{ "url": "https://cdn/img.png", "width": 512, "height": 512, "content_type": "image/png" }],
            "seed": 1
        })))
        .mount(&server)
        .await;

    let client = FalClient::with_base_url(server.uri());
    let submit = client
        .submit_with_body("test-key", VIDEO_MODEL, json!({"prompt": "test"}))
        .await
        .expect("submit ok");
    let err = client
        .fetch_video("test-key", &submit)
        .await
        .expect_err("fetch_video must fail when response has no 'video' key");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("decode") || msg.contains("missing") || msg.contains("video"),
        "error must mention decode failure, got: {msg}"
    );
}

// =============================================================================
// Plan 02 behavioral tests
// =============================================================================

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ironhermes_core::{Config, SessionKey, types::Platform};
use ironhermes_tools::Tool;
use ironhermes_tools::video_gen::{VideoAnimateTool, VideoGenerateTool, VideoSessionCounter};

fn make_session_key() -> SessionKey {
    SessionKey::new(Platform::Local, "test-session")
}

fn make_counter() -> VideoSessionCounter {
    Arc::new(Mutex::new(HashMap::new()))
}

fn make_config() -> Arc<Config> {
    // Phase 47 Plan 06: execute() now resolves the effective t2v/i2v model
    // from `video_gen.{t2v,i2v}.model` (D-01/D-03) instead of the legacy flat
    // `default_t2v_model`/`default_i2v_model` keys. `Config::default()` built
    // directly (not via YAML) does NOT run the legacy-key reconciliation
    // shadow, so t2v/i2v default to the venice models. Pin both modes to fal
    // explicitly so these wiremock-based tests keep exercising "existing
    // fal-routed behavior is preserved unchanged" (D-02/GEN-01 back-compat).
    let mut cfg = Config::default();
    cfg.video_gen.t2v.provider = Some("fal".to_string());
    cfg.video_gen.t2v.model = "fal-ai/ltx-2.3/text-to-video".to_string();
    cfg.video_gen.i2v.provider = Some("fal".to_string());
    cfg.video_gen.i2v.model = "fal-ai/ltx-2.3/image-to-video".to_string();
    Arc::new(cfg)
}

/// Full T2V queue flow via wiremock: submit body includes prompt, duration,
/// resolution, aspect_ratio, fps (NOT num_inference_steps). Returns MEDIA tag.
#[tokio::test]
async fn video_gen_submit_t2v() {
    let server = MockServer::start().await;
    let request_id = "req-t2v-01";
    let base = server.uri();

    // Submit endpoint
    Mock::given(method("POST"))
        .and(path(format!("/{VIDEO_MODEL}")))
        .and(header("Authorization", "Key fake-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": request_id,
            "status_url": format!("{base}{}", status_url_path(request_id)),
            "response_url": format!("{base}{}", result_path(request_id)),
            "cancel_url": format!("{base}{}", cancel_path(request_id)),
        })))
        .mount(&server)
        .await;

    // Status endpoint → COMPLETED immediately
    Mock::given(method("GET"))
        .and(path(status_url_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" })))
        .mount(&server)
        .await;

    // Result endpoint returns VideoResult
    let video_url = format!("{base}/cdn/out.mp4");
    Mock::given(method("GET"))
        .and(path(result_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "video": {
                "url": video_url,
                "content_type": "video/mp4",
                "duration": 6.0
            },
            "seed": 1
        })))
        .mount(&server)
        .await;

    // CDN endpoint — small mp4 bytes
    Mock::given(method("GET"))
        .and(path("/cdn/out.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 64]))
        .mount(&server)
        .await;

    // allow_loopback_cdn so wiremock acts as CDN
    let client = FalClient::with_base_url_allow_loopback(server.uri());
    let tool = VideoGenerateTool::new_with_session(
        make_session_key(),
        make_config(),
        client,
        make_counter(),
        None,
    );

    let _fal_key_guard = FAL_KEY_LOCK.lock().await;
    unsafe {
        std::env::set_var("FAL_KEY", "fake-key");
    }
    let result = tool.execute(json!({ "prompt": "a cat walks" })).await;
    unsafe {
        std::env::remove_var("FAL_KEY");
    }

    let output = result.expect("T2V execute must succeed");
    assert!(
        output.contains("<MEDIA: "),
        "output must contain MEDIA tag, got: {output}"
    );
    assert!(
        output.contains(".mp4"),
        "MEDIA tag must reference an .mp4 file, got: {output}"
    );

    // Verify the submit body had the correct T2V fields (NOT num_inference_steps)
    let reqs = server.received_requests().await.unwrap_or_default();
    let submit_req = reqs
        .iter()
        .find(|r| r.method == wiremock::http::Method::POST)
        .expect("must have a POST submit");
    let body: serde_json::Value = serde_json::from_slice(&submit_req.body).unwrap();
    assert!(body.get("prompt").is_some(), "body must have prompt");
    assert!(body.get("duration").is_some(), "body must have duration");
    assert!(
        body.get("resolution").is_some(),
        "body must have resolution"
    );
    assert!(
        body.get("aspect_ratio").is_some(),
        "body must have aspect_ratio"
    );
    assert!(body.get("fps").is_some(), "body must have fps");
    assert!(
        body.get("num_inference_steps").is_none(),
        "body must NOT have num_inference_steps (image field)"
    );

    // Clean up cached file
    if let Some(path_str) = output
        .split("<MEDIA: ")
        .nth(1)
        .and_then(|s| s.strip_suffix('>'))
    {
        let _ = std::fs::remove_file(path_str.trim());
    }
}

/// I2V with a public HTTPS URL: passes is_safe_url, submits with image_url unchanged.
#[tokio::test]
async fn video_gen_submit_i2v_url() {
    let server = MockServer::start().await;
    let request_id = "req-i2v-url";
    let base = server.uri();
    let i2v_model = "fal-ai/ltx-2.3/image-to-video";

    Mock::given(method("POST"))
        .and(path(format!("/{i2v_model}")))
        .and(header("Authorization", "Key fake-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": request_id,
            "status_url": format!("{base}{}", status_url_path(request_id)),
            "response_url": format!("{base}{}", result_path(request_id)),
            "cancel_url": format!("{base}{}", cancel_path(request_id)),
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(status_url_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" })))
        .mount(&server)
        .await;

    let video_url = format!("{base}/cdn/out2.mp4");
    Mock::given(method("GET"))
        .and(path(result_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "video": { "url": video_url, "content_type": "video/mp4", "duration": 6.0 },
            "seed": 2
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/cdn/out2.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 64]))
        .mount(&server)
        .await;

    let client = FalClient::with_base_url_allow_loopback(server.uri());
    let tool = VideoAnimateTool::new_with_session(
        make_session_key(),
        make_config(),
        client,
        make_counter(),
        None,
    );

    // Use a real public HTTPS URL (is_safe_url will validate it)
    let public_image_url = "https://cdn.fal.ai/example/image.png";

    let _fal_key_guard = FAL_KEY_LOCK.lock().await;
    unsafe {
        std::env::set_var("FAL_KEY", "fake-key");
    }
    let result = tool
        .execute(json!({
            "image_url": public_image_url,
            "prompt": "pan left"
        }))
        .await;
    unsafe {
        std::env::remove_var("FAL_KEY");
    }

    let output = result.expect("I2V URL execute must succeed");
    assert!(
        output.contains("<MEDIA: "),
        "output must contain MEDIA tag, got: {output}"
    );

    // Verify the submit body passed the image_url through unchanged (as HTTPS URL, not base64)
    let reqs = server.received_requests().await.unwrap_or_default();
    let submit_req = reqs
        .iter()
        .find(|r| r.method == wiremock::http::Method::POST)
        .expect("must have submit POST");
    let body: serde_json::Value = serde_json::from_slice(&submit_req.body).unwrap();
    assert_eq!(
        body["image_url"], public_image_url,
        "HTTPS URL must pass through unchanged"
    );

    if let Some(path_str) = output
        .split("<MEDIA: ")
        .nth(1)
        .and_then(|s| s.strip_suffix('>'))
    {
        let _ = std::fs::remove_file(path_str.trim());
    }
}

/// I2V with a local cache path: reads file, base64-encodes, submits as data URI.
#[tokio::test]
async fn video_animate_i2v_local_path() {
    let server = MockServer::start().await;
    let request_id = "req-i2v-local";
    let base = server.uri();
    let i2v_model = "fal-ai/ltx-2.3/image-to-video";

    Mock::given(method("POST"))
        .and(path(format!("/{i2v_model}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": request_id,
            "status_url": format!("{base}{}", status_url_path(request_id)),
            "response_url": format!("{base}{}", result_path(request_id)),
            "cancel_url": format!("{base}{}", cancel_path(request_id)),
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(status_url_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "COMPLETED" })))
        .mount(&server)
        .await;

    let video_url = format!("{base}/cdn/out3.mp4");
    Mock::given(method("GET"))
        .and(path(result_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "video": { "url": video_url, "content_type": "video/mp4", "duration": 6.0 },
            "seed": 3
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/cdn/out3.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 64]))
        .mount(&server)
        .await;

    // Write a real temp PNG file UNDER the cache directory
    let cache_dir = ironhermes_core::constants::get_hermes_home()
        .join("cache")
        .join("images");
    std::fs::create_dir_all(&cache_dir).expect("must create cache/images dir");
    let temp_file = cache_dir.join("test-i2v-input.png");
    // Minimal 8-byte PNG-ish bytes (content doesn't matter for this test)
    std::fs::write(&temp_file, b"\x89PNG\r\n\x1a\nfake").expect("must write temp file");

    let client = FalClient::with_base_url_allow_loopback(server.uri());
    let tool = VideoAnimateTool::new_with_session(
        make_session_key(),
        make_config(),
        client,
        make_counter(),
        None,
    );

    let _fal_key_guard = FAL_KEY_LOCK.lock().await;
    unsafe {
        std::env::set_var("FAL_KEY", "fake-key");
    }
    let result = tool
        .execute(json!({
            "image_url": temp_file.to_string_lossy().to_string(),
            "prompt": "pan"
        }))
        .await;
    unsafe {
        std::env::remove_var("FAL_KEY");
    }

    let _ = std::fs::remove_file(&temp_file);

    let output = result.expect("I2V local path execute must succeed");
    assert!(output.contains("<MEDIA: "), "output must contain MEDIA tag");

    // Verify submit body had a base64 data URI, not the raw path
    let reqs = server.received_requests().await.unwrap_or_default();
    let submit_req = reqs
        .iter()
        .find(|r| r.method == wiremock::http::Method::POST)
        .expect("must have submit POST");
    let body: serde_json::Value = serde_json::from_slice(&submit_req.body).unwrap();
    let image_url_sent = body["image_url"]
        .as_str()
        .expect("image_url must be a string");
    assert!(
        image_url_sent.starts_with("data:image/png;base64,"),
        "local path must be submitted as base64 data URI, got: {image_url_sent}"
    );

    if let Some(path_str) = output
        .split("<MEDIA: ")
        .nth(1)
        .and_then(|s| s.strip_suffix('>'))
    {
        let _ = std::fs::remove_file(path_str.trim());
    }
}

/// Path traversal to /etc/passwd (and ../../etc/passwd) is rejected before any fal call.
#[tokio::test]
async fn video_animate_path_traversal_rejected() {
    let client = FalClient::with_base_url("http://127.0.0.1:1"); // never reached
    let tool = VideoAnimateTool::new_with_session(
        make_session_key(),
        make_config(),
        client,
        make_counter(),
        None,
    );

    let _fal_key_guard = FAL_KEY_LOCK.lock().await;
    unsafe {
        std::env::set_var("FAL_KEY", "fake-key");
    }

    // Absolute path outside cache root
    let err = tool
        .execute(json!({
            "image_url": "/etc/passwd",
            "prompt": "m"
        }))
        .await
        .expect_err("/etc/passwd must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("cache") || msg.to_lowercase().contains("path"),
        "error must mention cache root or path restriction, got: {msg}"
    );

    // Traversal via ../../
    let err2 = tool
        .execute(json!({
            "image_url": "../../etc/passwd",
            "prompt": "m"
        }))
        .await
        .expect_err("../../etc/passwd must be rejected");
    let msg2 = format!("{err2:#}");
    assert!(
        !msg2.is_empty(),
        "must return an error for path traversal attempt"
    );

    unsafe {
        std::env::remove_var("FAL_KEY");
    }
}

/// CR-01 / WR-01: an oversized local image file (under the cache root, so the
/// path-traversal clamp PASSES) is rejected by the configurable inline size cap
/// (`config.video_gen.max_inline_bytes`) BEFORE the file is read into memory and
/// BEFORE any fal call. Proves the memory-exhaustion DoS gate fires and that the
/// cap is honoured from config (not a hardcoded const).
#[tokio::test]
async fn video_animate_oversized_local_file_rejected_no_fal_call() {
    // A wiremock server so we can prove NO request reaches fal. If the size-gate
    // failed to fire, execute() would attempt a submit POST against this server.
    let server = MockServer::start().await;

    // Tiny inline cap so a small real file is "oversized" without writing GBs.
    let mut config = Config::default();
    config.video_gen.max_inline_bytes = 8; // 8-byte cap
    let config = Arc::new(config);

    // Write a real file UNDER the cache root (passes the path-traversal clamp)
    // that exceeds the 8-byte cap.
    let cache_dir = ironhermes_core::constants::get_hermes_home()
        .join("cache")
        .join("images");
    std::fs::create_dir_all(&cache_dir).expect("must create cache/images dir");
    let temp_file = cache_dir.join("test-oversized-i2v-input.png");
    // 64 bytes > 8-byte cap.
    std::fs::write(&temp_file, vec![0u8; 64]).expect("must write oversized temp file");

    // allow_loopback so the SSRF gate is not what blocks us (isolate the size-gate).
    let client = FalClient::with_base_url_allow_loopback(server.uri());
    let tool = VideoAnimateTool::new_with_session(
        make_session_key(),
        config,
        client,
        make_counter(),
        None,
    );

    let _fal_key_guard = FAL_KEY_LOCK.lock().await;
    unsafe {
        std::env::set_var("FAL_KEY", "fake-key");
    }
    let err = tool
        .execute(json!({
            "image_url": temp_file.to_string_lossy().to_string(),
            "prompt": "pan"
        }))
        .await
        .expect_err("oversized local file must be rejected by the inline size cap");
    unsafe {
        std::env::remove_var("FAL_KEY");
    }

    let _ = std::fs::remove_file(&temp_file);

    // Error must clearly state the file is too large (non-retried).
    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("too large"),
        "error must mention the file is too large to inline, got: {msg}"
    );

    // Critically: NO fal call may have been made (the gate fires before submit).
    let reqs = server.received_requests().await.unwrap_or_default();
    assert!(
        reqs.is_empty(),
        "size-gate must reject BEFORE any fal request; saw {} request(s)",
        reqs.len()
    );
}

/// RFC1918 / link-local I2V image_url is rejected by SSRF check before any fal call.
#[tokio::test]
async fn video_animate_ssrf_rejected() {
    let client = FalClient::with_base_url("http://127.0.0.1:1"); // never reached
    let tool = VideoAnimateTool::new_with_session(
        make_session_key(),
        make_config(),
        client,
        make_counter(),
        None,
    );

    let _fal_key_guard = FAL_KEY_LOCK.lock().await;
    unsafe {
        std::env::set_var("FAL_KEY", "fake-key");
    }
    let err = tool
        .execute(json!({
            "image_url": "http://169.254.169.254/latest/meta-data/",
            "prompt": "m"
        }))
        .await
        .expect_err("link-local image_url must be rejected by SSRF");
    unsafe {
        std::env::remove_var("FAL_KEY");
    }

    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("ssrf")
            || msg.to_lowercase().contains("public")
            || msg.to_lowercase().contains("safe"),
        "error must mention SSRF/safe URL check, got: {msg}"
    );
}

/// Session cap blocks at 5: pre-loading counter to cap=5 returns a non-retried error.
#[tokio::test]
async fn video_gen_session_cap() {
    let counter = make_counter();
    let key = make_session_key();
    // Pre-load the counter to the cap (5)
    let cap = Config::default().video_gen.session_cap;
    counter.lock().unwrap().insert(key.clone(), cap);

    let client = FalClient::with_base_url("http://127.0.0.1:1"); // never reached
    let tool = VideoGenerateTool::new_with_session(
        key.clone(),
        make_config(),
        client,
        counter.clone(),
        None,
    );

    let _fal_key_guard = FAL_KEY_LOCK.lock().await;
    unsafe {
        std::env::set_var("FAL_KEY", "fake-key");
    }
    let err = tool
        .execute(json!({ "prompt": "x" }))
        .await
        .expect_err("6th generation must be blocked by session cap");
    unsafe {
        std::env::remove_var("FAL_KEY");
    }

    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("limit") || msg.to_lowercase().contains("cap"),
        "error must mention session limit/cap, got: {msg}"
    );
    // Must be non-retried: no "retry" or "try again" in message
    assert!(
        !msg.to_lowercase().contains("retry") && !msg.to_lowercase().contains("try again"),
        "cap message must be non-retried, got: {msg}"
    );

    // At 4 (cap-1), it must still allow generation (no fal call — just check slot)
    let counter2 = make_counter();
    counter2.lock().unwrap().insert(key.clone(), cap - 1);
    let tool2 = VideoGenerateTool::new_with_session(
        key,
        make_config(),
        FalClient::with_base_url("http://127.0.0.1:1"),
        counter2,
        None,
    );
    // try_reserve_slot should succeed at cap-1
    assert!(
        tool2.try_reserve_slot().is_ok(),
        "at cap-1, slot must be available"
    );
}

/// A never-completing job + tiny timeout yields an Err containing "video rendering timed out".
#[tokio::test]
async fn video_gen_timeout_cancel() {
    let server = MockServer::start().await;
    let request_id = "req-timeout";
    let base = server.uri();

    // Submit succeeds immediately
    Mock::given(method("POST"))
        .and(path(format!("/{VIDEO_MODEL}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "request_id": request_id,
            "status_url": format!("{base}{}", status_url_path(request_id)),
            "response_url": format!("{base}{}", result_path(request_id)),
            "cancel_url": format!("{base}{}", cancel_path(request_id)),
        })))
        .mount(&server)
        .await;

    // Status always returns IN_QUEUE (never completes)
    Mock::given(method("GET"))
        .and(path(status_url_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": "IN_QUEUE" })))
        .mount(&server)
        .await;

    // Cancel endpoint — just accept
    Mock::given(method("PUT"))
        .and(path(cancel_path(request_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let client = FalClient::with_base_url(server.uri());

    // Build a tool with a very short timeout (1 second) via a custom config.
    // Phase 47 Plan 06: pin t2v to fal explicitly (see make_config() comment)
    // so this keeps hitting the mocked fal-ai/ltx-2.3/text-to-video endpoint.
    let mut config = Config::default();
    config.video_gen.timeout_secs = 1;
    config.video_gen.t2v.provider = Some("fal".to_string());
    config.video_gen.t2v.model = VIDEO_MODEL.to_string();
    let tool = VideoGenerateTool::new_with_session(
        make_session_key(),
        Arc::new(config),
        client,
        make_counter(),
        None,
    );

    let _fal_key_guard = FAL_KEY_LOCK.lock().await;
    unsafe {
        std::env::set_var("FAL_KEY", "fake-key");
    }
    let err = tool
        .execute(json!({ "prompt": "y" }))
        .await
        .expect_err("timed-out job must return an error");
    unsafe {
        std::env::remove_var("FAL_KEY");
    }

    let msg = format!("{err:#}");
    // Must lead with our video-specific message (Pitfall 7: user sees "video rendering timed out"
    // not bare "image generation timed out" from the inner poll() error).
    assert!(
        msg.to_lowercase().starts_with("video rendering timed out"),
        "timeout error must start with 'video rendering timed out' (Pitfall 7), got: {msg}"
    );
}
