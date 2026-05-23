//! Tools screen — ported from `app.html` `<section id="screen-tools">`
//! (lines 960-1071). Renders tool cards from the live `api::list_tools()`
//! server fn via `use_server_future` suspension (Phase 26.7 Plan 01).
//! Loading state: empty `.grid`. Error state: inline danger-colored row.

use dioxus::prelude::*;

#[component]
pub fn ScreenTools(is_active: bool) -> Element {
    // Fetch the live tool catalog. The `?` suspends the component until the
    // first resolution (Dioxus 0.7 use_server_future pattern — sessions.rs:33).
    let tools_resource = use_server_future(crate::server::api::list_tools)?;

    // Extract data before the rsx! block — signal borrow discipline per
    // iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX closure).
    let tools_list: Vec<crate::server::api::ToolInfo> = match tools_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };

    // Error flag computed from a second resource() call — still before rsx!
    // so no signal borrow is held inside the macro (PATTERNS.md "Gotchas").
    let load_error = matches!(tools_resource(), Some(Err(_)));

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
                    // ENABLE ALL / DISABLE ALL: static affordance this phase (no onclick — D-12 / UI-SPEC §"Toggle buttons")
                    button { class: "btn btn--ghost btn--sm", "ENABLE ALL" }
                    button { class: "btn btn--ghost btn--sm", "DISABLE ALL" }
                }
            }

            div { class: "grid",
                if load_error {
                    // UI-SPEC §"Copywriting Contract" — verbatim copy, em dash, lowercase after.
                    div {
                        style: "color:var(--danger);font-size:var(--fs-12);",
                        "Could not load tools — check server connection."
                    }
                } else {
                    for tool in tools_list.iter() {
                        ToolCard { key: "{tool.name}", tool: tool.clone() }
                    }
                }
            }
        }
    }
}

/// One card in the tools grid. Takes `ToolInfo` from the live server fn.
/// `PartialEq + Clone` are satisfied automatically because `ToolInfo` derives both.
///
/// Per UI-SPEC §"ToolCard": the `.tgl` toggle renders in static off-state —
/// no `onclick` handler this phase (enable/disable writes are out of scope, D-12).
#[component]
fn ToolCard(tool: crate::server::api::ToolInfo) -> Element {
    rsx! {
        div {
            class: "tool-card",
            div { class: "tool-top",
                div { class: "tool-icon", "⊕" }
                // Static off-state toggle — no "on" class, no onclick (D-12 / threat model T-26.7-01-03).
                div { class: "tgl", "data-tgl": "true" }
            }
            div { class: "tool-name", "{tool.name}" }
            // stub .summary renamed to .description in ToolInfo (RESEARCH §"Type mapping")
            div { class: "tool-desc", "{tool.description}" }
        }
    }
}
