//! Phase 36.3.7.11 Plan 02 — Kanban transition validator + protocol types.
//!
//! Source-string lock (mirrors `tests/kanban_server_fns.rs` pattern). The
//! `iron_hermes_ui` crate is a `[[bin]]` (no `[lib]`), so behavior tests
//! cannot import its modules — those tests live as `#[cfg(test)] mod
//! tests` inside `src/kanban/transitions.rs` and `src/protocol.rs` and run
//! via `cargo test -p iron_hermes_ui --bin iron_hermes_ui`.
//!
//! This integration test asserts the source-level structure: required
//! identifiers, allowed-table arms (D-10), hint copy from UI-SPEC §7.4
//! (D-11), and the protocol type declarations (D-13).

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

// ---------------------------------------------------------------------------
// Module structure
// ---------------------------------------------------------------------------

#[test]
fn kanban_transitions_module_exists() {
    let _ = read("src/kanban/mod.rs");
    let src = read("src/kanban/transitions.rs");
    assert!(
        src.contains("pub fn is_drag_allowed"),
        "kanban::transitions::is_drag_allowed must be declared"
    );
    assert!(
        src.contains("pub fn drag_blocked_hint"),
        "kanban::transitions::drag_blocked_hint must be declared"
    );
}

// ---------------------------------------------------------------------------
// D-10: allowed table — assert each of the four canonical patterns appears
// inside `is_drag_allowed`.
// ---------------------------------------------------------------------------

#[test]
fn is_drag_allowed_lists_the_four_allowed_patterns() {
    let src = read("src/kanban/transitions.rs");
    // The canonical implementation uses `matches!` over the four arms.
    // We assert each pattern token appears in the file body.
    for needle in [
        "KanbanStatus::Triage, KanbanStatus::Todo",
        "KanbanStatus::Todo, KanbanStatus::Ready",
        "KanbanStatus::Blocked, KanbanStatus::Ready",
        "KanbanStatus::Archived",
    ] {
        assert!(
            src.contains(needle),
            "D-10: transitions.rs is_drag_allowed must include arm `{}`",
            needle,
        );
    }
}

// ---------------------------------------------------------------------------
// D-11 / UI-SPEC §7.4 — verbatim hint copy
// ---------------------------------------------------------------------------

#[test]
fn drag_blocked_hint_uses_verbatim_ui_spec_copy() {
    let src = read("src/kanban/transitions.rs");
    for needle in [
        "Completing a task needs a summary — open the card and click Complete…",
        "Blocking a task needs a reason — open the card and click Block…",
        "The dispatcher owns this transition — cannot assign running status from UI",
    ] {
        assert!(
            src.contains(needle),
            "D-11 / UI-SPEC §7.4: drag_blocked_hint must use verbatim copy: {}",
            needle,
        );
    }
}

// ---------------------------------------------------------------------------
// D-13 / Q6 — new protocol types declared in protocol.rs
// ---------------------------------------------------------------------------

#[test]
fn protocol_declares_create_task_payload() {
    let src = read("src/protocol.rs");
    assert!(
        src.contains("pub struct CreateTaskPayload"),
        "D-13: protocol.rs must declare CreateTaskPayload"
    );
    // Field set per CONTEXT D-13: title, assignee, parents, priority,
    // tenant, body, start_in_triage.
    for needle in [
        "pub title: String",
        "pub assignee: Option<String>",
        "pub parents: Vec<String>",
        "pub priority: i64",
        "pub tenant: Option<String>",
        "pub body: Option<String>",
        "pub start_in_triage: bool",
    ] {
        assert!(
            src.contains(needle),
            "D-13: CreateTaskPayload must declare `{}`",
            needle,
        );
    }
}

#[test]
fn protocol_declares_decompose_or_specify() {
    let src = read("src/protocol.rs");
    assert!(
        src.contains("pub enum DecomposeOrSpecify"),
        "D-13: protocol.rs must declare DecomposeOrSpecify"
    );
    assert!(
        src.contains("Decompose,"),
        "D-13: DecomposeOrSpecify must declare Decompose variant"
    );
    assert!(
        src.contains("Specify,"),
        "D-13: DecomposeOrSpecify must declare Specify variant"
    );
}

#[test]
fn protocol_declares_decompose_result() {
    let src = read("src/protocol.rs");
    assert!(
        src.contains("pub enum DecomposeResult"),
        "D-13: protocol.rs must declare DecomposeResult"
    );
    assert!(
        src.contains("children_count"),
        "D-13: DecomposeResult::Ok must declare children_count"
    );
    assert!(
        src.contains("NotWired"),
        "D-13: DecomposeResult::NotWired variant must exist"
    );
}

#[test]
fn protocol_declares_kanban_status_wire_enum() {
    let src = read("src/protocol.rs");
    assert!(
        src.contains("pub enum KanbanStatus"),
        "D-09 wire: protocol.rs must declare KanbanStatus wire-copy enum"
    );
    // Must list all seven canonical variants.
    for needle in [
        "Triage", "Todo", "Ready", "InProgress", "Blocked", "Done", "Archived",
    ] {
        assert!(
            src.contains(needle),
            "D-09 wire: KanbanStatus must declare variant `{}`",
            needle,
        );
    }
}
