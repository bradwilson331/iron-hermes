//! OSC8 hyperlink detection — the `http(s)`-only extraction filter (Phase
//! 36.6.4 Plan 04, TUI-LINK-01, D-14/D-15).
//!
//! Scope is exactly D-14: a bare `http://`/`https://` URL, or a markdown
//! `[label](target)` link whose LABEL is the visible span (the surrounding
//! `[`/`]`/`(target)` characters stay in the rendered text exactly as the
//! model emitted them — this codebase has no markdown-to-plain rendering
//! pass, so what's typed is what's on screen either way; only the label
//! substring additionally gets the Cyan+Underlined+OSC8 treatment when it
//! qualifies). File paths are explicitly out of scope (D-14).
//!
//! ## The scheme restriction is an INPUT FILTER, not a post-hoc check
//! RESEARCH Open Question 2 (adopted as binding by UI-SPEC §4) requires
//! that the markdown grammar can only ever match a target that is already
//! `http(s)`-schemed — a `javascript:`/`file://` target must never become a
//! link candidate that then gets rejected downstream; it must never match
//! the grammar in the first place. This mirrors the codebase's existing
//! `artifact_browser_url` discipline (`app.rs`) of never trusting an
//! arbitrary model-supplied string as a URL directly.
//!
//! ## Two-pass extraction (why not one combined regex)
//! Extraction runs in two passes rather than one alternation regex:
//!
//! 1. First, find every `[label](target)`-SHAPED span (`MD_SHAPE_RE`,
//!    deliberately permissive — `label`/`target` may be empty) and record
//!    its FULL byte range as "reserved" — text the model wrote as markdown
//!    link syntax, valid or not. A span only becomes a `Link` when its
//!    label is non-empty AND its target is a well-formed `http(s)` URL
//!    (`valid_http_target`); an empty label (`[]`) or a non-`http(s)`/
//!    hostless target is excluded, never becomes a link, per D-14/UI-SPEC
//!    §4.
//! 2. Then scan for bare `http(s)://` URLs (`BARE_URL_RE`), SKIPPING any
//!    match that overlaps a reserved span from pass 1.
//!
//! Pass 2's skip is the reason a single combined alternation regex is not
//! enough: `[](https://example.com)` must produce ZERO links — not a
//! rejected markdown match that then falls back to matching the SAME
//! `https://example.com` substring as an independent bare URL. A combined
//! regex has no way to remember "this URL text already failed the
//! markdown grammar, don't re-offer it to the bare-URL grammar" — the two
//! explicit passes do.
//!
//! A hostless scheme (`http://` with no host character, or `http:///path`)
//! is excluded by `BARE_URL_RE`'s/`valid_http_target`'s character classes
//! themselves, not a length check afterward: `[^\s)\]/][^\s)\]]*` requires
//! the FIRST character after `://` to be neither whitespace nor `/` (so
//! `http://` at end-of-text, `http:// `, and `http:///path` all fail to
//! match at all).
//!
//! ## Units: display cells, not chars or bytes
//! `Link::start_cell`/`end_cell` are measured via the crate's
//! `unicode-width` helpers (the same crate `App::transcript_rendered_plain_
//! rows`/`word_wrapped_line_count` already depend on) because Task 2's
//! render-time embedding writes directly into `ratatui::buffer::Buffer`
//! cells, which are addressed by DISPLAY COLUMN — not by `char` index (the
//! unit `selection.rs`'s `ContentPos` uses for its own, unrelated,
//! coordinate model; see that module's doc comment for why selection uses
//! a different unit).

use std::sync::LazyLock;

use ratatui::buffer::{Buffer, Cell, CellDiffOption};
use ratatui::style::{Color, Modifier, Style};
use regex::Regex;
use unicode_width::UnicodeWidthStr;

/// A link detected within a single rendered transcript row's PLAIN text
/// (i.e. one entry of `App::transcript_rendered_plain_rows`'s output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The visible span's text: the label for a markdown link, or the URL
    /// itself for a bare URL (there is no separate label in that case).
    pub label: String,
    /// The `http(s)` target this link's OSC8 escape points at.
    pub url: String,
    /// Display-cell offset (inclusive) of the first cell of `label` within
    /// the row.
    pub start_cell: usize,
    /// Display-cell offset (EXCLUSIVE) one past the last cell of `label`.
    pub end_cell: usize,
}

/// Pass 1: every `[label](target)`-SHAPED span, deliberately permissive —
/// `label`/`target` may each be EMPTY. Validity (non-empty label, `http(s)`
/// target) is checked separately by `valid_http_target` so a REJECTED match
/// still gets its full byte range recorded as "reserved" (module doc,
/// "Two-pass extraction").
static MD_SHAPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?P<label>[^\]]*)\]\((?P<target>[^\s)]*)\)")
        .expect("static markdown-shape grammar must compile")
});

/// Pass 2: a bare `http(s)://` URL. The first character after `://` must be
/// neither whitespace nor `/` — this is what rejects a hostless scheme
/// (`http://`, `http:///path`) at the grammar level (module doc).
static BARE_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://[^\s)\]/][^\s)\]]*").expect("static bare-URL grammar must compile")
});

/// Full-string validity check for a markdown link's captured `target`
/// group — reuses `BARE_URL_RE`'s exact character classes, anchored, so a
/// target is only accepted when the ENTIRE captured text (not just a
/// prefix of it) is a well-formed `http(s)` URL.
static VALID_TARGET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https?://[^\s)\]/][^\s)\]]*$").expect("static target-validity grammar must compile")
});

fn valid_http_target(target: &str) -> bool {
    VALID_TARGET_RE.is_match(target)
}

/// Extract every `http(s)`-scoped link from a single rendered transcript
/// row's PLAIN text (Phase 36.6.4 Plan 04, D-14). Pure and free-standing —
/// no `App`, no terminal — following the same factoring discipline that
/// put `chip_action_at` (`app.rs`) outside the mouse dispatch body. See the
/// module doc "Two-pass extraction" for why this is two scans, not one
/// combined regex.
pub fn extract_links(row_text: &str) -> Vec<Link> {
    let mut links = Vec::new();
    let mut reserved: Vec<(usize, usize)> = Vec::new();

    for caps in MD_SHAPE_RE.captures_iter(row_text) {
        let whole = caps.get(0).expect("capture 0 is always the whole match");
        reserved.push((whole.start(), whole.end()));

        let label_m = caps.name("label").expect("named group always present on a match");
        let target_m = caps.name("target").expect("named group always present on a match");
        if label_m.as_str().is_empty() || !valid_http_target(target_m.as_str()) {
            continue;
        }
        let start_cell = UnicodeWidthStr::width(&row_text[..label_m.start()]);
        let end_cell = start_cell + UnicodeWidthStr::width(label_m.as_str());
        links.push(Link {
            label: label_m.as_str().to_string(),
            url: target_m.as_str().to_string(),
            start_cell,
            end_cell,
        });
    }

    for m in BARE_URL_RE.find_iter(row_text) {
        let overlaps_reserved = reserved.iter().any(|&(s, e)| m.start() < e && m.end() > s);
        if overlaps_reserved {
            continue;
        }
        let start_cell = UnicodeWidthStr::width(&row_text[..m.start()]);
        let end_cell = start_cell + UnicodeWidthStr::width(m.as_str());
        links.push(Link {
            label: m.as_str().to_string(),
            url: m.as_str().to_string(),
            start_cell,
            end_cell,
        });
    }

    // Pass 1 (markdown) and pass 2 (bare URL) are each individually in
    // left-to-right row order, but interleaved links from the two passes
    // are not — restore overall row order so callers (Task 2's render-time
    // embedding) can walk `links` linearly against the row's cells.
    links.sort_by_key(|l| l.start_cell);
    links
}

// ── OSC8 cell embedding (Phase 36.6.4 Plan 04 Task 2, D-15) ─────────────────

/// OSC8 open-sequence prefix; the target follows, terminated by ST.
const OSC8_OPEN_PREFIX: &str = "\x1b]8;;";
/// String Terminator, closing the open sequence's target.
const OSC8_ST: &str = "\x1b\\";
/// OSC8 close sequence — an empty-target open immediately re-terminated.
const OSC8_CLOSE: &str = "\x1b]8;;\x1b\\";

/// Apply the unconditional Cyan+Underlined visible affordance (UI-SPEC §4)
/// to ONE cell, and — for the first and/or last cell of a link's span —
/// additionally prepend/append the real OSC8 escape bytes to that cell's
/// symbol, marking it `CellDiffOption::ForcedWidth` at its OWN true display
/// width (RESEARCH Pattern 1: the first-class `Cell` API ratatui's
/// maintainers ship for exactly this problem, so the escape-inflated symbol
/// string never miscounts in the buffer's own diff pass).
///
/// `Cell::set_style` INSERTS `add_modifier` bits without touching fg/bg it
/// doesn't set (ratatui-core semantics, same discipline `ui.rs::apply_
/// selection_highlight`'s doc comment relies on) — this styling call is the
/// BASE layer a later selection-highlight OR-Reversed pass composes onto
/// (UI-SPEC §2's OR-not-replace rule), never the other way around.
///
/// `is_first`/`is_last` are computed by the caller (`ui.rs::apply_
/// hyperlinks`, mirroring `apply_selection_highlight`'s own viewport-
/// clamped range walk) against whatever span is ACTUALLY reachable on this
/// render — a link entirely detected within one already-wrapped
/// `App::transcript_rendered_plain_rows` row can never exceed that row's
/// own width, so the "last cell actually displayed" a narrow-terminal
/// truncation produces (UI-SPEC §4) is simply the caller's own
/// `end_cell - 1`, with no separate detection needed here.
pub fn embed_osc8_cell(cell: &mut Cell, url: &str, is_first: bool, is_last: bool) {
    cell.set_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::UNDERLINED));
    if !is_first && !is_last {
        return;
    }
    let display_w = UnicodeWidthStr::width(cell.symbol()).max(1) as u16;
    let mut symbol = String::new();
    if is_first {
        symbol.push_str(OSC8_OPEN_PREFIX);
        symbol.push_str(url);
        symbol.push_str(OSC8_ST);
    }
    symbol.push_str(cell.symbol());
    if is_last {
        symbol.push_str(OSC8_CLOSE);
    }
    cell.set_symbol(&symbol);
    if let Some(w) = std::num::NonZeroU16::new(display_w) {
        cell.set_diff_option(CellDiffOption::ForcedWidth(w));
    }
}

/// Embed one link's visible span into `buf` at buffer row `vy`, over the
/// HALF-OPEN viewport-column range `[start_vx, end_vx)` — the caller is
/// responsible for clamping content-cell math to the live viewport/scroll
/// offset before calling this (exactly as `ui.rs::apply_selection_
/// highlight` already clamps the selection range). A no-op when the range
/// is empty (`start_vx >= end_vx`), which happens when a link scrolled
/// fully outside the current viewport.
pub fn embed_osc8(buf: &mut Buffer, vy: u16, start_vx: u16, end_vx: u16, url: &str) {
    if start_vx >= end_vx {
        return;
    }
    for vx in start_vx..end_vx {
        let Some(cell) = buf.cell_mut((vx, vy)) else {
            continue;
        };
        let is_first = vx == start_vx;
        let is_last = vx + 1 == end_vx;
        embed_osc8_cell(cell, url, is_first, is_last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // — Phase 36.6.4 Plan 04 Task 1 `<behavior>` tests (TDD) ─────────────────

    /// Test 1: both bare schemes are captured with correct start/end column
    /// offsets within the row's plain text.
    #[test]
    fn extract_links_finds_bare_http_and_https_urls() {
        let row = "see http://a.example and https://b.example too";
        let links = extract_links(row);
        assert_eq!(links.len(), 2, "expected exactly 2 bare URLs, got {links:?}");

        assert_eq!(links[0].url, "http://a.example");
        assert_eq!(links[0].label, "http://a.example");
        assert_eq!(links[0].start_cell, "see ".len());
        assert_eq!(links[0].end_cell, links[0].start_cell + "http://a.example".len());

        assert_eq!(links[1].url, "https://b.example");
        assert_eq!(links[1].label, "https://b.example");
        let prefix_len = "see http://a.example and ".len();
        assert_eq!(links[1].start_cell, prefix_len);
        assert_eq!(links[1].end_cell, prefix_len + "https://b.example".len());
    }

    /// Test 2: a markdown link yields the LABEL as the visible span and the
    /// target as the URL.
    #[test]
    fn extract_links_finds_markdown_link_label_and_target() {
        let row = "please [click here](https://example.com/path) now";
        let links = extract_links(row);
        assert_eq!(links.len(), 1, "expected exactly 1 markdown link, got {links:?}");
        let link = &links[0];
        assert_eq!(link.label, "click here");
        assert_eq!(link.url, "https://example.com/path");
        let prefix_len = "please [".len();
        assert_eq!(link.start_cell, prefix_len);
        assert_eq!(link.end_cell, prefix_len + "click here".len());
    }

    /// Test 3: markdown links with non-`http(s)` targets produce zero
    /// extracted links — the scheme restriction is a grammar-level input
    /// filter, not a downstream rejection.
    #[test]
    fn osc8_non_http_scheme_renders_plain_no_styling() {
        let js = "[click](javascript:alert(1))";
        assert!(extract_links(js).is_empty(), "javascript: target must never match: {js:?}");

        let file = "[open](file:///etc/passwd)";
        assert!(extract_links(file).is_empty(), "file:// target must never match: {file:?}");

        let mailto = "[email](mailto:a@b.com)";
        assert!(extract_links(mailto).is_empty(), "mailto: target must never match: {mailto:?}");
    }

    /// Test 4: a zero-length markdown label and a scheme with no host both
    /// produce zero extracted links.
    #[test]
    fn extract_links_rejects_empty_label_and_hostless_url() {
        let empty_label = "[](https://example.com)";
        assert!(
            extract_links(empty_label).is_empty(),
            "empty markdown label must never match: {empty_label:?}"
        );

        let hostless_end_of_text = "visit https://";
        assert!(
            extract_links(hostless_end_of_text).is_empty(),
            "scheme with nothing after :// must never match: {hostless_end_of_text:?}"
        );

        let hostless_triple_slash = "visit http:///no-host-here";
        assert!(
            extract_links(hostless_triple_slash).is_empty(),
            "scheme immediately followed by another slash (no host) must never match: \
             {hostless_triple_slash:?}"
        );

        let hostless_trailing_space = "visit http:// please";
        assert!(
            extract_links(hostless_trailing_space).is_empty(),
            "scheme followed immediately by whitespace (no host) must never match: \
             {hostless_trailing_space:?}"
        );
    }

    /// Test 5: a label containing a wide glyph reports its DISPLAY width,
    /// not its byte or char length.
    #[test]
    fn extract_links_measures_span_width_in_display_cells() {
        // "😀" is 4 UTF-8 bytes, 1 `char`, and 2 DISPLAY CELLS wide.
        let row = "go [👍😀](https://example.com) now";
        let links = extract_links(row);
        assert_eq!(links.len(), 1, "expected exactly 1 markdown link, got {links:?}");
        let link = &links[0];
        assert_eq!(link.label, "👍😀");
        // byte len = 8, char count = 2, display-cell width = 4 (2 wide glyphs).
        assert_ne!(
            link.end_cell - link.start_cell,
            link.label.len(),
            "span width must not be the BYTE length"
        );
        assert_ne!(
            link.end_cell - link.start_cell,
            link.label.chars().count(),
            "span width must not be the CHAR count"
        );
        assert_eq!(
            link.end_cell - link.start_cell,
            UnicodeWidthStr::width(link.label.as_str()),
            "span width must be the unicode-width DISPLAY width"
        );
        assert_eq!(link.end_cell - link.start_cell, 4);
    }
}
