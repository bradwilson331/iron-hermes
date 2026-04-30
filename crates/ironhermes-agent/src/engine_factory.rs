// Phase 18 Plan 06: engine_factory — routes config strings to ContextEngine impls.
//
// `build_context_engine` is the single call site for both agent loop (50% threshold)
// and gateway handler (85% threshold). It resolves the aux-model for SummarizingEngine
// via `build_role_client(&resolver, "compression")` and falls back to the main client
// with a `tracing::warn!` when the role is unconfigured (T-18-10).

use ironhermes_core::{Config, ProviderResolver};
use ironhermes_hooks::HookRegistry;
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

use crate::any_client::{build_main_client, build_role_client};
use crate::context_engine::{ContextEngine, LocalPruningEngine};
use crate::memory::MemoryManager;
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
    memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,  // GAP-2: forward to engine before Arc wrap
) -> Arc<dyn ContextEngine> {
    let sid = session_id.into();
    let protect_first = config.compression.protect_first_n;
    let protect_last = config.compression.protect_last_tokens;
    let shift = config.compression.tool_pair_shift_tokens;

    let build_local = |hooks: Option<Arc<HookRegistry>>,
                       tracker: Option<Arc<PressureTracker>>,
                       memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
                       sid: &str|
     -> Arc<dyn ContextEngine> {
        // Phase 18-13 gap-closure: attach session_id unconditionally so the
        // PressureTracker can key its per-session hysteresis state even when
        // no hook registry is installed (CLI default path).  tracker and hooks
        // are each attached independently — the old combined guard
        // `if let (Some(h), Some(t))` short-circuited both when hooks was None.
        let mut e = LocalPruningEngine::new(context_length, threshold)
            .with_protect(protect_first, protect_last)
            .with_tool_pair_shift(shift)
            .with_session_id(sid);
        if let Some(t) = tracker {
            e = e.with_pressure_tracker(t);
        }
        if let Some(h) = hooks {
            e = e.with_hooks(h);
        }
        // GAP-2: apply memory_manager BEFORE Arc::new() — method is on the
        // concrete struct (LocalPruningEngine), not the ContextEngine trait.
        if let Some(mgr) = memory_manager {
            e = e.with_memory_manager(mgr);
        }
        Arc::new(e) as Arc<dyn ContextEngine>
    };

    match engine_kind {
        "local_prune" => build_local(hooks, tracker, memory_manager, &sid),

        "summarizing" => {
            // === Phase 26 D-05/D-07: auxiliary roles greenfield in this phase ===
            // The following roles are RESERVED in config but have no consumer call sites
            // in the agent crate as of Phase 26. When the downstream phase that ships
            // each tool wires its consumer, drop in the same three-branch
            // build_role_client(resolver, "<role>") pattern used for "compression" below.
            //
            // - "vision":         awaiting vision-complete tool (TBD phase)
            // - "session_search": no LLM routing needed today (pure StateStore FTS5 query
            //                     in session_search.rs); wire here if/when an LLM-assisted
            //                     session rewrite adds a model call (Phase 17 / 21.5 follow-up)
            // - "skills_hub":     awaiting skills hub auxiliary query path (TBD phase)
            // - "mcp_helper":     awaiting MCP helper auto-routing (TBD phase)
            //
            // Resolver-side cascade (Plan 02 D-05) is already in place — only the
            // consumer call sites are pending. resolve_role() will return Some(endpoint)
            // for any role with auxiliary or per-role config; None falls through to main.
            // === end Phase 26 ===

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
                            return build_local(hooks, tracker, memory_manager, &sid);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "compression role resolution failed, falling back to main client");
                    match build_main_client(resolver) {
                        Ok(c) => c,
                        Err(e2) => {
                            tracing::warn!(error = ?e2, "main client also failed, falling back to local_prune");
                            return build_local(hooks, tracker, memory_manager, &sid);
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

            // Phase 18-13 gap-closure: same three-branch independent attachment
            // as build_local — session_id unconditional, tracker/hooks each
            // gated on Some independently.
            let mut e = SummarizingEngine::new(context_length, threshold, summarizer)
                .with_protect(protect_first, protect_last)
                .with_tool_pair_shift(shift)
                .with_session_id(&sid);
            if let Some(t) = tracker {
                e = e.with_pressure_tracker(t);
            }
            if let Some(h) = hooks {
                e = e.with_hooks(h);
            }
            // GAP-2: apply memory_manager BEFORE Arc::new() — method is on the
            // concrete struct (SummarizingEngine), not the ContextEngine trait.
            if let Some(mgr) = memory_manager {
                e = e.with_memory_manager(mgr);
            }
            Arc::new(e)
        }

        other => {
            tracing::warn!(
                engine_kind = %other,
                "unknown context engine kind, falling back to local_prune"
            );
            build_local(hooks, tracker, memory_manager, &sid)
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
            None, // memory_manager: None (GAP-2 backward compat)
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
            None, // memory_manager: None (GAP-2 backward compat)
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
            None, // memory_manager: None (GAP-2 backward compat)
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
            None, // memory_manager: None (GAP-2 backward compat)
        );
        // Must fall back to Hard mode (LocalPruningEngine)
        assert_eq!(engine.mode(), CompressionMode::Hard);
    }

    // =========================================================================
    // Phase 26 Plan 03 regression tests — D-05 cascade via agent layer
    // =========================================================================

    /// Test 1: compression role routes through the auxiliary endpoint when only
    /// `auxiliary` is set (no per-task `model.roles.compression` override).
    ///
    /// Verifies Plan 02 D-05 cascade level 2 is reachable from the agent crate:
    /// `build_role_client(resolver, "compression")` → `Ok(Some(client))` whose
    /// underlying endpoint base_url reflects the `auxiliary.provider` (openai),
    /// not the main provider (openrouter).
    #[test]
    fn compression_cascade_uses_auxiliary_when_no_per_role_set() {
        use ironhermes_core::config::AuxiliaryConfig;

        let mut config = Config::default();
        // Main provider is "openrouter" by default.
        // Set auxiliary to openai — no per-role compression override.
        config.auxiliary = AuxiliaryConfig {
            provider: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
        };
        // Ensure model.roles has no "compression" entry (cascade level 1 must miss).
        config.model.roles.remove("compression");

        let resolver = ProviderResolver::build(&config).expect("resolver ok");

        // build_role_client should hit cascade level 2 (auxiliary) and return Some.
        let result = crate::any_client::build_role_client(&resolver, "compression")
            .expect("build_role_client must not error");

        assert!(
            result.is_some(),
            "compression role must resolve via auxiliary when auxiliary is set and no per-role override exists"
        );

        // Confirm the resolved endpoint is the openai one (base_url contains api.openai.com).
        let ep = resolver.resolve_role("compression").expect("resolve_role must return Some");
        assert!(
            ep.base_url.contains("api.openai.com"),
            "compression role must resolve to openai base_url via auxiliary cascade; got: {}",
            ep.base_url
        );
    }

    /// Test 2: when neither auxiliary nor a per-role compression entry is set,
    /// `build_role_client("compression")` returns `Ok(None)` — the factory's
    /// `Ok(None)` arm fires and falls back to the main client.
    ///
    /// This locks the D-07 caller pattern: the `Ok(None)` branch in the match on
    /// `build_role_client` triggers `build_main_client`, and the engine is still
    /// `SummarizingEngine` (Soft mode) — not LocalPruningEngine.
    #[test]
    fn compression_falls_back_to_main_when_no_aux_no_role() {
        let mut config = Config::default();
        // Ensure no auxiliary is set.
        config.auxiliary = ironhermes_core::config::AuxiliaryConfig::default(); // is_set() == false
        // Ensure no per-role compression override.
        config.model.roles.remove("compression");

        let resolver = ProviderResolver::build(&config).expect("resolver ok");

        // Cascade level 3: no per-role, no auxiliary → resolve_role returns None.
        let result = crate::any_client::build_role_client(&resolver, "compression")
            .expect("build_role_client must not error");
        assert!(
            result.is_none(),
            "compression role must return None when no aux and no per-role config set"
        );

        // The engine built by the factory must still be Soft mode (SummarizingEngine),
        // because the Ok(None) arm falls back to build_main_client (not local_prune).
        let engine = build_context_engine(
            &config,
            "summarizing",
            &resolver,
            128_000,
            0.5,
            "sess-test2",
            None,
            None,
            None,
        );
        assert_eq!(
            engine.mode(),
            CompressionMode::Soft,
            "factory must return SummarizingEngine (Soft) even when compression falls back to main"
        );
    }

    /// Test 3: `summarizing_engine.rs` must not read `auxiliary_model` directly.
    ///
    /// Phase 26 D-05 / RESEARCH.md confirmation: the summarizing engine receives
    /// `Arc<dyn SummarizationClient>` from the factory and does not read
    /// `config.auxiliary_model` literally. This is a permanent static regression gate
    /// (per Plan 03 task spec, comment-stripped per Grep Gate Hygiene).
    #[test]
    fn summarizing_engine_does_not_read_auxiliary_model_directly() {
        let src = include_str!("summarizing_engine.rs");
        // Strip single-line comments before checking — a historical comment mentioning
        // "auxiliary_model" should not trigger the gate; only production code occurrences.
        let cleaned: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !cleaned.contains("auxiliary_model"),
            "summarizing_engine.rs MUST NOT read auxiliary_model directly — \
             routing flows through the factory's build_role_client(resolver, role) chain (Phase 26 D-07)"
        );
    }
}
