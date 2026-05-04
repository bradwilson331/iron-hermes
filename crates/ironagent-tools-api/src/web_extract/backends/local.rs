//! Phase 25.2 D-04 / D-18: local backend — reqwest GET + web_local::extract_content_local.
//!
//! Mirrors web_read.rs::fetch_local (lines 124-173) with two enhancements for D-03 mid-fetch reroute:
//! 1. Returns the response Content-Type so the dispatcher can re-route to PDF when needed.
//! 2. Returns the raw bytes alongside the extracted Markdown so the PDF handler (Plan 09) can
//!    re-use them without a second GET.

use anyhow::{Result, anyhow};
use ironhermes_core::config::WebConfig;
use std::time::Duration;
use tracing::debug;

use crate::web_extract::ExtractionResult;
use crate::web_local::{extract_content_local, validate_url_async};

/// Bundle of everything the dispatcher needs to decide whether to render this as HTML or
/// re-route to PDF mid-fetch. Returned even on success — the dispatcher reads `content_type`
/// to call `dispatch::reroute_for_pdf(&content_type)` and may re-process `raw_bytes` via
/// `pdf::extract_pdf_bytes(...)`.
#[derive(Debug)]
pub struct LocalFetchOutcome {
    pub result: ExtractionResult,
    pub content_type: Option<String>,
    pub raw_bytes: Option<Vec<u8>>,
}

/// Fetch the URL via reqwest, validate post-redirect for SSRF, then convert HTML→Markdown
/// via the shared web_local helpers. Returns `LocalFetchOutcome` so the dispatcher can
/// mid-fetch reroute to PDF on `application/pdf` Content-Type.
pub async fn fetch_local_content(url: &str, web_cfg: &WebConfig) -> Result<LocalFetchOutcome> {
    // D-18: SSRF pre-validation
    validate_url_async(url).await?;

    debug!("web_extract: fetching {} via local backend", url);

    let client = reqwest::Client::builder()
        .user_agent(&web_cfg.user_agent)
        .timeout(Duration::from_secs(web_cfg.timeout_secs))
        // Default redirect policy follows redirects — re-validated below.
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("local fetch failed: {}", e))?;

    // D-18 post-redirect re-validation (web_read.rs:142-150 verbatim pattern)
    let final_url = response.url().as_str().to_string();
    if final_url != url {
        validate_url_async(&final_url)
            .await
            .map_err(|_| anyhow!("URL blocked by security policy (private IP)"))?;
    }

    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("local fetch returned HTTP {}", status));
    }

    // Capture Content-Type before consuming body.
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Read body as bytes so we can either decode as HTML or hand to PDF handler.
    let bytes = response
        .bytes()
        .await
        .map_err(|e| anyhow!("local fetch body read failed: {}", e))?
        .to_vec();

    // If Content-Type is application/pdf, return the raw bytes WITHOUT attempting HTML extraction
    // — the dispatcher will call dispatch::reroute_for_pdf and route to pdf::extract_pdf_bytes.
    let is_pdf = content_type
        .as_deref()
        .map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/pdf")
        })
        .unwrap_or(false);

    if is_pdf {
        return Ok(LocalFetchOutcome {
            result: ExtractionResult {
                url: final_url,
                title: String::new(),
                content: String::new(),
                error: None, // dispatcher will populate via PDF handler
            },
            content_type,
            raw_bytes: Some(bytes),
        });
    }

    // HTML path: decode bytes as UTF-8 (lossy if needed) and run extract_content_local.
    let html = String::from_utf8_lossy(&bytes).into_owned();
    let markdown = extract_content_local(&html, &final_url)
        .map_err(|e| anyhow!("local HTML→Markdown extraction failed: {}", e))?;

    // Title: extract_content_local prepends `# {title}` header — surface the bare title
    // so the dispatcher can use it directly without re-parsing the Markdown.
    Ok(LocalFetchOutcome {
        result: ExtractionResult {
            url: final_url,
            title: extract_h1_or_empty(&markdown),
            content: markdown,
            error: None,
        },
        content_type,
        raw_bytes: Some(bytes),
    })
}

/// Helper: pull the first `# Heading` line as the title; empty string otherwise.
fn extract_h1_or_empty(markdown: &str) -> String {
    markdown
        .lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l.trim_start_matches("# ").trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_h1_pulls_title_from_markdown() {
        let md = "# Hello\nSource: https://x\n\nBody";
        assert_eq!(extract_h1_or_empty(md), "Hello");
    }

    #[test]
    fn extract_h1_returns_empty_when_no_header() {
        assert_eq!(extract_h1_or_empty("Just body text"), "");
    }

    // Real fetch behavior is exercised by Plan 14 wiremock integration tests
    // (web_extract_single_url_local_fallback_returns_markdown).
}
