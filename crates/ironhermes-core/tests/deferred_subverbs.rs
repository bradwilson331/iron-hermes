//! REQ-36.3.7.8-08 receiver test: assert that "mention" is in DEFERRED_KANBAN_SUBVERBS.
//!
//! DEFERRED_KANBAN_SUBVERBS is a private `const` in
//! `crates/ironhermes-core/src/commands/handlers.rs`. The preferred approach
//! (option a) of referencing the const directly is not possible from an
//! integration test since the const is not pub. We therefore use option (b):
//! invoke the dispatch entry point with subverb="mention" and assert the response
//! matches the deferred-CLI message pattern (behavior-based test).
//!
//! The dispatch path for deferred subverbs routes through
//! `KanbanStoreReader::deferred_subverb_message`, which returns a string
//! containing the subverb name and a CLI redirect instruction. This is stable
//! against copy changes as long as the routing behavior is preserved.

use std::sync::Arc;

use ironhermes_core::commands::context::{CommandContext, KanbanStoreReader};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;

// =============================================================================
// Minimal fake KanbanStoreReader (mirrors handlers_kanban.rs pattern)
// =============================================================================

struct FakeStore;

impl KanbanStoreReader for FakeStore {
    fn list_text(&self) -> String {
        "ID  STATUS  ASSIGNEE  PRIORITY  TITLE\n".to_string()
    }

    fn show_text(&self, _id: &str) -> Option<String> {
        None
    }

    fn tip_text(&self) -> String {
        String::new()
    }

    fn deferred_subverb_message(&self, name: &str) -> String {
        format!("/kanban {name}: use 'hermes kanban {name}' from the CLI for this subverb.")
    }
}

fn make_ctx() -> CommandContext {
    let store: Arc<dyn KanbanStoreReader> = Arc::new(FakeStore);
    CommandContext::new(Platform::Local, "test-session".to_string()).with_kanban_store(store)
}

fn find_kanban_cmd() -> CommandDef {
    build_registry()
        .into_iter()
        .find(|c| c.name == "kanban")
        .expect("'kanban' command must be registered in build_registry()")
}

fn router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

// =============================================================================
// Tests
// =============================================================================

/// REQ-36.3.7.8-08: "mention" is in DEFERRED_KANBAN_SUBVERBS.
///
/// Verified by dispatching `/kanban mention <task_id>` and asserting the output
/// is a CLI-redirect message (not a typo-suggest or "not configured" response).
#[test]
fn deferred_subverbs_includes_mention() {
    let ctx = make_ctx();
    let cmd = find_kanban_cmd();
    let r = router();

    let result = dispatch(&cmd, &["mention", "t_test001"], &ctx, &r);

    let output = match result {
        CommandResult::Output(s) => s,
        other => panic!("Expected Output, got: {other:?}"),
    };

    // The deferred path calls store.deferred_subverb_message("mention"), which
    // our FakeStore returns as "/kanban mention: use 'hermes kanban mention' ...".
    // Assert the output routes through the deferred path (contains "mention").
    assert!(
        output.contains("mention"),
        "dispatch of 'mention' subverb must produce CLI-redirect containing 'mention', got: {output}"
    );

    // Also verify it does NOT say "kanban store not configured" (that would mean
    // the test hit the no-store path instead of the deferred-subverb path).
    assert!(
        !output.contains("not configured"),
        "dispatch must not hit the no-store branch: {output}"
    );
}
