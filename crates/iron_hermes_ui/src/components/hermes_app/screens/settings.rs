use dioxus::prelude::*;

#[component]
pub fn ScreenSettings(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-settings",
            "data-screen-label": "12 Settings",
            div { class: "screen-placeholder",
                "Settings screen — wired by Plan 07"
            }
        }
    }
}
