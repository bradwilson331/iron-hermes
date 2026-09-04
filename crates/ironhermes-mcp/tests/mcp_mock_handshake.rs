//! Mock MCP server handshake tests — proves headers reach the wire (D-08).
//!
//! Verifies D-01/D-02: `McpServerConfig.headers`/`auth` must reach a real,
//! listening HTTP server, not just survive construction. A static source-grep
//! (see `transport.rs`'s `connect_http_uses_custom_headers`) proves construction
//! shape only; this file is the transmission proof the incident's post-mortem
//! (D-08) requires — asserting on `wiremock::MockServer::received_requests()`,
//! the receiving end, rather than on the `StreamableHttpClientTransportConfig`
//! that was built.
//!
//! The handshake response shape (`initialize` -> `mcp-session-id` response header)
//! is modelled on `atomicmail_bridge.py:32-64`'s known-good trace (D-09) — read
//! for its shape only; no credential value from that file appears here. Every
//! token in this file is the synthetic placeholder `expected-token` (or a sibling
//! placeholder for precedence tests), never a real key.
//!
//! Run with:
//!   cargo test -p ironhermes-mcp --test mcp_mock_handshake -- --test-threads=1

use ironhermes_mcp::McpServerConfig;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build the mock's `initialize` response body, mirroring the shape
/// `atomicmail_bridge.py:32-64` observed from the live Atomic Mail server.
fn initialize_response_body() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "serverInfo": { "name": "mock", "version": "0.1" }
        }
    })
}

#[tokio::test]
async fn header_from_config_reaches_the_wire() {
    let mock_server = MockServer::start().await;

    // Registration order matters — wiremock matches in registration order.
    // First: the exact expected Authorization header succeeds.
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("authorization", "Bearer expected-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("mcp-session-id", "test-session-123")
                .set_body_json(initialize_response_body()),
        )
        .mount(&mock_server)
        .await;
    // Catch-all: anything else (wrong/missing Authorization) 401s.
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&mock_server)
        .await;

    let config = McpServerConfig {
        url: Some(format!("{}/mcp", mock_server.uri())),
        headers: [("Authorization".to_string(), "Bearer expected-token".to_string())].into(),
        ..Default::default()
    };

    // The connect result is not the load-bearing assertion here — a static mock
    // may not satisfy the full rmcp handshake state machine. Tolerate either
    // outcome with an explanatory message; the received-request assertion below
    // is what proves transmission.
    let connect_result = ironhermes_mcp::transport::connect_http(&config).await;
    let _ = connect_result; // outcome intentionally not asserted — see comment above

    let received = mock_server.received_requests().await.expect(
        "wiremock must have request-recording enabled by default via MockServer::start()",
    );
    let carried_header = received.iter().any(|req| {
        req.headers
            .get("authorization")
            .map(|v| v == "Bearer expected-token")
            .unwrap_or(false)
    });
    assert!(
        carried_header,
        "D-01: a request carrying the exact configured Authorization header must reach \
         the mock server. Recorded requests: {received:#?}"
    );
}

#[tokio::test]
async fn absent_authorization_header_is_absent_on_the_wire_and_connect_fails() {
    let mock_server = MockServer::start().await;

    // No Authorization-matching mock is registered at all — every request 401s.
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&mock_server)
        .await;

    let config = McpServerConfig {
        url: Some(format!("{}/mcp", mock_server.uri())),
        headers: Default::default(),
        auth: None,
        ..Default::default()
    };

    let connect_result = ironhermes_mcp::transport::connect_http(&config).await;
    assert!(
        connect_result.is_err(),
        "connect_http must return Err when the mock server 401s every request \
         (no Authorization header configured)"
    );

    let received = mock_server
        .received_requests()
        .await
        .expect("wiremock request recording must be enabled");
    let carried_any_authorization = received
        .iter()
        .any(|req| req.headers.get("authorization").is_some());
    assert!(
        !carried_any_authorization,
        "D-01: with no configured headers and no auth, no recorded request may carry an \
         authorization header. Recorded requests: {received:#?}"
    );
}

/// Mount a mock that 200s ONLY for `expected_bearer_value` on `authorization` and
/// 401s everything else, then run `connect_http` and return the recorded requests.
async fn run_against_bearer_gated_mock(
    config_builder: impl FnOnce(String) -> McpServerConfig,
    expected_bearer_value: &str,
) -> Vec<wiremock::Request> {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("authorization", expected_bearer_value))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("mcp-session-id", "test-session-123")
                .set_body_json(initialize_response_body()),
        )
        .mount(&mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&mock_server)
        .await;

    let config = config_builder(format!("{}/mcp", mock_server.uri()));
    let _ = ironhermes_mcp::transport::connect_http(&config).await;

    mock_server
        .received_requests()
        .await
        .expect("wiremock request recording must be enabled")
}

/// D-02: an explicit `headers.Authorization` entry always wins over the `auth:`
/// shorthand — never both. Exactly one `authorization` header value must reach
/// the wire, and it must be the explicit one.
#[tokio::test]
async fn explicit_authorization_header_wins_over_auth_shorthand() {
    let received = run_against_bearer_gated_mock(
        |url| McpServerConfig {
            url: Some(url),
            auth: Some("shorthand-token".to_string()),
            headers: [(
                "Authorization".to_string(),
                "Bearer explicit-token".to_string(),
            )]
            .into(),
            ..Default::default()
        },
        "Bearer explicit-token",
    )
    .await;

    let values: Vec<_> = received
        .iter()
        .flat_map(|req| req.headers.get_all("authorization").iter().cloned())
        .collect();
    assert_eq!(
        values.len(),
        1,
        "D-02: exactly one authorization header value must reach the wire when both \
         auth and an explicit Authorization header are configured. Got: {values:?}"
    );
    assert_eq!(
        values[0], "Bearer explicit-token",
        "D-02: the explicit header must win over the auth: shorthand"
    );
}

/// D-02: `auth: "expected-token"` alone sends exactly one `Bearer expected-token`
/// header.
#[tokio::test]
async fn auth_shorthand_alone_sends_one_bearer_header() {
    let received = run_against_bearer_gated_mock(
        |url| McpServerConfig {
            url: Some(url),
            auth: Some("expected-token".to_string()),
            ..Default::default()
        },
        "Bearer expected-token",
    )
    .await;

    let values: Vec<_> = received
        .iter()
        .flat_map(|req| req.headers.get_all("authorization").iter().cloned())
        .collect();
    assert_eq!(
        values.len(),
        1,
        "D-02: auth: alone must send exactly one authorization header. Got: {values:?}"
    );
    assert_eq!(values[0], "Bearer expected-token");
}

/// D-02: `auth: "Bearer expected-token"` (operator already included the prefix)
/// must NOT be doubled into `Bearer Bearer expected-token`.
#[tokio::test]
async fn auth_shorthand_does_not_double_the_bearer_prefix() {
    let received = run_against_bearer_gated_mock(
        |url| McpServerConfig {
            url: Some(url),
            auth: Some("Bearer expected-token".to_string()),
            ..Default::default()
        },
        "Bearer expected-token",
    )
    .await;

    let values: Vec<_> = received
        .iter()
        .flat_map(|req| req.headers.get_all("authorization").iter().cloned())
        .collect();
    assert_eq!(
        values.len(),
        1,
        "D-02: auth: with an already-present Bearer prefix must still send exactly \
         one authorization header. Got: {values:?}"
    );
    assert_eq!(
        values[0], "Bearer expected-token",
        "D-02: the Bearer prefix must not be doubled"
    );
}

/// T-48.3-04: a config header whose key matches an rmcp-reserved header name is
/// rejected by name, before any network call.
#[tokio::test]
async fn reserved_header_key_is_rejected_by_name() {
    let config = McpServerConfig {
        url: Some("http://127.0.0.1:1/mcp".to_string()),
        headers: [("Mcp-Session-Id".to_string(), "x".to_string())].into(),
        ..Default::default()
    };

    let result = ironhermes_mcp::transport::connect_http(&config).await;
    let err = result.expect_err(
        "connect_http must reject a reserved header key (Mcp-Session-Id) before any network call",
    );
    let message = format!("{err:#}");
    assert!(
        message.contains("Mcp-Session-Id"),
        "T-48.3-04: the error must name the offending header key. Got: {message}"
    );
}
