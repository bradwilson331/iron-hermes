use ironhermes_core::{ChatMessage, global_estimate_tokens};
use tracing::{debug, info};

/// Token estimation backed by tiktoken BPE (Phase 21.3 D-09).
/// Falls back to text.len()/4+1 if global estimator not yet initialized.
pub fn estimate_tokens(text: &str) -> usize {
    global_estimate_tokens(text)
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
///
/// Phase 34b Plan 02: the per-session counters (`compression_count` and the
/// `last_*_tokens` parity fields ported from `context_compressor.py`) are stored
/// as `AtomicUsize` so the `ContextEngine::on_session_reset(&self)` override can
/// zero them through a shared `&self` reference (interior mutability) without
/// requiring `&mut self`.
pub struct ContextCompressor {
    context_length: usize,
    threshold_percent: f64,
    protect_first_n: usize,
    protect_last_tokens: usize,
    compression_count: std::sync::atomic::AtomicUsize,
    /// Python parity (`context_compressor.py` `_ineffective_compression_count`):
    /// counts compression passes that freed no tokens. Zeroed on session reset.
    ineffective_compression_count: std::sync::atomic::AtomicUsize,
    /// Python parity: last observed prompt/completion/total token usage. Zeroed
    /// on session reset so a fresh conversation does not inherit stale metrics.
    last_prompt_tokens: std::sync::atomic::AtomicUsize,
    last_completion_tokens: std::sync::atomic::AtomicUsize,
    last_total_tokens: std::sync::atomic::AtomicUsize,
}

impl ContextCompressor {
    pub fn new(context_length: usize, threshold_percent: f64) -> Self {
        use std::sync::atomic::AtomicUsize;
        let protect_last_tokens = 20_000.min(context_length / 4);

        Self {
            context_length,
            threshold_percent,
            protect_first_n: 3,
            protect_last_tokens,
            compression_count: AtomicUsize::new(0),
            ineffective_compression_count: AtomicUsize::new(0),
            last_prompt_tokens: AtomicUsize::new(0),
            last_completion_tokens: AtomicUsize::new(0),
            last_total_tokens: AtomicUsize::new(0),
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
    pub fn compress(&self, messages: &mut Vec<ChatMessage>) -> bool {
        // Step 0 (Phase 34a D-03): strip ephemeral recall messages before any
        // token estimation — they are re-derivable next turn and must be freed
        // first when context is tight.
        messages.retain(|m| !m.is_recall_context);

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

        use std::sync::atomic::Ordering;
        let count = self.compression_count.fetch_add(1, Ordering::SeqCst) + 1;
        let new_tokens = estimate_messages_tokens(messages);

        // Python parity: track passes that freed no tokens.
        if new_tokens >= original_tokens {
            self.ineffective_compression_count
                .fetch_add(1, Ordering::SeqCst);
        }

        info!(
            compression = count,
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
                    msg.content = Some(ironhermes_core::MessageContent::Text(format!(
                        "{}... [truncated, {} chars total]",
                        ironhermes_core::truncate_on_char_boundary(&text, 200),
                        text.len()
                    )));
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
        self.compression_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Phase 34b Plan 02: last observed prompt-token count (Python parity).
    pub fn last_prompt_tokens(&self) -> usize {
        self.last_prompt_tokens.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Phase 34b Plan 02: last observed completion-token count (Python parity).
    pub fn last_completion_tokens(&self) -> usize {
        self.last_completion_tokens.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Phase 34b Plan 02: last observed total-token count (Python parity).
    pub fn last_total_tokens(&self) -> usize {
        self.last_total_tokens.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Phase 34b Plan 02: record the per-response token usage (Python parity for
    /// `update_from_response`). Stored so a long-lived compressor instance can
    /// report last-turn usage; zeroed by `on_session_reset`.
    pub fn record_usage(&self, prompt_tokens: usize, completion_tokens: usize, total_tokens: usize) {
        use std::sync::atomic::Ordering;
        self.last_prompt_tokens.store(prompt_tokens, Ordering::SeqCst);
        self.last_completion_tokens
            .store(completion_tokens, Ordering::SeqCst);
        self.last_total_tokens.store(total_tokens, Ordering::SeqCst);
    }

    /// Phase 34b Plan 02: test-only helper to drive a compression pass through a
    /// shared `&self` reference (the production caller in `agent_loop.rs` holds a
    /// `Mutex` guard). Lets the reset unit test bump `compression_count` without
    /// `&mut`.
    #[cfg(test)]
    pub fn compress_for_test(&self, messages: &mut Vec<ChatMessage>) -> bool {
        self.compress(messages)
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

/// Phase 34b Plan 02 (D-06/D-10): `ContextCompressor` implements `ContextEngine`
/// so its `on_session_reset` override can zero the durable per-session counters
/// (`compression_count` + `last_*_tokens`) — Python parity with
/// `context_compressor.py::on_session_reset`. The compressor zeroes its OWN
/// fields directly; `PressureTracker` has no reset method to delegate to.
///
/// The `compress` trait method delegates to the inherent local prune+drop logic
/// (ignoring `stats`, which only the LLM-summarizing engine consumes).
#[async_trait::async_trait]
impl crate::context_engine::ContextEngine for ContextCompressor {
    async fn compress(
        &self,
        messages: &mut Vec<ChatMessage>,
        _stats: crate::context_engine::ContextStats,
    ) -> Result<crate::context_engine::CompressionOutcome, crate::context_engine::ContextError> {
        let before = estimate_messages_tokens(messages);
        let compressed = ContextCompressor::compress(self, messages);
        let after = estimate_messages_tokens(messages);
        Ok(crate::context_engine::CompressionOutcome {
            compressed,
            tokens_freed: before.saturating_sub(after),
            new_summary: None,
            pressure_warning_fired: false,
        })
    }

    fn threshold(&self) -> f32 {
        self.threshold_percent as f32
    }

    fn mode(&self) -> crate::context_engine::CompressionMode {
        crate::context_engine::CompressionMode::Hard
    }

    /// Zero ALL per-session counters (Python `on_session_reset` parity).
    fn on_session_reset(&self) {
        use std::sync::atomic::Ordering;
        self.compression_count.store(0, Ordering::SeqCst);
        self.ineffective_compression_count.store(0, Ordering::SeqCst);
        self.last_prompt_tokens.store(0, Ordering::SeqCst);
        self.last_completion_tokens.store(0, Ordering::SeqCst);
        self.last_total_tokens.store(0, Ordering::SeqCst);
    }

    /// Record per-response usage so a long-lived compressor reports last-turn
    /// metrics (Python `update_from_response` parity).
    fn update_from_response(&self, usage: &crate::agent_loop::AggregatedUsage) {
        self.record_usage(
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::ChatMessage;

    #[test]
    fn test_estimate_tokens() {
        // tiktoken BPE count for "hello" is 1 token (single BPE token).
        // If global estimator not initialized, falls back to heuristic: 5/4+1=2.
        let count = estimate_tokens("hello");
        assert!(
            count > 0,
            "estimate_tokens must return nonzero for non-empty text"
        );
        // Empty string: tiktoken returns 0, heuristic returns 1. Both are valid.
        let empty_count = estimate_tokens("");
        assert!(
            empty_count <= 1,
            "estimate_tokens for empty string must be 0 or 1"
        );
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

    #[test]
    fn compress_step0_evicts_recall_messages() {
        // Phase 34a D-03: recall messages must be stripped as step 0, even when
        // the context is below the compression threshold (no actual compression).
        let mut compressor = ContextCompressor::new(100_000, 0.9);
        let recall_msg = ChatMessage::recall_system("Recall: user prefers dark mode.");
        let normal_system = ChatMessage::system("You are Hermes.");
        let user_msg = ChatMessage::user("Hello");
        let mut messages = vec![normal_system, recall_msg, user_msg];

        // Step 0 runs even when should_compress returns false.
        compressor.compress(&mut messages);

        // No message with is_recall_context == true should remain.
        assert!(
            !messages.iter().any(|m| m.is_recall_context),
            "compressor step 0 must evict all recall messages"
        );
        // Normal messages survive.
        assert_eq!(messages.len(), 2, "normal system + user message should remain");
    }

    #[test]
    fn test_context_compressor_reset_zeroes_counter() {
        // Wave 2 (Plan 02 Task 1): build a ContextCompressor, drive
        // compression_count up, call on_session_reset(), assert all token counters +
        // compression_count are zero.
        use crate::context_engine::ContextEngine;

        // Small context window + low protect tail so the message vec exceeds
        // the threshold and a real compression pass runs (mirrors the
        // local_pruning_engine_parity fixture).
        let cc = ContextCompressor::new(1000, 0.5).with_protect(3, 250);

        // Drive compression_count > 0 by compressing a large message vec.
        let mut messages: Vec<ChatMessage> = (0..30)
            .map(|i| ChatMessage::user(format!("message {i} ").repeat(20)))
            .collect();
        let did_compress = cc.compress_for_test(&mut messages);
        assert!(did_compress, "test fixture must trigger a compression pass");

        // Also record some usage so the token counters are non-zero pre-reset.
        cc.record_usage(123, 45, 168);
        assert_eq!(cc.last_total_tokens(), 168, "usage recorded pre-reset");

        // Compression count should be non-zero after compression.
        assert!(
            cc.compression_count() > 0,
            "compression_count must be > 0 after compress"
        );

        // Call on_session_reset() — zeroes all counters.
        cc.on_session_reset();

        assert_eq!(
            cc.compression_count(),
            0,
            "compression_count must be 0 after on_session_reset"
        );
        assert_eq!(
            cc.last_prompt_tokens(),
            0,
            "last_prompt_tokens must be 0 after on_session_reset"
        );
        assert_eq!(
            cc.last_completion_tokens(),
            0,
            "last_completion_tokens must be 0 after on_session_reset"
        );
        assert_eq!(
            cc.last_total_tokens(),
            0,
            "last_total_tokens must be 0 after on_session_reset"
        );
    }

    #[test]
    fn test_has_content_to_compress_default_true() {
        use crate::context_engine::ContextEngine;
        use ironhermes_core::ChatMessage;

        let cc = ContextCompressor::new(100_000, 0.5);
        let msgs = vec![ChatMessage::user("hello")];
        // Default has_content_to_compress returns true.
        assert!(cc.has_content_to_compress(&msgs));
    }
}
