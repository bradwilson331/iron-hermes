//! Phase 36.3.7.11 Plan 02 Task 3 — Drag-and-drop client wiring locks.
//!
//! Source-string asserts on the four touched component files:
//!   card.rs   — draggable + ondragstart + ondragend + id stable for focus restore.
//!   column.rs — ondragover (Event<DragData>) + prevent_default() outer Event +
//!               ondrop + is_drag_allowed reachable.
//!   board.rs  — is_drag_allowed reachable from the shared optimistic helper.
//!   kanban.rs — live-region `role: "log"` + `aria_live: "polite"`.
//!
//! Pattern: source-read assertion (mirrors kanban_server_fns.rs).

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

// ---------------------------------------------------------------------------
// card.rs — drag source + focus-restore id
// ---------------------------------------------------------------------------

#[test]
fn card_has_draggable_true_attribute() {
    let src = read("src/components/hermes_app/screens/kanban/card.rs");
    assert!(
        src.contains("draggable: \"true\""),
        "D-06 / UI-SPEC §8.1: card.rs must declare `draggable: \"true\"`"
    );
}

#[test]
fn card_wires_ondragstart_and_ondragend() {
    let src = read("src/components/hermes_app/screens/kanban/card.rs");
    assert!(
        src.contains("ondragstart"),
        "D-06: card.rs must wire ondragstart (drag source)"
    );
    assert!(
        src.contains("ondragend"),
        "D-06: card.rs must wire ondragend (drag-end cleanup)"
    );
}

#[test]
fn card_carries_focus_restore_id() {
    let src = read("src/components/hermes_app/screens/kanban/card.rs");
    assert!(
        src.contains("kn-card-action-"),
        "Plan 03 dependency: card must carry id prefix \
         `kn-card-action-<task_id>` so the drawer-close focus restore works."
    );
}

// ---------------------------------------------------------------------------
// column.rs — drop zone + D-06 prevent_default contract
// ---------------------------------------------------------------------------

#[test]
fn column_wires_ondragover_and_ondrop() {
    let src = read("src/components/hermes_app/screens/kanban/column.rs");
    assert!(
        src.contains("ondragover"),
        "D-06: column.rs must wire ondragover (so ondrop can fire)"
    );
    assert!(
        src.contains("ondrop"),
        "D-06: column.rs must wire ondrop (the actual drop handler)"
    );
}

#[test]
fn column_calls_event_prevent_default_on_outer_event() {
    let src = read("src/components/hermes_app/screens/kanban/column.rs");
    // Risk 4: the call MUST be on the outer Event<DragData> wrapper, NOT
    // on DragData directly. We assert the literal `event.prevent_default()`
    // substring and reject the inner-DragData variant.
    assert!(
        src.contains("event.prevent_default()"),
        "Risk 4 / D-06: column.rs must call `event.prevent_default()` \
         on the outer Event<DragData> wrapper in ondragover (without this \
         ondrop never fires)"
    );
}

#[test]
fn column_handler_takes_event_drag_data_typed_arg() {
    let src = read("src/components/hermes_app/screens/kanban/column.rs");
    // The closure signature must spell out `Event<DragData>` so the
    // prevent_default method dispatches to the outer wrapper.
    assert!(
        src.contains("Event<DragData>"),
        "Risk 4: column.rs ondragover closure must be typed \
         `move |event: Event<DragData>|` (not `move |event: DragData|`)"
    );
}

#[test]
fn column_does_not_unwrap_inner_drag_data_for_prevent_default() {
    let src = read("src/components/hermes_app/screens/kanban/column.rs");
    // Forbid the inner-DragData prevent_default pattern (dyn_into etc.)
    // — there is no legitimate reason to call prevent_default on the
    // inner DragData type.
    assert!(
        !src.contains(".dyn_into::<DragData>"),
        "Risk 4: column.rs must NOT call dyn_into::<DragData>() — \
         prevent_default lives on the outer Event<DragData> wrapper."
    );
}

#[test]
fn column_or_board_reach_is_drag_allowed() {
    let column = read("src/components/hermes_app/screens/kanban/column.rs");
    let board = read("src/components/hermes_app/screens/kanban/board.rs");
    let reachable = column.contains("is_drag_allowed") || board.contains("is_drag_allowed");
    assert!(
        reachable,
        "D-14 client copy: column.rs or board.rs must call `is_drag_allowed` \
         (proves the client-side validator is reachable from the drop handler)"
    );
}

// ---------------------------------------------------------------------------
// kanban.rs — live-region announcement infrastructure
// ---------------------------------------------------------------------------

#[test]
fn kanban_screen_renders_a_live_region() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    assert!(
        src.contains("role: \"log\""),
        "UI-SPEC §6.5: ScreenKanban must render a `role: \"log\"` \
         live-region for board-update announcements"
    );
    assert!(
        src.contains("aria_live: \"polite\""),
        "UI-SPEC §6.5: live-region must declare `aria_live: \"polite\"`"
    );
}
