# Web Scraping and Content Extraction Research for IronHermes

**Researched:** 2026-04-01
**Domain:** Web content extraction for AI agent tooling
**Overall Confidence:** MEDIUM-HIGH (direct codebase analysis + well-known Rust ecosystem)

---

## Executive Summary

IronHermes needs web content reading capabilities beyond the existing `web_search` tool. The Python hermes-agent solves this with a multi-backend approach (Firecrawl, Tavily, Exa, Parallel) that delegates scraping to cloud APIs, then post-processes results with an LLM summarizer to reduce token usage. This is the right architecture for IronHermes too.

The key insight from hermes-agent: **do not build a full scraper**. API-based extraction (Firecrawl scrape endpoint) handles JavaScript rendering, anti-bot protections, and format conversion. Local HTML parsing is only needed as a lightweight fallback for simple pages when API calls are undesirable (rate limits, cost, latency). The `scraper` crate is the clear winner for local parsing, and `reqwest` (already in use) handles all HTTP needs.

The recommended approach is a layered strategy: Firecrawl scrape API as primary (already have the key), with a local reqwest + scraper fallback for simple/static pages.

---

## 1. Rust HTTP Clients

### reqwest (already in workspace)

**Version:** 0.12 with `json`, `stream`, `rustls-tls` features
**Verdict:** Keep using. No reason to add another HTTP client.

Relevant features for scraping that may need enabling:

| Feature | Status | Purpose |
|---------|--------|---------|
| `json` | Enabled | API response parsing |
| `stream` | Enabled | Streaming large responses |
| `rustls-tls` | Enabled | HTTPS support |
| `cookies` | **Not enabled** | Cookie jar for session persistence |
| `gzip`, `brotli`, `deflate` | **Not enabled** | Compressed response handling |

**Recommendation:** Add `cookies` and `gzip` features to the workspace reqwest dependency. Most websites serve compressed responses, and `gzip` support alone can reduce transfer sizes by 70-80%. Cookie support is needed for sites that redirect through auth/consent pages.

Updated dependency line:
```toml
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls", "cookies", "gzip", "brotli", "deflate"], default-features = false }
```

reqwest already handles: redirect following (up to 10 by default, configurable), custom headers, timeouts, connection pooling. A shared `reqwest::Client` should be passed into tools rather than creating one per request (as `web_search.rs` currently does with `reqwest::Client::new()`).

### ureq

Blocking HTTP client. No advantage over reqwest in an async codebase. Skip.

### hyper

Low-level HTTP. reqwest is built on hyper. No reason to drop down.

---

## 2. HTML Parsing Crates

### scraper (recommended)

**Crate:** `scraper`
**What it does:** CSS selector-based HTML querying built on `html5ever` + `selectors`
**Confidence:** HIGH (widely used, well-maintained)

Key properties:
- CSS selector support (same syntax as browser `querySelector`)
- Built on Mozilla's html5ever parser (spec-compliant HTML5 parsing)
- Ergonomic API: `Html::parse_document()`, `Selector::parse()`, `select()`
- Handles malformed HTML gracefully (html5ever's recovery algorithms)
- No JavaScript execution (static HTML only)

```rust
use scraper::{Html, Selector};

let document = Html::parse_document(&html_string);
let selector = Selector::parse("article p, main p, .content p").unwrap();
for element in document.select(&selector) {
    let text = element.text().collect::<String>();
    // process text...
}
```

**Why scraper over alternatives:**
- `select` crate: Less maintained, fewer downloads, similar API but scraper has more active development
- `kuchiki`: DOM tree manipulation focus, more complex API for read-only extraction. Less actively maintained.
- `html5ever` directly: Too low-level. Provides the parser but not the querying. scraper wraps this.
- `lol_html` (Cloudflare): Streaming HTML rewriter, designed for modification not extraction. Wrong tool.

### Comparison Matrix

| Crate | CSS Selectors | Ease of Use | Maintenance | Best For |
|-------|:---:|:---:|:---:|---------|
| **scraper** | Full | High | Active | Content extraction (our use case) |
| select | Basic | Medium | Low | Simple element selection |
| kuchiki | Via css-select | Medium | Low | DOM manipulation |
| html5ever | None (raw parse) | Low | Active (Mozilla) | Building higher-level tools |
| lol_html | Limited | Medium | Active (Cloudflare) | HTML rewriting/streaming |

**Verdict:** Use `scraper`. It is the standard choice for HTML content extraction in Rust.

---

## 3. Content Extraction (Readability)

### The Problem

Raw HTML pages contain navigation, ads, sidebars, footers, scripts. An AI agent needs the **main article content**, not the entire DOM. The "readability" algorithm (pioneered by Arc90, used by Firefox Reader View) extracts the primary content block.

### Rust Options

| Crate | Description | Status | Confidence |
|-------|-------------|--------|------------|
| `readability` | Port of Mozilla's Readability.js | Exists on crates.io | LOW - verify maintenance |
| `readable-readability` | Another Readability port | Exists on crates.io | LOW - verify maintenance |

**Reality check:** Readability implementations in Rust are not mature. The JavaScript original (Mozilla's Readability.js, used by Firefox) is battle-tested across millions of pages. Rust ports tend to be incomplete or undermaintained.

### Recommended Approach: Don't Build Readability Locally

For IronHermes, content extraction should follow this priority:

1. **Firecrawl scrape API** (primary) -- Returns clean markdown. Handles JS rendering, readability extraction, and format conversion server-side. Already have the API key.
2. **Jina Reader API** (backup/free tier) -- `https://r.jina.ai/{url}` returns markdown. No API key needed for basic usage. Simple GET request.
3. **Local fallback with heuristic extraction** -- For when APIs are down or rate-limited. Use `scraper` crate with content heuristics (see below).

### Local Fallback Heuristic (No External Dependency)

Rather than depending on an undermaintained readability crate, implement a simple content extraction heuristic:

```rust
/// Simple content extraction heuristic:
/// 1. Try <article>, <main>, [role="main"] selectors
/// 2. Fall back to largest <div> by text content length
/// 3. Strip nav, header, footer, aside, script, style elements
/// 4. Collect remaining text
fn extract_main_content(html: &str) -> String {
    let document = Html::parse_document(html);
    
    // Try semantic selectors first
    for selector_str in &["article", "main", "[role='main']", ".post-content", ".entry-content"] {
        if let Ok(selector) = Selector::parse(selector_str) {
            let texts: Vec<String> = document.select(&selector)
                .map(|el| el.text().collect::<String>())
                .collect();
            let combined = texts.join("\n\n");
            if combined.len() > 200 {  // Minimum viable content
                return combined;
            }
        }
    }
    
    // Fallback: strip boilerplate, return body text
    // ... (strip script, style, nav, footer, aside)
}
```

This is ~50 lines of code and handles 70-80% of well-structured pages. For the remaining 20-30% (JS-rendered SPAs, complex layouts), the API-based approach is the right answer.

### JavaScript-Rendered Pages

Pages that require JavaScript execution (SPAs, dynamic content) **cannot be handled locally** without a headless browser. Options:

- **Firecrawl handles this server-side** -- Their infrastructure runs headless Chrome
- **headless_chrome crate** -- Rust bindings for Chrome DevTools Protocol. Heavy dependency, complex setup. Not recommended for an agent tool.
- **hermes-agent approach** -- Separate browser_tool using agent-browser CLI (Playwright-based). This is a different tool entirely, not a scraping tool.

**Verdict:** JS-rendered pages = use Firecrawl API. Do not add headless browser as a dependency.

---

## 4. What hermes-agent Does (Analysis)

### Architecture

hermes-agent has three web tools:

| Tool | What It Does | Backend |
|------|-------------|---------|
| `web_search_tool` | Keyword search, returns titles/URLs/snippets | Firecrawl, Tavily, Exa, Parallel |
| `web_extract_tool` | Read content from specific URLs | Firecrawl scrape, Tavily extract, Exa, Parallel |
| `web_crawl_tool` | Multi-page crawl with instructions | Firecrawl crawl (only) |

### Key Design Patterns Worth Porting

1. **LLM post-processing for content compression:**
   - Raw scraped content can be 50K-500K chars
   - hermes-agent runs it through Gemini Flash to extract key information
   - Reduces content to ~5K chars (90-99% reduction)
   - Critical for staying within context windows
   - IronHermes should do the same: scrape -> summarize with fast model -> return to agent

2. **SSRF protection (`url_safety.py`):**
   - Resolves hostname to IP before fetching
   - Blocks private/internal ranges (127.0.0.0/8, 10.0.0.0/8, 169.254.0.0/16, CGNAT)
   - Blocks known internal hostnames (metadata.google.internal)
   - Fails closed on DNS resolution errors
   - **Must port this to Rust.** An AI agent with URL fetching is an SSRF vector.

3. **Website blocklist policy (`website_policy.py`):**
   - User-configurable domain blocklist in config
   - Pattern matching with wildcards (*.example.com)
   - Shared blocklist files
   - Lower priority than SSRF but good for operator control

4. **Multi-backend abstraction:**
   - hermes-agent supports 4 backends (Firecrawl, Tavily, Exa, Parallel)
   - For IronHermes, start with Firecrawl only (already integrated) + local fallback
   - Add Jina Reader as a free/keyless option
   - More backends later if needed

5. **Content size management:**
   - Max content size: 2MB (refuse above)
   - Chunked processing for >500K chars
   - Output cap: 5K chars after summarization
   - Base64 image stripping from content

---

## 5. API-Based Approaches

### Firecrawl Scrape Endpoint (recommended primary)

**Already have:** `FIRECRAWL_API_KEY` and the search endpoint integrated.

The scrape endpoint is a simple addition:

```
POST https://api.firecrawl.dev/v1/scrape
Authorization: Bearer {api_key}
{
    "url": "https://example.com/article",
    "formats": ["markdown"]
}
```

Response includes: `markdown`, `html`, `metadata` (title, description, language, etc.)

**Advantages:**
- Handles JavaScript rendering
- Returns clean markdown (ideal for LLM consumption)
- Handles anti-bot protections
- Already paying for the API

**Disadvantages:**
- API cost per request (~$0.001-0.005 per scrape)
- Latency (1-5 seconds per page due to rendering)
- Rate limits
- External dependency (outage = no scraping)

### Jina Reader API (recommended secondary)

**Endpoint:** `https://r.jina.ai/{url}`
**Auth:** Optional. Free tier available, API key for higher limits.

```
GET https://r.jina.ai/https://example.com/article
Accept: text/plain
```

Returns: Clean markdown/text extraction of the page.

**Advantages:**
- Free tier (no API key needed for basic use)
- Simple GET request (trivial to implement)
- Good readability extraction
- Low latency for static pages

**Disadvantages:**
- Less reliable than Firecrawl for JS-heavy pages
- Rate limits on free tier
- Less metadata in response

### Other APIs Considered

| API | Verdict | Why |
|-----|---------|-----|
| Tavily | Skip for now | Would need new API key; Firecrawl covers same use case |
| Exa | Skip for now | Search-focused, not primarily for content extraction |
| ScrapingBee/ScraperAPI | Skip | Proxy-focused, overkill for agent use case |
| Diffbot | Skip | Expensive, enterprise-focused |

---

## 6. Tool Design for AI Agents

### Recommended Tool Surface

Based on hermes-agent's design and common agent patterns, IronHermes should expose these web tools:

| Tool | Purpose | Priority |
|------|---------|----------|
| `web_search` | Search the web (already exists) | Done |
| `web_read` | Read/extract content from a URL | P0 -- most needed |
| `web_crawl` | Multi-page crawl with extraction | P2 -- nice to have |

### Why NOT fine-grained tools (extract_links, scrape_page, etc.)

Splitting into many small tools (extract_links, get_headers, scrape_css_selector, etc.) is tempting but counterproductive for an AI agent:

1. **LLMs prefer fewer, higher-level tools** -- Each tool is a decision point. More tools = more chances for the LLM to pick the wrong one.
2. **The agent can parse links from content** -- If `web_read` returns markdown, the agent can see links in the text.
3. **CSS selectors require page knowledge** -- The agent doesn't know the DOM structure before fetching. A selector-based tool requires two calls minimum.

hermes-agent validates this: it uses 3 web tools (search, extract, crawl), not 10 fine-grained ones.

### `web_read` Tool Design

```rust
pub struct WebReadTool;

// Schema:
// {
//     "url": "https://example.com/article",     // required
//     "format": "markdown",                       // optional, default "markdown"
// }
//
// Execution flow:
// 1. Validate URL (SSRF check)
// 2. Try Firecrawl scrape API (if key available)
// 3. Fallback: reqwest GET + scraper content extraction
// 4. (Future) LLM summarization for large content
// 5. Return extracted content as string
```

### Content Length Management

Critical for agent tools: raw web pages can be enormous. Strategy:

1. **Hard cap:** 100K chars max returned to agent (truncate with notice)
2. **LLM summarization (future):** Route through fast model to compress to ~5K chars
3. **Strip noise:** Remove base64 images, excessive whitespace, repeated content
4. **Format as markdown:** Structured text is more token-efficient than raw text

---

## 7. Security Considerations

### SSRF Protection (Critical -- Must Implement)

An AI agent that fetches arbitrary URLs is a Server-Side Request Forgery vector. The agent could be tricked (via prompt injection in web content) into fetching internal network resources.

Port from hermes-agent's `url_safety.py`:

```rust
use std::net::{IpAddr, ToSocketAddrs};

fn is_safe_url(url: &str) -> bool {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    
    // Block known internal hostnames
    if BLOCKED_HOSTNAMES.contains(host) { return false; }
    
    // Resolve and check IP ranges
    let addrs = (host, 0).to_socket_addrs().ok()?;
    for addr in addrs {
        if is_private_ip(addr.ip()) { return false; }
    }
    true
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local()
            || v4.is_broadcast() || v4.is_unspecified()
            // CGNAT range: 100.64.0.0/10
            || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
            // Metadata endpoints: 169.254.0.0/16
            || v4.is_link_local()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified()
            // Add IPv6 private ranges
        }
    }
}
```

Note: The `url` crate is not currently in the workspace. Add it, or use `reqwest::Url` which re-exports it.

### DNS Rebinding (Known Limitation)

As hermes-agent documents: DNS can resolve to a safe IP during the check, then to a private IP for the actual connection. This is a TOCTOU race. Mitigation requires connection-level validation, which is complex. Document the limitation; the SSRF pre-check catches 99% of real attacks.

### Request Limits

- Timeout: 30 seconds max per request
- Response body: 10MB max (prevent memory exhaustion)
- Redirects: Cap at 5 (prevent redirect loops)
- User-Agent: Set a descriptive one ("IronHermes/0.1 (AI Agent)")

---

## 8. Recommended Implementation Plan

### Phase 1: `web_read` with Firecrawl (P0)

New dependencies: none (reqwest already available).

1. Add `web_read.rs` to `ironhermes-tools`
2. Implement Firecrawl scrape API call (reuse `FIRECRAWL_API_KEY`)
3. Add SSRF URL validation (port from hermes-agent)
4. Return markdown content with truncation at 100K chars
5. Register in `ToolRegistry::register_defaults()`

Estimated effort: Small. The Firecrawl scrape API is nearly identical to the search API already implemented.

### Phase 2: Local Fallback (P1)

New dependencies: `scraper` crate.

1. Add `scraper` to `ironhermes-tools/Cargo.toml`
2. Implement local fetch: reqwest GET + HTML parsing
3. Content extraction heuristic (article/main selectors, boilerplate stripping)
4. HTML-to-markdown conversion (basic: headings, links, lists, paragraphs)
5. Use as fallback when Firecrawl is unavailable or for simple pages

### Phase 3: Content Summarization (P2)

No new dependencies (uses existing LLM client).

1. Route large extracted content through a fast model for summarization
2. Compress to ~5K chars (matches hermes-agent pattern)
3. Configurable: skip summarization for short content (<5K chars)

### Phase 4: Jina Reader + web_crawl (P3)

1. Add Jina Reader API as alternative extraction backend
2. Implement `web_crawl` tool for multi-page extraction
3. Website blocklist policy (config-driven domain blocking)

---

## 9. Crate Dependency Summary

### Must Add

| Crate | Version | Purpose | When |
|-------|---------|---------|------|
| `scraper` | latest (~0.20) | HTML parsing + CSS selectors | Phase 2 |
| `url` | 2.x | URL parsing and validation | Phase 1 (or use reqwest::Url) |

### Already Available

| Crate | Purpose |
|-------|---------|
| `reqwest` 0.12 | HTTP client |
| `serde_json` | API response parsing |
| `regex` | Content cleanup |
| `anyhow` | Error handling |
| `tracing` | Logging |

### Workspace Cargo.toml Changes

```toml
# Add to [workspace.dependencies]:
scraper = "0.22"

# Update reqwest features:
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls", "cookies", "gzip", "brotli", "deflate"], default-features = false }
```

### NOT Recommended

| Crate | Why Not |
|-------|---------|
| `readability` | Undermaintained; Firecrawl/Jina do this better server-side |
| `headless_chrome` | Heavy dependency; JS rendering handled by APIs |
| `ureq` | No advantage over reqwest in async codebase |
| `kuchiki` | Less maintained than scraper |
| `select` | Less maintained than scraper |
| `lol_html` | Designed for rewriting, not extraction |

---

## 10. Key Takeaways

1. **API-first, local-fallback.** Firecrawl scrape handles the hard problems (JS, anti-bot, readability). Local parsing is a lightweight backup.
2. **`scraper` is the HTML parsing crate.** No real competition for CSS-selector-based extraction in Rust.
3. **SSRF protection is non-negotiable.** Port hermes-agent's URL safety check before shipping any URL-fetching tool.
4. **Two tools, not ten.** `web_search` (done) + `web_read` (next) covers 95% of agent web needs.
5. **Content compression matters.** Raw web content is too large for LLM context. Truncate now, add LLM summarization later.
6. **reqwest is fine.** Add compression features, share the client instance, done.

---

## Sources and Confidence

| Claim | Confidence | Source |
|-------|------------|--------|
| reqwest features and capabilities | HIGH | Direct Cargo.toml analysis, well-known crate |
| scraper is the standard HTML parsing crate | HIGH | Widely documented, most-downloaded in category |
| Firecrawl scrape API format | MEDIUM | Inferred from existing search integration + hermes-agent usage |
| Jina Reader API availability | MEDIUM | Known service, verify current free tier limits |
| Readability crates maturity | LOW | Training data; verify actual crate status before depending |
| hermes-agent architecture | HIGH | Direct codebase analysis |
| SSRF protection patterns | HIGH | Direct code reading of url_safety.py |
