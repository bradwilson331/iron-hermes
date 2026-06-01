//! Phase 36.3.7.11 Plan 01 (D-02 / D-09) — Kanban dashboard screen.
//!
//! Walking skeleton: fetches the board via `fetch_board(None)` (D-18 — None
//! resolves to default board), opens a WebSocket to `/api/ws/kanban` to
//! receive live `KanbanWsEvent::TaskEventBatch` pushes from the dashboard
//! tail consumer, and calls `board_resource.restart()` on every event (D-08
//! minimal Plan 01 behavior — Plan 02 will swap to delta-apply).
//!
//! Plans 02 / 03 / 04 layer on drag-and-drop + detail drawer + wheel-nav
//! 11th-wedge wiring on top of this screen.

// Submodules in the sibling `kanban/` directory — board/column/card form
// the visual shell. Drawer + modals land in Plan 03.
pub mod board;
pub mod card;
pub mod column;

use crate::components::hermes_app::screens::kanban::board::KanbanBoard;
use crate::protocol::TaskRow;
use dioxus::prelude::*;

/// Phase 36.3.7.11 Plan 01: stylesheet for the kanban dashboard. Defines
/// the `.kn-card` cyan-glow rules, `.kn-board` layout, chip styles, and
/// `@media (prefers-reduced-motion: reduce)` overrides (UI-SPEC §6.6).
const KANBAN_CSS: Asset = asset!("/assets/kanban.css");

/// Phase 36.3.7.11 Plan 01: WS lifecycle state — drives a small status
/// dot in the toolbar. Plan 01 keeps the indicator minimal; richer
/// reconnect surfacing lands in Plan 04.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WsState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

/// Phase 36.3.7.11 Plan 01: kanban dashboard screen.
///
/// Renders six columns (D-09), an archive-toggle toolbar button, and a
/// WS status indicator. Subscribes to `/api/ws/kanban` and re-fetches
/// the board on every TaskEventBatch (Plan 02 will switch to delta-apply).
#[component]
pub fn ScreenKanban(is_active: bool) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).

    // Board fetch resource.
    let mut board_resource =
        use_resource(move || async move { crate::server::kanban_api::fetch_board(None).await });

    // Local Signal<Vec<TaskRow>> — Plan 02 will use this for optimistic
    // updates. Plan 01 mirrors board_resource via the use_effect below.
    let mut tasks: Signal<Vec<TaskRow>> = use_signal(Vec::<TaskRow>::new);

    // Reserved for Plan 02 (drag source) — declared so the signal is in
    // scope and the hook ordering is stable across plans.
    let _dragged_task_id: Signal<Option<String>> = use_signal(|| None);

    // Reserved for Plan 03 (drawer open state).
    let mut open_drawer_task_id: Signal<Option<String>> = use_signal(|| None);

    // Archive toggle — drives the 7th column visibility (D-09).
    let mut archived_visible: Signal<bool> = use_signal(|| false);

    // Reserved for Plan 02 toast surface.
    let _toast_msg: Signal<Option<String>> = use_signal(|| None);

    // WS connection state indicator.
    let mut ws_state: Signal<WsState> = use_signal(|| WsState::Connecting);

    // Sync the board_resource Ok value into tasks. This must run on every
    // render so a successful re-fetch propagates into the column children.
    // Signal-borrow safety: `.set()` is a value-copy operation — no
    // borrow held across .await.
    use_effect(move || {
        if let Some(Ok(rows)) = board_resource.value()() {
            tasks.set(rows);
        }
    });

    // WS client — opens to `/api/ws/kanban` and calls
    // `board_resource.restart()` on every TaskEventBatch (D-08 minimal
    // Plan 01 behavior; Plan 02 swaps to delta-apply). Mirrors the
    // canonical `/api/ws/chat` pattern from hermes_app/mod.rs lines 160-168.
    let mut ws = dioxus_fullstack::use_websocket(move || {
        crate::server::kanban_ws::ws_kanban(
            dioxus_fullstack::WebSocketOptions::new().with_automatic_reconnect(),
        )
    });

    use_future(move || async move {
        loop {
            let _state = ws.connect().await;
            if ws.is_err() {
                ws_state.set(WsState::Reconnecting);
                continue;
            }
            ws_state.set(WsState::Connected);
            loop {
                match ws.recv_raw().await {
                    Ok(dioxus_fullstack::Message::Text(t)) => {
                        // Parse a KanbanWsEvent. Malformed frames are
                        // silently skipped so a single bad payload does
                        // not break the stream.
                        let event: crate::protocol::KanbanWsEvent =
                            match serde_json::from_str(&t) {
                                Ok(e) => e,
                                Err(_) => continue,
                            };
                        match event {
                            crate::protocol::KanbanWsEvent::TaskEventBatch { .. } => {
                                // Plan 01: full re-fetch on any event. Plan
                                // 02 will swap to delta-apply per Q11.
                                board_resource.restart();
                            }
                            crate::protocol::KanbanWsEvent::Error { message } => {
                                #[cfg(target_arch = "wasm32")]
                                web_sys::console::log_1(
                                    &format!("[kanban-ws] error: {message}").into(),
                                );
                                let _ = message;
                            }
                            crate::protocol::KanbanWsEvent::Ping {} => {
                                // Server liveness — no client action needed.
                            }
                        }
                    }
                    Ok(dioxus_fullstack::Message::Close { .. }) => {
                        ws_state.set(WsState::Disconnected);
                        break;
                    }
                    Err(_) => {
                        ws_state.set(WsState::Reconnecting);
                        break;
                    }
                    // Skip Ping/Pong/Binary silently.
                    Ok(_) => continue,
                }
            }
        }
    });

    // Materialize loading / error states for the rendering branch.
    let is_loading = board_resource.value()().is_none();
    let has_error = matches!(board_resource.value()(), Some(Err(_)));

    let ws_state_class = match *ws_state.read() {
        WsState::Connecting => "is-connecting",
        WsState::Connected => "is-connected",
        WsState::Reconnecting => "is-reconnecting",
        WsState::Disconnected => "is-disconnected",
    };

    let archived_visible_ro: ReadSignal<bool> = archived_visible.into();
    let tasks_ro: ReadSignal<Vec<TaskRow>> = tasks.into();

    let on_open_drawer = move |task_id: String| {
        // Plan 03 wires the drawer; Plan 01 just records the id so the
        // hook is in place + the click is observable.
        open_drawer_task_id.set(Some(task_id));
    };

    rsx! {
        section {
            class: if is_active { "screen is-active" } else { "screen" },
            id: "screen-kanban",
            "data-screen-label": "// MODULE 10 — KANBAN",
            document::Link { rel: "stylesheet", href: KANBAN_CSS }
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 10" }
                    h1 { class: "screen-title", "Kanban" }
                }
                div { class: "screen-actions",
                    span {
                        class: "kn-ws-status {ws_state_class}",
                        "aria-label": "WebSocket status",
                        "•"
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            let cur = *archived_visible.read();
                            archived_visible.set(!cur);
                        },
                        if *archived_visible.read() { "HIDE ARCHIVED" } else { "SHOW ARCHIVED" }
                    }
                }
            }
            if has_error {
                div { class: "kn-error",
                    "Failed to load board. Retrying via WebSocket reconnect…"
                }
            } else if is_loading {
                div { class: "kn-loading", "Loading board…" }
            } else {
                KanbanBoard {
                    tasks: tasks_ro,
                    archived_visible: archived_visible_ro,
                    on_open_drawer: on_open_drawer,
                }
            }
        }
    }
}
