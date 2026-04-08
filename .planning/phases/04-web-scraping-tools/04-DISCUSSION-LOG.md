# Phase 4: Web Scraping Tools - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-07
**Phase:** 04-web-scraping-tools
**Areas discussed:** Fallback strategy, Content extraction, Tool UX, Truncation behavior

---

## Fallback Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Basic HTML extraction | reqwest GET + scraper crate: semantic selector heuristic, static HTML only | ✓ |
| reqwest + readability port | Rust readability algorithm for smarter extraction, heavier dependency | |
| No local fallback | Firecrawl-only, return error if unavailable | |

**User's choice:** Basic HTML extraction
**Notes:** None

### Follow-up: Fallback activation

| Option | Description | Selected |
|--------|-------------|----------|
| Both (no API key + Firecrawl failure) | Maximum availability — fallback on missing key or runtime failure | ✓ |
| No API key only | Fallback only when FIRECRAWL_API_KEY missing | |

**User's choice:** Both
**Notes:** None

### Follow-up: Redirect handling

| Option | Description | Selected |
|--------|-------------|----------|
| Follow redirects | Follow HTTP redirects, re-validate final URL against SSRF | ✓ |
| No redirects | Reject any redirect | |

**User's choice:** Follow redirects
**Notes:** None

### Follow-up: Timeout

| Option | Description | Selected |
|--------|-------------|----------|
| 30 seconds | Reasonable timeout for most pages | ✓ |
| 10 seconds | Aggressive timeout | |
| Configurable | Add web.timeout_secs to config | |

**User's choice:** 30 seconds
**Notes:** None

---

## Content Extraction

| Option | Description | Selected |
|--------|-------------|----------|
| Markdown | Return markdown text, Firecrawl returns this natively | ✓ |
| Plain text | Strip all formatting | |
| Structured JSON | { title, description, content, url } | |

**User's choice:** Markdown
**Notes:** None

### Follow-up: Metadata

| Option | Description | Selected |
|--------|-------------|----------|
| Title + URL header | Prepend "# {title}\nSource: {url}\n\n" | ✓ |
| Content only | Just extracted text | |

**User's choice:** Title + URL header
**Notes:** None

### Follow-up: Non-HTML content

| Option | Description | Selected |
|--------|-------------|----------|
| HTML only, reject others | Check Content-Type, error on non-HTML | ✓ |
| HTML + plain text | Accept text/html and text/plain | |
| Best effort | Try to extract text from anything | |

**User's choice:** HTML only, reject others
**Notes:** None

### Follow-up: HTML-to-markdown conversion

| Option | Description | Selected |
|--------|-------------|----------|
| Basic text extraction | Extract inner text, preserve paragraph breaks, no markdown formatting | |
| HTML to markdown | Convert headings, links, lists to markdown syntax | ✓ |

**User's choice:** HTML to markdown
**Notes:** None

### Follow-up: Boilerplate stripping

| Option | Description | Selected |
|--------|-------------|----------|
| Strip common boilerplate | Remove nav, header, footer, aside, role=navigation/banner/contentinfo | ✓ |
| Return everything | All text from body | |

**User's choice:** Strip common boilerplate
**Notes:** None

---

## Tool UX

| Option | Description | Selected |
|--------|-------------|----------|
| URL only | Single required param, fewer LLM decision points | ✓ |
| URL + optional selector | url + CSS selector for targeting sections | |
| URL + options | url + format, selector, include_links | |

**User's choice:** URL only
**Notes:** None

### Follow-up: Error reporting

| Option | Description | Selected |
|--------|-------------|----------|
| Descriptive error strings | Clear messages like "URL blocked by security policy" | ✓ |
| Error codes + messages | Structured JSON with error code | |

**User's choice:** Descriptive error strings
**Notes:** None

### Follow-up: Availability

| Option | Description | Selected |
|--------|-------------|----------|
| Always available | Tool always registered, errors at runtime | ✓ |
| Conditional availability | Hide tool when no internet | |

**User's choice:** Always available
**Notes:** None

### Follow-up: User-Agent

| Option | Description | Selected |
|--------|-------------|----------|
| Identify as IronHermes | Send "IronHermes/1.0 (+bot)" | Default ✓ |
| Browser-like UA | Chrome-like User-Agent string | Configurable option |

**User's choice:** Both as configurable option, defaults to IronHermes/1.0
**Notes:** User wants both options available via config.yaml (`web.user_agent`), defaulting to honest identification.

---

## Truncation Behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Smart boundary | Cut at nearest paragraph/sentence break before limit | ✓ |
| Hard cut | Cut at exact character limit | |

**User's choice:** Smart boundary
**Notes:** None

### Follow-up: Configurable limit

| Option | Description | Selected |
|--------|-------------|----------|
| Configurable with 50K default | web.max_content_chars in config.yaml | ✓ |
| Fixed at 50K | Hardcoded constant | |

**User's choice:** Configurable with 50K default
**Notes:** None

---

## Claude's Discretion

- HTML-to-markdown conversion approach (crate choice or custom)
- Exact semantic selector priority order
- reqwest client configuration (compression features, connection pooling)
- Error message wording details

## Deferred Ideas

- Cloudflare Browser Rendering (`/crawl` endpoint) — JS rendering for dynamic pages
- cmux (cmux.com) — browser automation / agent browser service
- agent-browser (vercel-labs/agent-browser) — AI-native browser
- All three address JS-rendered page scraping — evaluate together in future phase
