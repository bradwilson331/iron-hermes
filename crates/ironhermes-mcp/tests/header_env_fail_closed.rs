//! 54.8-02 Task 2 (D-18, T-54.8-06): `${VAR}` in MCP headers fails closed.
//!
//! An UNSET variable must fail the connect before any network call, with an error that
//! names the field and the variable but never a value. A SET variable reaches the wire
//! expanded. All values are the fake `test-value-548`.
//!
//! Run with:
//!   cargo test -p ironhermes-mcp --test header_env_fail_closed -- --test-threads=1

use std::time::Duration;

use ironhermes_mcp::McpServerConfig;
use ironhermes_mcp::transport::connect_http;
use tokio::sync::Mutex;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serializes tests that mutate process env (edition 2024 makes set_var unsafe).
/// An async-aware mutex: the guard is held across awaits.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

fn config_with_header(url: String, name: &str, value: &str) -> McpServerConfig {
    let mut cfg = McpServerConfig {
        url: Some(url),
        ..McpServerConfig::default()
    };
    cfg.headers.insert(name.to_string(), value.to_string());
    cfg
}

#[tokio::test]
async fn connect_http_refuses_unresolved_header_before_network() {
    let _guard = ENV_LOCK.lock().await;
    // SAFETY: serialized by ENV_LOCK; no other test thread reads this name.
    unsafe { std::env::remove_var("IH_TEST_548_UNSET") };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let cfg = config_with_header(
        format!("{}/mcp", server.uri()),
        "Authorization",
        "Bearer ${IH_TEST_548_UNSET}",
    );
    let err = match connect_http(&cfg).await {
        Ok(_) => panic!("an unresolved placeholder must fail the connect"),
        Err(e) => format!("{e:#}"),
    };

    assert!(
        err.contains("IH_TEST_548_UNSET"),
        "must name the variable: {err}"
    );
    assert!(err.contains("Authorization"), "must name the header: {err}");
    let received = server.received_requests().await.unwrap_or_default();
    assert!(
        received.is_empty(),
        "no request may be sent for an unresolved placeholder, saw {}",
        received.len()
    );
}

#[tokio::test]
async fn header_env_var_reaches_the_wire() {
    let _guard = ENV_LOCK.lock().await;
    // SAFETY: serialized by ENV_LOCK; no other test thread reads this name.
    unsafe { std::env::set_var("IH_TEST_548_HDR", "test-value-548") };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let mut cfg = config_with_header(
        format!("{}/mcp", server.uri()),
        "Authorization",
        "Bearer ${IH_TEST_548_HDR}",
    );
    ironhermes_mcp::interpolate_config(&mut cfg);

    // The handshake is expected to fail (401); only what reached the wire matters.
    let _ = tokio::time::timeout(Duration::from_secs(15), connect_http(&cfg)).await;

    let received = server.received_requests().await.unwrap_or_default();
    assert!(
        received.iter().any(
            |r| r.headers.get("authorization").and_then(|v| v.to_str().ok())
                == Some("Bearer test-value-548")
        ),
        "expected an Authorization header carrying the expanded value; saw {} request(s)",
        received.len()
    );

    // SAFETY: serialized by ENV_LOCK.
    unsafe { std::env::remove_var("IH_TEST_548_HDR") };
}
