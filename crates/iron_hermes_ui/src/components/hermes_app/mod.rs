use dioxus::prelude::*;

/// Phase 26.2.1 root component for the new wheel-driven shell.
///
/// This is a deliberate stub: it provides a compilable mount target for
/// `app.rs` under `#[cfg(not(feature = "legacy-shell"))]` so the default
/// build links cleanly while Plans 03–08 incrementally fill in the real
/// composition (wheel menu, screen frames, theme controls, etc.).
///
/// Plans 03+ will replace the body of this component. Do not add logic here.
#[component]
pub fn HermesApp() -> Element {
    rsx! {
        div {
            class: "app",
            id: "app",
            "Phase 26.2.1 — shell mount point (HermesApp stub)"
        }
    }
}
