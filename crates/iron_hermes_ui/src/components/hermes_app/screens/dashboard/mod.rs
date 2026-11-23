//! Phase 54.10 Plan 01 (D-01/D-03) — the Dashboard screen frame.
//!
//! Boards live server-side per profile (`server::dashboard_api`); this screen never reads or
//! writes them anywhere else (D-03: no localStorage). Later plans add the canvas; this frame
//! holds the header band, the board select and the no-boards state.

mod add_menu;
mod boards;
mod canvas;
mod items;
mod palette;
mod plan;
mod present;
mod tiles;
mod widgets;

pub(crate) use widgets::widget_frame_id;
use widgets::{ActivationEvent, activation_after};

use crate::canvas_math::{CanvasCmd, View};
use crate::components::hermes_app::dashboard_runtime::{DashboardGenCtx, GenJob, GenKind};
use crate::components::hermes_app::screens::openui::{
    OpenUiAction, action_turn_message, post_theme_to_frame_id,
};
use crate::server::dashboard_api::{
    BOARD_ITEMS_FULL_ERROR, BOARD_ITEMS_MAX, BOARD_NAME_ERROR, BOARD_NAME_MAX_CHARS,
    BOARD_SCHEMA_VERSION, Board, list_dashboard_boards, save_dashboard_board,
};
use crate::server::openui_views_api::list_openui_views;
use add_menu::{AddChoice, AddMenu, TRIGGER_ID};
use boards::{BOARD_MENU_ID, BoardMenu, NewBoardButton, PresetChooser, PresetNotice, RenameField};
use canvas::{
    DashboardCanvas, ResizeListener, ZoomGroup, add_item, install_resize, is_mac_platform,
    remove_resize, use_board_state, window_inner_width,
};
use dioxus::core::{spawn_forever, use_drop};
use dioxus::prelude::*;
use palette::{Palette, open_palette};
use plan::{PLAN_TRIGGER_ID, PlanPanel};
use present::{PRESENT_TRIGGER_ID, start_present};

/// UI-SPEC `.screen-status`: `· {board name} · {n} items`, singular `1 item`.
pub(crate) fn status_line(name: &str, items: usize) -> String {
    let noun = if items == 1 { "item" } else { "items" };
    format!("· {name} · {items} {noun}")
}

/// UI-SPEC announcement when a board opens: `Board {name} opened, {n} items.` (singular `1 item`).
pub(crate) fn board_opened_announcement(name: &str, items: usize) -> String {
    let noun = if items == 1 { "item" } else { "items" };
    format!("Board {name} opened, {items} {noun}.")
}

/// `(id, name)` per board, in list order, for the board select.
pub(crate) fn board_options(boards: &[Board]) -> Vec<(String, String)> {
    boards
        .iter()
        .map(|b| (b.id.clone(), b.name.clone()))
        .collect()
}

/// The board to show: the one `current` names, else the first listed.
pub(crate) fn selected_board<'a>(boards: &'a [Board], current: Option<&str>) -> Option<&'a Board> {
    current
        .and_then(|id| boards.iter().find(|b| b.id == id))
        .or_else(|| boards.first())
}

/// UI-SPEC E1: boards load failure copy.
pub(crate) fn load_error_copy(reason: &str) -> String {
    format!("Could not load boards: {reason}. Check server logs, then reload.")
}

/// UI-SPEC E9: a failed save keeps the prior state and says so.
const SAVE_FAILED: &str =
    "Save failed. Check server logs. Your changes stay in this window until you reload.";

/// Autosave state shown at the toolbar right (UI-SPEC A13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveStatus {
    Idle,
    Saving,
    Saved,
    Failed,
}

/// The status line text and its colour class, or `None` while idle.
pub(crate) fn save_status_line(s: SaveStatus) -> Option<(&'static str, &'static str)> {
    match s {
        SaveStatus::Idle => None,
        SaveStatus::Saving => Some(("Saving…", "ih-save--saving")),
        SaveStatus::Saved => Some(("Saved.", "ih-save--saved")),
        SaveStatus::Failed => Some((SAVE_FAILED, "ih-save--failed")),
    }
}

/// A new empty board. The server stamps the times.
pub(crate) fn new_board(name: &str) -> Board {
    Board {
        version: BOARD_SCHEMA_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        name: name.trim().to_string(),
        created_at: 0,
        updated_at: 0,
        items: vec![],
    }
}

/// The name error for a Create press, if any (UI-SPEC E9).
pub(crate) fn name_problem(name: &str) -> Option<&'static str> {
    let n = name.trim().chars().count();
    (!(1..=BOARD_NAME_MAX_CHARS).contains(&n)).then_some(BOARD_NAME_ERROR)
}

/// Write the board to the server and report the outcome on the status line. Returns whether
/// the write landed: callers advance their saved revision only on success (so a failed save is
/// retried) and keep the working copy open on failure.
async fn save_board(board: Board, mut status: Signal<SaveStatus>) -> bool {
    status.set(SaveStatus::Saving);
    let ok = save_dashboard_board(board).await.is_ok();
    status.set(if ok {
        SaveStatus::Saved
    } else {
        SaveStatus::Failed
    });
    ok
}

/// The revision to treat as saved after a write: `now` on success, otherwise unchanged so the
/// autosave loop retries it.
pub(crate) fn settled_rev(ok: bool, saved: u32, now: u32) -> u32 {
    if ok { now } else { saved }
}

/// The Dashboard screen (`// DASHBOARD`).
#[component]
pub fn ScreenDashboard(is_active: bool) -> Element {
    let mut boards_rev = use_signal(|| 0u32);
    let mut current_board = use_signal(|| None::<String>);
    // The board select is an inline rename field while this is set (D-03).
    let mut renaming = use_signal(|| false);
    // Boards whose preset notice was dismissed with `Not now` during this screen visit.
    let mut not_now = use_signal(std::collections::HashSet::<String>::new);
    // The docked Plan with AI panel (D-14/D-15).
    let mut plan_open = use_signal(|| false);
    // A plan confirm is creating views: the panel must stay mounted until it settles (C-03).
    let plan_busy = use_signal(|| false);
    let st = use_board_state(None);
    let save_status = use_signal(|| SaveStatus::Idle);
    // The revision last written (or flushed); `save_rev != saved_rev` means unsaved edits.
    let mut saved_rev = use_signal(|| 0u32);
    // The open board is being deleted: nothing may write it back (C-02).
    let mut deleting = use_signal(|| false);
    // Window width and platform are read after mount, so the first render matches the server's.
    let mut viewport_w = use_signal(|| 1280.0_f64);
    let mut mac = use_signal(|| false);
    let resize_slot: Signal<Option<ResizeListener>> = use_signal(|| None);
    let gen_ctx = use_context::<DashboardGenCtx>();
    use_effect(move || {
        viewport_w.set(window_inner_width());
        mac.set(is_mac_platform());
        install_resize(viewport_w, resize_slot);
    });
    use_drop(move || remove_resize(resize_slot));
    // A window resize or the Plan panel docking changes the canvas size, which drives the frame cap.
    use_effect(move || {
        let _ = (*viewport_w.read(), *plan_open.read());
        spawn(canvas::remeasure_settled(st));
    });
    // One screen-level clock for every tile's `Updated {relative}` label. It only writes `now`;
    // each tile polls on its own loop, which ends when this screen unmounts (D-06).
    use_future(move || async move {
        loop {
            crate::platform::timer::sleep(15_000).await;
            let mut now = st.now;
            now.set(crate::components::hermes_app::screens::openui::now_unix_secs());
        }
    });
    // Keyed on `boards_rev` so Retry load and later saves refetch (never use_server_future +
    // restart()). Completion also opens the working copy when none is open or the open board
    // is gone; it never replaces a board that is still listed (unsaved edits survive).
    let boards = use_resource(move || async move {
        let _ = boards_rev();
        let list = list_dashboard_boards().await.map_err(|e| e.to_string())?;
        let mut board = st.board;
        let open_listed = board
            .peek()
            .as_ref()
            .is_some_and(|b| list.iter().any(|l| l.id == b.id));
        if !open_listed {
            board.set(selected_board(&list, current_board.peek().as_deref()).cloned());
        }
        Ok::<Vec<Board>, String>(list)
    });

    // The saved OpenUI views AI widgets and the Add menu resolve against: one fetch for the
    // screen, refetched when the active profile changes or a Re-run replaced a snapshot.
    let profile_tick =
        use_context::<crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx>().0;
    let views_res = use_resource(move || async move {
        let _ = (*profile_tick.read(), *gen_ctx.views_rev.read());
        let list = list_openui_views().await.map_err(|e| e.to_string());
        let mut views = st.views;
        views.set(Some(list.clone()));
        list
    });
    let _ = &views_res;

    // D-05: a button press inside one widget's frame becomes ONE agent turn, attributed by
    // `event.source` identity. The callback only records the action; the effect below enqueues it
    // inside a component scope. Messages from any other source are ignored (T-54.10-27).
    let incoming = use_signal(|| None::<(u32, String, OpenUiAction)>);
    let mut handled = use_signal(|| 0u32);
    use_effect(move || {
        let Some((seq, item_id, action)) = incoming.read().clone() else {
            return;
        };
        if seq <= *handled.peek() {
            return;
        }
        handled.set(seq);
        let view_id = st.board.peek().as_ref().and_then(|b| {
            b.items.iter().find_map(|i| match &i.kind {
                crate::server::dashboard_api::ItemKind::Widget { view_id } if i.id == item_id => {
                    Some(view_id.clone())
                }
                _ => None,
            })
        });
        let Some(view_id) = view_id else {
            return;
        };
        let accepted = gen_ctx.enqueue.call(GenJob {
            item_id: item_id.clone(),
            view_id,
            message: action_turn_message(&action),
            kind: GenKind::Action,
        });
        if accepted {
            let mut labels = gen_ctx.action_labels;
            labels.with_mut(|m| {
                m.insert(item_id, action.label.clone());
            });
        }
    });
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        type MessageListener = Closure<dyn FnMut(web_sys::MessageEvent)>;

        let mut incoming = incoming;
        let mut listener_slot: Signal<Option<MessageListener>> = use_signal(|| None);
        let seq = use_hook(|| std::rc::Rc::new(std::cell::Cell::new(0u32)));
        use_effect(move || {
            if listener_slot.peek().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let seq = seq.clone();
            let cb = MessageListener::new(move |ev: web_sys::MessageEvent| {
                let Some(source) = ev.source() else {
                    return;
                };
                let source: wasm_bindgen::JsValue = source.into();
                // Which widget's frame sent it? The sandbox origin is opaque, so identity of
                // `event.source` is the only check. Then the 8192-byte cap and field rules.
                let ids: Vec<String> = st
                    .board
                    .peek()
                    .as_ref()
                    .map(|b| {
                        b.items
                            .iter()
                            .filter(|i| {
                                matches!(
                                    i.kind,
                                    crate::server::dashboard_api::ItemKind::Widget { .. }
                                )
                            })
                            .map(|i| i.id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let Some(item_id) = ids.into_iter().find(|id| {
                    crate::components::hermes_app::screens::openui::frame_window_for(
                        &widget_frame_id(id),
                    )
                    .is_some_and(|w| wasm_bindgen::JsValue::from(w) == source)
                }) else {
                    return;
                };
                let Some(raw) = js_sys::JSON::stringify(&ev.data())
                    .ok()
                    .and_then(|s| s.as_string())
                else {
                    return;
                };
                if let Ok(action) =
                    crate::components::hermes_app::screens::openui::parse_openui_action(&raw)
                {
                    seq.set(seq.get() + 1);
                    incoming.set(Some((seq.get(), item_id, action)));
                }
            });
            let _ = window.add_event_listener_with_callback("message", cb.as_ref().unchecked_ref());
            listener_slot.set(Some(cb));
        });
        use_drop(move || {
            if let Some(cb) = listener_slot.write().take() {
                if let Some(window) = web_sys::window() {
                    let _ = window.remove_event_listener_with_callback(
                        "message",
                        cb.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    // Every loaded widget frame follows the app theme (deferred a tick so ThemeEffects has
    // already rewritten `<html data-theme>`).
    let theme = use_context::<crate::state::ThemeContext>().0;
    use_effect(move || {
        let light = crate::components::hermes_app::theme_effects::is_light_theme(&theme.read());
        spawn(async move {
            crate::platform::timer::sleep(50).await;
            let ids: Vec<String> = gen_ctx.loaded_frames.peek().iter().cloned().collect();
            for id in ids {
                post_theme_to_frame_id(&widget_frame_id(&id), light);
            }
        });
    });
    // The frames go with the screen: forget them so a later mount reloads and replays.
    use_drop(move || {
        let mut loaded = gen_ctx.loaded_frames;
        loaded.with_mut(|s| s.clear());
    });

    // One autosave loop: a changed revision that stays stable for a tick is written whole.
    // `saved_rev` is only advanced by a successful write, so a failed save is retried (after a
    // pause) and the `on_pick` flush sees the edits as still unsaved.
    use_future(move || async move {
        let mut prev = *st.save_rev.peek();
        loop {
            crate::platform::timer::sleep(600).await;
            let now = *st.save_rev.peek();
            if now != prev {
                prev = now;
                continue;
            }
            if now == *saved_rev.peek() || *deleting.peek() {
                continue;
            }
            // Clone before the await: no signal borrow crosses it.
            let open = st.board.peek().clone();
            if let Some(b) = open {
                let ok = save_board(b, save_status).await;
                let settled = settled_rev(ok, *saved_rev.peek(), now);
                saved_rev.set(settled);
                if !ok {
                    // shortcut: fixed 5 s retry pause, no exponential backoff.
                    crate::platform::timer::sleep(5_000).await;
                }
            }
        }
    });

    // Leaving the Dashboard (or closing the tab, best effort) must not drop the last edits:
    // flush them on a task that outlives this scope. It touches no scope-owned signal.
    use_drop(move || {
        if *st.save_rev.peek() != *saved_rev.peek() && !*deleting.peek() {
            if let Some(b) = st.board.peek().clone() {
                spawn_forever(async move {
                    let _ = save_dashboard_board(b).await;
                });
            }
        }
    });

    // Board select: write out unsaved edits of the outgoing board, then open the fresh server
    // copy of the picked one (the listed copy can be stale).
    let on_pick = move |id: String| {
        spawn(async move {
            if *deleting.peek() {
                return;
            }
            let rev = *st.save_rev.peek();
            if rev != *saved_rev.peek() {
                let outgoing = st.board.peek().clone();
                if let Some(b) = outgoing {
                    // A failed flush keeps the outgoing board open with its edits; the Failed
                    // notice stays on the status line and the autosave loop retries.
                    if !save_board(b, save_status).await {
                        return;
                    }
                }
                saved_rev.set(rev);
            }
            match list_dashboard_boards().await {
                Ok(list) => {
                    if let Some(b) = list.into_iter().find(|b| b.id == id) {
                        st.say(board_opened_announcement(&b.name, b.items.len()));
                        let (mut board, mut view, mut selected) = (st.board, st.view, st.selected);
                        view.set(View::default());
                        selected.set(None);
                        current_board.set(Some(b.id.clone()));
                        board.set(Some(b));
                    }
                }
                Err(e) => {
                    let mut notice = st.notice;
                    notice.set(Some(load_error_copy(&e.to_string())));
                }
            }
            boards_rev += 1;
        });
    };

    // A deleted board: open its neighbour (the resource picks it) or fall to the no-boards state.
    let on_deleted = move |next: Option<String>| {
        deleting.set(false);
        current_board.set(next);
        let (mut board, mut view, mut selected) = (st.board, st.view, st.selected);
        board.set(None);
        view.set(View::default());
        selected.set(None);
        boards_rev += 1;
    };

    let loaded = boards.read().clone();
    let open = st
        .board
        .read()
        .as_ref()
        .map(|b| (b.id.clone(), b.name.clone(), b.items.len()));
    let (status, options, selected_id, body) = match &loaded {
        Some(Ok(list)) => {
            let sel = selected_board(list, current_board.read().as_deref());
            (
                open.as_ref()
                    .map(|(_, n, c)| status_line(n, *c))
                    .or_else(|| sel.map(|b| status_line(&b.name, b.items.len()))),
                board_options(list),
                open.as_ref()
                    .map(|(id, _, _)| id.clone())
                    .or_else(|| sel.map(|b| b.id.clone())),
                None::<String>,
            )
        }
        Some(Err(e)) => (None, vec![], None, Some(load_error_copy(e))),
        None => (None, vec![], None, None),
    };
    let status_title = status
        .as_deref()
        .map(|s| s.trim_start_matches("· ").to_string());
    let narrow = crate::canvas_math::narrow(*viewport_w.read());
    let has_board = open.is_some();
    let board_count = loaded
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map_or(0, Vec::len);
    let chooser_open = *st.chooser.read();
    let open_id = open.as_ref().map(|(id, _, _)| id.clone());
    let board_ids: Vec<String> = options.iter().map(|(id, _)| id.clone()).collect();
    let rename_el = (*renaming.read() && has_board).then(|| {
        let name = open.as_ref().map(|(_, n, _)| n.clone()).unwrap_or_default();
        rsx! {
            RenameField {
                st,
                name,
                status: save_status,
                boards_rev,
                on_done: move |_| {
                    renaming.set(false);
                    crate::components::hermes_app::header::focus_element_by_id(BOARD_MENU_ID);
                },
            }
        }
    });
    let menu_el = (has_board && !narrow).then(|| {
        let (board_id, board_name, item_count) = open.clone().unwrap_or_default();
        rsx! {
            BoardMenu {
                st,
                board_id,
                board_name,
                item_count,
                board_ids,
                status: save_status,
                on_rename: move |_| renaming.set(true),
                on_deleted,
                on_deleting: move |d: bool| deleting.set(d),
            }
        }
    });
    let notice_dismissed = open_id
        .as_ref()
        .is_some_and(|id| not_now.read().contains(id));
    let board_full = open.as_ref().is_some_and(|(_, _, n)| *n >= BOARD_ITEMS_MAX);
    let notice = st.notice.read().clone();
    let announce = st.announce.read().clone();
    let palette_open = st.palette.read().clone();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-dashboard",
            "data-screen-label": "Dashboard",
            onpointerdown: move |_| {
                // A press that reaches the screen is outside every frame (a press inside an
                // iframe never bubbles here): the active widget gives pointer input back.
                let mut active = st.active;
                let next = activation_after(
                    active.peek().as_deref(),
                    ActivationEvent::PointerDownOutside,
                );
                if next != *active.peek() {
                    active.set(next);
                }
            },

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// DASHBOARD" }
                    h1 { class: "screen-title", "Dashboard" }
                    if let Some(status) = status {
                        span {
                            class: "screen-status",
                            title: "{status_title.clone().unwrap_or_default()}",
                            style: "overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 40ch;",
                            "{status}"
                        }
                    }
                }
                if has_board && !narrow {
                    HeaderActions {
                        board_full,
                        item_count: open.as_ref().map_or(0, |(_, _, n)| *n),
                        plan_open: *plan_open.read(),
                        on_add: move |_| open_palette(st, None, false, ADD_WIDGET_ID),
                        on_plan: move |_| {
                            if *plan_busy.peek() {
                                return;
                            }
                            let next = !*plan_open.peek();
                            plan_open.set(next);
                        },
                        on_present: move |_| start_present(st),
                    }
                }
            }

            BoardToolbar {
                options,
                selected_id,
                narrow,
                has_board,
                board_full,
                view: st.view,
                views: add_menu::menu_views(&st.views.read()),
                status: *save_status.read(),
                board_count,
                rename_field: rename_el,
                board_menu: menu_el,
                on_pick,
                on_new: move |_| {
                    let mut chooser = st.chooser;
                    chooser.set(true);
                },
                on_cmd: move |c| st.view_cmd(c),
                on_add: move |c: AddChoice| match c.kind() {
                    Some(kind) => add_item(st, kind, None),
                    None => open_palette(st, None, true, TRIGGER_ID),
                },
            }
            if let Some(n) = notice {
                div { class: "ih-dash-notice", role: "alert", "{n}" }
            }
            if has_board && !narrow && chooser_open {
                PresetChooser {
                    board_count,
                    on_created: move |id: String| {
                        let mut chooser = st.chooser;
                        chooser.set(false);
                        on_pick(id);
                    },
                    on_cancel: move |_| {
                        let mut chooser = st.chooser;
                        chooser.set(false);
                    },
                }
            }
            if has_board && !narrow {
                PresetNotice {
                    st,
                    dismissed: notice_dismissed,
                    on_dismiss: move |_| {
                        if let Some(id) = st.board.peek().as_ref().map(|b| b.id.clone()) {
                            not_now.with_mut(|s| {
                                s.insert(id);
                            });
                        }
                    },
                }
            }

            match (&loaded, body) {
                (None, _) => rsx! {
                    div { class: "ih-dashboard-loading", "Loading…" }
                },
                (Some(Err(_)), Some(copy)) => rsx! {
                    div { class: "ih-dashboard-error", role: "alert",
                        span { style: "color: var(--danger);", "{copy}" }
                        button {
                            class: "btn btn--sm btn--ghost",
                            onclick: move |_| boards_rev += 1,
                            "Retry load"
                        }
                    }
                },
                (Some(Ok(list)), _) if list.is_empty() => rsx! {
                    NoBoardsState { on_created: move |id: String| on_pick(id) }
                },
                _ => rsx! {},
            }

            if let Some((id, name, _)) = open {
                // The Plan panel docks beside the canvas, which keeps working (D-14).
                div { class: "ih-canvas-row",
                    DashboardCanvas {
                        key: "{id}",
                        st,
                        board_name: name,
                        is_active,
                        narrow,
                        mac: *mac.read(),
                    }
                    if !narrow && *plan_open.read() {
                        PlanPanel {
                            st,
                            busy: plan_busy,
                            on_close: move |_| {
                                plan_open.set(false);
                                crate::components::hermes_app::header::focus_element_by_id(
                                    PLAN_TRIGGER_ID,
                                );
                            },
                        }
                    }
                }
            }
            if let Some(open) = palette_open {
                Palette { st, open }
            }
            // Polite live region for every announcement (UI-SPEC A10).
            div { class: "ih-sr-only", role: "status", "aria-live": "polite", "{announce}" }
        }
    }
}

/// DOM id of the header `Add widget` button (the palette's focus-restore target).
const ADD_WIDGET_ID: &str = "ih-add-widget";

/// UI-SPEC A1 header actions: `Add widget` (primary) opens the palette at the viewport centre.
#[component]
fn HeaderActions(
    board_full: bool,
    plan_open: bool,
    on_add: EventHandler<()>,
    on_plan: EventHandler<()>,
    #[props(default = 1)] item_count: usize,
    #[props(default)] on_present: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "screen-actions",
            // Hidden below 900px by the caller (the whole action row is).
            button {
                id: PRESENT_TRIGGER_ID,
                class: "btn btn--ghost",
                r#type: "button",
                disabled: item_count == 0,
                title: if item_count == 0 { "Add an item to present." },
                onclick: move |_| on_present.call(()),
                "Present"
            }
            button {
                id: PLAN_TRIGGER_ID,
                class: "btn btn--ghost",
                r#type: "button",
                "aria-pressed": "{plan_open}",
                onclick: move |_| on_plan.call(()),
                "Plan with AI"
            }
            button {
                id: ADD_WIDGET_ID,
                class: "btn",
                r#type: "button",
                disabled: board_full,
                title: if board_full { BOARD_ITEMS_FULL_ERROR },
                onclick: move |_| on_add.call(()),
                "Add widget"
            }
        }
    }
}

/// UI-SPEC A2 toolbar row: board select, then (wide windows with a board open) the save
/// status and the zoom group. Narrow windows keep only the select (A9: read-only).
#[component]
fn BoardToolbar(
    options: Vec<(String, String)>,
    selected_id: Option<String>,
    narrow: bool,
    has_board: bool,
    board_full: bool,
    view: Signal<View>,
    views: items::SavedViews,
    status: SaveStatus,
    board_count: usize,
    /// The inline rename field, shown in place of the select while renaming.
    #[props(default)]
    rename_field: Option<Element>,
    /// The `⋯` board menu.
    #[props(default)]
    board_menu: Option<Element>,
    on_pick: EventHandler<String>,
    on_new: EventHandler<()>,
    on_cmd: EventHandler<CanvasCmd>,
    on_add: EventHandler<AddChoice>,
) -> Element {
    let save_line = save_status_line(status);
    rsx! {
        div { class: "ih-toolbar",
            if let Some(field) = rename_field {
                {field}
            } else {
                select {
                    class: "field-input",
                    "aria-label": "Board",
                    style: "max-width: 200px; overflow: hidden; text-overflow: ellipsis;",
                    onchange: move |e| on_pick.call(e.value()),
                    for (id , name) in options {
                        option {
                            value: "{id}",
                            title: "{name}",
                            selected: selected_id.as_deref() == Some(id.as_str()),
                            "{name}"
                        }
                    }
                }
            }
            if !narrow && has_board {
                if let Some(menu) = board_menu {
                    {menu}
                }
                NewBoardButton { board_count, on_click: on_new }
                AddMenu { full: board_full, views, on_choose: on_add }
                div { class: "ih-toolbar__spacer" }
                if let Some((text, class)) = save_line {
                    span { class: "ih-save {class}", role: "status", "{text}" }
                }
                ZoomGroup { view, on_cmd }
            }
        }
    }
}

/// UI-SPEC E2: a board with no items. `on_add` and `on_preset` are `None` on the read-only
/// narrow stack.
#[component]
pub(crate) fn EmptyBoardState(
    #[props(default)] on_add: Option<EventHandler<()>>,
    #[props(default)] on_preset: Option<EventHandler<()>>,
) -> Element {
    rsx! {
        div { class: "ih-canvas__empty",
            h2 { "This board is empty" }
            p { "Press / to add a widget, N for a note, or start from a preset." }
            if let Some(on_add) = on_add {
                // The canvas pans on pointerdown and reads Space/arrows: keep both off the button.
                button {
                    class: "btn",
                    r#type: "button",
                    onpointerdown: move |e| e.stop_propagation(),
                    onkeydown: move |e| e.stop_propagation(),
                    onclick: move |_| on_add.call(()),
                    "Add widget"
                }
            }
            if let Some(on_preset) = on_preset {
                button {
                    class: "btn btn--ghost",
                    r#type: "button",
                    onpointerdown: move |e| e.stop_propagation(),
                    onkeydown: move |e| e.stop_propagation(),
                    onclick: move |_| on_preset.call(()),
                    "Use a preset"
                }
            }
        }
    }
}

/// UI-SPEC E2 + E9: the no-boards state: the same preset chooser as `New board`.
#[component]
fn NoBoardsState(on_created: EventHandler<String>) -> Element {
    rsx! {
        div { class: "ih-dashboard-empty",
            h2 { "No boards yet" }
            p { "A board holds live tiles, generated widgets and notes. Pick a starting point." }
            PresetChooser { board_count: 0, on_created }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::dashboard_api::BOARD_SCHEMA_VERSION;

    #[test]
    fn a_failed_save_leaves_the_revision_unsaved() {
        assert_eq!(settled_rev(true, 3, 7), 7);
        assert_eq!(settled_rev(false, 3, 7), 3);
    }

    fn board(id: &str, name: &str, n: usize) -> Board {
        use crate::server::dashboard_api::{BoardItem, ItemKind};
        Board {
            version: BOARD_SCHEMA_VERSION,
            id: id.into(),
            name: name.into(),
            created_at: 1,
            updated_at: 1,
            items: (0..n)
                .map(|i| BoardItem {
                    id: format!("i{i}"),
                    x: 0,
                    y: 0,
                    w: 160,
                    h: 96,
                    kind: ItemKind::Note {
                        text: String::new(),
                    },
                })
                .collect(),
        }
    }

    #[test]
    fn save_status_line_copy() {
        assert_eq!(save_status_line(SaveStatus::Idle), None);
        assert_eq!(save_status_line(SaveStatus::Saving).unwrap().0, "Saving…");
        assert_eq!(save_status_line(SaveStatus::Saved).unwrap().0, "Saved.");
        assert_eq!(
            save_status_line(SaveStatus::Failed).unwrap().0,
            "Save failed. Check server logs. Your changes stay in this window until you reload."
        );
        // Gray, green, red (UI-SPEC A13).
        assert_eq!(
            save_status_line(SaveStatus::Saving).unwrap().1,
            "ih-save--saving"
        );
        assert_eq!(
            save_status_line(SaveStatus::Saved).unwrap().1,
            "ih-save--saved"
        );
        assert_eq!(
            save_status_line(SaveStatus::Failed).unwrap().1,
            "ih-save--failed"
        );
    }

    #[test]
    fn canvas_css_is_registered_unconditionally() {
        const APP_RS: &str = include_str!("../../../../app.rs");
        assert!(APP_RS.contains("const CANVAS_CSS: Asset = asset!(\"/assets/canvas.css\");"));
        let link = APP_RS
            .lines()
            .find(|l| l.contains("href: CANVAS_CSS"))
            .expect("app.rs links the canvas sheet");
        assert!(link.trim_start().starts_with("document::Link"), "{link}");
        // Not behind a cfg: the preceding line must not be a cfg attribute.
        let idx = APP_RS.find("href: CANVAS_CSS").unwrap();
        let before = &APP_RS[..idx];
        let prev = before.lines().rev().nth(1).unwrap_or("");
        assert!(!prev.contains("cfg"), "{prev}");
    }

    #[test]
    fn board_switch_announcement() {
        assert_eq!(
            board_opened_announcement("Ops", 4),
            "Board Ops opened, 4 items."
        );
        assert_eq!(
            board_opened_announcement("Ops", 1),
            "Board Ops opened, 1 item."
        );
        // The board select announces through it.
        assert!(include_str!("mod.rs").contains("st.say(board_opened_announcement("));
    }

    #[test]
    fn status_line_pluralises() {
        assert_eq!(status_line("Ops", 1), "· Ops · 1 item");
        assert_eq!(status_line("Ops", 0), "· Ops · 0 items");
        assert_eq!(status_line("Ops", 12), "· Ops · 12 items");
    }

    #[test]
    fn selected_board_falls_back_to_the_first() {
        let list = vec![board("a", "A", 0), board("b", "B", 2)];
        assert_eq!(selected_board(&list, Some("b")).unwrap().name, "B");
        assert_eq!(selected_board(&list, Some("gone")).unwrap().name, "A");
        assert_eq!(selected_board(&list, None).unwrap().name, "A");
        assert!(selected_board(&[], None).is_none());
        assert_eq!(
            board_options(&list),
            vec![("a".into(), "A".into()), ("b".into(), "B".into())]
        );
    }

    fn ssr(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn no_boards_state_renders_the_preset_chooser() {
        let html = ssr(|| rsx! { NoBoardsState { on_created: |_| {} } });
        assert!(html.contains("No boards yet"), "{html}");
        assert!(
            html.contains(
                "A board holds live tiles, generated widgets and notes. Pick a starting point."
            ),
            "{html}"
        );
        for title in ["Blank", "Founder", "Engineering", "Sales", "Ops"] {
            assert!(html.contains(&format!(">{title}<")), "{html}");
        }
        assert!(html.contains("value=\"Blank\""), "{html}");
        assert!(html.contains("Create board"), "{html}");
    }

    #[test]
    fn header_actions_include_add_widget() {
        let html = ssr(
            || rsx! { HeaderActions { board_full: false, plan_open: false, on_add: |_| {}, on_plan: |_| {} } },
        );
        assert!(html.contains("screen-actions"), "{html}");
        assert!(html.contains("class=\"btn\""), "{html}");
        assert!(html.contains(">Add widget<"), "{html}");
        assert!(html.contains(">Plan with AI<"), "{html}");
        assert!(!html.contains("disabled"), "{html}");
        let full = ssr(
            || rsx! { HeaderActions { board_full: true, plan_open: false, on_add: |_| {}, on_plan: |_| {} } },
        );
        assert!(full.contains("disabled"), "{full}");
        // The empty board offers the same entry; the read-only stack offers none.
        let empty = ssr(|| rsx! { EmptyBoardState { on_add: |_| {} } });
        assert!(empty.contains(">Add widget<"), "{empty}");
        assert!(!ssr(|| rsx! { EmptyBoardState {} }).contains("Add widget"));
        let preset = ssr(|| rsx! { EmptyBoardState { on_preset: |_| {} } });
        assert!(preset.contains(">Use a preset<"), "{preset}");
        assert!(!empty.contains("Use a preset"), "{empty}");
        assert!(!ssr(|| rsx! { EmptyBoardState {} }).contains("Use a preset"));
    }

    #[test]
    fn present_button_disabled_when_empty() {
        let empty = ssr(
            || rsx! { HeaderActions { board_full: false, plan_open: false, item_count: 0, on_add: |_| {}, on_plan: |_| {} } },
        );
        assert!(empty.contains(">Present<"), "{empty}");
        assert!(empty.contains("disabled"), "{empty}");
        assert!(
            empty.contains("title=\"Add an item to present.\""),
            "{empty}"
        );
        let some = ssr(
            || rsx! { HeaderActions { board_full: false, plan_open: false, item_count: 2, on_add: |_| {}, on_plan: |_| {} } },
        );
        assert!(
            some.contains(">Present<") && !some.contains("Add an item to present."),
            "{some}"
        );
    }

    #[test]
    fn name_problem_bounds() {
        assert_eq!(name_problem(""), Some(BOARD_NAME_ERROR));
        assert_eq!(name_problem("   "), Some(BOARD_NAME_ERROR));
        assert_eq!(name_problem(&"n".repeat(41)), Some(BOARD_NAME_ERROR));
        assert_eq!(name_problem(&"n".repeat(40)), None);
        let b = new_board("  Ops ");
        assert_eq!(b.name, "Ops");
        assert!(uuid::Uuid::parse_str(&b.id).is_ok() && b.items.is_empty());
    }

    #[test]
    fn load_error_copy_matches_the_spec() {
        assert_eq!(
            load_error_copy("boom"),
            "Could not load boards: boom. Check server logs, then reload."
        );
    }
}
