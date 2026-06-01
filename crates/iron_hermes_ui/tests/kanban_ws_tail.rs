//! Phase 36.3.7.11 Plan 01 Wave 1 — kanban_ws.rs source-string assertions.
//!
//! Locks:
//! - D-08: `WS_KEEPALIVE_INTERVAL = Duration::from_secs(5)` + close-frame on
//!   teardown + `/api/ws/kanban` route annotation.
//! - D-15: tail consumer is INDEPENDENT of the gateway notifier — no
//!   `use ironhermes_kanban::notifier` import allowed.
//! - Tail loop uses `list_all_events_after` (Wave 0 helper).
//!
//! Pattern: source-read assertion (mirrors tests/websocket_lifecycle_parity.rs).

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn kanban_ws_uses_5s_keepalive_interval() {
    let src = read("src/server/kanban_ws.rs");
    assert!(
        src.contains("WS_KEEPALIVE_INTERVAL"),
        "D-08: kanban_ws.rs must declare WS_KEEPALIVE_INTERVAL"
    );
    assert!(
        src.contains("Duration::from_secs(5)"),
        "D-08: keepalive must be 5 seconds (matches /api/ws/chat)"
    );
    assert!(
        src.contains("tokio::time::interval(WS_KEEPALIVE_INTERVAL)"),
        "D-08: keepalive must drive cadence via tokio::time::interval"
    );
}

#[test]
fn kanban_ws_route_annotation_is_api_ws_kanban() {
    let src = read("src/server/kanban_ws.rs");
    assert!(
        src.contains("#[get(\"/api/ws/kanban\")]"),
        "D-08: kanban_ws.rs must mount /api/ws/kanban"
    );
}

#[test]
fn kanban_ws_emits_close_frame_on_teardown() {
    let src = read("src/server/kanban_ws.rs");
    assert!(
        src.contains("fn send_close_frame("),
        "D-08: kanban_ws.rs must funnel close-frame emission through a helper"
    );
    assert!(
        src.contains("Message::Close {"),
        "D-08: send_close_frame must emit a WebSocket Close variant"
    );
    assert!(
        src.contains("CloseCode::Normal") && src.contains("CloseCode::Away"),
        "D-08: teardown branches must classify close codes (Normal/Away)"
    );
}

#[test]
fn kanban_ws_tail_loop_uses_list_all_events_after() {
    let src = read("src/server/kanban_ws.rs");
    assert!(
        src.contains("list_all_events_after"),
        "Tail loop must call KanbanStore::list_all_events_after (Wave 0 helper)"
    );
    assert!(
        src.contains("fn run_kanban_tail_loop"),
        "kanban_ws.rs must expose run_kanban_tail_loop"
    );
}

#[test]
fn kanban_ws_tail_independence_no_notifier_import() {
    // D-15 enforcement: the dashboard tail is independent of the 36.3.7.5
    // gateway notifier — no shared primitive, no notifier refactor. This
    // assertion fails the moment someone adds an actual `use` statement
    // for `ironhermes_kanban::notifier`.
    //
    // We inspect non-comment lines only: doc comments documenting the
    // D-15 independence may LITERALLY mention `use ironhermes_kanban::notifier`
    // (e.g. "must not contain `use ironhermes_kanban::notifier`"). Stripping
    // line-comment prefixes `//` and `///` keeps the invariant tight without
    // false-positives.
    let src = read("src/server/kanban_ws.rs");
    let code_only: String = src
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("//") || trimmed.starts_with("///"))
        })
        .collect::<Vec<&str>>()
        .join("\n");
    assert!(
        !code_only.contains("use ironhermes_kanban::notifier"),
        "D-15: dashboard tail must NOT import ironhermes_kanban::notifier; \
         the two tail consumers are independent"
    );
}

#[test]
fn kanban_ws_broadcasts_via_kanban_tail_broadcast_subscribe() {
    let src = read("src/server/kanban_ws.rs");
    assert!(
        src.contains("kanban_tail_broadcast.subscribe()"),
        "WS handler must subscribe to AppState::kanban_tail_broadcast"
    );
}
