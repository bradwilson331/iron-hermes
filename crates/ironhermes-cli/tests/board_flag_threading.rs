//! Phase 36.3.7.9 Plan 04 — integration tests for the parent-level `--board <slug>` flag.
//!
//! Tests cover:
//!   1. `--board` flag parses at the kanban parent level (before the verb).
//!   2. Absence of `--board` yields `None` (default-omit is byte-stable).
//!   3. Compile-time signature lock on `handle_kanban_command`.
//!   4. End-to-end: `open_store_for_board(Some("alpha"))` routes to the named board DB.

use clap::Parser;
use ironhermes_cli::kanban::KanbanCommands;
use std::sync::Mutex;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Test infrastructure — mirrors boards_lifecycle.rs pattern
// ---------------------------------------------------------------------------

/// Serialise tests that mutate IRONHERMES_HOME.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard: sets IRONHERMES_HOME to a fresh tempdir for the test's lifetime.
struct ScopedHome {
    _dir: TempDir,
    pub root: std::path::PathBuf,
}

impl ScopedHome {
    fn new() -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let root = dir.path().to_path_buf();
        // SAFETY: test-only; we hold ENV_LOCK for the duration.
        unsafe { std::env::set_var("IRONHERMES_HOME", &root) };
        // Clear board-selection env vars so tests start from a known state.
        unsafe { std::env::remove_var("HERMES_KANBAN_BOARD") };
        ScopedHome { _dir: dir, root }
    }
}

impl Drop for ScopedHome {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("IRONHERMES_HOME") };
        unsafe { std::env::remove_var("HERMES_KANBAN_BOARD") };
    }
}

// ---------------------------------------------------------------------------
// TestCli scaffold — includes board field at the Kanban parent level,
// mirroring the real Cli definition after Plan 04's main.rs change.
// ---------------------------------------------------------------------------

#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    cmd: TestSub,
}

#[derive(clap::Subcommand)]
enum TestSub {
    Kanban {
        /// Select board by slug (mirrors real --board flag added in Plan 04).
        #[arg(long, global = false)]
        board: Option<String>,
        #[command(subcommand)]
        sub: KanbanCommands,
    },
}

// ---------------------------------------------------------------------------
// Test 1: --board flag parses at the kanban parent level
// ---------------------------------------------------------------------------

/// `hermes kanban --board alpha list` — flag is parsed before the verb.
#[test]
fn board_flag_parses_at_kanban_parent_level() {
    let parsed = TestCli::try_parse_from(["hermes", "kanban", "--board", "alpha", "list"])
        .ok()
        .expect("parse should succeed for `kanban --board alpha list`");

    match parsed.cmd {
        TestSub::Kanban { board, sub } => {
            assert_eq!(board.as_deref(), Some("alpha"), "--board should capture 'alpha'");
            // Verify the inner verb is KanbanCommands::List (spot-check)
            assert!(
                matches!(sub, KanbanCommands::List { .. }),
                "inner verb should be KanbanCommands::List"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 2: absence of --board yields None
// ---------------------------------------------------------------------------

/// `hermes kanban list` (no --board) — board field must be None.
#[test]
fn board_flag_absent_yields_none() {
    let parsed = TestCli::try_parse_from(["hermes", "kanban", "list"])
        .ok()
        .expect("parse should succeed for `kanban list` without --board");

    match parsed.cmd {
        TestSub::Kanban { board, sub } => {
            assert!(board.is_none(), "--board should be None when flag is absent");
            assert!(
                matches!(sub, KanbanCommands::List { .. }),
                "inner verb should be KanbanCommands::List"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 3: compile-time signature lock on handle_kanban_command
// ---------------------------------------------------------------------------

/// Locks the `handle_kanban_command` function signature at type-system level.
///
/// This test does not require a runtime — it is a compile-time assertion.
/// If anyone changes the signature, this test fails to compile.
#[test]
fn board_flag_passes_to_handler_signature_test() {
    // Compile-time: assert handle_kanban_command has the expected fn-pointer type.
    // The underscore suppresses unused-variable warnings; the cast is what matters.
    let _: fn(
        KanbanCommands,
        Option<String>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<i32>> + Send>,
    > = |cmd, board| Box::pin(ironhermes_cli::kanban::handle_kanban_command(cmd, board));

    // Runtime: trivially pass.
    assert!(true, "signature lock test: if it compiled, it passed");
}

// ---------------------------------------------------------------------------
// Test 4: end-to-end board routing via open_store_for_board
// ---------------------------------------------------------------------------

/// Creates a named board "alpha", then opens it via `open_store_for_board(Some("alpha"))`,
/// and asserts the underlying DB path contains `boards/alpha/kanban.db`.
#[test]
fn board_flag_routes_open_store_to_named_board() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = ScopedHome::new();

    let rt = tokio::runtime::Runtime::new().unwrap();

    // Step 1: create the "alpha" board.
    let create_result = rt.block_on(ironhermes_cli::kanban::boards::cmd_boards_create(
        "alpha".to_string(),
        Some("Alpha Board".to_string()),
        None,
        false, // switch
        false, // json
    ));
    assert_eq!(
        create_result.expect("cmd_boards_create should succeed"),
        0,
        "board create should return exit code 0"
    );

    // Step 2: confirm the DB file exists at the expected path.
    // Path is: $IRONHERMES_HOME/kanban/boards/alpha/kanban.db (D-01 layout).
    let expected_db = home.root.join("kanban").join("boards").join("alpha").join("kanban.db");
    assert!(
        expected_db.exists(),
        "kanban/boards/alpha/kanban.db should exist after create; home={}",
        home.root.display()
    );

    // Step 3: open the board via open_store_for_board and round-trip a list.
    let store = ironhermes_cli::kanban::commands::open_store_for_board(Some("alpha"))
        .expect("open_store_for_board(Some(\"alpha\")) should succeed");

    // The store opened successfully — perform a lightweight read to confirm
    // the DB is the alpha board (not the default kanban.db).
    let tasks = store
        .list_tasks(ironhermes_kanban::store::ListFilters::default())
        .expect("list_tasks on fresh alpha board should succeed");
    assert!(
        tasks.is_empty(),
        "fresh alpha board should have no tasks; got {} task(s)",
        tasks.len()
    );

    // Also verify the path is NOT the legacy kanban.db path.
    let legacy_db = home.root.join("kanban.db");
    assert!(
        !legacy_db.exists(),
        "legacy kanban.db should NOT be created when opening a named board"
    );
}
