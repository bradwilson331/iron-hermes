//! Phase 25.2 D-04 / D-06: Tavily backend — hand-rolled reqwest, no SDK.
//!
//! Endpoint: POST https://api.tavily.com/extract
//! Auth: `Authorization: Bearer <TAVILY_API_KEY>`
//! Body: `{"urls": ["{url}"], "format": "markdown", "extract_depth": "basic"}`
//!
//! Tavily does not return a `title` field; we derive title from the URL's last path segment.

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use serde::Deserialize;
use serde_json::json;
use tracing::debug;

use crate::web_extract::ExtractionResult;
use crate::web_local::validate_url_async;

const TAVILY_ENDPOINT: &str = "https://api.tavily.com/extract";

#[derive(Debug, Deserialize)]
struct TavilyExtractResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
    #[serde(default)]
    failed_results: Vec<TavilyFailedResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    url: String,
    #[serde(default)]
    raw_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TavilyFailedResult {
    #[serde(default)]
    #[allow(dead_code)]
    // Reserved for future per-URL failure surfacing (mirrors exa status pattern).
    url: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Fetch a URL via the Tavily `/extract` API with `format=markdown`, `extract_depth=basic`.
/// Returns `Err` on backend failure so the dispatcher (Plan 13) can fall through
/// to Local per D-04. Per D-18, runs SSRF pre-validation first.
pub async fn fetch_with_tavily(url: &str) -> Result<ExtractionResult> {
    // D-18: SSRF pre-validation BEFORE any network construction.
    validate_url_async(url).await?;

    let api_key = std::env::var("TAVILY_API_KEY").map_err(|_| anyhow!("TAVILY_API_KEY not set"))?;

    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);
    debug!("web_extract: fetching {} via Tavily", url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .post(resolve_endpoint())
        .bearer_auth(&api_key)
        .json(&json!({
            "urls": [url],
            "format": "markdown",
            "extract_depth": "basic"
        }))
        .send()
        .await
        .map_err(|e| anyhow!("Tavily request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("Tavily returned HTTP {}", status));
    }

    let body: TavilyExtractResponse = response
        .json()
        .await
        .map_err(|e| anyhow!("Tavily response decode failed: {}", e))?;

    if let Some(failed) = body.failed_results.into_iter().next() {
        let err_msg = failed
            .error
            .unwrap_or_else(|| "Tavily reported failure".into());
        return Err(anyhow!("Tavily: {}", err_msg));
    }

    let result = body
        .results
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Tavily: empty results"))?;

    let raw_content = result.raw_content.unwrap_or_default();
    if raw_content.is_empty() {
        return Err(anyhow!("Tavily returned no raw_content"));
    }

    // Tavily provides no title — derive from URL path last segment.
    let title = derive_title_from_url(&result.url);

    // D-07 Option B: inline Markdown header (matches web_read.rs:159 + firecrawl.rs/exa.rs precedent).
    let header = if title.is_empty() {
        format!("Source: {}\n\n", result.url)
    } else {
        format!("# {}\nSource: {}\n\n", title, result.url)
    };

    Ok(ExtractionResult {
        url: result.url,
        title,
        content: format!("{header}{raw_content}"),
        error: None,
    })
}

/// Test-override hook for the endpoint — production always returns the literal Tavily URL.
/// Tests in Plan 14 set the `TAVILY_ENDPOINT_OVERRIDE` env var to a wiremock base URL.
/// Mirrors the `resolve_endpoint()` helper in firecrawl.rs and exa.rs (Plan 06/07).
fn resolve_endpoint() -> String {
    std::env::var("TAVILY_ENDPOINT_OVERRIDE").unwrap_or_else(|_| TAVILY_ENDPOINT.to_string())
}

/// Strip protocol + host, take the last non-empty path segment, strip common
/// file extensions (`.pdf`, `.html`). Used as a title fallback when the upstream
/// provider does not return a title (Tavily today; possibly PDF backend later).
fn derive_title_from_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => {
            let segments: Vec<&str> = parsed
                .path_segments()
                .map(|s| s.collect())
                .unwrap_or_default();
            segments
                .iter()
                .rev()
                .find(|s| !s.is_empty())
                .map(|s| {
                    s.trim_end_matches(".pdf")
                        .trim_end_matches(".html")
                        .to_string()
                })
                .unwrap_or_default()
        }
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_constant_when_override_unset() {
        let ep = resolve_endpoint();
        assert!(ep.starts_with("http"), "endpoint = {ep}");
    }

    #[test]
    fn derive_title_from_url_strips_pdf_suffix() {
        assert_eq!(
            derive_title_from_url("https://arxiv.org/abs/2401.12345.pdf"),
            "2401.12345"
        );
    }

    #[test]
    fn derive_title_from_url_takes_last_segment() {
        assert_eq!(
            derive_title_from_url("https://example.com/articles/my-post"),
            "my-post"
        );
        assert_eq!(
            derive_title_from_url("https://example.com/articles/my-post.html"),
            "my-post"
        );
    }

    #[test]
    fn derive_title_from_url_empty_path() {
        assert_eq!(derive_title_from_url("https://example.com/"), "");
        assert_eq!(derive_title_from_url("https://example.com"), "");
    }

    #[test]
    fn derive_title_from_url_invalid() {
        assert_eq!(derive_title_from_url("not a url"), "");
    }

    // Real backend behavior is exercised by Plan 14 wiremock integration tests
    // (see web_extract_integration.rs `tavily_path` test that sets TAVILY_API_KEY +
    // TAVILY_ENDPOINT_OVERRIDE and confirms the bearer header + JSON body shape).
}
