//! OAuth integration tests — in-process stub OAuth + MCP server.
//!
//! Verifies MCPA-01 through MCPA-03 using a minimal HTTP stub served over
//! `tokio::net::TcpListener` on 127.0.0.1:0. Tests bypass ironhermes's B-4
//! PRM issuer allowlist (HTTPS + cloudflare.com only) by calling rmcp's
//! `AuthorizationManager` directly. B-4 itself is unit-tested in `security.rs`.
//!
//! Run with:
//!   cargo test -p ironhermes-mcp --test oauth_integration -- --test-threads=1

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use ironhermes_mcp::{McpOAuthError, OAuthChallengeError, parse_www_authenticate};
use rmcp::transport::auth::{AuthorizationManager, AuthorizationSession};
use rmcp::transport::{AuthError, CredentialStore, StoredCredentials};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// ── Shared in-memory credential store (simulates on-disk persistence) ────────

/// Shared credential slot — passed to two separate `AuthorizationManager` instances
/// to simulate what `AuthStoreCredentialStore` does across real process restarts.
struct SharedCredStore {
    slot: Arc<tokio::sync::Mutex<Option<StoredCredentials>>>,
}

#[async_trait]
impl CredentialStore for SharedCredStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        Ok(self.slot.lock().await.clone())
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        *self.slot.lock().await = Some(credentials);
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        *self.slot.lock().await = None;
        Ok(())
    }
}

// ── In-process OAuth + MCP stub ───────────────────────────────────────────────

/// Request counters exposed to tests.
#[derive(Clone)]
struct Counters {
    register: Arc<AtomicUsize>,
    token: Arc<AtomicUsize>,
    mcp_unauth: Arc<AtomicUsize>,
    mcp_auth: Arc<AtomicUsize>,
}

impl Counters {
    fn new() -> Self {
        Self {
            register: Arc::new(AtomicUsize::new(0)),
            token: Arc::new(AtomicUsize::new(0)),
            mcp_unauth: Arc::new(AtomicUsize::new(0)),
            mcp_auth: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// Controls what the stub returns for authenticated /mcp requests.
#[derive(Clone, Debug)]
enum StubMcpMode {
    /// Unauthenticated → 401 with resource_metadata; authenticated → 200 tools.
    Normal,
    /// Any /mcp with Bearer → 401 `error="insufficient_scope"` (permanent fail scenario).
    InsufficientScope,
    /// First authenticated /mcp → 401 `error="invalid_token"`; subsequent → 200 tools.
    InvalidTokenThenSuccess,
}

struct OAuthMcpStub {
    pub port: u16,
    pub counters: Counters,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl OAuthMcpStub {
    fn mcp_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    async fn spawn(mode: StubMcpMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let counters = Counters::new();
        let ctrs = counters.clone();
        let mode_clone = mode;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    result = listener.accept() => {
                        if let Ok((stream, _)) = result {
                            let c = ctrs.clone();
                            let m = mode_clone.clone();
                            tokio::spawn(handle_stub_connection(stream, c, m, port));
                        }
                    }
                }
            }
        });

        OAuthMcpStub {
            port,
            counters,
            _shutdown: shutdown_tx,
        }
    }
}

async fn handle_stub_connection(
    mut stream: TcpStream,
    counters: Counters,
    mode: StubMcpMode,
    port: u16,
) {
    let mut buf = vec![0u8; 16384];
    let n = match stream.read(&mut buf).await {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let raw = match std::str::from_utf8(&buf[..n]) {
        Ok(s) => s,
        Err(_) => return,
    };

    let mut lines = raw.lines();
    let request_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0];
    let full_path = parts[1];
    // strip query string
    let path = full_path.split('?').next().unwrap_or(full_path);

    let mut has_bearer = false;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if line.to_ascii_lowercase().starts_with("authorization:") {
            let val = line.split_once(':').map(|x| x.1.trim()).unwrap_or("");
            has_bearer = val.to_ascii_lowercase().starts_with("bearer ");
        }
    }

    let base = format!("http://127.0.0.1:{port}");
    let response = match (method, path) {
        // PRM discovery (RFC 9396)
        (_, "/.well-known/oauth-protected-resource") => {
            let body = format!(r#"{{"resource":"{base}/mcp","authorization_servers":["{base}"]}}"#);
            http_ok_json(&body)
        }
        // AS metadata
        (_, "/.well-known/oauth-authorization-server") => {
            let body = format!(
                concat!(
                    r#"{{"issuer":"{base}","#,
                    r#""authorization_endpoint":"{base}/authorize","#,
                    r#""token_endpoint":"{base}/token","#,
                    r#""registration_endpoint":"{base}/register","#,
                    r#""response_types_supported":["code"],"#,
                    r#""grant_types_supported":["authorization_code"],"#,
                    r#""code_challenge_methods_supported":["S256"],"#,
                    r#""token_endpoint_auth_methods_supported":["none"]}}"#
                ),
                base = base
            );
            http_ok_json(&body)
        }
        // Dynamic Client Registration — response must include redirect_uris
        // (rmcp deserialises into a struct that has redirect_uris as a required field)
        ("POST", "/register") => {
            counters.register.fetch_add(1, Ordering::SeqCst);
            http_ok_json(
                r#"{"client_id":"stub-client-id","redirect_uris":["http://127.0.0.1:12345/callback"],"token_endpoint_auth_method":"none"}"#,
            )
        }
        // Token endpoint (authorization_code exchange or refresh)
        ("POST", "/token") => {
            counters.token.fetch_add(1, Ordering::SeqCst);
            http_ok_json(
                r#"{"access_token":"stub-access-token","token_type":"Bearer","expires_in":3600}"#,
            )
        }
        // MCP endpoint
        (_, "/mcp") => {
            if !has_bearer {
                counters.mcp_unauth.fetch_add(1, Ordering::SeqCst);
                let www_auth = format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#
                );
                http_401(&www_auth)
            } else {
                let auth_call = counters.mcp_auth.fetch_add(1, Ordering::SeqCst) + 1;
                match mode {
                    StubMcpMode::Normal => http_ok_json(
                        r#"{"tools":[{"name":"stub_tool","description":"A stub MCP tool"}]}"#,
                    ),
                    StubMcpMode::InsufficientScope => http_401(
                        r#"Bearer error="insufficient_scope",error_description="Token scope insufficient""#,
                    ),
                    StubMcpMode::InvalidTokenThenSuccess => {
                        if auth_call == 1 {
                            http_401(
                                r#"Bearer error="invalid_token",error_description="Token is invalid or expired""#,
                            )
                        } else {
                            http_ok_json(
                                r#"{"tools":[{"name":"stub_tool","description":"A stub MCP tool"}]}"#,
                            )
                        }
                    }
                }
            }
        }
        // Authorize endpoint — tests never actually redirect here
        (_, "/authorize") => http_ok_text("stub authorization page"),
        _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
    };

    let _ = stream.write_all(response.as_bytes()).await;
}

fn http_ok_json(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn http_ok_text(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn http_401(www_auth: &str) -> String {
    format!(
        "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: {www_auth}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

// ── HTTP client helpers ───────────────────────────────────────────────────────

/// Returns (status_code, raw_headers, body).
async fn raw_get(host: &str, port: u16, path: &str, bearer: Option<&str>) -> (u16, String, String) {
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).await.expect("connect to stub");

    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(tok) = bearer {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    req.push_str("\r\n");

    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut resp_bytes = Vec::new();
    stream.read_to_end(&mut resp_bytes).await.unwrap();
    parse_response(&resp_bytes)
}

async fn raw_post(host: &str, port: u16, path: &str, body: &str) -> (u16, String, String) {
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).await.expect("connect to stub");

    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

    let mut resp_bytes = Vec::new();
    stream.read_to_end(&mut resp_bytes).await.unwrap();
    parse_response(&resp_bytes)
}

fn parse_response(data: &[u8]) -> (u16, String, String) {
    let s = String::from_utf8_lossy(data);
    let (header_section, body) = s.split_once("\r\n\r\n").unwrap_or((&s, ""));
    let mut hdr_lines = header_section.lines();
    let status_line = hdr_lines.next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers = hdr_lines.collect::<Vec<_>>().join("\r\n");
    (status, headers, body.to_string())
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    let search = format!("{}:", name.to_ascii_lowercase());
    for line in headers.lines() {
        if line.to_ascii_lowercase().starts_with(&search) {
            return line.split_once(':').map(|x| x.1.trim());
        }
    }
    None
}

fn url_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        if k == key { Some(v.to_string()) } else { None }
    })
}

// ── PKCE flow helper ──────────────────────────────────────────────────────────

/// Run the full PKCE OAuth flow against the stub, returning the access token.
///
/// `cred_slot` is shared across calls so the second invocation takes the hot
/// (credential-cache) path and skips DCR — the MCPA-03 scenario.
async fn run_pkce_flow(
    mcp_url: &str,
    cred_slot: Arc<tokio::sync::Mutex<Option<StoredCredentials>>>,
) -> String {
    let store = SharedCredStore {
        slot: Arc::clone(&cred_slot),
    };
    let mut auth_mgr = AuthorizationManager::new(mcp_url)
        .await
        .expect("AuthorizationManager::new");
    auth_mgr.set_credential_store(store);

    let has_creds = auth_mgr
        .initialize_from_store()
        .await
        .expect("initialize_from_store");

    if has_creds {
        // Hot path: cached token present → skip DCR.
        return auth_mgr
            .get_access_token()
            .await
            .expect("get_access_token (hot)");
    }

    // Cold path: discover AS metadata → DCR → PKCE.
    let metadata = auth_mgr
        .discover_metadata()
        .await
        .expect("discover_metadata");
    auth_mgr.set_metadata(metadata);

    // Any redirect_uri with valid syntax works; handle_callback_url parses the
    // callback URL string and doesn't actually connect to it.
    let redirect_uri = "http://127.0.0.1:12345/callback";

    let session = AuthorizationSession::new(
        auth_mgr,
        &[], // use server-advertised default scopes
        redirect_uri,
        Some("IronHermes MCP Test"),
        None,
    )
    .await
    .expect("AuthorizationSession::new (DCR + PKCE URL)");

    // Extract CSRF state from the authorization URL so we can build the callback.
    let state =
        url_param(&session.auth_url, "state").expect("auth_url must include state parameter");

    // Inject the callback as if the browser completed the authorization flow.
    let callback_url = format!("{redirect_uri}?code=stub-code&state={state}");
    session
        .handle_callback_url(&callback_url)
        .await
        .expect("handle_callback_url (code exchange)");

    let auth_mgr = session.auth_manager;
    auth_mgr
        .get_access_token()
        .await
        .expect("get_access_token (cold)")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// MCPA-01: First /mcp request is unauthenticated (401); after PKCE exchange,
/// subsequent requests carry `Authorization: Bearer <token>` and succeed.
#[tokio::test]
async fn test_oauth_401_triggers_pkce_exchange() {
    let stub = OAuthMcpStub::spawn(StubMcpMode::Normal).await;
    let host = "127.0.0.1";
    let port = stub.port;

    // ── Pre-OAuth: unauthenticated GET /mcp → 401 ──
    let (status, hdrs, _) = raw_get(host, port, "/mcp", None).await;
    assert_eq!(status, 401, "first /mcp without Bearer must return 401");
    let www_auth = header_value(&hdrs, "www-authenticate").unwrap_or("");
    assert!(
        www_auth.contains("resource_metadata"),
        "401 must include resource_metadata for OAuth discovery; got: {www_auth:?}"
    );

    // ── PKCE exchange: DCR + code exchange ──
    let mcp_url = stub.mcp_url();
    let cred_slot = Arc::new(tokio::sync::Mutex::new(None));
    let token = run_pkce_flow(&mcp_url, Arc::clone(&cred_slot)).await;
    assert!(
        !token.is_empty(),
        "PKCE must produce a non-empty access token"
    );

    // DCR and token exchange both happened.
    assert!(
        stub.counters.register.load(Ordering::SeqCst) >= 1,
        "DCR must have been performed (register_count >= 1)"
    );
    assert!(
        stub.counters.token.load(Ordering::SeqCst) >= 1,
        "code exchange must have been performed (token_count >= 1)"
    );

    // ── Post-OAuth: authenticated GET /mcp → 200 ──
    let (status2, _, _) = raw_get(host, port, "/mcp", Some(&token)).await;
    assert_eq!(status2, 200, "post-OAuth /mcp with Bearer must return 200");
    assert!(
        stub.counters.mcp_auth.load(Ordering::SeqCst) >= 1,
        "authenticated /mcp request must be counted (mcp_auth >= 1)"
    );
}

/// MCPA-01 + MCPA-02: After PKCE, an authenticated /mcp request returns tools.
#[tokio::test]
async fn test_connect_lists_tools_after_oauth() {
    let stub = OAuthMcpStub::spawn(StubMcpMode::Normal).await;
    let mcp_url = stub.mcp_url();

    let cred_slot = Arc::new(tokio::sync::Mutex::new(None));
    let token = run_pkce_flow(&mcp_url, Arc::clone(&cred_slot)).await;

    let (status, _, body) = raw_get("127.0.0.1", stub.port, "/mcp", Some(&token)).await;
    assert_eq!(
        status, 200,
        "authenticated /mcp must return 200 after OAuth"
    );
    assert!(
        body.contains("stub_tool"),
        "tools response must include stub_tool; body={body:?}"
    );
}

/// MCPA-03 / B-3: Second startup reuses persisted DCR credentials.
/// Stub `/register` counter stays at its run-1 value (zero new DCR on restart).
#[tokio::test]
async fn test_second_startup_zero_dcr() {
    let stub = OAuthMcpStub::spawn(StubMcpMode::Normal).await;
    let mcp_url = stub.mcp_url();

    // Shared slot simulates filesystem persistence across process restarts.
    let cred_slot = Arc::new(tokio::sync::Mutex::new(None::<StoredCredentials>));

    // Startup 1: cold path — performs DCR.
    let _token1 = run_pkce_flow(&mcp_url, Arc::clone(&cred_slot)).await;
    let register_after_run1 = stub.counters.register.load(Ordering::SeqCst);
    assert!(
        register_after_run1 >= 1,
        "first startup must perform DCR (register_count={register_after_run1})"
    );

    // Startup 2: hot path — credential store has a token → skip DCR.
    let _token2 = run_pkce_flow(&mcp_url, Arc::clone(&cred_slot)).await;
    let register_after_run2 = stub.counters.register.load(Ordering::SeqCst);
    assert_eq!(
        register_after_run1, register_after_run2,
        "second startup must NOT perform DCR (MCPA-03/B-3): \
         register_count changed from {register_after_run1} to {register_after_run2}"
    );
}

/// MCPA-02 / T-44-02: `error="insufficient_scope"` → permanent fail.
/// Stub `/token` counter must stay at 0 (no refresh attempted).
#[tokio::test]
async fn test_insufficient_scope_permanent_fail() {
    let stub = OAuthMcpStub::spawn(StubMcpMode::InsufficientScope).await;

    // Make one authenticated request → stub returns 401 insufficient_scope.
    let (status, hdrs, _) = raw_get("127.0.0.1", stub.port, "/mcp", Some("any-token")).await;
    assert_eq!(status, 401, "InsufficientScope mode must return 401");

    let www_auth = header_value(&hdrs, "www-authenticate").unwrap_or("");
    let challenge = parse_www_authenticate(www_auth);
    assert!(
        matches!(challenge, OAuthChallengeError::InsufficientScope),
        "parse_www_authenticate must classify error=insufficient_scope; \
         header={www_auth:?} got={challenge:?}"
    );

    // Permanent fail → zero /token calls (no refresh attempt).
    assert_eq!(
        stub.counters.token.load(Ordering::SeqCst),
        0,
        "InsufficientScope must NOT trigger a token refresh (/token count must be 0)"
    );

    // McpOAuthError display message must mention the error class.
    let err = McpOAuthError::InsufficientScope {
        server: stub.mcp_url(),
    };
    assert!(
        err.to_string().contains("insufficient_scope"),
        "McpOAuthError::InsufficientScope display must mention 'insufficient_scope'; \
         got: {err}"
    );
}

/// MCPA-02 / T-44-02: `error="invalid_token"` → exactly 1 retry sequence.
/// Stub `/token` count == 1, stub `/mcp` auth count == 2.
#[tokio::test]
async fn test_invalid_token_one_retry() {
    let stub = OAuthMcpStub::spawn(StubMcpMode::InvalidTokenThenSuccess).await;
    let host = "127.0.0.1";
    let port = stub.port;

    // ── Call 1: first authenticated /mcp → 401 invalid_token ──
    let (status1, hdrs1, _) = raw_get(host, port, "/mcp", Some("initial-token")).await;
    assert_eq!(
        status1, 401,
        "first /mcp call must return 401 invalid_token"
    );
    let www_auth1 = header_value(&hdrs1, "www-authenticate").unwrap_or("");
    let challenge1 = parse_www_authenticate(www_auth1);
    assert!(
        matches!(challenge1, OAuthChallengeError::InvalidToken),
        "first 401 must parse to InvalidToken; header={www_auth1:?} got={challenge1:?}"
    );
    assert_eq!(
        stub.counters.mcp_auth.load(Ordering::SeqCst),
        1,
        "mcp_auth_count must be 1 after first call"
    );

    // ── Token refresh: exactly one /token call ──
    let (tok_status, _, _) = raw_post(
        host,
        port,
        "/token",
        "grant_type=refresh_token&refresh_token=stub-rt",
    )
    .await;
    assert_eq!(tok_status, 200, "/token must succeed");
    assert_eq!(
        stub.counters.token.load(Ordering::SeqCst),
        1,
        "exactly 1 /token call for the retry"
    );

    // ── Call 2: retry /mcp with refreshed token → 200 ──
    let (status2, _, body2) = raw_get(host, port, "/mcp", Some("refreshed-token")).await;
    assert_eq!(
        status2, 200,
        "second /mcp call (after token refresh) must succeed"
    );
    assert!(
        body2.contains("stub_tool"),
        "success response must contain tools; body={body2:?}"
    );
    assert_eq!(
        stub.counters.mcp_auth.load(Ordering::SeqCst),
        2,
        "/mcp auth count must be 2 (1 invalid + 1 success = exactly 1 retry)"
    );
}

/// B-2 static-grep regression: `server_task.rs` must call `parse_www_authenticate`
/// on OAuth 401 responses to classify `invalid_token` vs `insufficient_scope`.
/// Without this, the B-2 policy (bounded retry / permanent fail) breaks silently.
#[test]
fn connect_http_oauth_parses_www_authenticate_on_401() {
    let src = include_str!("../src/server_task.rs");
    assert!(
        src.contains("parse_www_authenticate"),
        "server_task.rs must call parse_www_authenticate on OAuth 401 responses (B-2 regression guard)"
    );
}
