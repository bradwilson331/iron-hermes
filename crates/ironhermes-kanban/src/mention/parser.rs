//! Pure-function Markdown `@handle` scanner with fence-state machine.
//!
//! # Overview
//!
//! `parse_mentions(body)` extracts every literal `@<word>` occurrence in
//! Markdown prose, skipping text inside:
//! - Fenced code blocks (``` or `~~~`, CommonMark §6.6)
//! - Inline code spans (`` ` ``, CommonMark §6.5)
//! - HTML comments (`<!-- ... -->`)
//!
//! ## Design: redact-then-scan
//!
//! 1. `redact_non_prose(body)` produces an equal-length string where non-prose
//!    regions are replaced by ASCII spaces (preserving byte offsets).
//! 2. `HANDLE_RE.captures_iter(&redacted)` extracts handles at the original
//!    byte positions.
//!
//! ## Invariant
//!
//! `redact_non_prose(body).len() == body.len()` for every input — byte offsets
//! returned in `MentionSpan` always point into `body`.
//!
//! ## Limitations (v1)
//!
//! Indented (4-space) code blocks are NOT treated as fences — they are
//! uncommon in task bodies and deferred to a future phase.

use std::sync::OnceLock;

use regex::Regex;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One `@handle` occurrence extracted from Markdown prose.
///
/// `handle` is the raw-cased string captured after the `@` character. The
/// resolver is responsible for lowercasing before validation (REQ-04).
///
/// `byte_offset` is the byte position of the first character of `handle` in
/// the original body string (i.e., the character after `@`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionSpan {
    /// Raw-cased handle text (no leading `@`).
    pub handle: String,
    /// Byte offset of the first character of `handle` in the original body.
    pub byte_offset: usize,
}

// ---------------------------------------------------------------------------
// Regex — cached via OnceLock (std since Rust 1.70, zero new deps)
// ---------------------------------------------------------------------------

fn handle_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Capture group 1: handle text (no `@`).
        // First char must be alnum to avoid matching `@_foo`, `@@foo` (the
        // inner `@` can't match because it's not in [A-Za-z0-9-]).
        // Trailing punctuation (.,?!:) ends the greedy run cleanly.
        Regex::new(r"@([A-Za-z0-9][A-Za-z0-9-]*)").unwrap()
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Extract all `@handle` mentions from Markdown prose in `body`.
///
/// Returns one [`MentionSpan`] per literal `@<word>` occurrence. Mentions
/// inside fenced code blocks, inline code spans, or HTML comments are
/// silently skipped.
///
/// This is a pure function: no IO, no side effects, no async.
pub fn parse_mentions(body: &str) -> Vec<MentionSpan> {
    let redacted = redact_non_prose(body);
    handle_re()
        .captures_iter(&redacted)
        .filter_map(|cap| {
            let m = cap.get(1)?;
            Some(MentionSpan {
                handle: m.as_str().to_string(),
                byte_offset: m.start(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Fence-state machine
// ---------------------------------------------------------------------------

/// State for the redaction scanner.
#[derive(Debug)]
enum ScanState {
    /// Normal Markdown prose — `@mentions` are valid here.
    Prose,
    /// Inside a fenced code block opened with `fence_len` repetitions of
    /// `fence_char` (`` ` `` or `~`).
    FencedCode { fence_char: char, fence_len: usize },
    /// Inside an inline code span opened with `backtick_run` backticks.
    InlineCode { backtick_run: usize },
    /// Inside an HTML comment `<!-- ... -->`.
    HtmlComment,
}

/// Replace all non-prose regions in `body` with ASCII spaces.
///
/// # Invariant
///
/// `redact_non_prose(body).len() == body.len()` — byte offsets are preserved.
fn redact_non_prose(body: &str) -> String {
    let bytes = body.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(n);
    let mut i = 0usize;
    let mut state = ScanState::Prose;

    while i < n {
        match state {
            // ------------------------------------------------------------------
            // PROSE — look for the start of a non-prose region or an @-mention.
            // ------------------------------------------------------------------
            ScanState::Prose => {
                // 1. Try to open an HTML comment: <!--
                if bytes[i..].starts_with(b"<!--") {
                    pad_space(&mut out, 4);
                    i += 4;
                    state = ScanState::HtmlComment;
                    continue;
                }

                // 2. Try to open a fenced code block (only at line start).
                if is_line_start(bytes, i) {
                    let indent_end = skip_indent(bytes, i); // skip up to 3 spaces
                    if let Some((fc, fl, consumed)) = try_consume_fence_open(bytes, indent_end) {
                        // Emit spaces for the indent + fence opener bytes.
                        pad_space(&mut out, (indent_end - i) + consumed);
                        i = indent_end + consumed;
                        state = ScanState::FencedCode {
                            fence_char: fc,
                            fence_len: fl,
                        };
                        continue;
                    }
                }

                // 3. Try to open an inline code span (one or more backticks).
                if bytes[i] == b'`' {
                    let run = count_run(bytes, i, b'`');
                    pad_space(&mut out, run);
                    i += run;
                    state = ScanState::InlineCode { backtick_run: run };
                    continue;
                }

                // 4. Normal prose character — emit as-is.
                // Push the UTF-8 bytes of the current character verbatim.
                let ch_len = utf8_char_len(bytes[i]);
                let slice = &body[i..i + ch_len];
                out.push_str(slice);
                i += ch_len;
            }

            // ------------------------------------------------------------------
            // HTML COMMENT — consume until -->
            // ------------------------------------------------------------------
            ScanState::HtmlComment => {
                if bytes[i..].starts_with(b"-->") {
                    pad_space(&mut out, 3);
                    i += 3;
                    state = ScanState::Prose;
                } else {
                    let ch_len = utf8_char_len(bytes[i]);
                    pad_space(&mut out, ch_len);
                    i += ch_len;
                }
            }

            // ------------------------------------------------------------------
            // FENCED CODE BLOCK — consume until a matching closing fence.
            // ------------------------------------------------------------------
            ScanState::FencedCode {
                fence_char,
                fence_len,
            } => {
                // A closing fence must appear at line start (possibly indented).
                if is_line_start(bytes, i) {
                    let indent_end = skip_indent(bytes, i);
                    if let Some(consumed) =
                        try_consume_fence_close(bytes, indent_end, fence_char, fence_len)
                    {
                        pad_space(&mut out, (indent_end - i) + consumed);
                        i = indent_end + consumed;
                        state = ScanState::Prose;
                        continue;
                    }
                }
                let ch_len = utf8_char_len(bytes[i]);
                pad_space(&mut out, ch_len);
                i += ch_len;
            }

            // ------------------------------------------------------------------
            // INLINE CODE SPAN — consume until a matching backtick run.
            // ------------------------------------------------------------------
            ScanState::InlineCode { backtick_run } => {
                if bytes[i] == b'`' {
                    let run = count_run(bytes, i, b'`');
                    if run == backtick_run {
                        // Matching closer — return to prose.
                        pad_space(&mut out, run);
                        i += run;
                        state = ScanState::Prose;
                    } else {
                        // Different-length backtick run — emit as spaces, stay in inline code.
                        pad_space(&mut out, run);
                        i += run;
                    }
                } else {
                    let ch_len = utf8_char_len(bytes[i]);
                    pad_space(&mut out, ch_len);
                    i += ch_len;
                }
            }
        }
    }

    debug_assert_eq!(
        out.len(),
        body.len(),
        "redact_non_prose violated byte-length invariant"
    );
    out
}

// ---------------------------------------------------------------------------
// Helper functions (file-private)
// ---------------------------------------------------------------------------

/// Returns `true` if byte position `i` is at the start of a line (i.e., `i
/// == 0` or the previous byte is `\n`).
fn is_line_start(bytes: &[u8], i: usize) -> bool {
    i == 0 || bytes[i - 1] == b'\n'
}

/// Skip up to 3 ASCII space characters from position `i`. Returns the index
/// of the first non-space (or the original index if none were consumed).
fn skip_indent(bytes: &[u8], i: usize) -> usize {
    let mut j = i;
    while j < bytes.len() && j - i < 3 && bytes[j] == b' ' {
        j += 1;
    }
    j
}

/// Count consecutive `ch` bytes starting at `i`.
fn count_run(bytes: &[u8], i: usize, ch: u8) -> usize {
    let mut n = 0;
    while i + n < bytes.len() && bytes[i + n] == ch {
        n += 1;
    }
    n
}

/// Try to parse a fence opener (``` ` ``` or `~`) starting at `i` (after any
/// indent has been skipped).
///
/// Returns `Some((fence_char, fence_len, bytes_consumed))` if a valid opener
/// is found (≥ 3 matching backtick or tilde characters followed by a newline
/// or end-of-string). Returns `None` otherwise.
fn try_consume_fence_open(bytes: &[u8], i: usize) -> Option<(char, usize, usize)> {
    let ch = bytes.get(i)?;
    if *ch != b'`' && *ch != b'~' {
        return None;
    }
    let fence_char = *ch as char;
    let run = count_run(bytes, i, *ch);
    if run < 3 {
        return None;
    }
    // The info string follows; we consume until end of line.
    let mut j = i + run;
    while j < bytes.len() && bytes[j] != b'\n' {
        j += 1;
    }
    // Consume the newline itself if present.
    if j < bytes.len() && bytes[j] == b'\n' {
        j += 1;
    }
    Some((fence_char, run, j - i))
}

/// Try to parse a fence closer starting at `i` (after any indent).
///
/// Returns `Some(bytes_consumed)` if we find ≥ `fence_len` of `fence_char`
/// followed only by optional spaces and then a newline or end-of-string.
fn try_consume_fence_close(
    bytes: &[u8],
    i: usize,
    fence_char: char,
    fence_len: usize,
) -> Option<usize> {
    let run = count_run(bytes, i, fence_char as u8);
    if run < fence_len {
        return None;
    }
    // After the run there may only be spaces and then newline/EOF.
    let mut j = i + run;
    while j < bytes.len() && bytes[j] == b' ' {
        j += 1;
    }
    if j >= bytes.len() || bytes[j] == b'\n' {
        // Consume the newline if present.
        if j < bytes.len() && bytes[j] == b'\n' {
            j += 1;
        }
        Some(j - i)
    } else {
        None
    }
}

/// Push `n` ASCII space characters onto `out`.
fn pad_space(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}

/// Return the byte length of a UTF-8 sequence starting with byte `b`.
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // REQ-01: parse_mentions extracts handles from prose
    // ------------------------------------------------------------------

    #[test]
    fn extracts_simple_handle() {
        let spans = parse_mentions("hello @reviewer world");
        assert_eq!(
            spans,
            vec![MentionSpan {
                handle: "reviewer".to_string(),
                byte_offset: 7,
            }]
        );
    }

    // ------------------------------------------------------------------
    // REQ-02: fence-skip, inline-code-skip, HTML-comment-skip
    // ------------------------------------------------------------------

    #[test]
    fn skips_fenced_inline_html_comment() {
        // Fenced code block
        let spans = parse_mentions("```\n@reviewer ignored\n```");
        assert!(spans.is_empty(), "expected no mentions, got {:?}", spans);

        // Inline code span
        let spans = parse_mentions("foo `@reviewer` bar");
        assert!(spans.is_empty(), "expected no mentions, got {:?}", spans);

        // HTML comment — one mention outside, none inside
        let spans = parse_mentions("ok <!-- @reviewer hidden --> done @writer");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].handle, "writer");
    }

    // ------------------------------------------------------------------
    // REQ-01 / D-03: raw case preserved in MentionSpan.handle
    // ------------------------------------------------------------------

    #[test]
    fn case_preserved_in_span_handle_field() {
        let spans = parse_mentions("@Reviewer, take a look.");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].handle, "Reviewer");
        // byte_offset: `@` is at 0, so `R` is at 1
        assert_eq!(spans[0].byte_offset, 1);
    }

    // ------------------------------------------------------------------
    // Trailing punctuation stripped by regex char class
    // ------------------------------------------------------------------

    #[test]
    fn trailing_punctuation_stripped() {
        let spans = parse_mentions("@reviewer, please look at this.");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].handle, "reviewer");
        assert_eq!(spans[0].byte_offset, 1);
    }

    // ------------------------------------------------------------------
    // @@reviewer: inner @ is not alnum, so second @ starts the match
    // ------------------------------------------------------------------

    #[test]
    fn double_at_sign_extracts_inner_only() {
        let spans = parse_mentions("@@reviewer");
        // The regex will find @reviewer starting at position 1 (the second @).
        // Capture group 1 starts at position 2 (the 'r').
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].handle, "reviewer");
        assert_eq!(spans[0].byte_offset, 2);
    }

    // ------------------------------------------------------------------
    // Multiple mentions extracted in order
    // ------------------------------------------------------------------

    #[test]
    fn multi_mention_extracts_all_in_order() {
        let spans = parse_mentions("ping @alice and @bob please");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].handle, "alice");
        assert_eq!(spans[1].handle, "bob");
    }

    // ------------------------------------------------------------------
    // Newline-separated handles
    // ------------------------------------------------------------------

    #[test]
    fn newline_separated_handles_extracted() {
        let body = "a\nb\n@reviewer\nc";
        let spans = parse_mentions(body);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].handle, "reviewer");
        // "@reviewer" starts at index 4 (a \n b \n), handle at 5
        assert_eq!(spans[0].byte_offset, 5);
    }

    // ------------------------------------------------------------------
    // Mixed prose + fenced block: only prose mentions extracted
    // ------------------------------------------------------------------

    #[test]
    fn mixed_prose_fence_only_prose_extracted() {
        let body = "mixed prose @one and ```\n@two\n``` and @three";
        let spans = parse_mentions(body);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].handle, "one");
        assert_eq!(spans[1].handle, "three");
    }

    // ------------------------------------------------------------------
    // Invariant: redact_non_prose preserves byte length
    // ------------------------------------------------------------------

    #[test]
    fn redact_non_prose_preserves_byte_length() {
        let cases = [
            "",
            "hello world",
            "```\n@ignored\n```",
            "foo `@bar` baz",
            "<!-- @hidden --> @visible",
            "a\n```\n@x\n```\nb @y c",
            "unicode: 你好 @handle world",
            "~~~\nfenced\n~~~\n@outside",
        ];
        for body in &cases {
            let redacted = redact_non_prose(body);
            assert_eq!(
                redacted.len(),
                body.len(),
                "byte-length mismatch for: {:?}",
                body
            );
        }
    }
}
