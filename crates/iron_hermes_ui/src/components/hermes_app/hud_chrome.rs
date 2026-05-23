//! HUD chrome — the always-visible decorative shell.
//!
//! Renders the two full-viewport overlays (`hud-grid`, `hud-vignette`)
//! plus the four corner brackets. All elements are `aria-hidden="true"`
//! because they are pure decoration. CSS for every class lives in
//! `assets/site.css` (copied verbatim from the design-system bundle by
//! Plan 01).
//!
//! Body-class effects from Plan 05 (`body.no-breadcrumb`, etc.) toggle
//! visibility via CSS — these elements are always mounted, so the
//! tweaks panel can hide / reveal them with zero DOM churn.

use dioxus::prelude::*;

#[component]
pub fn HudChrome() -> Element {
    rsx! {
        div { class: "hud-grid", "aria-hidden": "true" }
        div { class: "hud-vignette", "aria-hidden": "true" }
        div { class: "corner tl", "aria-hidden": "true" }
        div { class: "corner tr", "aria-hidden": "true" }
        div { class: "corner bl", "aria-hidden": "true" }
        div { class: "corner br", "aria-hidden": "true" }
    }
}
