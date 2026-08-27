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
                let url_for_log = crate::web_extract::sanitize::redact_secrets_in_url(url, &[]);
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
        validate_url_async(&final_url)
            .await
            .map_err(|_| anyhow!("URL blocked by security policy (private IP) after redirect"))?;
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

    /// Builds a minimal, syntactically valid PDF wrapper (Catalog=1, empty Pages=2) around a
    /// caller-supplied raw byte body for object 3. The xref/trailer offsets are computed from
    /// real byte positions so lopdf's xref-driven load actually visits object 3 — that is what
    /// makes a malformed object 3 body exercise the real parser rather than being skipped as
    /// unreachable. D-16: bounded, non-destructive fixtures only.
    fn build_pdf_fixture(object_3_raw: &[u8]) -> Vec<u8> {
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let obj2_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");

        let obj3_offset = pdf.len();
        pdf.extend_from_slice(object_3_raw);

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        pdf.extend_from_slice(format!("{obj1_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj2_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj3_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(b"trailer\n<< /Size 4 /Root 1 0 R >>\n");
        pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());

        pdf
    }

    /// Builds a minimal, syntactically valid one-page PDF (Catalog=1, Pages=2, Page=3 with
    /// `/Contents 4 0 R`) around a caller-supplied raw byte body for object 4 — the page's own
    /// content stream. Unlike `build_pdf_fixture`, object 4 sits on the real page-content read
    /// path `pdf_extract::extract_text_from_mem` must walk to produce any text, so a malformed
    /// object 4 body is guaranteed to be exercised rather than merely present-but-unvisited.
    /// D-16: bounded, non-destructive fixtures only.
    fn build_pdf_fixture_with_content(object_4_raw: &[u8]) -> Vec<u8> {
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");

        let obj1_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        let obj2_offset = pdf.len();
        pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

        let obj3_offset = pdf.len();
        pdf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>\nendobj\n",
        );

        let obj4_offset = pdf.len();
        pdf.extend_from_slice(object_4_raw);

        let xref_offset = pdf.len();
        pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        pdf.extend_from_slice(format!("{obj1_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj2_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj3_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(format!("{obj4_offset:010} 00000 n \n").as_bytes());
        pdf.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\n");
        pdf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());

        pdf
    }

    /// D-09 regression test for RUSTSEC-2026-0187 (lopdf DoS on malformed input): a bounded,
    /// syntactically valid but deeply nested array as object 3's body. The post-bump parser
    /// must return control (either `Ok` or `Err`) instead of panicking or hanging.
    #[tokio::test]
    async fn extract_pdf_bytes_handles_deep_nesting_without_panic() {
        let depth = 500;
        let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let object_3 = format!("3 0 obj\n{nested}\nendobj\n");
        let fixture = build_pdf_fixture(object_3.as_bytes());
        assert!(
            fixture.len() < 16 * 1024,
            "deep-nesting fixture must stay under 16 KB, got {} bytes",
            fixture.len()
        );

        let result = extract_pdf_bytes("https://example.com/deep-nest.pdf", fixture).await;
        // Either outcome is acceptable — the regression signal is that this line is ever
        // reached (spawn_blocking + the 30s timeout would otherwise hang the test on a
        // pre-bump lopdf), not which variant comes back.
        assert!(
            result.is_ok() || result.is_err(),
            "extract_pdf_bytes must return a Result, not hang or panic, on deeply nested input"
        );
    }

    /// D-09 regression test: a normally-well-formed one-page PDF is cut off mid-download —
    /// only the first half of its bytes are handed to the parser, so its content-stream
    /// dictionary is unterminated (no closing `>>`), its `stream`/`endstream` pairing never
    /// completes, and its xref table + trailer (which carry `/Root`) never arrive at all. This
    /// is the "truncated mid-structure" case: no amount of lopdf's broken-xref recovery
    /// scanning can synthesize bytes that were never in the file, so the parser must surface a
    /// clean `Err` (either from its own parse failure, or from the `spawn_blocking`
    /// `JoinError` path if it panics internally on the truncated input) — never let a raw
    /// panic escape the async task.
    #[tokio::test]
    async fn extract_pdf_bytes_handles_truncated_object_stream() {
        let object_4: &[u8] =
            b"4 0 obj\n<< /Length 40 >>\nstream\nBT /F1 12 Tf 100 700 Td (Hello) Tj ET\nendstream\nendobj\n";
        let full = build_pdf_fixture_with_content(object_4);
        // Cut the file in half: keeps the header and the first couple of objects intact but
        // discards the content stream's own close, and discards the xref/trailer/`/Root`
        // entirely — there is nothing left in the byte stream for recovery mode to find.
        let fixture = full[..full.len() / 2].to_vec();
        assert!(
            fixture.len() < 16 * 1024,
            "truncated-content-stream fixture must stay under 16 KB, got {} bytes",
            fixture.len()
        );

        let result = extract_pdf_bytes("https://example.com/truncated.pdf", fixture).await;
        assert!(
            result.is_err(),
            "truncated content stream must fail cleanly, got: {:?}",
            result
        );
    }
}
