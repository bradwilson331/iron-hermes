//! Phase 26.2.1 Plan 04 — Wheel-rail docked variant.
//!
//! Renders the `aside.wheel-rail` panel beneath the wheel. Visibility is
//! owned entirely by CSS — `body.has-rail.on-chat #wheel-rail` reveals it
//! (`wheel.css` from Plan 01; Plan 05 toggles `body.has-rail` from
//! `UiPrefs.rail`). This component just emits the markup unconditionally
//! and lets the cascade decide whether to display.
//!
//! Position is computed declaratively from `Signal<WheelState>` per RESEARCH
//! Pitfall 9 — no ResizeObserver / MutationObserver needed. The rail
//! repositions automatically whenever WheelState changes (drag, resize,
//! hydration restore) because the rsx! re-renders on signal write.

use crate::state::WheelState;
use dioxus::prelude::*;

/// Docked wheel-rail variant. Always mounted; CSS controls visibility via
/// `body.has-rail.on-chat #wheel-rail { display: flex !important; }` rules
/// in `wheel.css`. Plan 05 toggles the body classes from `UiPrefs`.
///
/// Content is intentionally minimal (D-17 only requires the layout primitive
/// + reachability — no wired chat sidebar yet). The single `rail-row`
/// includes a status pill so the rail is visually identifiable when its
/// CSS becomes active.
#[component]
pub fn WheelRail() -> Element {
    let wheel_state = use_context::<Signal<WheelState>>();

    // Read position + size into locals, drop the borrow before formatting
    // the style string (clippy.toml signal-borrow rule).
    let (px, py, size) = {
        let s = wheel_state.read();
        (s.position.0, s.position.1, s.size)
    };
    // RESEARCH Pitfall 9: rail-top = wheel.top + wheel.size + 18.0
    let left = px;
    let top = py + size + 18.0;
    let width = size.max(280.0);
    let style = format!("left: {left}px; top: {top}px; width: {width}px;");

    rsx! {
        aside {
            class: "wheel-rail",
            id: "wheel-rail",
            style: "{style}",
            "aria-hidden": "true",
            div {
                class: "rail-row",
                span { class: "rail-pill", "RAIL" }
                span { class: "rail-status", "DOCKED" }
            }
        }
    }
}
