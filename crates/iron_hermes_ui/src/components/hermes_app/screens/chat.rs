use dioxus::prelude::*;

#[component]
pub fn ScreenChat(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-chat",
            "data-screen-label": "01 Chat",
            div { class: "screen-placeholder",
                "Chat screen — wired by Plan 06"
            }
        }
    }
}
