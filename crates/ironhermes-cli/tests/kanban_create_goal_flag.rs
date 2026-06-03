//! Phase 36.3.7.12 Plan 03 Task 1 — CLI parity smoke tests for the new
//! `--goal` / `--goal-max-turns` flags on `hermes kanban create` (D-01).
//!
//! These tests parse argv through a local `TestCli` wrapper using clap-derive
//! to verify that `KanbanCommands::Create` carries the two new fields with the
//! correct shape (D-01: `goal: bool` defaults `false`; `goal_max_turns: u32`
//! defaults `20` via `default_value_t`). They do NOT exercise `cmd_create`
//! end-to-end against a live DB — that path requires a tempdir-scoped
//! HERMES_HOME and is covered by the kanban-crate producer tests
//! (`tests/goal_mode_persistence.rs` from Plan 01).
//!
//! NOTE: `KanbanCommands` does NOT derive `Debug` (Phase 36.3.7 baseline; not
//! changed in this phase). So we cannot derive Debug on TestSub either — the
//! `{other:?}` panic-message form is unavailable. Tests use plain string panics
//! if the match arm is wrong (same convention as `heartbeat_cli.rs`,
//! `mention_cli.rs`, `decompose_cli.rs`).

use clap::Parser;
use ironhermes_cli::kanban::KanbanCommands;

// ---------------------------------------------------------------------------
// TestCli scaffold — mirrors decompose_cli.rs verbatim
// ---------------------------------------------------------------------------

#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    cmd: TestSub,
}

#[derive(clap::Subcommand)]
enum TestSub {
    Kanban {
        #[command(subcommand)]
        sub: KanbanCommands,
    },
}

// ---------------------------------------------------------------------------
// D-01 must-have tests — three behaviors per plan §Task 1 <behavior>
// ---------------------------------------------------------------------------

/// Test 1: `--goal --goal-max-turns 15` parses with both fields carried through.
#[test]
fn clap_parses_goal_flag() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "create",
        "--goal",
        "--goal-max-turns",
        "15",
        "--assignee",
        "alice",
        "--body",
        "Acceptance: every page translated",
        "Translate docs",
    ])
    .expect("parse should succeed for `kanban create --goal --goal-max-turns 15 ...`");

    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Create {
                title,
                assignee,
                body,
                goal,
                goal_max_turns,
                ..
            },
        } => {
            assert_eq!(title, "Translate docs");
            assert_eq!(assignee, "alice");
            assert_eq!(body.as_deref(), Some("Acceptance: every page translated"));
            assert!(goal, "--goal flag should set goal=true");
            assert_eq!(goal_max_turns, 15, "--goal-max-turns 15 should land");
        }
        _ => panic!("expected Kanban::Create variant"),
    }
}

/// Test 2: omitting `--goal-max-turns` defaults goal_max_turns to 20 per
/// `default_value_t = 20` (D-03).
#[test]
fn clap_default_max_turns_is_twenty() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "create",
        "--goal",
        "--assignee",
        "alice",
        "Title only",
    ])
    .expect("parse should succeed without --goal-max-turns");

    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Create {
                goal,
                goal_max_turns,
                ..
            },
        } => {
            assert!(goal, "--goal flag should set goal=true");
            assert_eq!(
                goal_max_turns, 20,
                "default_value_t = 20 should yield goal_max_turns=20 when --goal-max-turns is omitted"
            );
        }
        _ => panic!("expected Kanban::Create variant"),
    }
}

/// Test 3: omitting `--goal` entirely leaves goal=false (clap bool default).
#[test]
fn clap_no_goal_default_is_false() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "create",
        "--assignee",
        "alice",
        "Plain task",
    ])
    .expect("parse should succeed without --goal");

    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Create {
                goal,
                goal_max_turns,
                ..
            },
        } => {
            assert!(!goal, "absent --goal should leave goal=false");
            // goal_max_turns is still parseable but unused when goal=false.
            assert_eq!(
                goal_max_turns, 20,
                "default_value_t still applies even when --goal is absent"
            );
        }
        _ => panic!("expected Kanban::Create variant"),
    }
}

// ---------------------------------------------------------------------------
// D-01 must-have test — DB-landing end-to-end via the kanban store
// ---------------------------------------------------------------------------
//
// This test uses ironhermes-kanban's KanbanStore directly (not the cmd_create
// wrapper) because cmd_create resolves a board context from HERMES_HOME, which
// would either contaminate the developer's real DB or require process-wide env
// mutation. The kanban-crate persistence tests (Plan 01,
// goal_mode_persistence.rs) already exercise the full producer path. This test
// proves that the SAME CreateTaskOptions field shape we wire from the CLI in
// Step 3 lands in the DB with goal_mode=true / goal_max_turns=15.

#[test]
fn cli_create_with_goal_lands_in_db() {
    use ironhermes_kanban::store::CreateTaskOptions;
    use ironhermes_kanban::KanbanStore;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = KanbanStore::new(dir.path().join("kanban.db")).expect("open store");

    let opts = CreateTaskOptions {
        body: Some("Acceptance: X".to_string()),
        goal_mode: true,
        goal_max_turns: 15,
        ..CreateTaskOptions::default()
    };

    let task = store
        .create_task("Translate docs", "alice", opts)
        .expect("create_task");
    let reloaded = store.get_task(&task.id).expect("get_task");

    assert!(reloaded.goal_mode, "goal_mode should round-trip true");
    assert_eq!(
        reloaded.goal_max_turns, 15,
        "goal_max_turns=15 should round-trip from CLI-shape CreateTaskOptions"
    );
    assert_eq!(
        reloaded.goal_turns_used, 0,
        "fresh card should have goal_turns_used = 0"
    );
}

// ---------------------------------------------------------------------------
// D-01 must-have — `kanban show` formatter prints goal_* lines for goal cards
// and stays quiet for non-goal cards
// ---------------------------------------------------------------------------

#[test]
fn show_formatter_prints_goal_lines_for_goal_card() {
    use ironhermes_cli::kanban::format::format_task_detail;
    use ironhermes_kanban::store::CreateTaskOptions;
    use ironhermes_kanban::KanbanStore;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = KanbanStore::new(dir.path().join("kanban.db")).expect("open store");

    let opts = CreateTaskOptions {
        body: Some("Acceptance: docs translated".to_string()),
        goal_mode: true,
        goal_max_turns: 15,
        ..CreateTaskOptions::default()
    };
    let task = store
        .create_task("Translate docs", "alice", opts)
        .expect("create_task");
    let reloaded = store.get_task(&task.id).expect("get_task");

    let rendered = format_task_detail(&reloaded, &[], &[], &[]);
    assert!(
        rendered.contains("goal_mode: true"),
        "show output should include `goal_mode: true` line; got:\n{rendered}"
    );
    assert!(
        rendered.contains("goal_max_turns: 15"),
        "show output should include `goal_max_turns: 15` line; got:\n{rendered}"
    );
    assert!(
        rendered.contains("goal_turns_used: 0"),
        "show output should include `goal_turns_used: 0` line; got:\n{rendered}"
    );
}

#[test]
fn show_formatter_omits_goal_lines_for_non_goal_card() {
    use ironhermes_cli::kanban::format::format_task_detail;
    use ironhermes_kanban::store::CreateTaskOptions;
    use ironhermes_kanban::KanbanStore;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = KanbanStore::new(dir.path().join("kanban.db")).expect("open store");

    // No --goal flag → goal_mode stays false (clap-shape default).
    let opts = CreateTaskOptions {
        body: Some("Plain card".to_string()),
        ..CreateTaskOptions::default()
    };
    let task = store
        .create_task("Plain task", "alice", opts)
        .expect("create_task");
    let reloaded = store.get_task(&task.id).expect("get_task");

    let rendered = format_task_detail(&reloaded, &[], &[], &[]);
    assert!(
        !rendered.contains("goal_mode:"),
        "non-goal card should NOT print goal_* lines; got:\n{rendered}"
    );
}
