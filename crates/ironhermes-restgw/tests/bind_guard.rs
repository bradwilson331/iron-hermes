//! D-07 fail-closed bind guard — the four-cell predicate matrix
//! (loopback×unauthed, loopback×authed, non-loopback×unauthed,
//! non-loopback×authed). Pure function, no socket involved.
//!
//! Also covers Finding 7 (D-20): `cors_wildcard_guard_allows`, a SECOND,
//! independent fail-closed guard with the OPPOSITE polarity from
//! `bind_guard_allows` above — it refuses when a PERMISSION (a wildcard
//! cross-origin value) is PRESENT on an exposed bind, not when a
//! protection is absent. See `crates/ironhermes-restgw/src/bind_guard.rs`'s
//! doc comment on `cors_wildcard_guard_allows` for the full polarity note.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::adapter::{MessageHandler, PlatformAdapter};
use ironhermes_core::{MessageEvent, ModelRegistry};
use ironhermes_restgw::api_server::{
    ApiServerAdapter, ApiServerConfig, ApiServerHandles, run_api_server_adapter,
};
use ironhermes_restgw::bind_guard::{bind_guard_allows, cors_wildcard_guard_allows};
use tokio_util::sync::CancellationToken;

#[test]
fn loopback_ipv4_unauthed_allowed() {
    assert!(bind_guard_allows(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        false
    ));
}

#[test]
fn loopback_ipv4_authed_allowed() {
    assert!(bind_guard_allows(IpAddr::V4(Ipv4Addr::LOCALHOST), true));
}

#[test]
fn non_loopback_ipv4_unauthed_refused() {
    assert!(!bind_guard_allows(
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
        false
    ));
}

#[test]
fn non_loopback_ipv4_authed_allowed() {
    assert!(bind_guard_allows(
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
        true
    ));
}

#[test]
fn loopback_ipv6_unauthed_allowed() {
    assert!(bind_guard_allows(IpAddr::V6(Ipv6Addr::LOCALHOST), false));
}

#[test]
fn non_loopback_ipv6_unauthed_refused() {
    assert!(!bind_guard_allows(
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        false
    ));
}

#[test]
fn non_loopback_ipv6_authed_allowed() {
    assert!(bind_guard_allows(IpAddr::V6(Ipv6Addr::UNSPECIFIED), true));
}

/// D-07 falsifiable test, named exactly as PLAN.md's must_haves cites it:
/// the full four-cell matrix (loopback×unauthed, loopback×authed,
/// non-loopback×unauthed, non-loopback×authed) in one assertion group.
#[test]
fn bind_guard_allows_matrix() {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let non_loopback = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));

    assert!(bind_guard_allows(loopback, false), "loopback + unauthed");
    assert!(bind_guard_allows(loopback, true), "loopback + authed");
    assert!(
        !bind_guard_allows(non_loopback, false),
        "non-loopback + unauthed must be refused"
    );
    assert!(
        bind_guard_allows(non_loopback, true),
        "non-loopback + authed allowed"
    );
}

// ---------------------------------------------------------------------------
// Finding 7 (D-20): `cors_wildcard_guard_allows` — the six named cases.
// ---------------------------------------------------------------------------

#[test]
fn cors_wildcard_guard_loopback_v4_wildcard_allowed() {
    assert!(
        cors_wildcard_guard_allows(IpAddr::V4(Ipv4Addr::LOCALHOST), true),
        "local development with a permissive origin is the posture this deliberately permits"
    );
}

#[test]
fn cors_wildcard_guard_loopback_v6_wildcard_allowed() {
    assert!(
        cors_wildcard_guard_allows(IpAddr::V6(Ipv6Addr::LOCALHOST), true),
        "both IPv4 and IPv6 loopback spellings are treated as loopback"
    );
}

#[test]
fn cors_wildcard_guard_loopback_no_wildcard_allowed() {
    assert!(cors_wildcard_guard_allows(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        false
    ));
}

#[test]
fn cors_wildcard_guard_non_loopback_no_wildcard_allowed_today_normal_posture() {
    let non_loopback = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
    assert!(
        cors_wildcard_guard_allows(non_loopback, false),
        "this is today's normal exposed posture (no wildcard configured) and must not regress"
    );
}

#[test]
fn cors_wildcard_guard_non_loopback_wildcard_refused() {
    let non_loopback = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
    assert!(
        !cors_wildcard_guard_allows(non_loopback, true),
        "the single refused combination: a wildcard origin on a non-loopback bind"
    );
}

#[test]
fn cors_wildcard_guard_wildcard_bind_address_with_wildcard_origin_refused() {
    // A wildcard BIND ADDRESS (0.0.0.0) is itself non-loopback — the
    // exposed-deployment case.
    let wildcard_bind = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
    assert!(
        !cors_wildcard_guard_allows(wildcard_bind, true),
        "a wildcard bind address is treated as non-loopback, so this is refused too"
    );
}

/// Predicate-level half of the derivation guarantee: "configured but not
/// the wildcard" must behave identically to "not configured" — i.e. the
/// predicate itself is agnostic to field presence, only to the wildcard
/// value. The integration-level half — proving the real CALLER derives its
/// boolean input from the wildcard VALUE rather than the field's mere
/// PRESENCE — is `cors_wildcard_guard_derivation_non_wildcard_origin_is_not_refused`
/// below.
#[test]
fn cors_wildcard_guard_configured_non_wildcard_behaves_as_unconfigured() {
    let non_loopback = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
    assert!(
        cors_wildcard_guard_allows(non_loopback, false),
        "a configured, non-wildcard origin on a non-loopback bind is allowed — the guard keys \
         on the wildcard specifically, not on the field's mere presence"
    );
}

// ---------------------------------------------------------------------------
// Call-site test: a real `run_api_server_adapter` attempt with a
// non-loopback host and a wildcard origin must return an error and never
// open a socket. A predicate that is defined but never called is the exact
// failure mode this phase exists to prevent, and no amount of predicate
// testing above catches it.
// ---------------------------------------------------------------------------

struct UnusedHandler;

#[async_trait]
impl MessageHandler for UnusedHandler {
    async fn handle(
        &self,
        _event: &MessageEvent,
        _adapter: Arc<dyn PlatformAdapter>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        panic!("UnusedHandler::handle should never be called — the guard must refuse before any request is served");
    }
}

fn test_handles() -> ApiServerHandles {
    let db_path = std::env::temp_dir().join(format!(
        "ih-restgw-bindguard-test-state-{}.db",
        uuid::Uuid::new_v4()
    ));
    ApiServerHandles {
        turn_registry: Arc::new(ironhermes_core::concurrency::TurnRegistry::new()),
        state_store: Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(&db_path).expect("open test state store"),
        )),
        job_store: None,
        model_registry: Arc::new(ModelRegistry::new()),
        skill_registry: None,
        tool_registry: Arc::new(tokio::sync::RwLock::new(ironhermes_tools::ToolRegistry::new())),
        approval_gate: None,
        run_events: Arc::new(ironhermes_restgw::api_server::sse::RunEventRegistry::new()),
    }
}

/// Integration-level derivation proof: a configured, real (non-wildcard)
/// origin string on a non-loopback bind must NOT be refused by the D-20
/// guard. Drives the REAL call site (`run_api_server_adapter`), not just
/// the predicate — an implementation that derived its boolean from
/// `allowed_origin.is_some()` rather than `allowed_origin == "*"` would
/// refuse here and would still pass every predicate-level case above.
///
/// `203.0.113.7` (TEST-NET-3, RFC 5737) is not assigned to any local
/// interface, so the bind attempt that follows a passing guard is expected
/// to fail in a sandboxed test environment — this test does not assert a
/// successful bind. It asserts only that IF an error occurs, it is not the
/// D-20 wildcard refusal, which the call-site test above pins by exact
/// message content on an address that alone would have been refused.
#[tokio::test]
async fn cors_wildcard_guard_derivation_non_wildcard_origin_is_not_refused() {
    let key = format!("test-key-cors-wildcard-derivation-{}", uuid::Uuid::new_v4());
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    let config = ApiServerConfig {
        host: Some("203.0.113.7".to_string()),
        port: Some(0),
        public_opt_in: true,
        allowed_origin: Some("https://example.com".to_string()), // real, specific — NOT the wildcard
    };
    let adapter = Arc::new(
        ApiServerAdapter::new(config, test_handles()).expect("construction must succeed"),
    );
    let handler: Arc<dyn MessageHandler> = Arc::new(UnusedHandler);
    let cancel = CancellationToken::new();

    let result = run_api_server_adapter(adapter, handler, cancel).await;

    if let Err(e) = result {
        let msg = e.to_string();
        assert!(
            !msg.to_lowercase().contains("wildcard"),
            "a configured, non-wildcard origin must not trigger the D-20 wildcard refusal \
             (whatever other error occurred is unrelated to this guard): {msg}"
        );
    }
}

#[tokio::test]
async fn cors_wildcard_guard_call_site_refuses_and_does_not_open_socket() {
    let key = format!("test-key-cors-wildcard-call-site-{}", uuid::Uuid::new_v4());
    unsafe {
        std::env::set_var("IRONHERMES_API_SERVER_KEY", &key);
    }
    // `public_opt_in: true` deliberately isolates which guard fires: it
    // makes the FIRST guard (`bind_guard_allows`, D-07) pass on its own, so
    // a refusal here can only come from the SECOND guard
    // (`cors_wildcard_guard_allows`, D-20) under test.
    let config = ApiServerConfig {
        host: Some("203.0.113.7".to_string()),
        port: Some(0),
        public_opt_in: true,
        allowed_origin: Some("*".to_string()),
    };
    let adapter = Arc::new(
        ApiServerAdapter::new(config, test_handles()).expect("construction must succeed"),
    );
    let handler: Arc<dyn MessageHandler> = Arc::new(UnusedHandler);
    let cancel = CancellationToken::new();

    let result = run_api_server_adapter(adapter, handler, cancel).await;

    let err = match result {
        Ok(()) => panic!(
            "a wildcard origin on a non-loopback bind must refuse before any socket opens, but \
             run_api_server_adapter returned Ok"
        ),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("203.0.113.7"),
        "error must name the bind address: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("wildcard"),
        "error must name the wildcard origin as the refusal reason: {msg}"
    );
}
