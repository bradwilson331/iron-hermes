//! E-06 / S-14 / S-15 / S-16 — DeepProbe fault-injection tests.
//! Exercises MockDeepProbe outcomes end-to-end to prove the trait seam works.
//!
//! Uses `#[path = ...]` to compile probe.rs directly into this integration
//! test crate so `#[cfg(test)]`-gated types (MockDeepProbe) are visible.

#[path = "../src/status_cmd/probe.rs"]
mod probe;

use probe::*;
use std::time::Duration;

#[tokio::test]
async fn provider_probe_reports_unhealthy_on_mocked_500() {
    // S-14: inject provider-HEAD-500 -> assert provider.healthy=false.
    let mock = MockDeepProbe::new().set_provider(Ok(false));
    let healthy: bool = mock
        .provider_health("anthropic", Duration::from_millis(100))
        .await
        .unwrap();
    assert!(
        !healthy,
        "S-14: MockDeepProbe must propagate Ok(false) as healthy=false"
    );
}

#[tokio::test]
async fn provider_probe_surfaces_error_not_ok_false() {
    // Error path: probe error -> caller should distinguish from Ok(false).
    let mock = MockDeepProbe::new().set_provider(Err(anyhow::anyhow!("connection refused")));
    let result = mock
        .provider_health("anthropic", Duration::from_millis(100))
        .await;
    assert!(
        result.is_err(),
        "E-06: error path must surface as Err, not swallowed as Ok(false)"
    );
}

#[tokio::test]
async fn fts5_probe_reports_corrupt() {
    // S-15: inject FTS5 non-'ok' -> state_db_healthy=false.
    let mock = MockDeepProbe::new().set_fts5(Ok(false));
    let healthy = mock
        .fts5_integrity(
            std::path::Path::new("/nonexistent"),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
    assert!(
        !healthy,
        "S-15: FTS5 integrity_check returns non-ok -> healthy=false"
    );
}

#[tokio::test]
async fn mcp_probe_reports_timeout_unreachable() {
    // S-16: inject MCP timeout -> reachable=false.
    let mock = MockDeepProbe::new().set_mcp(
        "mem-fs",
        Ok(McpProbeResult {
            name: "mem-fs".into(),
            reachable: false,
            tool_count: 0,
            latency_ms: 0,
        }),
    );
    let r = mock
        .mcp_server("mem-fs", Duration::from_millis(100))
        .await
        .unwrap();
    assert!(
        !r.reachable,
        "S-16: timed-out MCP server -> reachable=false"
    );
    assert_eq!(r.tool_count, 0);
}

#[tokio::test]
async fn mcp_probe_unset_server_returns_default_healthy() {
    let mock = MockDeepProbe::new();
    let r = mock
        .mcp_server("unknown", Duration::from_millis(100))
        .await
        .unwrap();
    assert!(r.reachable);
}

#[tokio::test]
async fn noop_probe_always_returns_healthy() {
    let p = NoopDeepProbe;
    assert!(
        p.provider_health("x", Duration::from_millis(10))
            .await
            .unwrap()
    );
    assert!(
        p.fts5_integrity(std::path::Path::new("/x"), Duration::from_millis(10))
            .await
            .unwrap()
    );
    assert!(
        p.mcp_server("x", Duration::from_millis(10))
            .await
            .unwrap()
            .reachable
    );
}
