//! Phase 36.3.7.11 Plan 05 — WebSocket lifecycle integration test for the
//! kanban dashboard tail consumer.
//!
//! This file is the analog of `tests/websocket_lifecycle_parity.rs` for the
//! `/api/ws/kanban` surface introduced in Plan 01. It uses the same
//! source-string assertion pattern (iron_hermes_ui is bin-only, so we cannot
//! `use iron_hermes_ui::*` from an integration test).
//!
//! Locks (must-haves enumerated in the Plan 05 frontmatter):
//! - **D-08 WS lifecycle parity:** `WS_KEEPALIVE_INTERVAL = 5s`, keepalive
//!   driven by `tokio::time::interval`, `send_close_frame` helper present,
//!   `#[get("/api/ws/kanban")]` route annotation.
//! - **D-15 tail independence:** the tail loop uses
//!   `KanbanStore::list_all_events_after` (the Wave 0 helper) and does NOT
//!   import `ironhermes_kanban::notifier`.
//! - **D-05 no REST mount:** the WS file does not declare `/api/plugins/kanban`.
//!
//! These assertions extend the Plan 01 `kanban_ws_tail.rs` stub by adding
//! the keepalive-driver and close-frame-helper truths and by sharpening the
//! D-05 boundary. The Plan 01 file remains in place as a Wave-0-style basic
//! lock; this Plan 05 file is the full integration analog.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

/// Strip line-comment prefixes (`//`, `///`, `//!`) so we can assert "the
/// CODE does not contain X" without false-positives from doc comments that
/// LITERALLY name the prohibition (e.g. "no `use ironhermes_kanban::notifier`
/// import").
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!"))
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

#[test]
fn kanban_ws_emits_application_level_keepalive_ping() {
    // D-08 lifecycle parity: WS_KEEPALIVE_INTERVAL + 5s + tokio::time::interval
    // + send_close_frame helper + /api/ws/kanban route + list_all_events_after
    // tail consumer. Single test, multiple anchors — mirrors the
    // websocket_lifecycle_parity.rs `server_ws_emits_application_level_keepalive_ping`
    // multi-anchor pattern.
    let src = read("src/server/kanban_ws.rs");

    assert!(
        src.contains("WS_KEEPALIVE_INTERVAL"),
        "D-08: kanban_ws.rs must declare a named WS_KEEPALIVE_INTERVAL constant"
    );

    assert!(
        src.contains("Duration::from_secs(5)"),
        "D-08: keepalive must default to 5s (< 10s proxy idle threshold; \
         matches /api/ws/chat)"
    );

    assert!(
        src.contains("tokio::time::interval(WS_KEEPALIVE_INTERVAL)"),
        "D-08: keepalive must drive cadence via tokio::time::interval"
    );

    assert!(
        src.contains("fn send_close_frame("),
        "D-08: kanban_ws.rs must funnel close-frame emission through a helper"
    );

    assert!(
        src.contains("list_all_events_after"),
        "Plan 01 contract: tail loop must call KanbanStore::list_all_events_after"
    );
}

#[test]
fn kanban_ws_route_is_kanban_not_plugins() {
    // D-05: dashboard surface is Dioxus #[server] fns + the single WS route.
    // There is no /api/plugins/kanban mount on this file. The route annotation
    // is /api/ws/kanban.
    let src = read("src/server/kanban_ws.rs");

    assert!(
        src.contains("#[get(\"/api/ws/kanban\")]"),
        "D-08: kanban_ws.rs must mount /api/ws/kanban"
    );

    let code = code_only(&src);
    assert!(
        !code.contains("/api/plugins/kanban"),
        "D-05: kanban_ws.rs must NOT reference /api/plugins/kanban — the \
         dashboard surface is Dioxus #[server] fns + the single /api/ws/kanban \
         route, never a parallel REST mount"
    );
}

#[test]
fn kanban_tail_consumer_uses_list_all_events_after_not_notifier() {
    // D-15: dashboard tail is INDEPENDENT of the 36.3.7.5 gateway notifier.
    // It uses the Wave 0 helper `list_all_events_after`. It does NOT import
    // `ironhermes_kanban::notifier` (no shared primitive, no notifier
    // refactor). Comments documenting the prohibition are allowed; actual
    // `use` statements are not.
    let src = read("src/server/kanban_ws.rs");

    assert!(
        src.contains("fn run_kanban_tail_loop"),
        "kanban_ws.rs must expose run_kanban_tail_loop"
    );

    assert!(
        src.contains("list_all_events_after"),
        "D-15: tail loop must call KanbanStore::list_all_events_after (Wave 0)"
    );

    let code = code_only(&src);
    assert!(
        !code.contains("use ironhermes_kanban::notifier"),
        "D-15: the dashboard tail must NOT import ironhermes_kanban::notifier; \
         the two tail consumers are independent"
    );
}

#[test]
fn kanban_ws_handler_uses_close_frame_helper() {
    // D-08: every teardown branch funnels through `send_close_frame` so
    // proxies/clients never observe `Connection reset without closing
    // handshake`. The helper itself is defined in the file (asserted in
    // `kanban_ws_emits_application_level_keepalive_ping`); here we lock
    // that it is actually CALLED — at minimum once per teardown branch.
    let src = read("src/server/kanban_ws.rs");

    let calls = src.matches("send_close_frame(").count();
    // Definition (counts as 1) + 4 call sites (clean recv, broken recv,
    // broken send, keepalive failure) = 5. Mirrors the same threshold used
    // by tests/websocket_lifecycle_parity.rs::
    // server_ws_emits_close_frame_on_every_teardown_branch.
    assert!(
        calls >= 5,
        "D-08: expected send_close_frame to be invoked at every teardown branch \
         (clean recv close, broken recv, broken send, keepalive failure); \
         found {calls} occurrence(s)"
    );
}
