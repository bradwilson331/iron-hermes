use dioxus::prelude::*;

#[component]
pub fn ScreenProviders(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-providers",
            "data-screen-label": "13 Providers",
            div { class: "screen-placeholder",
                "Providers screen — stubbed by Plan 08"
            }
        }
    }
}
