use dioxus::prelude::*;

#[component]
pub fn ScreenModels(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-models",
            "data-screen-label": "05 Models",
            div { class: "screen-placeholder",
                "Models screen — stubbed by Plan 08"
            }
        }
    }
}
