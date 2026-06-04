//! Phase 36.17.5 — locks the build_app_runtime_bundle TTS wiring.
//!
//! These are source-text invariant tests: they assert structural properties of
//! the factory file rather than runtime behaviour. This pattern follows the
//! existing invariants_*.rs tests in this crate (budget_ordering_grep.rs etc).

/// Lock that register_tts_tools is called inside build_app_runtime_bundle
/// and that AppRuntimeFactoryInput carries the two new additive fields.
#[test]
fn build_app_runtime_bundle_source_calls_register_tts_tools() {
    const SRC: &str = include_str!("../src/app_runtime_factory.rs");
    assert!(
        SRC.contains("registry.register_tts_tools("),
        "Phase 36.17.5: build_app_runtime_bundle must invoke register_tts_tools \
         (look near the register_cronjob_tool call site at line ~89)"
    );
    assert!(
        SRC.contains("session_key: Option<ironhermes_core::SessionKey>"),
        "Phase 36.17.5: AppRuntimeFactoryInput must carry session_key field"
    );
    assert!(
        SRC.contains("telegram_adapter: Option<Arc<dyn ironhermes_tools::AudioDispatcher>>"),
        "Phase 36.17.5: AppRuntimeFactoryInput must carry telegram_adapter field"
    );
}

/// Lock that session_key is guarded — register_tts_tools only fires when Some.
#[test]
fn build_app_runtime_bundle_source_guards_session_key() {
    const SRC: &str = include_str!("../src/app_runtime_factory.rs");
    assert!(
        SRC.contains("if let Some(ref session_key) = input.session_key"),
        "Phase 36.17.5: register_tts_tools must be guarded by \
         `if let Some(ref session_key) = input.session_key`"
    );
}

/// Lock that agent_runtime.rs passes session_key: None for v1.
/// Per-turn threading deferred to a follow-up phase.
#[test]
fn agent_runtime_passes_session_key_none_for_v1() {
    const SRC: &str = include_str!("../src/agent_runtime.rs");
    assert!(
        SRC.contains("session_key: None") || SRC.contains("session_key:None"),
        "Phase 36.17.5: AgentRuntime::from_config must default session_key to None \
         for v1 — per-turn threading lands in a follow-up phase"
    );
}
