//! Shared wiring helper for Phase 18 context compression.
//!
//! All production AgentLoop construction sites (CLI single-turn, CLI
//! `run_agent_turn`, gateway `run_agent`) must call `attach_context_engine`
//! so `agent.compression_threshold` and `agent.context_engine` are honored.
//!
//! Without this call the agent's `context_engine` stays `None` and
//! `pre_chat_compress` falls through to the legacy compressor path
//! (which ignores the config threshold).

use std::sync::Arc;

use ironhermes_core::{Config, ProviderResolver};
use ironhermes_hooks::HookRegistry;

use crate::agent_loop::AgentLoop;
use crate::engine_factory::build_context_engine;
use crate::pressure_warning::PressureTracker;

/// Default context length used when no per-endpoint value is plumbed.
/// Matches the value CLI already hardcoded in `with_compression(128_000, _)`.
/// Phase 21 will derive this from the resolver.
pub const DEFAULT_CONTEXT_LENGTH: usize = 128_000;

/// Attach `agent.context_engine`, `PressureTracker`, and `session_id` to an
/// `AgentLoop` using the agent-side compression config
/// (`config.agent.context_engine` + `config.agent.compression_threshold`).
///
/// Returns the same `AgentLoop` with the builders applied. Call this
/// BEFORE `agent.run(messages).await`.
///
/// ## Phase 18 Plan 14: caller-provided tracker
///
/// When `tracker` is `Some`, the supplied `Arc<PressureTracker>` is reused
/// verbatim — enabling a single CLI REPL session to share one tracker across
/// multiple `run_agent_turn` calls and preserve hysteresis state between turns.
///
/// When `tracker` is `None` (the common one-shot path), a fresh
/// `PressureTracker` is created as before — backwards-compatible with all
/// existing call sites.
pub fn attach_context_engine(
    agent: AgentLoop,
    config: &Config,
    resolver: &ProviderResolver,
    session_id: impl Into<String>,
    hooks: Option<Arc<HookRegistry>>,
    tracker: Option<Arc<PressureTracker>>,
) -> AgentLoop {
    let sid = session_id.into();
    let tracker = tracker.unwrap_or_else(|| Arc::new(PressureTracker::new()));
    let engine = build_context_engine(
        config,
        &config.agent.context_engine,
        resolver,
        DEFAULT_CONTEXT_LENGTH,
        config.agent.compression_threshold,
        sid.clone(),
        hooks,
        Some(tracker.clone()),
    );
    agent
        .with_context_engine(engine, DEFAULT_CONTEXT_LENGTH)
        .with_pressure_tracker(tracker)
        .with_session_id(sid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentLoop, AnyClient, LlmClient};
    use ironhermes_core::ChatMessage;
    use ironhermes_tools::ToolRegistry;

    fn bare_agent() -> AgentLoop {
        let client = AnyClient::ChatCompletions(LlmClient::new(
            "http://localhost:0".to_string(),
            "test".to_string(),
            "test-model",
        ));
        AgentLoop::new(client, Arc::new(ToolRegistry::new()), 4)
    }

    #[test]
    fn attach_context_engine_wires_all_three_builders() {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        // Phase 18-14: pass None for tracker → backwards-compatible fresh tracker.
        let agent = attach_context_engine(bare_agent(), &config, &resolver, "sess-1", None, None);
        assert!(agent.has_context_engine());
        assert!(agent.has_pressure_tracker());
        assert_eq!(agent.session_id(), Some("sess-1".to_string()));
    }

    #[test]
    fn attach_context_engine_uses_config_threshold() {
        let mut config = Config::default();
        config.agent.context_engine = "local_prune".to_string();
        config.agent.compression_threshold = 0.42;
        let resolver = ProviderResolver::build(&config).unwrap();
        let agent = attach_context_engine(bare_agent(), &config, &resolver, "sess-2", None, None);
        let t = agent.context_engine_threshold().unwrap();
        assert!((t - 0.42).abs() < 1e-4);
    }

    /// Phase 18-14: when a caller-provided Arc<PressureTracker> is passed, it
    /// is reused verbatim.  The strong count increases as the tracker is cloned
    /// into both the context engine and the AgentLoop.
    #[test]
    fn attach_context_engine_reuses_caller_tracker() {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        let t = Arc::new(PressureTracker::new());
        // Baseline: caller holds one reference.
        assert_eq!(Arc::strong_count(&t), 1);
        let _agent =
            attach_context_engine(bare_agent(), &config, &resolver, "sess-3", None, Some(t.clone()));
        // After wiring: caller (1) + AgentLoop (1) + inside engine (1) = >= 3.
        assert!(
            Arc::strong_count(&t) >= 3,
            "expected >= 3 strong references, got {}",
            Arc::strong_count(&t)
        );
    }

    // ── Phase 18-14: REPL hysteresis harness ───────────────────────────────
    //
    // Simulates 3 consecutive user prompts in the same CLI session by calling
    // attach_context_engine three times on a fresh AgentLoop but reusing the
    // SAME Arc<PressureTracker>.  Exercises pre_chat_compress (the real
    // transient-drain + check_pressure code path the REPL hits) and asserts:
    //   - warn_count stays at exactly 1 across all three turns
    //     (hysteresis survives the AgentLoop rebuild)
    //   - Turn 2's outbound message vector contains a system message whose
    //     body starts with "[CONTEXT PRESSURE HIGH" (the transient drained)
    //
    // RED without the Task 1+2 fixes: attach_context_engine creates a fresh
    // PressureTracker each call, so turn 2 sees warn_count=1 again (fires),
    // and turn 2's messages never get the transient (it was queued on a
    // tracker that was dropped at end of turn 1).

    fn make_in_band_messages() -> Vec<ChatMessage> {
        // Craft messages whose estimate_messages_tokens lands in the 85%
        // pressure band but BELOW the compression threshold, so
        // pre_chat_compress's `if ratio >= threshold` gate takes the `else`
        // branch (check_pressure) instead of the compress branch.
        //
        // Engine config: threshold = 0.01, warning_trigger = 0.01 * 0.85 = 0.0085.
        // DEFAULT_CONTEXT_LENGTH = 128_000.
        // → need estimated_tokens ∈ [128_000 * 0.0085, 128_000 * 0.01)
        //   = [1088, 1280) tokens.
        //
        // estimate_message_tokens = 4 + estimate_tokens(content)
        //                         = 4 + (content.len() / 4 + 1)
        // Four 4_400-char user messages:
        //   4 + (4400/4 + 1) = 4 + 1101 = 1105 tokens each? No — we want smaller.
        // One 4_400-char user message:
        //   4 + (4400/4 + 1) = 4 + 1101 = 1105. Plus 3 overhead = 1108. ✓ in band.
        // Use len=4400 to be safely inside [1088, 1280).
        let filler = "x".repeat(4400);
        vec![ChatMessage::user(filler.as_str())]
    }

    fn band_config() -> Config {
        let mut config = Config::default();
        config.agent.context_engine = "local_prune".to_string();
        // 0.01 threshold gives a wider band so the test is robust against
        // small token-estimation drift. ratio ~0.0087 will fire check_pressure
        // (above 0.0085 trigger) without crossing compression (0.01).
        config.agent.compression_threshold = 0.01;
        config
    }

    /// PHASE 18-14 RED/GREEN: the D-24 hysteresis contract must survive across
    /// three consecutive REPL turns in the same session when the caller
    /// provides a shared Arc<PressureTracker> to attach_context_engine.
    #[tokio::test]
    async fn pressure_tracker_hysteresis_survives_across_repl_turns() {
        let config = band_config();
        let resolver = ProviderResolver::build(&config).unwrap();
        let session_id = "sess-repl-hysteresis";
        let tracker = Arc::new(PressureTracker::new());

        // ── Turn 1 ────────────────────────────────────────────────────────
        let mut agent1 = attach_context_engine(
            bare_agent(),
            &config,
            &resolver,
            session_id,
            None,
            Some(tracker.clone()),
        );
        let mut messages_1 = make_in_band_messages();
        agent1.pre_chat_compress(&mut messages_1).await;

        // After turn 1: warn fired exactly once, above_threshold=true,
        // pending_transient queued.
        assert_eq!(
            tracker.warn_count(session_id),
            1,
            "turn 1: warn must fire exactly once on first crossing"
        );
        assert!(
            tracker.was_warned(session_id),
            "turn 1: tracker must report above_threshold=true"
        );

        // ── Turn 2 ────────────────────────────────────────────────────────
        // Same tracker, fresh AgentLoop.  Still in band, no descent between
        // turns.  Expect: transient drained into messages, NO new warn fires.
        let mut agent2 = attach_context_engine(
            bare_agent(),
            &config,
            &resolver,
            session_id,
            None,
            Some(tracker.clone()),
        );
        let mut messages_2 = make_in_band_messages();
        let pre_len_2 = messages_2.len();
        agent2.pre_chat_compress(&mut messages_2).await;

        // Transient must be drained into turn 2's outbound message vector.
        assert!(
            messages_2.len() > pre_len_2,
            "turn 2 messages must gain the transient system message"
        );
        let transient_found = messages_2.iter().any(|m| {
            m.role == ironhermes_core::Role::System
                && m.content_text()
                    .is_some_and(|s| s.contains("CONTEXT PRESSURE HIGH"))
        });
        assert!(
            transient_found,
            "transient [CONTEXT PRESSURE HIGH ...] must reach turn 2's message vector"
        );
        // Warn count still 1 — hysteresis held across turn boundary.
        assert_eq!(
            tracker.warn_count(session_id),
            1,
            "turn 2: hysteresis must suppress re-fire (warn_count stays at 1)"
        );

        // ── Turn 3 ────────────────────────────────────────────────────────
        // Still in band, no descent.  Expect: NO new warn, no transient
        // (already consumed by take_transient on turn 2).
        let mut agent3 = attach_context_engine(
            bare_agent(),
            &config,
            &resolver,
            session_id,
            None,
            Some(tracker.clone()),
        );
        let mut messages_3 = make_in_band_messages();
        let pre_len_3 = messages_3.len();
        agent3.pre_chat_compress(&mut messages_3).await;

        assert_eq!(
            tracker.warn_count(session_id),
            1,
            "turn 3: hysteresis must still hold across 3 turns (warn_count stays at 1)"
        );
        // Turn 3 gets no transient (it was one-shot, consumed on turn 2).
        assert_eq!(
            messages_3.len(),
            pre_len_3,
            "turn 3 must NOT gain another transient (one-shot semantics)"
        );
    }
}
