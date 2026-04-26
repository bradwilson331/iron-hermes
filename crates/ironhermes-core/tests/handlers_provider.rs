//! Phase 22.4.2 Plan 02 — behavioral tests for ProviderResolver-backed handlers.
//!
//! Tests /model, /provider, /fast via the real
//! `ironhermes_core::commands::handlers::dispatch` entry point.
//!
//! Per CONTEXT.md D-10: placed in `ironhermes-core/tests/` because
//! `ProviderResolverHandle` is defined in ironhermes-core and there is no
//! circular dependency concern (unlike StateStore tests which must live in
//! ironhermes-cli/tests/ per RESEARCH OQ-7 — ironhermes-state imports core).
//!
//! Fixture pattern mirrors `make_ctx` (handlers.rs) and the fake trait-object
//! pattern from `cmd_agents_and_stop.rs`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ironhermes_core::commands::context::{CommandContext, ProviderResolverHandle};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;

// =============================================================================
// Fake ProviderResolverHandle implementations
// =============================================================================

/// A ProviderResolverHandle fake with a configurable fast-role model.
struct FakeResolver {
    main_provider: String,
    main_model: String,
    /// If Some, fast_role_model() returns this model name.
    fast_model: Option<String>,
    /// If set to an error string, validate_model returns Err for any input.
    validate_error: Option<String>,
}

impl FakeResolver {
    fn new(main_provider: &str, main_model: &str, fast_model: Option<&str>) -> Self {
        Self {
            main_provider: main_provider.to_string(),
            main_model: main_model.to_string(),
            fast_model: fast_model.map(|s| s.to_string()),
            validate_error: None,
        }
    }

    fn with_validate_error(mut self, err: &str) -> Self {
        self.validate_error = Some(err.to_string());
        self
    }
}

impl ProviderResolverHandle for FakeResolver {
    fn main_provider(&self) -> String {
        self.main_provider.clone()
    }

    fn main_model(&self) -> String {
        self.main_model.clone()
    }

    fn status_text(&self) -> String {
        // V8.1: must NOT include api_key
        format!(
            "Provider: {}\nModel: {}",
            self.main_provider, self.main_model
        )
    }

    fn validate_model(&self, model: &str) -> Result<String, String> {
        if let Some(ref err) = self.validate_error {
            return Err(err.clone());
        }
        // Accept any non-empty model string as valid
        if model.is_empty() {
            Err("Model name must not be empty.".to_string())
        } else {
            Ok(model.to_string())
        }
    }

    fn model_list_text(&self) -> String {
        format!("Available models:\n  {}", self.main_model)
    }

    fn fast_role_model(&self) -> Option<String> {
        self.fast_model.clone()
    }
}

// =============================================================================
// Fixture helpers
// =============================================================================

fn make_router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

fn find_cmd(name: &str) -> CommandDef {
    build_registry()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("Command '{}' not found in registry", name))
}

/// Minimal context with no provider_resolver wired.
fn make_ctx_no_resolver() -> CommandContext {
    CommandContext::new(
        Platform::Local,
        "test-session-provider".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
}

/// Context with a FakeResolver wired (fast_model configurable).
fn make_test_ctx_with_provider_resolver(fast_model: Option<&str>) -> CommandContext {
    let resolver: Arc<dyn ProviderResolverHandle> = Arc::new(FakeResolver::new(
        "openai",
        "gpt-4o",
        fast_model,
    ));
    CommandContext::new(
        Platform::Local,
        "test-session-provider".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
    .with_provider_resolver(resolver)
}

// =============================================================================
// /model tests
// =============================================================================

/// D-05 guard: when ctx.provider_resolver is None, /model returns informational text.
#[test]
fn cmd_model_none_guard_returns_not_configured() {
    let ctx = make_ctx_no_resolver();
    let router = make_router();
    let cmd = find_cmd("model");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("not configured"),
            "Expected 'not configured' message when provider_resolver is None, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

/// With resolver wired and no args, /model lists available models.
#[test]
fn cmd_model_no_args_lists_models() {
    let ctx = make_test_ctx_with_provider_resolver(None);
    let router = make_router();
    let cmd = find_cmd("model");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("Available models") || s.contains("gpt-4o"),
            "Expected model listing, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

/// With resolver wired and a valid model arg, /model returns selection confirmation.
#[test]
fn cmd_model_valid_arg_returns_confirmation() {
    let ctx = make_test_ctx_with_provider_resolver(None);
    let router = make_router();
    let cmd = find_cmd("model");
    let result = dispatch(&cmd, &["claude-3-haiku"], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("Selected model") || s.contains("claude-3-haiku"),
            "Expected selection confirmation, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

/// With resolver wired and an invalid model arg, /model returns an error.
#[test]
fn cmd_model_invalid_arg_returns_error() {
    let resolver: Arc<dyn ProviderResolverHandle> = Arc::new(
        FakeResolver::new("openai", "gpt-4o", None)
            .with_validate_error("Unknown model: nonexistent-model"),
    );
    let ctx = CommandContext::new(
        Platform::Local,
        "test-session-provider".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
    .with_provider_resolver(resolver);
    let router = make_router();
    let cmd = find_cmd("model");
    let result = dispatch(&cmd, &["nonexistent-model"], &ctx, &router);
    assert!(
        matches!(result, CommandResult::Error(_)),
        "Expected Error for invalid model, got {:?}",
        result
    );
}

// =============================================================================
// /provider tests
// =============================================================================

/// D-05 guard: when ctx.provider_resolver is None, /provider returns informational text.
#[test]
fn cmd_provider_none_guard_returns_not_configured() {
    let ctx = make_ctx_no_resolver();
    let router = make_router();
    let cmd = find_cmd("provider");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("not configured"),
            "Expected 'not configured' message when provider_resolver is None, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

/// With resolver wired, /provider returns status including provider name.
#[test]
fn cmd_provider_wired_returns_status() {
    let ctx = make_test_ctx_with_provider_resolver(None);
    let router = make_router();
    let cmd = find_cmd("provider");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => {
            assert!(
                s.contains("openai") || s.contains("Provider"),
                "Expected provider status text, got: {s}"
            );
            // V8.1: must NOT include api_key
            assert!(
                !s.to_lowercase().contains("api_key") && !s.to_lowercase().contains("apikey"),
                "INV V8.1: /provider output must NOT include api_key, got: {s}"
            );
        }
        other => panic!("Expected Output, got {:?}", other),
    }
}

/// With resolver wired, /provider status includes current model.
#[test]
fn cmd_provider_wired_includes_model() {
    let ctx = make_test_ctx_with_provider_resolver(None);
    let router = make_router();
    let cmd = find_cmd("provider");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("gpt-4o") || s.contains("Model"),
            "Expected model in provider status, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

// =============================================================================
// /fast tests
// =============================================================================

/// D-05 guard: when ctx.provider_resolver is None, /fast returns informational text.
#[test]
fn cmd_fast_none_guard_returns_not_configured() {
    let ctx = make_ctx_no_resolver();
    let router = make_router();
    let cmd = find_cmd("fast");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("not configured"),
            "Expected 'not configured' message when provider_resolver is None, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

/// With resolver wired and a fast model configured, /fast returns the model name.
#[test]
fn cmd_fast_wired_with_fast_model_returns_model_name() {
    let ctx = make_test_ctx_with_provider_resolver(Some("gpt-4o-mini"));
    let router = make_router();
    let cmd = find_cmd("fast");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("gpt-4o-mini"),
            "Expected fast model name in output, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

/// With resolver wired but no fast model configured, /fast says fast role not configured.
#[test]
fn cmd_fast_wired_no_fast_model_returns_not_configured_message() {
    let ctx = make_test_ctx_with_provider_resolver(None);
    let router = make_router();
    let cmd = find_cmd("fast");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.contains("not configured") || s.contains("no fast"),
            "Expected 'not configured' or 'no fast' message when no fast preset, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}

/// /fast output mentions 'fast mode' or 'Fast mode' to orient the user.
#[test]
fn cmd_fast_wired_output_mentions_fast_mode() {
    let ctx = make_test_ctx_with_provider_resolver(Some("claude-haiku-3"));
    let router = make_router();
    let cmd = find_cmd("fast");
    let result = dispatch(&cmd, &[], &ctx, &router);
    match result {
        CommandResult::Output(s) => assert!(
            s.to_lowercase().contains("fast"),
            "Expected 'fast' in output text, got: {s}"
        ),
        other => panic!("Expected Output, got {:?}", other),
    }
}
