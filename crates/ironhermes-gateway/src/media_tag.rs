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
    pub fn feed(&mut self, _delta: &str) -> String {
        unimplemented!()
    }

    /// Flush any held-back partial prefix at end-of-stream. Returns it as
    /// VISIBLE text — diverges from scrubber's discard semantics, per
    /// RESEARCH §Pattern Audit: user trust requires tag-like text never
    /// disappears.
    pub fn flush_tail(&mut self) -> String {
        unimplemented!()
    }

    /// Drain accumulated tags; subsequent calls return `vec![]` until new
    /// tags are fed.
    pub fn take_attachments(&mut self) -> Vec<MediaRef> {
        unimplemented!()
    }
}

impl Default for MediaTagExtractor {
    fn default() -> Self {
        Self::new()
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
