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
pub fn attach_context_engine(
    agent: AgentLoop,
    config: &Config,
    resolver: &ProviderResolver,
    session_id: impl Into<String>,
    hooks: Option<Arc<HookRegistry>>,
) -> AgentLoop {
    let sid = session_id.into();
    let tracker = Arc::new(PressureTracker::new());
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
        let agent = attach_context_engine(bare_agent(), &config, &resolver, "sess-1", None);
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
        let agent = attach_context_engine(bare_agent(), &config, &resolver, "sess-2", None);
        let t = agent.context_engine_threshold().unwrap();
        assert!((t - 0.42).abs() < 1e-4);
    }
}
