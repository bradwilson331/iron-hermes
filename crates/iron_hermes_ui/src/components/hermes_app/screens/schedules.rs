use dioxus::prelude::*;

#[component]
pub fn ScreenSchedules(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-schedules",
            "data-screen-label": "09 Schedules",
            div { class: "screen-placeholder",
                "Schedules screen — stubbed by Plan 08"
            }
        }
    }
}
