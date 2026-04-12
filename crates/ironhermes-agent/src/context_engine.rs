use async_trait::async_trait;
use ironhermes_core::ChatMessage;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMode {
    Soft,
    Hard,
}

#[derive(Debug, Clone)]
pub struct ContextStats {
    pub context_length: usize,
    pub estimated_tokens: usize,
    pub protect_first_n: usize,
    pub protect_last_tokens: usize,
    pub compression_count: usize,
    pub prior_summary: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CompressionOutcome {
    pub compressed: bool,
    pub tokens_freed: usize,
    pub new_summary: Option<String>,
    pub pressure_warning_fired: bool,
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("orphaned tool pair detected after compression")]
    OrphanedToolPair,
    #[error("memory flush failed: {0}")]
    FlushFailed(String),
    #[error("summarization llm call failed: {0}")]
    SummarizationFailed(String),
    #[error("internal error: {0}")]
    Internal(String),
}

#[async_trait]
pub trait ContextEngine: Send + Sync + 'static {
    async fn compress(
        &self,
        messages: &mut Vec<ChatMessage>,
        stats: ContextStats,
    ) -> Result<CompressionOutcome, ContextError>;
    fn threshold(&self) -> f32;
    fn mode(&self) -> CompressionMode;
}

pub struct LocalPruningEngine {
    context_length: usize,
    threshold: f32,
    protect_first_n: usize,
    protect_last_tokens: usize,
}

impl LocalPruningEngine {
    pub fn new(context_length: usize, threshold: f32) -> Self {
        let protect_last_tokens = 20_000.min(context_length / 4);
        Self {
            context_length,
            threshold,
            protect_first_n: 3,
            protect_last_tokens,
        }
    }

    pub fn with_protect(mut self, first_n: usize, last_tokens: usize) -> Self {
        self.protect_first_n = first_n;
        self.protect_last_tokens = last_tokens;
        self
    }
}

#[async_trait]
impl ContextEngine for LocalPruningEngine {
    async fn compress(
        &self,
        messages: &mut Vec<ChatMessage>,
        _stats: ContextStats,
    ) -> Result<CompressionOutcome, ContextError> {
        let before = crate::context_compressor::estimate_messages_tokens(messages);
        let mut cc = crate::context_compressor::ContextCompressor::new(
            self.context_length,
            self.threshold as f64,
        )
        .with_protect(self.protect_first_n, self.protect_last_tokens);
        let compressed = cc.compress(messages);
        let after = crate::context_compressor::estimate_messages_tokens(messages);
        Ok(CompressionOutcome {
            compressed,
            tokens_freed: before.saturating_sub(after),
            new_summary: None,
            pressure_warning_fired: false,
        })
    }

    fn threshold(&self) -> f32 {
        self.threshold
    }

    fn mode(&self) -> CompressionMode {
        CompressionMode::Hard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_bounds<T: ContextEngine>() {}

    #[test]
    fn context_engine_trait_bounds() {
        assert_bounds::<LocalPruningEngine>();
    }

    fn make_stats(n: usize) -> ContextStats {
        ContextStats {
            context_length: 1000,
            estimated_tokens: n,
            protect_first_n: 3,
            protect_last_tokens: 250,
            compression_count: 0,
            prior_summary: None,
        }
    }

    fn build_large_message_vec(n: usize) -> Vec<ChatMessage> {
        // Each message ~50 tokens; 30 msgs → ~1500 tokens, well over 500 threshold.
        (0..n)
            .map(|i| ChatMessage::user(format!("message {i} ").repeat(20)))
            .collect()
    }

    #[tokio::test]
    async fn local_pruning_engine_parity() {
        let mut via_engine = build_large_message_vec(30);
        let mut via_compressor = via_engine.clone();

        let engine = LocalPruningEngine::new(1000, 0.5);
        let _ = engine
            .compress(&mut via_engine, make_stats(0))
            .await
            .expect("engine compress ok");

        let mut cc = crate::context_compressor::ContextCompressor::new(1000, 0.5);
        let _ = cc.compress(&mut via_compressor);

        assert_eq!(via_engine.len(), via_compressor.len());
        for (a, b) in via_engine.iter().zip(via_compressor.iter()) {
            assert_eq!(a.content_text(), b.content_text());
        }
    }

    #[tokio::test]
    async fn test_protect_boundaries() {
        let mut messages = build_large_message_vec(30);
        let engine = LocalPruningEngine::new(1000, 0.5);
        let _ = engine
            .compress(&mut messages, make_stats(0))
            .await
            .expect("ok");
        // First 3 should still be the original user messages.
        for i in 0..3 {
            let text = messages[i].content_text().unwrap_or("");
            assert!(
                text.starts_with(&format!("message {i} ")),
                "first {i} preserved, got: {text}"
            );
        }
    }

    #[test]
    fn compression_mode_is_hard() {
        let engine = LocalPruningEngine::new(1000, 0.5);
        assert_eq!(engine.mode(), CompressionMode::Hard);
        assert!((engine.threshold() - 0.5).abs() < f32::EPSILON);
    }
}
