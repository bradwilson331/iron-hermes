use dioxus::prelude::*;

#[component]
pub fn ScreenMemory(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-memory",
            "data-screen-label": "06 Memory",
            div { class: "screen-placeholder",
                "Memory screen — stubbed by Plan 08"
            }
        }
    }
}
