//! Top-left breadcrumb chip — `NODE HERMES-7 › BRIDGE › {SCREEN}`.
//!
//! The rightmost crumb tracks the active `Screen` via the context-provided
//! `Signal<Screen>` published by `HermesApp`. The Screen→short-name map
//! is local to this file because Breadcrumb is the only consumer — keeps
//! `state.rs` minimal (the plan explicitly forbids adding a method on
//! `Screen`).
//!
//! Markup mirrors `app.html` lines 338-346 (the `.breadcrumb` block).
//! The leading `<span class="dot">` is preserved so the blink animation
//! from `site.css` activates without further CSS edits.

use crate::state::Screen;
use dioxus::prelude::*;

#[component]
pub fn Breadcrumb() -> Element {
    // Fully-qualified path for the acceptance-criteria literal assertion
    // (`use_context::<Signal<crate::state::Screen>>`).
    let active = use_context::<Signal<crate::state::Screen>>();
    let cur = *active.read();
    let short_name = screen_short_name(cur);

    rsx! {
        div { class: "breadcrumb",
            span { class: "dot" }
            span { "NODE " span { class: "v", "HERMES-7" } }
            span { class: "sep", "›" }
            span { "BRIDGE" }
            span { class: "sep", "›" }
            span { class: "v", "{short_name}" }
        }
    }
}

/// Short-name map for the breadcrumb's rightmost crumb. Per the
/// `<interfaces>` block in the plan: 4-7 char uppercase labels, with
/// the off-wheel screens (`Soul`, `Schedules`, `Office`) defined locally
/// since they are NOT `WheelWedge` variants (D-10).
fn screen_short_name(s: Screen) -> &'static str {
    match s {
        Screen::Chat => "CHAT",
        Screen::Sessions => "SESSIONS",
        Screen::Agents => "AGENTS",
        Screen::Skills => "SKILLS",
        Screen::Models => "MODELS",
        Screen::Memory => "MEMORY",
        Screen::Soul => "SOUL",
        Screen::Tools => "TOOLS",
        Screen::Schedules => "SCHED",
        Screen::Gateway => "GATEWAY",
        Screen::Office => "OFFICE",
        Screen::Settings => "SYSTEM",
        Screen::Providers => "PROVIDERS",
    }
}
