//! Phase 36.17.2.2 Plan 02 (D-05/D-06/D-08/D-09): streaming MediaTagExtractor.
//!
//! State machine that strips `<MEDIA: path|url>` inline tags from streaming
//! LLM output deltas and accumulates the extracted references as `MediaRef`
//! values. Mirrors `crates/ironhermes-agent/src/streaming_scrubber.rs`'s
//! partial-prefix buffering algorithm exactly, then adds three pieces of
//! D-09 / Pitfall 5 state (`in_fenced_code`, `in_inline_code`,
//! `prev_was_escape`) so tags inside fenced or inline code spans are NOT
//! extracted — they pass through verbatim to the visible stream.
//!
//! Critical policy divergence from the scrubber: `flush_tail()` on an
//! unterminated `<MEDIA: ...` open returns the held buffer as VISIBLE text
//! rather than discarding it. Scrubber discards (memory-context leakage is a
//! security concern); extractor emits (user trust requires tag-like text
//! never to disappear).

use std::path::PathBuf;

const OPEN_TAG: &str = "<MEDIA:";
const FENCE: &str = "```";

/// Where a media reference resolves to — a local filesystem path or an
/// http(s) URL Telegram will fetch on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSource {
    Path(PathBuf),
    Url(String),
}

/// Telegram-flavored media kind chosen by extension on the last path/URL
/// segment per D-06's dispatch table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Photo,
    Voice,
    Audio,
    Video,
    Document,
}

/// One extracted `<MEDIA: ...>` reference, preserving the original tag
/// literal for D-19's reinsert-on-failure path in handler.rs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRef {
    pub source: MediaSource,
    pub kind: MediaKind,
    /// Original tag literal as it appeared in the stream (e.g.
    /// `<MEDIA: /tmp/foo.png>`), used by handler.rs D-19 reinsert path
    /// when an attachment fails.
    pub original_tag_text: String,
}

/// Stateful extractor for streaming text that may contain `<MEDIA: ...>`
/// tags. Create one per turn (or call `reset()` between turns).
///
/// Usage pattern (Phase 36.17.2.2 D-08, mirrors the scrubber wire-up at
/// handler.rs:1333-1343):
/// ```ignore
/// let extractor = Arc::new(std::sync::Mutex::new(MediaTagExtractor::new()));
/// let extractor_cb = Arc::clone(&extractor);
/// let stream_callback = Box::new(move |delta: &str| {
///     let visible = extractor_cb.lock().unwrap().feed(delta);
///     if !visible.is_empty() { emit(visible); }
/// });
/// // After stream completes:
/// let tail = extractor.lock().unwrap().flush_tail();
/// if !tail.is_empty() { emit(tail); }
/// let attachments = extractor.lock().unwrap().take_attachments();
/// ```
pub struct MediaTagExtractor {
    in_tag: bool,
    in_fenced_code: bool,
    in_inline_code: bool,
    prev_was_escape: bool,
    tag_body: String,
    buf: String,
    attachments: Vec<MediaRef>,
}

impl MediaTagExtractor {
    pub fn new() -> Self {
        Self {
            in_tag: false,
            in_fenced_code: false,
            in_inline_code: false,
            prev_was_escape: false,
            tag_body: String::new(),
            buf: String::new(),
            attachments: Vec::new(),
        }
    }

    /// Reset to initial state (reuse for a new turn without reallocation).
    pub fn reset(&mut self) {
        self.in_tag = false;
        self.in_fenced_code = false;
        self.in_inline_code = false;
        self.prev_was_escape = false;
        self.tag_body.clear();
        self.buf.clear();
        self.attachments.clear();
    }

    /// Feed a streaming delta. Returns the visible portion after extraction.
    ///
    /// Algorithm (mirrors `StreamingContextScrubber::feed`):
    ///
    /// 1. Prepend any held-back buffer to the new delta.
    /// 2. Walk the combined buffer one char at a time, tracking three state
    ///    booleans (`in_tag`, `in_fenced_code`, `in_inline_code`) plus the
    ///    backslash-escape flag (`prev_was_escape`).
    /// 3. Inside `<MEDIA: ...>`: accumulate body until `>`. On close, parse
    ///    body per D-06 (URL detection, extension → MediaKind dispatch),
    ///    push `MediaRef` to `attachments`.
    /// 4. Inside fence or inline code: pass through verbatim to visible,
    ///    DO NOT toggle `in_tag`.
    /// 5. Otherwise: detect fence/inline-code/MEDIA-tag openers; held-back
    ///    partial-prefix machinery preserves the scrubber's split-delta
    ///    behavior for `<MEDIA:`, fence `` ``` ``, and inline `` ` ``.
    pub fn feed(&mut self, delta: &str) -> String {
        if delta.is_empty() {
            return String::new();
        }

        // Prepend any previously held partial-tag tail.
        let buf = if self.buf.is_empty() {
            delta.to_owned()
        } else {
            let mut b = std::mem::take(&mut self.buf);
            b.push_str(delta);
            b
        };

        let mut out = String::new();
        // Cursor tracked as a byte offset into `buf`. The cursor only ever
        // advances past ASCII bytes when it's used to slice (open-tag jumps,
        // fence jumps), or by `ch.len_utf8()` for char-walking inside spans.
        // All UTF-8 char boundaries are therefore preserved.
        let bytes = buf.as_bytes();
        let mut i: usize = 0;

        while i < bytes.len() {
            if self.in_tag {
                // Looking for `>` to close the tag.
                if let Some(rel) = find_byte(&bytes[i..], b'>') {
                    let close_at = i + rel;
                    // Append body chars (everything from i..close_at) to tag_body.
                    // SAFETY: i and close_at are byte offsets inside a valid &str;
                    // close_at points at an ASCII `>`, so the slice [i..close_at]
                    // ends on a char boundary.
                    self.tag_body.push_str(&buf[i..close_at]);
                    let media_ref = parse_tag_body(&self.tag_body);
                    self.attachments.push(media_ref);
                    self.tag_body.clear();
                    self.in_tag = false;
                    self.prev_was_escape = false;
                    i = close_at + 1;
                    continue;
                } else {
                    // No close yet — accumulate the rest into tag_body and hold.
                    self.tag_body.push_str(&buf[i..]);
                    return out;
                }
            }

            if self.in_fenced_code {
                // Look for the fence close (three backticks). Mirror scrubber's
                // partial-suffix held-back so a fence split across deltas
                // doesn't accidentally end early.
                if let Some(rel) = find_ascii_ci(&buf[i..], FENCE) {
                    let fence_end = i + rel + FENCE.len();
                    out.push_str(&buf[i..fence_end]);
                    self.in_fenced_code = false;
                    self.prev_was_escape = false;
                    i = fence_end;
                    continue;
                } else {
                    let tail = &buf[i..];
                    let held = max_partial_suffix(tail, FENCE);
                    if held > 0 {
                        let split = tail.len() - held;
                        out.push_str(&tail[..split]);
                        self.buf = tail[split..].to_owned();
                    } else {
                        out.push_str(tail);
                    }
                    return out;
                }
            }

            if self.in_inline_code {
                // Walk char-by-char to track backslash-escape and find the
                // first unescaped backtick.
                let remainder = &buf[i..];
                let mut walked: usize = 0;
                let mut closed = false;
                let mut local_prev_escape = self.prev_was_escape;
                for ch in remainder.chars() {
                    let ch_len = ch.len_utf8();
                    if ch == '`' && !local_prev_escape {
                        // Closing inline code — emit through the backtick.
                        let emit_end = i + walked + ch_len;
                        out.push_str(&buf[i..emit_end]);
                        self.in_inline_code = false;
                        self.prev_was_escape = false;
                        i = emit_end;
                        closed = true;
                        break;
                    }
                    local_prev_escape = ch == '\\' && !local_prev_escape;
                    walked += ch_len;
                }
                if closed {
                    continue;
                }
                // No closing backtick found — emit verbatim, persist escape state.
                out.push_str(remainder);
                self.prev_was_escape = local_prev_escape;
                return out;
            }

            // Plain text — find the earliest opener: fence, inline-code, or
            // <MEDIA:. Partial-prefix held-back applies when none of them is
            // found in the remaining buffer.
            let tail = &buf[i..];
            let fence_pos = find_byte_seq(tail.as_bytes(), FENCE.as_bytes());
            let media_pos = find_ascii_ci(tail, OPEN_TAG);
            // For inline backtick, we walk char-by-char so escape tracking
            // is correct.
            let inline_pos = find_first_unescaped_backtick(tail, self.prev_was_escape);

            // Choose the earliest opener; ties: fence wins over inline (since
            // `\`\`\`` includes the first backtick); inline wins over MEDIA
            // only if it strictly precedes (otherwise MEDIA takes its byte).
            let next = earliest(&[fence_pos, inline_pos, media_pos]);

            match next {
                None => {
                    // No opener in this remainder. Held-back: max partial of
                    // any of the three patterns at the tail of the buffer.
                    // We use max(fence, inline-backtick-1-byte, MEDIA) — the
                    // largest takes care of the smaller cases.
                    let held_fence = max_partial_suffix(tail, FENCE);
                    let held_media = max_partial_suffix(tail, OPEN_TAG);
                    // Inline backtick is a single byte — no partial possible.
                    let held = held_fence.max(held_media);
                    if held > 0 {
                        let split = tail.len() - held;
                        // Emit visible up to the held portion. Update
                        // prev_was_escape over the emitted chars.
                        let visible = &tail[..split];
                        self.update_escape_state_for(visible);
                        out.push_str(visible);
                        self.buf = tail[split..].to_owned();
                    } else {
                        self.update_escape_state_for(tail);
                        out.push_str(tail);
                    }
                    return out;
                }
                Some(opener) => match opener {
                    Opener::Fence(rel) => {
                        let visible = &tail[..rel];
                        self.update_escape_state_for(visible);
                        out.push_str(visible);
                        // Emit the three backticks too — fence text passes
                        // through verbatim.
                        out.push_str(FENCE);
                        self.in_fenced_code = true;
                        self.prev_was_escape = false;
                        i += rel + FENCE.len();
                    }
                    Opener::Inline(rel) => {
                        let visible = &tail[..rel];
                        self.update_escape_state_for(visible);
                        out.push_str(visible);
                        // Emit the backtick — inline-code body passes
                        // through verbatim.
                        out.push('`');
                        self.in_inline_code = true;
                        self.prev_was_escape = false;
                        i += rel + 1;
                    }
                    Opener::Media(rel) => {
                        // Honor escape state: if the byte immediately before
                        // the `<` was an unescaped `\`, the tag is escaped
                        // and should pass through verbatim. We test this by
                        // updating escape state across the pre-tag visible
                        // text and inspecting the final escape flag.
                        let visible = &tail[..rel];
                        let mut local_escape = self.prev_was_escape;
                        for ch in visible.chars() {
                            local_escape = ch == '\\' && !local_escape;
                        }
                        if local_escape {
                            // Tag is escaped — emit visible + literal `<MEDIA:`
                            // verbatim and continue scanning after it.
                            self.update_escape_state_for(visible);
                            out.push_str(visible);
                            out.push_str(OPEN_TAG);
                            // The `:` is not a backslash, so escape resets.
                            self.prev_was_escape = false;
                            i += rel + OPEN_TAG.len();
                        } else {
                            self.update_escape_state_for(visible);
                            out.push_str(visible);
                            self.in_tag = true;
                            self.tag_body.clear();
                            self.prev_was_escape = false;
                            i += rel + OPEN_TAG.len();
                        }
                    }
                },
            }
        }

        out
    }

    /// Flush any held-back partial prefix at end-of-stream. Returns it as
    /// VISIBLE text — diverges from scrubber's discard semantics, per
    /// RESEARCH §Pattern Audit: user trust requires tag-like text never
    /// disappears.
    pub fn flush_tail(&mut self) -> String {
        let mut out = String::new();
        if self.in_tag {
            // Unterminated tag — emit `<MEDIA: ` + whatever body we'd
            // accumulated as visible text.
            out.push_str(OPEN_TAG);
            // The grammar consumed `<MEDIA:` already; the body starts with
            // the raw chars we'd been accumulating, which the model emitted
            // verbatim (possibly with leading whitespace).
            out.push_str(&self.tag_body);
            self.tag_body.clear();
            self.in_tag = false;
        }
        // Any held partial prefix turned out not to be a real tag — emit
        // verbatim.
        if !self.buf.is_empty() {
            out.push_str(&std::mem::take(&mut self.buf));
        }
        // NOTE: do NOT clear self.attachments — those are drained separately
        // by take_attachments().
        self.prev_was_escape = false;
        out
    }

    /// Drain accumulated tags; subsequent calls return `vec![]` until new
    /// tags are fed.
    pub fn take_attachments(&mut self) -> Vec<MediaRef> {
        std::mem::take(&mut self.attachments)
    }

    /// Update `prev_was_escape` across a run of visible text. Called whenever
    /// we emit a chunk of plain text without entering a special state.
    fn update_escape_state_for(&mut self, text: &str) {
        let mut esc = self.prev_was_escape;
        for ch in text.chars() {
            esc = ch == '\\' && !esc;
        }
        self.prev_was_escape = esc;
    }
}

impl Default for MediaTagExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// -- Helpers --------------------------------------------------------------

/// Which opener type was found and its byte offset in the remainder.
#[derive(Debug, Clone, Copy)]
enum Opener {
    Fence(usize),
    Inline(usize),
    Media(usize),
}

fn earliest(candidates: &[Option<usize>; 3]) -> Option<Opener> {
    // Map each Some(pos) to (pos, kind) and pick the smallest pos.
    let mut best: Option<Opener> = None;
    if let Some(p) = candidates[0] {
        best = Some(Opener::Fence(p));
    }
    if let Some(p) = candidates[1] {
        let bp = match best {
            Some(Opener::Fence(x)) | Some(Opener::Inline(x)) | Some(Opener::Media(x)) => x,
            None => usize::MAX,
        };
        if p < bp {
            best = Some(Opener::Inline(p));
        }
    }
    if let Some(p) = candidates[2] {
        let bp = match best {
            Some(Opener::Fence(x)) | Some(Opener::Inline(x)) | Some(Opener::Media(x)) => x,
            None => usize::MAX,
        };
        if p < bp {
            best = Some(Opener::Media(p));
        }
    }
    best
}

/// Find the first ASCII-case-insensitive occurrence of `needle` in
/// `haystack`. `needle` must be pure ASCII.
fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Find a single ASCII byte in a byte slice (used for `>` close-tag search).
fn find_byte(haystack: &[u8], needle: u8) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

/// Find a byte sequence in a byte slice (used for `` ``` `` fence search
/// without case-folding overhead).
fn find_byte_seq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// Find the first unescaped backtick in `s`, given the escape state at
/// position 0. Walks char-by-char to preserve UTF-8 boundaries and to
/// honor `\` escapes.
fn find_first_unescaped_backtick(s: &str, initial_escape: bool) -> Option<usize> {
    let mut walked: usize = 0;
    let mut esc = initial_escape;
    for ch in s.chars() {
        if ch == '`' && !esc {
            return Some(walked);
        }
        esc = ch == '\\' && !esc;
        walked += ch.len_utf8();
    }
    None
}

/// Return the length of the longest buf-suffix that is a prefix of `tag`,
/// ASCII-case-insensitive. `tag` is pure ASCII so the returned offset
/// always lands on a UTF-8 char boundary.
fn max_partial_suffix(buf: &str, tag: &str) -> usize {
    let b = buf.as_bytes();
    let t = tag.as_bytes();
    if t.len() < 2 {
        return 0;
    }
    let max_check = b.len().min(t.len() - 1);
    for i in (1..=max_check).rev() {
        let suffix_start = b.len() - i;
        if b[suffix_start..].eq_ignore_ascii_case(&t[..i]) {
            return i;
        }
    }
    0
}

/// Parse a `<MEDIA: ...>` body per D-06.
///
/// - Leading whitespace trimmed (`<MEDIA:\s*([^>]+)>`).
/// - `http://` or `https://` prefix → `MediaSource::Url`; otherwise
///   `MediaSource::Path`.
/// - `MediaKind` chosen by the extension on the last path segment
///   (case-insensitive), stripping any URL query string before the
///   extension lookup.
fn parse_tag_body(raw_body: &str) -> MediaRef {
    let body = raw_body.trim_start();
    let original_tag_text = format!("<MEDIA: {}>", body);

    let source = if body.starts_with("http://") || body.starts_with("https://") {
        MediaSource::Url(body.to_string())
    } else {
        MediaSource::Path(PathBuf::from(body))
    };

    let kind = infer_kind(body);
    MediaRef {
        source,
        kind,
        original_tag_text,
    }
}

/// Extract a `MediaKind` from the body string per D-06's dispatch table.
fn infer_kind(body: &str) -> MediaKind {
    // Strip URL query string and fragment.
    let no_query = body.split(['?', '#']).next().unwrap_or(body);
    // Last path segment.
    let last_segment = no_query.rsplit('/').next().unwrap_or(no_query);
    // Extension (lowercase).
    let ext = last_segment
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" => MediaKind::Photo,
        "ogg" | "opus" => MediaKind::Voice,
        "mp3" | "m4a" | "flac" | "wav" => MediaKind::Audio,
        "mp4" | "mov" | "webm" => MediaKind::Video,
        _ => MediaKind::Document,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -- Grammar + happy path -------------------------------------------------

    #[test]
    fn full_tag_in_one_delta_produces_one_mediaref() {
        let mut x = MediaTagExtractor::new();
        let visible = x.feed("<MEDIA: /tmp/foo.png>");
        assert_eq!(visible, "", "visible stream should be empty (tag stripped)");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1, "exactly one MediaRef extracted");
        assert_eq!(refs[0].source, MediaSource::Path(PathBuf::from("/tmp/foo.png")));
        assert_eq!(refs[0].kind, MediaKind::Photo);
        assert_eq!(refs[0].original_tag_text, "<MEDIA: /tmp/foo.png>");
    }

    #[test]
    fn tag_with_leading_whitespace_in_body() {
        let mut x = MediaTagExtractor::new();
        let visible = x.feed("<MEDIA:   foo.png>");
        assert_eq!(visible, "");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source, MediaSource::Path(PathBuf::from("foo.png")));
        assert_eq!(refs[0].kind, MediaKind::Photo);
    }

    // -- Streaming partial-prefix buffering (mirrors scrubber) ---------------

    #[test]
    fn split_open_tag_across_two_deltas() {
        let mut x = MediaTagExtractor::new();
        let v1 = x.feed("<MEDIA: f");
        assert_eq!(v1, "", "partial tag held");
        let v2 = x.feed("oo.png>");
        assert_eq!(v2, "", "tag completed, still no visible output");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source, MediaSource::Path(PathBuf::from("foo.png")));
        assert_eq!(refs[0].kind, MediaKind::Photo);
    }

    #[test]
    fn partial_prefix_disproved_emits_verbatim() {
        let mut x = MediaTagExtractor::new();
        let v1 = x.feed("<MEDI");
        assert_eq!(v1, "", "potential tag prefix held back");
        let v2 = x.feed("um is the message");
        let combined = v1 + &v2;
        assert!(
            combined.contains("<MEDIum is the message"),
            "held prefix should be emitted verbatim once disambiguated; got {:?}",
            combined
        );
        let refs = x.take_attachments();
        assert!(refs.is_empty(), "no tag, no attachments");
    }

    #[test]
    fn tag_never_closes_flush_tail_emits_visible() {
        let mut x = MediaTagExtractor::new();
        let v = x.feed("prefix <MEDIA: foo");
        assert!(v.contains("prefix "), "text before tag emits normally");
        assert!(
            !v.contains("<MEDIA:"),
            "open tag is held while body is being accumulated"
        );
        let tail = x.flush_tail();
        assert!(
            tail.contains("<MEDIA: foo"),
            "extractor policy: unterminated tag is emitted as visible text on flush_tail; got {:?}",
            tail
        );
        let refs = x.take_attachments();
        assert!(refs.is_empty(), "no closed tag, no MediaRef");
    }

    // -- Multi-tag + ordering ------------------------------------------------

    #[test]
    fn multiple_tags_in_one_delta_preserve_stream_order() {
        let mut x = MediaTagExtractor::new();
        let visible = x.feed("<MEDIA: a.png>middle<MEDIA: b.ogg>");
        assert_eq!(visible, "middle");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].source, MediaSource::Path(PathBuf::from("a.png")));
        assert_eq!(refs[0].kind, MediaKind::Photo);
        assert_eq!(refs[1].source, MediaSource::Path(PathBuf::from("b.ogg")));
        assert_eq!(refs[1].kind, MediaKind::Voice);
    }

    #[test]
    fn take_attachments_drains_and_clears() {
        let mut x = MediaTagExtractor::new();
        let _ = x.feed("<MEDIA: a.png>middle<MEDIA: b.ogg>");
        let first = x.take_attachments();
        assert_eq!(first.len(), 2, "first drain returns all entries");
        let second = x.take_attachments();
        assert!(second.is_empty(), "second drain returns empty");
    }

    // -- D-09 fence + inline-code skip --------------------------------------

    #[test]
    fn tag_inside_fenced_code_passes_through() {
        let mut x = MediaTagExtractor::new();
        let visible = x.feed("here is a fence\n```\n<MEDIA: foo.png>\n```\ntail");
        assert!(
            visible.contains("<MEDIA: foo.png>"),
            "tag inside fence should pass through verbatim; got {:?}",
            visible
        );
        assert!(visible.contains("tail"), "post-fence text survives");
        let refs = x.take_attachments();
        assert!(refs.is_empty(), "no extraction inside fenced code");
    }

    #[test]
    fn tag_inside_inline_code_passes_through() {
        let mut x = MediaTagExtractor::new();
        let visible = x.feed("text `<MEDIA: foo.png>` more");
        assert!(
            visible.contains("<MEDIA: foo.png>"),
            "tag inside inline code should pass through verbatim; got {:?}",
            visible
        );
        assert!(visible.contains("more"), "post inline-code text survives");
        let refs = x.take_attachments();
        assert!(refs.is_empty(), "no extraction inside inline code");
    }

    #[test]
    fn escaped_backtick_does_not_open_inline_code() {
        // Pitfall 5: \` is a literal escaped backtick, NOT an inline-code
        // opener. The tag between two escaped backticks SHOULD extract.
        let mut x = MediaTagExtractor::new();
        let _ = x.feed("text \\`<MEDIA: foo.png>\\` more");
        let refs = x.take_attachments();
        assert_eq!(
            refs.len(),
            1,
            "escaped backticks don't toggle inline code; tag must extract"
        );
        assert_eq!(refs[0].kind, MediaKind::Photo);
    }

    #[test]
    fn inline_code_inside_fence_no_extraction_either_way() {
        let mut x = MediaTagExtractor::new();
        let visible = x.feed("```\n`<MEDIA: foo.png>`\n```");
        assert!(
            visible.contains("<MEDIA: foo.png>"),
            "tag inside backticked text inside fence passes through; got {:?}",
            visible
        );
        let refs = x.take_attachments();
        assert!(refs.is_empty(), "no extraction inside fence regardless of inline state");
    }

    // -- URL vs path + MediaKind dispatch (D-06) ----------------------------

    #[test]
    fn url_form_https_produces_url_source() {
        let mut x = MediaTagExtractor::new();
        let _ = x.feed("<MEDIA: https://example.com/image.jpg>");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].source,
            MediaSource::Url("https://example.com/image.jpg".to_string())
        );
        assert_eq!(refs[0].kind, MediaKind::Photo);
    }

    #[test]
    fn url_form_http_produces_url_source() {
        let mut x = MediaTagExtractor::new();
        let _ = x.feed("<MEDIA: http://example.com/image.jpg>");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].source,
            MediaSource::Url("http://example.com/image.jpg".to_string())
        );
        assert_eq!(refs[0].kind, MediaKind::Photo);
    }

    #[test]
    fn path_with_relative_segments_produces_path_source() {
        let mut x = MediaTagExtractor::new();
        let _ = x.feed("<MEDIA: ./relative/path.png>");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].source,
            MediaSource::Path(PathBuf::from("./relative/path.png"))
        );
        assert_eq!(refs[0].kind, MediaKind::Photo);
    }

    #[test]
    fn extension_dispatch_table() {
        // Each extension → MediaKind per D-06.
        let cases: &[(&str, MediaKind)] = &[
            ("a.png", MediaKind::Photo),
            ("a.jpg", MediaKind::Photo),
            ("a.jpeg", MediaKind::Photo),
            ("a.webp", MediaKind::Photo),
            ("a.gif", MediaKind::Photo),
            ("a.ogg", MediaKind::Voice),
            ("a.opus", MediaKind::Voice),
            ("a.mp3", MediaKind::Audio),
            ("a.m4a", MediaKind::Audio),
            ("a.flac", MediaKind::Audio),
            ("a.wav", MediaKind::Audio),
            ("a.mp4", MediaKind::Video),
            ("a.mov", MediaKind::Video),
            ("a.webm", MediaKind::Video),
            ("a.pdf", MediaKind::Document),
            ("a.txt", MediaKind::Document),
            ("noext", MediaKind::Document),
        ];
        for (name, expected) in cases {
            let mut x = MediaTagExtractor::new();
            let _ = x.feed(&format!("<MEDIA: {}>", name));
            let refs = x.take_attachments();
            assert_eq!(refs.len(), 1, "case {} produced wrong count", name);
            assert_eq!(refs[0].kind, *expected, "case {} got wrong kind", name);
        }
    }

    #[test]
    fn extension_case_insensitive() {
        let mut x = MediaTagExtractor::new();
        let _ = x.feed("<MEDIA: FOO.PNG>");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, MediaKind::Photo);
    }

    #[test]
    fn url_extension_in_path_segment() {
        let mut x = MediaTagExtractor::new();
        let _ = x.feed("<MEDIA: https://example.com/a/b/foo.mp3?token=x>");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1);
        assert!(matches!(refs[0].source, MediaSource::Url(_)));
        assert_eq!(
            refs[0].kind,
            MediaKind::Audio,
            "extension is read from the last path segment, ignoring query string"
        );
    }

    // -- Robustness regressions ---------------------------------------------

    #[test]
    fn non_ascii_text_around_tag_no_panic() {
        // Mirror of scrubber's CR-01 regression: U+0130 + Cyrillic.
        // ASCII-byte search must stay aligned with the original string.
        let mut x = MediaTagExtractor::new();
        let visible = x.feed("İİİ <MEDIA: foo.png> Привет");
        assert!(visible.contains("İİİ"), "non-ASCII prefix survives");
        assert!(visible.contains("Привет"), "non-ASCII suffix survives");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1, "tag still extracts despite non-ASCII context");
        assert_eq!(refs[0].kind, MediaKind::Photo);
    }

    #[test]
    fn empty_delta_returns_empty_string_no_state_change() {
        let mut x = MediaTagExtractor::new();
        let visible = x.feed("");
        assert_eq!(visible, "");
        let refs = x.take_attachments();
        assert!(refs.is_empty());
    }

    #[test]
    fn reset_clears_all_state() {
        let mut x = MediaTagExtractor::new();
        let _ = x.feed("<MEDIA: foo.png>");
        x.reset();
        let refs_after_reset = x.take_attachments();
        assert!(refs_after_reset.is_empty(), "reset clears accumulator");
        let _ = x.feed("<MEDIA: bar.png>");
        let refs = x.take_attachments();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source, MediaSource::Path(PathBuf::from("bar.png")));
    }

    // -- Use the OPEN_TAG constant in a smoke test so unused-const lints
    //    don't fire on the stub before GREEN. (Removed once feed() is real.)
    #[test]
    fn open_tag_constant_is_pure_ascii() {
        assert!(OPEN_TAG.is_ascii());
        assert_eq!(OPEN_TAG, "<MEDIA:");
    }
}
