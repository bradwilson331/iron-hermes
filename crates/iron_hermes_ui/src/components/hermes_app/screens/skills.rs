use dioxus::prelude::*;

#[component]
pub fn ScreenSkills(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-skills",
            "data-screen-label": "04 Skills",
            div { class: "screen-placeholder",
                "Skills screen — stubbed by Plan 08"
            }
        }
    }
}
