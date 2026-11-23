//! Phase 26.2.1 Plan 04 — Wheel SVG primitive.
//!
//! Ports `crates/iron_hermes_ui/filestoimport/ironhermes-design-system/project/wheel-v2.js`
//! into a self-contained Dioxus 0.7 component. Renders a draggable, resizable
//! 10-wedge SVG wheel with hover/click navigation. Geometry constants match
//! `wheel-v2.js` lines 26-37 byte-for-byte. (Phase 54.4 removed the 11th
//! `Kanban` wedge; the header KANBAN shortcut button covers it.)
//!
//! Canonical 10-wedge order from `wheel-v2.js` DEFAULT_SECTIONS
//! (CONTEXT D-10):
//!   chat, agents, models, tools, skills, memory, sessions, providers,
//!   gateway, settings
//!
//! Phase 54.4 Plan 01 (D-05, D-26): the wheel is now DOCKED in the persistent
//! header (`header::Header`, grid column 1) via `Wheel { docked: true }`. A
//! docked wheel is fixed at `--ih-wheel-size` and stays the full 10-wedge
//! launcher, but it has no rim drag, no resize ring, no `DRAG TO MOVE`
//! tooltip and no inline `left`/`top`; a stored `ih.ui.wheel` `WheelState`
//! position/size is ignored (never read for layout, never rewritten). It also
//! has NO hover/focus magnify, scale or transform (D-26): the wheel box is
//! taller than the banner and simply overhangs it by layout. The floating
//! (drag/resize) path below is kept behind `docked == false` and `WheelState`
//! is retained, so a future "undock" option is a one-prop change.
//!
//! Phase 57 R9b: while any approval card waits on the operator (or has a decision in
//! flight), the CHAT wedge — the only Chat nav item — carries one small amber badge
//! (`wheel-approval-badge`, an SVG circle with `aria-label` "Approval pending" and no text)
//! so an operator on another screen is not denied silently by the approval timeout. It is
//! read from `ApprovalCardsCtx`, clears when every card has resolved or the socket drops,
//! and plays no sound and writes no notice.
//!
//! Colours: the wheel's colours live in `wheel.css` on theme variables plus
//! `--wheel-accent` / `--wheel-accent-bright` (Phase 54.4 Plan 12, G-54.4-3);
//! the render code below carries class names only, never a colour attribute.
//!
//! Interactions (CONTEXT D-11 / wheel-v2.js parity):
//! - Hover wedge → set `WheelState.active_wedge`
//! - Click wedge → set wedge AND `active_screen = wedge.to_screen()`
//! - Click hub → launch currently-active wedge
//! - Drag rim → translate `WheelState.position` (window-level pointermove;
//!   floating mode only)
//! - Drag resize ring → rescale `WheelState.size` clamped [MIN_SIZE, MAX_SIZE]
//!   (floating mode only)
//! - Every wedge carries an SVG `title`, an `aria-label`, `role="link"` and
//!   `tabindex="0"`; Enter / Space navigate exactly like a click.
//!
//! Window-level pointermove + pointerup listeners are installed via the
//! `listener_slot: Signal<Option<Closure>>` + `use_drop` idiom mirrored from
//! `warp_hermes.rs:497-552`. This is the in-tree blessed pattern for
//! browser-side closure lifetime management — NOT `Box::leak` (which RESEARCH
//! Pattern 3 hand-waves but PATTERNS forbids for production).
//!
//! Pitfall 3 mitigation: drag clamp margin = 12 + RING_GAP + 7 = 33 so the
//! orange resize ring nodes always stay inside the viewport regardless of
//! where the wheel is dragged.
//!
//! UAT round 2 fix B: the drag/resize gesture state (`drag_state`/
//! `resize_state`) read inside the window-level pointermove/pointerup
//! closures is a plain `Rc<Cell<...>>`, deliberately NOT a `Signal`. A raw
//! JS callback fires outside Dioxus's own render/event-scope bookkeeping, so
//! reading a component-owned `Signal` there can trip Dioxus's owner/
//! descendant scope validation and hard-panic the WASM runtime — reproduced
//! live on the Tools page, where a background poll churns unrelated row
//! scopes every ~4s. See the doc comment on the window-listener block below
//! for the full mechanism.

use crate::components::hermes_app::screens::chat::approval_card::{
    any_approval_pending, ApprovalCardsCtx,
};
use crate::menu_layout::{catalog_entry, MenuLayout};
use crate::state::{Screen, WheelState};
#[cfg(target_arch = "wasm32")]
use dioxus::core::use_drop;
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Module-level geometry — values mirror wheel-v2.js lines 26-37 verbatim.
// ---------------------------------------------------------------------------

/// SVG-space wheel diameter. Visible size is driven by `--wheel-size` CSS.
#[allow(dead_code)] // wheel geometry constants used in SVG rendering; dead_code fires on test target
pub const SIZE: f64 = 380.0;
/// Outer rim radius — `SIZE / 2`.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const R_OUTER: f64 = 190.0;
/// Inner hub radius.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const R_INNER: f64 = 68.0;
/// Radius of label text ring.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const R_LABEL: f64 = 152.0;
/// Radius of glyph ring.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const R_GLYPH: f64 = 112.0;
/// Gap between rim and the floating resize ring.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const RING_GAP: f64 = 14.0;
/// Resize ring stroke width.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const RING_W: f64 = 4.0;
/// Extra SVG viewBox padding so the resize ring isn't clipped.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const PAD: f64 = 22.0;
/// Total viewBox extent — `SIZE + PAD * 2`.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const VB: f64 = SIZE + PAD * 2.0;
/// Shipped-default wedge count. The docked wheel's live count is the configured wheel
/// list length (6..=11, Phase 54.9 D-04); this constant is the default and the pinned
/// 54.4 parity value.
///
/// Number of wedges — 10 canonical (CONTEXT D-10 / wheel-v2.js DEFAULT_SECTIONS:
/// chat, agents, models, tools, skills, memory, sessions, providers, gateway, settings).
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const N: usize = 10;
/// Per-wedge angular step — `360 / N`.
#[allow(dead_code)] // wheel geometry constant; dead_code fires on test target
pub const STEP: f64 = 360.0 / N as f64;
/// Minimum allowed wheel size in CSS px (wheel-v2.js line 426 + Pitfall 4).
///
/// Only consumed inside the `#[cfg(target_arch = "wasm32")]` pointer-handler
/// block (drag-resize clamp) and inside `#[cfg(test)]` geometry assertions;
/// `#[allow(dead_code)]` suppresses the host-build dead-code warning.
#[allow(dead_code)]
pub const MIN_SIZE: f64 = 240.0;
/// Maximum allowed wheel size in CSS px (wheel-v2.js line 427).
#[allow(dead_code)]
pub const MAX_SIZE: f64 = 640.0;
/// Drag clamp margin — conservative `12 + RING_GAP + 7` per RESEARCH Pitfall 3.
#[allow(dead_code)]
pub const DRAG_MARGIN: f64 = 33.0;

// ---------------------------------------------------------------------------
// Module-level geometry helpers — module scope per RESEARCH anti-pattern
// "Compute wheel geometry inside `rsx!` body".
// ---------------------------------------------------------------------------

/// Convert polar coordinates `(angle_deg, r)` to Cartesian, with -90° rotation
/// so angle 0 points up (north). Mirrors wheel-v2.js `polar(angDeg, r)`.
#[allow(dead_code)] // called from wedge_path in wheel SVG rendering; dead_code fires on test target
pub fn polar(ang_deg: f64, r: f64) -> (f64, f64) {
    let a = (ang_deg - 90.0).to_radians();
    (a.cos() * r, a.sin() * r)
}

/// Build the SVG path `d` attribute for one wedge — an annular sector between
/// `r_inner` and `r_outer`, sweeping from `ang_a` to `ang_b` degrees.
/// Mirrors wheel-v2.js `wedgePath(angA, angB, rInner, rOuter)`.
#[allow(dead_code)] // called in wheel SVG rsx! rendering; dead_code fires on test target
pub fn wedge_path(ang_a: f64, ang_b: f64, r_inner: f64, r_outer: f64) -> String {
    let (x1, y1) = polar(ang_a, r_outer);
    let (x2, y2) = polar(ang_b, r_outer);
    let (x3, y3) = polar(ang_b, r_inner);
    let (x4, y4) = polar(ang_a, r_inner);
    format!(
        "M {x1} {y1} A {r_outer} {r_outer} 0 0 1 {x2} {y2} L {x3} {y3} A {r_inner} {r_inner} 0 0 0 {x4} {y4} Z"
    )
}

// ---------------------------------------------------------------------------
// Phase 54.4 Plan 01 — docked-mode pure helpers.
// ---------------------------------------------------------------------------

/// Class list of the `wheel-shell` div. The `--docked` modifier switches the
/// shell from `position: fixed` to an in-flow header-grid child.
pub(crate) fn wheel_shell_class(docked: bool) -> &'static str {
    if docked {
        "wheel-shell wheel-shell--docked"
    } else {
        "wheel-shell"
    }
}

/// Inline `style` of the `wheel-shell` div. Docked: size comes from the
/// `--ih-wheel-size` token and NO `left`/`top` is emitted (the stored
/// `WheelState` position/size is ignored — T-54.4-01). Floating: today's
/// string.
pub(crate) fn wheel_shell_style(docked: bool, size: f64, px: f64, py: f64) -> String {
    if docked {
        "--wheel-size: var(--ih-wheel-size);".to_string()
    } else {
        format!("--wheel-size: {size}px; left: {px}px; top: {py}px;")
    }
}

/// Whether `key` activates a wedge from the keyboard (Enter or Space), the
/// same way a click does.
pub(crate) fn is_wedge_activation_key(key: &Key) -> bool {
    match key {
        Key::Enter => true,
        Key::Character(c) => c == " ",
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Phase 54.9 (D-04) — n-driven geometry and the configured wheel list.
// ---------------------------------------------------------------------------

/// Per-wedge angular step for an `n`-section wheel.
pub fn step(n: usize) -> f64 {
    360.0 / n as f64
}

/// Wedge label font-size: 15.5 up to ten sections, 13.5 at eleven so a nine-character
/// label still fits the narrower chord.
pub(crate) fn label_font_size(n: usize) -> &'static str {
    if n <= 10 {
        "15.5"
    } else {
        "13.5"
    }
}

/// One resolved wedge: the screen it opens, its (uppercased) label, and the catalog
/// glyph and hub sub-line, which never change with a rename (D-02).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WheelItem {
    pub screen: Screen,
    pub label: String,
    pub glyph: &'static str,
    pub sub: &'static str,
    pub is_chat: bool,
}

/// Resolve the configured wheel list through the catalog. Unknown ids are skipped
/// (a validated layout has none).
pub(crate) fn wheel_items(layout: &MenuLayout) -> Vec<WheelItem> {
    layout
        .wheel
        .iter()
        .filter_map(|e| {
            let c = catalog_entry(&e.screen)?;
            Some(WheelItem {
                screen: c.screen,
                label: e.label.to_uppercase(),
                glyph: c.glyph,
                sub: c.sub,
                is_chat: c.screen == Screen::Chat,
            })
        })
        .collect()
}

/// A persisted active index that no longer exists after an edit falls back to Chat.
pub(crate) fn clamp_active(active: usize, items: &[WheelItem]) -> usize {
    if active < items.len() {
        active
    } else {
        items.iter().position(|i| i.is_chat).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Drag / resize internal state types.
// ---------------------------------------------------------------------------

/// Drag-rim gesture state — captured on pointerdown, mutated on pointermove.
///
/// Fields are only read inside the `#[cfg(target_arch = "wasm32")]` block
/// (window-level pointermove handler); `#[allow(dead_code)]` on the struct
/// suppresses host-build dead-code warnings.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct DragState {
    /// Client-space pointer coordinates at gesture start.
    start_client: (f64, f64),
    /// Wheel position at gesture start (so deltas apply atop the original).
    start_pos: (f64, f64),
    /// Pointer ID — preserved for future setPointerCapture wiring.
    pointer_id: i32,
    /// Max travel distance — used to suppress click after a drag.
    dist: f64,
}

/// Drag-resize-ring gesture state — captured on pointerdown.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct ResizeState {
    /// Client-space pointer coordinates at gesture start.
    start_client: (f64, f64),
    /// Wheel size at gesture start.
    start_size: f64,
    /// Pointer ID — preserved for future setPointerCapture wiring.
    pointer_id: i32,
}

// ---------------------------------------------------------------------------
// Wheel component — the SVG primitive itself.
// ---------------------------------------------------------------------------

/// The wheel SVG primitive — 10 wedges + 60 ticks + hub + resize ring +
/// floating tooltip. Reads/writes `Signal<WheelState>` and `Signal<Screen>`
/// from context (Plan 03 root provides both).
///
/// `docked: true` (Phase 54.4, D-05) mounts it fixed in the header grid:
/// no drag, no resize ring, no tooltip, no inline position.
#[component]
pub fn Wheel(#[props(default)] docked: bool) -> Element {
    let mut wheel = use_context::<Signal<WheelState>>();
    let mut active_screen = use_context::<Signal<Screen>>();
    // Phase 57 R9b: the approval-card map, provided at the HermesApp root. Snapshotted into an
    // owned bool before the rsx tree; reading it here subscribes the wheel, so the badge
    // appears and clears as cards change state.
    let approval_cards = use_context::<ApprovalCardsCtx>().0;
    let approval_pending = any_approval_pending(&approval_cards.read());
    // Phase 54.9 (D-04, D-06): the configured wheel list; `.read()` so an edit re-renders.
    let menu = use_context::<crate::components::hermes_app::MenuCtx>();
    let mut items: Vec<WheelItem> = wheel_items(&menu.layout.read());
    if items.is_empty() {
        items = wheel_items(&crate::menu_layout::default_layout());
    }
    let n = items.len();
    let wedge_step = step(n);

    // Local gesture state (UAT round 2 fix B). Deliberately a plain
    // `Rc<Cell<...>>`, NOT a `Signal` — see the fix-B doc comment on the
    // window-listener block below for why a Dioxus `Signal` here is unsafe
    // to read from a raw window-level JS callback. Nothing in the render
    // tree subscribes to this value (no reactive class or text depends on
    // it — it only drives the imperative `wheel.with_mut(...)` writes in
    // the pointermove handler), so dropping Signal reactivity here costs
    // nothing observable.
    let drag_state: std::rc::Rc<std::cell::Cell<Option<DragState>>> =
        use_hook(|| std::rc::Rc::new(std::cell::Cell::new(None)));
    let resize_state: std::rc::Rc<std::cell::Cell<Option<ResizeState>>> =
        use_hook(|| std::rc::Rc::new(std::cell::Cell::new(None)));

    // Tooltip local state.
    let mut tooltip_open = use_signal(|| false);
    let mut tooltip_resize = use_signal(|| false);
    let mut tooltip_xy = use_signal(|| (0.0_f64, 0.0_f64));

    // Read WheelState into locals BEFORE rsx! (signal-borrow safety —
    // clippy.toml forbids holding GenerationalRef across .await, and we
    // emit `wheel.write()` mutations inside event handlers below).
    let (px, py, size, active) = {
        let s = wheel.read();
        (
            s.position.0,
            s.position.1,
            s.size,
            clamp_active(s.active_index, &items),
        )
    };
    let viewbox_min = -(R_OUTER + PAD);
    let viewbox = format!("{viewbox_min} {viewbox_min} {VB} {VB}");

    // ── Window-level pointermove + pointerup listeners ──
    //
    // UAT round 2 fix B (wheel.rs regression): `dragging`/`resizing` used to
    // be `Signal<Option<...>>` values owned by THIS component's own scope,
    // read directly inside these window-level closures. That is unsafe: a
    // window listener installed here lives for the Wheel component's entire
    // mounted lifetime, but Dioxus dispatches a raw external JS callback
    // (this one included) OUTSIDE its own render/event-scope bookkeeping —
    // so whichever scope Dioxus's runtime happens to consider "current" at
    // the moment the browser actually fires a `pointermove` event is
    // effectively arbitrary. On the Tools page, where the MCP status poll
    // churns unrelated row scopes every ~4s, that "current" scope is
    // frequently one of those rows, not Wheel's own. Reading a Wheel-owned
    // `Signal` from inside the raw callback under a foreign "current" scope
    // trips Dioxus's owner/descendant validation ("a Copy Value created in
    // ScopeId(Wheel) ... was used in ScopeId(some row) ... which is not a
    // descendant of the owning scope") and can hard-panic the WASM runtime —
    // reproduced live on this page. `drag_state`/`resize_state` are
    // therefore plain `Rc<Cell<...>>`, never `Signal`s: ordinary interior
    // mutability carries no scope-ownership bookkeeping at all, so reading
    // or writing it from a raw JS callback firing under an unrelated
    // "current" scope is always safe, regardless of which scope Dioxus
    // believes is active. `wheel: Signal<WheelState>` itself stays a
    // `Signal` and is still read/written below via `wheel.read()` /
    // `wheel.with_mut(...)` — it is HermesApp-root-provided context, and
    // every scope on the page is a descendant of the root, so it never trips
    // this check the way a component-local signal does.
    //
    // Installed via the `listener_slot: Signal<Option<Closure>>` + `use_drop`
    // idiom REPLICATED from warp_hermes.rs:497-552. The cfg-gate is required
    // because `wasm_bindgen::Closure` only exists on wasm32 and the listeners
    // would not compile on the server build otherwise.
    //
    // We register exactly two window listeners (pointermove, pointerup) and
    // tear them down on Wheel unmount. Inside the move handler we branch on
    // `drag_state` / `resize_state` to drive translate / resize independently
    // — mirrors wheel-v2.js lines 402-465.
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        use web_sys::PointerEvent as WebPointerEvent;

        // Type alias to keep the Signal<Option<Closure<dyn FnMut(...)>>>
        // shape out of inline annotations (clippy::type_complexity).
        type PointerListener = Closure<dyn FnMut(WebPointerEvent)>;

        let mut move_slot: Signal<Option<PointerListener>> = use_signal(|| None);
        let mut up_slot: Signal<Option<PointerListener>> = use_signal(|| None);

        // Fresh `Rc` clones per closure — `move_cb` and `up_cb` each need
        // their own owned handle to `drag_state`/`resize_state`; cloning an
        // `Rc<Cell<...>>` is cheap (refcount bump only, no Dioxus machinery
        // involved) and carries none of the scope-ownership risk a `Signal`
        // clone would.
        let move_drag_state = drag_state.clone();
        let move_resize_state = resize_state.clone();
        let up_drag_state = drag_state.clone();
        let up_resize_state = resize_state.clone();

        use_effect(move || {
            let Some(window) = web_sys::window() else {
                return;
            };
            // Fallback dims (1920×1080) if `inner_width` is missing — shouldn't
            // happen in a real browser, but keeps the math sane in test envs.
            let win_w = window
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(1920.0);
            let win_h = window
                .inner_height()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(1080.0);

            // pointermove — drives both drag-translate and drag-resize.
            //
            // `use_effect`'s callback is `FnMut` (may run more than once),
            // so each `Rc` captured by the effect closure is `.clone()`d
            // again HERE, inside the body, rather than moved directly into
            // `move_cb`/`up_cb` — a plain `move` would only compile for a
            // single invocation of the effect body.
            let move_cb = Closure::<dyn FnMut(WebPointerEvent)>::new({
                let move_drag_state = move_drag_state.clone();
                let move_resize_state = move_resize_state.clone();
                move |e: WebPointerEvent| {
                    let cx = e.client_x() as f64;
                    let cy = e.client_y() as f64;

                    // Drag-rim translate.
                    let drag_snapshot = move_drag_state.get();
                    if let Some(mut d) = drag_snapshot {
                        let dx = cx - d.start_client.0;
                        let dy = cy - d.start_client.1;
                        d.dist = d.dist.max((dx * dx + dy * dy).sqrt());
                        // Update tracked drag distance for click suppression.
                        move_drag_state.set(Some(d));

                        let shell_size = {
                            let s = wheel.read();
                            s.size
                        };
                        let max_x = (win_w - shell_size - DRAG_MARGIN).max(DRAG_MARGIN);
                        let max_y = (win_h - shell_size - DRAG_MARGIN).max(DRAG_MARGIN);
                        let new_x = (d.start_pos.0 + dx).clamp(DRAG_MARGIN, max_x);
                        let new_y = (d.start_pos.1 + dy).clamp(DRAG_MARGIN, max_y);
                        wheel.with_mut(|s| {
                            s.position = (new_x, new_y);
                        });
                        return;
                    }

                    // Drag-resize-ring rescale.
                    let resize_snapshot = move_resize_state.get();
                    if let Some(r) = resize_snapshot {
                        let dx = cx - r.start_client.0;
                        let dy = cy - r.start_client.1;
                        // Diagonal scaling — average of the two deltas (wheel-v2.js
                        // line 458: `const delta = (dx + dy) / 2`).
                        let delta = (dx + dy) / 2.0;
                        let new_size = (r.start_size + delta).clamp(MIN_SIZE, MAX_SIZE);
                        wheel.with_mut(|s| {
                            s.size = new_size;
                        });
                    }
                }
            });
            let _ = window
                .add_event_listener_with_callback("pointermove", move_cb.as_ref().unchecked_ref());
            move_slot.set(Some(move_cb));

            // pointerup — clear both gesture states.
            let up_cb = Closure::<dyn FnMut(WebPointerEvent)>::new({
                let up_drag_state = up_drag_state.clone();
                let up_resize_state = up_resize_state.clone();
                move |_e: WebPointerEvent| {
                    up_drag_state.set(None);
                    up_resize_state.set(None);
                }
            });
            let _ = window
                .add_event_listener_with_callback("pointerup", up_cb.as_ref().unchecked_ref());
            up_slot.set(Some(up_cb));
        });

        use_drop(move || {
            if let Some(cb) = move_slot.write().take() {
                if let Some(window) = web_sys::window() {
                    let _ = window.remove_event_listener_with_callback(
                        "pointermove",
                        cb.as_ref().unchecked_ref(),
                    );
                }
                // cb drops here — wasm_bindgen JS-side reference released.
            }
            if let Some(cb) = up_slot.write().take() {
                if let Some(window) = web_sys::window() {
                    let _ = window.remove_event_listener_with_callback(
                        "pointerup",
                        cb.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    // Pre-compute the floating tooltip transform / class / labels.
    let (tx, ty) = *tooltip_xy.read();
    let tt_visible = *tooltip_open.read();
    let tt_resize = *tooltip_resize.read();
    let tt_cls = if tt_visible {
        if tt_resize {
            "wheel-tooltip is-visible is-resize"
        } else {
            "wheel-tooltip is-visible"
        }
    } else {
        "wheel-tooltip"
    };
    let tt_label = if tt_resize {
        "DRAG TO RESIZE"
    } else {
        "DRAG TO MOVE"
    };
    let tt_glyph = if tt_resize { "⤡" } else { "✥" };
    let tt_xform = format!("translate({tx}px, {ty}px)");

    // Hub label / sub from the currently-active wedge.
    let hub_label_text = items[active].label.clone();
    let hub_sub_text = items[active].sub;
    let hub_screen = items[active].screen;
    let font_size = label_font_size(n);

    let shell_class = wheel_shell_class(docked);
    let shell_style = wheel_shell_style(docked, size, px, py);

    rsx! {
        div {
            class: "{shell_class}",
            id: "wheel-shell",
            style: "{shell_style}",

            svg {
                class: "wheel-svg",
                id: "wheel-svg",
                "viewBox": "{viewbox}",

                defs {
                    radialGradient {
                        id: "hub-grad",
                        cx: "40%", cy: "40%", r: "70%",
                        stop { offset: "0%",  class: "hub-stop hub-stop--hi" }
                        stop { offset: "55%", class: "hub-stop hub-stop--mid" }
                        stop { offset: "100%", class: "hub-stop hub-stop--lo" }
                    }
                }

                // ── Opaque backing disc (G-54.4-3): wedges paint on the theme
                // card surface, never on scrolled content behind the wheel ──
                circle {
                    class: "wheel-backing",
                    cx: "0", cy: "0", r: "{R_OUTER}",
                    "pointer-events": "none",
                }

                // ── Rim decoration rings (static) ──
                {
                    let r_outer_minus_2 = R_OUTER - 2.0;
                    let r_outer_minus_12 = R_OUTER - 12.0;
                    rsx! {
                        circle {
                            cx: "0", cy: "0", r: "{r_outer_minus_2}",
                            class: "rim-ring rim-ring--outer",
                            fill: "none", "stroke-width": "1",
                        }
                        circle {
                            cx: "0", cy: "0", r: "{r_outer_minus_12}",
                            class: "rim-ring rim-ring--inner",
                            fill: "none", "stroke-width": "1", "stroke-dasharray": "2 4",
                        }
                    }
                }

                // ── Tick marks (60 ticks; non-interactive) ──
                g { id: "wheel-ticks", "pointer-events": "none",
                    for i in 0..60_usize {
                        {
                            let ang = ((i as f64) * 6.0 - 90.0).to_radians();
                            let r1 = R_OUTER - 14.0;
                            let r2 = R_OUTER - if i % 5 == 0 { 22.0 } else { 18.0 };
                            let (x1f, y1f) = (ang.cos() * r1, ang.sin() * r1);
                            let (x2f, y2f) = (ang.cos() * r2, ang.sin() * r2);
                            let tick_cls = if i % 5 == 0 {
                                "wheel-tick wheel-tick--major"
                            } else {
                                "wheel-tick wheel-tick--minor"
                            };
                            rsx! {
                                line {
                                    key: "{i}",
                                    x1: "{x1f}", y1: "{y1f}",
                                    x2: "{x2f}", y2: "{y2f}",
                                    class: "{tick_cls}", "stroke-width": "1",
                                }
                            }
                        }
                    }
                }

                // ── Rim drag hit-area (invisible annulus) ──
                {
                    let r_outer_minus_1 = R_OUTER - 1.0;
                    let neg_r_outer_minus_1 = -(R_OUTER - 1.0);
                    let r_outer_minus_28 = R_OUTER - 28.0;
                    let neg_r_outer_minus_28 = -(R_OUTER - 28.0);
                    let d = format!(
                        "M {r_outer_minus_1},0 \
                         A {r_outer_minus_1},{r_outer_minus_1} 0 1 1 {neg_r_outer_minus_1},0 \
                         A {r_outer_minus_1},{r_outer_minus_1} 0 1 1 {r_outer_minus_1},0 Z \
                         M {r_outer_minus_28},0 \
                         A {r_outer_minus_28},{r_outer_minus_28} 0 1 0 {neg_r_outer_minus_28},0 \
                         A {r_outer_minus_28},{r_outer_minus_28} 0 1 0 {r_outer_minus_28},0 Z"
                    );
                    rsx! {
                        path {
                            id: "wheel-rim",
                            class: "wheel-rim",
                            "fill-rule": "evenodd",
                            d: "{d}",
                            onpointerdown: {
                                // Fix B: fresh `Rc` clone for this handler
                                // closure — see the window-listener block's
                                // doc comment for why `drag_state` is a
                                // plain `Rc<Cell<...>>` rather than a
                                // `Signal`.
                                let drag_state = drag_state.clone();
                                move |e| {
                                    // Docked wheel (D-05): the rim is inert.
                                    if docked {
                                        return;
                                    }
                                    // Begin drag-rim translate. Per wheel-v2.js
                                    // line 391: only primary mouse button.
                                    if e.trigger_button() != Some(MouseButton::Primary) {
                                        return;
                                    }
                                    let c = e.client_coordinates();
                                    // Snapshot position INTO locals before writing gesture state.
                                    let start_pos = {
                                        let s = wheel.read();
                                        s.position
                                    };
                                    drag_state.set(Some(DragState {
                                        start_client: (c.x, c.y),
                                        start_pos,
                                        pointer_id: e.pointer_id(),
                                        dist: 0.0,
                                    }));
                                    tooltip_open.set(false);
                                    e.stop_propagation();
                                }
                            },
                            onmouseenter: move |_| {
                                tooltip_resize.set(false);
                                tooltip_open.set(true);
                            },
                            onmouseleave: move |_| {
                                tooltip_open.set(false);
                            },
                            onmousemove: move |e| {
                                let c = e.client_coordinates();
                                tooltip_xy.set((c.x, c.y));
                            },
                        }
                    }
                }

                // ── Wedges (n from the layout) — reactive is-active class ──
                g { id: "wheel-wedges",
                    for i in 0..n {
                        {
                            let ang_a = (i as f64) * wedge_step - wedge_step / 2.0;
                            let ang_b = ((i + 1) as f64) * wedge_step - wedge_step / 2.0;
                            let d = wedge_path(ang_a, ang_b, R_INNER + 8.0, R_OUTER - 30.0);
                            let cls = if i == active {
                                "wheel-wedge is-active"
                            } else {
                                "wheel-wedge"
                            };
                            let wedge_label = items[i].label.clone();
                            let target = items[i].screen;
                            // Shared by click and Enter/Space so keyboard
                            // navigation is byte-for-byte the click path.
                            let mut activate = move || {
                                // Pitfall 6 ordering: write wedge first, compute
                                // target from the new value, set screen.
                                wheel.with_mut(|s| {
                                    s.active_index = i;
                                });
                                active_screen.set(target);
                            };
                            rsx! {
                                path {
                                    key: "{i}",
                                    class: "{cls}",
                                    "data-i": "{i}",
                                    d: "{d}",
                                    // Docked wedges are too small for a legible
                                    // label (D-26), so each carries its screen
                                    // name as an SVG title + aria-label and is a
                                    // keyboard stop.
                                    role: "link",
                                    "aria-label": "{wedge_label}",
                                    tabindex: "0",
                                    onmouseenter: move |_| {
                                        // Pitfall 6 ordering: write wedge first.
                                        wheel.with_mut(|s| {
                                            s.active_index = i;
                                        });
                                    },
                                    onclick: move |_| activate(),
                                    onkeydown: move |e: KeyboardEvent| {
                                        if is_wedge_activation_key(&e.key()) {
                                            e.prevent_default();
                                            activate();
                                        }
                                    },
                                    title { "{wedge_label}" }
                                }
                            }
                        }
                    }
                }

                // ── Separators (n lines, non-interactive) ──
                g { id: "wheel-seps", "pointer-events": "none",
                    for i in 0..n {
                        {
                            let ang_a = (i as f64) * wedge_step - wedge_step / 2.0;
                            let (sx, sy) = polar(ang_a, R_INNER + 8.0);
                            let (ex, ey) = polar(ang_a, R_OUTER - 30.0);
                            rsx! {
                                line {
                                    key: "{i}",
                                    x1: "{sx}", y1: "{sy}",
                                    x2: "{ex}", y2: "{ey}",
                                    class: "wheel-sep",
                                    "stroke-width": "1",
                                }
                            }
                        }
                    }
                }

                // ── Glyph + label texts (20 nodes: 10 wedges × 2 text per wedge) ──
                g { id: "wheel-text", "pointer-events": "none",
                    for i in 0..n {
                        {
                            let mid = (i as f64) * wedge_step;
                            let wedge = &items[i];
                            let (gx, gy) = polar(mid, R_GLYPH);
                            let (lx, ly) = polar(mid, R_LABEL);
                            let g_cls = if i == active {
                                "wheel-glyph is-active"
                            } else {
                                "wheel-glyph"
                            };
                            let l_cls = if i == active {
                                "wheel-label is-active"
                            } else {
                                "wheel-label"
                            };
                            let g_xform = format!("rotate({mid} {gx} {gy})");
                            let l_xform = format!("rotate({mid} {lx} {ly})");
                            let gy5 = gy + 9.0;
                            let ly3 = ly + 5.0;
                            let glyph_text = wedge.glyph;
                            let label_text = wedge.label.clone();
                            // Phase 57 R9b: the pending-approval badge sits up and to the
                            // right of the CHAT glyph (the only Chat nav item), inside the
                            // viewBox. Bare circle: no text, no sound, no notice.
                            let show_badge = approval_pending && wedge.is_chat;
                            let badge_cx = gx + 16.0;
                            let badge_cy = gy - 14.0;
                            rsx! {
                                text {
                                    key: "{i}",
                                    x: "{gx}", y: "{gy5}",
                                    "text-anchor": "middle",
                                    "font-size": "28",
                                    class: "{g_cls}",
                                    "data-i": "{i}",
                                    transform: "{g_xform}",
                                    "{glyph_text}"
                                }
                                text {
                                    x: "{lx}", y: "{ly3}",
                                    "text-anchor": "middle",
                                    "font-family": "JetBrains Mono, monospace",
                                    "font-size": "{font_size}",
                                    "font-weight": "700",
                                    "letter-spacing": "0.5",
                                    class: "{l_cls}",
                                    "data-i": "{i}",
                                    transform: "{l_xform}",
                                    "{label_text}"
                                }
                                if show_badge {
                                    circle {
                                        class: "wheel-approval-badge",
                                        cx: "{badge_cx}", cy: "{badge_cy}", r: "6",
                                        role: "img",
                                        "aria-label": "Approval pending",
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Inner divider ring ──
                {
                    let r_inner_plus_6 = R_INNER + 6.0;
                    rsx! {
                        circle {
                            cx: "0", cy: "0", r: "{r_inner_plus_6}",
                            class: "wheel-inner-ring",
                            fill: "none",
                            "stroke-width": "1", "pointer-events": "none",
                        }
                    }
                }

                // ── HUB (launch button) — reactive label/sub from active_wedge ──
                {
                    let r_inner_minus_6 = R_INNER - 6.0;
                    let r_inner_minus_14 = R_INNER - 14.0;
                    let r_inner_minus_4 = R_INNER - 4.0;
                    rsx! {
                        g {
                            id: "wheel-hub",
                            class: "wheel-hub",
                            onclick: move |_| {
                                // Snapshot target before signal write to avoid
                                // holding the read borrow across .set().
                                active_screen.set(hub_screen);
                            },
                            circle {
                                id: "hub-bg",
                                cx: "0", cy: "0", r: "{R_INNER}",
                                fill: "url(#hub-grad)",
                                "stroke-width": "1",
                            }
                            circle {
                                cx: "0", cy: "0", r: "{r_inner_minus_6}",
                                class: "hub-ring hub-ring--inner",
                                fill: "none",
                            }
                            circle {
                                cx: "0", cy: "0", r: "{r_inner_minus_14}",
                                class: "hub-ring hub-ring--dashed",
                                fill: "none",
                                "stroke-dasharray": "2 3",
                            }
                            circle {
                                id: "hub-aura",
                                cx: "0", cy: "0", r: "{r_inner_minus_4}",
                            }

                            text {
                                id: "hub-glyph",
                                x: "0", y: "-26",
                                "text-anchor": "middle",
                                "font-family": "JetBrains Mono, monospace",
                                "font-size": "18",
                                "▶"
                            }
                            text {
                                id: "hub-label",
                                x: "0", y: "-4",
                                "text-anchor": "middle",
                                "font-family": "JetBrains Mono, monospace",
                                "font-size": "16",
                                "font-weight": "700",
                                "letter-spacing": "1",
                                "{hub_label_text}"
                            }
                            text {
                                id: "hub-sub",
                                x: "0", y: "11",
                                "text-anchor": "middle",
                                "font-family": "JetBrains Mono, monospace",
                                "font-size": "9",
                                "letter-spacing": "0.6",
                                "{hub_sub_text}"
                            }
                            text {
                                id: "hub-cta",
                                x: "0", y: "28",
                                "text-anchor": "middle",
                                "font-family": "JetBrains Mono, monospace",
                                "font-size": "9",
                                "font-weight": "700",
                                "letter-spacing": "1.5",
                                "▸ LAUNCH"
                            }
                        }
                    }
                }

                // ── Hub arrow triangle ──
                {
                    let arrow_tip_y = -(R_INNER + 12.0);
                    let arrow_base_y = -(R_INNER + 24.0);
                    let points = format!("0,{arrow_tip_y} -6,{arrow_base_y} 6,{arrow_base_y}");
                    rsx! {
                        polygon {
                            points: "{points}",
                            id: "hub-arrow",
                            "pointer-events": "none",
                        }
                    }
                }

                // ── Resize ring (3 nested circles) + 4 orbit nodes ──
                // Floating mode only: a docked wheel is not resizable (D-05).
                if !docked {
                  {
                    let r_resize = R_OUTER + RING_GAP;
                    let r_orbit = R_OUTER + RING_GAP + 7.0;
                    let glow_width = RING_W + 6.0;
                    let neg_r_resize = -(R_OUTER + RING_GAP);
                    rsx! {
                        circle {
                            id: "resize-glow",
                            cx: "0", cy: "0", r: "{r_resize}",
                            fill: "none",
                            "stroke-width": "{glow_width}",
                            "pointer-events": "none",
                        }
                        circle {
                            id: "resize-orbit",
                            cx: "0", cy: "0", r: "{r_orbit}",
                            fill: "none",
                            "stroke-width": "1",
                            "stroke-dasharray": "1 6",
                            "pointer-events": "none",
                        }
                        circle {
                            id: "resize-ring",
                            class: "resize-ring",
                            cx: "0", cy: "0", r: "{r_resize}",
                            fill: "none",
                            "stroke-width": "{RING_W}",
                            "pointer-events": "stroke",
                            onpointerdown: {
                                // Fix B: fresh `Rc` clone for this handler
                                // closure — see the window-listener block's
                                // doc comment for why `resize_state` is a
                                // plain `Rc<Cell<...>>` rather than a
                                // `Signal`.
                                let resize_state = resize_state.clone();
                                move |e| {
                                    if e.trigger_button() != Some(MouseButton::Primary) {
                                        return;
                                    }
                                    let c = e.client_coordinates();
                                    let start_size = {
                                        let s = wheel.read();
                                        s.size
                                    };
                                    resize_state.set(Some(ResizeState {
                                        start_client: (c.x, c.y),
                                        start_size,
                                        pointer_id: e.pointer_id(),
                                    }));
                                    tooltip_open.set(false);
                                    e.stop_propagation();
                                }
                            },
                            onmouseenter: move |_| {
                                tooltip_resize.set(true);
                                tooltip_open.set(true);
                            },
                            onmouseleave: move |_| {
                                tooltip_open.set(false);
                            },
                            onmousemove: move |e| {
                                let c = e.client_coordinates();
                                tooltip_xy.set((c.x, c.y));
                            },
                        }
                        g { id: "resize-nodes", "pointer-events": "none",
                            circle { cx: "{r_resize}",     cy: "0",            r: "3" }
                            circle { cx: "{neg_r_resize}", cy: "0",            r: "3" }
                            circle { cx: "0",              cy: "{r_resize}",   r: "3" }
                            circle { cx: "0",              cy: "{neg_r_resize}", r: "3" }
                        }
                    }
                  }
                }
            }

            // ── Floating tooltip (Pattern 2 tooltip block) ──
            // Floating mode only: the docked wheel has no "DRAG TO MOVE" hint.
            if !docked {
                div {
                    class: "{tt_cls}",
                    id: "wheel-tooltip",
                    "aria-hidden": "true",
                    style: "transform: {tt_xform};",
                    span { class: "wheel-tooltip-glyph", "{tt_glyph}" }
                    span { class: "wheel-tooltip-label", "{tt_label}" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — verify geometry constants + helper invariants. No DOM tests; the
// pointer/window glue is unit-untestable without a wasm-bindgen test runner.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_matches_v2_constants() {
        // wheel-v2.js lines 26-37 — exact values.
        assert_eq!(SIZE, 380.0);
        assert_eq!(R_OUTER, 190.0);
        assert_eq!(R_INNER, 68.0);
        assert_eq!(R_LABEL, 152.0);
        assert_eq!(R_GLYPH, 112.0);
        assert_eq!(RING_GAP, 14.0);
        assert_eq!(RING_W, 4.0);
        assert_eq!(PAD, 22.0);
        assert_eq!(VB, 424.0);
        // CONTEXT D-10 (Phase 54.4: Kanban wedge removed): 10 wedges total.
        assert_eq!(N, 10);
        let expected_step = 360.0_f64 / 10.0;
        assert!(
            (STEP - expected_step).abs() < 1e-9,
            "STEP={STEP} expected={expected_step} (10 wedges)"
        );
        // wheel-v2.js lines 426-427 + Pitfall 4 floor.
        assert_eq!(MIN_SIZE, 240.0);
        assert_eq!(MAX_SIZE, 640.0);
        // Pitfall 3 conservative margin (12 + RING_GAP + 7).
        assert_eq!(DRAG_MARGIN, 33.0);
    }

    #[test]
    fn polar_at_zero_is_top() {
        // -90° rotation in polar() puts angle 0 at the top (cos(-π/2)=0,
        // sin(-π/2)=-1 → multiplied by r=100 → (0, -100)).
        let (x, y) = polar(0.0, 100.0);
        assert!(x.abs() < 1e-9, "x={x} should be ~0");
        assert!((y + 100.0).abs() < 1e-9, "y={y} should be ~-100");
    }

    #[test]
    fn polar_at_ninety_is_right() {
        // angle 90° (after -90° rotation = 0°) is on the +x axis.
        let (x, y) = polar(90.0, 100.0);
        assert!((x - 100.0).abs() < 1e-9, "x={x} should be ~100");
        assert!(y.abs() < 1e-9, "y={y} should be ~0");
    }

    #[test]
    fn wedge_path_starts_with_m() {
        // One full wedge sweep at STEP=36° (Plan 04 wedge geometry).
        let p = wedge_path(0.0, STEP, R_INNER, R_OUTER);
        assert!(
            p.starts_with("M "),
            "wedge path should start with 'M ', got: {p}"
        );
        assert!(p.contains(" A "), "wedge path should contain arc segments");
        assert!(p.contains(" L "), "wedge path should contain line segment");
        assert!(p.ends_with(" Z"), "wedge path should close with Z");
    }

    #[test]
    fn wedge_path_renders_all_wedges() {
        // Smoke: every wedge index produces a non-empty path. N=10.
        for i in 0..N {
            let ang_a = (i as f64) * STEP - STEP / 2.0;
            let ang_b = ((i + 1) as f64) * STEP - STEP / 2.0;
            let p = wedge_path(ang_a, ang_b, R_INNER + 8.0, R_OUTER - 30.0);
            assert!(!p.is_empty(), "wedge {i} path is empty");
            assert!(p.starts_with("M "), "wedge {i} path malformed: {p}");
        }
    }

    // -----------------------------------------------------------------------
    // Plan 09 — Wave-0 named assertions extracted for VALIDATION.md grep lock.
    // -----------------------------------------------------------------------

    #[test]
    fn min_size_within_resize_range() {
        // wheel-v2.js lines 426-427 + Pitfall 4 floor: [MIN_SIZE, MAX_SIZE]
        // bounds the wheel resize gesture. Asserted as a standalone test so
        // VALIDATION.md Wave 0 can grep for the safety bound by name.
        assert_eq!(MIN_SIZE, 240.0);
        assert_eq!(MAX_SIZE, 640.0);
        // Compile-time guard: MIN_SIZE < MAX_SIZE must always hold.
        const _: () = assert!(
            MIN_SIZE < MAX_SIZE,
            "MIN_SIZE must be strictly less than MAX_SIZE"
        );
    }

    #[test]
    fn drag_margin_is_conservative() {
        // RESEARCH Pitfall 3: DRAG_MARGIN = 12 + RING_GAP + 7 = 33.
        // The margin must exceed RING_GAP so the resize ring stays inside
        // the viewport during translate.
        assert_eq!(DRAG_MARGIN, 33.0);
        // Compile-time guard: DRAG_MARGIN must exceed RING_GAP.
        const _: () = assert!(DRAG_MARGIN > RING_GAP, "DRAG_MARGIN must exceed RING_GAP");
    }

    #[test]
    fn step_tiles_full_circle() {
        // CONTEXT D-10: 10 wedges exactly tile the circle, so
        // STEP = 360 / N = 36°. The float STEP must equal 360.0 / 10.0 to
        // within 1e-9.
        let expected = 360.0_f64 / 10.0;
        assert!(
            (STEP - expected).abs() < 1e-9,
            "STEP={STEP} expected={expected}"
        );
        assert!(
            (STEP * N as f64 - 360.0).abs() < 1e-9,
            "10 wedges × STEP must exactly tile 360°"
        );
    }

    #[test]
    fn wedge_activation_keys_are_enter_and_space_only() {
        assert!(is_wedge_activation_key(&Key::Enter));
        assert!(is_wedge_activation_key(&Key::Character(" ".to_string())));
        assert!(!is_wedge_activation_key(&Key::Character("a".to_string())));
        assert!(!is_wedge_activation_key(&Key::Escape));
        assert!(!is_wedge_activation_key(&Key::Tab));
    }

    #[test]
    fn longest_label_fits_inside_its_wedge_at_label_radius() {
        // Label letter-spacing 0.5, JetBrains Mono advance 0.6em. The text is
        // tangential, so a nine-character label must fit the chord of an
        // n-section wedge at the label's INNER edge, for every allowed n.
        use crate::menu_layout::{WHEEL_MAX, WHEEL_MIN};
        let spacing = 0.5_f64;
        let r = |v: f64| std::hint::black_box(v);
        for n in WHEEL_MIN..=WHEEL_MAX {
            let font: f64 = label_font_size(n).parse().unwrap();
            let longest = 9.0_f64;
            let width = longest * (font * 0.6) + (longest - 1.0) * spacing;
            let inner_edge = R_LABEL - font * 0.37;
            let chord = 2.0 * inner_edge * (step(n) / 2.0).to_radians().sin();
            assert!(
                width < chord,
                "n={n}: label width {width} must fit chord {chord}"
            );
            assert!(
                r(R_LABEL) + font * 0.37 <= r(R_OUTER) - 30.0,
                "label clips the wedge band"
            );
        }
        // Glyph ring sits between the hub edge and the label.
        assert!(r(R_GLYPH) - 14.0 > r(R_INNER) + 8.0 && r(R_GLYPH) + 14.0 < r(R_LABEL) - 6.0);
    }

    #[test]
    fn label_font_steps_down_only_at_eleven() {
        assert_eq!(label_font_size(6), "15.5");
        assert_eq!(label_font_size(10), "15.5");
        assert_eq!(label_font_size(11), "13.5");
    }

    #[test]
    fn step_of_n_tiles_full_circle() {
        for n in 6..=11 {
            assert!((step(n) * n as f64 - 360.0).abs() < 1e-9, "n={n}");
        }
    }

    fn layout_of(ids: &[&str]) -> crate::menu_layout::MenuLayout {
        let mut l = crate::menu_layout::default_layout();
        l.wheel = ids
            .iter()
            .map(|id| crate::menu_layout::MenuEntry {
                screen: (*id).to_string(),
                label: catalog_entry(id).unwrap().name.to_string(),
            })
            .collect();
        l
    }

    #[test]
    fn docked_wheel_renders_configured_count() {
        let layout = layout_of(&["chat", "agents", "models", "tools", "skills", "settings"]);
        let html = render_wheel_with(vec![], layout);
        assert_eq!(html.matches("class=\"wheel-wedge").count(), 6, "{html}");
        assert_eq!(html.matches("wheel-sep\"").count(), 6, "{html}");
        let labels: Vec<String> = html
            .split("class=\"wheel-label")
            .skip(1)
            .map(|p| {
                let body = &p[p.find('>').unwrap() + 1..];
                body[..body.find('<').unwrap()].to_string()
            })
            .collect();
        assert_eq!(
            labels,
            ["CHAT", "AGENTS", "MODELS", "TOOLS", "SKILLS", "SETTINGS"]
        );
    }

    #[test]
    fn docked_wheel_eleven_sections_use_small_font() {
        let layout = layout_of(&[
            "chat",
            "agents",
            "models",
            "tools",
            "skills",
            "memory",
            "sessions",
            "providers",
            "gateway",
            "settings",
            "kanban",
        ]);
        let html = render_wheel_with(vec![], layout);
        assert_eq!(html.matches("class=\"wheel-wedge").count(), 11);
        assert_eq!(html.matches("font-size=\"13.5\"").count(), 11, "{html}");
        assert!(!html.contains("font-size=\"15.5\""));
    }

    #[test]
    fn renamed_label_drives_title_hub_and_uppercase() {
        let mut layout = crate::menu_layout::default_layout();
        layout.wheel[0].label = "talk".to_string();
        let html = render_wheel_with(vec![], layout);
        assert!(html.contains("aria-label=\"TALK\""), "{html}");
        assert!(html.contains("<title>TALK</title>"), "{html}");
        let hub = html.split("id=\"hub-label\"").nth(1).unwrap();
        assert!(
            hub[..hub.find("</text>").unwrap()].ends_with(">TALK"),
            "{hub}"
        );
        assert!(html.contains("▓"), "chat glyph unchanged");
    }

    #[test]
    fn header_origin_screen_on_wheel_uses_catalog_glyph() {
        let layout = layout_of(&["chat", "agents", "models", "tools", "skills", "kanban"]);
        let html = render_wheel_with(vec![], layout.clone());
        assert!(html.contains("▦"), "{html}");
        let items = wheel_items(&layout);
        assert_eq!(items[5].screen, Screen::Kanban);
        assert_eq!(items[5].glyph, "▦");
        assert_eq!(items[5].sub, "TASK BOARD");
    }

    #[test]
    fn approval_badge_follows_chat_position() {
        let layout = layout_of(&["agents", "models", "tools", "chat", "skills", "settings"]);
        let html = render_wheel_with(vec![pending_state()], layout);
        assert_eq!(html.matches("wheel-approval-badge").count(), 1, "{html}");
        let before = &html[..html.find("wheel-approval-badge").unwrap()];
        let last_glyph = before.rfind("class=\"wheel-glyph").unwrap();
        assert!(
            before[last_glyph..].contains("data-i=\"3\""),
            "{}",
            &before[last_glyph..]
        );
    }

    #[test]
    fn clamp_active_falls_back_to_chat() {
        let items = wheel_items(&layout_of(&[
            "agents", "models", "chat", "tools", "skills", "settings",
        ]));
        assert_eq!(clamp_active(10, &items), 2);
        assert_eq!(clamp_active(4, &items), 4);
    }

    // -----------------------------------------------------------------------
    // Phase 57 UI-SPEC R9b — the pending-approval badge on the CHAT wedge.
    // -----------------------------------------------------------------------

    use crate::components::hermes_app::screens::chat::approval_card::{
        any_approval_pending, ApprovalCardModel, ApprovalCardOutcome, ApprovalCardState,
        ApprovalCards, ApprovalCardsCtx,
    };
    use std::cell::RefCell;

    thread_local! {
        static BADGE_STATES: RefCell<Vec<ApprovalCardState>> = const { RefCell::new(Vec::new()) };
        static WHEEL_LAYOUT: RefCell<crate::menu_layout::MenuLayout> =
            RefCell::new(crate::menu_layout::default_layout());
    }

    fn cards_of(states: &[ApprovalCardState]) -> ApprovalCards {
        states
            .iter()
            .enumerate()
            .map(|(i, state)| {
                (
                    format!("a{i}"),
                    ApprovalCardModel {
                        session_id: "s".into(),
                        tool: "terminal".into(),
                        reason: "r".into(),
                        state: state.clone(),
                    },
                )
            })
            .collect()
    }

    fn render_docked_wheel(states: Vec<ApprovalCardState>) -> String {
        render_wheel_with(states, crate::menu_layout::default_layout())
    }

    fn render_wheel_with(
        states: Vec<ApprovalCardState>,
        layout: crate::menu_layout::MenuLayout,
    ) -> String {
        BADGE_STATES.with(|s| *s.borrow_mut() = states);
        WHEEL_LAYOUT.with(|l| *l.borrow_mut() = layout);
        fn root() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let wheel = use_signal(WheelState::default);
            use_context_provider(|| wheel);
            let cards = use_signal(|| BADGE_STATES.with(|s| cards_of(&s.borrow())));
            use_context_provider(|| ApprovalCardsCtx(cards));
            let layout = use_signal(|| WHEEL_LAYOUT.with(|l| l.borrow().clone()));
            let notice = use_signal(|| None::<String>);
            let write = use_signal(|| false);
            let save = crate::components::hermes_app::screens::settings_menu::use_save_queue();
            use_context_provider(|| crate::components::hermes_app::MenuCtx {
                layout,
                notice,
                write_enabled: write,
                save,
            });
            rsx! { Wheel { docked: true } }
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn pending_state() -> ApprovalCardState {
        ApprovalCardState::Pending { error: false }
    }

    #[test]
    fn chat_nav_badge_tracks_pending_approval() {
        let resolved = ApprovalCardState::Resolved {
            outcome: ApprovalCardOutcome::Denied,
        };
        let err = ApprovalCardState::Pending { error: true };
        let submitted = ApprovalCardState::Submitted { approved: true };
        assert!(!any_approval_pending(&ApprovalCards::new()));
        assert!(any_approval_pending(&cards_of(&[pending_state()])));
        assert!(any_approval_pending(&cards_of(&[
            resolved.clone(),
            submitted.clone()
        ])));
        assert!(!any_approval_pending(&cards_of(&[
            err.clone(),
            resolved.clone()
        ])));

        let html = render_docked_wheel(vec![pending_state()]);
        assert_eq!(
            html.matches("wheel-approval-badge").count(),
            1,
            "exactly one badge: {html}"
        );
        assert!(html.contains("aria-label=\"Approval pending\""), "{html}");
        let at = html.find("wheel-approval-badge").unwrap();
        let open = html[..at].rfind('<').unwrap();
        assert!(
            html[open..].starts_with("<circle"),
            "the badge is one SVG circle: {}",
            &html[open..at]
        );
        let tag_end = at + html[at..].find('>').unwrap();
        assert!(
            html[tag_end..].starts_with("></circle>"),
            "the badge has no text child: {}",
            &html[open..tag_end + 12]
        );

        for states in [vec![resolved.clone()], vec![err], vec![]] {
            assert!(
                !render_docked_wheel(states).contains("wheel-approval-badge"),
                "no badge without a card that waits on the operator"
            );
        }
        assert!(
            render_docked_wheel(vec![submitted]).contains("wheel-approval-badge"),
            "a decision in flight keeps the badge lit"
        );

        // Structural contracts a browser alone could observe.
        const WHEEL_RS: &str = include_str!("wheel.rs");
        const MOD_RS: &str = include_str!("mod.rs");
        const WHEEL_CSS: &str = include_str!("../../../assets/wheel.css");
        let code = |s: &str| {
            s.lines()
                .map(|l| l.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let wheel_code = code(&WHEEL_RS[..WHEEL_RS.rfind(concat!("#[cfg(", "test)]")).unwrap()]);
        for banned in [
            concat!("Html", "AudioElement"),
            concat!("audio", " {"),
            concat!("Agent", "NoticeCtx"),
        ] {
            assert!(!wheel_code.contains(banned), "wheel.rs contains `{banned}`");
        }
        assert!(
            code(MOD_RS).matches(concat!("apply_disconnect", "(")).count() >= 3,
            "the failed-connect, Close and Err socket-down paths each apply the disconnect"
        );
        let css: String = WHEEL_CSS.split("/*").enumerate().map(|(i, part)| {
            if i == 0 { part } else { part.split_once("*/").map_or("", |x| x.1) }
        }).collect();
        let rule = css
            .split('}')
            .find(|c| c.contains(".wheel-approval-badge"))
            .expect("wheel.css needs a .wheel-approval-badge rule");
        assert!(rule.contains("fill: var(--amber)"), "{rule}");
        assert!(!rule.contains('#'), "theme variables only: {rule}");
    }

    /// D-04: the shipped-default docked wheel is byte-identical to the golden captured
    /// before the n-driven refactor. `WHEEL_GOLDEN_UPDATE=1` rewrites the file.
    #[test]
    fn docked_default_wheel_matches_golden() {
        const GOLDEN: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/components/hermes_app/wheel_docked_default.golden.html"
        );
        let html = render_docked_wheel(vec![]);
        if std::env::var_os("WHEEL_GOLDEN_UPDATE").is_some() {
            std::fs::write(GOLDEN, &html).expect("write golden");
            return;
        }
        let golden = std::fs::read_to_string(GOLDEN).expect("read golden");
        assert!(
            html == golden,
            "docked default wheel drifted from the golden"
        );
    }

    #[test]
    fn viewbox_equals_size_plus_padding() {
        // wheel-v2.js line 33: VB = SIZE + PAD * 2 = 380 + 22*2 = 424.
        assert!((VB - (SIZE + PAD * 2.0)).abs() < 1e-9);
        assert_eq!(VB, 424.0);
    }
}

// Canonical 10-wedge order documented for grep lock-down (CONTEXT D-10):
// chat, agents, models, tools, skills, memory, sessions, providers, gateway,
// settings
