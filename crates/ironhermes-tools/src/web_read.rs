use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::{ToolSchema, config::Config};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, warn};

use crate::registry::Tool;
use crate::web_local::{extract_content_local, truncate_content, validate_url_async};

// --- Firecrawl response types (D-01) ---

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
    status_code: Option<u16>,
}

// --- Firecrawl fetch (D-02) ---

/// Fetch a URL via the Firecrawl scrape API.
async fn fetch_with_firecrawl(url: &str) -> anyhow::Result<String> {
    let api_key = std::env::var("FIRECRAWL_API_KEY")
        .map_err(|_| anyhow::anyhow!("FIRECRAWL_API_KEY not set"))?;

    let timeout_secs = Config::load().map(|c| c.web.timeout_secs).unwrap_or(30);

    debug!("Fetching {} via Firecrawl", url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let response = client
        .post("https://api.firecrawl.dev/v1/scrape")
        .bearer_auth(&api_key)
        .json(&json!({
            "url": url,
            "formats": ["markdown"]
        }))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Firecrawl request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Firecrawl API returned {}: {}",
            status,
            body
        ));
    }

    let scrape_response: FirecrawlScrapeResponse = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse Firecrawl response: {}", e))?;

    if !scrape_response.success {
        return Err(anyhow::anyhow!(
            "Firecrawl scrape failed: {}",
            scrape_response
                .error
                .unwrap_or_else(|| "unknown error".to_string())
        ));
    }

    let data = scrape_response
        .data
        .ok_or_else(|| anyhow::anyhow!("Firecrawl response missing data field"))?;

    let markdown = data
        .markdown
        .ok_or_else(|| anyhow::anyhow!("Firecrawl response missing markdown content"))?;

    // Check metadata for non-200 status codes.
    if let Some(ref meta) = data.metadata
        && let Some(code) = meta.status_code
        && code >= 400
    {
        return Err(anyhow::anyhow!("Target page returned HTTP {}", code));
    }

    let title = data.metadata.and_then(|m| m.title).unwrap_or_default();

    let header = if title.is_empty() {
        format!("Source: {}\n\n", url)
    } else {
        format!("# {}\nSource: {}\n\n", title, url)
    };

    Ok(format!("{}{}", header, markdown))
}

// --- Local fetch with reqwest (D-03, D-07, D-17) ---

/// Fetch a URL directly with reqwest and convert HTML to markdown.
async fn fetch_local(
    url: &str,
    config: &ironhermes_core::config::WebConfig,
) -> anyhow::Result<String> {
    debug!("Fetching {} via local fallback", url);

    let client = reqwest::Client::builder()
        .user_agent(&config.user_agent)
        .timeout(Duration::from_secs(config.timeout_secs))
        // Default redirect policy follows redirects (D-03).
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

    // Post-redirect SSRF re-validation (D-17): if redirected, validate the final URL.
    let final_url = response.url().as_str().to_string();
    if final_url != url {
        validate_url_async(&final_url)
            .await
            .map_err(|_| anyhow::anyhow!("URL blocked by security policy (private IP)"))?;
    }

    // Content-Type check (D-07): only accept text/html responses.
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if !content_type.starts_with("text/html") {
        return Err(anyhow::anyhow!(
            "web_read only supports HTML pages. Got: {}",
            content_type
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

    extract_content_local(&body, url)
}

// --- Unit tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // ---- truncate_content tests ----

    #[test]
    fn test_truncate_under_limit() {
        let content = "Short content.";
        let result = truncate_content(content, 100);
        assert_eq!(result, content);
        assert!(!result.contains("[Content truncated"));
    }

    #[test]
    fn test_truncate_at_limit() {
        let content = "Exactly at limit.";
        let result = truncate_content(content, content.chars().count());
        assert_eq!(result, content);
        assert!(!result.contains("[Content truncated"));
    }

    #[test]
    fn test_truncate_paragraph_break() {
        // Limit falls inside the third paragraph. Expect cut at "\n\n" after second paragraph.
        // "First paragraph.\n\nSecond paragraph." = 36 chars, "\n\n" at index 16.
        // With limit 45, slice = first 45 chars contains two "\n\n" breaks.
        // rfind("\n\n") finds the one at index 35. break_byte = 35.
        // trimmed = "First paragraph.\n\nSecond paragraph."
        let content = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph is longer.";
        let result = truncate_content(content, 45);
        assert!(result.starts_with("First paragraph.\n\nSecond paragraph."));
        assert!(result.contains("[Content truncated"));
        assert!(!result.contains("Third"));
    }

    #[test]
    fn test_truncate_sentence_break() {
        // No paragraph breaks; sentence break at ". " boundary.
        // content = "AAAA. BBBB." (11 chars). Limit 8.
        // slice = "AAAA. BB". rfind(". ") = 4. break_byte = 6.
        // trimmed = "AAAA. " (6 chars displayed).
        let content = "AAAA. BBBB.";
        let result = truncate_content(content, 8);
        assert!(result.contains("[Content truncated"));
        let trimmed_part = result.split("\n\n[Content truncated").next().unwrap();
        assert_eq!(trimmed_part, "AAAA. ");
    }

    #[test]
    fn test_truncate_word_break() {
        // No paragraph or sentence breaks; word break at space.
        // content = "one two three" (13 chars). Limit 8.
        // slice = "one two " (8 chars). rfind(' ') = 7. break_byte = 7.
        // trimmed = "one two" (7 chars).
        let content = "one two three";
        let result = truncate_content(content, 8);
        assert!(result.contains("[Content truncated"));
        let trimmed_part = result.split("\n\n[Content truncated").next().unwrap();
        assert_eq!(trimmed_part, "one two");
    }

    #[test]
    fn test_truncate_no_whitespace() {
        // No paragraph, sentence, or word breaks: hard cut at max_chars.
        let content = "abcdefghijklmnopqrstuvwxyz";
        let result = truncate_content(content, 10);
        assert!(result.contains("[Content truncated at 10 of 26 characters]"));
        let trimmed_part = result.split("\n\n[Content truncated").next().unwrap();
        assert_eq!(trimmed_part, "abcdefghij");
    }

    #[test]
    fn test_truncate_utf8_emoji() {
        // Each emoji is 4 bytes but 1 char. Should not panic and truncate at char boundary.
        // content = "Hello \u{1F600}\u{1F601}\u{1F602} world" = 17 chars. Limit 9.
        // slice = first 9 chars. rfind(' ') finds space at index 5. break_byte = 5.
        // trimmed = "Hello" (5 chars).
        let content = "Hello \u{1F600}\u{1F601}\u{1F602} world";
        let result = truncate_content(content, 9);
        // Must not panic — the test itself proves this.
        assert!(result.contains("[Content truncated"));
    }

    #[test]
    fn test_truncate_notice_char_counts() {
        // Verify the truncation notice shows char counts not byte counts.
        let content = "abcdefghijklmnopqrstuvwxyz";
        let total_chars = content.chars().count(); // 26
        let result = truncate_content(content, 10);
        assert!(result.contains(&format!("of {} characters]", total_chars)));
    }

    #[test]
    fn test_truncate_notice_displayed_chars() {
        // Hard cut: trimmed = "abcdefghij" (10 chars). Notice shows "at 10 of 26".
        let content = "abcdefghijklmnopqrstuvwxyz";
        let result = truncate_content(content, 10);
        assert!(result.contains("truncated at 10 of 26 characters]"));
    }

    // ---- extract_content_local tests ----

    #[test]
    fn test_extract_selects_article() {
        let html = r#"<html><head><title>Test</title></head><body>
            <nav>Navigation</nav>
            <article><h1>Main Content</h1><p>Article text.</p></article>
            <footer>Footer</footer>
        </body></html>"#;
        let result = extract_content_local(html, "https://example.com").unwrap();
        assert!(result.starts_with("# Test\nSource: https://example.com\n\n"));
        assert!(result.contains("Main Content"));
        assert!(result.contains("Article text"));
    }

    #[test]
    fn test_extract_selects_main_over_body() {
        let html = r#"<html><head><title>Page</title></head><body>
            <nav>Nav</nav>
            <main><p>Main content here.</p></main>
            <aside>Sidebar</aside>
        </body></html>"#;
        let result = extract_content_local(html, "https://example.com").unwrap();
        assert!(result.contains("Main content here"));
    }

    #[test]
    fn test_extract_falls_back_to_body() {
        let html = r#"<html><head><title>Simple</title></head><body>
            <div><p>Body content only.</p></div>
        </body></html>"#;
        let result = extract_content_local(html, "https://example.com").unwrap();
        assert!(result.contains("Body content only"));
    }

    #[test]
    fn test_extract_strips_boilerplate() {
        let html = r#"<html><head><title>Clean</title></head><body>
            <header>Site Header</header>
            <nav>Menu items here</nav>
            <article><p>Real content.</p></article>
            <footer>Copyright 2024</footer>
            <aside>Related links</aside>
        </body></html>"#;
        let result = extract_content_local(html, "https://example.com").unwrap();
        assert!(result.contains("Real content"));
    }

    #[test]
    fn test_extract_prepends_header() {
        let html = r#"<html><head><title>My Page</title></head><body><p>Content</p></body></html>"#;
        let result = extract_content_local(html, "https://test.org/page").unwrap();
        assert!(result.starts_with("# My Page\nSource: https://test.org/page\n\n"));
    }

    #[test]
    fn test_extract_converts_to_markdown() {
        let html = r#"<html><head><title>MD</title></head><body>
            <article>
                <h2>Heading Two</h2>
                <p>A paragraph with a <a href="https://link.com">link</a>.</p>
            </article>
        </body></html>"#;
        let result = extract_content_local(html, "https://example.com").unwrap();
        // htmd converts h2 to ## and links to [text](url)
        assert!(result.contains("##") || result.contains("Heading Two"));
        assert!(result.contains("link") || result.contains("[link]"));
    }

    #[test]
    fn test_extract_no_title_uses_source_only() {
        // When no <title> element, header should be "Source: {url}\n\n"
        let html = r#"<html><body><p>Content without title.</p></body></html>"#;
        let result = extract_content_local(html, "https://notitle.example.com").unwrap();
        assert!(result.starts_with("Source: https://notitle.example.com\n\n"));
        assert!(!result.starts_with("# "));
    }
}

// --- WebReadTool ---

pub struct WebReadTool;

#[async_trait]
impl Tool for WebReadTool {
    fn name(&self) -> &str {
        "web_read"
    }

    fn toolset(&self) -> &str {
        "web"
    }

    fn description(&self) -> &str {
        "Fetch a web page and return its content as markdown."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "web_read",
            "Fetch a web page and return its content as markdown.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL of the web page to fetch."
                    }
                },
                "required": ["url"]
            }),
        )
    }

    fn prerequisites(&self) -> Vec<crate::registry::Prerequisite> {
        vec![crate::registry::Prerequisite {
            kind: "env_var".to_string(),
            name: "FIRECRAWL_API_KEY".to_string(),
            description: "Firecrawl API key — optional. Without it, web_read uses the plain-text fallback path.".to_string(),
            required: false,
        }]
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let raw_url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: url"))?;

        // Normalize URL: prepend https:// if no scheme is provided.
        let url = if !raw_url.starts_with("http://") && !raw_url.starts_with("https://") {
            format!("https://{raw_url}")
        } else {
            raw_url.to_string()
        };
        let url = url.as_str();

        // SSRF validation before any fetch (D-16).
        validate_url_async(url).await?;

        // Load config (fall back to defaults if config file missing).
        let config = Config::load().unwrap_or_default();

        // Try Firecrawl first if API key is set (D-02).
        let content = if std::env::var("FIRECRAWL_API_KEY").is_ok() {
            match fetch_with_firecrawl(url).await {
                Ok(content) => content,
                Err(e) => {
                    warn!("Firecrawl failed, falling back to local fetch: {}", e);
                    fetch_local(url, &config.web).await?
                }
            }
        } else {
            fetch_local(url, &config.web).await?
        };

        // Smart truncation (D-13, D-14, D-15).
        Ok(truncate_content(&content, config.web.max_content_chars))
    }
}
