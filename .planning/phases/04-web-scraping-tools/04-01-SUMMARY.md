---
phase: 04-web-scraping-tools
plan: "01"
subsystem: tools
tags: [web-scraping, ssrf, firecrawl, htmd, scraper, reqwest]
dependency_graph:
  requires: [ironhermes-core/ssrf, ironhermes-core/config, ironhermes-tools/registry]
  provides: [WebReadTool, web_read tool]
  affects: [ironhermes-tools/registry, ironhermes-tools/lib]
tech_stack:
  added: [scraper 0.26, htmd 0.5, reqwest gzip/brotli/deflate]
  patterns: [Firecrawl API primary + local HTML fallback, spawn_blocking SSRF, smart truncation by char_indices]
key_files:
  created:
    - crates/ironhermes-tools/src/web_read.rs
  modified:
    - Cargo.toml
    - crates/ironhermes-tools/Cargo.toml
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-tools/src/lib.rs
    - crates/ironhermes-tools/src/registry.rs
decisions:
  - "WebConfig extended with user_agent, max_content_chars, timeout_secs — all serde(default) for backward compat"
  - "is_available() returns true unconditionally — Firecrawl fallback means tool is always usable"
  - "Boilerplate stripping done via re-parse and string replacement — scraper has no DOM mutation API"
  - "Post-redirect SSRF check compares response.url() string to original — catches any redirect chain endpoint"
metrics:
  duration: "~2 minutes"
  completed_date: "2026-04-08"
  tasks_completed: 2
  files_changed: 5
requirements: [WEB-01, WEB-02, WEB-03, WEB-04]
---

# Phase 04 Plan 01: Web Read Tool — Summary

**One-liner:** WebReadTool with Firecrawl primary and local scraper+htmd fallback, SSRF validation via spawn_blocking, post-redirect re-validation, Content-Type gating, and smart char-boundary truncation at 50K chars.

## What Was Built

`WebReadTool` is a new tool in `ironhermes-tools` that fetches any public web page and returns its content as markdown. It implements all 17 context decisions (D-01 through D-17) from the phase research.

### Architecture

1. **Firecrawl path (primary):** If `FIRECRAWL_API_KEY` is set, POST to `https://api.firecrawl.dev/v1/scrape` with `{"url": url, "formats": ["markdown"]}`. Returns pre-converted markdown. On any error, falls through to local.

2. **Local path (fallback):** Uses `reqwest` with config-driven `user_agent` and `timeout_secs`. Follows redirects (default policy). Checks final URL for post-redirect SSRF. Rejects non-`text/html` Content-Type. Parses HTML with `scraper`, selects content area (article > main > [role=main] > body), strips boilerplate (nav/header/footer/aside/script/style/noscript), converts to markdown via `htmd`.

3. **SSRF protection:** Every fetch (both paths) is preceded by `validate_url_async()` which wraps `ironhermes_core::ssrf::is_safe_url` in `tokio::task::spawn_blocking`. After redirects, the final URL is re-validated.

4. **Smart truncation:** `truncate_content()` uses `char_indices()` to find the UTF-8-safe byte offset, then searches backward for paragraph break > sentence break > word break > hard cut. Appends `[Content truncated at N of M characters]` notice.

5. **Config extension:** `WebConfig` gains `user_agent`, `max_content_chars`, `timeout_secs` — all with `serde(default)` so existing config files remain valid.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add dependencies and extend WebConfig | 4b5e017 | Cargo.toml, crates/ironhermes-tools/Cargo.toml, crates/ironhermes-core/src/config.rs |
| 2 | Implement WebReadTool with Firecrawl + local fallback + SSRF + truncation | d20356b | crates/ironhermes-tools/src/web_read.rs, src/lib.rs, src/registry.rs |

## Verification

- `cargo check --workspace` exits 0
- `cargo test -p ironhermes-tools` — 20 passed, 0 failed
- All 16 acceptance criteria patterns verified via grep

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None. WebReadTool is fully wired: Firecrawl calls real API, local fallback does real HTTP+HTML parsing, SSRF validation uses the real `is_safe_url` from ironhermes-core.

## Threat Flags

No new threat surface beyond what the plan's threat model covers. All T-04-01 through T-04-04 mitigations implemented. T-04-05 (DNS rebinding) accepted per plan.

## Self-Check: PASSED

- crates/ironhermes-tools/src/web_read.rs — FOUND
- crates/ironhermes-core/src/config.rs (max_content_chars) — FOUND
- Commit 4b5e017 — FOUND
- Commit d20356b — FOUND
