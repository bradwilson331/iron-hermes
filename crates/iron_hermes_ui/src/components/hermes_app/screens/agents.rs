use dioxus::prelude::*;

#[component]
pub fn ScreenAgents(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-agents",
            "data-screen-label": "03 Agents",
            div { class: "screen-placeholder",
                "Agents screen — stubbed by Plan 08"
            }
        }
    }
}
