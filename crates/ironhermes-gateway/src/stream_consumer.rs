use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::adapter::PlatformAdapter;
use crate::markdown_v2::escape_outside_code_blocks;
use crate::rate_limiter::with_rate_limit_retry;

const EDIT_INTERVAL: Duration = Duration::from_millis(300);
const MAX_MESSAGE_LEN: usize = 4096;
const CURSOR: &str = "\u{2588}"; // Block cursor per D-01

/// Selects how [`StreamConsumer`] delivers a turn's response (Phase 47.6
/// Plan 09, D-13).
///
/// `EditInPlace` (the pre-existing, only mode before this plan) publishes a
/// placeholder message up front and streams the response into it via
/// throttled edits, finalizing with Markdown on the last flush. Telegram,
/// Discord and Slack all support real message edits and use this mode.
///
/// `SendOnce` buffers the entire turn and publishes it exactly once, at
/// final flush, via `send_message` — no edits at all. This exists for
/// adapters whose events are immutable (Buzz / Nostr, D-13): editing such an
/// adapter is a logged no-op ([`PlatformAdapter::edit_message`]), so driving
/// it through `EditInPlace` would silently publish a placeholder and then
/// lose the real response. See [`PlatformAdapter::supports_in_place_edits`]
/// for the switch that selects between the two at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    EditInPlace,
    SendOnce,
}

/// Consumes streaming LLM output and drives throttled message edits (or, in
/// [`DeliveryMode::SendOnce`], a single buffered send at the end of a turn).
///
/// - Appends block cursor during generation (D-01, `EditInPlace` only)
/// - Shows tool status during execution (D-02, `EditInPlace` only)
/// - Plain text during streaming edits, Markdown on final edit (D-03)
/// - Chains messages at paragraph boundaries when >4096 chars (D-04)
pub struct StreamConsumer {
    adapter: Arc<dyn PlatformAdapter>,
    chat_id: String,
    current_message_id: String,
    buffer: String,
    tool_line: Option<String>,
    last_edit: Instant,
    overflow_message_ids: Vec<String>,
    dirty: bool,
    mode: DeliveryMode,
    /// Reply-target event id for [`DeliveryMode::SendOnce`]'s first
    /// published message (Phase 47.6 Plan 08, T-47.6-08-REPLY). `None` by
    /// default and for every `EditInPlace` consumer — only
    /// [`Self::with_reply_to`] sets it, and only `handler.rs`'s Buzz
    /// CHANNEL turn construction opts in (never DM — D-13 unaffected).
    reply_to_id: Option<String>,
}

impl StreamConsumer {
    /// Create a new StreamConsumer in [`DeliveryMode::EditInPlace`] — the
    /// pre-existing constructor, UNCHANGED, so every existing caller and
    /// test keeps constructing the edit-in-place path exactly as before.
    ///
    /// `last_edit` is set to `Instant::now() - EDIT_INTERVAL` so the first
    /// flush is always immediate.
    pub fn new(
        adapter: Arc<dyn PlatformAdapter>,
        chat_id: impl Into<String>,
        placeholder_message_id: impl Into<String>,
    ) -> Self {
        Self {
            adapter,
            chat_id: chat_id.into(),
            current_message_id: placeholder_message_id.into(),
            buffer: String::new(),
            tool_line: None,
            last_edit: Instant::now()
                .checked_sub(EDIT_INTERVAL)
                .unwrap_or_else(Instant::now),
            overflow_message_ids: Vec::new(),
            dirty: false,
            mode: DeliveryMode::EditInPlace,
            reply_to_id: None,
        }
    }

    /// Create a new StreamConsumer in an explicit [`DeliveryMode`] (Phase
    /// 47.6 Plan 09). `placeholder_message_id` is `None` for
    /// [`DeliveryMode::SendOnce`] — there is no placeholder to hold; the
    /// first published message id becomes `current_message_id` once the
    /// final flush actually sends something.
    pub fn new_with_mode(
        adapter: Arc<dyn PlatformAdapter>,
        chat_id: impl Into<String>,
        placeholder_message_id: Option<String>,
        mode: DeliveryMode,
    ) -> Self {
        Self {
            adapter,
            chat_id: chat_id.into(),
            current_message_id: placeholder_message_id.unwrap_or_default(),
            buffer: String::new(),
            tool_line: None,
            last_edit: Instant::now()
                .checked_sub(EDIT_INTERVAL)
                .unwrap_or_else(Instant::now),
            overflow_message_ids: Vec::new(),
            dirty: false,
            mode,
            reply_to_id: None,
        }
    }

    /// Attach a reply-target event id (Buzz channel replies only, Phase 47.6
    /// Plan 08, T-47.6-08-REPLY): when set, [`Self::flush_send_once`]'s
    /// FIRST published message passes this id as `thread_id` to
    /// `PlatformAdapter::send_message`, letting `BuzzAdapter::send_message`'s
    /// `Channel` arm attach a NIP-10 `["e", id, "", "reply"]` marker tag
    /// (plus a best-effort `p` tag when the adapter recorded the original
    /// sender). Builder-style specifically so every existing `new` /
    /// `new_with_mode` call site — production and test — is completely
    /// unaffected; only `handler.rs`'s Buzz CHANNEL turn construction opts
    /// in (never DM: `send_dm` already interprets a `thread_id` as a plain
    /// event id and must keep doing exactly that — this method is never
    /// called for a DM turn).
    pub fn with_reply_to(mut self, reply_to_id: Option<String>) -> Self {
        self.reply_to_id = reply_to_id;
        self
    }

    /// Append a text chunk to the buffer.
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        self.dirty = true;
    }

    /// Phase 36.17.2.2 D-10: expose the post-flush final body as a borrowed
    /// slice. Used by the D-19 dispatch loop in
    /// `GatewayMessageHandler::run_agent` to construct the reinsert body
    /// (`format!("{final_body}\n\n{failed_tags}")`) when one or more
    /// attachments fail or trip the D-15 size pre-check. The body is the
    /// canonical text that was edited into the placeholder by `flush(true)`
    /// at the call site above — accessible AFTER the final flush has run.
    /// Owned by the consumer task and handed to the parent task via a
    /// `tokio::sync::oneshot::channel<String>` (see handler.rs:1308 area).
    pub fn final_body(&self) -> &str {
        &self.buffer
    }

    /// Set a tool status line shown during tool execution (D-02).
    /// Format: "\n\n⚙️ Running: {tool_name}..."
    ///
    /// Phase 47.6 Plan 09: in [`DeliveryMode::SendOnce`] this is a no-op —
    /// tool-status lines are transient decoration on a message that is about
    /// to be replaced by an edit; on an immutable-event surface each one
    /// would instead become a permanent, separate published event.
    pub fn tool_status(&mut self, tool_name: &str) {
        if self.mode == DeliveryMode::SendOnce {
            return;
        }
        self.tool_line = Some(format!("\n\n\u{2699}\u{fe0f} Running: {}...", tool_name));
        self.dirty = true;
    }

    /// Clear the tool status line before next content push.
    pub fn clear_tool_status(&mut self) {
        self.tool_line = None;
        self.dirty = true;
    }

    /// Flush the current buffer.
    ///
    /// Dispatches on [`DeliveryMode`] (Phase 47.6 Plan 09) — `EditInPlace`
    /// keeps the exact pre-existing behaviour described on
    /// [`Self::flush_edit_in_place`]; `SendOnce` buffers until the final
    /// flush and then sends once, described on [`Self::flush_send_once`].
    pub async fn flush(&mut self, final_edit: bool) -> Result<()> {
        match self.mode {
            DeliveryMode::EditInPlace => self.flush_edit_in_place(final_edit).await,
            DeliveryMode::SendOnce => self.flush_send_once(final_edit).await,
        }
    }

    /// Flush the current buffer to an edit-capable adapter (Telegram,
    /// Discord, Slack).
    ///
    /// - If `final_edit` is false and the buffer hasn't changed or the throttle
    ///   interval hasn't elapsed, this is a no-op.
    /// - If `final_edit` is true, edits with Markdown parse mode and no cursor.
    /// - If content exceeds `MAX_MESSAGE_LEN`, splits at the best paragraph
    ///   boundary and chains a new message. This applies to both final and
    ///   intermediate flushes — Telegram rejects editMessageText calls with
    ///   content > 4096 chars regardless of parse_mode.
    ///
    /// Byte-for-byte unchanged by Phase 47.6 Plan 09 (mode addition, not a
    /// rewrite) — this is the pre-existing, already regression-tested
    /// Telegram overflow/escaping/throttle behaviour.
    async fn flush_edit_in_place(&mut self, final_edit: bool) -> Result<()> {
        let now = Instant::now();

        // Throttle: skip if not final, buffer unchanged, or interval not elapsed
        if !final_edit && (!self.dirty || now.duration_since(self.last_edit) < EDIT_INTERVAL) {
            return Ok(());
        }

        if final_edit {
            // RC-1 / REQ-37.2-03: do not issue an empty editMessageText (Telegram 400).
            // An empty buffer means the stream produced no text — the caller (handler.rs)
            // handles this case via body_rx + deliver_turn_end_fallback (final_response
            // edit or placeholder delete). Nothing to edit here; mark clean and return.
            if self.buffer.is_empty() {
                self.dirty = false;
                return Ok(());
            }

            // Final edit: Markdown mode, no cursor, no tool line.
            // Must chunk at MAX_MESSAGE_LEN — Telegram rejects editMessageText
            // with content > 4096 chars even in Markdown mode (Bug fix: the
            // overflow check was previously only in the non-final branch, causing
            // silent 400 errors and dropped long-form responses).
            let mut remaining = self.buffer.clone();
            let mut first_chunk = true;
            loop {
                if remaining.len() <= MAX_MESSAGE_LEN {
                    // Last (or only) chunk: edit the current placeholder with
                    // Telegram MarkdownV2 (Phase 36.17.2.2-04 D-01 rename;
                    // Phase 36.17.2.2-05 D-01 + D-04 escape applied).
                    //
                    // `escape_outside_code_blocks` walks the body to escape
                    // the 18 MarkdownV2 reserved chars OUTSIDE of fenced code
                    // blocks, inline-code spans, and `[label](url)` link
                    // URLs — preserving real model-authored markdown syntax
                    // (`*bold*`, ` ```fence``` `, links) while ensuring
                    // literal reserved chars in prose render correctly.
                    // The D-02 single-retry-as-plain-text fallback inside
                    // `TelegramAdapter::edit_message_markdown_v2` is now the
                    // safety net for the rare model-emitted-malformed-markdown
                    // case, not the common-path crutch.
                    self.adapter
                        .edit_message_markdown_v2(
                            &self.chat_id,
                            &self.current_message_id,
                            &escape_outside_code_blocks(&remaining),
                        )
                        .await?;
                    break;
                }

                // Content exceeds limit — split at best boundary
                let split_point = find_split_point(&remaining, MAX_MESSAGE_LEN);
                let chunk = remaining[..split_point].to_string();
                let rest = remaining[split_point..].trim_start().to_string();

                if first_chunk {
                    // Finalize the placeholder message with the first chunk (plain
                    // text, no Markdown, to avoid partial-markdown parse failures)
                    self.adapter
                        .edit_message(&self.chat_id, &self.current_message_id, &chunk)
                        .await?;
                    self.overflow_message_ids
                        .push(self.current_message_id.clone());
                    first_chunk = false;
                } else {
                    // Phase 36.17.2.2-05 D-Discretion: send overflow chunks
                    // as MarkdownV2 with pre-escaped body so the entire
                    // final response renders consistently (per CONTEXT.md
                    // "every overflow chunk uses send_message_markdown_v2").
                    let new_msg = self
                        .adapter
                        .send_message_markdown_v2(
                            &self.chat_id,
                            &escape_outside_code_blocks(&chunk),
                            None,
                        )
                        .await?;
                    self.overflow_message_ids
                        .push(self.current_message_id.clone());
                    self.current_message_id = new_msg.message_id;
                }

                // Phase 36.17.2.2-05: send the rest as a new MarkdownV2
                // message and continue the loop (the rest may either fit
                // on the next iteration and become the final MarkdownV2
                // edit, or split again into more overflow chunks).
                let new_msg = self
                    .adapter
                    .send_message_markdown_v2(
                        &self.chat_id,
                        &escape_outside_code_blocks(&rest),
                        None,
                    )
                    .await?;
                self.current_message_id = new_msg.message_id;
                remaining = rest;

                // If after sending the new message the remaining content fits,
                // the loop will edit it with Markdown on the next iteration.
                // Guard against infinite loops on empty remainder.
                if remaining.is_empty() {
                    break;
                }
            }
        } else {
            // Build display: buffer + optional tool line + cursor
            let mut display = self.buffer.clone();
            if let Some(ref tl) = self.tool_line {
                display.push_str(tl);
            }
            display.push_str(CURSOR);

            // Handle 4096-char overflow
            if display.len() > MAX_MESSAGE_LEN {
                let split_point = find_split_point(&self.buffer, MAX_MESSAGE_LEN - CURSOR.len());
                let finalized = self.buffer[..split_point].to_string();
                let remainder = self.buffer[split_point..].trim_start().to_string();

                // Finalize the current message (no cursor, no markdown)
                self.adapter
                    .edit_message(&self.chat_id, &self.current_message_id, &finalized)
                    .await?;

                // Send a new message for the continuation
                let new_msg = self
                    .adapter
                    .send_message(&self.chat_id, &remainder, None)
                    .await?;

                self.overflow_message_ids
                    .push(self.current_message_id.clone());
                self.current_message_id = new_msg.message_id;
                self.buffer = remainder;
            } else {
                self.adapter
                    .edit_message(&self.chat_id, &self.current_message_id, &display)
                    .await?;
            }
        }

        self.last_edit = now;
        self.dirty = false;
        Ok(())
    }

    /// Flush the current buffer to a non-edit-capable adapter (Buzz / D-13).
    ///
    /// - A non-final flush is always a no-op: nothing is published until the
    ///   turn ends, so there is no half-finished answer to throttle or
    ///   protect against a premature publish.
    /// - A final flush with an empty buffer sends nothing (matches the
    ///   `EditInPlace` empty-buffer short circuit — `handler.rs`'s turn-end
    ///   fallback owns the empty-turn case).
    /// - A final flush with content sends the buffer via `send_message`,
    ///   splitting on the SAME [`find_split_point`] boundary logic and the
    ///   same [`MAX_MESSAGE_LEN`] limit `flush_edit_in_place` uses, sending
    ///   each chunk in order. Unlike the edit path, the FIRST chunk is also a
    ///   SEND — there is no placeholder to finalize with an edit — which is
    ///   exactly the spot the cross-AI review identified as losing the
    ///   leading chunk on a non-editing platform. Every published id is
    ///   recorded in `overflow_message_ids` the same way the edit path does
    ///   (all-but-the-last chunk), so `message_ids()` behaves consistently
    ///   regardless of mode.
    /// - Markdown: plain `send_message`, not the MarkdownV2 surface — Buzz
    ///   has no MarkdownV2 dialect (D-13's `send_message_markdown_v2`
    ///   delegates to plain send already), and a send-once adapter that DID
    ///   have a Markdown dialect would still want its own plain send here
    ///   rather than inheriting Telegram's escaping convention.
    async fn flush_send_once(&mut self, final_edit: bool) -> Result<()> {
        if !final_edit {
            return Ok(());
        }

        if self.buffer.is_empty() {
            self.dirty = false;
            return Ok(());
        }

        let mut remaining = self.buffer.clone();
        let mut first = true;
        loop {
            let is_last = remaining.len() <= MAX_MESSAGE_LEN;
            let (chunk, rest) = if is_last {
                (remaining.clone(), String::new())
            } else {
                let split_point = find_split_point(&remaining, MAX_MESSAGE_LEN);
                (
                    remaining[..split_point].to_string(),
                    remaining[split_point..].trim_start().to_string(),
                )
            };

            // T-47.6-08-REPLY: only the FIRST published message threads
            // onto the triggering mention — this is a reply to what was
            // asked, not to a continuation chunk of our own answer.
            let thread_id_for_send = if first {
                self.reply_to_id.as_deref()
            } else {
                None
            };
            let sent = self
                .adapter
                .send_message(&self.chat_id, &chunk, thread_id_for_send)
                .await?;
            if !first {
                self.overflow_message_ids
                    .push(self.current_message_id.clone());
            }
            self.current_message_id = sent.message_id;
            first = false;

            if is_last {
                break;
            }
            remaining = rest;
        }

        self.last_edit = Instant::now();
        self.dirty = false;
        Ok(())
    }

    /// Returns all message IDs used (current + overflow) for cleanup.
    pub fn message_ids(&self) -> Vec<String> {
        let mut ids = self.overflow_message_ids.clone();
        ids.push(self.current_message_id.clone());
        ids
    }

    /// Current message ID (the one being actively edited).
    pub fn current_message_id(&self) -> &str {
        &self.current_message_id
    }
}

/// Send `text` to `chat_id` as one or more messages, each capped at
/// [`MAX_MESSAGE_LEN`] chars (G-41.1-5).
///
/// Reuses [`find_split_point`] — the same paragraph/newline/sentence-boundary
/// splitter the streaming final-edit overflow path (`StreamConsumer::flush`)
/// already relies on — so oversized one-shot replies (e.g. the `/skills`
/// catalog) chunk the same way long streamed responses do, instead of
/// hitting Telegram's `400 Bad Request: text is too long` via a single
/// unguarded `send_message` call.
///
/// Unlike `StreamConsumer::flush`, this has no placeholder message to edit
/// and no cursor/markdown concerns — it is a plain sequential multi-send.
/// Each chunk goes through [`with_rate_limit_retry`] so per-chunk 429
/// backoff is preserved, matching every other adapter call in the fast path.
pub(crate) async fn send_chunked(
    adapter: &Arc<dyn PlatformAdapter>,
    chat_id: &str,
    text: &str,
) -> Result<()> {
    let mut remaining = text.to_string();

    loop {
        if remaining.len() <= MAX_MESSAGE_LEN {
            with_rate_limit_retry(|| adapter.send_message(chat_id, &remaining, None)).await?;
            return Ok(());
        }

        let split_point = find_split_point(&remaining, MAX_MESSAGE_LEN);
        let chunk = remaining[..split_point].to_string();
        with_rate_limit_retry(|| adapter.send_message(chat_id, &chunk, None)).await?;

        remaining = remaining[split_point..].trim_start().to_string();
        if remaining.is_empty() {
            return Ok(());
        }
    }
}

/// Find the best split point in `text` at or before `max_len`.
///
/// Priority: last `\n\n` → last `\n` → last `. ` → `max_len` (hard split).
pub(crate) fn find_split_point(text: &str, max_len: usize) -> usize {
    if text.len() <= max_len {
        return text.len();
    }

    let slice = ironhermes_core::truncate_on_char_boundary(text, max_len);

    // Try last double newline (paragraph boundary)
    if let Some(pos) = slice.rfind("\n\n") {
        return pos + 2;
    }

    // Try last single newline
    if let Some(pos) = slice.rfind('\n') {
        return pos + 1;
    }

    // Try last sentence boundary
    if let Some(pos) = slice.rfind(". ") {
        return pos + 2;
    }

    // Hard split at max_len (on a char boundary)
    let mut split = max_len;
    while !text.is_char_boundary(split) {
        split -= 1;
    }
    split
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::{MessageResponse, Platform};
    use std::sync::Mutex;

    // -------------------------------------------------------------------------
    // MockAdapter — records calls for assertions
    // -------------------------------------------------------------------------

    #[derive(Debug)]
    #[allow(dead_code)] // test-recording enum; fields constructed for Debug output and future assertion patterns
    enum AdapterCall {
        EditMessage {
            chat_id: String,
            message_id: String,
            content: String,
        },
        EditMessageMarkdownV2 {
            chat_id: String,
            message_id: String,
            content: String,
        },
        SendMessage {
            chat_id: String,
            content: String,
        },
        SendMessageMarkdownV2 {
            chat_id: String,
            content: String,
        },
    }

    struct MockAdapter {
        calls: Arc<Mutex<Vec<AdapterCall>>>,
        /// message_id to return for send_message
        next_message_id: Arc<Mutex<String>>,
    }

    impl MockAdapter {
        fn new() -> (Arc<Self>, Arc<Mutex<Vec<AdapterCall>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let adapter = Arc::new(MockAdapter {
                calls: calls.clone(),
                next_message_id: Arc::new(Mutex::new("msg-2".to_string())),
            });
            (adapter, calls)
        }
    }

    #[async_trait]
    impl PlatformAdapter for MockAdapter {
        fn platform(&self) -> Platform {
            Platform::Telegram
        }

        async fn send_message(
            &self,
            chat_id: &str,
            content: &str,
            _thread_id: Option<&str>,
        ) -> Result<MessageResponse> {
            let id = self.next_message_id.lock().unwrap().clone();
            self.calls.lock().unwrap().push(AdapterCall::SendMessage {
                chat_id: chat_id.to_string(),
                content: content.to_string(),
            });
            Ok(MessageResponse {
                message_id: id,
                chat_id: chat_id.to_string(),
                platform: Platform::Telegram,
            })
        }

        async fn edit_message(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()> {
            self.calls.lock().unwrap().push(AdapterCall::EditMessage {
                chat_id: chat_id.to_string(),
                message_id: message_id.to_string(),
                content: content.to_string(),
            });
            Ok(())
        }

        async fn edit_message_markdown_v2(
            &self,
            chat_id: &str,
            message_id: &str,
            content: &str,
        ) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(AdapterCall::EditMessageMarkdownV2 {
                    chat_id: chat_id.to_string(),
                    message_id: message_id.to_string(),
                    content: content.to_string(),
                });
            Ok(())
        }

        async fn send_message_markdown_v2(
            &self,
            chat_id: &str,
            content: &str,
            _thread_id: Option<&str>,
        ) -> Result<MessageResponse> {
            // Phase 36.17.2.2-05: record overflow-chunk MarkdownV2 sends
            // as a distinct call so tests can distinguish plain `SendMessage`
            // (used by the intermediate-edit / non-final cursor-strip
            // overflow path per D-03) from MarkdownV2 sends (the final-edit
            // overflow path).
            let id = self.next_message_id.lock().unwrap().clone();
            self.calls
                .lock()
                .unwrap()
                .push(AdapterCall::SendMessageMarkdownV2 {
                    chat_id: chat_id.to_string(),
                    content: content.to_string(),
                });
            Ok(MessageResponse {
                message_id: id,
                chat_id: chat_id.to_string(),
                platform: Platform::Telegram,
            })
        }

        async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> Result<()> {
            Ok(())
        }

        fn is_running(&self) -> bool {
            true
        }
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_flush_non_final_appends_cursor() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");
        sc.push("hello");
        sc.flush(false).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            AdapterCall::EditMessage { content, .. } => {
                assert_eq!(content, "hello\u{2588}");
            }
            other => panic!("Expected EditMessage, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_flush_final_strips_cursor_and_uses_markdown() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");
        sc.push("hello");
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            AdapterCall::EditMessageMarkdownV2 { content, .. } => {
                assert!(
                    !content.contains('\u{2588}'),
                    "Final edit should not have cursor"
                );
                assert_eq!(content, "hello");
            }
            other => panic!("Expected EditMessageMarkdownV2, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_flush_throttle_within_300ms() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");

        // First flush — should go through (last_edit set to now-300ms in constructor)
        sc.push("first");
        sc.flush(false).await.unwrap();

        // Immediate second flush — should be throttled
        sc.push(" second");
        sc.flush(false).await.unwrap();

        let calls = calls.lock().unwrap();
        // Only 1 edit call — second was throttled
        assert_eq!(
            calls.len(),
            1,
            "Second flush within 300ms should be throttled"
        );
    }

    #[tokio::test]
    async fn test_flush_after_300ms_sends_edit() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");

        // First flush
        sc.push("first");
        sc.flush(false).await.unwrap();

        // Manually backdating last_edit to simulate 300ms elapsed
        sc.last_edit = Instant::now()
            .checked_sub(Duration::from_millis(350))
            .unwrap_or_else(Instant::now);

        // Second flush after interval
        sc.push(" second");
        sc.flush(false).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "Second flush after 300ms should send edit");
    }

    #[tokio::test]
    async fn test_overflow_chains_new_message() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");

        // Build a buffer that exceeds 4096 chars (display = buffer + cursor)
        // Use two paragraphs so there's a \n\n split point
        // Buffer must be > 4096 to trigger overflow (display = buffer + "\u{2588}")
        let para1 = "A".repeat(2500);
        let para2 = "B".repeat(2500);
        let big_content = format!("{}\n\n{}", para1, para2);

        sc.push(&big_content);
        sc.flush(false).await.unwrap();

        let calls = calls.lock().unwrap();
        // Should have: 1 edit_message (finalize first part) + 1 send_message (new message)
        let edit_count = calls
            .iter()
            .filter(|c| matches!(c, AdapterCall::EditMessage { .. }))
            .count();
        let send_count = calls
            .iter()
            .filter(|c| matches!(c, AdapterCall::SendMessage { .. }))
            .count();
        assert_eq!(edit_count, 1, "Should finalize first message via edit");
        assert_eq!(send_count, 1, "Should send new message for overflow");
    }

    /// Regression test: final flush with content > 4096 chars must chunk, not
    /// attempt a single editMessageText that Telegram would reject with 400.
    #[tokio::test]
    async fn test_final_flush_overflow_chunks_long_content() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");

        // Build content clearly over 4096 chars with a paragraph split point
        let para1 = "A".repeat(2500);
        let para2 = "B".repeat(2500);
        let big_content = format!("{}\n\n{}", para1, para2);
        assert!(
            big_content.len() > MAX_MESSAGE_LEN,
            "test content must exceed limit"
        );

        sc.push(&big_content);
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        // First chunk: edit_message (plain text, no Markdown) on placeholder
        let edit_plain_count = calls
            .iter()
            .filter(|c| matches!(c, AdapterCall::EditMessage { .. }))
            .count();
        // Phase 36.17.2.2-05: continuation chunk(s) now route through
        // send_message_markdown_v2 (D-Discretion: "every overflow chunk
        // uses send_message_markdown_v2 so the entire response renders
        // consistently"). Plain SendMessage from the final branch is now
        // never expected.
        let send_md_count = calls
            .iter()
            .filter(|c| matches!(c, AdapterCall::SendMessageMarkdownV2 { .. }))
            .count();
        let send_plain_count = calls
            .iter()
            .filter(|c| matches!(c, AdapterCall::SendMessage { .. }))
            .count();
        // Final chunk: edit_message_markdown_v2 on the last message id
        let edit_md_count = calls
            .iter()
            .filter(|c| matches!(c, AdapterCall::EditMessageMarkdownV2 { .. }))
            .count();

        assert!(
            edit_plain_count >= 1,
            "Should have at least one plain edit for the first chunk"
        );
        assert!(
            send_md_count >= 1,
            "Should send at least one new MarkdownV2 message for overflow content (D-Discretion)"
        );
        assert_eq!(
            send_plain_count, 0,
            "Final-edit overflow path must NOT use plain SendMessage (D-Discretion: every overflow chunk is MarkdownV2)"
        );
        assert_eq!(
            edit_md_count, 1,
            "Should have exactly one final markdown edit for the last chunk"
        );
    }

    #[tokio::test]
    async fn test_tool_status_appears_in_display() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");
        sc.push("searching...");
        sc.tool_status("search");
        sc.flush(false).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            AdapterCall::EditMessage { content, .. } => {
                assert!(
                    content.contains("Running: search"),
                    "Tool status should be in display: {}",
                    content
                );
            }
            other => panic!("Expected EditMessage, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_clear_tool_status_removes_line() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");
        sc.push("content");
        sc.tool_status("search");
        sc.clear_tool_status();
        sc.flush(false).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            AdapterCall::EditMessage { content, .. } => {
                assert!(
                    !content.contains("Running:"),
                    "Tool status should be cleared: {}",
                    content
                );
            }
            other => panic!("Expected EditMessage, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_final_edit_uses_edit_message_markdown_v2() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");
        sc.push("**bold** text");
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(
            matches!(&calls[0], AdapterCall::EditMessageMarkdownV2 { .. }),
            "Final edit should use edit_message_markdown_v2, got {:?}",
            calls[0]
        );
    }

    #[test]
    fn test_find_split_point_paragraph_boundary() {
        let text = format!("{}\n\n{}", "A".repeat(2000), "B".repeat(2000));
        let split = find_split_point(&text, 2500);
        // Should split after the \n\n at position 2002
        assert_eq!(split, 2002, "Should split after paragraph break");
    }

    #[test]
    fn test_find_split_point_no_paragraph_uses_newline() {
        let text = format!("{}\n{}", "A".repeat(2000), "B".repeat(2000));
        let split = find_split_point(&text, 2500);
        // Should split after the \n at position 2001
        assert_eq!(split, 2001, "Should split after newline");
    }

    #[test]
    fn test_find_split_point_hard_split_when_no_boundary() {
        let text = "A".repeat(5000);
        let split = find_split_point(&text, 4096);
        assert_eq!(split, 4096, "Should hard split at max_len");
    }

    #[test]
    fn test_find_split_point_short_text() {
        let text = "short";
        let split = find_split_point(text, 4096);
        assert_eq!(split, 5, "Short text returns full length");
    }

    // -------------------------------------------------------------------------
    // send_chunked — G-41.1-5 regression coverage (Telegram fast-path chunking)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_send_chunked_fits_in_one_message() {
        let (mock, calls) = MockAdapter::new();
        let adapter: Arc<dyn PlatformAdapter> = mock;
        send_chunked(&adapter, "chat1", "hello").await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "Text under the limit sends exactly once");
        match &calls[0] {
            AdapterCall::SendMessage { content, .. } => assert_eq!(content, "hello"),
            other => panic!("Expected SendMessage, got {:?}", other),
        }
    }

    /// Regression test for G-41.1-5: a >4096-char reply (e.g. the /skills
    /// catalog) must be split into multiple <=4096-char messages instead of
    /// hitting Telegram's 400 "text is too long" via one unguarded send.
    #[tokio::test]
    async fn test_send_chunked_splits_oversized_text_into_bounded_chunks() {
        let (mock, calls) = MockAdapter::new();
        let adapter: Arc<dyn PlatformAdapter> = mock;

        let para1 = "A".repeat(2500);
        let para2 = "B".repeat(2500);
        let big_content = format!("{}\n\n{}", para1, para2);
        assert!(
            big_content.len() > MAX_MESSAGE_LEN,
            "test content must exceed the limit"
        );

        send_chunked(&adapter, "chat1", &big_content).await.unwrap();

        let calls = calls.lock().unwrap();
        assert!(
            calls.len() >= 2,
            "Oversized text must be split into multiple messages, got {}",
            calls.len()
        );

        let mut reassembled = String::new();
        for call in calls.iter() {
            match call {
                AdapterCall::SendMessage { content, .. } => {
                    assert!(
                        content.len() <= MAX_MESSAGE_LEN,
                        "Every chunk must be <=MAX_MESSAGE_LEN, got {}",
                        content.len()
                    );
                    reassembled.push_str(content);
                }
                other => panic!("Expected only SendMessage calls, got {:?}", other),
            }
        }
        assert_eq!(
            reassembled, big_content,
            "Concatenated chunks must reproduce the original content"
        );
    }

    #[tokio::test]
    async fn test_send_chunked_hard_split_with_no_boundary() {
        let (mock, calls) = MockAdapter::new();
        let adapter: Arc<dyn PlatformAdapter> = mock;

        // No newline/sentence boundary anywhere — forces the hard-split path.
        let big_content = "A".repeat(9000);
        send_chunked(&adapter, "chat1", &big_content).await.unwrap();

        let calls = calls.lock().unwrap();
        assert!(calls.len() >= 3, "9000 chars at 4096/chunk needs >=3 sends");
        for call in calls.iter() {
            match call {
                AdapterCall::SendMessage { content, .. } => {
                    assert!(content.len() <= MAX_MESSAGE_LEN);
                }
                other => panic!("Expected only SendMessage calls, got {:?}", other),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Wave 0 RED test — REQ-37.2-03
    // Plan 02 turns this GREEN by adding an early-return guard to flush(true)
    // that skips edit_message_markdown_v2 when the buffer is empty.
    // -----------------------------------------------------------------------

    /// RED test: `flush(true)` on an empty buffer must make zero adapter calls.
    ///
    /// Current production code calls `edit_message_markdown_v2(&chat_id, &msg_id, "")`
    /// unconditionally (stream_consumer.rs:126-131 — `remaining.len() (0) <= MAX_MESSAGE_LEN`
    /// path), which Telegram rejects with a 400 error. This assertion therefore FAILS today.
    ///
    /// Plan 02 Task 1 turns it GREEN by adding:
    /// ```
    /// if self.buffer.is_empty() { self.dirty = false; return Ok(()); }
    /// ```
    /// at the top of the `if final_edit` branch.
    #[tokio::test]
    async fn flush_true_empty_buffer_does_not_emit_empty_edit() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");
        // push nothing — buffer stays empty
        sc.flush(true).await.unwrap();
        let calls = calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "flush(true) on empty buffer must make zero adapter calls, got {:?}",
            calls.len()
        );
    }

    // -------------------------------------------------------------------------
    // Phase 47.6 Plan 09 — DeliveryMode::SendOnce (D-13)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn edit_mode_is_the_default_for_an_adapter_that_supports_edits() {
        // A fake adapter reporting edit support (the MockAdapter default)
        // drives the existing placeholder-and-edit path, call for call,
        // unchanged — `StreamConsumer::new` always constructs EditInPlace.
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");
        sc.push("hello");
        sc.flush(false).await.unwrap();
        sc.push(" world");
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, AdapterCall::EditMessage { .. })),
            "edit mode should still issue plain edits for intermediate flushes"
        );
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, AdapterCall::EditMessageMarkdownV2 { .. })),
            "edit mode should still finalize with a markdown edit"
        );
    }

    #[tokio::test]
    async fn send_once_mode_issues_no_edit_calls() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new_with_mode(adapter, "chat1", None, DeliveryMode::SendOnce);
        sc.push("hello ");
        sc.flush(false).await.unwrap();
        sc.push("world");
        sc.flush(false).await.unwrap();
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert!(
            !calls.iter().any(|c| matches!(
                c,
                AdapterCall::EditMessage { .. } | AdapterCall::EditMessageMarkdownV2 { .. }
            )),
            "send-once mode must issue zero edit calls, got {:?}",
            *calls
        );
    }

    #[tokio::test]
    async fn send_once_mode_sends_exactly_one_message_for_a_short_response() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new_with_mode(adapter, "chat1", None, DeliveryMode::SendOnce);
        sc.push("hello world");
        sc.flush(false).await.unwrap();
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "send-once mode should send exactly once for a short response, got {:?}",
            *calls
        );
        match &calls[0] {
            AdapterCall::SendMessage { content, .. } => assert_eq!(content, "hello world"),
            other => panic!("Expected SendMessage, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn send_once_mode_sends_nothing_before_the_final_flush() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new_with_mode(adapter, "chat1", None, DeliveryMode::SendOnce);
        sc.push("partial answer");
        sc.flush(false).await.unwrap();
        sc.push(" still going");
        sc.flush(false).await.unwrap();

        let calls = calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "intermediate flushes in send-once mode must produce no sends, got {:?}",
            *calls
        );
    }

    #[tokio::test]
    async fn send_once_mode_chunks_a_long_response_without_losing_the_first_chunk() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new_with_mode(adapter, "chat1", None, DeliveryMode::SendOnce);

        let para1 = "A".repeat(2500);
        let para2 = "B".repeat(2500);
        let big_content = format!("{}\n\n{}", para1, para2);
        assert!(big_content.len() > MAX_MESSAGE_LEN);

        sc.push(&big_content);
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert!(
            calls.len() >= 2,
            "oversized content must be split into multiple sends, got {}",
            calls.len()
        );
        let mut reassembled = String::new();
        for call in calls.iter() {
            match call {
                AdapterCall::SendMessage { content, .. } => reassembled.push_str(content),
                other => panic!(
                    "send-once mode must use only SendMessage calls, got {:?}",
                    other
                ),
            }
        }
        assert_eq!(
            reassembled, big_content,
            "in-order concatenation of sends must reconstruct the whole body \
             with nothing missing from the front"
        );
    }

    #[tokio::test]
    async fn edit_mode_chunking_is_unchanged() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");

        let para1 = "A".repeat(2500);
        let para2 = "B".repeat(2500);
        let big_content = format!("{}\n\n{}", para1, para2);
        sc.push(&big_content);
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, AdapterCall::EditMessage { .. })),
            "edit mode must still finalize the placeholder with the first chunk"
        );
    }

    #[tokio::test]
    async fn send_once_mode_empty_stream_sends_nothing() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new_with_mode(adapter, "chat1", None, DeliveryMode::SendOnce);
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "an empty buffer at final flush must send nothing, got {:?}",
            *calls
        );
    }

    #[tokio::test]
    async fn send_once_mode_tool_status_publishes_nothing() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new_with_mode(adapter, "chat1", None, DeliveryMode::SendOnce);
        sc.tool_status("web_search");
        sc.flush(false).await.unwrap();
        sc.push("the answer");
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "tool-status updates must publish nothing in send-once mode, got {:?}",
            *calls
        );
        match &calls[0] {
            AdapterCall::SendMessage { content, .. } => {
                assert!(
                    !content.contains("Running:"),
                    "tool status line must not leak into the published body: {}",
                    content
                );
            }
            other => panic!("Expected SendMessage, got {:?}", other),
        }
    }
}
