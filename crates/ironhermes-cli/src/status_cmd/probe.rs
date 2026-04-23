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

/// Real probe impl (Phase 21.7 Plan 09 Task 9-03).
///
/// Each method performs a bounded, timeout-wrapped round-trip against the
/// corresponding subsystem:
/// - `provider_health`: `reqwest` HEAD against a canonical endpoint URL.
/// - `fts5_integrity`: `rusqlite` `PRAGMA integrity_check` via spawn_blocking.
/// - `mcp_server`: reports unreachable with latency=0 (ISS-11 fallback —
///   requires McpManager plumbing that 21.7 scope does not include; a
///   future phase wires rmcp `list_all_tools`).
///
/// Every method emits `tracing::info!(target="ironhermes_cli::status",
/// component, ok, latency_ms)` so FW-05 / M-05 observers can aggregate
/// deep-probe outcomes from the same log stream.
#[cfg(not(test))]
pub struct LiveDeepProbe {
    http: reqwest::Client,
}

#[cfg(not(test))]
impl LiveDeepProbe {
    pub fn new() -> Self {
        // Outer client timeout is a safety belt; the per-call `timeout`
        // parameter is the primary deadline. We set a 5s upper bound on
        // the client itself so connection-establishment doesn't hang
        // indefinitely even when the caller passes a longer timeout.
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { http }
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
        provider_name: &str,
        timeout: Duration,
    ) -> anyhow::Result<bool> {
        let url = match provider_probe_url(provider_name) {
            Some(u) => u,
            None => {
                tracing::info!(
                    target: "ironhermes_cli::status",
                    component = "provider",
                    provider = provider_name,
                    "no probe URL configured; reporting unhealthy"
                );
                return Ok(false);
            }
        };

        let start = std::time::Instant::now();
        let result = tokio::time::timeout(timeout, self.http.head(&url).send()).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        let ok = match result {
            Ok(Ok(resp)) => {
                let s = resp.status();
                // 2xx means healthy; 401/403 means the endpoint is reachable
                // (we're unauthenticated on a HEAD — that's fine for liveness).
                // 3xx redirects also indicate the endpoint is serving.
                s.is_success()
                    || s.is_redirection()
                    || s.as_u16() == 401
                    || s.as_u16() == 403
            }
            Ok(Err(_)) | Err(_) => false,
        };

        tracing::info!(
            target: "ironhermes_cli::status",
            component = "provider",
            provider = provider_name,
            ok,
            latency_ms,
            "deep probe complete"
        );
        Ok(ok)
    }

    async fn fts5_integrity(
        &self,
        db_path: &std::path::Path,
        timeout: Duration,
    ) -> anyhow::Result<bool> {
        let path_clone = db_path.to_path_buf();
        let start = std::time::Instant::now();

        // rusqlite is sync; hop to a blocking thread and bound wall clock
        // via tokio::time::timeout. `PRAGMA integrity_check` returns
        // exactly "ok" on healthy DBs — any other string means corrupt.
        let blocking = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            if !path_clone.exists() {
                // A missing state.db is "not unhealthy" — it just hasn't
                // been populated. Return false so the status line shows
                // a distinct state from a validated-ok DB; the text layer
                // surfaces "bad" for operator attention.
                return Ok(false);
            }
            let conn = rusqlite::Connection::open(&path_clone)?;
            let res: String =
                conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
            Ok(res == "ok")
        });

        let outcome = tokio::time::timeout(timeout, blocking).await;
        let latency_ms = start.elapsed().as_millis() as u64;
        let ok = match outcome {
            Ok(Ok(Ok(b))) => b,
            _ => false,
        };
        tracing::info!(
            target: "ironhermes_cli::status",
            component = "fts5",
            ok,
            latency_ms,
            "deep probe complete"
        );
        Ok(ok)
    }

    async fn mcp_server(
        &self,
        name: &str,
        _timeout: Duration,
    ) -> anyhow::Result<McpProbeResult> {
        // ISS-11 (21.7 MVP): `hermes status` does not construct an
        // McpManager for a one-shot probe. Reporting `reachable=false`
        // honestly is better than lying with `reachable=true` (FM-5 —
        // false-negative masking outage). A future phase threads the
        // McpManager handle through `status_cmd::run_status` so this
        // path can do a real `list_all_tools` round-trip.
        tracing::warn!(
            target: "ironhermes_cli::status",
            component = "mcp",
            server = name,
            ok = false,
            latency_ms = 0_u64,
            "LiveDeepProbe::mcp_server wiring deferred — reporting \
             unreachable honestly (ISS-11)"
        );
        Ok(McpProbeResult {
            name: name.into(),
            reachable: false,
            tool_count: 0,
            latency_ms: 0,
        })
    }
}

/// Provider-name → canonical probe URL. Returns `None` for providers
/// without a known health endpoint; the caller maps that to `Ok(false)`.
#[cfg(not(test))]
fn provider_probe_url(name: &str) -> Option<String> {
    match name {
        "anthropic" => Some("https://api.anthropic.com/v1/messages".into()),
        "openai" => Some("https://api.openai.com/v1/models".into()),
        "openrouter" => Some("https://openrouter.ai/api/v1/models".into()),
        _ => None,
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
