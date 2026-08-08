//! Phase 46.3 Plan 02 — Source-string regression locks for D-03 (in-flight
//! pending state) and D-04 (board auto-refresh on success) in the three
//! triage-action client handlers (`on_decompose`, `on_specify`,
//! `on_triage_action`) in `kanban.rs`.
//!
//! D-03: each handler must insert into `pending_task_ids` before the
//!       `run_decompose_or_specify` spawn and remove after the `.await`
//!       resolves, mirroring the canonical `move_task_optimistic`
//!       scoped-write pattern (kanban/board.rs).
//! D-04: the `Ok(DecomposeResult::Ok { .. })` arm of each handler must call
//!       `board_resource.restart()` to re-fetch the board so new/promoted
//!       cards appear without a manual reload.
//!
//! Pattern: source-read assertion (mirrors `kanban_write_fns.rs`). Own file
//! (does not conflict with Plan 01's edits to `kanban_write_fns.rs`).
//! Runs without the `server` feature; the binary crate is not importable
//! externally.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

const KANBAN_SRC: &str = "src/components/hermes_app/screens/kanban.rs";

/// Slice a single handler closure's body out of the full source by locating
/// `start_marker` and cutting at the next occurrence of `end_marker` (the
/// start of the following sibling declaration). Keeps assertions scoped to
/// just that handler instead of the whole file, so a per-handler check
/// can't be satisfied by wiring that landed in a *different* handler.
fn handler_body<'a>(src: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start = src
        .find(start_marker)
        .unwrap_or_else(|| panic!("expected to find `{start_marker}` in kanban.rs"));
    let rest = &src[start..];
    let end = rest.find(end_marker).unwrap_or_else(|| {
        panic!("expected to find `{end_marker}` after `{start_marker}` in kanban.rs")
    });
    &rest[..end]
}

// ---------------------------------------------------------------------------
// D-03 — pending_task_ids wiring.
//
// Baseline (pre-Plan-02) substring occurrence count of `pending_task_ids` in
// kanban.rs was 4 (module-doc comment, the `Signal<HashSet<String>>`
// declaration, and the `KanbanBoard` prop pass-through). Plan 02 adds one
// `let mut p = pending_task_ids;` rebind (plus an explanatory `D-03:` comment
// referencing the name) per handler across the three triage handlers,
// bringing the observed post-impl count to 10. Threshold is set at
// baseline + 3 (>= 7): removing all three handlers' wiring drops the count
// back toward the 4-occurrence baseline, which trips this assertion.
// ---------------------------------------------------------------------------

#[test]
fn triage_handlers_reference_pending_task_ids() {
    let src = read(KANBAN_SRC);

    let occurrences = src.matches("pending_task_ids").count();
    assert!(
        occurrences >= 7,
        "D-03: expected >= 7 occurrences of `pending_task_ids` in kanban.rs \
         (baseline 4 + >=3 handler references), found {occurrences} — pending-state \
         wiring may have been removed from one or more triage handlers",
    );

    // A global count alone can't prove *per-handler* wiring — slice each
    // closure body out and check it directly.
    let decompose_body = handler_body(&src, "let on_decompose = move", "let on_specify = move");
    assert!(
        decompose_body.contains("pending_task_ids"),
        "D-03: on_decompose must reference pending_task_ids"
    );

    let specify_body = handler_body(&src, "let on_specify = move", "let on_post_comment = move");
    assert!(
        specify_body.contains("pending_task_ids"),
        "D-03: on_specify must reference pending_task_ids"
    );

    let triage_action_body = handler_body(
        &src,
        "let on_triage_action =",
        "// Modal-success handlers refresh the board + close.",
    );
    assert!(
        triage_action_body.contains("pending_task_ids"),
        "D-03: on_triage_action must reference pending_task_ids"
    );
}

// ---------------------------------------------------------------------------
// D-04 — board_resource.restart() on success.
//
// Baseline (pre-Plan-02) substring occurrence count of
// `board_resource.restart()` in kanban.rs was 6 (module-doc + inline
// comments referencing the call, the live WS-effect refresh, the
// `restart_board` closure definition, and a modal-success refresh call
// site). Plan 02 adds exactly one new `board_resource.restart()` call site
// per handler (in the `Ok(DecomposeResult::Ok { .. })` arm only), bringing
// the observed post-impl count to 9. Threshold is baseline + 3 (>= 9):
// removing any of the three new success-arm refreshes regresses the count
// below this.
// ---------------------------------------------------------------------------

#[test]
fn triage_handlers_refresh_board_on_success() {
    let src = read(KANBAN_SRC);

    let occurrences = src.matches("board_resource.restart()").count();
    assert!(
        occurrences >= 9,
        "D-04: expected >= 9 occurrences of `board_resource.restart()` in kanban.rs \
         (baseline 6 + 3 new success-arm refreshes), found {occurrences} — the \
         auto-refresh wiring may have been removed from one or more triage handlers",
    );

    let decompose_body = handler_body(&src, "let on_decompose = move", "let on_specify = move");
    assert!(
        decompose_body.contains("board_resource.restart()"),
        "D-04: on_decompose's Ok(DecomposeResult::Ok) arm must call board_resource.restart()"
    );

    let specify_body = handler_body(&src, "let on_specify = move", "let on_post_comment = move");
    assert!(
        specify_body.contains("board_resource.restart()"),
        "D-04: on_specify's Ok(DecomposeResult::Ok) arm must call board_resource.restart()"
    );

    let triage_action_body = handler_body(
        &src,
        "let on_triage_action =",
        "// Modal-success handlers refresh the board + close.",
    );
    assert!(
        triage_action_body.contains("board_resource.restart()"),
        "D-04: on_triage_action's Ok(DecomposeResult::Ok) arm must call board_resource.restart()"
    );
}
