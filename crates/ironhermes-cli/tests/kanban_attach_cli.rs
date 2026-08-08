//! Phase 46.4 Plan 05 — CLI parity smoke tests for `kanban attach` (D-04/D-03)
//! and `kanban complete --output-path` (D-10).
//!
//! These tests parse argv through a local `TestCli` wrapper using clap-derive
//! to verify that `KanbanCommands::Attach` and `KanbanCommands::Complete`
//! carry the new fields with the correct shape. They do NOT exercise
//! `cmd_attach`/`cmd_complete` end-to-end against a live DB — that path
//! requires a tempdir-scoped IRONHERMES_HOME and is covered by the
//! kanban-crate producer tests (`tests/attachment_store.rs`,
//! `tests/tools_smoke.rs` from Plan 03).
//!
//! NOTE: `KanbanCommands` does NOT derive `Debug` (Phase 36.3.7 baseline).
//! So we cannot derive Debug on `TestSub` either — the `{other:?}`
//! panic-message form is unavailable. Tests use plain string panics if the
//! match arm is wrong (same convention as `kanban_create_goal_flag.rs`).

use clap::Parser;
use ironhermes_cli::kanban::KanbanCommands;

// ---------------------------------------------------------------------------
// TestCli scaffold — mirrors kanban_create_goal_flag.rs verbatim
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
// D-04/D-03 must-have test — `attach <id> <file>` parses into
// KanbanCommands::Attach { id, file }
// ---------------------------------------------------------------------------

#[test]
fn clap_parses_attach_id_and_file() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "attach",
        "t_abc123",
        "/tmp/some/path.txt",
    ])
    .expect("parse should succeed for `kanban attach <id> <file>`");

    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Attach { id, file },
        } => {
            assert_eq!(id, "t_abc123");
            assert_eq!(file, std::path::PathBuf::from("/tmp/some/path.txt"));
        }
        _ => panic!("expected Kanban::Attach variant"),
    }
}

// ---------------------------------------------------------------------------
// D-10 must-have test — `complete <id> --output-path <path>` parses into
// KanbanCommands::Complete { output_path: Some(..), .. }
// ---------------------------------------------------------------------------

#[test]
fn clap_parses_complete_output_path() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "complete",
        "t_abc123",
        "--output-path",
        "/x/y",
    ])
    .expect("parse should succeed for `kanban complete <id> --output-path <path>`");

    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Complete {
                ids, output_path, ..
            },
        } => {
            assert_eq!(ids, vec!["t_abc123".to_string()]);
            assert_eq!(output_path.as_deref(), Some("/x/y"));
        }
        _ => panic!("expected Kanban::Complete variant"),
    }
}

/// Omitting `--output-path` leaves it `None` (clap Option default).
#[test]
fn clap_complete_without_output_path_defaults_none() {
    let parsed = TestCli::try_parse_from(["hermes", "kanban", "complete", "t_abc123"])
        .expect("parse should succeed for `kanban complete <id>` without --output-path");

    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Complete { output_path, .. },
        } => {
            assert_eq!(output_path, None);
        }
        _ => panic!("expected Kanban::Complete variant"),
    }
}
