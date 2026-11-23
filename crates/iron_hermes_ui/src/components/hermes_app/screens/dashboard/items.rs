//! Phase 54.10 Plan 06 (D-13/D-14/D-16): the shared board-item chrome.
//!
//! Every item kind (tile, AI widget, note) renders through [`BoardItemView`]: header strip
//! (drag handle, title, `✕`), body, resize handle and the inline remove confirm. Geometry comes
//! from the pure `canvas_math`; this file only turns pointer and key events into
//! [`ItemAction`]s that the canvas applies to the board working copy. Tile bodies come from `tiles.rs`;
//! Widget bodies arrive with plan 54.10-09 (until then chrome and title only).

use super::tiles::TileBody;
use super::widgets::{WidgetHost, widget_title, widget_view_state};
use crate::canvas_math::{self as cm, DragMode, DragState, Rect, lod_full, reading_order};
use crate::platform::pointer::{capture_pointer, release_pointer};
use crate::server::dashboard_api::{
    BoardItem, COORD_MAX, ITEM_SIDE_MAX, ItemKind, NOTE_TEXT_MAX_CHARS, TileSpec,
    item_default_size, item_min_size,
};
use crate::server::openui_views_api::OpenUiView;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use std::rc::Rc;

/// Show the `{n} left` counter only this close to the note limit (UI-SPEC A7).
const NOTE_COUNTER_WITHIN: usize = 100;

pub(crate) fn rect_of(i: &BoardItem) -> Rect {
    Rect {
        x: i.x,
        y: i.y,
        w: i.w,
        h: i.h,
    }
}

/// The saved-view list a widget resolves against; `None` until the first fetch lands.
pub(crate) type SavedViews = Option<Result<Vec<OpenUiView>, String>>;

/// What an AI widget item needs from the screen: the saved views and the active-frame signal.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct WidgetEnv {
    pub views: Signal<SavedViews>,
    pub active: Signal<Option<String>>,
}

/// [`item_title`], with an AI widget showing its saved view's name (or `View deleted`).
pub(crate) fn item_title_with(kind: &ItemKind, views: &SavedViews) -> String {
    match (kind, views) {
        (ItemKind::Widget { view_id }, Some(Ok(list))) => {
            widget_title(&widget_view_state(view_id, list))
        }
        _ => item_title(kind),
    }
}

/// Header title (UI-SPEC "Tile titles"). Generic for an AI widget: its saved view's name needs
/// the view list, see [`item_title_with`].
pub(crate) fn item_title(kind: &ItemKind) -> String {
    match kind {
        ItemKind::Tile { tile } => tile.title(),
        ItemKind::Widget { .. } => "AI widget".to_string(),
        ItemKind::Note { .. } => "Note".to_string(),
    }
}

fn kind_word(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::Tile { .. } => "tile",
        ItemKind::Widget { .. } => "widget",
        ItemKind::Note { .. } => "note",
    }
}

fn kind_glyph(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::Tile { .. } => "▦",
        ItemKind::Widget { .. } => "✦",
        ItemKind::Note { .. } => "NOTE",
    }
}

/// UI-SPEC aria label: `{title}, {kind}, at x {x} y {y}, {w} by {h}`.
fn item_aria_label_titled(item: &BoardItem, title: &str) -> String {
    format!(
        "{}, {}, at x {} y {}, {} by {}",
        title,
        kind_word(&item.kind),
        item.x,
        item.y,
        item.w,
        item.h
    )
}

/// UI-SPEC "Destructive: remove item", per kind.
pub(crate) fn remove_confirm_copy(kind: &ItemKind, title: &str) -> String {
    let tail = match kind {
        ItemKind::Widget { .. } => "The saved view stays in OpenUI.",
        _ => "Its settings and text are lost.",
    };
    format!("Remove from board: Take \"{title}\" off this board? {tail}")
}

/// DOM id of an item's group element (focus target after a remove).
pub(crate) fn item_dom_id(id: &str) -> String {
    format!("ih-dash-item-{id}")
}

fn note_dom_id(id: &str) -> String {
    format!("ih-dash-note-{id}")
}

/// Focus an element by DOM id. Ids are UUID-derived or fixed, but the charset is checked
/// anyway because the id is interpolated into script text.
pub(crate) fn focus_dom(dom_id: &str) {
    if dom_id.is_empty()
        || !dom_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return;
    }
    let _ = document::eval(&format!("document.getElementById('{dom_id}')?.focus();"));
}

/// `{n} left` when within [`NOTE_COUNTER_WITHIN`] characters of the limit.
pub(crate) fn note_chars_left(text: &str) -> Option<usize> {
    let left = NOTE_TEXT_MAX_CHARS.saturating_sub(text.chars().count());
    (left <= NOTE_COUNTER_WITHIN).then_some(left)
}

/// A new item of `kind` centred on `center` (world units), on the grid, nudged off any item
/// already on that spot (D-14).
pub(crate) fn new_item_at(items: &[BoardItem], center: (i32, i32), kind: ItemKind) -> BoardItem {
    let existing: Vec<Rect> = items.iter().map(rect_of).collect();
    let r = cm::place_at_center_stack(&existing, center, item_default_size(&kind));
    BoardItem {
        id: uuid::Uuid::new_v4().to_string(),
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        kind,
    }
}

/// A new empty note; see [`new_item_at`].
#[cfg(test)]
pub(crate) fn new_note_at(items: &[BoardItem], center: (i32, i32)) -> BoardItem {
    new_item_at(
        items,
        center,
        ItemKind::Note {
            text: String::new(),
        },
    )
}

/// Move the item to the end of the list: z-order is array order (UI-SPEC A3). `true` when it
/// actually moved.
pub(crate) fn raise(items: &mut Vec<BoardItem>, id: &str) -> bool {
    match items.iter().position(|i| i.id == id) {
        Some(p) if p + 1 < items.len() => {
            let it = items.remove(p);
            items.push(it);
            true
        }
        _ => false,
    }
}

/// Commit a drag / resize / key rect to one item and raise it. The client clamps to the
/// server's bounds, but the server re-validates every save (T-54.10-19).
pub(crate) fn apply_item_rect(items: &mut Vec<BoardItem>, id: &str, r: Rect) {
    if let Some(it) = items.iter_mut().find(|i| i.id == id) {
        let (min_w, min_h) = item_min_size(&it.kind);
        it.x = r.x.clamp(-COORD_MAX, COORD_MAX);
        it.y = r.y.clamp(-COORD_MAX, COORD_MAX);
        it.w = r.w.clamp(min_w, ITEM_SIDE_MAX);
        it.h = r.h.clamp(min_h, ITEM_SIDE_MAX);
    }
    raise(items, id);
}

/// Item ids in reading order (DOM and Tab order, UI-SPEC A10).
pub(crate) fn reading_order_ids(items: &[BoardItem]) -> Vec<String> {
    let pairs: Vec<(String, Rect)> = items.iter().map(|i| (i.id.clone(), rect_of(i))).collect();
    reading_order(&pairs)
}

/// Where focus goes after `removed` is gone: the next item in reading order, else the previous
/// one, else `None` (the canvas region).
pub(crate) fn focus_target_after_remove(order: &[String], removed: &str) -> Option<String> {
    let p = order.iter().position(|i| i == removed)?;
    order
        .get(p + 1)
        .or_else(|| p.checked_sub(1).and_then(|q| order.get(q)))
        .cloned()
}

/// What a key does on a focused item (UI-SPEC A10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemKeyAction {
    /// Move by whole grid steps.
    Nudge(i32, i32),
    /// Resize by whole grid steps.
    Resize(i32, i32),
    ConfirmRemove,
    Select,
    Escape,
    /// Enter: focus a note's textarea.
    Activate,
}

pub(crate) fn item_key_action(
    key: &str,
    shift: bool,
    ctrl_or_meta: bool,
    alt: bool,
) -> Option<ItemKeyAction> {
    if ctrl_or_meta || alt {
        return None;
    }
    let step = match key {
        "ArrowLeft" => Some((-1, 0)),
        "ArrowRight" => Some((1, 0)),
        "ArrowUp" => Some((0, -1)),
        "ArrowDown" => Some((0, 1)),
        _ => None,
    };
    if let Some((dx, dy)) = step {
        return Some(if shift {
            ItemKeyAction::Resize(dx, dy)
        } else {
            ItemKeyAction::Nudge(dx, dy)
        });
    }
    match key {
        "Delete" | "Backspace" => Some(ItemKeyAction::ConfirmRemove),
        " " => Some(ItemKeyAction::Select),
        "Escape" => Some(ItemKeyAction::Escape),
        "Enter" => Some(ItemKeyAction::Activate),
        _ => None,
    }
}

/// Everything an item reports to the canvas; the canvas knows which item (the handler gets
/// `(item id, action)`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ItemAction {
    Select,
    Moved(Rect),
    Resized(Rect),
    Text(String),
    /// A tile's `⚙` changed its typed settings (D-08).
    TileSettings(TileSpec),
    RequestRemove,
    CancelRemove,
    ConfirmRemove,
    Escape,
    /// A note textarea lost focus.
    NoteBlur,
}

type El = Signal<Option<Rc<MountedData>>>;

fn drag_begin(
    mut drag: Signal<DragState>,
    el: El,
    mode: DragMode,
    e: &PointerEvent,
    rect: Rect,
    min: (i32, i32),
) {
    if e.trigger_button() != Some(MouseButton::Primary) {
        return;
    }
    e.stop_propagation();
    let p = e.client_coordinates();
    if let Some(el) = el.peek().as_ref() {
        capture_pointer(el, e.pointer_id());
    }
    drag.set(DragState::begin(mode, (p.x, p.y), rect, min));
}

fn drag_move(
    mut drag: Signal<DragState>,
    mut live: Signal<Option<Rect>>,
    e: &PointerEvent,
    zoom: f64,
) {
    let p = e.client_coordinates();
    let alt = e.modifiers().alt();
    if let Some(r) = drag.write().update((p.x, p.y), zoom, alt) {
        live.set(Some(r));
    }
}

/// One shared pointer-up/cancel/lost-capture path. Returns the mode and final rect.
fn drag_finish(
    mut drag: Signal<DragState>,
    mut live: Signal<Option<Rect>>,
    els: [El; 2],
    pointer_id: i32,
) -> Option<(DragMode, Rect)> {
    let mode = match *drag.peek() {
        DragState::Active { mode, .. } => mode,
        DragState::Idle => return None,
    };
    let r = drag.write().end();
    live.set(None);
    for el in els {
        if let Some(el) = el.peek().as_ref() {
            release_pointer(el, pointer_id);
        }
    }
    r.map(|r| (mode, r))
}

/// One board item. `read_only` is the narrow stacked mode (D-17): flows in its column, no
/// editing controls and no handlers.
#[component]
pub(crate) fn BoardItemView(
    item: BoardItem,
    z: usize,
    zoom: f64,
    selected: bool,
    confirming: bool,
    fresh: bool,
    read_only: bool,
    pan_mode: bool,
    // Present mode (D-14): read-only, but Enter still activates a widget's frame.
    #[props(default)] present: bool,
    // Reading-order position (first-fetch stagger) and the screen's 15s clock, for tiles.
    #[props(default)] tile_index: usize,
    #[props(default)] now: i64,
    // AI widgets: the saved views, the active-frame signal, and whether this widget's frame is
    // within the frame cap (UI-SPEC A5).
    #[props(default)] widget_env: Option<WidgetEnv>,
    #[props(default = true)] mounted: bool,
    on_action: EventHandler<(String, ItemAction)>,
) -> Element {
    let id = use_signal(|| item.id.clone());
    let emit = move |a: ItemAction| on_action.call((id.peek().clone(), a));
    let drag = use_signal(|| DragState::Idle);
    let live = use_signal(|| None::<Rect>);
    let mut grip_el: El = use_signal(|| None);
    let mut handle_el: El = use_signal(|| None);

    let rect = rect_of(&item);
    let min = item_min_size(&item.kind);
    let shown = (*live.read()).unwrap_or(rect);
    let dragging = live.read().is_some();
    let full = lod_full(zoom) && !read_only;
    let title = match widget_env {
        Some(env) => item_title_with(&item.kind, &env.views.read()),
        None => item_title(&item.kind),
    };
    let aria = item_aria_label_titled(&item, &title);
    let is_widget = matches!(item.kind, ItemKind::Widget { .. });
    let is_note = matches!(item.kind, ItemKind::Note { .. });
    let item_id = item.id.clone();

    let style = if read_only {
        format!("height: {}px;", super::canvas::stacked_height(item.h))
    } else {
        format!(
            "left: {}px; top: {}px; width: {}px; height: {}px; z-index: {z};",
            shown.x, shown.y, shown.w, shown.h
        )
    };

    // A press selects this item and hands pointer input back from another item's active frame.
    // The grip and handle stop propagation, so they run it themselves.
    let press = move || {
        if let Some(env) = widget_env {
            let mut active = env.active;
            if active
                .peek()
                .as_deref()
                .is_some_and(|a| a != id.peek().as_str())
            {
                active.set(None);
            }
        }
        emit(ItemAction::Select);
    };

    let finish = move |pointer_id: i32| {
        if let Some((mode, r)) = drag_finish(drag, live, [grip_el, handle_el], pointer_id) {
            if r != rect {
                emit(match mode {
                    DragMode::Move => ItemAction::Moved(r),
                    DragMode::Resize => ItemAction::Resized(r),
                });
            }
        }
    };

    rsx! {
        div {
            id: "{item_dom_id(&item_id)}",
            class: "ih-item",
            class: if selected { "ih-item--selected" },
            class: if dragging { "ih-item--dragging" },
            class: if !full { "ih-item--lod" },
            class: if confirming { "ih-item--confirm" },
            class: if read_only { "ih-item--stacked" },
            role: "group",
            tabindex: "0",
            "aria-label": "{aria}",
            style: "{style}",
            onpointerdown: move |e| {
                // Space+drag pans, so the press must reach the canvas.
                if read_only || pan_mode {
                    return;
                }
                e.stop_propagation();
                press();
            },
            onkeydown: move |e| {
                if read_only && !present {
                    return;
                }
                let m = e.modifiers();
                let key = e.key().to_string();
                let Some(act) = item_key_action(&key, m.shift(), m.ctrl() || m.meta(), m.alt())
                else {
                    return;
                };
                if present && act != ItemKeyAction::Activate {
                    return;
                }
                if confirming && act != ItemKeyAction::Escape {
                    return;
                }
                match act {
                    ItemKeyAction::Nudge(dx, dy) => {
                        e.prevent_default();
                        e.stop_propagation();
                        emit(ItemAction::Moved(cm::nudge(rect, dx, dy)));
                    }
                    ItemKeyAction::Resize(dw, dh) => {
                        e.prevent_default();
                        e.stop_propagation();
                        emit(ItemAction::Resized(cm::resize_step(rect, dw, dh, min)));
                    }
                    ItemKeyAction::ConfirmRemove => {
                        e.prevent_default();
                        e.stop_propagation();
                        emit(ItemAction::RequestRemove);
                    }
                    // Not stopped: the canvas tracks Space for Space+drag panning.
                    ItemKeyAction::Select => {
                        e.prevent_default();
                        emit(ItemAction::Select);
                    }
                    ItemKeyAction::Escape => {
                        e.stop_propagation();
                        emit(ItemAction::Escape);
                    }
                    ItemKeyAction::Activate => {
                        if is_note {
                            e.prevent_default();
                            focus_dom(&note_dom_id(&id.peek()));
                        } else if let (true, Some(env)) = (is_widget, widget_env) {
                            // Enter activates the frame (UI-SPEC A5).
                            e.prevent_default();
                            let mut active = env.active;
                            active.set(Some(id.peek().clone()));
                            let frame = super::widgets::widget_frame_id(&id.peek());
                            spawn(async move {
                                crate::platform::timer::sleep(50).await;
                                focus_dom(&frame);
                            });
                        }
                    }
                }
            },

            div { class: "ih-item__head",
                div {
                    class: "ih-item__grip",
                    onmounted: move |m| grip_el.set(Some(m.data())),
                    onpointerdown: move |e| {
                        if read_only || pan_mode || confirming {
                            return;
                        }
                        if e.trigger_button() == Some(MouseButton::Primary) {
                            press();
                        }
                        drag_begin(drag, grip_el, DragMode::Move, &e, rect, min);
                    },
                    onpointermove: move |e| drag_move(drag, live, &e, zoom),
                    onpointerup: move |e| finish(e.pointer_id()),
                    onpointercancel: move |e| finish(e.pointer_id()),
                    onlostpointercapture: move |e| finish(e.pointer_id()),
                    span { class: "ih-item__glyph", "{kind_glyph(&item.kind)}" }
                    if !is_note {
                        span { class: "ih-item__title", title: "{title}", "{title}" }
                    }
                }
                if full {
                    div { class: "ih-item__ctrl",
                        button {
                            class: "btn btn--icon btn--ghost",
                            "aria-label": "Remove {title} from board",
                            title: "Remove {title} from board",
                            onkeydown: move |e| e.stop_propagation(),
                            onclick: move |_| emit(ItemAction::RequestRemove),
                            "✕"
                        }
                    }
                }
            }

            div { class: "ih-item__body",
                if confirming {
                    div {
                        class: "ih-item__confirm",
                        role: "alertdialog",
                        "aria-label": "Remove {title}",
                        onkeydown: move |e| {
                            e.stop_propagation();
                            if e.key().to_string() == "Escape" {
                                emit(ItemAction::CancelRemove);
                            }
                        },
                        p { "{remove_confirm_copy(&item.kind, &title)}" }
                        div { class: "ih-item__confirm-actions",
                            button {
                                class: "btn btn--sm btn--danger",
                                onclick: move |_| emit(ItemAction::ConfirmRemove),
                                "Remove"
                            }
                            button {
                                class: "btn btn--sm btn--ghost",
                                onmounted: move |m| {
                                    spawn(async move {
                                        let _ = m.data().set_focus(true).await;
                                    });
                                },
                                onclick: move |_| emit(ItemAction::CancelRemove),
                                "Keep"
                            }
                        }
                    }
                } else if let ItemKind::Note { text } = &item.kind {
                    NoteBody {
                        dom_id: note_dom_id(&item_id),
                        text: text.clone(),
                        fresh,
                        read_only,
                        on_action: emit,
                    }
                } else if let ItemKind::Tile { tile } = &item.kind {
                    TileBody {
                        item_id: item_id.clone(),
                        spec: tile.clone(),
                        index: tile_index,
                        now,
                        selected,
                        read_only,
                        on_settings: move |s: TileSpec| emit(ItemAction::TileSettings(s)),
                    }
                } else if let (ItemKind::Widget { view_id }, Some(env)) = (&item.kind, widget_env) {
                    WidgetHost {
                        item_id: item_id.clone(),
                        view_id: view_id.clone(),
                        views: env.views,
                        active: env.active,
                        mounted,
                        read_only,
                        on_remove: move |_| emit(ItemAction::ConfirmRemove),
                    }
                }
            }

            if full && !confirming {
                div {
                    class: "ih-item__handle",
                    "aria-label": "Resize {title}",
                    onmounted: move |m| handle_el.set(Some(m.data())),
                    onpointerdown: move |e| {
                        if pan_mode {
                            return;
                        }
                        if e.trigger_button() == Some(MouseButton::Primary) {
                            press();
                        }
                        drag_begin(drag, handle_el, DragMode::Resize, &e, rect, min);
                    },
                    onpointermove: move |e| drag_move(drag, live, &e, zoom),
                    onpointerup: move |e| finish(e.pointer_id()),
                    onpointercancel: move |e| finish(e.pointer_id()),
                    onlostpointercapture: move |e| finish(e.pointer_id()),
                }
            }
        }
    }
}

/// A note's borderless textarea (UI-SPEC A7): plain text, never rendered as HTML or markdown
/// (T-54.10-20).
#[component]
fn NoteBody(
    dom_id: String,
    text: String,
    fresh: bool,
    read_only: bool,
    on_action: EventHandler<ItemAction>,
) -> Element {
    let left = note_chars_left(&text);
    rsx! {
        textarea {
            id: "{dom_id}",
            class: "ih-note__text",
            placeholder: "Write a note…",
            maxlength: "{NOTE_TEXT_MAX_CHARS}",
            readonly: read_only,
            "aria-label": "Note text",
            value: "{text}",
            onmounted: move |m| {
                if fresh {
                    spawn(async move {
                        let _ = m.data().set_focus(true).await;
                    });
                }
            },
            oninput: move |e| on_action.call(ItemAction::Text(e.value())),
            onblur: move |_| on_action.call(ItemAction::NoteBlur),
            // Canvas shortcuts are suspended while typing (UI-SPEC A7); Esc leaves the field.
            onkeydown: move |e| {
                e.stop_propagation();
                if e.key().to_string() == "Escape" {
                    e.prevent_default();
                    on_action.call(ItemAction::Escape);
                }
            },
        }
        if let Some(n) = left {
            div { class: "ih-note__count", "{n} left" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::dashboard_api::{BoardItem, ItemKind, TileSpec};

    fn note(id: &str, x: i32, y: i32) -> BoardItem {
        BoardItem {
            id: id.into(),
            x,
            y,
            w: 224,
            h: 160,
            kind: ItemKind::Note {
                text: String::new(),
            },
        }
    }

    fn ssr(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn new_note_item_is_placed_on_grid_and_focused_target() {
        let n = new_note_at(&[], (500, 300));
        assert!(uuid::Uuid::parse_str(&n.id).is_ok());
        assert_eq!((n.w, n.h), (224, 160));
        assert_eq!((n.x % 16, n.y % 16), (0, 0));
        assert!(matches!(&n.kind, ItemKind::Note { text } if text.is_empty()));
        // Centred on the spot, not corner-anchored.
        assert_eq!((n.x, n.y), (cm::snap(500 - 112), cm::snap(300 - 80)));
        // A second note on the same spot is nudged off it.
        let again = new_note_at(std::slice::from_ref(&n), (500, 300));
        assert_eq!((again.x, again.y), (n.x + 16, n.y + 16));
    }

    #[test]
    fn apply_item_rect_updates_only_that_item() {
        let mut items = vec![note("a", 0, 0), note("b", 32, 32), note("c", 64, 64)];
        let before_c = items[2].clone();
        apply_item_rect(
            &mut items,
            "a",
            Rect {
                x: 160,
                y: 48,
                w: 256,
                h: 192,
            },
        );
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            ["b", "c", "a"],
            "dragged item goes to the end (on top)"
        );
        let a = &items[2];
        assert_eq!((a.x, a.y, a.w, a.h), (160, 48, 256, 192));
        assert_eq!(items[1], before_c);
        assert_eq!((items[0].x, items[0].y), (32, 32));
        // Hostile rects are clamped to the server's bounds.
        apply_item_rect(
            &mut items,
            "b",
            Rect {
                x: i32::MAX,
                y: i32::MIN,
                w: 1,
                h: i32::MAX,
            },
        );
        let b = items.iter().find(|i| i.id == "b").unwrap();
        assert_eq!(
            (b.x, b.y, b.w, b.h),
            (COORD_MAX, -COORD_MAX, 160, ITEM_SIDE_MAX)
        );
    }

    fn chrome_root() -> Element {
        let mut item = note("a1", 32, 48);
        item.kind = ItemKind::Note {
            text: "hello".into(),
        };
        rsx! {
            BoardItemView {
                item,
                z: 0,
                zoom: 1.0,
                selected: false,
                confirming: false,
                fresh: false,
                read_only: false,
                pan_mode: false,
                on_action: |_| {},
            }
        }
    }

    #[test]
    fn board_item_view_renders_note_chrome() {
        let html = ssr(chrome_root);
        assert!(html.contains("role=\"group\""), "{html}");
        assert!(html.contains("tabindex=\"0\""), "{html}");
        assert!(
            html.contains("aria-label=\"Note, note, at x 32 y 48, 224 by 160\""),
            "{html}"
        );
        assert!(html.contains(">NOTE<"), "{html}");
        assert!(
            html.contains("aria-label=\"Remove Note from board\""),
            "{html}"
        );
        assert!(html.contains("✕"), "{html}");
        assert!(html.contains("<textarea"), "{html}");
        assert!(html.contains("placeholder=\"Write a note…\""), "{html}");
        assert!(html.contains("maxlength=\"2000\""), "{html}");
        assert!(
            html.contains("left: 32px; top: 48px; width: 224px; height: 160px;"),
            "{html}"
        );
        assert!(
            !html.contains("left</div>"),
            "counter only near the limit: {html}"
        );
    }

    #[test]
    fn note_counter_appears_only_near_the_limit() {
        assert_eq!(note_chars_left("short"), None);
        assert_eq!(note_chars_left(&"x".repeat(1899)), None);
        assert_eq!(note_chars_left(&"x".repeat(1900)), Some(100));
        assert_eq!(note_chars_left(&"x".repeat(2000)), Some(0));
    }

    #[test]
    fn titles_and_labels() {
        let tile = ItemKind::Tile {
            tile: TileSpec::Cron {},
        };
        assert_eq!(item_title(&tile), "Cron");
        assert_eq!(
            item_title(&ItemKind::Widget {
                view_id: String::new()
            }),
            "AI widget"
        );
        assert_eq!(item_dom_id("abc"), "ih-dash-item-abc");
    }

    #[test]
    fn item_key_action_maps_keys() {
        use ItemKeyAction::*;
        assert_eq!(
            item_key_action("ArrowLeft", false, false, false),
            Some(Nudge(-1, 0))
        );
        assert_eq!(
            item_key_action("ArrowRight", false, false, false),
            Some(Nudge(1, 0))
        );
        assert_eq!(
            item_key_action("ArrowUp", false, false, false),
            Some(Nudge(0, -1))
        );
        assert_eq!(
            item_key_action("ArrowDown", false, false, false),
            Some(Nudge(0, 1))
        );
        assert_eq!(
            item_key_action("ArrowRight", true, false, false),
            Some(Resize(1, 0))
        );
        assert_eq!(
            item_key_action("ArrowUp", true, false, false),
            Some(Resize(0, -1))
        );
        assert_eq!(
            item_key_action("Delete", false, false, false),
            Some(ConfirmRemove)
        );
        assert_eq!(
            item_key_action("Backspace", false, false, false),
            Some(ConfirmRemove)
        );
        assert_eq!(item_key_action(" ", false, false, false), Some(Select));
        assert_eq!(item_key_action("Escape", false, false, false), Some(Escape));
        assert_eq!(
            item_key_action("Enter", false, false, false),
            Some(Activate)
        );
        assert_eq!(item_key_action("x", false, false, false), None);
        for k in ["ArrowLeft", "Delete", " ", "Escape"] {
            assert_eq!(
                item_key_action(k, false, true, false),
                None,
                "ctrl/meta {k}"
            );
            assert_eq!(item_key_action(k, false, false, true), None, "alt {k}");
            assert_eq!(item_key_action(k, true, true, true), None, "all {k}");
        }
    }

    fn confirm_note_root() -> Element {
        rsx! {
            BoardItemView {
                item: note("n1", 0, 0),
                z: 0,
                zoom: 1.0,
                selected: true,
                confirming: true,
                fresh: false,
                read_only: false,
                pan_mode: false,
                on_action: |_| {},
            }
        }
    }

    fn confirm_widget_root() -> Element {
        let mut item = note("w1", 0, 0);
        item.w = 480;
        item.h = 320;
        item.kind = ItemKind::Widget {
            view_id: "00000000-0000-4000-8000-000000000000".into(),
        };
        rsx! {
            BoardItemView {
                item,
                z: 0,
                zoom: 1.0,
                selected: false,
                confirming: true,
                fresh: false,
                read_only: false,
                pan_mode: false,
                on_action: |_| {},
            }
        }
    }

    #[test]
    fn remove_confirm_copy_per_kind() {
        let tile = ItemKind::Tile {
            tile: TileSpec::Cron {},
        };
        assert_eq!(
            remove_confirm_copy(&tile, "Cron"),
            "Remove from board: Take \"Cron\" off this board? Its settings and text are lost."
        );
        let widget = ItemKind::Widget {
            view_id: String::new(),
        };
        assert_eq!(
            remove_confirm_copy(&widget, "AI widget"),
            "Remove from board: Take \"AI widget\" off this board? The saved view stays in OpenUI."
        );
        // Rendered: copy plus a danger Remove and a Keep, and no textarea while confirming.
        let html = ssr(confirm_note_root);
        assert!(html.contains("Remove from board: Take"), "{html}");
        assert!(
            html.contains("off this board? Its settings and text are lost."),
            "{html}"
        );
        assert!(html.contains("btn--danger"), "{html}");
        assert!(
            html.contains(">Remove<") && html.contains(">Keep<"),
            "{html}"
        );
        assert!(!html.contains("<textarea"), "{html}");
        let html = ssr(confirm_widget_root);
        assert!(html.contains("The saved view stays in OpenUI."), "{html}");
    }

    #[test]
    fn remove_moves_focus_target_to_next_in_reading_order() {
        let order: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(focus_target_after_remove(&order, "b"), Some("c".into()));
        assert_eq!(focus_target_after_remove(&order, "a"), Some("b".into()));
        // The last of several falls back to the previous one.
        assert_eq!(focus_target_after_remove(&order, "c"), Some("b".into()));
        // Nothing left: the canvas region.
        assert_eq!(
            focus_target_after_remove(&["only".to_string()], "only"),
            None
        );
        assert_eq!(focus_target_after_remove(&order, "zzz"), None);
        // Reading order is (floor(y/64), x, y), not array order.
        let items = vec![
            note("late", 0, 200),
            note("right", 300, 0),
            note("left", 0, 0),
        ];
        assert_eq!(reading_order_ids(&items), ["left", "right", "late"]);
    }
}
