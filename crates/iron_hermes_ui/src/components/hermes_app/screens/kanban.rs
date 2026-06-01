//! Plan 04 placeholder screen for `Screen::Kanban`.
//!
//! Phase 36.3.7.11 Plan 04 ships the navigation surface (wheel-nav wedge +
//! Agents-page toolbar button) BEFORE the live board lands in Plan 01.
//! This placeholder renders a minimal `<section>` so the screen-router fan-out
//! has a target to mount and the user sees a calm screen header rather than a
//! blank panel when they click `KANBAN BOARD →` or the wheel's `KANBAN` wedge.
//!
//! Plan 01 replaces the body of `ScreenKanban` with the real KanbanBoard +
//! drawer + WS-live wiring. The screen module name and component signature
//! (`pub fn ScreenKanban(is_active: bool) -> Element`) are stable.

use dioxus::prelude::*;

#[component]
pub fn ScreenKanban(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-kanban",
            "data-screen-label": "11 Kanban",

            // Header — mirrors the existing screen header pattern from
            // screens/agents.rs lines 334-342 so the visual treatment is
            // consistent until Plan 01 replaces the body.
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 11" }
                    h1 { class: "screen-title", "Kanban" }
                    p { class: "screen-sub",
                        "Task board — live dashboard coming soon. Plan 01 wires the live KanbanBoard, drawer, and WebSocket tail."
                    }
                }
            }
        }
    }
}
