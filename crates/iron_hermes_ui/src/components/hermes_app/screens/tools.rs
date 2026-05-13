use dioxus::prelude::*;

#[component]
pub fn ScreenTools(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-tools",
            "data-screen-label": "08 Tools",
            div { class: "screen-placeholder",
                "Tools screen — stubbed by Plan 08"
            }
        }
    }
}
