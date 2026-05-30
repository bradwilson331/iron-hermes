//! Phase 36.3.7.7 D-cli-verb-shape — CLI parity smoke tests for the new
//! `hermes kanban swarm` verb (BUG-36.3.7.7-01 CLI parity).
//!
//! These tests parse argv through a local `TestCli` wrapper using clap-derive
//! to verify that the `KanbanCommands::Swarm { ... }` variant is reachable
//! from the top-level CLI subcommand surface. They do NOT exercise
//! `cmd_swarm` end-to-end (that would require a tempfile-scoped HERMES_HOME
//! and is covered by the LLM-tool's tools_smoke tests, which share the same
//! `KanbanStore::create_swarm` path).

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
#[ignore = "Wave 0 scaffold - implemented in Wave 5"]
fn swarm_verb_parses_flat_workers() {
    panic!("Wave 0 scaffold");
}

#[test]
#[ignore = "Wave 0 scaffold - implemented in Wave 5"]
fn swarm_verb_parses_rich_workers_json() {
    panic!("Wave 0 scaffold");
}

#[test]
#[ignore = "Wave 0 scaffold - implemented in Wave 5"]
fn swarm_verb_parses_reference_md_664_example() {
    panic!("Wave 0 scaffold");
}
