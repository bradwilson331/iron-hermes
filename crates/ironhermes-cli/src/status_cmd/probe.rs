//! Deep-probe trait seam for `hermes status --deep` (D-21, E-06).
//!
//! `trait DeepProbe` gates fault injection via #[cfg(test)].
//! Production builds use `LiveDeepProbe` (real HTTP/DB/rmcp roundtrips).
//! Tests use `MockDeepProbe` with pre-programmed fault outcomes (S-14/S-15/S-16).
//!
//! Threat-model anchors:
//! - T-21.7-04-01 (false-negative masking outage): MockDeepProbe scenarios in
//!   tests/status_cmd_probe_seam.rs prove each probe-class reports unhealthy
//!   under the matching fault. Plan 09 closes the live half.
//! - T-21.7-04-02 (DoS via hung probe): every method accepts a `timeout`
//!   Duration; implementers MUST wrap their work in `tokio::time::timeout`.
//! - T-21.7-04-05 (test-only mock leaks into release): MockDeepProbe is
//!   `#[cfg(test)]` so it is compiled out of the release binary.

#![allow(dead_code)] // Plan 09 consumes LiveDeepProbe; NoopDeepProbe is the
// default when --deep is unset.

use async_trait::async_trait;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct McpProbeResult {
    pub name: String,
    pub reachable: bool,
    pub tool_count: usize,
    pub latency_ms: u64,
}

#[async_trait]
pub trait DeepProbe: Send + Sync {
    /// Probe provider health (HEAD / lightweight ping).
    /// MUST enforce the given timeout; NEVER hang.
    async fn provider_health(
        &self,
        provider_name: &str,
        timeout: Duration,
    ) -> anyhow::Result<bool>;

    /// Probe state.db FTS5 integrity via PRAGMA integrity_check.
    async fn fts5_integrity(
        &self,
        db_path: &std::path::Path,
        timeout: Duration,
    ) -> anyhow::Result<bool>;

    /// Probe MCP server via rmcp tools/list roundtrip.
    async fn mcp_server(
        &self,
        name: &str,
        timeout: Duration,
    ) -> anyhow::Result<McpProbeResult>;
}

/// No-op probe returned when `--deep` is NOT set.
/// All three methods return quickly without side effects.
pub struct NoopDeepProbe;

#[async_trait]
impl DeepProbe for NoopDeepProbe {
    async fn provider_health(&self, _: &str, _: Duration) -> anyhow::Result<bool> {
        Ok(true)
    }
    async fn fts5_integrity(
        &self,
        _: &std::path::Path,
        _: Duration,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
    async fn mcp_server(
        &self,
        name: &str,
        _: Duration,
    ) -> anyhow::Result<McpProbeResult> {
        Ok(McpProbeResult {
            name: name.into(),
            reachable: true,
            tool_count: 0,
            latency_ms: 0,
        })
    }
}

/// Real probe impl — placeholder body. Plan 09 fills each method.
///
/// Each method returns the literal string
/// `LiveDeepProbe::<method> not yet implemented — Plan 09`
/// so Plan 09's executor can `grep -rn "LiveDeepProbe::" crates/ironhermes-cli`
/// to locate the three bail sites.
#[cfg(not(test))]
pub struct LiveDeepProbe;

#[cfg(not(test))]
impl LiveDeepProbe {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(test))]
impl Default for LiveDeepProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(test))]
#[async_trait]
impl DeepProbe for LiveDeepProbe {
    async fn provider_health(
        &self,
        _provider_name: &str,
        _timeout: Duration,
    ) -> anyhow::Result<bool> {
        anyhow::bail!("LiveDeepProbe::provider_health not yet implemented — Plan 09")
    }
    async fn fts5_integrity(
        &self,
        _db_path: &std::path::Path,
        _timeout: Duration,
    ) -> anyhow::Result<bool> {
        anyhow::bail!("LiveDeepProbe::fts5_integrity not yet implemented — Plan 09")
    }
    async fn mcp_server(
        &self,
        _name: &str,
        _timeout: Duration,
    ) -> anyhow::Result<McpProbeResult> {
        anyhow::bail!("LiveDeepProbe::mcp_server not yet implemented — Plan 09")
    }
}

// ------------------------------------------------------------------
// Test-only mock — pre-programmable outcomes for S-14/S-15/S-16.
// ------------------------------------------------------------------

#[cfg(test)]
#[derive(Default)]
pub struct MockDeepProbe {
    pub provider_override: Option<anyhow::Result<bool>>,
    pub fts5_override: Option<anyhow::Result<bool>>,
    pub mcp_overrides: std::collections::HashMap<String, anyhow::Result<McpProbeResult>>,
}

#[cfg(test)]
impl MockDeepProbe {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_provider(mut self, v: anyhow::Result<bool>) -> Self {
        self.provider_override = Some(v);
        self
    }
    pub fn set_fts5(mut self, v: anyhow::Result<bool>) -> Self {
        self.fts5_override = Some(v);
        self
    }
    pub fn set_mcp(
        mut self,
        name: impl Into<String>,
        v: anyhow::Result<McpProbeResult>,
    ) -> Self {
        self.mcp_overrides.insert(name.into(), v);
        self
    }
}

#[cfg(test)]
#[async_trait]
impl DeepProbe for MockDeepProbe {
    async fn provider_health(&self, _: &str, _: Duration) -> anyhow::Result<bool> {
        match &self.provider_override {
            Some(Ok(b)) => Ok(*b),
            Some(Err(e)) => Err(anyhow::anyhow!("{}", e)),
            None => Ok(true),
        }
    }
    async fn fts5_integrity(
        &self,
        _: &std::path::Path,
        _: Duration,
    ) -> anyhow::Result<bool> {
        match &self.fts5_override {
            Some(Ok(b)) => Ok(*b),
            Some(Err(e)) => Err(anyhow::anyhow!("{}", e)),
            None => Ok(true),
        }
    }
    async fn mcp_server(
        &self,
        name: &str,
        _: Duration,
    ) -> anyhow::Result<McpProbeResult> {
        match self.mcp_overrides.get(name) {
            Some(Ok(r)) => Ok(r.clone()),
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
