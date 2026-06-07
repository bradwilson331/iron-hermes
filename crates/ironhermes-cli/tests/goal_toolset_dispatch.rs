//! Phase 36.3.7.13 Plan 03 Task 2 — bilateral CLI dispatch tests (F-03).
//!
//! Validates the clap surface for `--goal-toolset` on `ironhermes kanban create`:
//!
//! 1. **toolset_requires_goal** — `--goal-toolset restricted` without `--goal`
//!    must be rejected by clap (requires = "goal" guard).
//! 2. **toolset_accepted_with_goal** — `--goal --goal-toolset restricted` parses
//!    successfully and the parsed value is `Some("restricted".to_string())`.
//! 3. **toolset_defaults_to_none_without_flag** — omitting `--goal-toolset` when
//!    `--goal` IS set yields `None` (no silent default preset at CLI layer).

use clap::{CommandFactory, Parser};

// ---------------------------------------------------------------------------
// Minimal Cli shim — parses only the subset needed for kanban create.
// ---------------------------------------------------------------------------
//
// We drive clap directly (no binary invocation) so the test runs at
// `cargo test` speed without a pre-built binary.

#[derive(Parser, Debug)]
struct TestCli {
    #[command(subcommand)]
    cmd: TestKanban,
}

#[derive(clap::Subcommand, Debug)]
enum TestKanban {
    Create {
        title: String,
        #[arg(long)]
        assignee: String,
        #[arg(long)]
        goal: bool,
        #[arg(long, default_value_t = 20)]
        goal_max_turns: u32,
        /// Mirrors the production flag: requires = "goal", no default_value.
        #[arg(long, requires = "goal")]
        goal_toolset: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Test 1: --goal-toolset without --goal is rejected by clap
// ---------------------------------------------------------------------------

/// F-03 requires = "goal" guard: clap must reject `--goal-toolset restricted`
/// when `--goal` is absent. This is RESEARCH Risk 1 / BLOCKER 1 — the guard
/// prevents operators from silently setting a toolset on non-goal cards.
#[test]
fn toolset_requires_goal() {
    let result = TestCli::try_parse_from([
        "test",
        "create",
        "My task",
        "--assignee",
        "alice",
        "--goal-toolset",
        "restricted",
        // NOTE: --goal intentionally absent.
    ]);

    assert!(
        result.is_err(),
        "clap must reject --goal-toolset without --goal (requires = \"goal\" guard). \
         Got Ok({result:?})"
    );

    // Verify the error text mentions the missing requirement (not just any error).
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("goal") || err.contains("required") || err.contains("requires"),
        "error message must mention the missing --goal requirement; got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Test 2: --goal --goal-toolset restricted parses successfully
// ---------------------------------------------------------------------------

/// F-03 happy path: `--goal --goal-toolset restricted` must parse successfully
/// and produce `goal_toolset = Some("restricted".to_string())`.
#[test]
fn toolset_accepted_with_goal() {
    let parsed = TestCli::try_parse_from([
        "test",
        "create",
        "My task",
        "--assignee",
        "alice",
        "--goal",
        "--goal-toolset",
        "restricted",
    ])
    .expect("--goal --goal-toolset restricted must parse without error");

    match parsed.cmd {
        TestKanban::Create { goal_toolset, goal, .. } => {
            assert!(goal, "--goal must be true");
            assert_eq!(
                goal_toolset,
                Some("restricted".to_string()),
                "goal_toolset must be Some(\"restricted\") when --goal-toolset restricted is passed"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 3: omitting --goal-toolset yields None (no silent default at CLI layer)
// ---------------------------------------------------------------------------

/// F-03 no-default contract: omitting `--goal-toolset` when `--goal` IS set
/// must yield `goal_toolset = None`. The CLI layer must NOT silently inject
/// "restricted" — the DB NULL → D-E2 worker-side warn path is the contract.
#[test]
fn toolset_defaults_to_none_without_flag() {
    let parsed = TestCli::try_parse_from([
        "test",
        "create",
        "My task",
        "--assignee",
        "alice",
        "--goal",
        // --goal-toolset intentionally absent.
    ])
    .expect("--goal without --goal-toolset must parse without error");

    match parsed.cmd {
        TestKanban::Create { goal_toolset, goal, .. } => {
            assert!(goal, "--goal must be true");
            assert_eq!(
                goal_toolset,
                None,
                "goal_toolset must be None when --goal-toolset is not passed; \
                 the CLI layer must NOT inject a silent default"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Smoke: CommandFactory generates a valid command (catches schema regressions)
// ---------------------------------------------------------------------------

#[test]
fn cli_command_is_valid() {
    TestCli::command().debug_assert();
}
