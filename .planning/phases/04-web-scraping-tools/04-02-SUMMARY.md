---
phase: 04-web-scraping-tools
plan: "02"
subsystem: tools
tags: [testing, web-scraping, truncation, html-extraction, utf8]
dependency_graph:
  requires: [ironhermes-tools/web_read]
  provides: [web_read unit tests]
  affects: [ironhermes-tools/web_read.rs]
tech_stack:
  added: []
  patterns: [#[cfg(test)] mod tests inline with implementation, char_indices-based UTF-8 boundary assertions]
key_files:
  created: []
  modified:
    - crates/ironhermes-tools/src/web_read.rs
decisions:
  - "test_truncate_sentence_break uses simple AAAA./BBBB. input to make rfind('. ') deterministic and assertions exact"
  - "test_truncate_word_break uses 'one two three' with limit 8 to confirm rfind(' ') at index 7 gives trimmed 'one two'"
  - "Added test_extract_no_title_uses_source_only beyond plan spec to cover empty-title branch in extract_content_local"
metrics:
  duration: "~3 minutes"
  completed_date: "2026-04-08"
  tasks_completed: 2
  files_changed: 1
requirements: [WEB-01, WEB-02, WEB-03, WEB-04]
---

# Phase 04 Plan 02: Web Read Tool Unit Tests — Summary

**One-liner:** 16 unit tests for truncate_content (boundary priority + UTF-8 safety + char-count notice) and extract_content_local (selector priority + boilerplate stripping + header format + markdown conversion).

## What Was Built

Added a `#[cfg(test)] mod tests` block to `crates/ironhermes-tools/src/web_read.rs` with comprehensive unit tests for the two pure functions introduced in Plan 01.

### truncate_content tests (9 tests)

| Test | What it verifies |
|------|-----------------|
| test_truncate_under_limit | Content shorter than limit returns unchanged, no truncation notice |
| test_truncate_at_limit | Content exactly at limit returns unchanged |
| test_truncate_paragraph_break | rfind("\n\n") is preferred — cut drops content after second paragraph |
| test_truncate_sentence_break | rfind(". ") fires when no paragraph break — exact trimmed assertion |
| test_truncate_word_break | rfind(' ') fires when no sentence break — space excluded from trimmed |
| test_truncate_no_whitespace | Hard cut at char limit when no break point exists |
| test_truncate_utf8_emoji | Multi-byte emoji (4 bytes/1 char) does not panic; char boundary respected |
| test_truncate_notice_char_counts | Notice format "of N characters]" uses char count not byte count |
| test_truncate_notice_displayed_chars | Notice "at N of M" matches actual trimmed char length |

### extract_content_local tests (7 tests)

| Test | What it verifies |
|------|-----------------|
| test_extract_selects_article | article > body selector priority; header format "# Title\nSource: url" |
| test_extract_selects_main_over_body | main > body when no article present |
| test_extract_falls_back_to_body | body fallback when no semantic container |
| test_extract_strips_boilerplate | nav/header/footer/aside stripped from output |
| test_extract_prepends_header | Exact "# Title\nSource: url\n\n" prefix format (D-06) |
| test_extract_converts_to_markdown | htmd converts h2 to ## and links to [text](url) |
| test_extract_no_title_uses_source_only | Empty title falls back to "Source: url\n\n" without "#" prefix |

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Unit tests for truncate_content | db14b0f | crates/ironhermes-tools/src/web_read.rs |
| 2 | Unit tests for extract_content_local | db14b0f | crates/ironhermes-tools/src/web_read.rs |

(Both tasks committed in a single atomic commit since the tests are in the same file and both verified green before committing.)

## Verification

- `cargo test -p ironhermes-tools -- truncate_` — 9 passed, 0 failed
- `cargo test -p ironhermes-tools -- extract_` — 7 passed, 0 failed
- `cargo test -p ironhermes-tools` — 36 passed, 0 failed (no regressions)

## Deviations from Plan

**1. [Rule 2 - Missing Critical Functionality] Added test_extract_no_title_uses_source_only**
- **Found during:** Task 2
- **Issue:** The plan spec listed 6 extraction tests but the empty-title branch in extract_content_local (which outputs "Source: url\n\n" instead of "# title\nSource: url\n\n") had no test coverage
- **Fix:** Added a 7th extraction test covering the no-title path
- **Files modified:** crates/ironhermes-tools/src/web_read.rs
- **Commit:** db14b0f

**2. [Rule 1 - Accuracy] Simplified sentence/word break test inputs**
- **Found during:** Task 1
- **Issue:** Plan's suggested test content for sentence/word break tests had ambiguous rfind results (multiple ". " occurrences within the limit window), making assertions uncertain
- **Fix:** Used minimal, deterministic inputs (AAAA./BBBB. and "one two three") with exact trimmed assertions computed from the implementation's boundary logic
- **Commit:** db14b0f

## Known Stubs

None. Tests are fully wired to the real implementation functions.

## Threat Flags

No new threat surface introduced. Tests exercise existing functions with controlled HTML inputs; no new network endpoints or auth paths created.

## Self-Check: PASSED

- crates/ironhermes-tools/src/web_read.rs contains #[cfg(test)] — FOUND (line 307)
- crates/ironhermes-tools/src/web_read.rs contains mod tests — FOUND (line 308)
- 9 truncation test functions — FOUND (lines 314-409)
- 7 extraction test functions — FOUND (lines 411-488)
- Commit db14b0f — FOUND
- cargo test -p ironhermes-tools exits 0 with 36 tests — VERIFIED
