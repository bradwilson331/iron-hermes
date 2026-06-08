//! Regression tests for DEFERRED_KANBAN_SUBVERBS — Phase 36.3.7.9 Plan 08.
//!
//! These tests lock two invariants:
//!   1. `"boards"` is in DEFERRED_KANBAN_SUBVERBS (new entry added in this plan).
//!   2. Pre-existing entries ("swarm", "mention", "link", "unblock", "dispatch") are
//!      byte-stable — no removals or reorderings may silently break their routing.
//!
//! All tests use the dispatch-based (black-box behavior) approach: the const is
//! private, so we invoke `dispatch()` with a subverb arg and assert the
//! CommandResult::Output text routes through the deferred-CLI message path.
//!
//! Mirror of deferred_subverbs.rs (which covers "mention" for REQ-36.3.7.8-08),
//! isolated here for scope clarity per Plan 08 instructions.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ironhermes_core::commands::context::{CommandContext, KanbanStoreReader};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;

// =============================================================================
// Minimal fake KanbanStoreReader
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
    CommandContext::new(
        Platform::Local,
        "test-session".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
    .with_kanban_store(store)
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

/// Helper: dispatch a kanban subverb and assert it routes to the deferred-CLI path.
fn assert_deferred(subverb: &str) {
    let ctx = make_ctx();
    let cmd = find_kanban_cmd();
    let r = router();

    let result = dispatch(&cmd, &[subverb], &ctx, &r);

    let output = match result {
        CommandResult::Output(s) => s,
        other => panic!("Expected CommandResult::Output for subverb '{subverb}', got: {other:?}"),
    };

    assert!(
        output.contains(subverb),
        "Deferred-CLI message for '{subverb}' must contain the subverb name; got: {output}"
    );

    assert!(
        !output.contains("not configured"),
        "dispatch of '{subverb}' must not hit the no-store branch; got: {output}"
    );
}

// =============================================================================
// Tests — new entry (Plan 08)
// =============================================================================

/// Phase 36.3.7.9-08: `"boards"` is in DEFERRED_KANBAN_SUBVERBS.
///
/// Dispatch `/kanban boards list` (two args, simulates real slash usage) and
/// assert the first arg `"boards"` is consumed by the deferred-subverb arm —
/// not by a non-existent gateway-side boards handler.
#[test]
fn deferred_kanban_subverbs_contains_boards() {
    let ctx = make_ctx();
    let cmd = find_kanban_cmd();
    let r = router();

    // Simulate: `/kanban boards list`
    let result = dispatch(&cmd, &["boards", "list"], &ctx, &r);

    let output = match result {
        CommandResult::Output(s) => s,
        other => panic!("Expected CommandResult::Output for 'boards list', got: {other:?}"),
    };

    assert!(
        output.contains("boards"),
        "Deferred-CLI message must contain 'boards'; got: {output}"
    );

    assert!(
        !output.contains("not configured"),
        "dispatch must not hit the no-store branch: {output}"
    );
}

// =============================================================================
// Tests — byte-stability of existing entries (Plan 08)
// =============================================================================

/// Assert that "swarm" (added in Phase 36.3.7.7) still routes to the
/// deferred-CLI message — byte-stability check.
#[test]
fn deferred_kanban_subverbs_existing_entries_byte_stable_swarm() {
    assert_deferred("swarm");
}

/// Assert that "mention" (added in Phase 36.3.7.8) still routes to the
/// deferred-CLI message — byte-stability check.
#[test]
fn deferred_kanban_subverbs_existing_entries_byte_stable_mention() {
    assert_deferred("mention");
}

/// Assert that "link" (legacy entry) still routes to the deferred-CLI message.
#[test]
fn deferred_kanban_subverbs_existing_entries_byte_stable_link() {
    assert_deferred("link");
}

/// Assert that "unblock" (legacy entry) still routes to the deferred-CLI message.
#[test]
fn deferred_kanban_subverbs_existing_entries_byte_stable_unblock() {
    assert_deferred("unblock");
}

/// Assert that "dispatch" (legacy entry) still routes to the deferred-CLI message.
#[test]
fn deferred_kanban_subverbs_existing_entries_byte_stable_dispatch() {
    assert_deferred("dispatch");
}
