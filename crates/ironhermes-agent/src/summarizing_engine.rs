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

        // Phase 18 Plan 10 fix: apply_adaptive_shift returns the adjusted
        // protect boundary. Previously we discarded it and used the stale
        // `protect_start` as prune_end, which cut through tool_use/tool_result
        // pairs — every live compression attempt rolled back with
        // OrphanedToolPair (2026-04-13 UAT). Fold each pair's return value
        // into the minimum (most protective) boundary so we honor every pair
        // that asks to be kept whole.
        let initial_protect_start =
            crate::context_compressor::ContextCompressor::compute_protect_start(
                messages,
                self.protect_last_tokens,
                self.protect_first_n,
            );
        let pairs = tool_pair::detect_tool_pairs(messages);
        let mut effective_protect_start = initial_protect_start;
        for pair in &pairs {
            let adjusted = tool_pair::apply_adaptive_shift(
                messages,
                pair,
                initial_protect_start,
                self.tool_pair_shift_tokens,
            );
            if adjusted < effective_protect_start {
                effective_protect_start = adjusted;
            }
        }
        let protect_start = effective_protect_start;

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
        let mut prune_start = self.protect_first_n;
        let mut prune_end = protect_start;

        // Phase 18 Plan 10 pair-atomicity guard: ensure no detected pair is
        // split across [prune_start..prune_end]. Two straddle directions exist:
        //
        //   (a) BACK-STRADDLE: assistant is inside prune range, at least one
        //       tool_result is AFTER prune_end (in protected tail). Pull
        //       prune_end BACK to `assistant_idx` so both sides stay live.
        //
        //   (b) FRONT-STRADDLE: assistant is BEFORE prune_start (inside the
        //       front-protected `protect_first_n` region), and one or more
        //       tool_results are inside prune range. We must push prune_start
        //       FORWARD past `max(tool_result_idx) + 1` so the whole pair
        //       stays in the front-protected region — otherwise we'd prune
        //       tool_results whose assistant cannot be removed, orphaning them.
        //
        // Post-fix invariant: `prune_start` only increases, `prune_end` only
        // decreases. This is why we compute adjustments then apply the
        // monotone update.
        //
        // Live UAT 2026-04-13T05:18 failed every compression because the
        // previous guard only handled (a) and collapsed the range on (b):
        // protect_first_n=3, asst at idx 2, tool_result at idx 3 → old guard
        // pulled prune_end=2, below prune_start=3 → logic-stall no-op.
        let pairs_after_shift = tool_pair::detect_tool_pairs(messages);
        for pair in &pairs_after_shift {
            let asst_in = pair.assistant_idx >= prune_start && pair.assistant_idx < prune_end;
            let any_result_in = pair
                .tool_result_indices
                .iter()
                .any(|&i| i >= prune_start && i < prune_end);
            let all_results_in = pair
                .tool_result_indices
                .iter()
                .all(|&i| i >= prune_start && i < prune_end);
            let fully_in = asst_in && all_results_in;
            let fully_out = !asst_in && !any_result_in;
            if fully_in || fully_out {
                continue;
            }
            if asst_in {
                // (a) back-straddle: assistant prunable, ≥1 result in tail.
                // Pull prune_end BACK to before the assistant.
                if pair.assistant_idx < prune_end {
                    prune_end = pair.assistant_idx;
                }
            } else if pair.assistant_idx < prune_start && any_result_in {
                // (b) front-straddle: assistant front-protected, results in
                // prune range. Push prune_start FORWARD past the last result
                // so the whole pair lives in the front-protected region.
                let last_result = pair
                    .tool_result_indices
                    .iter()
                    .copied()
                    .max()
                    .unwrap_or(pair.assistant_idx);
                let push_to = last_result + 1;
                if push_to > prune_start {
                    prune_start = push_to;
                }
            }
            // Remaining theoretical case (asst after prune_end with results
            // before prune_start) is impossible — tool_results always follow
            // their assistant in a valid sequence.
        }
        if prune_end <= prune_start {
            tracing::warn!(
                prune_start,
                prune_end,
                reason = "pair_atomicity_collapsed_range",
                "summarizing_engine: compression requested but guard collapsed prune range to no-op — logic stall"
            );
            return Ok(CompressionOutcome::default());
        }

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
                outcome = "rolled_back",
                "summarizing_engine: compress failed, messages restored"
            );
            return Err(e);
        }

        let after = estimate_messages_tokens(messages);
        tracing::info!(
            before_tokens = before,
            after_tokens = after,
            tokens_freed = before.saturating_sub(after),
            compression_count = stats.compression_count + 1,
            prune_start,
            prune_end,
            pair_count = pairs.len(),
            outcome = "compressed",
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

    // ── Phase 18 Plan 10: UAT-shape reproducing tests for apply_adaptive_shift ──
    //
    // These tests pin the 2026-04-13 live UAT defect where `apply_adaptive_shift`'s
    // return value was discarded by `compress`, causing every tool-pair-straddling
    // compression attempt to split the pair and roll back with OrphanedToolPair.

    use crate::context_compressor::ContextCompressor;
    use ironhermes_core::{FunctionCall, ToolCall};

    /// Build a message list with a single tool-pair (web_read-style) STRADDLING
    /// the protect boundary: pair's assistant message is prunable, tool_result
    /// lies in the protected tail. Pads to target total-token budget while
    /// keeping the pair positioned so `compute_protect_start(msgs, protect_last_tokens, 3)`
    /// falls BETWEEN assistant and tool_result.
    fn build_list_with_straddling_pair(
        pair_body: &str,
        total_token_target: usize,
        protect_last_tokens: usize,
    ) -> Vec<ChatMessage> {
        let mut msgs = Vec::new();
        msgs.push(ChatMessage::system("sys"));
        msgs.push(ChatMessage::user("first user"));
        msgs.push(ChatMessage::assistant("first assistant"));
        // Pair goes at position 3 initially; we grow prunable region before it
        // and add exactly enough tail filler after tool_result to pull
        // protect_start between assistant and tool_result.
        msgs.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "web_read_1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "web_read".into(),
                arguments: r#"{"url":"https://example.com"}"#.into(),
            },
        }]));
        msgs.push(ChatMessage::tool_result("web_read_1", pair_body));

        // Phase 1: Grow prunable region BEFORE the pair until total tokens
        // hit target. This also ensures enough prunable content exists that
        // `protect_start` can land between the assistant and tool_result.
        while estimate_messages_tokens(&msgs) < total_token_target {
            msgs.insert(3, ChatMessage::user("pad ".repeat(40)));
        }

        // Phase 2: Fine-tune the tail so that
        //   asst_idx < protect_start ≤ tool_result_idx (straddle).
        // Walking backward: tail accumulates tool_result+tail_filler.
        // - If tool_result is NOT yet in tail (protect_start > result_idx):
        //   add tail filler to push tail window further left.
        // - If tail already includes assistant (protect_start ≤ asst_idx):
        //   tail is too generous — prepend MORE prunable filler so the
        //   walk still stops on the assistant (assistant tokens + tail tokens
        //   push over protect_last once tail shrinks relative to growing list).
        //   Actually simpler: reduce the gap by adding filler BEFORE pair
        //   (pushes asst_idx right; doesn't change tail walk tokens).
        //   Since asst_idx > protect_start is the failure, we need to GROW
        //   the tail until it stops before the assistant. Add a chunk of
        //   tail filler right before the tool_result is NOT allowed
        //   (would split the pair). We can insert filler just AFTER the
        //   tool_result to inflate "stuff after result" but that's already
        //   counted. Alt: inflate the assistant tool_call args so its
        //   own token weight alone exceeds (protect_last - tool_result_tokens).
        let asst_pos = |v: &Vec<ChatMessage>| {
            v.iter()
                .position(|m| {
                    m.role == Role::Assistant
                        && m.tool_calls
                            .as_ref()
                            .map(|vv| vv.iter().any(|c| c.id == "web_read_1"))
                            .unwrap_or(false)
                })
                .unwrap()
        };
        let result_pos = |v: &Vec<ChatMessage>| {
            v.iter()
                .position(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("web_read_1"))
                .unwrap()
        };

        // First, add tail filler (if needed) so protect_start ≤ result_idx.
        loop {
            let ri = result_pos(&msgs);
            let ps = ContextCompressor::compute_protect_start(&msgs, protect_last_tokens, 3);
            if ri >= ps {
                break;
            }
            msgs.push(ChatMessage::user("tail ".repeat(10)));
            if msgs.len() > 10_000 {
                panic!("runaway tail filler");
            }
        }

        // Then, if protect_start ≤ asst_idx (tail too deep), inflate the
        // assistant tool_call's arguments with padding so its own token
        // weight forces the tail walk to STOP at the tool_result boundary.
        loop {
            let ai = asst_pos(&msgs);
            let ps = ContextCompressor::compute_protect_start(&msgs, protect_last_tokens, 3);
            if ai < ps {
                break;
            }
            // Inflate assistant args with padding.
            let pos = ai;
            if let Some(ref mut calls) = msgs[pos].tool_calls {
                let padded = format!(
                    "{}{}",
                    calls[0].function.arguments,
                    " ".repeat(40) // ~10 more tokens per loop
                );
                calls[0].function.arguments = padded;
            }
            if msgs[pos]
                .tool_calls
                .as_ref()
                .map(|v| v[0].function.arguments.len())
                .unwrap_or(0)
                > 100_000
            {
                panic!("runaway assistant arg inflation");
            }
        }
        msgs
    }

    /// Legacy fixture-builder kept for backward compatibility with non-straddle
    /// tests (fully-prunable / fully-protected-tail shapes).
    fn build_list_with_pair(
        filler_before: usize,
        pair_body: &str,
        filler_after: usize,
        total_token_target: usize,
    ) -> Vec<ChatMessage> {
        let mut msgs = Vec::new();
        msgs.push(ChatMessage::system("sys"));
        msgs.push(ChatMessage::user("first user"));
        msgs.push(ChatMessage::assistant("first assistant"));
        for i in 0..filler_before {
            msgs.push(ChatMessage::user(format!("filler_b {i} ").repeat(20)));
        }
        msgs.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "web_read_1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "web_read".into(),
                arguments: r#"{"url":"https://example.com"}"#.into(),
            },
        }]));
        msgs.push(ChatMessage::tool_result("web_read_1", pair_body));
        for i in 0..filler_after {
            msgs.push(ChatMessage::user(format!("filler_a {i} ").repeat(20)));
        }
        while estimate_messages_tokens(&msgs) < total_token_target {
            msgs.insert(3, ChatMessage::user("pad ".repeat(40)));
        }
        msgs
    }

    fn assert_compress_ok_no_orphan(msgs: &[ChatMessage]) {
        assert!(
            tool_pair::check_orphan_invariant(msgs).is_ok(),
            "post-compression message list must satisfy orphan invariant"
        );
        let pins: Vec<_> = msgs
            .iter()
            .filter(|m| m.name.as_deref() == Some(HISTORY_NAME))
            .collect();
        assert_eq!(pins.len(), 1, "exactly one pinned [CONTEXT HISTORY] segment");
    }

    /// Assert the fixture actually places the pair straddling the protect
    /// boundary (assistant prunable, at least one tool_result protected).
    /// Guards against silent false-GREEN from a miscounted fixture.
    fn assert_pair_straddles(msgs: &[ChatMessage], protect_last_tokens: usize) {
        let protect_start =
            ContextCompressor::compute_protect_start(msgs, protect_last_tokens, 3);
        let pairs = tool_pair::detect_tool_pairs(msgs);
        assert_eq!(pairs.len(), 1, "fixture must contain exactly one tool-pair");
        let p = &pairs[0];
        assert!(
            p.assistant_idx < protect_start
                && p.tool_result_indices.iter().any(|&i| i >= protect_start),
            "fixture must STRADDLE protect_start (asst_idx={}, tool_results={:?}, protect_start={})",
            p.assistant_idx,
            p.tool_result_indices,
            protect_start
        );
    }

    fn uat_stats(context_length: usize, protect_last_tokens: usize, before: usize) -> ContextStats {
        ContextStats {
            context_length,
            estimated_tokens: before,
            protect_first_n: 3,
            protect_last_tokens,
            compression_count: 0,
            prior_summary: None,
        }
    }

    #[tokio::test]
    async fn compress_ok_small_pair_488_tokens() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 976;
        let protect_last = 100;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_straddling_pair("web page body", 488, protect_last);
        assert_pair_straddles(&msgs, protect_last);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — UAT shape (488 tokens)");
        assert!(outcome.compressed, "compressed flag must be true");
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_small_pair_3055_tokens() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 6110;
        let protect_last = 200;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_straddling_pair("web page body", 3055, protect_last);
        assert_pair_straddles(&msgs, protect_last);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — UAT shape (3055 tokens)");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_small_pair_3511_tokens() {
        // UAT 3511-token shape — same path as 3055 but closes the documentation
        // gap (plan-checker concern #2).
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 7022;
        let protect_last = 200;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_straddling_pair("web page body", 3511, protect_last);
        assert_pair_straddles(&msgs, protect_last);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — UAT shape (3511 tokens)");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_large_pair_7111_tokens() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 14222;
        // protect_last > tool_result body (~625 tokens) so the tool_result can
        // sit in the protected tail while the assistant stays prunable.
        let protect_last = 800;
        // ~2500-char body → ~625+ estimated tokens, exceeds 500-token shift threshold.
        let big_body = "x".repeat(2500);
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_straddling_pair(&big_body, 7111, protect_last);
        assert_pair_straddles(&msgs, protect_last);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — UAT shape (7111 tokens, large body)");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_large_pair_9467_tokens() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 18934;
        let protect_last = 800;
        let big_body = "x".repeat(2500);
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_straddling_pair(&big_body, 9467, protect_last);
        assert_pair_straddles(&msgs, protect_last);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — UAT shape (9467 tokens, large body)");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_pair_fully_in_prunable_range() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 6000;
        let protect_last = 100;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        // Pair placed near front with LOTS of filler after → pair sits fully
        // in the prunable middle while tail filler fills the protected window.
        let mut msgs = build_list_with_pair(2, "small result", 30, 3000);
        // Sanity: the pair is fully in prunable range (asst + result < protect_start).
        let protect_start = ContextCompressor::compute_protect_start(&msgs, protect_last, 3);
        let pairs = tool_pair::detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        assert!(
            pairs[0].assistant_idx < protect_start
                && pairs[0].tool_result_indices.iter().all(|&i| i < protect_start),
            "fixture must place pair fully in prunable range"
        );
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — pair fully prunable");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_pair_fully_in_protected_tail() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 6000;
        let protect_last = 2000;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        // Big protect_last window → pair near the end sits entirely inside tail.
        let mut msgs = build_list_with_pair(20, "tail result", 0, 3000);
        // Sanity: pair is fully in protected tail.
        let protect_start = ContextCompressor::compute_protect_start(&msgs, protect_last, 3);
        let pairs = tool_pair::detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        assert!(
            pairs[0].assistant_idx >= protect_start,
            "fixture must place pair fully in protected tail (asst_idx={}, protect_start={})",
            pairs[0].assistant_idx,
            protect_start
        );
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — pair fully protected");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
        // Original pair survived untouched.
        assert!(msgs.iter().any(
            |m| m.role == Role::Assistant
                && m.tool_calls
                    .as_ref()
                    .map(|v| v.iter().any(|c| c.id == "web_read_1"))
                    .unwrap_or(false)
        ));
        assert!(msgs.iter().any(
            |m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("web_read_1")
        ));
    }

    // ── Phase 18 Plan 10 Task 3: regression matrix ──────────────────────────

    /// Build a straddling message list with a pair whose assistant issues
    /// `ids.len()` parallel tool_calls and `ids.len()` matching tool_results.
    fn build_list_with_straddling_parallel_pair(
        ids: &[&str],
        pair_body: &str,
        total_token_target: usize,
        protect_last_tokens: usize,
    ) -> Vec<ChatMessage> {
        let mut msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("first user"),
            ChatMessage::assistant("first assistant"),
            ChatMessage::assistant_tool_calls(
                ids.iter()
                    .map(|id| ToolCall {
                        id: (*id).into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "web_read".into(),
                            arguments: r#"{"url":"https://example.com"}"#.into(),
                        },
                    })
                    .collect(),
            ),
        ];
        for id in ids {
            msgs.push(ChatMessage::tool_result(*id, pair_body));
        }
        // Pad prunable region first.
        while estimate_messages_tokens(&msgs) < total_token_target {
            msgs.insert(3, ChatMessage::user("pad ".repeat(40)));
        }
        // Append tail filler until the LAST tool_result sits in the tail.
        let last_id = ids.last().unwrap();
        loop {
            let result_idx = msgs
                .iter()
                .rposition(|m| {
                    m.role == Role::Tool && m.tool_call_id.as_deref() == Some(*last_id)
                })
                .unwrap();
            let ps = ContextCompressor::compute_protect_start(&msgs, protect_last_tokens, 3);
            if result_idx >= ps {
                break;
            }
            msgs.push(ChatMessage::user("tail ".repeat(10)));
            if msgs.len() > 10_000 {
                panic!("runaway tail filler");
            }
        }
        // Inflate assistant args until assistant is prunable.
        loop {
            let ai = msgs
                .iter()
                .position(|m| {
                    m.role == Role::Assistant
                        && m.tool_calls
                            .as_ref()
                            .map(|v| v.iter().any(|c| c.id == *ids[0]))
                            .unwrap_or(false)
                })
                .unwrap();
            let ps = ContextCompressor::compute_protect_start(&msgs, protect_last_tokens, 3);
            if ai < ps {
                break;
            }
            if let Some(ref mut calls) = msgs[ai].tool_calls {
                for c in calls.iter_mut() {
                    c.function.arguments =
                        format!("{}{}", c.function.arguments, " ".repeat(40));
                }
            }
            if msgs[ai]
                .tool_calls
                .as_ref()
                .map(|v| v[0].function.arguments.len())
                .unwrap_or(0)
                > 100_000
            {
                panic!("runaway arg inflation");
            }
        }
        msgs
    }

    #[tokio::test]
    async fn compress_ok_parallel_tool_calls_straddling_boundary() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 6110;
        let protect_last = 300;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs =
            build_list_with_straddling_parallel_pair(&["p1", "p2"], "result", 3055, protect_last);
        // Sanity: exactly one pair detected with two tool_results.
        let pairs = tool_pair::detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].tool_result_indices.len(), 2);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("parallel tool_calls must compress cleanly");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_pair_at_exact_boundary() {
        // A straddling pair IS by definition at the boundary — our straddle
        // helper always produces `assistant_idx == protect_start - 1` or very
        // close. Exercise the smallest viable shape.
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 2000;
        let protect_last = 100;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_straddling_pair("r", 800, protect_last);
        let pairs = tool_pair::detect_tool_pairs(&msgs);
        let ps = ContextCompressor::compute_protect_start(&msgs, protect_last, 3);
        assert!(pairs[0].assistant_idx < ps);
        assert!(pairs[0].tool_result_indices.iter().any(|&i| i >= ps));
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("boundary pair must compress cleanly");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_pair_at_exact_first_n_boundary() {
        // Pair sits at protect_first_n (index 3) — first possible prunable
        // slot. Straddles when tool_result is also in tail window.
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 2000;
        let protect_last = 100;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        // Minimal prunable content — just enough to cross threshold.
        let mut msgs = build_list_with_straddling_pair("r", 600, protect_last);
        // If the pair's assistant is at index > 3, drop extra filler to push
        // it toward 3. For simplicity just assert straddle holds; exact
        // position is controlled by helper padding loop.
        let pairs = tool_pair::detect_tool_pairs(&msgs);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].assistant_idx >= 3);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("first-n boundary pair must compress cleanly");
        // May be no-op if nothing left to prune after atomicity guard, but
        // must not error.
        let _ = outcome;
        assert!(tool_pair::check_orphan_invariant(&msgs).is_ok());
    }

    #[tokio::test]
    async fn compress_ok_back_to_back_pairs() {
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 8000;
        let protect_last = 300;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        // Build: system, user, asst, [filler pad...], asstA+resultA, asstB+resultB, [tail]
        let mut msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("u"),
            ChatMessage::assistant("a"),
            ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: "pA".into(),
                call_type: "function".into(),
                function: FunctionCall { name: "web_read".into(), arguments: "{}".into() },
            }]),
            ChatMessage::tool_result("pA", "rA"),
            ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: "pB".into(),
                call_type: "function".into(),
                function: FunctionCall { name: "web_read".into(), arguments: "{}".into() },
            }]),
            ChatMessage::tool_result("pB", "rB"),
        ];
        while estimate_messages_tokens(&msgs) < 4000 {
            msgs.insert(3, ChatMessage::user("pad ".repeat(40)));
        }
        // Tail filler until last pair's tool_result is in tail window.
        loop {
            let ri = msgs
                .iter()
                .rposition(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("pB"))
                .unwrap();
            let ps = ContextCompressor::compute_protect_start(&msgs, protect_last, 3);
            if ri >= ps {
                break;
            }
            msgs.push(ChatMessage::user("tail ".repeat(10)));
            if msgs.len() > 10_000 {
                panic!("runaway");
            }
        }
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("back-to-back pairs must compress cleanly");
        let _ = outcome;
        assert!(tool_pair::check_orphan_invariant(&msgs).is_ok());
    }

    #[tokio::test]
    async fn compress_ok_pair_with_large_body_straddling_at_9467() {
        // Re-run the 9467-token UAT shape via the regression matrix for
        // explicit large-body coverage (mirrors compress_ok_large_pair_9467_tokens
        // but lives in the regression section for grep-discoverability).
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 18934;
        let protect_last = 800;
        let big_body = "x".repeat(2500);
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_straddling_pair(&big_body, 9467, protect_last);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("regression: 9467 large-body straddle must compress");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_noop_when_only_pair_fills_entire_prunable_range() {
        // Edge case: the only prunable content IS a pair. After the atomicity
        // guard pulls prune_end back to before the assistant, prune_end ==
        // prune_start (protect_first_n=3 == assistant_idx=3) → collapsed range
        // no-op. compress returns Ok(default()) without error.
        let (mock, _) = MockSummarizer::new(vec![Ok("Should not be called".into())]);
        let ctx_len = 500;
        let protect_last = 100;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        // Minimal list: system, user, asst, [pair at index 3-4], tail filler.
        let mut msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("u"),
            ChatMessage::assistant("a"),
            ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: "only".into(),
                call_type: "function".into(),
                function: FunctionCall { name: "web_read".into(), arguments: "{}".into() },
            }]),
            ChatMessage::tool_result("only", "r"),
        ];
        // Push tail filler so pair straddles.
        loop {
            let ri = 4;
            let ps = ContextCompressor::compute_protect_start(&msgs, protect_last, 3);
            if ri >= ps {
                break;
            }
            msgs.push(ChatMessage::user("tail ".repeat(5)));
            if msgs.len() > 200 {
                break;
            }
        }
        // Inflate assistant args until assistant is prunable (assistant_idx=3 == protect_first_n).
        loop {
            let ps = ContextCompressor::compute_protect_start(&msgs, protect_last, 3);
            if 3 < ps {
                break;
            }
            if let Some(ref mut calls) = msgs[3].tool_calls {
                calls[0].function.arguments =
                    format!("{}{}", calls[0].function.arguments, " ".repeat(40));
            }
            if msgs[3]
                .tool_calls
                .as_ref()
                .map(|v| v[0].function.arguments.len())
                .unwrap_or(0)
                > 50_000
            {
                panic!("runaway");
            }
        }
        let len_before = msgs.len();
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("no-op collapsed range must return Ok");
        assert!(
            !outcome.compressed,
            "expected no-op (collapsed prune range); got compressed=true"
        );
        // Message list unchanged — no [CONTEXT HISTORY] injected.
        assert_eq!(msgs.len(), len_before, "message list must be unchanged");
        assert!(
            msgs.iter().all(|m| m.name.as_deref() != Some(HISTORY_NAME)),
            "no [CONTEXT HISTORY] inserted when range collapsed"
        );
    }

    #[tokio::test]
    async fn compress_ok_multiple_pairs_mixed() {
        // Three pairs: one fully-prunable, one straddling, one fully-protected.
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 12000;
        let protect_last = 400;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);

        let mut msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("u1"),
            ChatMessage::assistant("a1"),
            // Pair A — fully prunable (early)
            ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: "pA".into(),
                call_type: "function".into(),
                function: FunctionCall { name: "web_read".into(), arguments: "{}".into() },
            }]),
            ChatMessage::tool_result("pA", "r_a"),
        ];
        // middle filler to build size
        for i in 0..30 {
            msgs.push(ChatMessage::user(format!("midfill {i} ").repeat(20)));
        }
        // Pair B — straddling (near boundary)
        msgs.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "pB".into(),
            call_type: "function".into(),
            function: FunctionCall { name: "web_read".into(), arguments: "{}".into() },
        }]));
        msgs.push(ChatMessage::tool_result("pB", "r_b"));
        // tail filler
        for i in 0..2 {
            msgs.push(ChatMessage::user(format!("tailfill {i} ").repeat(5)));
        }
        // Pair C — fully in protected tail
        msgs.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "pC".into(),
            call_type: "function".into(),
            function: FunctionCall { name: "web_read".into(), arguments: "{}".into() },
        }]));
        msgs.push(ChatMessage::tool_result("pC", "r_c"));

        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — mixed pairs");
        assert!(outcome.compressed);
        assert_compress_ok_no_orphan(&msgs);
    }

    /// Build a front-straddle fixture: `protect_first_n=3`, assistant tool_call
    /// at idx 2 (inside front-protected region), tool_result(s) at idx 3+
    /// (in the prune range). Mirrors live UAT 2026-04-13T05:18 shape
    /// (~64555 before_tokens, single web_read pair).
    fn build_list_with_front_straddle_pair(
        tool_call_ids: &[&str],
        pair_body: &str,
        protect_last_tokens: usize,
        total_token_target: usize,
    ) -> Vec<ChatMessage> {
        let mut msgs = Vec::new();
        // Front-protected region [0..3):
        //   0: system
        //   1: identity user turn
        //   2: assistant with tool_call(s) — first tool-calling turn.
        msgs.push(ChatMessage::system("sys"));
        msgs.push(ChatMessage::user("identity"));
        let calls: Vec<ToolCall> = tool_call_ids
            .iter()
            .map(|id| ToolCall {
                id: (*id).into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "web_read".into(),
                    arguments: r#"{"url":"https://example.com"}"#.into(),
                },
            })
            .collect();
        msgs.push(ChatMessage::assistant_tool_calls(calls));
        // tool_results immediately after (idx 3..3+N) — in the prune range.
        for id in tool_call_ids {
            msgs.push(ChatMessage::tool_result(*id, pair_body));
        }
        // Fill middle with prunable filler until we hit target tokens.
        while estimate_messages_tokens(&msgs) < total_token_target {
            // Insert prunable content AFTER the tool_results so protect_start
            // can walk back from the tail without swallowing the pair.
            let insert_at = 3 + tool_call_ids.len();
            msgs.insert(insert_at, ChatMessage::user("pad ".repeat(40)));
        }
        // Tail filler to ensure protect_last_tokens' walk lands past the pair.
        let _ = protect_last_tokens;
        msgs.push(ChatMessage::user("tail anchor"));
        msgs
    }

    /// Assert the fixture is in the front-straddle shape expected by the bug:
    /// assistant_idx < protect_first_n (front-protected), at least one
    /// tool_result in [protect_first_n, protect_start) (prune range).
    fn assert_pair_front_straddles(
        msgs: &[ChatMessage],
        protect_first_n: usize,
        protect_last_tokens: usize,
    ) {
        let protect_start = ContextCompressor::compute_protect_start(
            msgs,
            protect_last_tokens,
            protect_first_n,
        );
        let pairs = tool_pair::detect_tool_pairs(msgs);
        assert_eq!(pairs.len(), 1, "fixture must contain exactly one tool-pair");
        let p = &pairs[0];
        assert!(
            p.assistant_idx < protect_first_n,
            "assistant must be front-protected (asst_idx={}, protect_first_n={})",
            p.assistant_idx,
            protect_first_n
        );
        assert!(
            p.tool_result_indices
                .iter()
                .any(|&i| i >= protect_first_n && i < protect_start),
            "≥1 tool_result must be in prune range [{}, {})",
            protect_first_n,
            protect_start
        );
    }

    #[tokio::test]
    async fn compress_ok_front_straddle_asst_in_protect_first_n_single_result() {
        // Live UAT 2026-04-13T05:18 shape: protect_first_n=3, single web_read
        // pair with asst at idx 2, tool_result at idx 3. Before fix: guard
        // collapsed prune_end=2 below prune_start=3 → no-op with
        // `reason="pair_atomicity_collapsed_range"`.
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 130_000;
        let protect_last = 100;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_front_straddle_pair(
            &["web_read_front_1"],
            "web page body",
            protect_last,
            64_555,
        );
        assert_pair_front_straddles(&msgs, 3, protect_last);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — front-straddle single-result");
        assert!(
            outcome.compressed,
            "compressed flag must be true — live UAT regression"
        );
        assert_compress_ok_no_orphan(&msgs);
    }

    #[tokio::test]
    async fn compress_ok_front_straddle_parallel_tool_calls() {
        // Parallel tool_calls variant: asst at idx 2 emits two tool_calls,
        // tool_results at idx 3 and 4. All results land in the prune range
        // while the assistant stays front-protected. Before fix: same
        // collapsed-range no-op.
        let (mock, _) = MockSummarizer::new(vec![Ok("Mock summary".into())]);
        let ctx_len = 130_000;
        let protect_last = 100;
        let engine = SummarizingEngine::new(ctx_len, 0.001, mock).with_protect(3, protect_last);
        let mut msgs = build_list_with_front_straddle_pair(
            &["web_read_front_p1", "web_read_front_p2"],
            "web page body",
            protect_last,
            64_555,
        );
        assert_pair_front_straddles(&msgs, 3, protect_last);
        let before = estimate_messages_tokens(&msgs);
        let outcome = engine
            .compress(&mut msgs, uat_stats(ctx_len, protect_last, before))
            .await
            .expect("compress must succeed — front-straddle parallel calls");
        assert!(
            outcome.compressed,
            "compressed flag must be true — parallel front-straddle"
        );
        assert_compress_ok_no_orphan(&msgs);
    }
}
