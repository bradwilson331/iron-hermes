use ironhermes_core::ChatMessage;
use tracing::{debug, info};

/// Rough token estimation (~4 chars per token).
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4 + 1
}

/// Estimate tokens for a single message.
pub fn estimate_message_tokens(msg: &ChatMessage) -> usize {
    let mut tokens = 4; // message overhead

    if let Some(content) = msg.content_text() {
        tokens += estimate_tokens(content);
    }

    if let Some(ref tool_calls) = msg.tool_calls {
        for tc in tool_calls {
            tokens += estimate_tokens(&tc.function.name);
            tokens += estimate_tokens(&tc.function.arguments);
            tokens += 4; // tool call overhead
        }
    }

    if let Some(ref name) = msg.name {
        tokens += estimate_tokens(name);
    }

    tokens
}

/// Estimate total tokens for a message list.
pub fn estimate_messages_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum::<usize>() + 3 // conversation overhead
}

/// Context compressor that summarizes old messages to stay within context window.
pub struct ContextCompressor {
    context_length: usize,
    threshold_percent: f64,
    protect_first_n: usize,
    protect_last_tokens: usize,
    compression_count: usize,
}

impl ContextCompressor {
    pub fn new(context_length: usize, threshold_percent: f64) -> Self {
        let protect_last_tokens = 20_000.min(context_length / 4);

        Self {
            context_length,
            threshold_percent,
            protect_first_n: 3,
            protect_last_tokens,
            compression_count: 0,
        }
    }

    /// Check if compression should be triggered based on current token usage.
    pub fn should_compress(&self, messages: &[ChatMessage]) -> bool {
        let estimated = estimate_messages_tokens(messages);
        let threshold = (self.context_length as f64 * self.threshold_percent) as usize;
        let should = estimated > threshold;
        if should {
            debug!(
                estimated_tokens = estimated,
                threshold = threshold,
                context_length = self.context_length,
                "Context compression triggered"
            );
        }
        should
    }

    /// Compress messages by pruning old tool results and summarizing middle messages.
    ///
    /// This is a local compression that doesn't require an LLM call.
    /// For LLM-based summarization, use `compress_with_summary`.
    pub fn compress(&mut self, messages: &mut Vec<ChatMessage>) -> bool {
        if !self.should_compress(messages) {
            return false;
        }

        let original_count = messages.len();
        let original_tokens = estimate_messages_tokens(messages);

        // Step 1: Prune old tool results (replace long results with truncated versions)
        self.prune_tool_results(messages);

        // Step 2: If still over threshold, drop middle messages
        if self.should_compress(messages) {
            self.drop_middle_messages(messages);
        }

        self.compression_count += 1;
        let new_tokens = estimate_messages_tokens(messages);

        info!(
            compression = self.compression_count,
            messages_before = original_count,
            messages_after = messages.len(),
            tokens_before = original_tokens,
            tokens_after = new_tokens,
            tokens_freed = original_tokens.saturating_sub(new_tokens),
            "Context compressed"
        );

        true
    }

    /// Prune old tool results to reduce token usage.
    fn prune_tool_results(&self, messages: &mut [ChatMessage]) {
        let total = messages.len();
        if total <= self.protect_first_n {
            return;
        }

        // Calculate how many tail messages to protect
        let mut tail_tokens = 0;
        let mut tail_start = total;
        for i in (0..total).rev() {
            let msg_tokens = estimate_message_tokens(&messages[i]);
            if tail_tokens + msg_tokens > self.protect_last_tokens {
                break;
            }
            tail_tokens += msg_tokens;
            tail_start = i;
        }

        // Prune tool results in the middle section
        for msg in messages[self.protect_first_n..tail_start].iter_mut() {
            if msg.tool_call_id.is_some()
                && let Some(ref content) = msg.content
            {
                let text = match content {
                    ironhermes_core::MessageContent::Text(t) => t.clone(),
                    ironhermes_core::MessageContent::Parts(_) => continue,
                };
                if text.len() > 500 {
                    msg.content = Some(ironhermes_core::MessageContent::Text(
                        format!("{}... [truncated, {} chars total]", &text[..200], text.len()),
                    ));
                }
            }
        }
    }

    /// Drop middle messages when tool result pruning isn't enough.
    fn drop_middle_messages(&self, messages: &mut Vec<ChatMessage>) {
        let total = messages.len();
        if total <= self.protect_first_n + 4 {
            return;
        }

        // Calculate tail protection boundary
        let mut tail_tokens = 0;
        let mut tail_start = total;
        for i in (0..total).rev() {
            let msg_tokens = estimate_message_tokens(&messages[i]);
            if tail_tokens + msg_tokens > self.protect_last_tokens {
                break;
            }
            tail_tokens += msg_tokens;
            tail_start = i;
        }

        let tail_start = tail_start.max(self.protect_first_n + 1);

        if tail_start <= self.protect_first_n + 1 {
            return;
        }

        // Count what we're dropping
        let dropped_count = tail_start - self.protect_first_n;

        // Create summary message
        let summary = format!(
            "[CONTEXT COMPACTED] {} earlier messages were removed to save context space. \
             The conversation continues from the most recent messages below.",
            dropped_count
        );

        // Replace middle with summary
        let mut new_messages = Vec::with_capacity(self.protect_first_n + 1 + (total - tail_start));
        new_messages.extend_from_slice(&messages[..self.protect_first_n]);
        new_messages.push(ChatMessage::system(summary));
        new_messages.extend_from_slice(&messages[tail_start..]);

        *messages = new_messages;
    }

    pub fn compression_count(&self) -> usize {
        self.compression_count
    }

    /// Phase 18 D-15: compute the index where the protected tail segment begins.
    /// Walks from end-of-vec accumulating message tokens until `protect_last_tokens`
    /// would be exceeded. Floored at `protect_first_n`. Exposed for `tool_pair`
    /// adaptive shift math; callers use it to decide whether a tool pair straddles
    /// the boundary.
    pub fn compute_protect_start(
        messages: &[ChatMessage],
        protect_last_tokens: usize,
        protect_first_n: usize,
    ) -> usize {
        let total = messages.len();
        if total <= protect_first_n {
            return total;
        }
        let mut tail_tokens = 0;
        let mut tail_start = total;
        for i in (0..total).rev() {
            let msg_tokens = estimate_message_tokens(&messages[i]);
            if tail_tokens + msg_tokens > protect_last_tokens {
                break;
            }
            tail_tokens += msg_tokens;
            tail_start = i;
        }
        tail_start.max(protect_first_n)
    }

    pub fn with_protect(mut self, first_n: usize, last_tokens: usize) -> Self {
        self.protect_first_n = first_n;
        self.protect_last_tokens = last_tokens;
        self
    }

    pub fn protect_first_n(&self) -> usize {
        self.protect_first_n
    }

    pub fn protect_last_tokens(&self) -> usize {
        self.protect_last_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::ChatMessage;

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello"), 2); // 5/4 + 1
        assert_eq!(estimate_tokens(""), 1); // 0/4 + 1
    }

    #[test]
    fn test_should_compress() {
        let compressor = ContextCompressor::new(1000, 0.5);
        let short_messages = vec![ChatMessage::user("hi")];
        assert!(!compressor.should_compress(&short_messages));
    }

    #[test]
    fn context_compressor_custom_protect() {
        let cc = ContextCompressor::new(10_000, 0.5).with_protect(5, 4_000);
        assert_eq!(cc.protect_first_n(), 5);
        assert_eq!(cc.protect_last_tokens(), 4_000);
    }
}
