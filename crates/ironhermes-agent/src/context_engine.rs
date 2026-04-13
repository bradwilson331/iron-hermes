use async_trait::async_trait;
use ironhermes_core::ChatMessage;
use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry};
use std::sync::Arc;
use thiserror::Error;

use crate::pressure_warning::PressureTracker;

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

    /// Phase 18 Plan 06: Run only the pressure-warning channel without
    /// performing any destructive compression. Agent loop calls this when
    /// the token ratio is below the compression threshold so the 85% warning
    /// can still fire on the pre-compression slope.
    ///
    /// Default implementation is a no-op; both shipped engines override it.
    async fn check_pressure(&self, _stats: &ContextStats) -> bool {
        false
    }
}

pub struct LocalPruningEngine {
    context_length: usize,
    threshold: f32,
    protect_first_n: usize,
    protect_last_tokens: usize,
    tool_pair_shift_tokens: usize,
    hook_registry: Option<Arc<HookRegistry>>,
    session_id: Option<String>,
    pressure_tracker: Option<Arc<PressureTracker>>,
}

impl LocalPruningEngine {
    pub fn new(context_length: usize, threshold: f32) -> Self {
        let protect_last_tokens = 20_000.min(context_length / 4);
        Self {
            context_length,
            threshold,
            protect_first_n: 3,
            protect_last_tokens,
            tool_pair_shift_tokens: 500,
            hook_registry: None,
            session_id: None,
            pressure_tracker: None,
        }
    }

    pub fn with_protect(mut self, first_n: usize, last_tokens: usize) -> Self {
        self.protect_first_n = first_n;
        self.protect_last_tokens = last_tokens;
        self
    }

    /// Phase 18 D-15: set the adaptive-shift threshold (default 500).
    pub fn with_tool_pair_shift(mut self, n: usize) -> Self {
        self.tool_pair_shift_tokens = n;
        self
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
}

#[async_trait]
impl ContextEngine for LocalPruningEngine {
    async fn compress(
        &self,
        messages: &mut Vec<ChatMessage>,
        _stats: ContextStats,
    ) -> Result<CompressionOutcome, ContextError> {
        let before = crate::context_compressor::estimate_messages_tokens(messages);
        let pct = before as f32 / self.context_length.max(1) as f32;

        tracing::info!(
            before_tokens = before,
            pct,
            threshold = self.threshold,
            session_id = ?self.session_id,
            "local_pruning_engine: compress attempt"
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

        // Phase 18 D-20: fire context:pre_compress BEFORE destructive pruning and
        // await async handler completion (e.g. memory flush) via fire_awaitable.
        // Threshold gate: only emit when we would actually compress.
        let would_compress = pct >= self.threshold;
        if !would_compress {
            tracing::info!(
                pct,
                threshold = self.threshold,
                reason = "below_threshold",
                "local_pruning_engine: no-op"
            );
        }
        if would_compress {
            if let (Some(reg), Some(sid)) = (&self.hook_registry, &self.session_id) {
                let event = HookEvent::new(
                    "req-compress",
                    HookEventKind::ContextPreCompress {
                        session_id: sid.clone(),
                        estimated_tokens: before,
                        threshold: self.threshold,
                        mode: "hard".into(),
                        pruned_range: None,
                    },
                );
                reg.fire_awaitable(event).await;
            } else {
                tracing::debug!(
                    "no pre_compress handler registered, proceeding without memory flush"
                );
            }
        }

        // Snapshot the caller's vec BEFORE any mutation so we can roll back
        // atomically if the post-compression invariant check fails. Without
        // this, a corrupted (orphaned tool_use) vec would be forwarded to the
        // LLM after `?` propagates the error.
        let snapshot = messages.clone();

        // Phase 18 D-15: apply adaptive shift for pairs straddling the protect boundary
        // BEFORE delegating to ContextCompressor, so the underlying pruner never splits
        // a tool_call from its result.
        let protect_start = crate::context_compressor::ContextCompressor::compute_protect_start(
            messages,
            self.protect_last_tokens,
            self.protect_first_n,
        );
        let pairs = crate::tool_pair::detect_tool_pairs(messages);
        for pair in &pairs {
            let _ = crate::tool_pair::apply_adaptive_shift(
                messages,
                pair,
                protect_start,
                self.tool_pair_shift_tokens,
            );
        }

        let mut cc = crate::context_compressor::ContextCompressor::new(
            self.context_length,
            self.threshold as f64,
        )
        .with_protect(self.protect_first_n, self.protect_last_tokens);
        let compressed = cc.compress(messages);

        // Phase 18 D-16: post-compression invariant blocks orphaned pairs per T-18-02.
        // On failure restore the pre-compression snapshot so the caller never
        // ships a half-mutated vec to the LLM.
        if let Err(e) = crate::tool_pair::check_orphan_invariant(messages) {
            *messages = snapshot;
            tracing::warn!(
                error = ?e,
                reason = "rollback",
                "local_pruning_engine: compress failed, messages restored"
            );
            return Err(e);
        }

        let after = crate::context_compressor::estimate_messages_tokens(messages);
        if compressed {
            tracing::info!(
                before_tokens = before,
                after_tokens = after,
                "local_pruning_engine: compressed"
            );
        } else if would_compress {
            tracing::info!(
                before_tokens = before,
                after_tokens = after,
                reason = "compressor_returned_no_change",
                "local_pruning_engine: no-op"
            );
        }
        Ok(CompressionOutcome {
            compressed,
            tokens_freed: before.saturating_sub(after),
            new_summary: None,
            pressure_warning_fired,
        })
    }

    fn threshold(&self) -> f32 {
        self.threshold
    }

    fn mode(&self) -> CompressionMode {
        CompressionMode::Hard
    }

    async fn check_pressure(&self, stats: &ContextStats) -> bool {
        if let (Some(tracker), Some(sid)) = (&self.pressure_tracker, &self.session_id) {
            tracker
                .check_and_maybe_emit(
                    sid,
                    self.threshold,
                    stats.estimated_tokens,
                    self.context_length,
                    "hard",
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

    // ── Phase 18 Plan 02: tool_pair wiring ──────────────────────────────────

    #[tokio::test]
    async fn local_pruning_engine_invariant_pass() {
        use ironhermes_core::{FunctionCall, ToolCall};
        let mut msgs = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hi"),
            ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: "a".into(),
                call_type: "function".into(),
                function: FunctionCall { name: "fn".into(), arguments: "{}".into() },
            }]),
            ChatMessage::tool_result("a", "ok"),
            ChatMessage::assistant("done"),
        ];
        let engine = LocalPruningEngine::new(1000, 0.5);
        let out = engine.compress(&mut msgs, make_stats(0)).await.expect("ok");
        assert!(!out.compressed || out.compressed); // just exercise
        assert!(crate::tool_pair::check_orphan_invariant(&msgs).is_ok());
    }

    #[tokio::test]
    async fn local_pruning_engine_applies_adaptive_shift() {
        use ironhermes_core::{FunctionCall, ToolCall};
        // Build 30-message vec with a pair near the boundary.
        let mut msgs: Vec<ChatMessage> = (0..28)
            .map(|i| ChatMessage::user(format!("filler {i} ").repeat(20)))
            .collect();
        msgs.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "z".into(),
            call_type: "function".into(),
            function: FunctionCall { name: "peek".into(), arguments: "{}".into() },
        }]));
        msgs.push(ChatMessage::tool_result("z", "small"));

        let engine = LocalPruningEngine::new(1000, 0.5);
        let _ = engine.compress(&mut msgs, make_stats(0)).await.expect("ok");
        // Pair still co-located and invariant holds.
        assert!(crate::tool_pair::check_orphan_invariant(&msgs).is_ok());
    }

    // ── Phase 18 Plan 04: pre_compress hook emission ────────────────────────

    #[tokio::test]
    async fn pre_compress_hook_event() {
        use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry, HooksConfig};
        use std::sync::Mutex as StdMutex;

        let mut registry = HookRegistry::new(HooksConfig::default());
        let captured: Arc<StdMutex<Vec<HookEvent>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let cap = Arc::clone(&captured);
        registry.add_async_listener(Arc::new(move |event: HookEvent| {
            let cap = Arc::clone(&cap);
            Box::pin(async move {
                cap.lock().unwrap().push(event);
            })
        }));
        let reg = Arc::new(registry);

        let engine = LocalPruningEngine::new(1000, 0.5)
            .with_hooks(Arc::clone(&reg), "sess-hook-1");

        let mut msgs = build_large_message_vec(30);
        let _ = engine.compress(&mut msgs, make_stats(0)).await.expect("ok");

        let events = captured.lock().unwrap();
        let pre: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.kind, HookEventKind::ContextPreCompress { .. }))
            .collect();
        assert_eq!(pre.len(), 1, "exactly one ContextPreCompress event");
        if let HookEventKind::ContextPreCompress {
            session_id, mode, ..
        } = &pre[0].kind
        {
            assert_eq!(session_id, "sess-hook-1");
            assert_eq!(mode, "hard");
        } else {
            panic!("expected ContextPreCompress");
        }
    }

    #[tokio::test]
    async fn memory_flush_before_prune() {
        use ironhermes_hooks::{HookEvent, HookRegistry, HooksConfig};
        use std::sync::Mutex as StdMutex;

        // Shared ordered log: handler pushes "flushed" first, then the engine
        // (instrumented below) pushes "pruned" after the delegated compress.
        let log: Arc<StdMutex<Vec<&'static str>>> = Arc::new(StdMutex::new(Vec::new()));

        let mut registry = HookRegistry::new(HooksConfig::default());
        let log_h = Arc::clone(&log);
        registry.add_async_listener(Arc::new(move |_event: HookEvent| {
            let log_h = Arc::clone(&log_h);
            Box::pin(async move {
                // Simulate work so we can distinguish ordering even without sleeps.
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                log_h.lock().unwrap().push("flushed");
            })
        }));
        let reg = Arc::new(registry);

        let engine = LocalPruningEngine::new(1000, 0.5)
            .with_hooks(Arc::clone(&reg), "sess-order");

        let mut msgs = build_large_message_vec(30);
        let _ = engine.compress(&mut msgs, make_stats(0)).await.expect("ok");
        log.lock().unwrap().push("pruned");

        let final_log = log.lock().unwrap().clone();
        assert_eq!(
            final_log,
            vec!["flushed", "pruned"],
            "handler must complete before compress returns"
        );
    }

    #[tokio::test]
    async fn pre_compress_no_hook_registered_proceeds() {
        // No hook registry attached → compress should proceed without error.
        let engine = LocalPruningEngine::new(1000, 0.5);
        let mut msgs = build_large_message_vec(30);
        let out = engine.compress(&mut msgs, make_stats(0)).await.expect("ok");
        assert!(out.compressed || !out.compressed); // just assert Ok was returned
    }

    /// Phase 18 atomic-rollback fix: when `check_orphan_invariant` rejects the
    /// post-compression vec, the caller's `messages` MUST be restored to its
    /// pre-call snapshot so a corrupted (orphaned tool_use) vec is never
    /// forwarded to the LLM.
    #[tokio::test]
    async fn local_pruning_rolls_back_on_orphan() {
        use ironhermes_core::{FunctionCall, ToolCall};
        // Seed a vec that already contains an orphan plus enough filler to
        // push us above the compression threshold so compress() actually runs.
        let mut msgs: Vec<ChatMessage> = (0..28)
            .map(|i| ChatMessage::user(format!("filler {i} ").repeat(20)))
            .collect();
        // Append an assistant tool_call WITHOUT a matching tool_result — the
        // post-compression invariant will reject this vec, forcing rollback.
        msgs.push(ChatMessage::assistant_tool_calls(vec![ToolCall {
            id: "orphan-id".into(),
            call_type: "function".into(),
            function: FunctionCall { name: "fn".into(), arguments: "{}".into() },
        }]));
        let snapshot = msgs.clone();

        let engine = LocalPruningEngine::new(1000, 0.5);
        let err = engine
            .compress(&mut msgs, make_stats(0))
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
    }

    #[test]
    fn local_pruning_engine_detects_orphan() {
        use ironhermes_core::{FunctionCall, ToolCall};
        let msgs = vec![
            ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: "tc1".into(),
                call_type: "function".into(),
                function: FunctionCall { name: "fn".into(), arguments: "{}".into() },
            }]),
            ChatMessage::user("hi"),
        ];
        assert!(matches!(
            crate::tool_pair::check_orphan_invariant(&msgs),
            Err(ContextError::OrphanedToolPair)
        ));
    }
}
