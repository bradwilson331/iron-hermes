use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::adapter::PlatformAdapter;

const EDIT_INTERVAL: Duration = Duration::from_millis(300);
const MAX_MESSAGE_LEN: usize = 4096;
const CURSOR: &str = "\u{2588}"; // Block cursor per D-01

/// Consumes streaming LLM output and drives throttled Telegram message edits.
///
/// - Appends block cursor during generation (D-01)
/// - Shows tool status during execution (D-02)
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
}

impl StreamConsumer {
    /// Create a new StreamConsumer.
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
        }
    }

    /// Append a text chunk to the buffer.
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        self.dirty = true;
    }

    /// Set a tool status line shown during tool execution (D-02).
    /// Format: "\n\n⚙️ Running: {tool_name}..."
    pub fn tool_status(&mut self, tool_name: &str) {
        self.tool_line = Some(format!("\n\n\u{2699}\u{fe0f} Running: {}...", tool_name));
        self.dirty = true;
    }

    /// Clear the tool status line before next content push.
    pub fn clear_tool_status(&mut self) {
        self.tool_line = None;
        self.dirty = true;
    }

    /// Flush the current buffer to Telegram.
    ///
    /// - If `final_edit` is false and the buffer hasn't changed or the throttle
    ///   interval hasn't elapsed, this is a no-op.
    /// - If `final_edit` is true, edits with Markdown parse mode and no cursor.
    /// - If content exceeds `MAX_MESSAGE_LEN`, splits at the best paragraph
    ///   boundary and chains a new message.
    pub async fn flush(&mut self, final_edit: bool) -> Result<()> {
        let now = Instant::now();

        // Throttle: skip if not final, buffer unchanged, or interval not elapsed
        if !final_edit && (!self.dirty || now.duration_since(self.last_edit) < EDIT_INTERVAL) {
            return Ok(());
        }

        if final_edit {
            // Final edit: Markdown mode, no cursor, no tool line
            let content = self.buffer.clone();
            self.adapter
                .edit_message_markdown(&self.chat_id, &self.current_message_id, &content)
                .await?;
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

/// Find the best split point in `text` at or before `max_len`.
///
/// Priority: last `\n\n` → last `\n` → last `. ` → `max_len` (hard split).
fn find_split_point(text: &str, max_len: usize) -> usize {
    if text.len() <= max_len {
        return text.len();
    }

    let slice = &text[..max_len];

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
    enum AdapterCall {
        EditMessage {
            chat_id: String,
            message_id: String,
            content: String,
        },
        EditMessageMarkdown {
            chat_id: String,
            message_id: String,
            content: String,
        },
        SendMessage {
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

        async fn edit_message(
            &self,
            chat_id: &str,
            message_id: &str,
            content: &str,
        ) -> Result<()> {
            self.calls.lock().unwrap().push(AdapterCall::EditMessage {
                chat_id: chat_id.to_string(),
                message_id: message_id.to_string(),
                content: content.to_string(),
            });
            Ok(())
        }

        async fn edit_message_markdown(
            &self,
            chat_id: &str,
            message_id: &str,
            content: &str,
        ) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(AdapterCall::EditMessageMarkdown {
                    chat_id: chat_id.to_string(),
                    message_id: message_id.to_string(),
                    content: content.to_string(),
                });
            Ok(())
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
            AdapterCall::EditMessageMarkdown { content, .. } => {
                assert!(!content.contains('\u{2588}'), "Final edit should not have cursor");
                assert_eq!(content, "hello");
            }
            other => panic!("Expected EditMessageMarkdown, got {:?}", other),
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
        assert_eq!(calls.len(), 1, "Second flush within 300ms should be throttled");
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
    async fn test_final_edit_uses_edit_message_markdown() {
        let (adapter, calls) = MockAdapter::new();
        let mut sc = StreamConsumer::new(adapter, "chat1", "msg-1");
        sc.push("**bold** text");
        sc.flush(true).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(
            matches!(&calls[0], AdapterCall::EditMessageMarkdown { .. }),
            "Final edit should use edit_message_markdown, got {:?}",
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
}
