use dioxus::prelude::*;

#[component]
pub fn ScreenSoul(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-soul",
            "data-screen-label": "07 Soul",
            div { class: "screen-placeholder",
                "Soul screen — stubbed by Plan 08"
            }
        }
    }
}
