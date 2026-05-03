//! Phase 25.2 D-09: PDF handler.
//!
//! Two entry points:
//! - `extract_pdf(url)` — full pipeline: Firecrawl primary + pdf-extract local fallback.
//! - `extract_pdf_bytes(url, bytes)` — mid-fetch reroute (Plan 13 calls this when local backend
//!   detects `Content-Type: application/pdf` and already has the bytes).
//!
//! Safety guards (RESEARCH.md threat T5 + Pitfall 1):
//! - 50 MB byte-size cap BEFORE extraction (`PDF_MAX_BYTES`).
//! - `tokio::task::spawn_blocking` so the synchronous pdf-extract parser doesn't stall the runtime.
//! - 30s `tokio::time::timeout` wraps the spawn_blocking — runaway extractions surface as Err.
//! - Pre-fetch SSRF + post-redirect re-validation when fetching bytes directly (D-18).

use std::time::Duration;

use anyhow::{Result, anyhow};
use ironhermes_core::config::Config;
use tracing::{debug, warn};

use crate::web_extract::ExtractionResult;
use crate::web_extract::backends::firecrawl::fetch_with_firecrawl;
use crate::web_local::validate_url_async;

const PDF_MAX_BYTES: usize = 50 * 1024 * 1024; // 50 MB
const PDF_EXTRACT_TIMEOUT_SECS: u64 = 30;

/// D-09 entry point. Tries Firecrawl primary if `FIRECRAWL_API_KEY` is set, then falls back
/// to local byte-fetch + `pdf_extract::extract_text_from_mem`.
pub async fn extract_pdf(url: &str) -> Result<ExtractionResult> {
    // D-18: SSRF pre-validation
    validate_url_async(url).await?;

    // 1. Firecrawl primary: it can handle PDFs natively when given the URL with formats=["markdown"]
    if std::env::var("FIRECRAWL_API_KEY").is_ok() {
        match fetch_with_firecrawl(url).await {
            Ok(mut r) => {
                // If Firecrawl returned content, use it; else fall through to local
                if !r.content.is_empty() {
                    if r.title.is_empty() {
                        r.title = filename_title(url);
                    }
                    return Ok(r);
                }
                debug!(
                    "Firecrawl returned empty content for PDF {}, falling back to pdf-extract",
                    url
                );
            }
            Err(e) => {
                // Plan 25.2-16 (UAT Issue 9): redact secret-bearing URL fields before
                // they hit tracing log sinks. cfg.extract.redact_url_patterns is not in
                // scope here (extract_pdf takes only `url: &str`); the const
                // SECRET_URL_PATTERNS list still fires via &[]. Threading operator
                // extras through pdf.rs is a future ≤5-LOC refactor (out of Plan 16 scope).
                let url_for_log =
                    crate::web_extract::sanitize::redact_secrets_in_url(url, &[]);
                warn!(
                    "Firecrawl failed for PDF {}: {}; falling back to pdf-extract",
                    url_for_log, e
                );
            }
        }
    }

    // 2. Local fallback: fetch bytes, then pdf_extract
    let bytes = fetch_pdf_bytes(url).await?;
    extract_pdf_bytes(url, bytes).await
}

/// D-03 mid-fetch reroute entry. Called by Plan 13 when Plan 08's local backend
/// returns `LocalFetchOutcome { content_type: "application/pdf", raw_bytes: Some(_), .. }`.
/// Skips the GET because the bytes are already in hand.
pub async fn extract_pdf_bytes(url: &str, bytes: Vec<u8>) -> Result<ExtractionResult> {
    // RESEARCH.md threat T5: enforce byte-size cap
    if bytes.len() > PDF_MAX_BYTES {
        return Err(anyhow!(
            "pdf_too_large: {} bytes exceeds {} MB cap",
            bytes.len(),
            PDF_MAX_BYTES / (1024 * 1024)
        ));
    }

    // RESEARCH.md Pitfall 1: pdf_extract is synchronous CPU-bound; wrap in spawn_blocking.
    // RESEARCH.md threat T5: 30s outer timeout.
    let extract_fut =
        tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem(&bytes));

    let text = match tokio::time::timeout(
        Duration::from_secs(PDF_EXTRACT_TIMEOUT_SECS),
        extract_fut,
    )
    .await
    {
        Ok(Ok(Ok(text))) => text,
        Ok(Ok(Err(e))) => return Err(anyhow!("pdf_text_extraction_failed: {}", e)),
        Ok(Err(join_err)) => return Err(anyhow!("pdf extract task panicked: {}", join_err)),
        Err(_) => {
            return Err(anyhow!(
                "pdf_text_extraction_timeout: exceeded {}s",
                PDF_EXTRACT_TIMEOUT_SECS
            ));
        }
    };

    let title = filename_title(url);
    let header = if title.is_empty() {
        format!("Source: {}\n\n", url)
    } else {
        format!("# {}\nSource: {}\n\n", title, url)
    };

    Ok(ExtractionResult {
        url: url.to_string(),
        title,
        content: format!("{header}{text}"),
        error: None,
    })
}

/// Fetch PDF bytes via reqwest with SSRF + post-redirect re-validation (D-18).
async fn fetch_pdf_bytes(url: &str) -> Result<Vec<u8>> {
    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("PDF fetch failed: {}", e))?;

    // D-18 post-redirect re-validation
    let final_url = response.url().as_str().to_string();
    if final_url != url {
        validate_url_async(&final_url).await.map_err(|_| {
            anyhow!("URL blocked by security policy (private IP) after redirect")
        })?;
    }

    if !response.status().is_success() {
        return Err(anyhow!("PDF fetch returned HTTP {}", response.status()));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| anyhow!("PDF body read failed: {}", e))?
        .to_vec();

    if bytes.len() > PDF_MAX_BYTES {
        return Err(anyhow!(
            "pdf_too_large: {} bytes exceeds {} MB cap",
            bytes.len(),
            PDF_MAX_BYTES / (1024 * 1024)
        ));
    }

    Ok(bytes)
}

/// Derive a title from the URL: take the last path segment, strip `.pdf` (case-insensitive).
fn filename_title(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => parsed
            .path_segments()
            .and_then(|mut s| s.next_back().map(|x| x.to_string()))
            .map(|s| {
                // Case-insensitive .pdf strip
                let l = s.to_ascii_lowercase();
                if l.ends_with(".pdf") {
                    s[..s.len() - 4].to_string()
                } else {
                    s
                }
            })
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_title_strips_pdf_extension_case_insensitive() {
        assert_eq!(
            filename_title("https://arxiv.org/abs/2401.12345.pdf"),
            "2401.12345"
        );
        assert_eq!(filename_title("https://example.com/doc.PDF"), "doc");
        assert_eq!(
            filename_title("https://example.com/multi/path/file.pdf"),
            "file"
        );
    }

    #[test]
    fn filename_title_empty_for_no_path() {
        assert_eq!(filename_title("https://example.com/"), "");
    }

    #[test]
    fn pdf_max_bytes_constant_is_50_mb() {
        assert_eq!(PDF_MAX_BYTES, 50 * 1024 * 1024);
    }

    #[tokio::test]
    async fn extract_pdf_bytes_rejects_oversize() {
        let big = vec![0u8; PDF_MAX_BYTES + 1];
        let r = extract_pdf_bytes("https://example.com/big.pdf", big).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("pdf_too_large"));
    }

    // Real PDF parsing exercised in Plan 14 wiremock test (web_extract_pdf_url_routes_to_pdf_backend).
}
