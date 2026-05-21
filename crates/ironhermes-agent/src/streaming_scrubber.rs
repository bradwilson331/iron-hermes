//! Phase 34a Plan 02 (MEM-READ-04): streaming context scrubber.
//!
//! State machine that strips `<memory-context>...</memory-context>` spans
//! from streaming LLM output deltas. Handles tags split across chunk
//! boundaries by holding back partial-tag tails.
//!
//! Ported from `hermes-agent/agent/memory_manager.py` `StreamingContextScrubber`.
//! The one-shot `sanitize_context` regex cannot survive chunk boundaries:
//! a `<memory-context>` opened in one delta and closed in a later delta
//! would leak its payload to the UI. This scrubber runs a small state
//! machine across deltas, holding back partial-tag tails and discarding
//! everything inside a span.

const OPEN_TAG: &str = "<memory-context>";  // 16 chars
const CLOSE_TAG: &str = "</memory-context>"; // 17 chars

/// Stateful scrubber for streaming text that may contain split memory-context
/// spans. Create one per turn (or call `reset()` between turns).
///
/// Usage pattern (see Plan 02 PATTERNS.md Arc<Mutex> flush pattern):
/// ```ignore
/// let scrubber = Arc::new(std::sync::Mutex::new(StreamingContextScrubber::new()));
/// let scrubber_cb = Arc::clone(&scrubber);
/// let stream_callback = Box::new(move |delta: &str| {
///     let visible = scrubber_cb.lock().unwrap().feed(delta);
///     if !visible.is_empty() { emit(visible); }
/// });
/// // After stream completes:
/// let tail = scrubber.lock().unwrap().flush();
/// if !tail.is_empty() { emit(tail); }
/// ```
pub struct StreamingContextScrubber {
    in_span: bool,
    buf: String,
}

impl StreamingContextScrubber {
    pub fn new() -> Self {
        Self {
            in_span: false,
            buf: String::new(),
        }
    }

    /// Reset to initial state (reuse for a new turn without reallocation).
    pub fn reset(&mut self) {
        self.in_span = false;
        self.buf.clear();
    }

    /// Feed a streaming delta. Returns the visible portion after scrubbing.
    ///
    /// Any trailing fragment that could be the start of an open/close tag is
    /// held back internally and surfaced on the next `feed()` call or
    /// discarded/emitted by `flush()`.
    pub fn feed(&mut self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }

        // Prepend any previously held partial-tag tail.
        let mut buf = if self.buf.is_empty() {
            text.to_owned()
        } else {
            let mut b = std::mem::take(&mut self.buf);
            b.push_str(text);
            b
        };

        let mut out = String::new();

        loop {
            if buf.is_empty() {
                break;
            }

            if self.in_span {
                let buf_lower = buf.to_lowercase();
                match buf_lower.find(CLOSE_TAG) {
                    None => {
                        // No close tag yet — hold back potential partial close-tag suffix.
                        let held = Self::max_partial_suffix(&buf, CLOSE_TAG);
                        if held > 0 {
                            let split = buf.len() - held;
                            self.buf = buf[split..].to_owned();
                        }
                        // Drop everything before the held portion (we're inside a span).
                        return out;
                    }
                    Some(idx) => {
                        // Found close tag — skip span content + tag, continue.
                        buf = buf[idx + CLOSE_TAG.len()..].to_owned();
                        self.in_span = false;
                    }
                }
            } else {
                let buf_lower = buf.to_lowercase();
                match buf_lower.find(OPEN_TAG) {
                    None => {
                        // No open tag — hold back potential partial open-tag suffix.
                        let held = Self::max_partial_suffix(&buf, OPEN_TAG);
                        if held > 0 {
                            let split = buf.len() - held;
                            out.push_str(&buf[..split]);
                            self.buf = buf[split..].to_owned();
                        } else {
                            out.push_str(&buf);
                        }
                        return out;
                    }
                    Some(idx) => {
                        // Emit text before the tag, enter span.
                        if idx > 0 {
                            out.push_str(&buf[..idx]);
                        }
                        buf = buf[idx + OPEN_TAG.len()..].to_owned();
                        self.in_span = true;
                    }
                }
            }
        }

        out
    }

    /// Emit any held-back buffer at end-of-stream.
    ///
    /// If we're still inside an unterminated span, the remaining content is
    /// discarded (safer: leaking partial memory context is worse than a
    /// truncated answer). Otherwise the held-back partial-tag tail is emitted
    /// verbatim (it turned out not to be a real tag).
    pub fn flush(&mut self) -> String {
        if self.in_span {
            self.buf.clear();
            self.in_span = false;
            return String::new();
        }
        let tail = std::mem::take(&mut self.buf);
        tail
    }

    /// Return the length of the longest buf-suffix that is a prefix of `tag`.
    ///
    /// Case-insensitive. Returns 0 if no suffix could start the tag.
    fn max_partial_suffix(buf: &str, tag: &str) -> usize {
        let tag_lower = tag.to_lowercase();
        let buf_lower = buf.to_lowercase();
        let max_check = buf_lower.len().min(tag_lower.len() - 1);
        for i in (1..=max_check).rev() {
            let suffix_start = buf_lower.len() - i;
            if tag_lower.starts_with(&buf_lower[suffix_start..]) {
                return i;
            }
        }
        0
    }
}

impl Default for StreamingContextScrubber {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_block_in_one_delta() {
        let mut s = StreamingContextScrubber::new();
        let out = s.feed("hello <memory-context>secret</memory-context> world");
        assert!(out.contains("hello"), "should contain 'hello'");
        assert!(out.contains("world"), "should contain 'world'");
        assert!(!out.contains("secret"), "should NOT contain 'secret'");
        assert!(!out.contains("<memory-context>"), "should NOT contain open tag");
        assert!(!out.contains("</memory-context>"), "should NOT contain close tag");
    }

    #[test]
    fn split_open_tag_across_two_deltas() {
        let mut s = StreamingContextScrubber::new();
        let out1 = s.feed("hi <memory-con");
        let out2 = s.feed("text>secret</memory-context> bye");
        let combined = out1 + &out2;
        assert!(combined.contains("hi "), "should contain 'hi '");
        assert!(combined.contains(" bye"), "should contain ' bye'");
        assert!(!combined.contains("secret"), "should NOT contain 'secret'");
        assert!(!combined.contains("<memory-con"), "should NOT leak partial open tag");
        assert!(!combined.contains("text>"), "should NOT leak tag fragment");
    }

    #[test]
    fn split_close_tag_across_two_deltas() {
        let mut s = StreamingContextScrubber::new();
        let out1 = s.feed("a<memory-context>secret</memory-con");
        let out2 = s.feed("text>b");
        let combined = out1 + &out2;
        assert!(combined.contains('a'), "should contain 'a'");
        assert!(combined.contains('b'), "should contain 'b'");
        assert!(!combined.contains("secret"), "should NOT contain 'secret'");
        assert!(!combined.contains("</memory-con"), "should NOT leak partial close tag");
    }

    #[test]
    fn partial_tail_held_then_completes() {
        let mut s = StreamingContextScrubber::new();
        // The partial tag tail should be held, not emitted
        let out1 = s.feed("ok <memory-cont");
        assert_eq!(out1, "ok ", "partial tag tail must be held back, not emitted");
        // Next delta disproves the tag — held buffer + new text should appear
        let out2 = s.feed("inues normally");
        let combined = out1 + &out2;
        assert!(
            combined.contains("<memory-cont"),
            "held partial tail must be emitted once it's proven not a tag"
        );
        assert!(
            combined.contains("inues normally"),
            "remainder should be emitted"
        );
    }

    #[test]
    fn span_never_closes_flush_returns_empty() {
        let mut s = StreamingContextScrubber::new();
        let out = s.feed("x<memory-context>open forever");
        assert!(out.contains('x'), "text before open tag should be emitted");
        assert!(!out.contains("open forever"), "span content should NOT be emitted");
        // flush() must discard the unterminated span and return "" (no leak)
        let tail = s.flush();
        assert_eq!(tail, "", "flush of unterminated span must return empty string");
    }

    #[test]
    fn two_complete_blocks_back_to_back() {
        let mut s = StreamingContextScrubber::new();
        let out = s.feed("<memory-context>a</memory-context><memory-context>b</memory-context>tail");
        assert_eq!(out, "tail", "only text after both blocks should be visible");
        let tail = s.flush();
        assert_eq!(tail, "", "flush should return empty after clean stream");
    }
}
