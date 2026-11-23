//! Phase 54.10 Plan 13 (D-14): Present mode, a zoom-to-fit slideshow of the board.
//!
//! The canvas wrap itself turns into a fixed full-window overlay (CSS class) so the mounted
//! frames are never re-created, and fullscreen is requested on top of it (RESEARCH Pitfall 13):
//! a refused request leaves the overlay covering the app chrome. Step 0 fits the whole board,
//! steps 1..n fit each item in reading order.

use super::canvas::{BoardState, CANVAS_DOM_ID, measure};
use super::items::{focus_dom, item_title_with, reading_order_ids, rect_of};
use crate::canvas_math::{View, fit};
use crate::components::hermes_app::header::focus_element_by_id;
use crate::server::dashboard_api::BoardItem;
use dioxus::core::use_drop;
use dioxus::prelude::*;

/// DOM id of the canvas wrap: the fullscreen target and the overlay.
pub(crate) const CANVAS_WRAP_ID: &str = "ih-canvas-wrap";
/// DOM id of the header `Present` button (focus returns here on exit).
pub(crate) const PRESENT_TRIGGER_ID: &str = "ih-present-trigger";
/// The strip hides after this long without pointer movement.
const IDLE_HIDE_MS: u32 = 3000;
/// Poll period of the idle loop.
const IDLE_TICK_MS: u32 = 250;

/// Step ids in order: `None` is the overview, then every item in reading order.
pub(crate) fn present_steps(items: &[BoardItem]) -> Vec<Option<String>> {
    std::iter::once(None)
        .chain(reading_order_ids(items).into_iter().map(Some))
        .collect()
}

/// The view for `step`: the whole board capped at 100%, or one item capped at 200%.
pub(crate) fn present_view(step: usize, items: &[BoardItem], viewport: (f64, f64)) -> View {
    let steps = present_steps(items);
    let rect_for = |id: &str| items.iter().find(|i| i.id == id).map(rect_of);
    match steps
        .get(step)
        .cloned()
        .flatten()
        .and_then(|id| rect_for(&id))
    {
        Some(r) => fit(&[r], viewport, 2.0),
        None => {
            let all: Vec<_> = items.iter().map(rect_of).collect();
            fit(&all, viewport, 1.0)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentAction {
    Next,
    Prev,
    First,
    Last,
    Exit,
}

pub(crate) fn present_key(key: &str) -> Option<PresentAction> {
    match key {
        "ArrowRight" | " " | "PageDown" => Some(PresentAction::Next),
        "ArrowLeft" | "PageUp" => Some(PresentAction::Prev),
        "Home" => Some(PresentAction::First),
        "End" => Some(PresentAction::Last),
        "Escape" => Some(PresentAction::Exit),
        _ => None,
    }
}

/// The step after `action` (clamped to `0..=last`), or `None` to leave Present.
pub(crate) fn present_nav(step: usize, action: PresentAction, last: usize) -> Option<usize> {
    match action {
        PresentAction::Next => Some((step + 1).min(last)),
        PresentAction::Prev => Some(step.saturating_sub(1)),
        PresentAction::First => Some(0),
        PresentAction::Last => Some(last),
        PresentAction::Exit => None,
    }
}

/// Widget ids whose frames must be mounted at `step`: the current item and the next one.
pub(crate) fn present_forced(
    step: usize,
    steps: &[Option<String>],
    widget_ids: &[String],
) -> Vec<String> {
    [step, step + 1]
        .into_iter()
        .filter_map(|s| steps.get(s).cloned().flatten())
        .filter(|id| widget_ids.contains(id))
        .collect()
}

/// The strip shows while the pointer moved within 3s, or while it holds focus.
pub(crate) fn controls_visible(ms_since_pointer_move: u32, strip_has_focus: bool) -> bool {
    strip_has_focus || ms_since_pointer_move < IDLE_HIDE_MS
}

/// Live-region text for a step; `n` is the item count.
pub(crate) fn present_announcement(step: usize, n: usize, title: Option<&str>) -> String {
    match title {
        Some(t) => format!("Showing {t}, {step} of {n}."),
        None => format!("Showing the whole board, {step} of {n}."),
    }
}

// ---- fullscreen (wasm; native twins do nothing) ----

/// Ask the browser to fullscreen the element. Refusal is fine: the overlay already covers
/// the app chrome. Must run inside a user gesture.
pub(crate) fn request_fullscreen(dom_id: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(dom_id))
    {
        let _ = el.request_fullscreen();
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = dom_id;
}

pub(crate) fn exit_fullscreen() {
    #[cfg(target_arch = "wasm32")]
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if doc.fullscreen_element().is_some() {
            let _ = doc.exit_fullscreen();
        }
    }
}

#[cfg(target_arch = "wasm32")]
type FsListener = (
    web_sys::Document,
    wasm_bindgen::closure::Closure<dyn FnMut()>,
);
#[cfg(not(target_arch = "wasm32"))]
type FsListener = ();

/// One `fullscreenchange` listener (`listener_slot` + `use_drop` idiom). Entering or resizing
/// fullscreen bumps `tick` (refit); leaving it after it was seen ends Present, so an unrelated
/// fullscreen change (a video) never does. The callback only writes signals.
fn install_fs_listener(mut slot: Signal<Option<FsListener>>, st: BoardState, tick: Signal<u32>) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        if slot.peek().is_some() {
            return;
        }
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let saw_full = std::rc::Rc::new(std::cell::Cell::new(false));
        let d2 = doc.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let mut tick = tick;
            if d2.fullscreen_element().is_some() {
                saw_full.set(true);
                tick += 1;
            } else if saw_full.get() {
                end_present(st);
            }
        });
        let _ =
            doc.add_event_listener_with_callback("fullscreenchange", cb.as_ref().unchecked_ref());
        slot.set(Some((doc, cb)));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (&mut slot, st, tick);
}

fn remove_fs_listener(mut slot: Signal<Option<FsListener>>) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some((doc, cb)) = slot.write().take() {
            let _ = doc.remove_event_listener_with_callback(
                "fullscreenchange",
                cb.as_ref().unchecked_ref(),
            );
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &mut slot;
}

// ---- enter / leave ----

/// Enter Present at the overview. Call from a user gesture (fullscreen needs one).
pub(crate) fn start_present(st: BoardState) {
    if st.present.peek().is_some() || st.items().is_empty() {
        return;
    }
    let (mut present, mut saved, mut active, mut animate) =
        (st.present, st.present_saved, st.active, st.animate);
    saved.set(*st.view.peek());
    active.set(None);
    animate.set(true);
    present.set(Some(0));
    request_fullscreen(CANVAS_WRAP_ID);
    st.say("Presenting. Right arrow for next, Escape to exit.");
    // The region takes keyboard focus once the overlay is rendered.
    spawn(async {
        crate::platform::timer::sleep(80).await;
        focus_dom(CANVAS_DOM_ID);
    });
}

/// Leave Present and restore the editing view. Signals only, so the fullscreen callback may
/// call it.
pub(crate) fn end_present(st: BoardState) {
    if st.present.peek().is_none() {
        return;
    }
    let (mut present, mut animate, mut view) = (st.present, st.animate, st.view);
    animate.set(false);
    view.set(*st.present_saved.peek());
    present.set(None);
}

/// Exit from a handler: end Present, leave fullscreen and return focus to the header button.
pub(crate) fn leave_present(st: BoardState) {
    end_present(st);
    exit_fullscreen();
    focus_element_by_id(PRESENT_TRIGGER_ID);
}

/// Apply a navigation action to the current step.
pub(crate) fn present_dispatch(st: BoardState, action: PresentAction) {
    let Some(step) = *st.present.peek() else {
        return;
    };
    let last = present_steps(&st.items()).len() - 1;
    match present_nav(step, action, last) {
        None => leave_present(st),
        Some(next) if next != step => {
            let (mut present, mut animate) = (st.present, st.animate);
            animate.set(true);
            present.set(Some(next));
        }
        Some(_) => {}
    }
}

/// Fit the current step to the measured viewport.
async fn refit(st: BoardState) {
    // The overlay or fullscreen needs a moment to reach its final size.
    crate::platform::timer::sleep(60).await;
    let Some((_, _, w, h)) = measure(st.vp_el).await else {
        return;
    };
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let Some(step) = *st.present.peek() else {
        return;
    };
    let v = present_view(step, &st.items(), (w, h));
    let (mut size, mut animate, mut view) = (st.vp_size, st.animate, st.view);
    size.set((w, h));
    animate.set(true);
    view.set(v);
}

/// The strip: `Previous`, `{i} of {n}`, `Next`, `Exit` and one dot per item.
#[component]
pub(crate) fn PresentStrip(
    step: usize,
    total: usize,
    visible: bool,
    on_prev: EventHandler<()>,
    on_next: EventHandler<()>,
    on_exit: EventHandler<()>,
    #[props(default)] on_key: EventHandler<String>,
    #[props(default)] on_focus: EventHandler<bool>,
) -> Element {
    rsx! {
        div {
            class: "ih-present__strip",
            class: if !visible { "ih-present__strip--hidden" },
            role: "group",
            "aria-label": "Presentation controls",
            onfocusin: move |_| on_focus.call(true),
            onfocusout: move |_| on_focus.call(false),
            onkeydown: move |e| {
                // Space and Enter belong to the focused button.
                let key = e.key().to_string();
                if key != " " && key != "Enter" {
                    on_key.call(key);
                }
            },
            button {
                class: "btn btn--sm btn--ghost",
                r#type: "button",
                onclick: move |_| on_prev.call(()),
                "Previous"
            }
            span { class: "ih-present__count", "{step} of {total}" }
            button {
                class: "btn btn--sm btn--ghost",
                r#type: "button",
                onclick: move |_| on_next.call(()),
                "Next"
            }
            button {
                class: "btn btn--sm btn--ghost",
                r#type: "button",
                onclick: move |_| on_exit.call(()),
                "Exit"
            }
            div { class: "ih-present__dots", "aria-hidden": "true",
                for i in 1..=total {
                    span {
                        key: "{i}",
                        class: "ih-present__dot",
                        class: if i == step { "ih-present__dot--current" },
                    }
                }
            }
        }
    }
}

/// Mounted only while Present is on: owns the refit effect, the fullscreen listener, the idle
/// hide loop, the strip and the live region.
#[component]
pub(crate) fn PresentMode(st: BoardState) -> Element {
    let tick = use_signal(|| 0u32);
    let fs_slot: Signal<Option<FsListener>> = use_signal(|| None);
    let mut visible = use_signal(|| true);
    let mut focused = use_signal(|| false);
    use_effect(move || install_fs_listener(fs_slot, st, tick));
    use_drop(move || remove_fs_listener(fs_slot));
    // A new step (or a fullscreen size change) re-fits the view.
    use_effect(move || {
        let _ = (*st.present.read(), *tick.read());
        spawn(refit(st));
    });
    // Idle loop: writes only `visible`; pointer moves and focus are read with `peek`.
    use_future(move || async move {
        let (mut seen, mut idle) = (*st.present_moves.peek(), 0u32);
        loop {
            crate::platform::timer::sleep(IDLE_TICK_MS).await;
            let moves = *st.present_moves.peek();
            if moves != seen {
                seen = moves;
                idle = 0;
            } else {
                idle = idle.saturating_add(IDLE_TICK_MS);
            }
            let show = controls_visible(idle, *focused.peek());
            if show != *visible.peek() {
                visible.set(show);
            }
        }
    });

    let step = (*st.present.read()).unwrap_or(0);
    let items = st
        .board
        .read()
        .as_ref()
        .map(|b| b.items.clone())
        .unwrap_or_default();
    let steps = present_steps(&items);
    let title = steps
        .get(step)
        .cloned()
        .flatten()
        .and_then(|id| items.iter().find(|i| i.id == id).cloned())
        .map(|i| item_title_with(&i.kind, &st.views.read()));
    let total = items.len();
    let say = present_announcement(step, total, title.as_deref());
    rsx! {
        PresentStrip {
            step,
            total,
            visible: *visible.read(),
            on_prev: move |_| present_dispatch(st, PresentAction::Prev),
            on_next: move |_| present_dispatch(st, PresentAction::Next),
            on_exit: move |_| leave_present(st),
            on_key: move |k: String| {
                if let Some(a) = present_key(&k) {
                    present_dispatch(st, a);
                }
            },
            on_focus: move |f| focused.set(f),
        }
        div { class: "ih-sr-only", role: "status", "aria-live": "polite", "{say}" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::dashboard_api::ItemKind;

    fn ssr(root: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn item(id: &str, x: i32, y: i32, w: i32, h: i32, widget: bool) -> BoardItem {
        BoardItem {
            id: id.into(),
            x,
            y,
            w,
            h,
            kind: if widget {
                ItemKind::Widget {
                    view_id: format!("v-{id}"),
                }
            } else {
                ItemKind::Note {
                    text: String::new(),
                }
            },
        }
    }

    /// Reading order is b (y 0), a (y 128), c (y 256) although stored a, b, c.
    fn three() -> Vec<BoardItem> {
        vec![
            item("a", 0, 128, 224, 96, true),
            item("b", 0, 0, 480, 320, true),
            item("c", 0, 256, 224, 96, false),
        ]
    }

    #[test]
    fn present_steps_follow_reading_order() {
        let steps = present_steps(&three());
        assert_eq!(
            steps,
            vec![None, Some("b".into()), Some("a".into()), Some("c".into())]
        );
    }

    #[test]
    fn present_view_fits_each_step() {
        let items = three();
        let vp = (1280.0, 720.0);
        let all: Vec<_> = items.iter().map(rect_of).collect();
        assert_eq!(present_view(0, &items, vp), fit(&all, vp, 1.0));
        // Step 3 is c (224x96): small, so it reaches zoom 2.0 and no more.
        let c = present_view(3, &items, vp);
        assert_eq!(c, fit(&[rect_of(&items[2])], vp, 2.0));
        assert_eq!(c.zoom, 2.0);
        // Step 1 is b, framed alone.
        assert_eq!(
            present_view(1, &items, vp),
            fit(&[rect_of(&items[1])], vp, 2.0)
        );
    }

    #[test]
    fn present_key_navigation() {
        for k in ["ArrowRight", " ", "PageDown"] {
            assert_eq!(present_key(k), Some(PresentAction::Next), "{k}");
        }
        for k in ["ArrowLeft", "PageUp"] {
            assert_eq!(present_key(k), Some(PresentAction::Prev), "{k}");
        }
        assert_eq!(present_key("Home"), Some(PresentAction::First));
        assert_eq!(present_key("End"), Some(PresentAction::Last));
        assert_eq!(present_key("Escape"), Some(PresentAction::Exit));
        assert_eq!(present_key("x"), None);
        // Clamped at both ends; Exit leaves.
        assert_eq!(present_nav(3, PresentAction::Next, 3), Some(3));
        assert_eq!(present_nav(0, PresentAction::Prev, 3), Some(0));
        assert_eq!(present_nav(2, PresentAction::First, 3), Some(0));
        assert_eq!(present_nav(1, PresentAction::Last, 3), Some(3));
        assert_eq!(present_nav(1, PresentAction::Exit, 3), None);
    }

    #[test]
    fn present_forced_mounts_current_and_next() {
        let items = three();
        let steps = present_steps(&items);
        let widgets = vec!["a".to_string(), "b".to_string()];
        // Step 1 = b, step 2 = a: both widgets.
        assert_eq!(present_forced(1, &steps, &widgets), vec!["b", "a"]);
        // Step 2 = a, step 3 = c (a note): the note is never forced.
        assert_eq!(present_forced(2, &steps, &widgets), vec!["a"]);
        assert!(present_forced(3, &steps, &widgets).is_empty());
        assert!(present_forced(0, &steps, &[]).is_empty());
    }

    #[test]
    fn controls_hide_after_idle_unless_focused() {
        assert!(controls_visible(0, false));
        assert!(controls_visible(2999, false));
        assert!(!controls_visible(3000, false));
        assert!(controls_visible(60_000, true));
    }

    #[test]
    fn present_announcement_copy() {
        assert_eq!(
            present_announcement(2, 4, Some("Usage")),
            "Showing Usage, 2 of 4."
        );
        assert_eq!(
            present_announcement(0, 4, None),
            "Showing the whole board, 0 of 4."
        );
    }

    #[test]
    fn present_strip_markup() {
        let html = ssr(|| {
            rsx! {
                PresentStrip {
                    step: 2,
                    total: 4,
                    visible: true,
                    on_prev: |_| {},
                    on_next: |_| {},
                    on_exit: |_| {},
                }
            }
        });
        for s in ["Previous", "2 of 4", "Next", "Exit"] {
            assert!(html.contains(s), "{s}: {html}");
        }
        assert_eq!(
            html.matches("<span class=\"ih-present__dot").count(),
            4,
            "{html}"
        );
        assert_eq!(
            html.matches("ih-present__dot--current").count(),
            1,
            "{html}"
        );
        assert!(!html.contains("ih-present__strip--hidden"), "{html}");
        let hidden = ssr(|| {
            rsx! {
                PresentStrip {
                    step: 0,
                    total: 1,
                    visible: false,
                    on_prev: |_| {},
                    on_next: |_| {},
                    on_exit: |_| {},
                }
            }
        });
        assert!(hidden.contains("ih-present__strip--hidden"), "{hidden}");
    }
}
