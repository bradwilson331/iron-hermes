//! Mock-server tests for Phase 43 PKCE flow success criteria (D-03).
//!
//! Both tests are active (not ignored). They use an injectable `open_fn` closure to
//! capture the authorization URL and drive the loopback callback — no real browser
//! or live OAuth provider is involved.
//!
//! # Success criteria
//!
//! - SC#1 (`test_pkce_auth_url_contains_s256`): the authorization URL passed to
//!   `open_fn` contains `code_challenge_method=S256` — PKCE downgrade is blocked.
//! - SC#2 (`test_pkce_exchange_persists_token_no_verifier_on_disk`): after the
//!   loopback exchange a token exists under `ns` in `auth.json` AND the file
//!   contains no `verifier` / `code_verifier` field (A-3, PITFALLS §A-3).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ironhermes_core::auth::AuthStore;
use ironhermes_core::auth::pkce::{PkceFlowParams, run_pkce_flow};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ─────────────────────────────────────────────────────────────────────────────
// Helper: drive the loopback callback from within an open_fn closure.
//
// Called inside the synchronous `open_fn`, this helper:
//   1. Parses `state` and `redirect_uri` port from the auth URL query params.
//   2. Optionally includes `iss` derived from the authorization URL's origin.
//   3. Spawns a tokio task that sleeps 20 ms (enough for `listener.accept()` to
//      be ready) then POSTs a synthetic callback GET request via reqwest.
//
// The 20 ms delay is conservative — `listener.accept()` is ready as soon as
// `run_pkce_flow` yields at its first `.await`, which happens immediately after
// calling `open_fn`.
// ─────────────────────────────────────────────────────────────────────────────
fn spawn_loopback_callback(auth_url_str: &str, include_iss: bool) {
    let parsed = url::Url::parse(auth_url_str).expect("auth URL must be valid");

    let mut state_val = String::new();
    let mut redirect_port: u16 = 0;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "state" => state_val = value.into_owned(),
            "redirect_uri" => {
                let redir =
                    url::Url::parse(&value).expect("redirect_uri query param must be a valid URL");
                redirect_port = redir
                    .port()
                    .expect("redirect_uri must have an explicit port");
            }
            _ => {}
        }
    }

    // Derive the expected iss from the authorization URL's origin (RFC 9207).
    // For mock servers this is "http://127.0.0.1:PORT".
    let iss_val = if include_iss {
        Some(parsed.origin().unicode_serialization())
    } else {
        None
    };

    tokio::runtime::Handle::current().spawn(async move {
        // Small delay: let run_pkce_flow reach listener.accept().await before we connect.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Build the callback URL with properly percent-encoded query parameters.
        let mut callback_url =
            url::Url::parse(&format!("http://127.0.0.1:{redirect_port}/callback"))
                .expect("callback URL must be constructable");
        {
            let mut pairs = callback_url.query_pairs_mut();
            pairs.append_pair("code", "mock-auth-code");
            pairs.append_pair("state", &state_val);
            if let Some(ref iss) = iss_val {
                pairs.append_pair("iss", iss);
            }
        }

        // Use reqwest to send the callback — it drives the raw TCP listener in run_pkce_flow.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client must build");
        // Ignore errors: run_pkce_flow may have already closed the stream after
        // reading the request, causing a "connection reset" on this side.
        let _ = client.get(callback_url.as_str()).send().await;
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// SC#1: authorization URL must use S256 PKCE challenge (A-3)
// ─────────────────────────────────────────────────────────────────────────────

/// SC#1 — the auth URL opened in the browser must contain `code_challenge_method=S256`.
///
/// Tests the PKCE downgrade-protection invariant: even if `plain` is supported by
/// the server, ironhermes must assert S256 before opening the browser (A-3, PITFALLS §A-3).
/// Uses an injectable `open_fn` closure to capture the URL without launching a real browser.
/// The closure also drives the loopback callback so `run_pkce_flow` completes.
#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn test_pkce_auth_url_contains_s256() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");
    let store = AuthStore::open(auth_path).await.unwrap();

    // SC#1: capture the authorization URL for assertion.
    let captured_url: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let cap = Arc::clone(&captured_url);

    // Mock OAuth server — token endpoint returns a fresh access token.
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "pkce-test-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
        })))
        .mount(&mock_server)
        .await;

    let params = PkceFlowParams {
        authorization_url: format!("{}/authorize", mock_server.uri()),
        token_url: format!("{}/token", mock_server.uri()),
        client_id: "test-client-id".to_string(),
        scopes: vec!["read".to_string()],
    };

    let open_fn = |url: &str| -> anyhow::Result<()> {
        // SC#1: capture the URL for post-flow assertion.
        *cap.lock().unwrap() = url.to_string();
        // Drive the loopback callback so run_pkce_flow can complete.
        spawn_loopback_callback(url, false /* no iss */);
        Ok(())
    };

    run_pkce_flow(&store, "test_ns_s256", params, &open_fn)
        .await
        .unwrap();

    let url = captured_url.lock().unwrap().clone();
    assert!(
        !url.is_empty(),
        "open_fn must have been called with the authorization URL"
    );
    assert!(
        url.contains("code_challenge_method=S256"),
        "authorization URL must specify S256 challenge method; got: {url}"
    );
    assert!(
        !url.contains("code_challenge_method=plain"),
        "plain PKCE challenge is forbidden; got: {url}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// SC#2: token persisted to auth.json; PKCE verifier not on disk (A-3)
// ─────────────────────────────────────────────────────────────────────────────

/// SC#2 — after a successful PKCE exchange, a token exists in `auth.json` AND the
/// file contains no `verifier` or `code_verifier` field (A-3, PITFALLS §A-3).
///
/// The verifier is ephemeral — it lives only in memory for the duration of the flow
/// and must be dropped after the exchange. Writing it to disk alongside the token
/// would defeat PKCE's interception protection.
///
/// This test also exercises the RFC 9207 `iss` validation path by including a
/// correct `iss` value in the callback.
#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn test_pkce_exchange_persists_token_no_verifier_on_disk() {
    let dir = TempDir::new().unwrap();
    let auth_path = dir.path().join("auth.json");
    let store = AuthStore::open(auth_path.clone()).await.unwrap();

    // Mock OAuth server — token endpoint returns a fresh access token + refresh token.
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "pkce-persisted-access-token",
            "refresh_token": "pkce-refresh-token",
            "token_type": "Bearer",
            "expires_in": 3600,
        })))
        .mount(&mock_server)
        .await;

    let params = PkceFlowParams {
        authorization_url: format!("{}/authorize", mock_server.uri()),
        token_url: format!("{}/token", mock_server.uri()),
        client_id: "test-client-id".to_string(),
        scopes: vec!["read".to_string()],
    };

    // Drive the loopback callback — include iss so the RFC 9207 validation path is exercised.
    let open_fn = |url: &str| -> anyhow::Result<()> {
        spawn_loopback_callback(url, true /* include iss */);
        Ok(())
    };

    run_pkce_flow(&store, "test_ns_persist", params, &open_fn)
        .await
        .unwrap();

    // ── Assertion 1: token must be persisted ────────────────────────────────
    let token = store
        .get_token("test_ns_persist")
        .await
        .expect("token must be stored in auth.json after PKCE exchange");
    assert_eq!(
        token.access_token, "pkce-persisted-access-token",
        "persisted access_token must match the server response"
    );

    // ── Assertion 2: PKCE verifier must NOT be on disk (A-3) ────────────────
    let auth_json =
        std::fs::read_to_string(&auth_path).expect("auth.json must exist after put_token");
    assert!(
        !auth_json.contains("verifier"),
        "auth.json must not contain any 'verifier' field; got: {auth_json}"
    );
    assert!(
        !auth_json.contains("code_verifier"),
        "auth.json must not contain 'code_verifier'; got: {auth_json}"
    );

    // ── Assertion 3: expiry must be populated (A-6: expires_in wired correctly)
    assert!(
        token.expires_at.is_some(),
        "expires_at must be populated from expires_in in the token response"
    );
    let expires_at = token.expires_at.unwrap();
    let now = std::time::SystemTime::now();
    assert!(
        expires_at > now + Duration::from_secs(3500),
        "expires_at must be ~3600s in the future; got a time in the past or too soon"
    );
}
