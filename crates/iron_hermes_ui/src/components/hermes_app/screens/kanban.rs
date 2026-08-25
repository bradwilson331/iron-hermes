//! Phase 36.3.7.11 Plan 02 (D-02 / D-09 / D-06 / D-07) — Kanban dashboard screen.
//!
//! Walking skeleton (Plan 01) + drag-and-drop wiring (Plan 02):
//! - Fetches the board via `fetch_board(None)` (D-18 — None resolves to
//!   default board).
//! - Opens a WebSocket to `/api/ws/kanban` to receive live
//!   `KanbanWsEvent::TaskEventBatch` pushes from the dashboard tail
//!   consumer (D-08); calls `board_resource.restart()` on every event
//!   (Plan 02 minimal behavior — Plan 03 will swap to delta-apply).
//! - Owns the shared drag-and-drop signals: `dragged_task_id`,
//!   `pending_task_ids`, `toast_msg`, `live_region_msg`,
//!   `archive_modal_task` — all passed down to `KanbanBoard` for the
//!   column + card components.
//! - Renders a hidden `role="log" aria-live="polite"` live region per
//!   UI-SPEC §6.5 for screen-reader announcements of board updates,
//!   move confirmations, and disallowed-transition hints.

// Submodules in the sibling `kanban/` directory — board/column/card form
// the visual shell. Plan 03 adds drawer + modals.
pub mod board;
pub mod card;
pub mod column;
pub mod drawer;
pub mod modals;
// Phase 47.4 Plan 08 (D-04): the profile detail drawer (editable).
pub mod profile_drawer;
// Phase 47.4 Plan 01 (D-05): board-header PROFILE dropdown.
pub mod profile_switcher;
// Phase 47.4 Plan 07 (D-01): the Create Kanban Profile wizard.
pub mod wizard;

use crate::components::hermes_app::screens::kanban::board::KanbanBoard;
use crate::components::hermes_app::screens::kanban::drawer::TaskDrawer;
use crate::components::hermes_app::screens::kanban::modals::{
    ArchiveConfirmModal, BlockModal, CompleteModal, CreateTaskModal,
};
use crate::components::hermes_app::screens::kanban::profile_switcher::ProfileSwitcher;
use crate::components::hermes_app::screens::kanban::wizard::CreateProfileWizard;
// Phase 50.1 Plan 02 (D-10): the context prop shared by CreateProfileWizard
// and ProfileDetailDrawer — Kanban's mounts below pass `Kanban` explicitly.
use crate::components::hermes_app::screens::profile_shared::ProfileDialogContext;
use crate::protocol::TaskRow;
use dioxus::prelude::*;
use std::collections::{HashMap, HashSet};

/// Phase 36.3.7.11 Plan 01: stylesheet for the kanban dashboard. Defines
/// the `.kn-card` cyan-glow rules, `.kn-board` layout, chip styles, and
/// `@media (prefers-reduced-motion: reduce)` overrides (UI-SPEC §6.6).
#[allow(dead_code)] // used in ScreenKanban rsx! document::Link; dead_code fires on test target
const KANBAN_CSS: Asset = asset!("/assets/kanban.css");

/// Phase 36.3.7.11 Plan 01: WS lifecycle state — drives a small status
/// dot in the toolbar. Plan 01 keeps the indicator minimal; richer
/// reconnect surfacing lands in Plan 04.
#[allow(dead_code)] // variants set/matched in ScreenKanban; dead_code fires on test target
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WsState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

/// Phase 36.3.7.11 Plan 02: kanban dashboard screen.
///
/// Renders six columns (D-09), an archive-toggle toolbar button, a WS
/// status indicator, and the off-screen live region for ARIA
/// announcements. Subscribes to `/api/ws/kanban` and re-fetches the
/// board on every TaskEventBatch.
#[component]
pub fn ScreenKanban(is_active: bool) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).

    // Archive toggle — drives the 7th column visibility (D-09) AND the
    // include_archived parameter on `fetch_board` (BUG-1 fix from
    // 36.3.7.11 UAT). Declared BEFORE `board_resource` so the
    // use_resource closure can capture it.
    let mut archived_visible: Signal<bool> = use_signal(|| false);

    // Phase 46.6 gap-closure: the root screen router, so the toolbar can link to
    // the Artifacts gallery (a kanban task's output is often an artifact).
    let mut nav_screen = use_context::<Signal<crate::state::Screen>>();

    // Board fetch resource. Re-runs whenever `archived_visible` flips
    // (the toggle handler calls `board_resource.restart()` after `.set()`).
    // The `.read()` is a Copy-out (bool) — no borrow held across the
    // await, clippy-safe.
    let mut board_resource = use_resource(move || async move {
        // Copy the bool out before await — GenerationalRef must not be held across await points.
        let show_archived: bool = *archived_visible.read();
        crate::server::kanban_api::fetch_board(None, show_archived).await
    });

    // Local Signal<Vec<TaskRow>> — Plan 02 mutates this optimistically
    // on drop. The use_effect below syncs it from `board_resource`.
    let mut tasks: Signal<Vec<TaskRow>> = use_signal(Vec::<TaskRow>::new);

    // Plan 02 drag-and-drop signals.
    let dragged_task_id: Signal<Option<String>> = use_signal(|| None);
    let pending_task_ids: Signal<HashSet<String>> = use_signal(HashSet::new);
    let toast_msg: Signal<Option<String>> = use_signal(|| None);
    let live_region_msg: Signal<Option<String>> = use_signal(|| None);

    // Plan 03 archive-confirm modal target. Plan 02 sets `Some(task_id)`
    // when a card is dragged to ARCHIVED; Plan 03 reads it to open the
    // confirm modal. Mutable here because Plan 03's drag-archive AND
    // drawer Archive button both write through this signal.
    let mut archive_modal_task: Signal<Option<String>> = use_signal(|| None);

    // Plan 03 (D-13) modal-target signals — the drawer emits modal-open
    // events that set these; modals.rs reads them to decide whether to
    // render. Each modal closes by resetting its signal to None.
    let mut complete_modal_task: Signal<Option<String>> = use_signal(|| None);
    let mut block_modal_task: Signal<Option<String>> = use_signal(|| None);
    let mut create_modal_open: Signal<bool> = use_signal(|| false);

    // Plan 03 (D-21): per-task event counter. Increments on every WS
    // TaskEventBatch row whose task_id matches the currently-open drawer
    // task — drives the drawer's `use_resource` re-fetch (UI-SPEC §8.4).
    // 200ms debounce is applied below. Mutations go through `.write()`;
    // the signal handle itself is Copy so no `mut` binding is required.
    let per_task_event_counter: Signal<HashMap<String, u64>> = use_signal(HashMap::new);

    // Drawer open state — Plan 03 wires this end-to-end.
    let mut open_drawer_task_id: Signal<Option<String>> = use_signal(|| None);

    // WS connection state indicator.
    let mut ws_state: Signal<WsState> = use_signal(|| WsState::Connecting);

    // Phase 47.4 Plan 01 (D-05): the PROFILE switcher's assignee lens
    // (`None` = "ALL PROFILES", no filter) and the profile detail-drawer
    // target the switcher's EDIT chip / footer link set. Neither drives a
    // DB re-resolution (D-05 prohibition) — `active_profile` is purely a
    // client-side highlight/filter axis.
    let active_profile: Signal<Option<String>> = use_signal(|| None);
    let mut detail_profile: Signal<Option<String>> = use_signal(|| None);

    // Phase 47.4 Plan 07 (D-01): the Create Kanban Profile wizard's open
    // state — mirrors `create_modal_open`'s existing idiom exactly.
    let mut wizard_open: Signal<bool> = use_signal(|| false);
    // Working copy of the known profile names, refreshed after a
    // successful create by calling `list_profiles()` directly (a plain
    // async fn) — never by restarting a resource, which would desync
    // hook ordering for `ProfileSwitcher`'s own signals.
    let mut profile_list_refresh_tick: Signal<u32> = use_signal(|| 0);

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
    // Plan 01 behavior; later plans may delta-apply). Mirrors the
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
                        let event: crate::protocol::KanbanWsEvent = match serde_json::from_str(&t) {
                            Ok(e) => e,
                            Err(_) => continue,
                        };
                        match event {
                            crate::protocol::KanbanWsEvent::TaskEventBatch { events, .. } => {
                                // Plan 01 behavior preserved: full re-fetch.
                                board_resource.restart();
                                // Plan 03 (D-21 / UI-SPEC §8.4): per-task
                                // event counter increments for each row in
                                // the batch. The 200ms debounce: we collect
                                // per-tick increments by delaying the write
                                // until the next event-loop tick. With
                                // gloo-timers on WASM and tokio::time on
                                // native, schedule the increment after 200ms.
                                let task_ids: Vec<String> =
                                    events.iter().map(|e| e.task_id.clone()).collect();
                                if !task_ids.is_empty() {
                                    let mut counter = per_task_event_counter;
                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        let task_ids_for_timer = task_ids;
                                        wasm_bindgen_futures::spawn_local(async move {
                                            gloo_timers::future::TimeoutFuture::new(200).await;
                                            let mut w = counter.write();
                                            for tid in task_ids_for_timer {
                                                *w.entry(tid).or_insert(0) += 1;
                                            }
                                        });
                                    }
                                    #[cfg(not(target_arch = "wasm32"))]
                                    {
                                        let task_ids_for_timer = task_ids;
                                        // Native build (tests): apply
                                        // increments immediately. The
                                        // 200ms debounce is a UX nicety
                                        // on the browser; native build is
                                        // exercised by source-string tests.
                                        let mut w = counter.write();
                                        for tid in task_ids_for_timer {
                                            *w.entry(tid).or_insert(0) += 1;
                                        }
                                    }
                                }
                            }
                            crate::protocol::KanbanWsEvent::Error { message } => {
                                #[cfg(target_arch = "wasm32")]
                                web_sys::console::log_1(
                                    &format!("[kanban-ws] error: {message}").into(),
                                );
                                let _ = message;
                            }
                            crate::protocol::KanbanWsEvent::Ping {} => {}
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
    // Phase 47.4 Plan 09 (D-05): read-only view of the lens for
    // `KanbanBoard` — pure pass-through, never used to re-fetch or
    // re-resolve the board.
    let active_profile_ro: ReadSignal<Option<String>> = active_profile.into();

    let on_open_drawer = move |task_id: String| {
        // Plan 03 opens the drawer by writing the task_id signal.
        open_drawer_task_id.set(Some(task_id));
    };

    let live_msg_str = live_region_msg.read().clone().unwrap_or_default();
    let toast_text = toast_msg.read().clone();

    // GAP-10 (47.4-16 Task 1): whether either drawer is currently open,
    // snapshotted into a plain bool BEFORE `rsx!`. `.read()` (not
    // `.peek()`) so the toast's corner reacts when a drawer opens — the
    // reads are temporaries dropped at the end of this statement, so no
    // live borrow crosses into the rsx! tree (clippy.toml).
    let drawer_open = open_drawer_task_id.read().is_some() || detail_profile.read().is_some();

    // Phase 47.4 Plan 09 (D-05): the toolbar lens indicator. Computed
    // from `tasks` — already-fetched, unfiltered board data — never a
    // new fetch. `None` (ALL PROFILES) renders no indicator at all
    // (E7/covered). The template is the same at n=0, n=1, and n=many —
    // no special-casing (E7/zero-one-many).
    let lens_indicator_text: Option<String> = active_profile.read().clone().map(|name| {
        let all_tasks = tasks.read();
        let total = all_tasks.len();
        let matching = all_tasks.iter().filter(|t| t.assignee == name).count();
        format!("Showing {matching} of {total} for {name}")
    });

    // Read-only views of the drawer/counter signals for the TaskDrawer
    // component (props use ReadSignal for read-only access).
    let drawer_task_id_ro: ReadSignal<Option<String>> = open_drawer_task_id.into();
    let per_task_counter_ro: ReadSignal<HashMap<String, u64>> = per_task_event_counter.into();
    // Phase 47.4 Plan 12 (GAP-2/GAP-3): read-only view of the profile
    // refresh tick, shared by `ProfileSwitcher` and `TaskDrawer` so both
    // consumers of the known-profile list re-fetch on the same bump.
    let profile_list_refresh_tick_ro: ReadSignal<u32> = profile_list_refresh_tick.into();

    // Drawer-event handlers — they update the modal-target signals or
    // spawn one-click writes (Unblock).
    let on_drawer_close = move |_| {
        open_drawer_task_id.set(None);
    };
    let on_open_complete = move |task_id: String| {
        complete_modal_task.set(Some(task_id));
    };
    let on_open_block = move |task_id: String| {
        block_modal_task.set(Some(task_id));
    };
    let on_open_archive = move |task_id: String| {
        archive_modal_task.set(Some(task_id));
    };
    let on_unblock = move |task_id: String| {
        // Unblock is one-click — no modal. Spawn patch_task_status to Ready.
        let mut tm = toast_msg;
        spawn(async move {
            match crate::server::kanban_api::patch_task_status(
                task_id,
                None,
                crate::protocol::KanbanStatus::Ready,
                None,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => tm.set(Some(format!("Unblock failed: {e}"))),
            }
        });
    };
    let on_decompose = move |task_id: String| {
        let mut tm = toast_msg;
        // D-03: rebind pending_task_ids to a mutable local — the signal
        // read/write API needs `&mut self`. Mirrors move_task_optimistic
        // (board.rs) exactly.
        let mut p = pending_task_ids;
        let task_id_owned = task_id.clone();
        // D-03: scoped write — mark pending BEFORE the spawn. WriteLock
        // drops at this closing brace, well before the .await below.
        {
            p.write().insert(task_id_owned.clone());
        }
        spawn(async move {
            match crate::server::kanban_api::run_decompose_or_specify(
                task_id,
                None,
                crate::protocol::DecomposeOrSpecify::Decompose,
            )
            .await
            {
                Ok(crate::protocol::DecomposeResult::Ok {
                    children_count,
                    summary,
                }) => {
                    // D-03: scoped write — clear pending after the .await
                    // resolves (Ok arm).
                    {
                        p.write().remove(&task_id_owned);
                    }
                    tm.set(Some(format!(
                        "Decomposed into {children_count} children: {summary}"
                    )));
                    // D-04: re-fetch the board so new children appear
                    // without a manual reload.
                    board_resource.restart();
                }
                Ok(crate::protocol::DecomposeResult::NotWired { message }) => {
                    // D-03: scoped write — clear pending (NotWired arm).
                    {
                        p.write().remove(&task_id_owned);
                    }
                    tm.set(Some(format!("Decompose not configured. {message}")));
                }
                Err(e) => {
                    // D-03: scoped write — clear pending (Err arm) so the
                    // card never stays stuck disabled.
                    {
                        p.write().remove(&task_id_owned);
                    }
                    tm.set(Some(format!("Decompose failed: {e}")));
                }
            }
        });
    };
    let on_specify = move |task_id: String| {
        let mut tm = toast_msg;
        // D-03: rebind pending_task_ids to a mutable local — see on_decompose.
        let mut p = pending_task_ids;
        let task_id_owned = task_id.clone();
        // D-03: scoped write — mark pending BEFORE the spawn.
        {
            p.write().insert(task_id_owned.clone());
        }
        spawn(async move {
            match crate::server::kanban_api::run_decompose_or_specify(
                task_id,
                None,
                crate::protocol::DecomposeOrSpecify::Specify,
            )
            .await
            {
                Ok(crate::protocol::DecomposeResult::Ok {
                    children_count,
                    summary,
                }) => {
                    // D-03: scoped write — clear pending (Ok arm).
                    {
                        p.write().remove(&task_id_owned);
                    }
                    tm.set(Some(format!(
                        "Specified ({children_count} children): {summary}"
                    )));
                    // D-04: re-fetch the board so the rewritten+promoted
                    // card appears without a manual reload.
                    board_resource.restart();
                }
                Ok(crate::protocol::DecomposeResult::NotWired { message }) => {
                    // D-03: scoped write — clear pending (NotWired arm).
                    {
                        p.write().remove(&task_id_owned);
                    }
                    tm.set(Some(format!("Specify not configured. {message}")));
                }
                Err(e) => {
                    // D-03: scoped write — clear pending (Err arm).
                    {
                        p.write().remove(&task_id_owned);
                    }
                    tm.set(Some(format!("Specify failed: {e}")));
                }
            }
        });
    };
    let on_post_comment = move |(task_id, body): (String, String)| {
        let mut tm = toast_msg;
        spawn(async move {
            if let Err(e) = crate::server::kanban_api::post_comment(task_id, None, body).await {
                tm.set(Some(format!("Comment not saved. Try again. ({e})")));
            }
        });
    };

    // Plan 03 (D-12): TRIAGE card decompose/specify handler — used by both
    // the card-level buttons (in TRIAGE column) AND the drawer's
    // TriageActionRow. Spawns `run_decompose_or_specify` and surfaces the
    // result via toast (NotWired tooltip per UI-SPEC §4.3 / §7.5).
    let on_triage_action =
        move |(task_id, action): (String, crate::protocol::DecomposeOrSpecify)| {
            let mut tm = toast_msg;
            // D-03: rebind pending_task_ids to a mutable local — see
            // on_decompose above. task_id is moved into the spawn below, so
            // clone an owned copy for the pre-spawn insert + post-await
            // removal.
            let mut p = pending_task_ids;
            let task_id_owned = task_id.clone();
            // D-03: scoped write — mark pending BEFORE the spawn.
            {
                p.write().insert(task_id_owned.clone());
            }
            spawn(async move {
                match crate::server::kanban_api::run_decompose_or_specify(task_id, None, action)
                    .await
                {
                    Ok(crate::protocol::DecomposeResult::Ok {
                        children_count,
                        summary,
                    }) => {
                        // D-03: scoped write — clear pending (Ok arm).
                        {
                            p.write().remove(&task_id_owned);
                        }
                        tm.set(Some(format!(
                            "{:?}: {children_count} children. {summary}",
                            action
                        )));
                        // D-04: re-fetch the board so new children / the
                        // rewritten+promoted card appear without a manual
                        // reload.
                        board_resource.restart();
                    }
                    Ok(crate::protocol::DecomposeResult::NotWired { message }) => {
                        // D-03: scoped write — clear pending (NotWired arm).
                        {
                            p.write().remove(&task_id_owned);
                        }
                        // UI-SPEC §7.5 toast: "{action} not configured. Run: ..."
                        tm.set(Some(format!("{} not configured. {message}", action.slug())));
                    }
                    Err(e) => {
                        // D-03: scoped write — clear pending (Err arm) so
                        // the card never stays stuck disabled.
                        {
                            p.write().remove(&task_id_owned);
                        }
                        tm.set(Some(format!("{}: {e}", action.slug())));
                    }
                }
            });
        };

    // Modal-success handlers refresh the board + close.
    let mut close_complete_modal = move || complete_modal_task.set(None);
    let mut close_block_modal = move || block_modal_task.set(None);
    let mut close_archive_modal = move || archive_modal_task.set(None);
    let mut close_create_modal = move || create_modal_open.set(false);
    let mut restart_board = move || board_resource.restart();

    rsx! {
        section {
            class: if is_active { "screen is-active" } else { "screen" },
            id: "screen-kanban",
            "data-screen-label": "// MODULE 14 — KANBAN",
            document::Link { rel: "stylesheet", href: KANBAN_CSS }
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 14" }
                    h1 { class: "screen-title", "Kanban" }
                }
                div { class: "screen-actions",
                    span {
                        class: "kn-ws-status {ws_state_class}",
                        "aria-label": "WebSocket status",
                        "•"
                    }
                    // Phase 47.4 Plan 01 (D-05 / D-10): board profile
                    // switcher — placed before `+ Add card` per UI-SPEC
                    // Navigation & Placement. Phase 47.4 Plan 12 (GAP-2):
                    // `refresh_tick` is passed as a prop the switcher's own
                    // `use_resource` reads in its sync prefix — a bump
                    // re-fetches internally, no remount required.
                    ProfileSwitcher {
                        active: active_profile,
                        refresh_tick: profile_list_refresh_tick_ro,
                        on_edit: move |name: String| {
                            // Phase 47.4 Plan 08: opening the drawer closes
                            // the wizard if it's open — the two surfaces
                            // are mutually exclusive (Claude's-discretion
                            // interaction detail per the plan).
                            detail_profile.set(Some(name));
                            wizard_open.set(false);
                        },
                    }
                    // Phase 47.4 Plan 09 (D-05 / UI-SPEC "Board profile-lens
                    // highlight"): the lens-match toolbar indicator, next to
                    // the PROFILE trigger. Absent entirely when the lens is
                    // ALL PROFILES (`None`) — matches the switcher's own
                    // clear-on-ALL-PROFILES behavior.
                    if let Some(text) = lens_indicator_text.clone() {
                        span { class: "kn-lens-indicator", "{text}" }
                    }
                    // Phase 47.4 Plan 07 (D-01 / D-10): opens the Create
                    // Kanban Profile wizard — placed between the switcher
                    // and `+ Add card` per UI-SPEC Navigation & Placement.
                    button {
                        // Phase 47.4 Plan 14 (GAP-4): `btn btn--sm` resolves
                        // this button's size against the same site.css:165-186
                        // / screens.css:229 rules its three toolbar siblings
                        // use. `btn--ghost` is deliberately omitted so the
                        // accent stays distinct (`.btn`'s base teal fill) —
                        // only size had to match, per operator sign-off.
                        class: "btn btn--sm",
                        onclick: move |_| {
                            // Phase 47.4 Plan 08: mutually exclusive with
                            // the profile detail drawer — see the on_edit
                            // handler above.
                            wizard_open.set(true);
                            detail_profile.set(None);
                        },
                        "+ NEW PROFILE"
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| create_modal_open.set(true),
                        "+ Add card"
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            let cur = *archived_visible.read();
                            archived_visible.set(!cur);
                            // BUG-1 fix (quick-260602-ds9): re-fetch with
                            // the new include_archived value so the
                            // ARCHIVED column populates/empties. restart()
                            // is sync — no signal-borrow span over await.
                            board_resource.restart();
                        },
                        if *archived_visible.read() { "HIDE ARCHIVED" } else { "SHOW ARCHIVED" }
                    }
                    // Phase 46.6 gap-closure: jump to the Artifacts gallery.
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| nav_screen.set(crate::state::Screen::Artifacts),
                        "▤ ARTIFACTS"
                    }
                }
            }
            // UI-SPEC §6.1 / §6.5: off-screen live region for board
            // updates and disallowed-transition announcements. `polite`
            // = announce when the user is idle. Hidden visually but
            // accessible to assistive tech.
            div {
                class: "kn-live-region",
                role: "log",
                aria_live: "polite",
                "aria-atomic": "false",
                "aria-label": "Board updates",
                "{live_msg_str}"
            }
            if has_error {
                div { class: "kn-error",
                    "Failed to load board. Retrying via WebSocket reconnect…"
                }
            } else if is_loading {
                div { class: "kn-loading", "Loading board…" }
            } else {
                KanbanBoard {
                    tasks: tasks,
                    archived_visible: archived_visible_ro,
                    on_open_drawer: on_open_drawer,
                    dragged_task_id: dragged_task_id,
                    pending_task_ids: pending_task_ids,
                    toast_msg: toast_msg,
                    live_region_msg: live_region_msg,
                    archive_modal_task: archive_modal_task,
                    on_triage_action: on_triage_action,
                    active_profile: active_profile_ro,
                }
            }
            // UI-SPEC §7.5: optimistic-revert toast surface (visible).
            // GAP-10 (47.4-16 Task 1): the toast now carries an explicit
            // dismiss control and moves off the drawer's corner while a
            // drawer is open — see `drawer_open` above. No auto-dismiss
            // timer: the message may be failure text the operator has not
            // read yet.
            if let Some(toast) = toast_text {
                div {
                    class: "kn-toast",
                    role: "status",
                    aria_live: "polite",
                    "data-drawer-open": if drawer_open { "true" },
                    span { "{toast}" }
                    button {
                        class: "kn-toast-dismiss",
                        "aria-label": "Dismiss board update",
                        onclick: move |_| {
                            let mut tm = toast_msg;
                            tm.set(None);
                        },
                        "✕"
                    }
                }
            }
            // Plan 03 (D-20 / UI-SPEC §3.9): the slide-in detail drawer.
            // Mounted unconditionally — TaskDrawer itself renders nothing
            // until `task_id.read().is_some()`, but mounting it
            // unconditionally keeps the use_resource hooks registered
            // across opens/closes (Pattern E discipline).
            TaskDrawer {
                task_id: drawer_task_id_ro,
                per_task_event_counter: per_task_counter_ro,
                profile_refresh_tick: profile_list_refresh_tick_ro,
                on_close: on_drawer_close,
                on_open_complete_modal: on_open_complete,
                on_open_block_modal: on_open_block,
                on_open_archive_modal: on_open_archive,
                on_unblock: on_unblock,
                on_decompose: on_decompose,
                on_specify: on_specify,
                on_post_comment: on_post_comment,
            }
            // Phase 47.4 Plan 08 (D-02/D-04/D-10): the profile detail
            // drawer — mounted unconditionally, same discipline as
            // TaskDrawer above. Opened from ProfileSwitcher's EDIT chip /
            // MANAGE ALL PROFILES footer link via `detail_profile`.
            crate::components::hermes_app::screens::kanban::profile_drawer::ProfileDetailDrawer {
                profile_id: detail_profile,
                // Phase 50.1 Plan 02 (D-10): explicit at the call site, not
                // implicit in the default.
                context: ProfileDialogContext::Kanban,
                on_close: move |_| detail_profile.set(None),
                on_profile_updated: move |_| {
                    // Refreshes the dropdown's profile list the same way
                    // the wizard's on_created handler already does — key-
                    // remounting ProfileSwitcher forces its own internal
                    // fetch, never a resource-restart call on this file's
                    // own resources.
                    let cur = *profile_list_refresh_tick.read();
                    profile_list_refresh_tick.set(cur + 1);
                },
            }
            // Plan 03 (D-13 / UI-SPEC §3.10): modals — render conditionally
            // based on the modal-target signals.
            if let Some(tid) = complete_modal_task.read().clone() {
                CompleteModal {
                    task_id: tid,
                    on_dismiss: move |_| { close_complete_modal(); },
                    on_success: move |_| {
                        close_complete_modal();
                        open_drawer_task_id.set(None);
                        restart_board();
                    },
                }
            }
            if let Some(tid) = block_modal_task.read().clone() {
                BlockModal {
                    task_id: tid,
                    on_dismiss: move |_| { close_block_modal(); },
                    on_success: move |_| {
                        close_block_modal();
                        restart_board();
                    },
                }
            }
            if let Some(tid) = archive_modal_task.read().clone() {
                ArchiveConfirmModal {
                    task_id: tid,
                    on_dismiss: move |_| { close_archive_modal(); },
                    on_success: move |_| {
                        close_archive_modal();
                        open_drawer_task_id.set(None);
                        restart_board();
                    },
                }
            }
            if *create_modal_open.read() {
                CreateTaskModal {
                    on_dismiss: move |_| { close_create_modal(); },
                    on_success: move |_| {
                        close_create_modal();
                        restart_board();
                    },
                    // Phase 47.4 Plan 09 (D-05 + D-12): seed the assignee
                    // field with the active lens profile, if any — an
                    // initial value only, still freely editable. `None`
                    // seeds an empty string exactly as before this plan.
                    initial_assignee: active_profile.read().clone(),
                }
            }
            // Phase 47.4 Plan 07 (D-01): the Create Kanban Profile wizard,
            // mounted conditionally exactly like `CreateTaskModal` above.
            if *wizard_open.read() {
                CreateProfileWizard {
                    // Phase 50.1 Plan 02 (D-10): explicit at the call site,
                    // not implicit in the default.
                    context: ProfileDialogContext::Kanban,
                    on_dismiss: move |_| wizard_open.set(false),
                    on_created: move |_name: String| {
                        let cur = *profile_list_refresh_tick.read();
                        profile_list_refresh_tick.set(cur + 1);
                    },
                }
            }
        }
    }
}
