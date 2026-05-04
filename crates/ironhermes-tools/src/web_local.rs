//! Phase 25.2 D-04: shared HTML→Markdown helpers used by web_read.rs and web_extract.rs.
//! Functions and constants moved verbatim from web_read.rs (refactor target — DRY constraint).
//!
//! See .planning/phases/25.2-web-extract-tools/25.2-CONTEXT.md D-04 for the no-copy-paste rule.

use ironhermes_core::ssrf::is_safe_url;
use scraper::{Html, Selector};

// --- Boilerplate and content selectors (D-01, D-08) ---

pub const BOILERPLATE_SELECTORS: &[&str] = &[
    "nav",
    "header",
    "footer",
    "aside",
    "[role=navigation]",
    "[role=banner]",
    "[role=contentinfo]",
    "script",
    "style",
    "noscript",
];

pub const CONTENT_SELECTORS: &[&str] = &["article", "main", "[role=main]", "body"];

// --- SSRF validation (D-16) ---

/// Wrap `is_safe_url` in spawn_blocking for async callers.
///
/// **Test-only escape hatch:** when the env var
/// `IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK` is set (any value), this function
/// returns `Ok(())` for URLs whose host is `127.0.0.1` / `::1` / `localhost`
/// without consulting `is_safe_url`. This lets wiremock-backed integration
/// tests reach loopback servers (which `is_safe_url` correctly blocks in
/// production) without disabling SSRF protection for any non-loopback host.
/// The bypass mirrors the `IRONHERMES_BROWSER_TEST_DISABLE` pattern from
/// Phase 25.1 (Plan 25.1-10) — the `_TEST_` infix makes the test-only intent
/// crystal-clear and the env var is never read in production code paths.
pub async fn validate_url_async(url: &str) -> anyhow::Result<()> {
    if std::env::var("IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK").is_ok() && is_loopback_host(url) {
        return Ok(());
    }
    let url_owned = url.to_string();
    let safe = tokio::task::spawn_blocking(move || is_safe_url(&url_owned))
        .await
        .map_err(|e| anyhow::anyhow!("SSRF check task panicked: {}", e))?;
    if !safe {
        return Err(anyhow::anyhow!(
            "URL blocked by security policy (private IP)"
        ));
    }
    Ok(())
}

/// Test-only helper: `true` when `url`'s host is loopback (`127.0.0.1`, `::1`,
/// or `localhost`). Only consulted when the `IRONHERMES_SSRF_TEST_ALLOW_LOOPBACK`
/// env var is set; production code never reaches this branch.
fn is_loopback_host(url: &str) -> bool {
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            return host == "127.0.0.1" || host == "::1" || host == "localhost";
        }
    }
    false
}

// --- Smart truncation (D-13, D-14, D-15) ---

/// Truncate content at a smart boundary (paragraph > sentence > word > hard cut).
/// Appends a truncation notice with char counts (not byte counts).
pub fn truncate_content(content: &str, max_chars: usize) -> String {
    let total_chars = content.chars().count();
    if total_chars <= max_chars {
        return content.to_string();
    }

    // Find the byte offset at the max_chars'th character (UTF-8 safe — avoids mid-codepoint split).
    let cut_byte = content
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(content.len());

    let slice = &content[..cut_byte];

    // Search backward for the best break point.
    let break_byte = if let Some(pos) = slice.rfind("\n\n") {
        pos
    } else if let Some(pos) = slice.rfind(". ") {
        pos + 2 // include the period and space
    } else if let Some(pos) = slice.rfind(' ') {
        pos
    } else {
        cut_byte
    };

    let trimmed = &content[..break_byte];
    let displayed_chars = trimmed.chars().count();

    format!(
        "{}\n\n[Content truncated at {} of {} characters]",
        trimmed, displayed_chars, total_chars
    )
}

// --- Local HTML extraction (D-01, D-05, D-06, D-08) ---

/// Extract and convert HTML content to markdown using scraper + htmd.
pub fn extract_content_local(html: &str, url: &str) -> anyhow::Result<String> {
    let document = Html::parse_document(html);

    // Extract title.
    let title = {
        let title_sel = Selector::parse("title").unwrap();
        document
            .select(&title_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default()
    };

    // Find main content area by iterating selectors in priority order.
    let mut content_html = String::new();
    for &sel_str in CONTENT_SELECTORS {
        if let Ok(sel) = Selector::parse(sel_str)
            && let Some(el) = document.select(&sel).next()
        {
            content_html = el.html();
            break;
        }
    }

    if content_html.is_empty() {
        content_html = document.root_element().html();
    }

    // Strip boilerplate: re-parse the content fragment and replace boilerplate
    // element outer HTML with empty strings in the content HTML string.
    let content_doc = Html::parse_fragment(&content_html);
    for &boilerplate in BOILERPLATE_SELECTORS {
        if let Ok(sel) = Selector::parse(boilerplate) {
            for el in content_doc.select(&sel) {
                let outer = el.html();
                content_html = content_html.replacen(&outer, "", 1);
            }
        }
    }

    // Convert cleaned HTML to markdown.
    let markdown = htmd::convert(&content_html)
        .map_err(|e| anyhow::anyhow!("htmd conversion failed: {}", e))?;

    // Assemble output with header (D-06).
    let header = if title.is_empty() {
        format!("Source: {}\n\n", url)
    } else {
        format!("# {}\nSource: {}\n\n", title, url)
    };

    Ok(format!("{}{}", header, markdown))
}

// --- Smoke tests (D-04 ensures the moved code compiles + behaves) ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_content_local_smoke() {
        let html = r#"<html><head><title>Hello</title></head><body><article><p>World</p></article></body></html>"#;
        let md = extract_content_local(html, "https://example.com/x").expect("extract ok");
        assert!(
            md.contains("# Hello"),
            "expected Markdown header from <title>"
        );
        assert!(md.contains("World"), "expected body content preserved");
        assert!(
            md.contains("Source: https://example.com/x"),
            "expected source line"
        );
    }

    #[tokio::test]
    async fn validate_url_async_blocks_private_ip() {
        let r = validate_url_async("http://127.0.0.1/x").await;
        assert!(r.is_err(), "private IP must be blocked");
    }

    #[test]
    fn truncate_content_respects_max() {
        let s = "a".repeat(10_000);
        let out = truncate_content(&s, 100);
        assert!(
            out.len() <= 200,
            "truncated len = {} should be near 100",
            out.len()
        );
        assert!(
            out.contains("[Content truncated"),
            "expected truncation marker"
        );
    }
}
