use dioxus::prelude::*;

#[component]
pub fn ScreenGateway(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-gateway",
            "data-screen-label": "10 Gateway",
            div { class: "screen-placeholder",
                "Gateway screen — stubbed by Plan 08"
            }
        }
    }
}
