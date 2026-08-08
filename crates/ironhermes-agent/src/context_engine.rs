use async_trait::async_trait;
use ironhermes_core::ChatMessage;
use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex as TokioMutex;

use crate::memory::MemoryManager;
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

    // ── Phase 34b Plan 02 (D-06): lifecycle hooks ──────────────────────────
    //
    // Ported from `hermes-agent/agent/context_engine.py`. All five are ADDITIVE
    // default-no-op methods following the `check_pressure` idiom, so existing
    // implementors (`LocalPruningEngine`, `SummarizingEngine`) inherit the
    // no-ops and compile unchanged. `&self` only — any state clearing uses
    // interior mutability (see `ContextCompressor`). Per-turn hooks
    // (`update_from_response`, `update_model`) are invoked ONCE centrally in
    // `AgentRuntime::run_turn` (D-09); per-session hooks (`on_session_start`,
    // `on_session_reset`) are wired at the surfaces where the durable
    // per-session counter lives (D-10).

    /// Called when a new session begins. Default no-op.
    fn on_session_start(&self, _session_id: &str) {}

    /// Called when a session is reset (`/new`). Engines holding durable
    /// per-session state zero it here. Default no-op.
    fn on_session_reset(&self) {}

    /// Called once per turn (centrally in `run_turn`) with the post-run
    /// aggregated token usage. Default no-op.
    fn update_from_response(&self, _usage: &crate::agent_loop::AggregatedUsage) {}

    /// Called once per turn (centrally in `run_turn`) with the resolved model
    /// identity for the turn. Default no-op.
    fn update_model(&self, _model: &str, _context_length: usize, _base_url: Option<&str>) {}

    /// Whether there is content worth compressing. Default `true`.
    fn has_content_to_compress(&self, _messages: &[ChatMessage]) -> bool {
        true
    }
}

pub struct LocalPruningEngine {
    context_length: usize,
    threshold: f32,
    /// Phase 47.5 (D-05): an UPPER BOUND on the protected leading
    /// system-message run — not a raw message count. `ContextCompressor`
    /// applies `system_prefix_len(messages).min(protect_first_n)` internally,
    /// so the first user/assistant pair is never pinned regardless of this
    /// value.
    protect_first_n: usize,
    protect_last_tokens: usize,
    tool_pair_shift_tokens: usize,
    hook_registry: Option<Arc<HookRegistry>>,
    session_id: Option<String>,
    pressure_tracker: Option<Arc<PressureTracker>>,
    /// Plan 20-02: memory manager is invoked at the top of `compress` so the
    /// provider's `on_pre_compress` hook can react (e.g. flush working-memory
    /// deltas) BEFORE destructive pruning.
    memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
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
            memory_manager: None,
        }
    }

    /// Plan 20-02: attach the MemoryManager so its `on_pre_compress` hook
    /// fires before prune.
    pub fn with_memory_manager(mut self, manager: Arc<TokioMutex<MemoryManager>>) -> Self {
        self.memory_manager = Some(manager);
        self
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

    /// Phase 18 D-20: attach a hook registry so `compress` fires
    /// `context:pre_compress` and awaits handler completion before pruning.
    ///
    /// Session id is now attached independently via `with_session_id` (18-13
    /// gap-closure). `with_hooks` no longer accepts a session_id argument so
    /// that PressureTracker wiring works even when no hook registry is installed
    /// (the CLI default wiring path).
    pub fn with_hooks(mut self, registry: Arc<HookRegistry>) -> Self {
        self.hook_registry = Some(registry);
        self
    }

    /// Phase 18-13: attach a session id independently of hook registry presence.
    ///
    /// Required for PressureTracker wiring in the CLI default path where
    /// `hooks = None` but the tracker still needs a `session_id` to key its
    /// per-session hysteresis state.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
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

        // Plan 20-02 hook-ordering contract: `on_pre_compress` fires at the top
        // of compress BEFORE any mutation so providers can stash facts that
        // would otherwise be pruned. Mirror failures are logged by the manager
        // and do not abort compression.
        if let Some(mgr) = &self.memory_manager {
            let guard = mgr.lock().await;
            if let Err(e) = guard.on_pre_compress(messages).await {
                tracing::warn!(error = %e, "memory.on_pre_compress failed; continuing");
            }
        }

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

        let cc = crate::context_compressor::ContextCompressor::new(
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

        let cc = crate::context_compressor::ContextCompressor::new(1000, 0.5);
        let _ = cc.compress(&mut via_compressor);

        assert_eq!(via_engine.len(), via_compressor.len());
        for (a, b) in via_engine.iter().zip(via_compressor.iter()) {
            assert_eq!(a.content_text(), b.content_text());
        }
    }

    #[tokio::test]
    async fn test_protect_boundaries() {
        // Phase 47.5 (D-05): `build_large_message_vec` is all-`Role::User` —
        // no leading system-message run — so under the role-aware floor
        // NOTHING is front-pinned (`system_prefix_len` is 0 regardless of the
        // configured `protect_first_n`). This inverts the pre-47.5 assertion,
        // which expected `messages[0..3]` to survive verbatim purely by raw
        // index. That was exactly the D-01/D-05 bug: a non-system first
        // conversation pair getting pinned forever.
        let mut messages = build_large_message_vec(30);
        let engine = LocalPruningEngine::new(1000, 0.5);
        let _ = engine
            .compress(&mut messages, make_stats(0))
            .await
            .expect("ok");
        // None of the original leading user messages are guaranteed to
        // survive verbatim at a fixed index anymore — the front is prunable.
        assert!(
            !messages
                .iter()
                .take(3)
                .any(|m| m.content_text().unwrap_or("").starts_with("message 0 ")),
            "no leading user message should be structurally pinned by raw index"
        );
    }

    // ── Phase 47.5 Plan 03 (D-07): first-conversation-pair pin regression ──

    /// D-07 RED test 3 (research Mechanism 1 / W2): the session's FIRST
    /// conversation pair — whatever topic happened to open it — must not be
    /// structurally pinned into every compressed context forever. Reproduces
    /// the incident shape: a days-old off-topic reel exchange opens a long
    /// session and derails an unrelated codebase question. Pre-fix,
    /// `protect_first_n: 3` pins `messages[0..3]` = system + first user +
    /// first assistant verbatim on every pass, so the reel content survives
    /// in the head no matter how the conversation moves on.
    #[tokio::test]
    async fn compression_does_not_pin_first_conversation_pair() {
        let mut msgs = vec![
            ChatMessage::system("You are Hermes."),
            ChatMessage::user("https://instagram.com/reel/offtopic"),
            ChatMessage::assistant("Reel analysis: it's a cooking video."),
        ];
        // Bulk filler large enough to cross the 0.5 threshold of a 1_000-token
        // test context (mirrors build_large_message_vec's ~50 tokens/message).
        for i in 0..50 {
            msgs.push(ChatMessage::user(format!("bulk message {i} ").repeat(20)));
        }
        msgs.push(ChatMessage::user("show me the /new command code"));

        let engine = LocalPruningEngine::new(1_000, 0.5);
        let _ = engine
            .compress(&mut msgs, make_stats(0))
            .await
            .expect("ok");

        // The system prompt survives at index 0.
        assert_eq!(
            msgs[0].role,
            ironhermes_core::Role::System,
            "system prompt must survive at index 0"
        );
        assert_eq!(
            msgs[0].content_text(),
            Some("You are Hermes."),
            "system prompt content must survive unchanged at index 0"
        );

        // No message in the surviving head (first 3 positions) contains the
        // off-topic reel content — the first conversation pair was prunable
        // like everything else. RED today: protect_first_n=3 pins indices
        // 0..3, so the reel user/assistant pair survives verbatim at 1..3.
        for (i, m) in msgs.iter().take(3).enumerate() {
            let text = m.content_text().unwrap_or("");
            assert!(
                !text.contains("instagram.com/reel") && !text.contains("Reel analysis"),
                "head[{i}] must not contain pinned off-topic reel content, got: {text}"
            );
        }

        // The current question still survives in the protected tail.
        assert!(
            msgs.iter().any(|m| m
                .content_text()
                .map(|t| t.contains("/new command code"))
                .unwrap_or(false)),
            "current question must survive compression in the protected tail"
        );
    }

    /// Review finding 8: system-run matrix. Asserts the exact protected-front
    /// length (`ContextCompressor::compute_protect_start`) against a
    /// configured cap of 3, across zero/one/below-cap/above-cap leading
    /// system-message shapes, plus the small-vec no-op guard (review finding
    /// 13) — re-keying the guards on `front` (0 for an all-user vec) must not
    /// start compressing tiny conversations.
    #[tokio::test]
    async fn compression_protects_system_run_matrix() {
        use crate::context_compressor::ContextCompressor;

        let cap = 3usize;
        // Large enough that every small fixture below fits entirely inside
        // the tail-token walk, so compute_protect_start's tail_start walk
        // reaches index 0 and the returned value collapses to exactly the
        // computed front (`tail_start.max(front)` == `front`).
        let protect_last_tokens = 100_000usize;

        // 0 system messages (all-user vec): protected front is 0 -- nothing
        // is pinned.
        let zero_system = vec![ChatMessage::user("a"), ChatMessage::user("b")];
        let ps = ContextCompressor::compute_protect_start(&zero_system, protect_last_tokens, cap);
        assert_eq!(ps, 0, "0 leading system messages -> protected front 0");

        // 1 system message: protected front is 1 -- the system prompt and
        // nothing else.
        let one_system = vec![ChatMessage::system("sys"), ChatMessage::user("a")];
        let ps = ContextCompressor::compute_protect_start(&one_system, protect_last_tokens, cap);
        assert_eq!(ps, 1, "1 leading system message -> protected front 1");

        // 2 system messages (below the cap): protected front is 2 -- the
        // whole run.
        let two_system = vec![
            ChatMessage::system("sys1"),
            ChatMessage::system("sys2"),
            ChatMessage::user("a"),
        ];
        let ps = ContextCompressor::compute_protect_start(&two_system, protect_last_tokens, cap);
        assert_eq!(
            ps, 2,
            "2 leading system messages (below cap 3) -> protected front 2 (whole run)"
        );

        // 5 system messages (above the cap): protected front is 3 -- capped,
        // the deliberate upper-bound behavior.
        let mut five_system: Vec<ChatMessage> = (0..5)
            .map(|i| ChatMessage::system(format!("sys{i}")))
            .collect();
        five_system.push(ChatMessage::user("a"));
        let ps = ContextCompressor::compute_protect_start(&five_system, protect_last_tokens, cap);
        assert_eq!(
            ps, 3,
            "5 leading system messages (above cap 3) -> protected front capped at 3"
        );

        // Small-vec guard: a short all-user vec below the compression
        // threshold comes back UNCHANGED — re-keying the no-op guards on
        // `front` (0 here) must not start compressing tiny conversations.
        let mut tiny = vec![ChatMessage::user("hi"), ChatMessage::user("there")];
        let tiny_before = tiny.clone();
        let engine = LocalPruningEngine::new(1_000, 0.5);
        let _ = engine
            .compress(&mut tiny, make_stats(0))
            .await
            .expect("ok");
        assert_eq!(tiny.len(), tiny_before.len(), "tiny vec length unchanged");
        for (a, b) in tiny.iter().zip(tiny_before.iter()) {
            assert_eq!(
                a.content_text(),
                b.content_text(),
                "tiny vec content unchanged"
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
                function: FunctionCall {
                    name: "fn".into(),
                    arguments: "{}".into(),
                },
            }]),
            ChatMessage::tool_result("a", "ok"),
            ChatMessage::assistant("done"),
        ];
        let engine = LocalPruningEngine::new(1000, 0.5);
        // .expect("ok") is the success assertion; this test exercises compress
        // and then verifies the orphan invariant below.
        engine.compress(&mut msgs, make_stats(0)).await.expect("ok");
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
            function: FunctionCall {
                name: "peek".into(),
                arguments: "{}".into(),
            },
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
        let captured: Arc<StdMutex<Vec<HookEvent>>> = Arc::new(StdMutex::new(Vec::new()));
        let cap = Arc::clone(&captured);
        registry.add_async_listener(Arc::new(move |event: HookEvent| {
            let cap = Arc::clone(&cap);
            Box::pin(async move {
                cap.lock().unwrap().push(event);
            })
        }));
        let reg = Arc::new(registry);

        let engine = LocalPruningEngine::new(1000, 0.5)
            .with_hooks(Arc::clone(&reg))
            .with_session_id("sess-hook-1");

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
            .with_hooks(Arc::clone(&reg))
            .with_session_id("sess-order");

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
        // No hook registry attached → compress should proceed without error;
        // .expect("ok") is the assertion that Ok was returned.
        engine.compress(&mut msgs, make_stats(0)).await.expect("ok");
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
            function: FunctionCall {
                name: "fn".into(),
                arguments: "{}".into(),
            },
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

    // ── Phase 18 Plan 13: pressure-check fires without hooks (gap-closure) ──

    #[tokio::test]
    async fn pressure_check_fires_when_session_id_attached_without_hooks() {
        use crate::pressure_warning::PressureTracker;

        let tracker = Arc::new(PressureTracker::new());
        // No .with_hooks() — simulates CLI wiring (hooks = None).
        // context_length = 100_000, threshold = 0.50
        // warning_trigger = 0.50 * 0.85 = 0.425 → fires at estimated_tokens >= 42_500.
        let engine = LocalPruningEngine::new(100_000, 0.50)
            .with_session_id("sess-test-1")
            .with_pressure_tracker(tracker.clone());

        // Use check_pressure directly with stats that put us in the band
        // [42_500, 50_000) — avoids running the full compress path and the
        // need to craft a correctly-sized message vec.
        let stats = ContextStats {
            context_length: 100_000,
            estimated_tokens: 46_000, // ratio=0.46, above 85% warning trigger (0.425)
            protect_first_n: 3,
            protect_last_tokens: 100,
            compression_count: 0,
            prior_summary: None,
        };

        // check_pressure exercises the (tracker, sid) pressure gate directly.
        // Before 18-13: with_session_id didn't exist, so tracker.was_warned
        // would always be false (session_id was None).
        // After 18-13: session_id is set, pressure gate fires.
        let fired = engine.check_pressure(&stats).await;

        assert!(
            fired,
            "pressure_warning_fired must be true when session_id attached without hooks"
        );
        assert!(
            tracker.was_warned("sess-test-1"),
            "pressure tracker must fire when session_id attached without hooks"
        );
    }

    #[test]
    fn local_pruning_engine_detects_orphan() {
        use ironhermes_core::{FunctionCall, ToolCall};
        let msgs = vec![
            ChatMessage::assistant_tool_calls(vec![ToolCall {
                id: "tc1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "fn".into(),
                    arguments: "{}".into(),
                },
            }]),
            ChatMessage::user("hi"),
        ];
        assert!(matches!(
            crate::tool_pair::check_orphan_invariant(&msgs),
            Err(ContextError::OrphanedToolPair)
        ));
    }
}
