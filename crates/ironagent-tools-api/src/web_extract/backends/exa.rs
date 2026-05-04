//! Phase 25.2 D-04 / D-06: Exa backend — hand-rolled reqwest, no SDK.
//!
//! Endpoint: POST https://api.exa.ai/contents
//! Auth: `x-api-key: <EXA_API_KEY>` header (NOT Bearer; per Exa API docs verified 2026-05-01)
//! Body: `{"ids": ["{url}"], "text": true}`
//!
//! Exa's `text` field is plain text (not Markdown). Per RESEARCH.md Open Question §6
//! we wrap with a `# {title}\nSource: {url}\n\n` header to keep D-07 Markdown contract.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::web_extract::ExtractionResult;
use crate::web_local::validate_url_async;

const EXA_ENDPOINT: &str = "https://api.exa.ai/contents";

#[derive(Debug, Deserialize)]
struct ExaContentsResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
    #[serde(default)]
    statuses: Vec<ExaStatus>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
    url: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExaStatus {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    // Reserved for future per-URL status surfacing (mirrors firecrawl status_code).
    status: Option<String>,
}

/// Fetch a URL via the Exa `/contents` API with `text=true`.
/// Returns `Err` on backend failure so the dispatcher (Plan 13) can fall through
/// to Tavily → Local per D-04. Per D-18, runs SSRF pre-validation first.
pub async fn fetch_with_exa(url: &str) -> Result<ExtractionResult> {
    // D-18: SSRF pre-validation BEFORE any network construction.
    validate_url_async(url).await?;

    let api_key = std::env::var("EXA_API_KEY").map_err(|_| anyhow!("EXA_API_KEY not set"))?;

    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_extract: fetching {} via Exa", url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .post(resolve_endpoint())
        .header("x-api-key", &api_key)
        .json(&json!({ "ids": [url], "text": true }))
        .send()
        .await
        .map_err(|e| anyhow!("Exa request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Exa returned HTTP {}", status));
    }

    let body: ExaContentsResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Exa response decode failed: {}", e))?;

    // Surface explicit per-URL errors from the statuses array.
    if let Some(err) = body.statuses.iter().find_map(|s| s.error.as_deref()) {
        return Err(anyhow!("Exa: {}", err));
    }

    let result = body
        .results
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Exa: empty results"))?;

    let text = result.text.unwrap_or_default();
    if text.is_empty() {
        return Err(anyhow!("Exa returned no text"));
    }

    let title = result.title.unwrap_or_default();
    // D-07 Option B: inline Markdown header (matches web_read.rs:159 + RESEARCH.md Open Q §6).
    let header = if title.is_empty() {
        format!("Source: {}\n\n", result.url)
    } else {
        format!("# {}\nSource: {}\n\n", title, result.url)
    };

    Ok(ExtractionResult {
        url: result.url,
        title,
        content: format!("{header}{text}"),
        error: None,
    })
}

/// Test-override hook for the endpoint — production always returns the literal Exa URL.
/// Tests in Plan 14 set the `EXA_ENDPOINT_OVERRIDE` env var to a wiremock base URL.
/// Mirrors the `resolve_endpoint()` helper in firecrawl.rs (Plan 06).
fn resolve_endpoint() -> String {
    std::env::var("EXA_ENDPOINT_OVERRIDE").unwrap_or_else(|_| EXA_ENDPOINT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_constant_when_override_unset() {
        // Don't mutate env in unit tests — just confirm the helper returns SOMETHING reasonable.
        let ep = resolve_endpoint();
        assert!(ep.starts_with("http"), "endpoint = {ep}");
    }

    // Real backend behavior is exercised by Plan 14 wiremock integration tests
    // (see web_extract_integration.rs `exa_path` test that sets EXA_API_KEY +
    // EXA_ENDPOINT_OVERRIDE and confirms the x-api-key header + JSON body shape).
}
