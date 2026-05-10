//! Behavioral tests for `/personality` handler (Phase 22.4.2 Plan 03).
//!
//! Uses a FakePersonalityRegistry that implements PersonalityHandle trait.
//! PATTERNS.md Cat-5A + Cat-5B: make_test_ctx_with_personality + fake trait impl.
//! Note: ironhermes-agent is NOT a dep of ironhermes-core (leaf crate invariant),
//! so we implement PersonalityHandle directly via a fake struct.

use ironhermes_core::commands::context::{CommandContext, PersonalityHandle};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandResult, CommandRouter};
use ironhermes_core::types::Platform;
use std::sync::{Arc, atomic::AtomicBool};

// =============================================================================
// Fake PersonalityHandle implementation (PATTERNS.md Cat-5B)
// =============================================================================

/// Fake personality registry with a small set of known presets.
struct FakePersonalityRegistry {
    presets: Vec<(&'static str, &'static str)>,
}

impl FakePersonalityRegistry {
    fn new() -> Self {
        Self {
            presets: vec![
                ("concise", "Be brief and to the point."),
                ("helpful", "Be warm, friendly, and thorough."),
                ("technical", "Use precise technical language."),
            ],
        }
    }
}

impl PersonalityHandle for FakePersonalityRegistry {
    fn get_preset(&self, name: &str) -> Option<String> {
        self.presets
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, text)| text.to_string())
    }

    fn list_presets(&self) -> Vec<String> {
        self.presets.iter().map(|(n, _)| n.to_string()).collect()
    }
}

// =============================================================================
// Test fixtures
// =============================================================================

fn make_test_ctx_with_personality() -> CommandContext {
    let registry: Arc<dyn PersonalityHandle> = Arc::new(FakePersonalityRegistry::new());
    CommandContext::new(
        Platform::Local,
        "test-session".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
    .with_personality_overlay(registry)
}

fn make_ctx_no_personality() -> CommandContext {
    CommandContext::new(
        Platform::Local,
        "test-session".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
}

fn make_router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

// =============================================================================
// Tests
// =============================================================================

/// /personality with no args and registry wired returns list of presets.
#[test]
fn personality_list_mode_returns_preset_names() {
    let ctx = make_test_ctx_with_personality();
    let router = make_router();
    let def = router
        .commands
        .iter()
        .find(|c| c.name == "personality")
        .unwrap();
    let result = dispatch(def, &[], &ctx, &router);
    match result {
        CommandResult::Output(text) => {
            assert!(
                text.contains("Available") || text.contains("concise"),
                "list mode must show preset names, got: {text}"
            );
        }
        other => panic!("expected Output for list mode, got {other:?}"),
    }
}

/// /personality with a known preset name returns overlay text.
#[test]
fn personality_apply_known_preset_returns_overlay_text() {
    let ctx = make_test_ctx_with_personality();
    let router = make_router();
    let def = router
        .commands
        .iter()
        .find(|c| c.name == "personality")
        .unwrap();
    let result = dispatch(def, &["concise"], &ctx, &router);
    match result {
        CommandResult::Output(text) => {
            assert!(!text.is_empty(), "overlay text must not be empty");
            assert!(
                text.contains("brief") || text.contains("point"),
                "overlay text must be the preset content, got: {text}"
            );
        }
        other => panic!("expected Output for known preset, got {other:?}"),
    }
}

/// /personality with unknown preset name returns Error.
#[test]
fn personality_apply_unknown_returns_error() {
    let ctx = make_test_ctx_with_personality();
    let router = make_router();
    let def = router
        .commands
        .iter()
        .find(|c| c.name == "personality")
        .unwrap();
    let result = dispatch(def, &["nonexistent_preset_xyz"], &ctx, &router);
    match result {
        CommandResult::Error(msg) => {
            assert!(
                msg.contains("Unknown personality"),
                "error must mention 'Unknown personality', got: {msg}"
            );
        }
        other => panic!("expected Error for unknown preset, got {other:?}"),
    }
}

/// /personality with no registry returns "not configured" informational text.
#[test]
fn personality_no_registry_returns_not_configured() {
    let ctx = make_ctx_no_personality();
    let router = make_router();
    let def = router
        .commands
        .iter()
        .find(|c| c.name == "personality")
        .unwrap();
    let result = dispatch(def, &[], &ctx, &router);
    match result {
        CommandResult::Output(text) => {
            assert!(
                text.contains("not configured"),
                "None-guard must return 'not configured', got: {text}"
            );
        }
        other => panic!("expected Output (not configured), got {other:?}"),
    }
}

/// Phase 21.8.3.1: /personality clear dispatched to core returns Error("Unknown personality: clear").
/// Documents the contract: "clear" is NOT handled at core level.
/// Surface-level handlers (TUI handle_subsystem_mutator pre-check; gateway pre-dispatch
/// in handler.rs) intercept "clear" before it reaches cmd_personality.
/// This test guards against future refactors that might add "clear" to core
/// without coordinating with both surface-level pre-checks.
#[test]
fn personality_clear_at_core_returns_unknown_error() {
    let ctx = make_test_ctx_with_personality();
    let router = make_router();
    let def = router
        .commands
        .iter()
        .find(|c| c.name == "personality")
        .unwrap();
    let result = dispatch(def, &["clear"], &ctx, &router);
    match result {
        CommandResult::Error(msg) => {
            assert!(
                msg.contains("Unknown personality") || msg.contains("clear"),
                "core must not know about 'clear'; expected error mentioning 'Unknown personality' or 'clear', got: {msg}"
            );
        }
        other => panic!(
            "expected Error when 'clear' reaches core (surface handlers must intercept first), got {other:?}"
        ),
    }
}
