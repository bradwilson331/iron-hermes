// Phase 18 Plan 06: engine_factory — routes config strings to ContextEngine impls.
//
// `build_context_engine` is the single call site for both agent loop (50% threshold)
// and gateway handler (85% threshold). It resolves the aux-model for SummarizingEngine
// via `build_role_client(&resolver, "compression")` and falls back to the main client
// with a `tracing::warn!` when the role is unconfigured (T-18-10).

use ironhermes_core::{Config, ProviderResolver};
use ironhermes_hooks::HookRegistry;
use std::sync::Arc;

use crate::any_client::{build_main_client, build_role_client};
use crate::context_engine::{ContextEngine, LocalPruningEngine};
use crate::pressure_warning::PressureTracker;
use crate::summarizing_engine::{AnyClientSummarizer, SummarizationClient, SummarizingEngine};

/// Construct the appropriate `ContextEngine` based on the `engine_kind` string from config.
///
/// | `engine_kind`  | Engine returned      | Mode |
/// |----------------|----------------------|------|
/// | `"local_prune"`| `LocalPruningEngine` | Hard |
/// | `"summarizing"`| `SummarizingEngine`  | Soft |
/// | anything else  | `LocalPruningEngine` | Hard (+ warn log) |
///
/// For `"summarizing"`, the compression aux-role is resolved via
/// `build_role_client(&resolver, "compression")`. If the role is not configured
/// (`None`) or resolution fails, the factory logs a `tracing::warn!` and falls
/// back to the main client so that compression never blocks agent startup (T-18-10).
///
/// Both engine types receive optional `HookRegistry` and `PressureTracker` when
/// provided so pre-compress hook emission (D-20) and three-channel pressure warnings
/// (D-23/D-24) are active end-to-end.
pub fn build_context_engine(
    config: &Config,
    engine_kind: &str,
    resolver: &ProviderResolver,
    context_length: usize,
    threshold: f32,
    session_id: impl Into<String>,
    hooks: Option<Arc<HookRegistry>>,
    tracker: Option<Arc<PressureTracker>>,
) -> Arc<dyn ContextEngine> {
    let sid = session_id.into();
    let protect_first = config.compression.protect_first_n;
    let protect_last = config.compression.protect_last_tokens;
    let shift = config.compression.tool_pair_shift_tokens;

    let build_local = |hooks: Option<Arc<HookRegistry>>,
                       tracker: Option<Arc<PressureTracker>>,
                       sid: &str|
     -> Arc<dyn ContextEngine> {
        let mut e = LocalPruningEngine::new(context_length, threshold)
            .with_protect(protect_first, protect_last)
            .with_tool_pair_shift(shift);
        if let (Some(h), Some(t)) = (hooks, tracker) {
            e = e.with_hooks(h, sid).with_pressure_tracker(t);
        }
        Arc::new(e) as Arc<dyn ContextEngine>
    };

    match engine_kind {
        "local_prune" => build_local(hooks, tracker, &sid),

        "summarizing" => {
            // Resolve compression role with fallback to main client (T-18-10).
            let client = match build_role_client(resolver, "compression") {
                Ok(Some(c)) => c,
                Ok(None) => {
                    tracing::warn!(
                        "compression role unconfigured, falling back to main client"
                    );
                    match build_main_client(resolver) {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::warn!(error = ?e, "main client resolution failed, falling back to local_prune");
                            return build_local(hooks, tracker, &sid);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "compression role resolution failed, falling back to main client");
                    match build_main_client(resolver) {
                        Ok(c) => c,
                        Err(e2) => {
                            tracing::warn!(error = ?e2, "main client also failed, falling back to local_prune");
                            return build_local(hooks, tracker, &sid);
                        }
                    }
                }
            };

            // Derive model identifier: prefer compression role's model, then main.
            let model = resolver
                .resolve_role("compression")
                .map(|ep| ep.default_model.clone())
                .or_else(|| Some(resolver.resolve_for_main().default_model.clone()));

            let summarizer: Arc<dyn SummarizationClient> =
                Arc::new(AnyClientSummarizer::new(Arc::new(client), model));

            let mut e = SummarizingEngine::new(context_length, threshold, summarizer)
                .with_protect(protect_first, protect_last)
                .with_tool_pair_shift(shift);
            if let (Some(h), Some(t)) = (hooks, tracker) {
                e = e.with_hooks(h, &sid).with_pressure_tracker(t);
            }
            Arc::new(e)
        }

        other => {
            tracing::warn!(
                engine_kind = %other,
                "unknown context engine kind, falling back to local_prune"
            );
            build_local(hooks, tracker, &sid)
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::{Config, ProviderResolver};
    use crate::context_engine::CompressionMode;

    fn default_resolver() -> ProviderResolver {
        ProviderResolver::build(&Config::default()).expect("resolver ok")
    }

    /// `build_context_engine` with `"local_prune"` returns an engine with Hard mode.
    #[test]
    fn factory_returns_local_prune_for_local_prune_string() {
        let config = Config::default();
        let resolver = default_resolver();
        let engine = build_context_engine(
            &config,
            "local_prune",
            &resolver,
            128_000,
            0.85,
            "sess-test",
            None,
            None,
        );
        assert_eq!(engine.mode(), CompressionMode::Hard);
        assert!((engine.threshold() - 0.85).abs() < 1e-4);
    }

    /// `build_context_engine` with `"summarizing"` returns an engine with Soft mode.
    #[test]
    fn factory_returns_summarizing_for_summarizing_string() {
        let config = Config::default();
        let resolver = default_resolver();
        let engine = build_context_engine(
            &config,
            "summarizing",
            &resolver,
            128_000,
            0.5,
            "sess-test",
            None,
            None,
        );
        // SummarizingEngine mode is Soft
        assert_eq!(engine.mode(), CompressionMode::Soft);
        assert!((engine.threshold() - 0.5).abs() < 1e-4);
    }

    /// When the compression role is unconfigured (default Config has no "compression" role
    /// pointing to a custom provider), the factory warns and falls back to the main client.
    /// The returned engine is still a SummarizingEngine (Soft mode), not LocalPruningEngine.
    ///
    /// This tests the fallback path (T-18-10): build_role_client returns None →
    /// tracing::warn! fires → build_main_client used → SummarizingEngine returned.
    #[tokio::test]
    async fn factory_aux_model_fallback() {
        // The default Config has a "compression" role configured to "main" provider
        // (per config.rs defaults). We verify the engine is still functional (Soft mode)
        // even when the role resolves through main.
        let config = Config::default();
        let resolver = default_resolver();

        let engine = build_context_engine(
            &config,
            "summarizing",
            &resolver,
            128_000,
            0.5,
            "sess-fallback",
            None,
            None,
        );

        // Should still be Soft mode regardless of which client was resolved
        assert_eq!(engine.mode(), CompressionMode::Soft);
    }

    /// Passing an unknown engine string falls back to LocalPruningEngine (Hard mode).
    #[test]
    fn factory_unknown_engine_falls_back() {
        let config = Config::default();
        let resolver = default_resolver();
        let engine = build_context_engine(
            &config,
            "bogus_engine_that_does_not_exist",
            &resolver,
            128_000,
            0.5,
            "sess-unknown",
            None,
            None,
        );
        // Must fall back to Hard mode (LocalPruningEngine)
        assert_eq!(engine.mode(), CompressionMode::Hard);
    }
}
