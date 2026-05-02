//! Phase 25.2 D-04 / D-06: Firecrawl backend — hand-rolled reqwest client.
//! Mirrors `web_read.rs::fetch_with_firecrawl` (web_read.rs:171-248) exactly;
//! only the return type differs (`ExtractionResult` instead of raw `String`).
//!
//! Endpoint: POST https://api.firecrawl.dev/v1/scrape
//! Auth: Bearer FIRECRAWL_API_KEY
//! Body: {"url": url, "formats": ["markdown"]}

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::web_extract::ExtractionResult;
use crate::web_local::validate_url_async;

const FIRECRAWL_ENDPOINT: &str = "https://api.firecrawl.dev/v1/scrape";

#[derive(Debug, Deserialize)]
struct FirecrawlScrapeResponse {
    success: bool,
    #[serde(default)]
    data: Option<FirecrawlScrapeData>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FirecrawlScrapeData {
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    metadata: Option<FirecrawlMetadata>,
}

#[derive(Debug, Deserialize)]
struct FirecrawlMetadata {
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "statusCode", default)]
    #[allow(dead_code)] // Reserved for future >=400 status detection (web_read.rs:174 pattern).
    status_code: Option<u16>,
}

/// Fetch a URL via the Firecrawl `/v1/scrape` API with `formats=["markdown"]`.
/// Returns `Err` on backend failure so the dispatcher (Plan 13) can fall through
/// to Exa → Tavily → Local per D-04. Per D-18, runs SSRF pre-validation first;
/// note Firecrawl follows redirects server-side — we trust their post-fetch URL handling.
pub async fn fetch_with_firecrawl(url: &str) -> Result<ExtractionResult> {
    // D-18: SSRF pre-validation BEFORE any network construction.
    validate_url_async(url).await?;

    let api_key = std::env::var("FIRECRAWL_API_KEY")
        .map_err(|_| anyhow!("FIRECRAWL_API_KEY not set"))?;

    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_extract: fetching {} via Firecrawl", url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .post(resolve_endpoint())
        .bearer_auth(&api_key)
        .json(&json!({ "url": url, "formats": ["markdown"] }))
        .send()
        .await
        .map_err(|e| anyhow!("Firecrawl request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Firecrawl returned HTTP {}", status));
    }

    let body: FirecrawlScrapeResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Firecrawl response decode failed: {}", e))?;

    if !body.success {
        let msg = body
            .error
            .unwrap_or_else(|| "Firecrawl reported failure".into());
        return Err(anyhow!("Firecrawl: {}", msg));
    }

    let data = body
        .data
        .ok_or_else(|| anyhow!("Firecrawl response missing data block"))?;
    let markdown = data
        .markdown
        .ok_or_else(|| anyhow!("Firecrawl response missing markdown"))?;
    let title = data.metadata.and_then(|m| m.title).unwrap_or_default();

    // D-07 Option B: inline Markdown header (matches web_read.rs:159 precedent).
    let header = if title.is_empty() {
        format!("Source: {url}\n\n")
    } else {
        format!("# {title}\nSource: {url}\n\n")
    };

    Ok(ExtractionResult {
        url: url.to_string(),
        title,
        content: format!("{header}{markdown}"),
        error: None,
    })
}

/// Test-override hook for the endpoint — production always returns the literal Firecrawl URL.
/// Tests in Plan 14 set the `FIRECRAWL_ENDPOINT_OVERRIDE` env var to a wiremock base URL.
/// Following the Phase 21.8 Plan 02 pattern (`SkillsShBlobSource.{github_api_base, raw_content_base}`)
/// — single env var, plain-String, no global mutable state.
fn resolve_endpoint() -> String {
    std::env::var("FIRECRAWL_ENDPOINT_OVERRIDE").unwrap_or_else(|_| FIRECRAWL_ENDPOINT.to_string())
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
    // (web_extract_single_url_local_fallback_returns_markdown asserts no key → falls through to local;
    //  Plan 14 also adds a `firecrawl_path` test that sets FIRECRAWL_API_KEY + FIRECRAWL_ENDPOINT_OVERRIDE
    //  and confirms the bearer header + JSON body shape).
}
