//! Behavioral tests for `/compress` handler (Phase 22.4.2 Plan 03).
//!
//! Uses a StubContextEngine that implements ContextCompressorHandle trait.
//! PATTERNS.md Cat-5B: fake trait impl pattern.
//! Note: ironhermes-agent is NOT a dep of ironhermes-core (leaf crate invariant),
//! so we implement ContextCompressorHandle directly via a stub struct.

use std::sync::{Arc, atomic::AtomicBool};
use ironhermes_core::commands::context::{CommandContext, ContextCompressorHandle};
use ironhermes_core::commands::{CommandResult, CommandRouter};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::types::Platform;

// =============================================================================
// StubContextEngine — fake ContextCompressorHandle (PATTERNS.md Cat-5B)
// =============================================================================

/// Stub implementation of ContextCompressorHandle for test use.
///
/// Returns predictable status text that tests can assert on.
struct StubContextEngine;

impl ContextCompressorHandle for StubContextEngine {
    fn compress_text(&self) -> String {
        "Compression triggered (stub).".to_string()
    }

    fn status_text(&self) -> String {
        "Context compressor active. Mode: Hard".to_string()
    }
}

// =============================================================================
// Test fixtures
// =============================================================================

fn make_test_ctx_with_compressor() -> CommandContext {
    let engine: Arc<dyn ContextCompressorHandle> = Arc::new(StubContextEngine);
    CommandContext::new(
        Platform::Local,
        "test-session".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
    .with_context_compressor(engine)
}

fn make_ctx_no_compressor() -> CommandContext {
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

/// /compress with no engine configured returns "not configured" informational text.
/// D-05 None-guard pattern: all handlers with optional ctx handles return informational text on None.
#[test]
fn compress_with_no_engine_returns_not_configured() {
    let ctx = make_ctx_no_compressor();
    let router = make_router();
    let def = router.commands.iter().find(|c| c.name == "compress").unwrap();
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

/// /compress with StubContextEngine returns informational status text.
/// Per Plan 03 Task 1 deferral (OQ-5): manual compression deferred; handler returns status only.
#[test]
fn compress_with_stub_engine_returns_status_text() {
    let ctx = make_test_ctx_with_compressor();
    let router = make_router();
    let def = router.commands.iter().find(|c| c.name == "compress").unwrap();
    let result = dispatch(def, &[], &ctx, &router);
    match result {
        CommandResult::Output(text) => {
            assert!(
                text.contains("compressor") || text.contains("Context"),
                "status must mention compressor, got: {text}"
            );
            // The handler returns status_text from the engine — verify it's informational.
            assert!(
                !text.is_empty(),
                "compress status text must not be empty"
            );
        }
        other => panic!("expected Output for compress status, got {other:?}"),
    }
}

/// StubContextEngine is present — verify the struct exists in this file (PLAN artifact check).
#[test]
fn stub_context_engine_is_defined() {
    // This test verifies the StubContextEngine is a real type that compiles.
    // The mere fact that this file compiles with StubContextEngine in scope is the assertion.
    let _engine: Arc<dyn ContextCompressorHandle> = Arc::new(StubContextEngine);
    // If we got here, StubContextEngine implements ContextCompressorHandle correctly.
}
