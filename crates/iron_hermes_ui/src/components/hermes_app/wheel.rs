//! Phase 26.2.1 Plan 04 — Wheel SVG primitive.
//!
//! Ports `crates/iron_hermes_ui/filestoimport/ironhermes-design-system/project/wheel-v2.js`
//! into a self-contained Dioxus 0.7 component. Renders a draggable, resizable
//! 10-wedge SVG wheel with hover/click navigation. Geometry constants match
//! `wheel-v2.js` lines 26-37 byte-for-byte.
//!
//! Canonical 10-wedge order from `wheel-v2.js` DEFAULT_SECTIONS (CONTEXT D-10):
//!   chat, agents, models, tools, skills, memory, sessions, providers, gateway, settings
//!
//! Interactions (CONTEXT D-11 / wheel-v2.js parity):
//! - Hover wedge → set `WheelState.active_wedge`
//! - Click wedge → set wedge AND `active_screen = wedge.to_screen()`
//! - Click hub → launch currently-active wedge
//! - Drag rim → translate `WheelState.position` (window-level pointermove)
//! - Drag resize ring → rescale `WheelState.size` clamped [MIN_SIZE, MAX_SIZE]
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

use crate::state::{Screen, WheelState, WheelWedge};
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use dioxus::core::use_drop;

// ---------------------------------------------------------------------------
// Module-level geometry — values mirror wheel-v2.js lines 26-37 verbatim.
// ---------------------------------------------------------------------------

/// SVG-space wheel diameter. Visible size is driven by `--wheel-size` CSS.
pub const SIZE: f64 = 380.0;
/// Outer rim radius — `SIZE / 2`.
pub const R_OUTER: f64 = 190.0;
/// Inner hub radius.
pub const R_INNER: f64 = 78.0;
/// Radius of label text ring.
pub const R_LABEL: f64 = 148.0;
/// Radius of glyph ring.
pub const R_GLYPH: f64 = 118.0;
/// Gap between rim and the floating resize ring.
pub const RING_GAP: f64 = 14.0;
/// Resize ring stroke width.
pub const RING_W: f64 = 4.0;
/// Extra SVG viewBox padding so the resize ring isn't clipped.
pub const PAD: f64 = 22.0;
/// Total viewBox extent — `SIZE + PAD * 2`.
pub const VB: f64 = SIZE + PAD * 2.0;
/// Number of wedges (CONTEXT D-10 / wheel-v2.js DEFAULT_SECTIONS — 10 wedges:
/// chat, agents, models, tools, skills, memory, sessions, providers, gateway, settings).
pub const N: usize = 10;
/// Per-wedge angular step — `360 / N`.
pub const STEP: f64 = 360.0 / N as f64;
/// Minimum allowed wheel size in CSS px (wheel-v2.js line 426 + Pitfall 4).
pub const MIN_SIZE: f64 = 240.0;
/// Maximum allowed wheel size in CSS px (wheel-v2.js line 427).
pub const MAX_SIZE: f64 = 640.0;
/// Drag clamp margin — conservative `12 + RING_GAP + 7` per RESEARCH Pitfall 3.
pub const DRAG_MARGIN: f64 = 33.0;

// ---------------------------------------------------------------------------
// Module-level geometry helpers — module scope per RESEARCH anti-pattern
// "Compute wheel geometry inside `rsx!` body".
// ---------------------------------------------------------------------------

/// Convert polar coordinates `(angle_deg, r)` to Cartesian, with -90° rotation
/// so angle 0 points up (north). Mirrors wheel-v2.js `polar(angDeg, r)`.
pub fn polar(ang_deg: f64, r: f64) -> (f64, f64) {
    let a = (ang_deg - 90.0).to_radians();
    (a.cos() * r, a.sin() * r)
}

/// Build the SVG path `d` attribute for one wedge — an annular sector between
/// `r_inner` and `r_outer`, sweeping from `ang_a` to `ang_b` degrees.
/// Mirrors wheel-v2.js `wedgePath(angA, angB, rInner, rOuter)`.
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
// Drag / resize internal state types.
// ---------------------------------------------------------------------------

/// Drag-rim gesture state — captured on pointerdown, mutated on pointermove.
#[derive(Clone, Copy, Debug)]
struct DragState {
    /// Client-space pointer coordinates at gesture start.
    start_client: (f64, f64),
    /// Wheel position at gesture start (so deltas apply atop the original).
    start_pos: (f64, f64),
    /// Pointer ID — preserved for future setPointerCapture wiring.
    #[allow(dead_code)]
    pointer_id: i32,
    /// Max travel distance — used to suppress click after a drag.
    dist: f64,
}

/// Drag-resize-ring gesture state — captured on pointerdown.
#[derive(Clone, Copy, Debug)]
struct ResizeState {
    /// Client-space pointer coordinates at gesture start.
    start_client: (f64, f64),
    /// Wheel size at gesture start.
    start_size: f64,
    /// Pointer ID — preserved for future setPointerCapture wiring.
    #[allow(dead_code)]
    pointer_id: i32,
}

// ---------------------------------------------------------------------------
// Wheel component — the SVG primitive itself.
// ---------------------------------------------------------------------------

/// The wheel SVG primitive — 10 wedges + 60 ticks + hub + resize ring +
/// floating tooltip. Reads/writes `Signal<WheelState>` and `Signal<Screen>`
/// from context (Plan 03 root provides both).
#[component]
pub fn Wheel() -> Element {
    let mut wheel = use_context::<Signal<WheelState>>();
    let mut active_screen = use_context::<Signal<Screen>>();

    // Local gesture state.
    let mut dragging = use_signal(|| Option::<DragState>::None);
    let mut resizing = use_signal(|| Option::<ResizeState>::None);

    // Tooltip local state.
    let mut tooltip_open = use_signal(|| false);
    let mut tooltip_resize = use_signal(|| false);
    let mut tooltip_xy = use_signal(|| (0.0_f64, 0.0_f64));

    // Read WheelState into locals BEFORE rsx! (signal-borrow safety —
    // clippy.toml forbids holding GenerationalRef across .await, and we
    // emit `wheel.write()` mutations inside event handlers below).
    let (px, py, size, active) = {
        let s = wheel.read();
        (s.position.0, s.position.1, s.size, s.active_wedge.index())
    };
    let viewbox_min = -(R_OUTER + PAD);
    let viewbox = format!("{viewbox_min} {viewbox_min} {VB} {VB}");

    // ── Window-level pointermove + pointerup listeners ──
    //
    // Installed via the `listener_slot: Signal<Option<Closure>>` + `use_drop`
    // idiom REPLICATED from warp_hermes.rs:497-552. The cfg-gate is required
    // because `wasm_bindgen::Closure` only exists on wasm32 and the listeners
    // would not compile on the server build otherwise.
    //
    // We register exactly two window listeners (pointermove, pointerup) and
    // tear them down on Wheel unmount. Inside the move handler we branch on
    // `dragging` / `resizing` to drive translate / resize independently —
    // mirrors wheel-v2.js lines 402-465.
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        use web_sys::PointerEvent as WebPointerEvent;

        let mut move_slot: Signal<Option<Closure<dyn FnMut(WebPointerEvent)>>> =
            use_signal(|| None);
        let mut up_slot: Signal<Option<Closure<dyn FnMut(WebPointerEvent)>>> = use_signal(|| None);

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
            let move_cb = Closure::<dyn FnMut(WebPointerEvent)>::new(move |e: WebPointerEvent| {
                let cx = e.client_x() as f64;
                let cy = e.client_y() as f64;

                // Drag-rim translate.
                let drag_snapshot = *dragging.read();
                if let Some(mut d) = drag_snapshot {
                    let dx = cx - d.start_client.0;
                    let dy = cy - d.start_client.1;
                    d.dist = d.dist.max((dx * dx + dy * dy).sqrt());
                    // Update tracked drag distance for click suppression.
                    dragging.set(Some(d));

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
                let resize_snapshot = *resizing.read();
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
            });
            let _ = window
                .add_event_listener_with_callback("pointermove", move_cb.as_ref().unchecked_ref());
            move_slot.set(Some(move_cb));

            // pointerup — clear both gesture signals.
            let up_cb = Closure::<dyn FnMut(WebPointerEvent)>::new(move |_e: WebPointerEvent| {
                dragging.set(None);
                resizing.set(None);
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
    let active_wedge = WheelWedge::from_index(active);
    let hub_label_text = active_wedge.label();
    let hub_sub_text = active_wedge.sub();

    rsx! {
        div {
            class: "wheel-shell",
            id: "wheel-shell",
            style: "--wheel-size: {size}px; left: {px}px; top: {py}px;",

            svg {
                class: "wheel-svg",
                id: "wheel-svg",
                "viewBox": "{viewbox}",

                defs {
                    radialGradient {
                        id: "hub-grad",
                        cx: "40%", cy: "40%", r: "70%",
                        stop { offset: "0%",  "stop-color": "#1f2934" }
                        stop { offset: "55%", "stop-color": "#121820" }
                        stop { offset: "100%", "stop-color": "#070b10" }
                    }
                    radialGradient {
                        id: "hub-grad-hover",
                        cx: "40%", cy: "40%", r: "70%",
                        stop { offset: "0%",  "stop-color": "#1d3a44" }
                        stop { offset: "55%", "stop-color": "#0c2329" }
                        stop { offset: "100%", "stop-color": "#06141a" }
                    }
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
                            let color = if i % 5 == 0 {
                                "rgba(57,197,207,0.55)"
                            } else {
                                "rgba(110,118,129,0.35)"
                            };
                            rsx! {
                                line {
                                    key: "{i}",
                                    x1: "{x1f}", y1: "{y1f}",
                                    x2: "{x2f}", y2: "{y2f}",
                                    stroke: "{color}", "stroke-width": "1",
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
                            fill: "rgba(57,197,207,0.001)",
                            "fill-rule": "evenodd",
                            d: "{d}",
                            onpointerdown: move |e| {
                                // Begin drag-rim translate. Per wheel-v2.js
                                // line 391: only primary mouse button.
                                if e.trigger_button() != Some(MouseButton::Primary) {
                                    return;
                                }
                                let c = e.client_coordinates();
                                // Snapshot position INTO locals before writing dragging signal.
                                let start_pos = {
                                    let s = wheel.read();
                                    s.position
                                };
                                dragging.set(Some(DragState {
                                    start_client: (c.x, c.y),
                                    start_pos,
                                    pointer_id: e.pointer_id(),
                                    dist: 0.0,
                                }));
                                tooltip_open.set(false);
                                e.stop_propagation();
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

                // ── Wedges (N=10) — reactive is-active class ──
                g { id: "wheel-wedges",
                    for i in 0..N {
                        {
                            let ang_a = (i as f64) * STEP - STEP / 2.0;
                            let ang_b = ((i + 1) as f64) * STEP - STEP / 2.0;
                            let d = wedge_path(ang_a, ang_b, R_INNER + 8.0, R_OUTER - 30.0);
                            let cls = if i == active {
                                "wheel-wedge is-active"
                            } else {
                                "wheel-wedge"
                            };
                            rsx! {
                                path {
                                    key: "{i}",
                                    class: "{cls}",
                                    "data-i": "{i}",
                                    d: "{d}",
                                    onmouseenter: move |_| {
                                        // Pitfall 6 ordering: write wedge first.
                                        wheel.with_mut(|s| {
                                            s.active_wedge = WheelWedge::from_index(i);
                                        });
                                    },
                                    onclick: move |_| {
                                        // Pitfall 6 ordering: write wedge first, compute
                                        // target from the new value, set screen.
                                        wheel.with_mut(|s| {
                                            s.active_wedge = WheelWedge::from_index(i);
                                        });
                                        let target = WheelWedge::from_index(i).to_screen();
                                        active_screen.set(target);
                                    },
                                }
                            }
                        }
                    }
                }

                // ── Separators (N=10 lines, non-interactive) ──
                g { id: "wheel-seps", "pointer-events": "none",
                    for i in 0..N {
                        {
                            let ang_a = (i as f64) * STEP - STEP / 2.0;
                            let (sx, sy) = polar(ang_a, R_INNER + 8.0);
                            let (ex, ey) = polar(ang_a, R_OUTER - 30.0);
                            rsx! {
                                line {
                                    key: "{i}",
                                    x1: "{sx}", y1: "{sy}",
                                    x2: "{ex}", y2: "{ey}",
                                    stroke: "rgba(57,197,207,0.22)",
                                    "stroke-width": "1",
                                }
                            }
                        }
                    }
                }

                // ── Glyph + label texts (20 nodes: 10 wedges × 2 text per wedge) ──
                g { id: "wheel-text", "pointer-events": "none",
                    for i in 0..N {
                        {
                            let mid = (i as f64) * STEP;
                            let wedge = WheelWedge::from_index(i);
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
                            let gy5 = gy + 5.0;
                            let ly3 = ly + 3.0;
                            let glyph_text = wedge.glyph();
                            let label_text = wedge.label();
                            rsx! {
                                text {
                                    key: "{i}",
                                    x: "{gx}", y: "{gy5}",
                                    "text-anchor": "middle",
                                    "font-size": "15",
                                    fill: "rgba(57,197,207,0.65)",
                                    class: "{g_cls}",
                                    "data-i": "{i}",
                                    transform: "{g_xform}",
                                    "{glyph_text}"
                                }
                                text {
                                    x: "{lx}", y: "{ly3}",
                                    "text-anchor": "middle",
                                    "font-family": "JetBrains Mono, monospace",
                                    "font-size": "9",
                                    "font-weight": "700",
                                    "letter-spacing": "2",
                                    fill: "#9aa4ad",
                                    class: "{l_cls}",
                                    "data-i": "{i}",
                                    transform: "{l_xform}",
                                    "{label_text}"
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
                            fill: "none", stroke: "rgba(57,197,207,0.22)",
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
                                let target = {
                                    let s = wheel.read();
                                    s.active_wedge.to_screen()
                                };
                                active_screen.set(target);
                            },
                            circle {
                                id: "hub-bg",
                                cx: "0", cy: "0", r: "{R_INNER}",
                                fill: "url(#hub-grad)",
                                stroke: "rgba(57,197,207,0.55)",
                                "stroke-width": "1",
                            }
                            circle {
                                cx: "0", cy: "0", r: "{r_inner_minus_6}",
                                fill: "none", stroke: "rgba(57,197,207,0.18)",
                            }
                            circle {
                                cx: "0", cy: "0", r: "{r_inner_minus_14}",
                                fill: "none", stroke: "rgba(57,197,207,0.10)",
                                "stroke-dasharray": "2 3",
                            }
                            circle {
                                id: "hub-aura",
                                cx: "0", cy: "0", r: "{r_inner_minus_4}",
                                fill: "rgba(57,197,207,0.0)",
                            }

                            text {
                                id: "hub-glyph",
                                x: "0", y: "-22",
                                "text-anchor": "middle",
                                "font-family": "JetBrains Mono, monospace",
                                "font-size": "16",
                                fill: "#56d4dd",
                                "▶"
                            }
                            text {
                                id: "hub-label",
                                x: "0", y: "-2",
                                "text-anchor": "middle",
                                "font-family": "JetBrains Mono, monospace",
                                "font-size": "13",
                                "font-weight": "700",
                                fill: "#e6edf3",
                                "letter-spacing": "2.5",
                                "{hub_label_text}"
                            }
                            text {
                                id: "hub-sub",
                                x: "0", y: "14",
                                "text-anchor": "middle",
                                "font-family": "JetBrains Mono, monospace",
                                "font-size": "6.5",
                                fill: "#6e7681",
                                "letter-spacing": "3",
                                "{hub_sub_text}"
                            }
                            text {
                                id: "hub-cta",
                                x: "0", y: "30",
                                "text-anchor": "middle",
                                "font-family": "JetBrains Mono, monospace",
                                "font-size": "7",
                                "font-weight": "700",
                                fill: "#39c5cf",
                                "letter-spacing": "3",
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
                            fill: "#39c5cf",
                            id: "hub-arrow",
                            "pointer-events": "none",
                        }
                    }
                }

                // ── Resize ring (3 nested circles) + 4 orbit nodes ──
                {
                    let r_resize = R_OUTER + RING_GAP;
                    let r_orbit = R_OUTER + RING_GAP + 7.0;
                    let glow_width = RING_W + 6.0;
                    let neg_r_resize = -(R_OUTER + RING_GAP);
                    rsx! {
                        circle {
                            id: "resize-glow",
                            cx: "0", cy: "0", r: "{r_resize}",
                            fill: "none", stroke: "rgba(255,166,87,0.0)",
                            "stroke-width": "{glow_width}",
                            "pointer-events": "none",
                        }
                        circle {
                            id: "resize-orbit",
                            cx: "0", cy: "0", r: "{r_orbit}",
                            fill: "none", stroke: "rgba(255,166,87,0.30)",
                            "stroke-width": "1",
                            "stroke-dasharray": "1 6",
                            "pointer-events": "none",
                        }
                        circle {
                            id: "resize-ring",
                            class: "resize-ring",
                            cx: "0", cy: "0", r: "{r_resize}",
                            fill: "none", stroke: "rgba(255,166,87,0.55)",
                            "stroke-width": "{RING_W}",
                            "pointer-events": "stroke",
                            onpointerdown: move |e| {
                                if e.trigger_button() != Some(MouseButton::Primary) {
                                    return;
                                }
                                let c = e.client_coordinates();
                                let start_size = {
                                    let s = wheel.read();
                                    s.size
                                };
                                resizing.set(Some(ResizeState {
                                    start_client: (c.x, c.y),
                                    start_size,
                                    pointer_id: e.pointer_id(),
                                }));
                                tooltip_open.set(false);
                                e.stop_propagation();
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
                            circle { cx: "{r_resize}",     cy: "0",            r: "3", fill: "#ffa657" }
                            circle { cx: "{neg_r_resize}", cy: "0",            r: "3", fill: "#ffa657" }
                            circle { cx: "0",              cy: "{r_resize}",   r: "3", fill: "#ffa657" }
                            circle { cx: "0",              cy: "{neg_r_resize}", r: "3", fill: "#ffa657" }
                        }
                    }
                }
            }

            // ── Floating tooltip (Pattern 2 tooltip block) ──
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
        assert_eq!(R_INNER, 78.0);
        assert_eq!(R_LABEL, 148.0);
        assert_eq!(R_GLYPH, 118.0);
        assert_eq!(RING_GAP, 14.0);
        assert_eq!(RING_W, 4.0);
        assert_eq!(PAD, 22.0);
        assert_eq!(VB, 424.0);
        // CONTEXT D-10: 10 wedges per wheel-v2.js DEFAULT_SECTIONS.
        assert_eq!(N, 10);
        assert_eq!(STEP, 36.0);
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
        assert!(p.starts_with("M "), "wedge path should start with 'M ', got: {p}");
        assert!(p.contains(" A "), "wedge path should contain arc segments");
        assert!(p.contains(" L "), "wedge path should contain line segment");
        assert!(p.ends_with(" Z"), "wedge path should close with Z");
    }

    #[test]
    fn wedge_path_renders_all_ten_wedges() {
        // Smoke: every wedge index produces a non-empty path.
        for i in 0..N {
            let ang_a = (i as f64) * STEP - STEP / 2.0;
            let ang_b = ((i + 1) as f64) * STEP - STEP / 2.0;
            let p = wedge_path(ang_a, ang_b, R_INNER + 8.0, R_OUTER - 30.0);
            assert!(!p.is_empty(), "wedge {i} path is empty");
            assert!(p.starts_with("M "), "wedge {i} path malformed: {p}");
        }
    }
}

// Canonical 10-wedge order documented for grep lock-down (CONTEXT D-10):
// chat, agents, models, tools, skills, memory, sessions, providers, gateway, settings
