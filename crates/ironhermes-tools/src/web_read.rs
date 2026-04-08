use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::{config::Config, ssrf::is_safe_url, ToolSchema};
use scraper::{Html, Selector};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, warn};

use crate::registry::Tool;

// --- Boilerplate and content selectors (D-01, D-08) ---

const BOILERPLATE_SELECTORS: &[&str] = &[
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

const CONTENT_SELECTORS: &[&str] = &["article", "main", "[role=main]", "body"];

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

// --- SSRF validation (D-16) ---

/// Wrap `is_safe_url` in spawn_blocking for async callers.
async fn validate_url_async(url: &str) -> anyhow::Result<()> {
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

// --- Smart truncation (D-13, D-14, D-15) ---

/// Truncate content at a smart boundary (paragraph > sentence > word > hard cut).
/// Appends a truncation notice with char counts (not byte counts).
fn truncate_content(content: &str, max_chars: usize) -> String {
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
fn extract_content_local(html: &str, url: &str) -> anyhow::Result<String> {
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
        if let Ok(sel) = Selector::parse(sel_str) {
            if let Some(el) = document.select(&sel).next() {
                content_html = el.html();
                break;
            }
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

// --- Firecrawl fetch (D-02) ---

/// Fetch a URL via the Firecrawl scrape API.
async fn fetch_with_firecrawl(url: &str) -> anyhow::Result<String> {
    let api_key = std::env::var("FIRECRAWL_API_KEY")
        .map_err(|_| anyhow::anyhow!("FIRECRAWL_API_KEY not set"))?;

    let timeout_secs = Config::load()
        .map(|c| c.web.timeout_secs)
        .unwrap_or(30);

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
    if let Some(ref meta) = data.metadata {
        if let Some(code) = meta.status_code {
            if code >= 400 {
                return Err(anyhow::anyhow!("Target page returned HTTP {}", code));
            }
        }
    }

    let title = data
        .metadata
        .and_then(|m| m.title)
        .unwrap_or_default();

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
        validate_url_async(&final_url).await.map_err(|_| {
            anyhow::anyhow!(
                "URL blocked by security policy (private IP)"
            )
        })?;
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

    fn is_available(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: url"))?;

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
