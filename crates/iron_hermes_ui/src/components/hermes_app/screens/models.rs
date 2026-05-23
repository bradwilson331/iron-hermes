//! Models screen — wired to the live `api::list_models()` server fn
//! (Phase 26.7 Plan 04 / D-10, R-1).
//!
//! Renders the full ModelRegistry catalog grouped by inferred family.
//! The configured default model (state.config.model.default) renders with
//! status `DEFAULT`; all others show `AVAILABLE`. Family grouping uses
//! owned `Vec<String>` (not `Vec<&'static str>`) per PATTERNS.md gotcha.
//! Context window formatted as human-readable string ("200k", "1M", etc.).

use dioxus::prelude::*;

#[component]
pub fn ScreenModels(is_active: bool) -> Element {
    let models_resource = use_server_future(crate::server::api::list_models)?;

    // Extract data and error flag BEFORE rsx! — signal borrow discipline
    // per iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let models_list: Vec<crate::server::api::ModelInfo> = match models_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let load_error = matches!(models_resource(), Some(Err(_)));

    // Family dedup loop — owned Vec<String>, source order preserved.
    // &'static str would fail because ModelInfo.family is a String.
    let mut families: Vec<String> = Vec::new();
    for m in models_list.iter() {
        if !families.contains(&m.family) {
            families.push(m.family.clone());
        }
    }

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-models",
            "data-screen-label": "05 Models",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 05" }
                    h1 { class: "screen-title", "Models" }
                    p { class: "screen-sub",
                        "Saved language-model configurations grouped by provider. Each agent binds to one model at a time."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--sm", "+ NEW CONFIG" }
                }
            }

            if load_error {
                div {
                    style: "color:var(--danger);font-size:var(--fs-12);",
                    "Could not load models — check server connection."
                }
            } else {
                for family in families.iter() {
                    {
                        // Snapshot rows for this family — owned Vec, no borrow into RSX.
                        let family_name = family.clone();
                        let rows: Vec<crate::server::api::ModelInfo> = models_list
                            .iter()
                            .filter(|m| m.family == family_name)
                            .cloned()
                            .collect();
                        let count = rows.len();
                        rsx! {
                            div { key: "{family_name}", class: "model-family-group",
                                div { class: "section-label",
                                    "{family_name} "
                                    span { class: "count", "· {count} configs" }
                                }
                                div { class: "grid wide",
                                    for m in rows.iter() {
                                        ModelCard { key: "{m.id}", model: m.clone() }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ModelCard(model: crate::server::api::ModelInfo) -> Element {
    let is_default = model.status == "DEFAULT";
    rsx! {
        div {
            class: "card",
            class: if is_default { "is-active" },
            div { class: "card-head",
                div { class: "card-icon", "◉" }
                div { style: "flex:1",
                    div { class: "card-title", "{model.id}" }
                    div { class: "card-meta",
                        "{model.family} · {model.context_window} context"
                    }
                }
                if is_default {
                    span { class: "pill teal", "{model.status}" }
                }
            }
            div { class: "card-footer",
                div { style: "display:flex;gap:14px;font-size:10px;color:var(--gray);letter-spacing:0.06em;",
                    span { "CTX " span { style: "color:var(--teal);font-weight:700", "{model.context_window}" } }
                    span { "STATE " span { style: "color:var(--teal);font-weight:700", "{model.status}" } }
                }
                button { class: "btn btn--ghost btn--sm", "EDIT" }   // no onclick — out of scope
            }
        }
    }
}
