//! Phase 21.7 Plan 09 Task 9-03 — end-to-end deep-probe scenarios
//! (S-14 / S-15 / S-16) driven through `StatusReport::collect`.
//!
//! We build a local test-only probe (`FaultyProbe`) that implements the
//! public `DeepProbe` trait exposed by the lib. `MockDeepProbe` is
//! `#[cfg(test)]`-gated inside the lib crate's own test scope and is NOT
//! reachable from an integration binary, so we re-create the small subset
//! of behaviour the scenarios need right here. This also keeps
//! `status_cmd_probe_seam.rs` independent — that file drives probe.rs
//! directly via `#[path = ...]`.

use async_trait::async_trait;
use ironhermes_cli::status_cmd::probe::{DeepProbe, McpProbeResult};
use ironhermes_cli::status_cmd::{StatusArgs, StatusReport};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// Fault-injection probe implementing the public `DeepProbe` trait.
/// Each override is wrapped in a `Mutex<Option<...>>` so we can move the
/// `Err` variant out exactly once when consumed.
#[derive(Default)]
struct FaultyProbe {
    provider: Mutex<Option<anyhow::Result<bool>>>,
    fts5: Mutex<Option<anyhow::Result<bool>>>,
    mcp: Mutex<HashMap<String, anyhow::Result<McpProbeResult>>>,
}

impl FaultyProbe {
    fn new() -> Self {
        Self::default()
    }
    fn with_provider(self, v: anyhow::Result<bool>) -> Self {
        *self.provider.lock().unwrap() = Some(v);
        self
    }
    fn with_fts5(self, v: anyhow::Result<bool>) -> Self {
        *self.fts5.lock().unwrap() = Some(v);
        self
    }
    fn with_mcp(self, name: &str, v: anyhow::Result<McpProbeResult>) -> Self {
        self.mcp.lock().unwrap().insert(name.into(), v);
        self
    }
}

#[async_trait]
impl DeepProbe for FaultyProbe {
    async fn provider_health(&self, _name: &str, _timeout: Duration) -> anyhow::Result<bool> {
        match self.provider.lock().unwrap().take() {
            Some(Ok(b)) => Ok(b),
            Some(Err(e)) => Err(anyhow::anyhow!("{}", e)),
            None => Ok(true),
        }
    }

    async fn fts5_integrity(
        &self,
        _db_path: &Path,
        _timeout: Duration,
    ) -> anyhow::Result<bool> {
        match self.fts5.lock().unwrap().take() {
            Some(Ok(b)) => Ok(b),
            Some(Err(e)) => Err(anyhow::anyhow!("{}", e)),
            None => Ok(true),
        }
    }

    async fn mcp_server(
        &self,
        name: &str,
        _timeout: Duration,
    ) -> anyhow::Result<McpProbeResult> {
        match self.mcp.lock().unwrap().remove(name) {
            Some(Ok(r)) => Ok(r),
            Some(Err(e)) => Err(anyhow::anyhow!("{}", e)),
            None => Ok(McpProbeResult {
                name: name.into(),
                reachable: true,
                tool_count: 0,
                latency_ms: 0,
            }),
        }
    }
}

fn baseline_args(deep: bool, all: bool) -> StatusArgs {
    StatusArgs {
        all,
        deep,
        json: false,
    }
}

#[tokio::test]
async fn s14_provider_500_reflected_in_report() {
    let probe = FaultyProbe::new().with_provider(Ok(false));
    let config = ironhermes_core::Config::default();
    let tmp = tempfile::tempdir().unwrap();
    let args = baseline_args(true, false);
    let snap = StatusReport::collect(&config, tmp.path(), &args, &probe)
        .await
        .unwrap();
    assert_eq!(
        snap.provider.healthy,
        Some(false),
        "S-14: provider.healthy must reflect injected Ok(false)"
    );
}

#[tokio::test]
async fn s14_provider_err_surfaces_as_unhealthy_not_panic() {
    let probe = FaultyProbe::new()
        .with_provider(Err(anyhow::anyhow!("connection refused")));
    let config = ironhermes_core::Config::default();
    let tmp = tempfile::tempdir().unwrap();
    let args = baseline_args(true, false);
    let snap = StatusReport::collect(&config, tmp.path(), &args, &probe)
        .await
        .unwrap();
    assert_eq!(
        snap.provider.healthy,
        Some(false),
        "probe Err must map to Some(false) in the collector (caller-tolerant)"
    );
}

#[tokio::test]
async fn s15_fts5_corrupt_reflected_in_report() {
    let probe = FaultyProbe::new().with_fts5(Ok(false));
    let config = ironhermes_core::Config::default();
    let tmp = tempfile::tempdir().unwrap();
    let args = baseline_args(true, false);
    let snap = StatusReport::collect(&config, tmp.path(), &args, &probe)
        .await
        .unwrap();
    assert_eq!(
        snap.memory.state_db_healthy,
        Some(false),
        "S-15: memory.state_db_healthy must reflect injected Ok(false)"
    );
}

#[tokio::test]
async fn s16_mcp_unreachable_reflected_in_report() {
    let mut config = ironhermes_core::Config::default();
    config
        .mcp_servers
        .insert("fs".into(), serde_yaml::Value::Null);

    let probe = FaultyProbe::new().with_mcp(
        "fs",
        Ok(McpProbeResult {
            name: "fs".into(),
            reachable: false,
            tool_count: 0,
            latency_ms: 0,
        }),
    );

    let tmp = tempfile::tempdir().unwrap();
    let args = baseline_args(true, true);
    let snap = StatusReport::collect(&config, tmp.path(), &args, &probe)
        .await
        .unwrap();

    let per = snap
        .mcp
        .per_server
        .as_ref()
        .expect("--all|--deep must populate mcp.per_server");
    assert_eq!(per.len(), 1);
    assert_eq!(per[0].name, "fs");
    assert_eq!(
        per[0].reachable,
        Some(false),
        "S-16: per_server[0].reachable must reflect injected reachable=false"
    );
}

#[tokio::test]
async fn without_deep_flag_no_deep_fields_populated() {
    let probe = FaultyProbe::new()
        .with_provider(Ok(false))
        .with_fts5(Ok(false));
    let config = ironhermes_core::Config::default();
    let tmp = tempfile::tempdir().unwrap();
    let args = baseline_args(false, false);
    let snap = StatusReport::collect(&config, tmp.path(), &args, &probe)
        .await
        .unwrap();
    assert_eq!(snap.provider.healthy, None);
    assert_eq!(snap.memory.state_db_healthy, None);
    assert!(snap.mcp.per_server.is_none());
}
