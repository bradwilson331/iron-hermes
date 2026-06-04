//! Phase 36.17.7 Plan 01 — Wave 0 tests for per-turn TTS wiring (D-01).
//!
//! Tests 1–7 are source-grep + struct-construction tests; they compile RED until
//! Task 2 ships `TtsPerTurnWiring`, `TurnRequest::tts_wiring`, and the
//! `..Default::default()` rewrite in `agent_runtime.rs`.
//!
//! Test 8 (`tts_wiring_registers_tools_in_run_turn`) is a behavioral test that
//! uses `AgentRuntime::for_tests()` (gated `#[cfg(any(test, feature = "test-support"))]`
//! in `agent_runtime.rs:587`). It calls `run_turn` with `tts_wiring: Some(...)` and
//! asserts that `text_to_speech` + `send_audio` appear in the registry after the call.
//! `run_turn` may Err on the mock LLM endpoint (localhost:0) — the test only needs
//! the registry mutation, which happens before the LLM call.

use std::sync::Arc;

use ironhermes_agent::{TtsPerTurnWiring, TurnRequest};
use ironhermes_core::{Config, types::Platform, SessionKey};
use ironhermes_tools::{AudioDispatcher, NotSupportedAudioDispatcher};

// ── Test 1: TurnRequest::default() has no tts_wiring (regression guard) ──────

#[test]
fn turn_request_default_has_no_tts_wiring() {
    let req = TurnRequest::default();
    assert!(
        req.tts_wiring.is_none(),
        "TurnRequest::default() must have tts_wiring = None; \
         any caller using ..Default::default() must not accidentally get TTS wiring"
    );
}

// ── Test 2: TtsPerTurnWiring constructs with session_key + None dispatcher ───

#[test]
fn tts_per_turn_wiring_constructs_with_no_dispatcher() {
    let wiring = TtsPerTurnWiring {
        session_key: SessionKey::new(Platform::Local, "test"),
        audio_dispatcher: None,
    };
    assert!(
        wiring.audio_dispatcher.is_none(),
        "TtsPerTurnWiring with None dispatcher must keep audio_dispatcher = None (TUI case)"
    );
}

// ── Test 3: TtsPerTurnWiring constructs with Some(Arc<NotSupportedAudioDispatcher>) ─

#[test]
fn tts_per_turn_wiring_constructs_with_some_dispatcher() {
    let dispatcher: Arc<dyn AudioDispatcher> = Arc::new(NotSupportedAudioDispatcher::new("discord"));
    let wiring = TtsPerTurnWiring {
        session_key: SessionKey::new(Platform::Local, "test"),
        audio_dispatcher: Some(dispatcher),
    };
    assert!(
        wiring.audio_dispatcher.is_some(),
        "TtsPerTurnWiring with Some dispatcher must keep audio_dispatcher = Some (Discord/Slack case)"
    );
}

// ── Test 4: Source-grep — agent_runtime.rs declares tts_wiring field ─────────

#[test]
fn agent_runtime_source_declares_tts_wiring_field() {
    const SRC: &str = include_str!("../src/agent_runtime.rs");
    assert!(
        SRC.contains("pub tts_wiring: Option<TtsPerTurnWiring>"),
        "agent_runtime.rs must declare `pub tts_wiring: Option<TtsPerTurnWiring>` on TurnRequest"
    );
}

// ── Test 5: Source-grep — run_turn calls register_tts_tools + has if-let guard ─

#[test]
fn agent_runtime_source_calls_register_tts_tools_in_run_turn() {
    const SRC: &str = include_str!("../src/agent_runtime.rs");
    assert!(
        SRC.contains("register_tts_tools("),
        "agent_runtime.rs run_turn must call register_tts_tools()"
    );
    assert!(
        SRC.contains("if let Some(ref"),
        "agent_runtime.rs run_turn must guard the register call with `if let Some(ref wiring)`"
    );
}

// ── Test 6: Source-grep — from_config uses ..Default::default() (BLOCKER 1 lock) ─

#[test]
fn agent_runtime_source_uses_default_default_in_from_config() {
    const SRC: &str = include_str!("../src/agent_runtime.rs");
    assert!(
        SRC.contains("..Default::default()"),
        "agent_runtime.rs from_config must use `..Default::default()` to build AppRuntimeFactoryInput \
         (D-05 prep — eliminates the `session_key: None,` literal)"
    );
    assert!(
        !SRC.contains("session_key: None,"),
        "agent_runtime.rs must NOT contain the literal `session_key: None,` after Plan 01 Task 2 \
         (D-05 negative-assert: invariants_36_17_7.rs Test 1 targets this)"
    );
    assert!(
        !SRC.contains("telegram_adapter: None,"),
        "agent_runtime.rs must NOT contain the literal `telegram_adapter: None,` after Plan 01 Task 2 \
         (eliminated via ..Default::default())"
    );
}

// ── Test 7: Source-grep — app_runtime_factory.rs derives Default on AppRuntimeFactoryInput ─

#[test]
fn app_runtime_factory_input_derives_default() {
    const SRC: &str = include_str!("../src/app_runtime_factory.rs");
    assert!(
        SRC.contains("#[derive(Default)]") || SRC.contains("#[derive(Clone, Default)]") || SRC.contains("#[derive(Default, Clone)]"),
        "app_runtime_factory.rs must have #[derive(Default)] on AppRuntimeFactoryInput"
    );
    assert!(
        SRC.contains("pub struct AppRuntimeFactoryInput"),
        "app_runtime_factory.rs must still declare `pub struct AppRuntimeFactoryInput` (no rename)"
    );
}

// ── Test 8: Behavioral — register_tts_tools mutates the registry per the wiring contract ─
//
// **Path chosen: direct ToolRegistry call** (the fallback path explicitly allowed in
// Plan 01 Task 1 `<behavior>` block for Test 8).
//
// Rationale: `AgentRuntime::for_tests()` is gated `#[cfg(any(test, feature = "test-support"))]`.
// From an integration-test crate (`tests/*.rs`), the `test` cfg does NOT activate for the
// `ironhermes-agent` dep — integration tests link against the regular library build, so
// `for_tests` is invisible unless the `test-support` feature is enabled on the dev-dep.
// Adding `ironhermes-agent` as a self-dep with `features = ["test-support"]` would force
// every workspace consumer's test build to compile that code path. Direct ToolRegistry
// exercise is cleaner: it tests the exact mutation contract the per-turn block (Task 2)
// commits to, without spinning up a runtime.
//
// The wiring is verified end-to-end by:
//   - Source-grep Test 5 (`agent_runtime_source_calls_register_tts_tools_in_run_turn`)
//     which asserts the per-turn `register_tts_tools(` block exists in `run_turn`.
//   - Plan 02 UAT-T which proves an audible voice message arrives on Telegram after a
//     real `text_to_speech` + `send_audio` invocation through the live agent.

#[tokio::test]
async fn tts_wiring_registers_tools_in_run_turn() {
    use ironhermes_tools::ToolRegistry;

    // Build the same wiring shape that AgentRuntime::run_turn will receive on TurnRequest.
    let dispatcher: Arc<dyn AudioDispatcher> = Arc::new(NotSupportedAudioDispatcher::new("test"));
    let wiring = TtsPerTurnWiring {
        session_key: SessionKey::new(Platform::Local, "test"),
        audio_dispatcher: Some(dispatcher),
    };

    let mut registry = ToolRegistry::new();
    let config = Arc::new(Config::default());

    // Pre-condition: empty registry has no TTS tools.
    let pre = registry.list_tools();
    assert!(
        !pre.contains(&"text_to_speech"),
        "pre-condition: text_to_speech must NOT be in fresh registry; got: {pre:?}"
    );
    assert!(
        !pre.contains(&"send_audio"),
        "pre-condition: send_audio must NOT be in fresh registry; got: {pre:?}"
    );

    // Mirror the per-turn block that Task 2 ships in agent_runtime.rs::run_turn:
    //
    //   if let Some(ref wiring) = request.tts_wiring {
    //       let mut reg = self.bundle.registry.write().await;
    //       reg.register_tts_tools(
    //           wiring.session_key.clone(),
    //           wiring.audio_dispatcher.clone(),
    //           self.config.clone(),
    //       );
    //   }
    registry.register_tts_tools(
        wiring.session_key.clone(),
        wiring.audio_dispatcher.clone(),
        config,
    );

    // Behavioral assertion: both TTS tools now appear in the registry.
    let tools = registry.list_tools();
    assert!(
        tools.contains(&"text_to_speech"),
        "text_to_speech must be registered after register_tts_tools; got: {tools:?}"
    );
    assert!(
        tools.contains(&"send_audio"),
        "send_audio must be registered after register_tts_tools; got: {tools:?}"
    );
}
