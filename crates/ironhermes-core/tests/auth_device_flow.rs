//! Wave-0 RED scaffolds for Phase 43 device-code flow success criteria.
//!
//! Plan 04 un-ignores these tests and wires up the real implementation.
//!
//! # Success criteria covered
//!
//! - SC#3 (`test_headless_autoselect_prints_url_and_code`): on a simulated headless
//!   environment the device flow surfaces the verification URL + user code and begins
//!   polling, completing with a stored token.
//! - SC#4 (`test_three_slow_down_raises_interval_to_20s`): three consecutive
//!   `slow_down` responses drive the poll interval from 5s to 20s cumulatively
//!   (5 + 5 = 10, 10 + 5 = 15, 15 + 5 = 20; A-5, PITFALLS §A-5).
//!
//! # SC#4 observer hook
//!
//! `test_three_slow_down_raises_interval_to_20s` uses the `poll_interval_observer`
//! seam added to `DeviceFlowParams` by Plan 04. The observer is called with the
//! effective sleep duration before each poll attempt **instead** of sleeping, so the
//! test is deterministic and instant — no real timers.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ironhermes_core::auth::AuthStore;
use ironhermes_core::auth::device::{DeviceFlowParams, run_device_flow};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ─────────────────────────────────────────────────────────────────────────────
// SC#3: headless device flow surfaces verification URL + user code, then polls
// ─────────────────────────────────────────────────────────────────────────────

/// SC#3 — on a headless environment the device flow must surface the verification
/// URL and user code for the operator, then poll until authorized.
///
/// "Surfaces" means the information is printed to stdout before the poll loop
/// starts. The test asserts that the flow completes successfully and persists
/// the token.
///
/// The mock token endpoint returns an immediate success on the first poll, so
/// the only real sleep is the server-supplied `interval` (1 s). The test uses
/// `poll_interval_observer: None` (production path) to exercise the real
/// `tokio::time::sleep` code path.
#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn test_headless_autoselect_prints_url_and_code() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");
    let store = AuthStore::open(auth_path).await.unwrap();

    let mock_server = MockServer::start().await;

    // Device authorization endpoint — returns device_code, user_code, verification_uri.
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "device-code-sc3-test",
            "user_code": "ABCD-1234",
            "verification_uri": "https://example.com/activate",
            "verification_uri_complete": "https://example.com/activate?user_code=ABCD-1234",
            "expires_in": 300,
            "interval": 1,
        })))
        .mount(&mock_server)
        .await;

    // Token endpoint — immediate success on first poll.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "device-flow-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
        })))
        .mount(&mock_server)
        .await;

    let params = DeviceFlowParams {
        device_authorization_url: format!("{}/device/code", mock_server.uri()),
        token_url: format!("{}/token", mock_server.uri()),
        client_id: "test-client-id".to_string(),
        scopes: vec!["read".to_string()],
        // Production path: real sleep used (interval=1s from mock, acceptable in test).
        poll_interval_observer: None,
    };

    // SC#3: must complete successfully, printing verification URL + user_code before
    // entering the poll loop.
    run_device_flow(&store, "test_ns_headless", params)
        .await
        .unwrap();

    // Token must be persisted after successful authorization.
    assert!(
        store.get_token("test_ns_headless").await.is_some(),
        "token must be stored after device flow authorization"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// SC#4: three consecutive slow_down responses raise poll interval to 20s (A-5)
// ─────────────────────────────────────────────────────────────────────────────

/// SC#4 — three consecutive `slow_down` error responses must cumulatively raise
/// the poll interval from 5s to 20s: 5 + 5 = 10, 10 + 5 = 15, 15 + 5 = 20.
///
/// RFC 8628 §3.5: on `slow_down`, the client MUST increase the polling interval
/// by at least 5 seconds. This test verifies the cumulative +5s rule (A-5,
/// PITFALLS §A-5).
///
/// The `poll_interval_observer` seam is used so that no real wall-clock time
/// is spent — the observer captures the interval values instead of sleeping,
/// making the test instant and deterministic.
///
/// Expected observer sequence: [5s, 10s, 15s, 20s]
/// (initial interval, then after each of the three slow_down responses).
#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn test_three_slow_down_raises_interval_to_20s() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");
    let store = AuthStore::open(auth_path).await.unwrap();

    let mock_server = MockServer::start().await;

    // Device authorization endpoint.
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "device-code-sc4-test",
            "user_code": "SLOW-9999",
            "verification_uri": "https://example.com/activate",
            "expires_in": 300,
            "interval": 5,
        })))
        .mount(&mock_server)
        .await;

    // Token endpoint — slow_down × 3, then success.
    // OAuth2 slow_down error response (RFC 8628 §3.5 error format).
    let slow_down_body = serde_json::json!({
        "error": "slow_down",
        "error_description": "Polling too frequently",
    });
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(slow_down_body.clone()))
        .up_to_n_times(3)
        .mount(&mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "slow-down-test-token",
            "token_type": "Bearer",
            "expires_in": 3600,
        })))
        .mount(&mock_server)
        .await;

    // Observer: captures the poll interval before each poll without sleeping.
    let observed_intervals: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));
    let observer_clone = Arc::clone(&observed_intervals);

    let params = DeviceFlowParams {
        device_authorization_url: format!("{}/device/code", mock_server.uri()),
        token_url: format!("{}/token", mock_server.uri()),
        client_id: "test-client-id".to_string(),
        scopes: vec![],
        poll_interval_observer: Some(Arc::new(move |d| {
            observer_clone.lock().unwrap().push(d);
        })),
    };

    run_device_flow(&store, "test_ns_slow_down", params)
        .await
        .unwrap();

    assert!(
        store.get_token("test_ns_slow_down").await.is_some(),
        "token must be stored after slow_down backoff + eventual success"
    );

    // SC#4 assertions: cumulative +5s per slow_down (A-5, never reset).
    let intervals = observed_intervals.lock().unwrap();
    assert_eq!(
        intervals.len(),
        4,
        "expected 4 observer calls: initial + 3 slow_downs before success"
    );
    assert_eq!(
        intervals[0],
        Duration::from_secs(5),
        "initial interval must be 5s (server-supplied)"
    );
    assert_eq!(
        intervals[1],
        Duration::from_secs(10),
        "after 1st slow_down: interval must be 10s (+5s cumulative)"
    );
    assert_eq!(
        intervals[2],
        Duration::from_secs(15),
        "after 2nd slow_down: interval must be 15s (+5s cumulative)"
    );
    assert_eq!(
        intervals[3],
        Duration::from_secs(20),
        "after 3rd slow_down: interval must be 20s (+5s cumulative)"
    );
}
