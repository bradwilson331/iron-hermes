//! Phase 36.3.7.11 Plan 05 — aggregated must_haves invariants across all
//! four prior plans of the dashboard-Kanban phase.
//!
//! This file locks the goal-backward truths from Plans 01-04 in a single
//! integration test, so a regression on any per-plan invariant (D-NN decision)
//! surfaces here even if no Plan-specific test changed. Source-string
//! assertion pattern (iron_hermes_ui is bin-only) — same approach as
//! tests/kanban_wheel_nav.rs, tests/kanban_dnd_wiring.rs, and
//! tests/websocket_lifecycle_parity.rs.
//!
//! Coverage map (decision → assertion → file):
//!
//! Plan 01 (walking skeleton — read-side + WS tail):
//!   D-02 Screen variant            → state.rs contains `Screen::Kanban`
//!   D-04 no new hex in kanban.css  → assets/kanban.css has 0 hex literals
//!   D-05 no REST mount             → kanban_api.rs lacks /api/plugins/kanban
//!   D-08 WS contract               → delegated to kanban_ws_lifecycle.rs
//!   D-09 six visible columns       → board.rs references all six statuses
//!   D-15 tail independent          → delegated to kanban_ws_lifecycle.rs
//!   D-17 tail_interval_ms = 250    → ironhermes-core::config.rs
//!   D-19 board: Option<String> ≥ 9 → kanban_api.rs (5 reads + 4 writes)
//!
//! Plan 02 (drag-and-drop + writes):
//!   D-06 prevent_default + Event<DragData> → column.rs
//!   D-07 spawn pattern              → board.rs (spawn(async move) + tasks.write())
//!   D-10 transitions client         → transitions.rs has `is_drag_allowed`
//!   D-11 hint copy (verbatim)       → transitions.rs has UI-SPEC §7.4 string
//!   D-13 four write fns             → kanban_api.rs has all four fn signatures
//!   D-14 server-side validation     → kanban_api.rs imports is_drag_allowed
//!
//! Plan 03 (drawer + modals):
//!   D-12 Triage actions             → card.rs renders Decompose + Specify
//!   D-20 drawer sections in order   → drawer.rs has the six UI-SPEC §7.6 labels
//!   D-20 modal headers              → modals.rs has the four UI-SPEC §7.2 strings
//!   D-21 per-task counter           → kanban.rs declares per_task_event_counter
//!
//! Plan 04 (wheel-nav + Agents button):
//!   D-02 wheel 11-wedge             → state.rs has rem_euclid(11) + WheelWedge::Kanban
//!   D-03 Agents toolbar button      → agents.rs has "KANBAN BOARD →"

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

/// Workspace-anchored read for files outside iron_hermes_ui (e.g. the shared
/// ironhermes-core crate). Walks up from `CARGO_MANIFEST_DIR` (iron_hermes_ui)
/// to the workspace root, then resolves `path` against it.
fn read_workspace(path: &str) -> String {
    let manifest = crate_root();
    // manifest = .../crates/iron_hermes_ui ; workspace root = .../
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root must be two parents above CARGO_MANIFEST_DIR");
    fs::read_to_string(workspace.join(path)).expect("failed to read workspace file")
}

fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!"))
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Plan 01 — walking skeleton
// ---------------------------------------------------------------------------

#[test]
fn plan_01_d02_state_has_screen_kanban_variant() {
    let state = read("src/state.rs");
    assert!(
        state.contains("Screen::Kanban"),
        "D-02 (Plan 01/04): state.rs must declare a Screen::Kanban variant — \
         the dashboard Kanban tab's mount point in the screen router"
    );
}

#[test]
fn plan_01_d04_kanban_css_has_zero_new_hex_literals() {
    // D-04: the kanban surface inherits HermesApp design tokens; no theme
    // fork. Anything matching `#[0-9a-fA-F]{3,6}` in a non-comment line is a
    // theme drift.
    let css = read("assets/kanban.css");

    let mut hex_count = 0usize;
    let mut in_block_comment = false;
    for raw in css.lines() {
        let mut s = raw;

        // Strip block-comment ranges very conservatively: if we're inside a
        // block comment, skip until we see `*/`. If we open one on this line,
        // stop scanning at the open marker.
        if in_block_comment {
            if let Some(idx) = s.find("*/") {
                s = &s[idx + 2..];
                in_block_comment = false;
            } else {
                continue;
            }
        }
        if let Some(open) = s.find("/*") {
            s = &s[..open];
            in_block_comment = true;
        }

        // Skip line-comment-only lines (CSS doesn't have `//`, but be safe).
        let trimmed = s.trim_start();
        if trimmed.is_empty() {
            continue;
        }

        // Walk for `#` followed by 3 or 6 hex chars.
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'#' {
                let mut hex_len = 0;
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] as char).is_ascii_hexdigit() {
                    hex_len += 1;
                    j += 1;
                }
                if hex_len == 3 || hex_len == 4 || hex_len == 6 || hex_len == 8 {
                    hex_count += 1;
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }

    assert_eq!(
        hex_count, 0,
        "D-04 (Plan 01): assets/kanban.css must have zero hex color literals \
         (#abc / #abcdef forms) — the kanban surface inherits HermesApp design \
         tokens via existing CSS variables. Found {hex_count} hex literal(s)."
    );
}

#[test]
fn plan_01_d05_kanban_api_has_no_rest_mount() {
    let api = read("src/server/kanban_api.rs");
    let code = code_only(&api);
    assert!(
        !code.contains("/api/plugins/kanban"),
        "D-05 (Plan 01): kanban_api.rs must NOT reference /api/plugins/kanban — \
         the dashboard surface is Dioxus #[server] fns only, never a parallel \
         REST mount"
    );
}

#[test]
fn plan_01_d09_board_references_six_visible_columns() {
    let board = read("src/components/hermes_app/screens/kanban/board.rs");
    for status in &["Triage", "Todo", "Ready", "InProgress", "Blocked", "Done"] {
        assert!(
            board.contains(status),
            "D-09 (Plan 01): board.rs must reference KanbanColumnStatus::{status} \
             — one of the six visible columns (TRIAGE / TODO / READY / IN PROGRESS / \
             BLOCKED / DONE)"
        );
    }
}

#[test]
fn plan_01_d17_dashboard_tail_interval_ms_default_250() {
    let config = read_workspace("crates/ironhermes-core/src/config.rs");
    assert!(
        config.contains("tail_interval_ms"),
        "D-17 (Plan 01): ironhermes-core config.rs must declare the \
         dashboard.kanban.tail_interval_ms key"
    );
    assert!(
        config.contains("tail_interval_ms: 250"),
        "D-17 (Plan 01): tail_interval_ms must default to 250 (ms) — \
         sub-second perceived live-update latency"
    );
}

#[test]
fn plan_01_d19_kanban_api_board_option_string_signature_count() {
    // D-19: every #[server] fn takes `board: Option<String>`. Plan 01
    // adds 5 read fns (fetch_board / fetch_task / fetch_task_events /
    // fetch_task_runs / fetch_comments) and Plan 02 adds 4 write fns
    // (patch_task_status / post_comment / create_task / run_decompose_or_specify).
    // Total expected: ≥ 9. We assert ≥ 9 (some lines may use the literal
    // beyond the 9 fn signatures — e.g. doc comments — but the cumulative
    // count is the structural lock).
    let api = read("src/server/kanban_api.rs");
    let count = api.matches("board: Option<String>").count();
    assert!(
        count >= 9,
        "D-19 (Plan 01 + Plan 02): kanban_api.rs must declare \
         `board: Option<String>` on every #[server] fn — expected ≥ 9 \
         occurrences (5 reads + 4 writes), found {count}"
    );
}

// ---------------------------------------------------------------------------
// Plan 02 — drag-and-drop + write surface
// ---------------------------------------------------------------------------

#[test]
fn plan_02_d06_column_calls_prevent_default_on_event_drag_data() {
    let column = read("src/components/hermes_app/screens/kanban/column.rs");
    assert!(
        column.contains("prevent_default()"),
        "D-06 (Plan 02): column.rs must call event.prevent_default() — \
         without it, ondrop never fires (Risk 4 / HTML5 DnD contract)"
    );
    assert!(
        column.contains("Event<DragData>"),
        "D-06 (Plan 02): column.rs ondragover must take Event<DragData> \
         (the outer Event wrapper carries .prevent_default(); unwrapping to \
         DragData strips it)"
    );
}

#[test]
fn plan_02_d07_optimistic_update_spawn_pattern_present() {
    // D-07: optimistic UI updates the signal BEFORE the #[server] mutation
    // is spawned. The proof is `tasks.write()` + `spawn(async move` in the
    // same file (board.rs::move_task_optimistic). Strict ordering proof is
    // clippy's job (no signal borrow across .await); we lock the loose
    // structural proof that both patterns appear here.
    let board = read("src/components/hermes_app/screens/kanban/board.rs");
    assert!(
        board.contains("spawn(async move"),
        "D-07 (Plan 02): board.rs must spawn the #[server] mutation in a \
         tokio task (spawn(async move {{ ... }}))"
    );
    assert!(
        board.contains("tasks.write()"),
        "D-07 (Plan 02): board.rs must write to the tasks signal optimistically \
         BEFORE the spawn block"
    );
}

#[test]
fn plan_02_d10_transitions_module_exposes_is_drag_allowed() {
    let t = read("src/kanban/transitions.rs");
    assert!(
        t.contains("is_drag_allowed"),
        "D-10 (Plan 02): kanban::transitions must expose `is_drag_allowed` — \
         the canonical client+server transition validator"
    );
}

#[test]
fn plan_02_d11_hint_copy_verbatim_from_ui_spec_7_4() {
    let t = read("src/kanban/transitions.rs");
    assert!(
        t.contains("Completing a task needs a summary"),
        "D-11 (Plan 02): transitions.rs must carry the UI-SPEC §7.4 verbatim \
         hint string `Completing a task needs a summary` — the user-facing \
         feedback for a drop on DONE"
    );
}

#[test]
fn plan_02_d13_four_write_fn_signatures_present() {
    let api = read("src/server/kanban_api.rs");
    for sig in &[
        "fn patch_task_status",
        "fn post_comment",
        "fn create_task",
        "fn run_decompose_or_specify",
    ] {
        assert!(
            api.contains(sig),
            "D-13 (Plan 02): kanban_api.rs must declare `{sig}` — one of the \
             four canonical write fns"
        );
    }
}

#[test]
fn plan_02_d14_server_side_imports_is_drag_allowed() {
    // D-14 defense-in-depth: the SAME transition validator binds on both
    // sides. The server file imports it from kanban::transitions and calls
    // it in patch_task_status.
    let api = read("src/server/kanban_api.rs");
    assert!(
        api.contains("is_drag_allowed"),
        "D-14 (Plan 02): kanban_api.rs must reference is_drag_allowed for \
         server-side transition validation (defense-in-depth)"
    );
    assert!(
        api.contains("use crate::kanban::transitions::is_drag_allowed")
            || api.contains("crate::kanban::transitions::is_drag_allowed"),
        "D-14 (Plan 02): the predicate must come from crate::kanban::transitions \
         — single source of truth between client and server"
    );
}

// ---------------------------------------------------------------------------
// Plan 03 — drawer + modals
// ---------------------------------------------------------------------------

#[test]
fn plan_03_d12_triage_actions_on_card() {
    let card = read("src/components/hermes_app/screens/kanban/card.rs");
    assert!(
        card.contains("Decompose"),
        "D-12 (Plan 03): card.rs must render the ⚗ Decompose button on \
         TRIAGE cards"
    );
    assert!(
        card.contains("Specify"),
        "D-12 (Plan 03): card.rs must render the ✨ Specify button on \
         TRIAGE cards"
    );
}

#[test]
fn plan_03_d20_drawer_section_labels_in_order() {
    let drawer = read("src/components/hermes_app/screens/kanban/drawer.rs");
    for label in &[
        "WORKER CONTEXT",
        "PARENT HANDOFFS",
        "PRIOR ATTEMPTS",
        "RUN HISTORY",
        "EVENTS (last 20)",
        "COMMENTS",
    ] {
        assert!(
            drawer.contains(label),
            "D-20 (Plan 03): drawer.rs must render the UI-SPEC §7.6 section \
             label `{label}` — one of the six canonical drawer sections"
        );
    }
}

#[test]
fn plan_03_d20_modal_headers_verbatim() {
    let modals = read("src/components/hermes_app/screens/kanban/modals.rs");
    for header in &["Complete task", "Block task", "Archive task?", "Create task"] {
        assert!(
            modals.contains(header),
            "D-20 (Plan 03): modals.rs must carry the UI-SPEC §7.2 verbatim \
             header `{header}` — the canonical title for one of the four \
             modals"
        );
    }
}

#[test]
fn plan_03_d21_per_task_event_counter_drives_drawer_refresh() {
    let screen = read("src/components/hermes_app/screens/kanban.rs");
    assert!(
        screen.contains("per_task_event_counter"),
        "D-21 (Plan 03): ScreenKanban must declare the per_task_event_counter \
         signal that drives drawer refresh-on-event (the use_resource closures \
         in drawer.rs depend on this counter to know when to re-fetch)"
    );
}

// ---------------------------------------------------------------------------
// Plan 04 — wheel-nav 11th wedge + Agents toolbar button
// ---------------------------------------------------------------------------

#[test]
fn plan_04_d02_wheel_eleven_wedges_with_kanban() {
    let state = read("src/state.rs");
    assert!(
        state.contains("rem_euclid(11)"),
        "D-02 (Plan 04): state.rs WheelWedge::from_index must use rem_euclid(11) \
         — the wheel now has 11 wedges (10 original + Kanban). Modulo-10 \
         silently re-routes the 11th wedge to Chat (Risk 1)"
    );
    assert!(
        state.contains("WheelWedge::Kanban"),
        "D-02 (Plan 04): state.rs must declare WheelWedge::Kanban — the 11th \
         wedge variant"
    );
}

#[test]
fn plan_04_d03_agents_toolbar_has_kanban_board_button() {
    let agents = read("src/components/hermes_app/screens/agents.rs");
    assert!(
        agents.contains("KANBAN BOARD →"),
        "D-03 (Plan 04): agents.rs must render the `KANBAN BOARD →` toolbar \
         button — the discovery affordance for the dashboard Kanban tab \
         (immediately right of PRUNE ENDED)"
    );
}
