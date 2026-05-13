//! Top-right `.sys-meta` chrome — BUILD / UPTIME / OP fields.
//!
//! Static placeholders for Phase 26.2.1 MVP. D-03 explicitly scopes
//! Settings (and by extension the system-meta surface) to read-only;
//! there is no live uptime / op-status feed in 26.2.1, so the values
//! are hardcoded literals.
//!
//! Markup mirrors `app.html` lines 348-354 (the `.sys-meta` block).

use dioxus::prelude::*;

#[component]
pub fn SysMeta() -> Element {
    rsx! {
        div { class: "sys-meta",
            span { "BUILD " span { class: "v", "1.0.0" } }
            span { "·" }
            span { "UPTIME " span { class: "v", "00:00:00" } }
            span { "·" }
            span { "OP " span { class: "v", "READY" } }
        }
    }
}
