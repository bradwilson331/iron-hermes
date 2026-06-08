//! Phase 36.3.7.10 — CLI parity smoke tests for `hermes kanban decompose` and
//! `hermes kanban specify`.
//!
//! Run: `cargo test -p ironhermes-cli --test decompose_cli`
//!
//! These tests parse argv through a local `TestCli` wrapper using clap-derive
//! to verify that `KanbanCommands::Decompose` and `KanbanCommands::Specify` are
//! reachable from the top-level CLI subcommand surface. They do NOT exercise
//! `cmd_decompose` or `cmd_specify` end-to-end (those require a configured
//! provider + live kanban DB).
//!
//! NOTE: `KanbanCommands` does NOT derive `Debug` (Phase 36.3.7 baseline; not
//! changed in this phase). So we cannot derive Debug on TestSub either — the
//! `{other:?}` panic-message form is unavailable. Tests use plain string panics
//! if the match arm is wrong (same convention as heartbeat_cli.rs and mention_cli.rs).

use clap::Parser;
use ironhermes_cli::kanban::KanbanCommands;

// ---------------------------------------------------------------------------
// TestCli scaffold — mirrors heartbeat_cli.rs / mention_cli.rs verbatim
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
// DEC-08 / DEC-09 locked tests (≥4 required)
// ---------------------------------------------------------------------------

/// Test 1: positional `<id>` parses correctly; all flags default correctly.
#[test]
fn decompose_single_id_parses() {
    let parsed = TestCli::try_parse_from(["hermes", "kanban", "decompose", "t_abc"])
        .expect("parse should succeed for `kanban decompose t_abc`");
    match parsed.cmd {
        TestSub::Kanban {
            sub:
                KanbanCommands::Decompose {
                    id,
                    all,
                    tenant,
                    json,
                    board,
                },
        } => {
            assert_eq!(id.as_deref(), Some("t_abc"));
            assert!(!all, "all should default to false");
            assert!(tenant.is_none());
            assert!(!json);
            assert!(board.is_none());
        }
        _ => panic!("expected Kanban::Decompose variant"),
    }
}

/// Test 2: --all flag with --tenant and --json all parse correctly.
#[test]
fn decompose_all_with_tenant_and_json_parses() {
    let parsed = TestCli::try_parse_from([
        "hermes",
        "kanban",
        "decompose",
        "--all",
        "--tenant",
        "engineering",
        "--json",
    ])
    .expect("parse should succeed for `kanban decompose --all --tenant engineering --json`");
    match parsed.cmd {
        TestSub::Kanban {
            sub:
                KanbanCommands::Decompose {
                    id,
                    all,
                    tenant,
                    json,
                    ..
                },
        } => {
            assert!(id.is_none(), "id should be None when --all is set");
            assert!(all, "all should be true");
            assert_eq!(tenant.as_deref(), Some("engineering"));
            assert!(json, "json should be true");
        }
        _ => panic!("expected Kanban::Decompose variant"),
    }
}

/// Test 3: providing both `<id>` and `--all` must be rejected by clap's conflicts_with.
#[test]
fn decompose_rejects_id_and_all_together() {
    let result = TestCli::try_parse_from(["hermes", "kanban", "decompose", "t_abc", "--all"]);
    assert!(
        result.is_err(),
        "clap should reject simultaneous positional id and --all (conflicts_with)"
    );
}

/// Test 4: `hermes kanban specify t_xyz --author operator` parses correctly.
#[test]
fn specify_id_with_author_parses() {
    let parsed = TestCli::try_parse_from([
        "hermes", "kanban", "specify", "t_xyz", "--author", "operator",
    ])
    .expect("parse should succeed for `kanban specify t_xyz --author operator`");
    match parsed.cmd {
        TestSub::Kanban {
            sub:
                KanbanCommands::Specify {
                    id,
                    all,
                    author,
                    json,
                    ..
                },
        } => {
            assert_eq!(id.as_deref(), Some("t_xyz"));
            assert!(!all);
            assert_eq!(author.as_deref(), Some("operator"));
            assert!(!json);
        }
        _ => panic!("expected Kanban::Specify variant"),
    }
}

/// Test 5: `hermes kanban specify --all --json` parses correctly.
#[test]
fn specify_all_with_json_parses() {
    let parsed = TestCli::try_parse_from(["hermes", "kanban", "specify", "--all", "--json"])
        .expect("parse should succeed for `kanban specify --all --json`");
    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Specify { id, all, json, .. },
        } => {
            assert!(id.is_none());
            assert!(all);
            assert!(json);
        }
        _ => panic!("expected Kanban::Specify variant"),
    }
}

/// Test 6 (bonus): --board flag on decompose is captured correctly.
#[test]
fn decompose_with_board_flag_parses() {
    let parsed =
        TestCli::try_parse_from(["hermes", "kanban", "decompose", "t_abc", "--board", "alpha"])
            .expect("parse should succeed for `kanban decompose t_abc --board alpha`");
    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Decompose { id, board, .. },
        } => {
            assert_eq!(id.as_deref(), Some("t_abc"));
            assert_eq!(board.as_deref(), Some("alpha"));
        }
        _ => panic!("expected Kanban::Decompose variant"),
    }
}
