# Phase 4: Web Scraping Tools - Research

**Researched:** 2026-04-07
**Domain:** HTTP fetching, HTML parsing, content extraction, Firecrawl API
**Confidence:** HIGH

## Summary

Phase 4 adds a `web_read` tool that fetches a URL and returns its content as markdown. The primary backend is Firecrawl's scrape API (same auth pattern as the existing `web_search` tool). A local fallback uses the `scraper` crate for CSS-selector-based content extraction and `htmd` for HTML-to-markdown conversion. SSRF protection from Phase 3 (`is_safe_url()`) validates all URLs before fetching.

This is a well-scoped phase. The Firecrawl scrape endpoint mirrors the already-integrated search endpoint (same auth, same `data.markdown` response field). The local fallback is a lightweight HTML parser with semantic selectors. All building blocks exist in the workspace -- reqwest for HTTP, `is_safe_url` for SSRF, `ToolSchema` + `Tool` trait for registration.

**Primary recommendation:** Build `WebReadTool` in a single new file (`web_read.rs`) following the `WebSearchTool` pattern exactly. Use Firecrawl `/v1/scrape` as primary, `scraper` + `htmd` as local fallback. Add `gzip`/`brotli`/`deflate` features to reqwest in workspace Cargo.toml.

<user_constraints>

## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Local fallback uses `scraper` crate with semantic selector heuristic (`<article>`, `<main>`, `[role=main]`, then `<body>`). ~50-line extractor with boilerplate stripping.
- **D-02:** Fallback activates when no `FIRECRAWL_API_KEY` configured OR Firecrawl request fails (500, timeout, network error). Maximum availability.
- **D-03:** Follow HTTP redirects up to reqwest default limit. Re-validate the final URL against SSRF before fetching content (redirect-to-internal attack prevention).
- **D-04:** 30-second timeout for HTTP requests (both Firecrawl and local fallback).
- **D-05:** Return markdown format. Firecrawl already returns markdown; local fallback converts HTML to markdown (headings to `#`, links to `[text](url)`, lists to `- items`).
- **D-06:** Prepend `# {title}\nSource: {url}\n\n` header before content. Gives LLM attribution context.
- **D-07:** HTML only -- check `Content-Type` header. If not `text/html`, return error: "web_read only supports HTML pages. Got: {content_type}". Keep scope minimal for v1.
- **D-08:** Strip common boilerplate elements: `<nav>`, `<header>`, `<footer>`, `<aside>`, `[role=navigation]`, `[role=banner]`, `[role=contentinfo]`. Focus on main content area.
- **D-09:** Single required parameter: `url`. No optional selector, format, or other params.
- **D-10:** Descriptive error strings for all failure modes.
- **D-11:** Tool is always registered and available (`is_available()` returns `true`). If fetch fails at runtime, return descriptive error -- don't hide the tool.
- **D-12:** Configurable User-Agent in `config.yaml` (`web.user_agent`). Default: `IronHermes/1.0 (+bot)`.
- **D-13:** Smart boundary truncation: cut at nearest paragraph break (`\n\n`) or sentence break (`. `) before the limit. Never break mid-word.
- **D-14:** Append `\n\n[Content truncated at {displayed_chars} of {total_chars} characters]` notice when truncation occurs.
- **D-15:** Configurable limit via `web.max_content_chars` in `config.yaml`. Default: 50,000 characters.
- **D-16:** `is_safe_url()` from `ironhermes-core::ssrf` runs before every fetch. Wrap with `tokio::task::spawn_blocking()` for async context.
- **D-17:** SSRF validation runs on initial URL AND on final URL after redirects.

### Claude's Discretion
- HTML-to-markdown conversion approach (htmd crate, custom converter, or scraper + manual conversion)
- Exact semantic selector priority order for content extraction
- reqwest client configuration details (compression features, connection pooling)
- Whether to add gzip/brotli/deflate features to reqwest (ROADMAP suggests yes)
- Error message wording details

### Deferred Ideas (OUT OF SCOPE)
- Cloudflare Browser Rendering (`/crawl` endpoint) for JS-rendered pages
- cmux browser automation service
- agent-browser (vercel-labs) AI-native browser
- These address JavaScript-rendered pages -- future "Advanced Web Tools" phase

</user_constraints>

<phase_requirements>

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| WEB-01 | web_read tool: fetch URL content via Firecrawl scrape API, return extracted text | Firecrawl `/v1/scrape` endpoint returns `data.markdown`; same auth pattern as existing `WebSearchTool` |
| WEB-02 | SSRF protection: validate URLs before fetching (block private IPs, localhost, internal ranges) | `is_safe_url()` exists in `ironhermes-core::ssrf`; needs `spawn_blocking` wrapper for async context + post-redirect re-validation |
| WEB-03 | Content truncation: cap extracted text to context-window-safe length (configurable, default 50K chars) | Smart boundary truncation at `\n\n` or `. ` boundaries; config field `web.max_content_chars` |
| WEB-04 | Local HTML fallback: scraper crate for content extraction when Firecrawl is unavailable | `scraper` 0.26.0 for CSS selector extraction + `htmd` 0.5.4 for HTML-to-markdown conversion |

</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| scraper | 0.26.0 | HTML parsing and CSS selector queries | De facto Rust HTML parser; built on html5ever [VERIFIED: cargo search] |
| htmd | 0.5.4 | HTML to markdown conversion | turndown.js-inspired converter; `convert(html) -> Result<String>` API [VERIFIED: docs.rs] |
| reqwest | 0.12.28 | HTTP client (already in workspace) | Already used by WebSearchTool; add compression features [VERIFIED: cargo check] |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| url | 2.x | URL parsing (already in workspace) | Used by ssrf.rs, reuse for redirect URL extraction |
| tokio | 1.x | Async runtime (already in workspace) | `spawn_blocking` for SSRF validation in async context |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| htmd | Manual HTML-to-markdown in scraper selectors | More control but 200+ lines vs htmd's single `convert()` call |
| htmd | html2md crate | html2md is older, less maintained; htmd is actively maintained (turndown.js port) |
| scraper | select.rs | scraper has broader adoption and better CSS selector support |

**Installation (additions to workspace Cargo.toml):**
```toml
scraper = "0.26"
htmd = "0.5"
```

**Additions to ironhermes-tools/Cargo.toml:**
```toml
scraper = { workspace = true }
htmd = { workspace = true }
```

**reqwest feature additions (workspace Cargo.toml):**
```toml
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls", "gzip", "brotli", "deflate"], default-features = false }
```

**Version verification:**
- scraper 0.26.0 -- current on crates.io [VERIFIED: cargo search]
- htmd 0.5.4 -- current on crates.io [VERIFIED: cargo search]
- reqwest 0.12.28 -- already installed in workspace [VERIFIED: cargo check output]

## Architecture Patterns

### Recommended File Structure
```
crates/ironhermes-tools/src/
├── web_read.rs          # NEW: WebReadTool implementation
├── web_search.rs        # Existing: pattern reference
├── registry.rs          # MODIFY: add WebReadTool to register_defaults()
├── lib.rs               # MODIFY: add pub mod web_read
└── ...

crates/ironhermes-core/src/
├── config.rs            # MODIFY: extend WebConfig with new fields
├── ssrf.rs              # EXISTING: is_safe_url() -- no changes needed
└── ...
```

### Pattern 1: Firecrawl Scrape Request/Response
**What:** POST to Firecrawl `/v1/scrape` with URL, receive markdown content
**When to use:** Primary path when `FIRECRAWL_API_KEY` is set and API is reachable

The existing `WebSearchTool` uses `/v1/search`. The scrape endpoint follows the same pattern. Note: Firecrawl docs now show `/v2/scrape` as the current version, but since the existing search tool uses v1, consistency suggests using v1 for scrape as well. If v1 scrape is unavailable, the local fallback handles it transparently (D-02). [ASSUMED -- v1 scrape endpoint availability should be tested at implementation time]

```rust
// Source: existing web_search.rs pattern + Firecrawl docs
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
```

**Firecrawl scrape request body:**
```json
{
    "url": "https://example.com",
    "formats": ["markdown"]
}
```

**Response contains `data.markdown` with the page content as markdown, and `data.metadata.title` for the page title.** [VERIFIED: docs.firecrawl.dev]

### Pattern 2: Local Fallback with scraper + htmd
**What:** HTTP GET with reqwest, parse HTML with scraper, strip boilerplate, convert to markdown with htmd
**When to use:** When Firecrawl is unavailable (no API key or request failure)

```rust
// Source: docs.rs/scraper + docs.rs/htmd
use scraper::{Html, Selector};
use htmd::convert;

fn extract_content_local(html: &str, url: &str) -> anyhow::Result<String> {
    let document = Html::parse_document(html);

    // Strip boilerplate elements (D-08)
    // Note: scraper doesn't support in-place removal, so select
    // the main content area instead of removing boilerplate

    // Semantic selector priority (D-01):
    // 1. <article>
    // 2. <main>
    // 3. [role=main]
    // 4. <body> (last resort)
    let selectors = ["article", "main", "[role=main]", "body"];
    let mut content_html = String::new();

    for sel_str in &selectors {
        if let Ok(selector) = Selector::parse(sel_str) {
            if let Some(element) = document.select(&selector).next() {
                content_html = element.html();
                break;
            }
        }
    }

    // Convert HTML to markdown
    let markdown = convert(&content_html)?;

    // Extract title
    let title = Selector::parse("title")
        .ok()
        .and_then(|s| document.select(&s).next())
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default();

    // Prepend header (D-06)
    Ok(format!("# {}\nSource: {}\n\n{}", title, url, markdown))
}
```

### Pattern 3: SSRF Validation with spawn_blocking
**What:** Wrap synchronous `is_safe_url()` for async context
**When to use:** Before every HTTP fetch (both initial URL and post-redirect)

```rust
// Source: ssrf.rs doc comment
use ironhermes_core::ssrf::is_safe_url;

async fn validate_url(url: &str) -> anyhow::Result<()> {
    let url_owned = url.to_string();
    let is_safe = tokio::task::spawn_blocking(move || is_safe_url(&url_owned))
        .await
        .map_err(|e| anyhow::anyhow!("SSRF validation task failed: {}", e))?;

    if !is_safe {
        anyhow::bail!("URL blocked by security policy (private IP)");
    }
    Ok(())
}
```

### Pattern 4: Smart Content Truncation
**What:** Truncate at clean boundaries, never mid-word
**When to use:** After content extraction, before returning to agent

```rust
fn truncate_content(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    let search_range = &content[..max_chars];

    // Try paragraph break first
    let cut_point = search_range.rfind("\n\n")
        // Then sentence break
        .or_else(|| search_range.rfind(". ").map(|i| i + 2))
        // Then word break
        .or_else(|| search_range.rfind(' '))
        // Last resort: hard cut at limit
        .unwrap_or(max_chars);

    let truncated = &content[..cut_point];
    format!(
        "{}\n\n[Content truncated at {} of {} characters]",
        truncated.trim_end(),
        cut_point,
        content.len()
    )
}
```

**Important:** This assumes the content is ASCII-safe for byte indexing. For UTF-8 safety, use `content.char_indices()` or `content.chars().take(max_chars)` to avoid panics on multi-byte boundaries. The planner should ensure truncation uses char-aware indexing.

### Pattern 5: Post-Redirect SSRF Re-validation (D-03, D-17)
**What:** After reqwest follows redirects, check the final URL against SSRF
**When to use:** Local fallback path (Firecrawl handles this server-side)

```rust
// reqwest provides the final URL after redirects via response.url()
let response = client.get(url).send().await?;
let final_url = response.url().to_string();

// Re-validate final URL if it differs from original
if final_url != url {
    validate_url(&final_url).await?;
}
```

### Anti-Patterns to Avoid
- **Building a full HTML-to-markdown converter by hand:** Use `htmd::convert()`. HTML edge cases are endless (nested tables, malformed tags, entities).
- **Stripping boilerplate by removing elements:** `scraper` doesn't support DOM mutation well. Instead, select the main content area positively (article/main/body) rather than trying to remove nav/footer/header.
- **Blocking the tokio runtime with DNS resolution:** `is_safe_url()` uses synchronous `ToSocketAddrs`. Always wrap in `spawn_blocking`.
- **Trusting the initial URL after redirects:** A redirect from `example.com` to `169.254.169.254` (metadata endpoint) bypasses initial SSRF check. Always re-validate post-redirect.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| HTML to markdown | Custom string-replacement converter | `htmd::convert()` | Handles edge cases (nested elements, entities, malformed HTML) |
| HTML parsing | Regex-based extraction | `scraper` crate (html5ever) | HTML is not regular; regex fails on real-world pages |
| HTTP compression | Manual Accept-Encoding/decompression | reqwest `gzip`/`brotli`/`deflate` features | Automatic negotiation and decompression |
| CSS selector matching | Custom DOM traversal | `scraper::Selector::parse()` | Handles complex selectors, combinators, pseudo-classes |

**Key insight:** HTML extraction looks simple but real-world pages are messy. Boilerplate, malformed HTML, encoding issues, and edge cases make hand-rolled solutions fragile. The `scraper` + `htmd` combination handles these robustly with ~50 lines of glue code.

## Common Pitfalls

### Pitfall 1: UTF-8 Truncation Panic
**What goes wrong:** Slicing a `String` at a byte offset that falls within a multi-byte UTF-8 character causes a panic.
**Why it happens:** `content[..max_chars]` indexes bytes, not chars. Pages with CJK text, emoji, or accented characters have multi-byte sequences.
**How to avoid:** Use `content.char_indices()` to find the byte offset of the Nth character, or use `.floor_char_boundary(max_chars)` (nightly) / a helper function.
**Warning signs:** Panics only on non-ASCII content -- tests with ASCII-only strings pass.

### Pitfall 2: Content-Type Header Parsing
**What goes wrong:** Checking `content_type == "text/html"` fails because headers often include charset: `text/html; charset=utf-8`.
**Why it happens:** HTTP Content-Type is a media type with optional parameters.
**How to avoid:** Use `.starts_with("text/html")` or parse with the `mime` crate. reqwest's `response.headers()` returns the raw header value.
**Warning signs:** Tool rejects valid HTML pages that include charset in Content-Type.

### Pitfall 3: Redirect to Internal IP (SSRF Bypass)
**What goes wrong:** Initial URL passes SSRF validation, but server redirects to `http://169.254.169.254/latest/meta-data/` (AWS metadata).
**Why it happens:** SSRF check only runs on the initial URL, not the redirect chain.
**How to avoid:** After `response = client.get(url).send().await`, check `response.url()` against SSRF before reading the body. This is D-17.
**Warning signs:** Works correctly for direct URLs but metadata endpoints are reachable via redirects.

### Pitfall 4: reqwest Default Redirect Policy
**What goes wrong:** reqwest follows up to 10 redirects by default. Some redirect chains are longer or circular.
**Why it happens:** Default `redirect::Policy::default()` allows 10 hops.
**How to avoid:** The default is fine for most cases (D-03 says use reqwest default). For circular redirects, reqwest returns an error automatically.
**Warning signs:** Timeouts on redirect loops (caught by 30s timeout, D-04).

### Pitfall 5: Large HTML Pages Causing OOM
**What goes wrong:** Fetching a massive page (e.g., a 50MB HTML file) exhausts memory before truncation kicks in.
**Why it happens:** `response.text().await` loads the entire body into memory. Truncation happens after.
**How to avoid:** Consider setting a max response body size. reqwest doesn't have a built-in limit, but you can stream the response and stop reading after a threshold (e.g., 5MB raw HTML). For v1, the 30s timeout provides a practical limit.
**Warning signs:** Memory spikes when fetching unusually large pages.

## Code Examples

### Complete WebReadTool Skeleton
```rust
// Source: Pattern derived from existing web_search.rs
use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use crate::registry::Tool;

pub struct WebReadTool;

#[async_trait]
impl Tool for WebReadTool {
    fn name(&self) -> &str { "web_read" }
    fn toolset(&self) -> &str { "web" }
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
                        "description": "The URL of the web page to read."
                    }
                },
                "required": ["url"]
            }),
        )
    }

    fn is_available(&self) -> bool {
        true // Always available (D-11)
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: url"))?;

        // 1. SSRF validation (D-16)
        // 2. Try Firecrawl scrape (D-02)
        // 3. On failure, try local fallback (D-02)
        // 4. Prepend header (D-06)
        // 5. Truncate (D-13, D-14, D-15)
        todo!()
    }
}
```

### WebConfig Extension
```rust
// Source: existing config.rs pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    pub backend: String,
    pub user_agent: String,
    pub max_content_chars: usize,
    pub timeout_secs: u64,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            backend: "firecrawl".to_string(),
            user_agent: "IronHermes/1.0 (+bot)".to_string(),
            max_content_chars: 50_000,
            timeout_secs: 30,
        }
    }
}
```

### Boilerplate Stripping with scraper
```rust
// Source: docs.rs/scraper
use scraper::{Html, Selector};

/// Selectors for boilerplate elements to exclude (D-08)
const BOILERPLATE_SELECTORS: &[&str] = &[
    "nav", "header", "footer", "aside",
    "[role=navigation]", "[role=banner]", "[role=contentinfo]",
    "script", "style", "noscript",
];

/// Content area selectors in priority order (D-01)
const CONTENT_SELECTORS: &[&str] = &[
    "article", "main", "[role=main]", "body",
];
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Firecrawl v1 API | Firecrawl v2 API (latest docs) | 2025 | v1 may still work; existing search uses v1. Use v1 for consistency, fallback handles failures |
| html2md crate | htmd crate | 2024 | htmd is actively maintained turndown.js port; html2md appears less maintained |
| Manual gzip handling | reqwest built-in compression features | reqwest 0.12 | Enable `gzip`/`brotli`/`deflate` cargo features for automatic handling |

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Firecrawl v1 scrape endpoint (`/v1/scrape`) is still available and functional | Architecture Patterns | LOW -- if v1 is deprecated, local fallback handles it; can switch to v2 endpoint trivially |
| A2 | htmd handles the HTML produced by scraper's `.html()` method correctly | Standard Stack | LOW -- both use standard HTML; if edge cases arise, pre-processing can fix them |
| A3 | 50KB default truncation limit is appropriate for LLM context windows | Common Pitfalls | LOW -- configurable via config.yaml; 50K chars is well under typical context limits |

## Open Questions

1. **Firecrawl v1 vs v2 scrape endpoint**
   - What we know: Existing `WebSearchTool` uses `/v1/search`. Firecrawl docs now show `/v2/scrape` as current.
   - What's unclear: Whether `/v1/scrape` still works or has been deprecated.
   - Recommendation: Try `/v1/scrape` first for consistency with existing code. If it fails (404), switch to `/v2/scrape`. The local fallback (D-02) handles API failures gracefully regardless.

2. **Config propagation to WebReadTool**
   - What we know: `WebSearchTool` currently creates a new `reqwest::Client` on every call and reads `FIRECRAWL_API_KEY` from env directly.
   - What's unclear: Whether `WebReadTool` should follow the same pattern or receive config at construction time.
   - Recommendation: Follow the same pattern as `WebSearchTool` for consistency. Config (user_agent, max_content_chars, timeout_secs) can be loaded from `Config::load()` inside `execute()`, matching the tool's stateless pattern. If this becomes a performance concern, refactor both tools later to accept config at construction.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| reqwest | HTTP fetching | Yes | 0.12.28 | -- |
| scraper | Local HTML fallback | No (new dep) | 0.26.0 | Must add to Cargo.toml |
| htmd | HTML-to-markdown conversion | No (new dep) | 0.5.4 | Must add to Cargo.toml |
| FIRECRAWL_API_KEY | Firecrawl scrape API | Runtime env var | -- | Local fallback (D-02) |

**Missing dependencies with no fallback:**
- None -- all missing crates are addable via Cargo.toml

**Missing dependencies with fallback:**
- `FIRECRAWL_API_KEY` at runtime -- local fallback provides full functionality without it (D-02)

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | -- |
| V3 Session Management | No | -- |
| V4 Access Control | No | -- |
| V5 Input Validation | Yes | SSRF validation via `is_safe_url()` + Content-Type check (D-07) |
| V6 Cryptography | No | -- |

### Known Threat Patterns for Web Scraping

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| SSRF via URL parameter | Tampering / Information Disclosure | `is_safe_url()` before every fetch (D-16) |
| SSRF via redirect chain | Tampering | Post-redirect URL re-validation (D-17) |
| Resource exhaustion (large pages) | Denial of Service | 30s timeout (D-04) + content truncation (D-15) |
| Cloud metadata access | Information Disclosure | Blocked hostnames in ssrf.rs (metadata.google.internal, metadata.goog) |

## Sources

### Primary (HIGH confidence)
- `crates/ironhermes-tools/src/web_search.rs` -- Firecrawl API auth pattern, response parsing
- `crates/ironhermes-core/src/ssrf.rs` -- SSRF validation implementation and async usage notes
- `crates/ironhermes-core/src/config.rs` -- WebConfig struct, Config loading pattern
- `crates/ironhermes-tools/src/registry.rs` -- Tool trait, register_defaults() pattern
- docs.rs/scraper/0.26.0 -- HTML parsing API (Html, Selector, ElementRef)
- docs.rs/htmd/0.5.4 -- `convert(html: &str) -> Result<String>` API
- docs.firecrawl.dev -- Scrape endpoint request/response format, markdown output

### Secondary (MEDIUM confidence)
- docs.rs/reqwest/0.12 -- Compression features (gzip, brotli, deflate), ClientBuilder config

### Tertiary (LOW confidence)
- Firecrawl v1 vs v2 endpoint availability -- could not confirm v1 scrape still works

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- all crates verified on crates.io, existing patterns in codebase
- Architecture: HIGH -- follows established WebSearchTool pattern exactly
- Pitfalls: HIGH -- well-understood domain (UTF-8 safety, SSRF, Content-Type parsing)
- Firecrawl API version: MEDIUM -- v1 assumed available based on existing search endpoint usage

**Research date:** 2026-04-07
**Valid until:** 2026-05-07 (stable domain, mature crates)
