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
fn swarm_verb_parses_flat_workers() {
    let parsed =
        TestCli::try_parse_from(["hermes", "kanban", "swarm", "my goal", "--workers", "a,b,c"])
            .expect("parse should succeed for `kanban swarm <goal> --workers a,b,c`");
    match parsed.cmd {
        TestSub::Kanban {
            sub:
                KanbanCommands::Swarm {
                    goal,
                    workers,
                    workers_json,
                    ..
                },
        } => {
            assert_eq!(goal, "my goal");
            assert_eq!(workers.as_deref(), Some("a,b,c"));
            assert!(workers_json.is_none());
        }
        _ => panic!("expected Kanban::Swarm variant"),
    }
}

#[test]
fn swarm_verb_parses_rich_workers_json() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "swarm",
        "rich goal",
        "--workers-json",
        r#"[{"assignee":"a"},{"assignee":"b","title":"T"}]"#,
    ])
    .expect("parse should succeed for `kanban swarm <goal> --workers-json '[...]'`");
    match parsed.cmd {
        TestSub::Kanban {
            sub:
                KanbanCommands::Swarm {
                    goal,
                    workers,
                    workers_json,
                    ..
                },
        } => {
            assert_eq!(goal, "rich goal");
            assert!(workers.is_none());
            assert!(workers_json.is_some());
            let wj = workers_json.unwrap();
            assert!(
                wj.starts_with('['),
                "workers_json should round-trip as a JSON array literal"
            );
            assert!(wj.contains(r#""assignee":"b""#));
            assert!(wj.contains(r#""title":"T""#));
        }
        _ => panic!("expected Kanban::Swarm variant"),
    }
}

#[test]
fn swarm_verb_parses_reference_md_664_example() {
    // The verbatim reference.md §664 example — this command line must remain
    // runnable end-to-end after the phase ships.
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "swarm",
        "Design a multi-region failover plan",
        "--workers",
        "researcher,architect,sre",
        "--verifier",
        "reviewer",
        "--synthesizer",
        "writer",
    ])
    .expect("parse should succeed for the reference.md §664 example");
    match parsed.cmd {
        TestSub::Kanban {
            sub:
                KanbanCommands::Swarm {
                    goal,
                    workers,
                    verifier,
                    synthesizer,
                    ..
                },
        } => {
            assert_eq!(goal, "Design a multi-region failover plan");
            assert_eq!(workers.as_deref(), Some("researcher,architect,sre"));
            assert_eq!(verifier.as_deref(), Some("reviewer"));
            assert_eq!(synthesizer.as_deref(), Some("writer"));
        }
        _ => panic!("expected Kanban::Swarm variant"),
    }
}
