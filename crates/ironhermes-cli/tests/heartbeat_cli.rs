//! Phase 36.3.7.6 D-cli-heartbeat-parity — CLI parity smoke tests for the
//! new `hermes kanban heartbeat` verb (BUG-36.3.7.6-01 CLI parity).
//!
//! These tests parse argv through a local `TestCli` wrapper using clap-derive
//! to verify that the `KanbanCommands::Heartbeat { id, note }` variant is
//! reachable from the top-level CLI subcommand surface. They do NOT exercise
//! `cmd_heartbeat` end-to-end (that would require a tempfile-scoped HERMES_HOME
//! and is covered by the LLM-tool's tools_smoke tests, which share the same
//! append_event path).

use clap::Parser;
use ironhermes_cli::kanban::KanbanCommands;

// NOTE: `KanbanCommands` does NOT derive `Debug` (Phase 36.3.7 baseline; not
// changed in this phase). So we cannot derive Debug on TestSub either — the
// `{other:?}` panic-message form is unavailable. Tests use a plain string
// panic if the match arm is wrong, which is sufficient for parse-shape
// verification.
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

#[test]
fn heartbeat_verb_parses_with_id_only() {
    let parsed = TestCli::try_parse_from(["hermes", "kanban", "heartbeat", "t_x"])
        .expect("parse should succeed for `kanban heartbeat t_x`");
    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Heartbeat { id, note },
        } => {
            assert_eq!(id, "t_x");
            assert_eq!(note, None);
        }
        _ => panic!("expected Kanban::Heartbeat variant"),
    }
}

#[test]
fn heartbeat_verb_parses_with_id_and_note() {
    let parsed =
        TestCli::try_parse_from(["hermes", "kanban", "heartbeat", "t_x", "--note", "halfway"])
            .expect("parse should succeed for `kanban heartbeat t_x --note halfway`");
    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Heartbeat { id, note },
        } => {
            assert_eq!(id, "t_x");
            assert_eq!(note, Some("halfway".to_string()));
        }
        _ => panic!("expected Kanban::Heartbeat variant"),
    }
}
