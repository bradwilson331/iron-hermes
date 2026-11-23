//! Phase 54.10 Plan 06 (D-13/D-14/D-16/D-17): the Dashboard canvas.
//!
//! One viewport `div` holds one transformed world layer; items sit on it at `left/top/width/
//! height` in world units. The board working copy and every signal the canvas shares with the
//! toolbar live in [`BoardState`] (created by `ScreenDashboard`, passed as a prop: no context
//! provider below the HermesApp root).

use super::EmptyBoardState;
use super::items::{
    BoardItemView, ItemAction, SavedViews, WidgetEnv, apply_item_rect, focus_dom,
    focus_target_after_remove, item_dom_id, item_title_with, new_item_at, raise, reading_order_ids,
    rect_of,
};
use super::palette::{PaletteOpen, open_palette_from_canvas};
use super::present::{
    CANVAS_WRAP_ID, PresentMode, end_present, present_dispatch, present_forced, present_key,
    present_steps, start_present,
};
use super::widgets::pending_view_to_delete;
use crate::canvas_math::{
    CanvasCmd, FRAME_CAP, GRID, PAN_KEY_STEP, Rect, View, WHEEL_FACTOR, canvas_shortcut, fit,
    grid_visible, lod_full, screen_to_world, step_zoom, visible_set, zoom_at,
};
use crate::components::hermes_app::dashboard_runtime::DashboardGenCtx;
use crate::platform::pointer::{capture_pointer, release_pointer};
use crate::server::dashboard_api::{
    BOARD_ITEMS_FULL_ERROR, BOARD_ITEMS_MAX, Board, BoardItem, COORD_MAX, ItemKind,
    NOTE_TEXT_MAX_CHARS,
};
use crate::server::openui_views_api::delete_openui_view;
use dioxus::core::use_drop;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use std::rc::Rc;

/// DOM id of the canvas region (focus target when no item remains).
pub(crate) const CANVAS_DOM_ID: &str = "ih-dash-canvas";

/// UI-SPEC A9: a stacked item's height is its stored height clamped to 192..=480.
pub(crate) fn stacked_height(h: i32) -> i32 {
    h.clamp(192, 480)
}

/// Everything the canvas and the toolbar share. All fields are `Copy` signals.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct BoardState {
    /// The board working copy; replaced only by handlers, never by an effect that reads it.
    pub board: Signal<Option<Board>>,
    pub view: Signal<View>,
    /// True for Fit / step zoom (200ms ease); false for drag, pan and wheel.
    pub animate: Signal<bool>,
    pub selected: Signal<Option<String>>,
    pub announce: Signal<String>,
    /// Bumped by every committed change; the autosave loop watches it.
    pub save_rev: Signal<u32>,
    /// A visible one-line notice (for example the item cap).
    pub notice: Signal<Option<String>>,
    /// The note just created: its textarea takes focus, and it goes away again if left empty.
    pub fresh: Signal<Option<String>>,
    /// Unix seconds, refreshed by one screen-level 15s tick (tile `Updated {relative}` labels).
    pub now: Signal<i64>,
    pub vp_el: Signal<Option<Rc<MountedData>>>,
    /// The saved OpenUI views AI widgets resolve against (one fetch for the screen).
    pub views: Signal<SavedViews>,
    /// The widget whose frame currently owns pointer input (UI-SPEC A5); at most one.
    pub active: Signal<Option<String>>,
    /// Viewport size in client px, for the frame cap. Measured on mount, on show and after a pan.
    pub vp_size: Signal<(f64, f64)>,
    /// The `/` palette while it is open (D-14).
    pub palette: Signal<Option<PaletteOpen>>,
    /// The preset chooser (D-14) is open over the current board.
    pub chooser: Signal<bool>,
    /// Present mode (D-14): the current step (0 = overview), or `None` while editing.
    pub present: Signal<Option<usize>>,
    /// The editing view to restore when Present ends.
    pub present_saved: Signal<View>,
    /// Bumped by pointer movement in Present (drives the strip's idle hide).
    pub present_moves: Signal<u32>,
}

pub(crate) fn use_board_state(initial: Option<Board>) -> BoardState {
    BoardState {
        board: use_signal(|| initial),
        view: use_signal(View::default),
        animate: use_signal(|| false),
        selected: use_signal(|| None),
        announce: use_signal(String::new),
        save_rev: use_signal(|| 0),
        notice: use_signal(|| None),
        fresh: use_signal(|| None),
        now: use_signal(crate::components::hermes_app::screens::openui::now_unix_secs),
        vp_el: use_signal(|| None),
        views: use_signal(|| None),
        active: use_signal(|| None),
        vp_size: use_signal(|| (1280.0, 720.0)),
        palette: use_signal(|| None),
        chooser: use_signal(|| false),
        present: use_signal(|| None),
        present_saved: use_signal(View::default),
        present_moves: use_signal(|| 0),
    }
}

/// Widget ids whose frame is mounted (UI-SPEC A5): at most [`FRAME_CAP`], nearest the viewport
/// centre, the selected widget forced in. Below the level-of-detail zoom nothing is mounted.
pub(crate) fn mounted_widget_ids(
    items: &[BoardItem],
    view: View,
    vp_size: (f64, f64),
    selected: Option<&str>,
) -> Vec<String> {
    let forced: Vec<String> = selected.map(String::from).into_iter().collect();
    mounted_with_forced(items, view, vp_size, &forced)
}

/// [`mounted_widget_ids`] with any number of forced ids (Present: the current and next widget).
fn mounted_with_forced(
    items: &[BoardItem],
    view: View,
    vp_size: (f64, f64),
    forced: &[String],
) -> Vec<String> {
    if !lod_full(view.zoom) {
        return Vec::new();
    }
    let rects: Vec<(String, Rect)> = items
        .iter()
        .filter(|i| matches!(i.kind, ItemKind::Widget { .. }))
        .map(|i| (i.id.clone(), rect_of(i)))
        .collect();
    let forced: Vec<String> = forced
        .iter()
        .filter(|s| rects.iter().any(|(id, _)| id == *s))
        .cloned()
        .collect();
    let world = (
        -view.pan_x / view.zoom,
        -view.pan_y / view.zoom,
        vp_size.0 / view.zoom,
        vp_size.1 / view.zoom,
    );
    visible_set(world, view.zoom, &rects, &forced, FRAME_CAP)
}

impl BoardState {
    pub(crate) fn say(self, msg: impl Into<String>) {
        let mut a = self.announce;
        a.set(msg.into());
    }

    /// Mutate the open board's items and schedule an autosave.
    pub(crate) fn edit(self, f: impl FnOnce(&mut Vec<BoardItem>)) {
        let (mut board, mut rev) = (self.board, self.save_rev);
        if let Some(b) = board.write().as_mut() {
            f(&mut b.items);
        }
        rev += 1;
    }

    /// Header title of an item, with an AI widget showing its saved view's name.
    fn title_of(self, kind: &ItemKind) -> String {
        item_title_with(kind, &self.views.peek())
    }

    pub(crate) fn items(self) -> Vec<BoardItem> {
        self.board
            .peek()
            .as_ref()
            .map(|b| b.items.clone())
            .unwrap_or_default()
    }
}

/// Viewport `(client x, client y, width, height)`.
pub(crate) async fn measure(vp: Signal<Option<Rc<MountedData>>>) -> Option<(f64, f64, f64, f64)> {
    let el = vp.peek().clone()?;
    let r = el.get_client_rect().await.ok()?;
    Some((r.origin.x, r.origin.y, r.size.width, r.size.height))
}

/// Store a measured viewport size (drives the frame cap) when it changed.
fn store_vp_size(st: BoardState, w: f64, h: f64) {
    let mut size = st.vp_size;
    if *size.peek() != (w, h) {
        size.set((w, h));
    }
}

/// Refresh the stored viewport size (drives the frame cap).
async fn remeasure(st: BoardState) {
    if let Some((_, _, w, h)) = measure(st.vp_el).await {
        store_vp_size(st, w, h);
    }
}

/// [`remeasure`] once a layout change (window resize, Plan panel, leaving Present or fullscreen)
/// has settled.
pub(crate) async fn remeasure_settled(st: BoardState) {
    crate::platform::timer::sleep(120).await;
    remeasure(st).await;
}

/// `Zoom {pct} percent.` source: whole percent of the zoom factor.
pub(crate) fn zoom_pct(zoom: f64) -> u32 {
    (zoom * 100.0).round() as u32
}

/// The new view for a zoom/fit command, or `None` for any other command. Zoom steps and the
/// reset pivot on the viewport centre; Fit frames every item (capped at 100%, UI-SPEC A3).
pub(crate) fn run_view_cmd(
    cmd: CanvasCmd,
    view: View,
    size: (f64, f64),
    rects: &[Rect],
) -> Option<View> {
    let center = (size.0 / 2.0, size.1 / 2.0);
    match cmd {
        CanvasCmd::ZoomIn => Some(step_zoom(view, true, center)),
        CanvasCmd::ZoomOut => Some(step_zoom(view, false, center)),
        CanvasCmd::ZoomReset => Some(zoom_at(view, 1.0 / view.zoom, center)),
        CanvasCmd::Fit => Some(fit(rects, size, 1.0)),
        CanvasCmd::Palette | CanvasCmd::Note | CanvasCmd::Present => None,
    }
}

/// Wheel gesture on the canvas: Ctrl/Cmd (and trackpad pinch, which arrives with ctrlKey) zooms
/// around the cursor, 1.1x per 100 units of delta; anything else pans. `lines` is a
/// line-based delta (Firefox mouse wheel). Non-finite deltas change nothing.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn wheel_view(
    view: View,
    delta: (f64, f64),
    zoom_mode: bool,
    cursor: (f64, f64),
    lines: bool,
) -> View {
    if !(delta.0.is_finite() && delta.1.is_finite()) {
        return view;
    }
    let k = if lines { 16.0 } else { 1.0 };
    if zoom_mode {
        let dy = (delta.1 * k).clamp(-300.0, 300.0);
        zoom_at(view, WHEEL_FACTOR.powf(-dy / 100.0), cursor)
    } else {
        View {
            pan_x: view.pan_x - delta.0 * k,
            pan_y: view.pan_y - delta.1 * k,
            ..view
        }
    }
}

impl BoardState {
    /// Zoom / Fit from the toolbar or the keyboard. Measures the viewport first.
    pub(crate) fn view_cmd(self, cmd: CanvasCmd) {
        spawn(async move {
            let Some((_, _, w, h)) = measure(self.vp_el).await else {
                return;
            };
            store_vp_size(self, w, h);
            let rects: Vec<Rect> = self.items().iter().map(rect_of).collect();
            let cur = *self.view.peek();
            if let Some(v) = run_view_cmd(cmd, cur, (w, h), &rects) {
                let (mut view, mut animate) = (self.view, self.animate);
                animate.set(true);
                view.set(v);
                self.say(format!("Zoom {} percent.", zoom_pct(v.zoom)));
            }
        });
    }
}

/// Opening a board frames all its items once the viewport has a size (it has none while the
/// screen is hidden, so the screen becoming active retries).
async fn fit_open(st: BoardState, mut fitted: Signal<bool>) {
    if *fitted.peek() {
        return;
    }
    let Some((_, _, w, h)) = measure(st.vp_el).await else {
        return;
    };
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let rects: Vec<Rect> = st.items().iter().map(rect_of).collect();
    let (mut view, mut animate, mut size) = (st.view, st.animate, st.vp_size);
    size.set((w, h));
    animate.set(false);
    view.set(fit(&rects, (w, h), 1.0));
    fitted.set(true);
}

#[cfg(target_arch = "wasm32")]
type WheelListener = (
    web_sys::Element,
    wasm_bindgen::closure::Closure<dyn FnMut(web_sys::WheelEvent)>,
);
#[cfg(not(target_arch = "wasm32"))]
type WheelListener = ();

/// Raw NON-passive `wheel` listener (RESEARCH Pitfall 6): a Dioxus `onwheel` is passive, so it
/// could not stop the browser zooming the page on Ctrl+wheel / pinch. Installed once, through
/// the `listener_slot` + `use_drop` idiom of openui.rs. The native twin does nothing.
fn install_wheel(el: &MountedData, mut slot: Signal<Option<WheelListener>>, st: BoardState) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        // The viewport element is re-created when the window crosses the narrow breakpoint,
        // so a stale listener on the old element is replaced, not kept.
        remove_wheel(slot);
        let Some(elem) = el.downcast::<web_sys::Element>() else {
            return;
        };
        let elem = elem.clone();
        let target = elem.clone();
        let cb = Closure::<dyn FnMut(web_sys::WheelEvent)>::new(move |ev: web_sys::WheelEvent| {
            // A plain wheel over scrollable content (a long note, a tile list) scrolls that
            // content; only Ctrl/Cmd (zoom, and pinch) and empty canvas are the canvas's.
            if !(ev.ctrl_key() || ev.meta_key()) && over_scrollable(&ev, &target) {
                return;
            }
            ev.prevent_default();
            if st.present.peek().is_some() {
                return;
            }
            let r = target.get_bounding_client_rect();
            store_vp_size(st, r.width(), r.height());
            let cursor = (
                f64::from(ev.client_x()) - r.left(),
                f64::from(ev.client_y()) - r.top(),
            );
            let (mut view, mut animate) = (st.view, st.animate);
            animate.set(false);
            let next = wheel_view(
                *view.peek(),
                (ev.delta_x(), ev.delta_y()),
                ev.ctrl_key() || ev.meta_key(),
                cursor,
                ev.delta_mode() == 1,
            );
            view.set(next);
        });
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_passive(false);
        let _ = elem.add_event_listener_with_callback_and_add_event_listener_options(
            "wheel",
            cb.as_ref().unchecked_ref(),
            &opts,
        );
        slot.set(Some((elem, cb)));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (el, &mut slot, st);
}

/// True when the wheel event's target, or an ancestor below `viewport`, scrolls vertically.
#[cfg(target_arch = "wasm32")]
fn over_scrollable(ev: &web_sys::WheelEvent, viewport: &web_sys::Element) -> bool {
    use wasm_bindgen::JsCast;
    let window = web_sys::window();
    let mut node = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok());
    while let Some(el) = node {
        if el == *viewport {
            return false;
        }
        if el.scroll_height() > el.client_height() {
            let overflow = window
                .as_ref()
                .and_then(|w| w.get_computed_style(&el).ok().flatten())
                .and_then(|c| c.get_property_value("overflow-y").ok())
                .unwrap_or_default();
            if overflow == "auto" || overflow == "scroll" {
                return true;
            }
        }
        node = el.parent_element();
    }
    false
}

fn remove_wheel(mut slot: Signal<Option<WheelListener>>) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some((el, cb)) = slot.write().take() {
            let _ = el.remove_event_listener_with_callback("wheel", cb.as_ref().unchecked_ref());
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &mut slot;
}

/// Window width for the narrow-screen switch (D-17). Native twin: a wide window.
pub(crate) fn window_inner_width() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|v| v.as_f64())
            .unwrap_or(1280.0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        1280.0
    }
}

/// True on macOS, where the hint strip says `Cmd`. Native twin: false.
pub(crate) fn is_mac_platform() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.navigator().platform().ok())
            .is_some_and(|p| p.contains("Mac"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) type ResizeListener = (web_sys::Window, wasm_bindgen::closure::Closure<dyn FnMut()>);
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type ResizeListener = ();

/// One window `resize` listener feeding the width signal (`listener_slot` + `use_drop`).
pub(crate) fn install_resize(mut width: Signal<f64>, mut slot: Signal<Option<ResizeListener>>) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        if slot.peek().is_some() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let cb = Closure::<dyn FnMut()>::new(move || width.set(window_inner_width()));
        let _ = window.add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref());
        slot.set(Some((window, cb)));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (&mut width, &mut slot);
}

pub(crate) fn remove_resize(mut slot: Signal<Option<ResizeListener>>) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some((window, cb)) = slot.write().take() {
            let _ =
                window.remove_event_listener_with_callback("resize", cb.as_ref().unchecked_ref());
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &mut slot;
}

/// UI-SPEC "Hint strip": the canvas footer on wide screens (`Cmd` on macOS).
pub(crate) fn hint_text(mac: bool) -> String {
    let modifier = if mac { "Cmd" } else { "Ctrl" };
    format!(
        "/ add · N note · Space + drag pan · {modifier} + scroll zoom · arrows move · Shift + arrows resize"
    )
}

/// UI-SPEC A9 notice strip.
#[component]
fn NarrowNotice() -> Element {
    rsx! {
        div { class: "ih-notice", role: "note",
            span { class: "pill ih-notice__pill", "READ-ONLY" }
            span { "Editing needs a wider window. Widen it to at least 900px to move or add items." }
        }
    }
}

/// UI-SPEC A2 zoom group: `−`, `{zoom}%` (resets to 100%), `+`, `Fit`.
#[component]
pub(crate) fn ZoomGroup(view: Signal<View>, on_cmd: EventHandler<CanvasCmd>) -> Element {
    let pct = zoom_pct(view.read().zoom);
    rsx! {
        div { class: "ih-zoom", role: "group", "aria-label": "Zoom",
            button {
                class: "btn btn--sm btn--ghost btn--icon",
                "aria-label": "Zoom out",
                title: "Zoom out",
                onclick: move |_| on_cmd.call(CanvasCmd::ZoomOut),
                "−"
            }
            button {
                class: "btn btn--sm btn--ghost",
                title: "Reset zoom to 100%",
                onclick: move |_| on_cmd.call(CanvasCmd::ZoomReset),
                "{pct}%"
            }
            button {
                class: "btn btn--sm btn--ghost btn--icon",
                "aria-label": "Zoom in",
                title: "Zoom in",
                onclick: move |_| on_cmd.call(CanvasCmd::ZoomIn),
                "+"
            }
            button {
                class: "btn btn--sm btn--ghost",
                title: "Fit all items",
                onclick: move |_| on_cmd.call(CanvasCmd::Fit),
                "Fit"
            }
        }
    }
}

/// Add an item of `kind` at the pointer when it is over the canvas, else at the viewport centre
/// (D-14: `N` and the Add menu). Selects it; a note also takes focus.
pub(crate) fn add_item(st: BoardState, kind: ItemKind, ptr: Option<(f64, f64)>) {
    spawn(async move {
        let is_note = matches!(kind, ItemKind::Note { .. });
        match place_item(st, kind, ptr).await {
            Some(id) if !is_note => focus_placed(id).await,
            _ => {}
        }
    });
}

/// The item mounts on the next render; focus it then.
pub(crate) async fn focus_placed(id: String) {
    crate::platform::timer::sleep(50).await;
    focus_dom(&item_dom_id(&id));
}

/// Place a new item (see [`add_item`]) and return its id, or `None` when the board is full.
pub(crate) async fn place_item(
    st: BoardState,
    kind: ItemKind,
    ptr: Option<(f64, f64)>,
) -> Option<String> {
    let (x0, y0, w, h) = measure(st.vp_el).await.unwrap_or((0.0, 0.0, 800.0, 480.0));
    let screen = ptr
        .map(|(cx, cy)| (cx - x0, cy - y0))
        .filter(|(sx, sy)| (0.0..=w).contains(sx) && (0.0..=h).contains(sy))
        .unwrap_or((w / 2.0, h / 2.0));
    let (wx, wy) = screen_to_world(screen, *st.view.peek());
    let lim = f64::from(COORD_MAX);
    let center = (wx.clamp(-lim, lim) as i32, wy.clamp(-lim, lim) as i32);
    let items = st.items();
    let mut notice = st.notice;
    if items.len() >= BOARD_ITEMS_MAX {
        notice.set(Some(BOARD_ITEMS_FULL_ERROR.to_string()));
        st.say(BOARD_ITEMS_FULL_ERROR);
        return None;
    }
    notice.set(None);
    let is_note = matches!(kind, ItemKind::Note { .. });
    let item = new_item_at(&items, center, kind);
    let (id, title) = (item.id.clone(), st.title_of(&item.kind));
    st.edit(|items| items.push(item));
    let mut selected = st.selected;
    selected.set(Some(id.clone()));
    st.say(format!("{title} added to the board."));
    if is_note {
        let mut fresh = st.fresh;
        fresh.set(Some(id.clone()));
    }
    Some(id)
}

/// Apply one item event to the working copy.
fn apply_action(
    st: BoardState,
    mut fresh: Signal<Option<String>>,
    mut confirming: Signal<Option<String>>,
    gen_ctx: Option<DashboardGenCtx>,
    id: String,
    act: ItemAction,
) {
    let mut selected = st.selected;
    let title = st
        .items()
        .iter()
        .find(|i| i.id == id)
        .map(|i| st.title_of(&i.kind))
        .unwrap_or_default();
    match act {
        ItemAction::Select => {
            selected.set(Some(id.clone()));
            if st.items().last().is_some_and(|l| l.id != id) {
                st.edit(|items| {
                    raise(items, &id);
                });
            }
        }
        ItemAction::Moved(r) => {
            selected.set(Some(id.clone()));
            st.edit(|items| apply_item_rect(items, &id, r));
            if let Some(i) = st.items().iter().find(|i| i.id == id) {
                st.say(format!("{title} moved to x {}, y {}.", i.x, i.y));
            }
        }
        ItemAction::Resized(r) => {
            selected.set(Some(id.clone()));
            st.edit(|items| apply_item_rect(items, &id, r));
            if let Some(i) = st.items().iter().find(|i| i.id == id) {
                st.say(format!("{title} resized to {} by {}.", i.w, i.h));
            }
        }
        ItemAction::Text(text) => {
            let text: String = text.chars().take(NOTE_TEXT_MAX_CHARS).collect();
            st.edit(|items| {
                if let Some(ItemKind::Note { text: t }) =
                    items.iter_mut().find(|i| i.id == id).map(|i| &mut i.kind)
                {
                    *t = text;
                }
            });
        }
        ItemAction::TileSettings(spec) => {
            st.edit(|items| {
                if let Some(ItemKind::Tile { tile }) =
                    items.iter_mut().find(|i| i.id == id).map(|i| &mut i.kind)
                {
                    *tile = spec;
                }
            });
        }
        ItemAction::RequestRemove => confirming.set(Some(id)),
        ItemAction::CancelRemove => {
            confirming.set(None);
            focus_dom(&item_dom_id(&id));
        }
        ItemAction::ConfirmRemove => {
            remove_item(st, gen_ctx, &id, &title);
            confirming.set(None);
        }
        ItemAction::Escape => {
            let mut active = st.active;
            if active.peek().is_some() {
                active.set(None);
            }
            if confirming.peek().is_some() {
                confirming.set(None);
                focus_dom(&item_dom_id(&id));
            } else {
                selected.set(None);
                focus_dom(&item_dom_id(&id));
            }
        }
        ItemAction::NoteBlur => {
            let was_fresh = fresh.peek().as_deref() == Some(id.as_str());
            if was_fresh {
                fresh.set(None);
                let empty = st.items().iter().any(|i| {
                    i.id == id && matches!(&i.kind, ItemKind::Note { text } if text.is_empty())
                });
                // A just-created note left empty goes away without a confirm (UI-SPEC A4).
                if empty {
                    st.edit(|items| items.retain(|i| i.id != id));
                    if st.selected.peek().as_deref() == Some(id.as_str()) {
                        selected.set(None);
                    }
                }
            }
        }
    }
}

fn remove_item(st: BoardState, gen_ctx: Option<DashboardGenCtx>, id: &str, title: &str) {
    // The widget's queued or running generation job goes with it (B-05).
    if let Some(ctx) = gen_ctx {
        ctx.cancel_item.call(id.to_string());
    }
    let items = st.items();
    // RESEARCH A9: a never-generated widget owns its pending view, so removing it deletes it.
    let pending = pending_view_to_delete(&items, id, &st.views.peek());
    let target = focus_target_after_remove(&reading_order_ids(&items), id);
    st.edit(|items| items.retain(|i| i.id != id));
    if let Some(view_id) = pending {
        spawn(async move {
            // A failed delete must not block the removal: the view just stays listed in OpenUI.
            let _ = delete_openui_view(view_id).await;
            if let Some(mut rev) = gen_ctx.map(|c| c.views_rev) {
                rev.with_mut(|r| *r += 1);
            }
        });
    }
    let mut selected = st.selected;
    selected.set(None);
    st.say(format!("{title} removed."));
    match target {
        Some(t) => focus_dom(&item_dom_id(&t)),
        None => focus_dom(CANVAS_DOM_ID),
    }
}

/// The board canvas. Mounted keyed by board id, so opening another board starts fresh.
#[component]
pub(crate) fn DashboardCanvas(
    st: BoardState,
    board_name: String,
    is_active: bool,
    narrow: bool,
    mac: bool,
) -> Element {
    let fresh = st.fresh;
    let confirming = use_signal(|| None::<String>);
    let mut last_ptr = use_signal(|| None::<(f64, f64)>);
    let mut space = use_signal(|| false);
    // Pan gesture: pointer start (client px) and the view when it began.
    let mut pan = use_signal(|| None::<(f64, f64, View)>);
    let fitted = use_signal(|| false);
    let wheel_slot: Signal<Option<WheelListener>> = use_signal(|| None);
    use_drop(move || remove_wheel(wheel_slot));
    // The screen is hidden (no viewport size) until first shown: fit then.
    use_effect(use_reactive!(|is_active| {
        if is_active {
            spawn(fit_open(st, fitted));
        }
    }));

    // The read-only column has no Present: a window narrowed mid-presentation ends it.
    use_effect(use_reactive!(|narrow| {
        if narrow {
            end_present(st);
        }
    }));
    let items = st
        .board
        .read()
        .as_ref()
        .map(|b| b.items.clone())
        .unwrap_or_default();
    let present_step = *st.present.read();
    // Leaving Present (or fullscreen) changes the viewport's size, so the frame cap is
    // re-measured then. Window and Plan-panel changes do the same from the screen's own effect.
    use_effect(move || {
        if st.present.read().is_none() {
            spawn(remeasure_settled(st));
        }
    });
    let v = *st.view.read();
    let animate = *st.animate.read();
    let panning = *space.read();
    let selected = if present_step.is_some() {
        None
    } else {
        st.selected.read().clone()
    };
    let now = *st.now.read();
    let confirm_id = confirming.read().clone();
    let fresh_id = fresh.read().clone();
    let widget_env = WidgetEnv {
        views: st.views,
        active: st.active,
    };
    // Narrow: the first FRAME_CAP widgets of the column; wide: the frame cap around the viewport.
    let mounted: Vec<String> = if narrow {
        reading_order_ids(&items)
            .into_iter()
            .filter(|id| {
                items
                    .iter()
                    .any(|i| &i.id == id && matches!(i.kind, ItemKind::Widget { .. }))
            })
            .take(FRAME_CAP)
            .collect()
    } else if let Some(step) = present_step {
        let widget_ids: Vec<String> = items
            .iter()
            .filter(|i| matches!(i.kind, ItemKind::Widget { .. }))
            .map(|i| i.id.clone())
            .collect();
        let forced = present_forced(step, &present_steps(&items), &widget_ids);
        mounted_with_forced(&items, v, *st.vp_size.read(), &forced)
    } else {
        mounted_widget_ids(&items, v, *st.vp_size.read(), selected.as_deref())
    };
    let world_style = format!(
        "transform: translate({:.2}px, {:.2}px) scale({:.4});",
        v.pan_x, v.pan_y, v.zoom
    );
    // Dot grid: 1px dots every 16 world px, hidden where it would moire.
    let grid_style = if grid_visible(v.zoom) {
        let step = f64::from(GRID) * v.zoom;
        format!(
            "background-image: radial-gradient(circle, var(--border-faint) 1px, transparent 1px); \
             background-size: {step:.2}px {step:.2}px; background-position: {:.2}px {:.2}px;",
            v.pan_x, v.pan_y
        )
    } else {
        String::new()
    };
    let gen_ctx = try_use_context::<DashboardGenCtx>();
    let on_action = move |(id, act): (String, ItemAction)| {
        apply_action(st, fresh, confirming, gen_ctx, id, act)
    };
    let order = reading_order_ids(&items);
    let empty = items.is_empty();

    // D-17: narrow windows get the read-only stacked column. No handler is attached and no
    // pan, zoom, drag, resize, remove or add control is rendered.
    if narrow {
        return rsx! {
            div { class: "ih-canvas-wrap ih-canvas-wrap--narrow",
                NarrowNotice {}
                div {
                    id: CANVAS_DOM_ID,
                    class: "ih-stack",
                    role: "region",
                    "aria-label": "Dashboard canvas: {board_name}",
                    if empty {
                        EmptyBoardState {}
                    }
                    for (pos , id) in order.into_iter().enumerate() {
                        if let Some((z, item)) = items.iter().enumerate().find(|(_, i)| i.id == id) {
                            BoardItemView {
                                key: "{id}",
                                item: item.clone(),
                                z,
                                zoom: 1.0,
                                selected: false,
                                confirming: false,
                                fresh: false,
                                read_only: true,
                                pan_mode: false,
                                tile_index: pos,
                                now,
                                widget_env,
                                mounted: mounted.contains(&id),
                                on_action,
                            }
                        }
                    }
                }
            }
        };
    }
    let hint = hint_text(mac);

    let end_pan = move |pointer_id: i32| {
        let mut pan = pan;
        if pan.peek().is_some() {
            pan.set(None);
            spawn(remeasure(st));
            if let Some(el) = st.vp_el.peek().as_ref() {
                release_pointer(el, pointer_id);
            }
        }
    };

    rsx! {
        div {
            id: CANVAS_WRAP_ID,
            class: "ih-canvas-wrap",
            class: if present_step.is_some() { "ih-canvas-wrap--present" },
            onpointermove: move |_| {
                if st.present.peek().is_some() {
                    let mut moves = st.present_moves;
                    moves += 1;
                }
            },
            div {
                id: CANVAS_DOM_ID,
                class: "ih-canvas",
                class: if panning { "ih-canvas--pan" },
                role: "region",
                "aria-label": "Dashboard canvas: {board_name}",
                tabindex: "0",
                style: "{grid_style}",
                onmounted: move |m| {
                    let data = m.data();
                    install_wheel(&data, wheel_slot, st);
                    let mut vp = st.vp_el;
                    vp.set(Some(data));
                    spawn(fit_open(st, fitted));
                },
                onpointerdown: move |e| {
                    if e.trigger_button() != Some(MouseButton::Primary)
                        || st.present.peek().is_some()
                    {
                        return;
                    }
                    let mut selected = st.selected;
                    let mut animate = st.animate;
                    let mut active = st.active;
                    // A press on empty canvas (or Space+drag anywhere) pans.
                    if !*space.peek() {
                        selected.set(None);
                        active.set(None);
                    }
                    animate.set(false);
                    if let Some(el) = st.vp_el.peek().as_ref() {
                        capture_pointer(el, e.pointer_id());
                    }
                    let p = e.client_coordinates();
                    pan.set(Some((p.x, p.y, *st.view.peek())));
                },
                onpointermove: move |e| {
                    let p = e.client_coordinates();
                    last_ptr.set(Some((p.x, p.y)));
                    let start = *pan.peek();
                    if let Some((sx, sy, v0)) = start {
                        let mut view = st.view;
                        view.set(View {
                            pan_x: v0.pan_x + (p.x - sx),
                            pan_y: v0.pan_y + (p.y - sy),
                            ..v0
                        });
                    }
                },
                onpointerup: move |e| end_pan(e.pointer_id()),
                onpointercancel: move |e| end_pan(e.pointer_id()),
                onlostpointercapture: move |e| end_pan(e.pointer_id()),
                onkeyup: move |e| {
                    if e.key().to_string() == " " {
                        space.set(false);
                    }
                },
                // The Space keyup can land elsewhere (Alt-Tab, focus moved to a field), which
                // would leave pan mode stuck on.
                onfocusout: move |_| {
                    if *space.peek() {
                        space.set(false);
                    }
                },
                onkeydown: move |e| {
                    let m = e.modifiers();
                    let key = e.key().to_string();
                    let plain = !(m.ctrl() || m.meta() || m.alt());
                    // Present: the slideshow keys only. Enter still reaches the focused item.
                    if st.present.peek().is_some() {
                        if let (true, Some(action)) = (plain, present_key(&key)) {
                            e.prevent_default();
                            present_dispatch(st, action);
                        }
                        return;
                    }
                    if key == " " {
                        e.prevent_default();
                        if !*space.peek() {
                            space.set(true);
                        }
                        return;
                    }
                    // Arrows reach here only from the region itself: items and fields stop theirs.
                    let pan_step = match key.as_str() {
                        "ArrowLeft" => Some((PAN_KEY_STEP, 0.0)),
                        "ArrowRight" => Some((-PAN_KEY_STEP, 0.0)),
                        "ArrowUp" => Some((0.0, PAN_KEY_STEP)),
                        "ArrowDown" => Some((0.0, -PAN_KEY_STEP)),
                        _ => None,
                    };
                    if let (true, Some((dx, dy))) = (plain, pan_step) {
                        e.prevent_default();
                        let (mut view, mut animate) = (st.view, st.animate);
                        let cur = *view.peek();
                        animate.set(false);
                        view.set(View {
                            pan_x: cur.pan_x + dx,
                            pan_y: cur.pan_y + dy,
                            ..cur
                        });
                        spawn(remeasure(st));
                        return;
                    }
                    let frame_active = st.active.peek().is_some();
                    match canvas_shortcut(&key, m.ctrl() || m.meta(), m.alt(), false, frame_active) {
                        Some(CanvasCmd::Note) => {
                            e.prevent_default();
                            let note = ItemKind::Note {
                                text: String::new(),
                            };
                            add_item(st, note, *last_ptr.peek());
                        }
                        Some(
                            cmd @ (CanvasCmd::ZoomIn
                            | CanvasCmd::ZoomOut
                            | CanvasCmd::ZoomReset
                            | CanvasCmd::Fit),
                        ) => {
                            e.prevent_default();
                            st.view_cmd(cmd);
                        }
                        Some(CanvasCmd::Palette) => {
                            e.prevent_default();
                            open_palette_from_canvas(st, *last_ptr.peek());
                        }
                        Some(CanvasCmd::Present) => {
                            e.prevent_default();
                            start_present(st);
                        }
                        _ => {}
                    }
                },
                div {
                    class: "ih-world",
                    class: if animate { "ih-world--animate" },
                    style: "{world_style}",
                    for (pos , id) in order.into_iter().enumerate() {
                        if let Some((z, item)) = items.iter().enumerate().find(|(_, i)| i.id == id) {
                            BoardItemView {
                                key: "{id}",
                                item: item.clone(),
                                z,
                                zoom: v.zoom,
                                selected: selected.as_deref() == Some(id.as_str()),
                                confirming: confirm_id.as_deref() == Some(id.as_str()),
                                fresh: fresh_id.as_deref() == Some(id.as_str()),
                                read_only: present_step.is_some(),
                                present: present_step.is_some(),
                                pan_mode: panning,
                                tile_index: pos,
                                now,
                                widget_env,
                                mounted: mounted.contains(&id),
                                on_action,
                            }
                        }
                    }
                }
                if empty && present_step.is_none() {
                    EmptyBoardState {
                        on_add: move |_| open_palette_from_canvas(st, None),
                        on_preset: move |_| {
                            let mut chooser = st.chooser;
                            chooser.set(true);
                        },
                    }
                }
            }
            if present_step.is_some() {
                PresentMode { st }
            } else {
                div { class: "ih-hint", "{hint}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas_math::ZOOM_MAX;

    fn ssr(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn zoom_root() -> Element {
        let view = use_signal(View::default);
        rsx! {
            ZoomGroup { view, on_cmd: |_| {} }
        }
    }

    fn widget_at(i: usize) -> BoardItem {
        BoardItem {
            id: format!("00000000-0000-4000-8000-{i:012}"),
            x: (i as i32) * 500,
            y: 0,
            w: 480,
            h: 320,
            kind: ItemKind::Widget {
                view_id: format!("00000000-0000-4000-8000-{:012}", 100 + i),
            },
        }
    }

    #[test]
    fn mounted_widgets_follow_visible_set() {
        let items: Vec<BoardItem> = (0..8).map(widget_at).collect();
        let view = View::default();
        let vp = (1280.0, 720.0);
        let rects: Vec<(String, Rect)> = items.iter().map(|i| (i.id.clone(), rect_of(i))).collect();
        // Nothing selected: exactly visible_set at the frame cap.
        let expect = visible_set((0.0, 0.0, 1280.0, 720.0), 1.0, &rects, &[], FRAME_CAP);
        assert_eq!(mounted_widget_ids(&items, view, vp, None), expect);
        assert!(expect.len() <= FRAME_CAP && !expect.is_empty());
        // The selected, far-away widget is forced in and the count stays capped.
        let far = items[7].id.clone();
        let with = mounted_widget_ids(&items, view, vp, Some(&far));
        assert!(with.contains(&far) && with.len() <= FRAME_CAP, "{with:?}");
        // Below the level-of-detail zoom nothing is mounted, selected or not.
        let low = View { zoom: 0.25, ..view };
        assert!(mounted_widget_ids(&items, low, vp, Some(&far)).is_empty());
        // Notes never count.
        let mut mixed = items.clone();
        mixed[0].kind = ItemKind::Note {
            text: String::new(),
        };
        assert!(!mounted_widget_ids(&mixed, view, vp, None).contains(&mixed[0].id));
    }

    #[test]
    fn zoom_label_and_buttons() {
        let html = ssr(zoom_root);
        assert!(html.contains(">−<") && html.contains(">+<"), "{html}");
        assert!(html.contains(">100%<"), "{html}");
        assert!(html.contains(">Fit<"), "{html}");
        assert!(html.contains("aria-label=\"Zoom out\""), "{html}");
        assert!(html.contains("aria-label=\"Zoom in\""), "{html}");
        assert_eq!(zoom_pct(0.25), 25);
        assert_eq!(zoom_pct(1.5), 150);
        assert_eq!(zoom_pct(ZOOM_MAX), 200);
    }

    fn empty_canvas_root() -> Element {
        let st = use_board_state(Some(super::super::new_board("Ops")));
        rsx! {
            DashboardCanvas {
                st,
                board_name: "Ops".to_string(),
                is_active: true,
                narrow: false,
                mac: false,
            }
        }
    }

    fn notes_board() -> Board {
        let mut b = super::super::new_board("Ops");
        for (id, y, h) in [
            ("00000000-0000-4000-8000-00000000000a", 320, 96),
            ("00000000-0000-4000-8000-00000000000b", 0, 300),
            ("00000000-0000-4000-8000-00000000000c", 160, 900),
        ] {
            b.items.push(BoardItem {
                id: id.into(),
                x: 0,
                y,
                w: 224,
                h,
                kind: ItemKind::Note {
                    text: String::new(),
                },
            });
        }
        b
    }

    fn narrow_root() -> Element {
        let st = use_board_state(Some(notes_board()));
        rsx! {
            DashboardCanvas {
                st,
                board_name: "Ops".to_string(),
                is_active: true,
                narrow: true,
                mac: false,
            }
        }
    }

    fn wide_root() -> Element {
        let st = use_board_state(Some(notes_board()));
        rsx! {
            DashboardCanvas {
                st,
                board_name: "Ops".to_string(),
                is_active: true,
                narrow: false,
                mac: true,
            }
        }
    }

    #[test]
    fn narrow_board_renders_read_only_stack() {
        let html = ssr(narrow_root);
        assert!(html.contains("READ-ONLY"), "{html}");
        assert!(
            html.contains(
                "Editing needs a wider window. Widen it to at least 900px to move or add items."
            ),
            "{html}"
        );
        // No editing controls anywhere.
        for gone in [
            "ih-item__handle",
            "✕",
            "ih-zoom",
            "ih-hint",
            "ih-world",
            "Remove ",
        ] {
            assert!(
                !html.contains(gone),
                "{gone} must not render when narrow: {html}"
            );
        }
        // Reading order (floor(y/64), x, y): y=0, then 160, then 320; clamped heights 300/480/192.
        let at = |needle: &str| {
            html.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {html}"))
        };
        let (first, second, third) = (
            at("at x 0 y 0, 224 by 300"),
            at("at x 0 y 160, 224 by 900"),
            at("at x 0 y 320, 224 by 96"),
        );
        assert!(first < second && second < third, "{html}");
        let (h300, h480, h192) = (
            at("height: 300px"),
            at("height: 480px"),
            at("height: 192px"),
        );
        assert!(h300 < h480 && h480 < h192, "{html}");
        // The wide board keeps its controls (the toolbar's board select lives in mod.rs).
        let wide = ssr(wide_root);
        assert!(
            wide.contains("ih-item__handle") && wide.contains("✕"),
            "{wide}"
        );
        assert!(!wide.contains("READ-ONLY"), "{wide}");
    }

    fn toolbar_root(narrow: bool) -> Element {
        let view = use_signal(View::default);
        rsx! {
            super::super::BoardToolbar {
                views: None,
                options: vec![("b1".to_string(), "Ops".to_string())],
                selected_id: Some("b1".to_string()),
                narrow,
                has_board: true,
                board_full: false,
                view,
                status: super::super::SaveStatus::Saved,
                board_count: 1,
                on_pick: |_| {},
                on_new: |_| {},
                on_cmd: |_| {},
                on_add: |_| {},
            }
        }
    }

    #[test]
    fn narrow_toolbar_keeps_the_board_select_only() {
        let narrow = ssr(|| toolbar_root(true));
        assert!(
            narrow.contains("aria-label=\"Board\"") && narrow.contains("Ops"),
            "{narrow}"
        );
        assert!(
            !narrow.contains("Fit")
                && !narrow.contains("ih-zoom")
                && !narrow.contains("Saved.")
                && !narrow.contains("Add ▾")
        );
        let wide = ssr(|| toolbar_root(false));
        assert!(wide.contains("Fit") && wide.contains("Saved."), "{wide}");
        assert!(wide.contains("Add ▾"), "{wide}");
    }

    #[test]
    fn stacked_height_is_clamped() {
        assert_eq!(stacked_height(96), 192);
        assert_eq!(stacked_height(300), 300);
        assert_eq!(stacked_height(900), 480);
    }

    #[test]
    fn hint_strip_uses_platform_modifier() {
        assert_eq!(
            hint_text(false),
            "/ add · N note · Space + drag pan · Ctrl + scroll zoom · arrows move · Shift + arrows resize"
        );
        assert_eq!(
            hint_text(true),
            "/ add · N note · Space + drag pan · Cmd + scroll zoom · arrows move · Shift + arrows resize"
        );
        assert!(ssr(wide_root).contains("Cmd + scroll zoom"));
    }

    #[test]
    fn empty_board_state_renders() {
        let html = ssr(empty_canvas_root);
        assert!(html.contains("This board is empty"), "{html}");
        assert!(
            html.contains("Press / to add a widget, N for a note, or start from a preset."),
            "{html}"
        );
        assert!(html.contains("role=\"region\""), "{html}");
        assert!(
            html.contains("aria-label=\"Dashboard canvas: Ops\""),
            "{html}"
        );
        assert!(html.contains("tabindex=\"0\""), "{html}");
        // Dot grid at 100% zoom: 16px spacing.
        assert!(html.contains("background-size: 16.00px 16.00px"), "{html}");
    }

    #[test]
    fn view_commands_zoom_around_the_centre_and_fit() {
        let size = (800.0, 600.0);
        let v0 = View::default();
        let zin = run_view_cmd(CanvasCmd::ZoomIn, v0, size, &[]).unwrap();
        assert_eq!(zin.zoom, 1.25);
        // The viewport centre's world point stays under the centre.
        let w0 = screen_to_world((400.0, 300.0), v0);
        let w1 = screen_to_world((400.0, 300.0), zin);
        assert!((w0.0 - w1.0).abs() < 1e-9 && (w0.1 - w1.1).abs() < 1e-9);
        let zout = run_view_cmd(CanvasCmd::ZoomOut, zin, size, &[]).unwrap();
        assert_eq!(zout.zoom, 1.0);
        let reset = run_view_cmd(CanvasCmd::ZoomReset, zin, size, &[]).unwrap();
        assert_eq!(reset.zoom, 1.0);
        let wide = Rect {
            x: 0,
            y: 0,
            w: 3200,
            h: 240,
        };
        let f = run_view_cmd(CanvasCmd::Fit, v0, size, &[wide]).unwrap();
        // 3200 wide into (800 - 96): fits below 100%, and never above it.
        assert!(f.zoom < 0.3 && f.zoom >= 0.25, "{}", f.zoom);
        let small = Rect {
            x: 0,
            y: 0,
            w: 160,
            h: 96,
        };
        let capped = run_view_cmd(CanvasCmd::Fit, v0, size, &[small]).unwrap();
        assert_eq!(capped.zoom, 1.0);
        assert_eq!(run_view_cmd(CanvasCmd::Note, v0, size, &[]), None);
    }

    #[test]
    fn wheel_zooms_with_ctrl_and_pans_without() {
        let v = View::default();
        let cursor = (200.0, 100.0);
        let zoomed = wheel_view(v, (0.0, -100.0), true, cursor, false);
        assert!((zoomed.zoom - 1.1).abs() < 1e-9);
        let w0 = screen_to_world(cursor, v);
        let w1 = screen_to_world(cursor, zoomed);
        assert!((w0.0 - w1.0).abs() < 1e-9 && (w0.1 - w1.1).abs() < 1e-9);
        let panned = wheel_view(v, (10.0, 30.0), false, cursor, false);
        assert_eq!(
            (panned.pan_x, panned.pan_y, panned.zoom),
            (-10.0, -30.0, 1.0)
        );
        let lines = wheel_view(v, (0.0, 1.0), false, cursor, true);
        assert_eq!(lines.pan_y, -16.0);
        assert_eq!(wheel_view(v, (f64::NAN, 1.0), true, cursor, false), v);
        // Bounded: a huge delta never leaves 25%..200%.
        let big = wheel_view(v, (0.0, 1.0e9), true, cursor, false);
        assert!(big.zoom >= 0.25);
    }
}
