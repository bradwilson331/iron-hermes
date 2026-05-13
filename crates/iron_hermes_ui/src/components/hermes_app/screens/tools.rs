//! Tools screen — ported from `app.html` `<section id="screen-tools">`
//! (lines 960-1071). Renders 12 tool cards from `stub_data::tools()` in
//! a flat grid (the prototype is flat — toolset grouping happens via
//! the `group` field but the prototype does not section-label them).
//! Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{tools, ToolStub};

#[component]
pub fn ScreenTools(is_active: bool) -> Element {
    let tools_list = tools();
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-tools",
            "data-screen-label": "08 Tools",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 08" }
                    h1 { class: "screen-title", "Tools" }
                    p { class: "screen-sub",
                        "Enable or disable individual toolsets available to the agent during conversations."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "ENABLE ALL" }
                    button { class: "btn btn--ghost btn--sm", "DISABLE ALL" }
                }
            }

            div { class: "grid",
                for t in tools_list.iter() {
                    ToolCard { key: "{t.name}", tool: t.clone() }
                }
            }
        }
    }
}

#[component]
fn ToolCard(tool: ToolStub) -> Element {
    let on = tool.status == "ENABLED";
    rsx! {
        div {
            class: "tool-card",
            class: if on { "on" },
            "data-tool-group": "{tool.group}",
            div { class: "tool-top",
                div { class: "tool-icon", "⊕" }
                div {
                    class: if on { "tgl on" } else { "tgl" },
                    "data-tgl": "true",
                }
            }
            div { class: "tool-name", "{tool.name}" }
            div { class: "tool-desc", "{tool.summary}" }
        }
    }
}
