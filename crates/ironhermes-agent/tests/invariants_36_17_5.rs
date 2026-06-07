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

/// Phase 36.17.5 → 36.17.7 transition: this test originally locked the D-15
/// deferral that `agent_runtime.rs::from_config` passed the literal
/// `session_key: None,` to `AppRuntimeFactoryInput`. Phase 36.17.7 Plan 01
/// (BLOCKER 1 fix for D-05) eliminates that literal by rewriting `from_config`
/// to use `..Default::default()` and threading per-turn TTS through
/// `TurnRequest::tts_wiring` instead. The forward-compat invariants now live in
/// `tests/tts_per_turn_wiring.rs` (Plan 01) and `tests/invariants_36_17_7.rs`
/// (Plan 05 Task 6). This test is repurposed to assert the rewrite is in place.
#[test]
fn agent_runtime_from_config_uses_default_default_residual() {
    const SRC: &str = include_str!("../src/agent_runtime.rs");
    assert!(
        SRC.contains("..Default::default()"),
        "Phase 36.17.7 Plan 01: AgentRuntime::from_config must build the startup \
         AppRuntimeFactoryInput via `..Default::default()` so the D-05 negative \
         assert (no `session_key: None,` literal) is structurally guaranteed."
    );
    assert!(
        !SRC.contains("session_key: None,"),
        "Phase 36.17.7 Plan 01 (BLOCKER 1 fix): the literal `session_key: None,` \
         must be absent from agent_runtime.rs — per-turn TTS now lives in \
         TurnRequest::tts_wiring (D-01)."
    );
}
