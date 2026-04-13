// Phase 18 Plan 03: SummarizingEngine — agent-side ContextEngine (D-09).
//
// Uses an auxiliary LLM (Phase 12 build_role_client("compression")) to produce
// a structured running summary stored as a pinned `[CONTEXT HISTORY]` system
// message (D-17, D-19). Iterative re-compression updates the prior summary in
// place per D-18: NewSummary = Summarize(OldSummary + NewPrunedBlocks).
//
// On summarization failure the engine falls back to `LocalPruningEngine` so
// compression never blocks the agent loop (T-18-03).

use async_trait::async_trait;
use ironhermes_core::{ChatMessage, MessageContent, Role};
use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry};
use std::sync::Arc;

use crate::any_client::AnyClient;
use crate::context_compressor::{estimate_message_tokens, estimate_messages_tokens};
use crate::context_engine::{
    CompressionMode, CompressionOutcome, ContextEngine, ContextError, ContextStats,
    LocalPruningEngine,
};
use crate::pressure_warning::PressureTracker;
use crate::tool_pair;

/// Sentinel prefix for the pinned history segment (D-17).
pub const HISTORY_SENTINEL: &str = "[CONTEXT HISTORY]";
/// Stable `name` field value used to locate the pinned history segment (D-17).
pub const HISTORY_NAME: &str = "context_history";

/// Hard cap on summary content to bound prompt-injection surface (T-18-01).
const HISTORY_SUMMARY_MAX_CHARS: usize = 8_000;
/// Per-pruned-block character truncation before concatenation into prompt.
const PRUNED_BLOCK_MAX_CHARS: usize = 4_000;
/// D-24 runaway loop guard — refuse to compress after this many passes.
const MAX_COMPRESSION_PASSES: usize = 10;

/// Locate the pinned history segment index if present (D-17).
pub fn locate_history_segment(messages: &[ChatMessage]) -> Option<usize> {
    messages
        .iter()
        .position(|m| m.role == Role::System && m.name.as_deref() == Some(HISTORY_NAME))
}

fn make_history_message(summary_body: &str) -> ChatMessage {
    let truncated = if summary_body.len() > HISTORY_SUMMARY_MAX_CHARS {
        &summary_body[..HISTORY_SUMMARY_MAX_CHARS]
    } else {
        summary_body
    };
    ChatMessage {
        role: Role::System,
        content: Some(MessageContent::Text(format!(
            "{}\n{}",
            HISTORY_SENTINEL, truncated
        ))),
        tool_calls: None,
        tool_call_id: None,
        name: Some(HISTORY_NAME.into()),
    }
}

/// Abstracted summarization client so tests can mock without hitting the
/// network. The production impl (`AnyClientSummarizer`) delegates to
/// `AnyClient::chat_completion`.
#[async_trait]
pub trait SummarizationClient: Send + Sync + 'static {
    async fn summarize(&self, prompt: String) -> Result<String, ContextError>;
}

/// Production summarizer backed by `AnyClient` (Phase 12).
pub struct AnyClientSummarizer {
    client: Arc<AnyClient>,
    model: Option<String>,
}

impl AnyClientSummarizer {
    pub fn new(client: Arc<AnyClient>, model: Option<String>) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl SummarizationClient for AnyClientSummarizer {
    async fn summarize(&self, prompt: String) -> Result<String, ContextError> {
        let msg = ChatMessage::user(prompt);
        let resp = self
            .client
            .chat_completion(&[msg], None, self.model.as_deref(), None, None, None)
            .await
            .map_err(|e| ContextError::SummarizationFailed(e.to_string()))?;
        let body = resp
            .choices
            .first()
            .and_then(|c| c.message.content_text())
            .unwrap_or_default()
            .to_string();
        Ok(body)
    }
}

/// Agent-side ContextEngine using LLM summarization (D-09).
pub struct SummarizingEngine {
    context_length: usize,
    threshold: f32,
    protect_first_n: usize,
    protect_last_tokens: usize,
    tool_pair_shift_tokens: usize,
    summarizer: Arc<dyn SummarizationClient>,
    fallback: LocalPruningEngine,
    hook_registry: Option<Arc<HookRegistry>>,
    session_id: Option<String>,
    pressure_tracker: Option<Arc<PressureTracker>>,
}

impl SummarizingEngine {
    pub fn new(
        context_length: usize,
        threshold: f32,
        summarizer: Arc<dyn SummarizationClient>,
    ) -> Self {
        let protect_last_tokens = 20_000.min(context_length / 4);
        Self {
            context_length,
            threshold,
            protect_first_n: 3,
            protect_last_tokens,
            tool_pair_shift_tokens: 500,
            summarizer,
            fallback: LocalPruningEngine::new(context_length, threshold),
            hook_registry: None,
            session_id: None,
            pressure_tracker: None,
        }
    }

    /// Phase 18 D-20: attach a hook registry + session id so `compress` fires
    /// `context:pre_compress` and awaits handler completion before pruning.
    pub fn with_hooks(
        mut self,
        registry: Arc<HookRegistry>,
        session_id: impl Into<String>,
    ) -> Self {
        self.hook_registry = Some(registry);
        self.session_id = Some(session_id.into());
        self
    }

    /// Phase 18 D-23/D-24: attach a `PressureTracker` to enable three-channel
    /// pressure warnings at 85% of the compression threshold.
    pub fn with_pressure_tracker(mut self, tracker: Arc<PressureTracker>) -> Self {
        self.pressure_tracker = Some(tracker);
        self
    }

    pub fn with_protect(mut self, first_n: usize, last_tokens: usize) -> Self {
        self.protect_first_n = first_n;
        self.protect_last_tokens = last_tokens;
        self.fallback = LocalPruningEngine::new(self.context_length, self.threshold)
            .with_protect(first_n, last_tokens)
            .with_tool_pair_shift(self.tool_pair_shift_tokens);
        self
    }

    pub fn with_tool_pair_shift(mut self, n: usize) -> Self {
        self.tool_pair_shift_tokens = n;
        self.fallback = LocalPruningEngine::new(self.context_length, self.threshold)
            .with_protect(self.protect_first_n, self.protect_last_tokens)
            .with_tool_pair_shift(n);
        self
    }

    fn serialize_blocks(&self, messages: &[ChatMessage]) -> String {
        let mut out = String::new();
        for msg in messages {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let body = msg.content_text().unwrap_or("");
            let truncated = if body.len() > PRUNED_BLOCK_MAX_CHARS {
                &body[..PRUNED_BLOCK_MAX_CHARS]
            } else {
                body
            };
            out.push_str(role);
            out.push_str(": ");
            out.push_str(truncated);
            out.push('\n');
            if let Some(ref calls) = msg.tool_calls {
                for c in calls {
                    out.push_str(&format!(
                        "assistant[tool_call]: {}({})\n",
                        c.function.name, c.function.arguments
                    ));
                }
            }
        }
        out
    }
}

#[async_trait]
impl ContextEngine for SummarizingEngine {
    async fn compress(
        &self,
        messages: &mut Vec<ChatMessage>,
        stats: ContextStats,
    ) -> Result<CompressionOutcome, ContextError> {
        // D-24 runaway loop guard
        if stats.compression_count >= MAX_COMPRESSION_PASSES {
            tracing::warn!(
                compression_count = stats.compression_count,
                "SummarizingEngine refusing further compression (MAX_COMPRESSION_PASSES)"
            );
            return Ok(CompressionOutcome::default());
        }

        let before = estimate_messages_tokens(messages);
        let pct = before as f32 / self.context_length.max(1) as f32;

        tracing::info!(
            before_tokens = before,
            pct,
            threshold = self.threshold,
            session_id = ?self.session_id,
            "summarizing_engine: compress attempt"
        );

        // Phase 18 D-23/D-24: emit pressure warning at 85% of compression threshold.
        let mut pressure_warning_fired = false;
        if let (Some(tracker), Some(sid)) = (&self.pressure_tracker, &self.session_id) {
            let mode_str = match self.mode() {
                CompressionMode::Soft => "soft",
                CompressionMode::Hard => "hard",
            };
            pressure_warning_fired = tracker
                .check_and_maybe_emit(
                    sid,
                    self.threshold,
                    before,
                    self.context_length,
                    mode_str,
                    self.hook_registry.as_deref(),
                )
                .await;
        }

        if pct < self.threshold {
            tracing::info!(
                pct,
                threshold = self.threshold,
                reason = "below_threshold",
                "summarizing_engine: no-op"
            );
            return Ok(CompressionOutcome::default());
        }

        // Phase 18 D-20: fire context:pre_compress BEFORE destructive pruning.
        if let (Some(reg), Some(sid)) = (&self.hook_registry, &self.session_id) {
            let event = HookEvent::new(
                "req-compress",
                HookEventKind::ContextPreCompress {
                    session_id: sid.clone(),
                    estimated_tokens: before,
                    threshold: self.threshold,
                    mode: "soft".into(),
                    pruned_range: None,
                },
            );
            reg.fire_awaitable(event).await;
        } else {
            tracing::debug!(
                "no pre_compress handler registered, proceeding without memory flush"
            );
        }

        // Snapshot the caller's vec BEFORE any mutation so we can roll back
        // atomically if any later step (adaptive shift, prune, invariant check)
        // returns Err. Without this, a corrupted vec could be forwarded to the
        // LLM after `?` propagates an error.
        let snapshot = messages.clone();

        // Apply adaptive shift pre-prune (D-15) — same as LocalPruningEngine.
        let protect_start = crate::context_compressor::ContextCompressor::compute_protect_start(
            messages,
            self.protect_last_tokens,
            self.protect_first_n,
        );
        let pairs = tool_pair::detect_tool_pairs(messages);
        for pair in &pairs {
            let _ = tool_pair::apply_adaptive_shift(
                messages,
                pair,
                protect_start,
                self.tool_pair_shift_tokens,
            );
        }

        // Locate prior pinned history segment (if any) for iterative re-compression.
        let history_idx = locate_history_segment(messages);
        let prior_summary_text = history_idx.and_then(|i| {
            messages[i]
                .content_text()
                .and_then(|t| t.strip_prefix(HISTORY_SENTINEL))
                .map(|s| s.trim_start_matches('\n').to_string())
        });

        // Determine prune range [protect_first_n .. protect_start], excluding
        // the pinned history segment (D-19).
        if protect_start <= self.protect_first_n {
            tracing::info!(
                protect_start,
                protect_first_n = self.protect_first_n,
                protect_last_tokens = self.protect_last_tokens,
                reason = "nothing_to_prune_first_n",
                "summarizing_engine: no-op"
            );
            return Ok(CompressionOutcome::default());
        }
        let prune_start = self.protect_first_n;
        let prune_end = protect_start;

        let pruned_blocks: Vec<ChatMessage> = messages[prune_start..prune_end]
            .iter()
            .enumerate()
            .filter(|(offset, _)| {
                match history_idx {
                    Some(h) => h != prune_start + offset,
                    None => true,
                }
            })
            .map(|(_, m)| m.clone())
            .collect();

        if pruned_blocks.is_empty() {
            tracing::info!(
                prune_start,
                prune_end,
                reason = "prune_range_empty_after_history_filter",
                "summarizing_engine: no-op"
            );
            return Ok(CompressionOutcome::default());
        }

        let serialized_blocks = self.serialize_blocks(&pruned_blocks);

        // Build prompt (D-18 formula).
        let prompt = if let Some(prior) = prior_summary_text.as_deref() {
            format!(
                "Summarize the following conversation segment. Preserve user intents, decisions made, and key facts.\n\nPrior summary:\n{}\n\nNew pruned blocks:\n{}\n\nReturn a single concise paragraph.",
                prior, serialized_blocks
            )
        } else {
            format!(
                "Summarize the following conversation segment. Preserve user intents, decisions made, and key facts.\n\n{}\n\nReturn a single concise paragraph.",
                serialized_blocks
            )
        };

        let new_summary = match self.summarizer.summarize(prompt).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = ?e, "SummarizingEngine fell back to LocalPruningEngine");
                return self.fallback.compress(messages, stats).await;
            }
        };

        // Remove pruned blocks (skipping history segment if present).
        let mut kept_before: Vec<ChatMessage> = messages[..prune_start].to_vec();
        let mut kept_after: Vec<ChatMessage> = messages[prune_end..].to_vec();

        // If an old history segment lived inside the prune range we drop it;
        // if it lived outside, we strip it to avoid duplicate pins after insert.
        if let Some(h) = history_idx {
            if h < prune_start {
                kept_before.remove(h);
            } else if h >= prune_end {
                let rel = h - prune_end;
                kept_after.remove(rel);
            }
            // Range-internal history segment is simply omitted from pruned_blocks.
        }

        let mut new_messages = Vec::with_capacity(kept_before.len() + 1 + kept_after.len());
        new_messages.extend(kept_before);
        // Re-check pin index doesn't exceed bounds — clamp to len.
        let pin_idx = self.protect_first_n.min(new_messages.len());
        // If insertion point moved (history removed above), fix up by truncation.
        while new_messages.len() > pin_idx {
            // Move the tail into kept_after so we can insert cleanly.
            let m = new_messages.pop().unwrap();
            kept_after.insert(0, m);
        }
        new_messages.push(make_history_message(&new_summary));
        new_messages.extend(kept_after);

        *messages = new_messages;

        // D-16 invariant check. Roll back the caller's vec on failure so a
        // corrupted (orphaned tool_use) message list is never forwarded to the
        // LLM after `?` propagates the error.
        if let Err(e) = tool_pair::check_orphan_invariant(messages) {
            *messages = snapshot;
            tracing::warn!(
                error = ?e,
                reason = "rollback",
                "summarizing_engine: compress failed, messages restored"
            );
            return Err(e);
        }

        let after = estimate_messages_tokens(messages);
        tracing::info!(
            before_tokens = before,
            after_tokens = after,
            compression_count = stats.compression_count + 1,
            "summarizing_engine: compressed"
        );
        Ok(CompressionOutcome {
            compressed: true,
            tokens_freed: before.saturating_sub(after),
            new_summary: Some(new_summary),
            pressure_warning_fired,
        })
    }

    fn threshold(&self) -> f32 {
        self.threshold
    }

    fn mode(&self) -> CompressionMode {
        CompressionMode::Soft
    }

    async fn check_pressure(&self, stats: &ContextStats) -> bool {
        if let (Some(tracker), Some(sid)) = (&self.pressure_tracker, &self.session_id) {
            tracker
                .check_and_maybe_emit(
                    sid,
                    self.threshold,
                    stats.estimated_tokens,
                    self.context_length,
                    "soft",
                    self.hook_registry.as_deref(),
                )
                .await
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::sync::Mutex as AsyncMutex;

    struct MockSummarizer {
        calls: Arc<Mutex<Vec<String>>>,
        responses: Arc<AsyncMutex<Vec<Result<String, ContextError>>>>,
    }

    impl MockSummarizer {
        fn new(responses: Vec<Result<String, ContextError>>) -> (Arc<Self>, Arc<Mutex<Vec<String>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let m = Arc::new(Self {
                calls: calls.clone(),
                responses: Arc::new(AsyncMutex::new(responses)),
            });
            (m, calls)
        }
    }

    #[async_trait]
    impl SummarizationClient for MockSummarizer {
        async fn summarize(&self, prompt: String) -> Result<String, ContextError> {
            self.calls.lock().unwrap().push(prompt);
            let mut r = self.responses.lock().await;
            if r.is_empty() {
                return Err(ContextError::SummarizationFailed("no more mock responses".into()));
            }
            r.remove(0)
        }
    }

    fn build_large(n: usize) -> Vec<ChatMessage> {
        (0..n)
            .map(|i| ChatMessage::user(format!("message {i} ").repeat(20)))
            .collect()
    }

    fn make_stats() -> ContextStats {
        ContextStats {
            context_length: 1000,
            estimated_tokens: 0,
            protect_first_n: 3,
            protect_last_tokens: 250,
            compression_count: 0,
            prior_summary: None,
        }
    }

    #[tokio::test]
    async fn history_segment_pin() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary body".into())]);
        let engine = SummarizingEngine::new(1000, 0.5, mock);
        let mut msgs = build_large(30);
        let _ = engine
            .compress(&mut msgs, make_stats())
            .await
            .expect("compress ok");

        let hits: Vec<usize> = msgs
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.role == Role::System
                    && m.name.as_deref() == Some("context_history")
                    && m.content_text()
                        .map(|t| t.starts_with("[CONTEXT HISTORY]"))
                        .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect();

        assert_eq!(hits.len(), 1, "exactly one pinned history segment");
        assert_eq!(hits[0], 3, "pinned at protect_first_n index");
    }

    #[tokio::test]
    async fn summarizing_engine_aux_model() {
        let (mock, calls) = MockSummarizer::new(vec![Ok("Mock summary body".into())]);
        let engine = SummarizingEngine::new(1000, 0.5, mock);
        let mut msgs = build_large(30);
        let _ = engine
            .compress(&mut msgs, make_stats())
            .await
            .expect("compress ok");

        let recorded = calls.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1, "exactly one summarization call");
        assert!(
            recorded[0].contains("Summarize the following conversation"),
            "prompt contains canonical instruction: {}",
            recorded[0]
        );

        let pin = msgs
            .iter()
            .find(|m| m.name.as_deref() == Some("context_history"))
            .expect("pin exists");
        assert_eq!(
            pin.content_text().unwrap(),
            "[CONTEXT HISTORY]\nMock summary body"
        );
    }

    #[tokio::test]
    async fn iterative_summary() {
        let (mock, calls) = MockSummarizer::new(vec![
            Ok("Summary1".into()),
            Ok("Summary2".into()),
        ]);
        let engine = SummarizingEngine::new(1000, 0.5, mock);
        let mut msgs = build_large(30);

        let _ = engine
            .compress(&mut msgs, make_stats())
            .await
            .expect("first compress");

        let pin1 = msgs
            .iter()
            .find(|m| m.name.as_deref() == Some("context_history"))
            .expect("pin1");
        assert_eq!(pin1.content_text().unwrap(), "[CONTEXT HISTORY]\nSummary1");

        // Grow the list again to trigger a second compression pass.
        msgs.extend(build_large(30));

        let _ = engine
            .compress(&mut msgs, make_stats())
            .await
            .expect("second compress");

        // Exactly one pinned segment (replaced, not duplicated).
        let pins: Vec<_> = msgs
            .iter()
            .filter(|m| m.name.as_deref() == Some("context_history"))
            .collect();
        assert_eq!(pins.len(), 1, "single pin after iterative compression");
        assert_eq!(pins[0].content_text().unwrap(), "[CONTEXT HISTORY]\nSummary2");

        let recorded = calls.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2, "two summarization calls");
        assert!(recorded[1].contains("Summary1"), "second prompt includes prior summary");
        assert!(
            recorded[1].contains("New pruned blocks"),
            "second prompt uses iterative template"
        );
    }

    #[tokio::test]
    async fn summarizing_engine_fallback_on_failure() {
        let (mock, _) = MockSummarizer::new(vec![Err(ContextError::SummarizationFailed(
            "network".into(),
        ))]);
        let engine = SummarizingEngine::new(1000, 0.5, mock);
        let mut msgs = build_large(30);
        let outcome = engine
            .compress(&mut msgs, make_stats())
            .await
            .expect("fallback returns Ok");
        assert!(outcome.compressed, "LocalPruningEngine fallback compressed");
        assert!(tool_pair::check_orphan_invariant(&msgs).is_ok());
    }

    /// Phase 18 atomic-rollback fix: when `check_orphan_invariant` rejects the
    /// post-compression vec, the caller's `messages` MUST be restored to its
    /// pre-call snapshot. Constructed input contains an orphan tool_call that
    /// the invariant will reject after the prune+pin step.
    #[tokio::test]
    async fn compress_rolls_back_on_orphan_invariant_failure() {
        use ironhermes_core::{FunctionCall, ToolCall};
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary body".into())]);
        let engine = SummarizingEngine::new(1000, 0.5, mock);

        // Filler to push us above threshold.
        let mut msgs: Vec<ChatMessage> = (0..28)
            .map(|i| ChatMessage::user(format!("filler {i} ").repeat(20)))
            .collect();
        // Append orphan assistant tool_call (no matching tool_result anywhere).
        msgs.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "orphan-id".into(),
            call_type: "function".into(),
            function: FunctionCall { name: "fn".into(), arguments: "{}".into() },
        }]));
        let snapshot = msgs.clone();

        let err = engine
            .compress(&mut msgs, make_stats())
            .await
            .expect_err("orphan must surface as Err");
        assert!(matches!(err, ContextError::OrphanedToolPair));

        assert_eq!(
            msgs.len(),
            snapshot.len(),
            "rollback restored original length"
        );
        for (a, b) in msgs.iter().zip(snapshot.iter()) {
            assert_eq!(a.content_text(), b.content_text());
            assert_eq!(
                a.tool_calls.as_ref().map(|v| v.len()),
                b.tool_calls.as_ref().map(|v| v.len())
            );
        }
        // Must NOT contain a [CONTEXT HISTORY] pin — that would mean the
        // mutated state leaked through.
        assert!(
            msgs.iter()
                .all(|m| m.name.as_deref() != Some(HISTORY_NAME)),
            "no pinned history segment in restored vec"
        );
    }

    #[test]
    fn summarizing_engine_is_soft_mode() {
        let (mock, _) = MockSummarizer::new(vec![]);
        let engine = SummarizingEngine::new(1000, 0.5, mock);
        assert_eq!(engine.mode(), CompressionMode::Soft);
        assert!((engine.threshold() - 0.5).abs() < f32::EPSILON);
    }
}
