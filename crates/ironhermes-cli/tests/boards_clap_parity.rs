//! Phase 36.3.7.9 Plan 03 — clap parse-shape tests for `hermes kanban boards`.
//!
//! These tests verify that the `KanbanCommands::Boards { cmd: BoardsCommands::* }`
//! variant is reachable from the top-level CLI surface and that each leaf verb's
//! arguments parse correctly. They do NOT exercise cmd_boards_* end-to-end
//! (those tests live in boards_lifecycle.rs which uses a tempdir IRONHERMES_HOME).
//!
//! NOTE: `KanbanCommands` does NOT derive `Debug` (Phase 36.3.7 baseline; not
//! changed in this phase). Neither does `BoardsCommands`. Tests use plain string
//! panics on mismatch per the precedent in heartbeat_cli.rs and mention_cli.rs.

use clap::Parser;
use ironhermes_cli::kanban::{KanbanCommands, boards::BoardsCommands};

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
// Test helpers
// ---------------------------------------------------------------------------

fn boards_cmd(argv: &[&str]) -> BoardsCommands {
    let parsed = TestCli::try_parse_from(argv)
        .ok()
        .unwrap_or_else(|| panic!("parse failed for: {:?}", argv));
    match parsed.cmd {
        TestSub::Kanban {
            sub: KanbanCommands::Boards { cmd },
        } => cmd,
        _ => panic!("expected Kanban::Boards variant"),
    }
}

// ---------------------------------------------------------------------------
// boards list
// ---------------------------------------------------------------------------

#[test]
fn boards_list_parses_with_no_flags() {
    match boards_cmd(&["hermes", "kanban", "boards", "list"]) {
        BoardsCommands::List { json } => {
            assert!(!json, "json should default to false");
        }
        _ => panic!("expected BoardsCommands::List"),
    }
}

#[test]
fn boards_list_parses_with_json_flag() {
    match boards_cmd(&["hermes", "kanban", "boards", "list", "--json"]) {
        BoardsCommands::List { json } => {
            assert!(json, "json flag should be true");
        }
        _ => panic!("expected BoardsCommands::List"),
    }
}

// ---------------------------------------------------------------------------
// boards create
// ---------------------------------------------------------------------------

#[test]
fn boards_create_parses_minimum_slug_arg() {
    match boards_cmd(&["hermes", "kanban", "boards", "create", "alpha"]) {
        BoardsCommands::Create {
            slug,
            name,
            description,
            switch,
            json,
        } => {
            assert_eq!(slug, "alpha");
            assert!(name.is_none());
            assert!(description.is_none());
            assert!(!switch);
            assert!(!json);
        }
        _ => panic!("expected BoardsCommands::Create"),
    }
}

#[test]
fn boards_create_parses_with_switch_and_name() {
    match boards_cmd(&[
        "hermes",
        "kanban",
        "boards",
        "create",
        "atm10-server",
        "--switch",
        "--name",
        "ATM10 Server",
    ]) {
        BoardsCommands::Create {
            slug,
            name,
            description,
            switch,
            json,
        } => {
            assert_eq!(slug, "atm10-server");
            assert_eq!(name.as_deref(), Some("ATM10 Server"));
            assert!(description.is_none());
            assert!(switch);
            assert!(!json);
        }
        _ => panic!("expected BoardsCommands::Create"),
    }
}

#[test]
fn boards_create_parses_with_description() {
    match boards_cmd(&[
        "hermes",
        "kanban",
        "boards",
        "create",
        "beta",
        "--description",
        "Beta board",
    ]) {
        BoardsCommands::Create {
            slug, description, ..
        } => {
            assert_eq!(slug, "beta");
            assert_eq!(description.as_deref(), Some("Beta board"));
        }
        _ => panic!("expected BoardsCommands::Create"),
    }
}

// ---------------------------------------------------------------------------
// boards switch
// ---------------------------------------------------------------------------

#[test]
fn boards_switch_parses_slug_arg() {
    match boards_cmd(&["hermes", "kanban", "boards", "switch", "my-board"]) {
        BoardsCommands::Switch { slug } => {
            assert_eq!(slug, "my-board");
        }
        _ => panic!("expected BoardsCommands::Switch"),
    }
}

// ---------------------------------------------------------------------------
// boards show
// ---------------------------------------------------------------------------

#[test]
fn boards_show_parses_with_no_flags() {
    match boards_cmd(&["hermes", "kanban", "boards", "show"]) {
        BoardsCommands::Show { json } => {
            assert!(!json);
        }
        _ => panic!("expected BoardsCommands::Show"),
    }
}

#[test]
fn boards_show_parses_with_json_flag() {
    match boards_cmd(&["hermes", "kanban", "boards", "show", "--json"]) {
        BoardsCommands::Show { json } => {
            assert!(json);
        }
        _ => panic!("expected BoardsCommands::Show"),
    }
}

// ---------------------------------------------------------------------------
// boards rename
// ---------------------------------------------------------------------------

#[test]
fn boards_rename_parses_slug_and_name() {
    match boards_cmd(&[
        "hermes",
        "kanban",
        "boards",
        "rename",
        "alpha",
        "Alpha Board",
    ]) {
        BoardsCommands::Rename { slug, new_name } => {
            assert_eq!(slug, "alpha");
            assert_eq!(new_name, "Alpha Board");
        }
        _ => panic!("expected BoardsCommands::Rename"),
    }
}

// ---------------------------------------------------------------------------
// boards rm
// ---------------------------------------------------------------------------

#[test]
fn boards_rm_parses_slug_only() {
    match boards_cmd(&["hermes", "kanban", "boards", "rm", "alpha"]) {
        BoardsCommands::Rm { slug, delete } => {
            assert_eq!(slug, "alpha");
            assert!(!delete, "delete should default to false");
        }
        _ => panic!("expected BoardsCommands::Rm"),
    }
}

#[test]
fn boards_rm_parses_with_delete_flag() {
    match boards_cmd(&["hermes", "kanban", "boards", "rm", "alpha", "--delete"]) {
        BoardsCommands::Rm { slug, delete } => {
            assert_eq!(slug, "alpha");
            assert!(delete, "delete flag should be true");
        }
        _ => panic!("expected BoardsCommands::Rm"),
    }
}
