//! Phase 36.3.7.0 Plan 02 (BUG-36.3.7-02) — receiver-end dispatch-chain test
//! for the `/kanban` slash command handler using a minimal fake KanbanStoreReader
//! trait-object impl.
//!
//! Mirrors `handlers_cron.rs` exactly: driven through `dispatch(...)` so the
//! slash-command routing, the registry match arm, and the `cmd_kanban` handler
//! body are all exercised end-to-end through the trait-object boundary.
//!
//! These tests prove the receiver-end gap that caused BUG-36.3.7-02 is closed:
//! `/kanban list` now routes to `cmd_kanban` (NOT `todo_stub`).

use std::sync::Arc;

use ironhermes_core::commands::context::{CommandContext, KanbanStoreReader};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;

// =============================================================================
// Fake KanbanStoreReader implementation (in-memory, no DB required)
// =============================================================================

struct FakeKanbanStoreReader;

impl KanbanStoreReader for FakeKanbanStoreReader {
    fn list_text(&self) -> String {
        // Must include STATUS and ASSIGNEE substrings to match production format_task_table contract.
        "ID                STATUS    ASSIGNEE         PRIORITY  TITLE\n\
         t_test01          ready     alice            0         test task\n"
            .to_string()
    }

    fn show_text(&self, id: &str) -> Option<String> {
        if id == "t_known" {
            Some(
                "=== TASK: t_known ===\n\
                 TITLE:    Known Task\n\
                 STATUS:   ready\n\
                 ASSIGNEE: alice\n\
                 PRIORITY: 0\n"
                    .to_string(),
            )
        } else {
            None
        }
    }

    fn tip_text(&self) -> String {
        "Scratch workspaces live at ~/.ironhermes/kanban/workspaces/<task_id>/\n\
         Use --workspace dir:/abs/path or --workspace worktree on ironhermes kanban create.\n"
            .to_string()
    }

    fn deferred_subverb_message(&self, name: &str) -> String {
        format!(
            "/kanban {}: use 'ironhermes kanban {}' from the CLI for v1.",
            name, name
        )
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn make_ctx_with_store() -> CommandContext {
    let store: Arc<dyn KanbanStoreReader> = Arc::new(FakeKanbanStoreReader);
    CommandContext::new(Platform::Local, "test-session".to_string()).with_kanban_store(store)
}

fn make_ctx_without_store() -> CommandContext {
    CommandContext::new(Platform::Local, "test-session".to_string())
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

/// Test 1: /kanban list returns table format with STATUS and ASSIGNEE columns.
/// Proves dispatch reaches cmd_kanban → list_text (NOT todo_stub "not yet available").
#[test]
fn dispatch_kanban_list_returns_table_format() {
    let ctx = make_ctx_with_store();
    let cmd = find_kanban_cmd();
    let r = router();

    let res = dispatch(&cmd, &["list"], &ctx, &r);

    match res {
        CommandResult::Output(s) => {
            assert!(
                s.contains("STATUS"),
                "list output must contain 'STATUS' column header; got: {}",
                s
            );
            assert!(
                s.contains("ASSIGNEE"),
                "list output must contain 'ASSIGNEE' column header; got: {}",
                s
            );
            assert!(
                !s.contains("not yet available"),
                "list output must NOT contain 'not yet available' (todo_stub); got: {}",
                s
            );
        }
        other => panic!(
            "expected CommandResult::Output for /kanban list, got {:?}",
            other
        ),
    }
}

/// Test 2: /kanban show returns detail view with STATUS: and ASSIGNEE: labels.
#[test]
fn dispatch_kanban_show_returns_detail() {
    let ctx = make_ctx_with_store();
    let cmd = find_kanban_cmd();
    let r = router();

    let res = dispatch(&cmd, &["show", "t_known"], &ctx, &r);

    match res {
        CommandResult::Output(s) => {
            assert!(
                s.contains("STATUS:"),
                "show output must contain 'STATUS:' label; got: {}",
                s
            );
            assert!(
                s.contains("ASSIGNEE:"),
                "show output must contain 'ASSIGNEE:' label; got: {}",
                s
            );
        }
        other => panic!(
            "expected CommandResult::Output for /kanban show t_known, got {:?}",
            other
        ),
    }
}

/// Test 3: /kanban show without an id returns Error with "missing id".
#[test]
fn dispatch_kanban_show_missing_id_returns_error() {
    let ctx = make_ctx_with_store();
    let cmd = find_kanban_cmd();
    let r = router();

    let res = dispatch(&cmd, &["show"], &ctx, &r);

    match res {
        CommandResult::Error(msg) => {
            assert!(
                msg.contains("missing id"),
                "error must contain 'missing id'; got: {}",
                msg
            );
        }
        other => panic!(
            "expected CommandResult::Error for /kanban show (no id), got {:?}",
            other
        ),
    }
}

/// Test 4: /kanban tip returns tip text containing "Scratch".
#[test]
fn dispatch_kanban_tip_returns_tip_text() {
    let ctx = make_ctx_with_store();
    let cmd = find_kanban_cmd();
    let r = router();

    let res = dispatch(&cmd, &["tip"], &ctx, &r);

    match res {
        CommandResult::Output(s) => {
            assert!(
                s.contains("Scratch"),
                "tip output must contain 'Scratch' (scratch workspace reference); got: {}",
                s
            );
        }
        other => panic!(
            "expected CommandResult::Output for /kanban tip, got {:?}",
            other
        ),
    }
}

/// Test 5: /kanban claim (deferred subverb) routes to CLI-redirect message,
/// NOT to todo_stub "not yet available".
#[test]
fn dispatch_kanban_deferred_subverb_routes_to_cli_message() {
    let ctx = make_ctx_with_store();
    let cmd = find_kanban_cmd();
    let r = router();

    let res = dispatch(&cmd, &["claim"], &ctx, &r);

    match res {
        CommandResult::Output(s) => {
            assert!(
                s.contains("use 'ironhermes kanban claim'"),
                "deferred subverb must redirect to CLI; got: {}",
                s
            );
            assert!(
                !s.contains("not yet available"),
                "deferred subverb must NOT say 'not yet available' (todo_stub); got: {}",
                s
            );
        }
        other => panic!(
            "expected CommandResult::Output for /kanban claim (deferred), got {:?}",
            other
        ),
    }
}

/// Test 6: /kanban <typo> returns Error with "Unknown /kanban subcommand".
/// Typo-suggest should suggest "list" for "lst".
#[test]
fn dispatch_kanban_unknown_subverb_typo_suggests() {
    let ctx = make_ctx_with_store();
    let cmd = find_kanban_cmd();
    let r = router();

    let res = dispatch(&cmd, &["lst"], &ctx, &r);

    match res {
        CommandResult::Error(msg) => {
            assert!(
                msg.contains("Unknown /kanban subcommand: lst"),
                "error must name the unknown subcommand; got: {}",
                msg
            );
            // Typo-suggest is best-effort; "list" is the closest candidate.
            assert!(
                msg.contains("list"),
                "error should suggest 'list' as typo correction for 'lst'; got: {}",
                msg
            );
        }
        other => panic!(
            "expected CommandResult::Error for /kanban lst (unknown subcommand), got {:?}",
            other
        ),
    }
}

/// Test 7: /kanban list with no store returns "kanban store not configured".
/// Proves the None-fallback matches cmd_cron line 1064 behavior.
#[test]
fn dispatch_kanban_without_store_returns_not_configured() {
    let ctx = make_ctx_without_store();
    let cmd = find_kanban_cmd();
    let r = router();

    let res = dispatch(&cmd, &["list"], &ctx, &r);

    match res {
        CommandResult::Output(s) => {
            assert!(
                s.contains("kanban store not configured"),
                "without store, output must say 'kanban store not configured'; got: {}",
                s
            );
        }
        other => panic!(
            "expected CommandResult::Output for /kanban list (no store), got {:?}",
            other
        ),
    }
}

/// Static grep: verify the dispatch arm is present in handlers.rs source.
/// This locks the pattern against future refactors that might accidentally remove it.
#[test]
fn static_grep_kanban_dispatch_arm_present() {
    let handlers_src = include_str!("../src/commands/handlers.rs");
    assert!(
        handlers_src.contains("\"kanban\" => cmd_kanban(args, ctx)"),
        "handlers.rs must contain '\"kanban\" => cmd_kanban(args, ctx)' dispatch arm"
    );
}
