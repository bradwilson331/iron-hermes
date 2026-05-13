use dioxus::prelude::*;

#[component]
pub fn ScreenSessions(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-sessions",
            "data-screen-label": "02 Sessions",
            div { class: "screen-placeholder",
                "Sessions screen — wired by Plan 07"
            }
        }
    }
}
