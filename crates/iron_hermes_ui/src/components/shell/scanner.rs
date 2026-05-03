use dioxus::prelude::*;

/// Knight-rider scanner — 10 sibling spans whose color animates via pure CSS
/// `@keyframes wh-scanner-tick` defined in `assets/scanner-anim.css`. The CSS
/// staggers `animation-delay` per `:nth-child` so the lit cell appears to bounce
/// across the row.
///
/// Phase 3 hardcodes `active = true` at every call site (per CONTEXT D-08); the
/// Phase 4 work pulses `active` for ~1400ms post-submission.
///
/// Port of `warp2ironhermes/project/app/shell.jsx` lines 5-27 per CONTEXT D-01.
/// React's setInterval-driven class assignment is replaced by pure CSS per D-08.
#[component]
pub fn Scanner(active: bool) -> Element {
    rsx! {
        span {
            class: "wh-scanner",
            class: if active { "is-active" },
            "aria-hidden": "true",
            for i in 0..10 {
                span { key: "{i}", "░" }
            }
        }
    }
}
