# Phase 4: Web Scraping Tools - Context

**Gathered:** 2026-04-07
**Status:** Ready for planning

<domain>
## Phase Boundary

Agent can fetch and read web page content via a `web_read` tool, with Firecrawl scrape API as primary backend and local HTML fallback via `scraper` crate. SSRF protection (built in Phase 3) validates all URLs before fetching. Content is returned as markdown with smart truncation for context-window safety.

</domain>

<decisions>
## Implementation Decisions

### Fallback strategy
- **D-01:** Local fallback uses `scraper` crate with semantic selector heuristic (`<article>`, `<main>`, `[role=main]`, then `<body>`). ~50-line extractor with boilerplate stripping.
- **D-02:** Fallback activates in both cases: no `FIRECRAWL_API_KEY` configured OR Firecrawl request fails (500, timeout, network error). Maximum availability.
- **D-03:** Follow HTTP redirects up to reqwest default limit. Re-validate the final URL against SSRF before fetching content (redirect-to-internal attack prevention).
- **D-04:** 30-second timeout for HTTP requests (both Firecrawl and local fallback).

### Content extraction
- **D-05:** Return markdown format. Firecrawl already returns markdown; local fallback converts HTML to markdown (headings to `#`, links to `[text](url)`, lists to `- items`).
- **D-06:** Prepend `# {title}\nSource: {url}\n\n` header before content. Gives LLM attribution context.
- **D-07:** HTML only — check `Content-Type` header. If not `text/html`, return error: "web_read only supports HTML pages. Got: {content_type}". Keep scope minimal for v1.
- **D-08:** Strip common boilerplate elements: `<nav>`, `<header>`, `<footer>`, `<aside>`, `[role=navigation]`, `[role=banner]`, `[role=contentinfo]`. Focus on main content area.

### Tool UX
- **D-09:** Single required parameter: `url`. No optional selector, format, or other params. LLMs prefer fewer decision points (per ROADMAP).
- **D-10:** Descriptive error strings for all failure modes: "URL blocked by security policy (private IP)", "Page returned HTTP 404", "Request timed out after 30s", "web_read only supports HTML pages".
- **D-11:** Tool is always registered and available (`is_available()` returns `true`). If fetch fails at runtime, return descriptive error — don't hide the tool.
- **D-12:** Configurable User-Agent in `config.yaml` (`web.user_agent`). Default: `IronHermes/1.0 (+bot)`. Users can override with a browser-like UA if needed for compatibility.

### Truncation
- **D-13:** Smart boundary truncation: cut at nearest paragraph break (`\n\n`) or sentence break (`. `) before the limit. Never break mid-word.
- **D-14:** Append `\n\n[Content truncated at {displayed_chars} of {total_chars} characters]` notice when truncation occurs.
- **D-15:** Configurable limit via `web.max_content_chars` in `config.yaml`. Default: 50,000 characters.

### SSRF integration
- **D-16:** `is_safe_url()` from `ironhermes-core::ssrf` runs before every fetch. Wrap with `tokio::task::spawn_blocking()` for async context (per ssrf.rs doc comment).
- **D-17:** SSRF validation runs on initial URL AND on final URL after redirects (D-03).

### Claude's Discretion
- HTML-to-markdown conversion approach (htmd crate, custom converter, or scraper + manual conversion)
- Exact semantic selector priority order for content extraction
- reqwest client configuration details (compression features, connection pooling)
- Whether to add gzip/brotli/deflate features to reqwest (ROADMAP suggests yes)
- Error message wording details

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements and architecture
- `.planning/REQUIREMENTS.md` -- WEB-01 through WEB-04 requirements
- `.planning/ROADMAP.md` -- Phase 4 key technical decisions, success criteria, estimated complexity
- `.planning/codebase/ARCH.md` -- Crate dependency graph, module structure

### Existing IronHermes code
- `crates/ironhermes-core/src/ssrf.rs` -- SSRF validator (`is_safe_url()`). Synchronous DNS resolution; async callers must use `spawn_blocking`. Phase 4 handles the async wrapping.
- `crates/ironhermes-tools/src/web_search.rs` -- `WebSearchTool` using Firecrawl `/v1/search` API. Pattern reference for Firecrawl authentication, error handling, response parsing.
- `crates/ironhermes-tools/src/registry.rs` -- `ToolRegistry` with `register_defaults()`. New `WebReadTool` registers here.
- `crates/ironhermes-core/src/config.rs` -- `Config` struct with `WebConfig`. Extend with `user_agent`, `max_content_chars`, `timeout_secs`.

### Python reference
- `/Users/twilson/code/hermes-agent/tools/url_safety.py` -- SSRF reference (already ported in Phase 3)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `WebSearchTool`: Firecrawl API auth pattern (bearer token from `FIRECRAWL_API_KEY` env var), error handling, response parsing — reuse for scrape endpoint
- `is_safe_url()`: SSRF validator in `ironhermes-core` — call before every fetch
- `reqwest::Client`: Already a dependency, used by `WebSearchTool` — reuse for local fallback HTTP requests
- `WebConfig`: Existing config struct with `backend` field — extend with new fields
- `ToolSchema::new()`: Tool definition pattern — follow for `web_read` schema

### Established Patterns
- Tool results as plain `String` — `web_read` returns markdown string, errors as `anyhow::Result`
- `is_available()` on tools — `WebSearchTool` checks for API key; `WebReadTool` always returns `true`
- Firecrawl API uses bearer auth, JSON request/response — scrape endpoint follows same pattern
- `register_defaults()` registers all built-in tools — add `WebReadTool` here

### Integration Points
- `register_defaults()` in `registry.rs` — register `WebReadTool`
- `WebConfig` in `config.rs` — add `user_agent`, `max_content_chars`, `timeout_secs` fields
- Cargo.toml for `ironhermes-tools` — add `scraper` crate dependency
- Cargo.toml for `ironhermes-tools` — potentially add HTML-to-markdown crate

</code_context>

<specifics>
## Specific Ideas

- Firecrawl scrape API (`/v1/scrape`) is nearly identical to the already-integrated search API — same auth, same response shape with `markdown` field
- Local fallback is a safety net, not the primary path — keep it simple, don't over-engineer
- SSRF re-validation after redirects is important — a redirect from `example.com` to `192.168.1.1` must be caught
- User-Agent configurability addresses real-world sites that block bot UAs without adding complexity

</specifics>

<deferred>
## Deferred Ideas

- **Cloudflare Browser Rendering** (`/crawl` endpoint) — JS-rendering backend for dynamic pages. Would be a new capability beyond static HTML scraping. Consider for WEB-05/WEB-06.
- **cmux** (cmux.com) — Browser automation / agent browser service. Potential future backend for complex scraping scenarios.
- **agent-browser** (vercel-labs/agent-browser) — AI-native browser for agent use. Another JS-rendering option for future phases.
- These three options address the same gap: JavaScript-rendered pages that static HTML extraction can't handle. Worth evaluating together in a future "Advanced Web Tools" phase.

</deferred>

---

*Phase: 04-web-scraping-tools*
*Context gathered: 2026-04-07*
