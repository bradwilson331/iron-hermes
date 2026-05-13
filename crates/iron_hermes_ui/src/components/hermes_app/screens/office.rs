use dioxus::prelude::*;

#[component]
pub fn ScreenOffice(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-office",
            "data-screen-label": "11 Office",
            div { class: "screen-placeholder",
                "Office screen — stubbed by Plan 08"
            }
        }
    }
}
