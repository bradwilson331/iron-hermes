//! Models screen — ported from `app.html` `<section id="screen-models">`
//! (lines 691-794). Renders models from `stub_data::models()` grouped
//! by family. Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{models, ModelStub};

#[component]
pub fn ScreenModels(is_active: bool) -> Element {
    let models_list = models();

    // Compute the unique family ordering as it appears in the data so we
    // can render section labels in source order without an extra signal.
    let mut families: Vec<&'static str> = Vec::new();
    for m in models_list.iter() {
        if !families.contains(&m.family) {
            families.push(m.family);
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

            for family in families.iter() {
                {
                    let family = *family;
                    let rows: Vec<ModelStub> = models_list
                        .iter()
                        .filter(|m| m.family == family)
                        .cloned()
                        .collect();
                    let count = rows.len();
                    rsx! {
                        div { key: "{family}", class: "model-family-group",
                            div { class: "section-label",
                                "{family} "
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

#[component]
fn ModelCard(model: ModelStub) -> Element {
    let is_default = model.status == "DEFAULT";
    rsx! {
        div {
            class: "card",
            class: if is_default { "is-active" },

            div { class: "card-head",
                div { class: "card-icon", "◉" }
                div { style: "flex:1",
                    div { class: "card-title", "{model.id}" }
                    div { class: "card-meta", "{model.family} · {model.context_window} context" }
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
                button { class: "btn btn--ghost btn--sm", "EDIT" }
            }
        }
    }
}
