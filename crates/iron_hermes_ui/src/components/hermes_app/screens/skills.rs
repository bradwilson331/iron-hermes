//! Skills screen — wired to the live `api::list_skills()` server fn
//! (Phase 26.7 Plan 03 / D-09, R-1, R-4).
//!
//! Renders the full SkillRegistry catalog with per-skill enabled state
//! derived from `runtime_bundle.active_skills`. Dynamic count in sub-copy
//! and ALL tab. SkillCard reflects enabled state via `.tgl on` class;
//! no onclick handler (write ops deferred per D-09 + deferred list).

use dioxus::prelude::*;

#[component]
pub fn ScreenSkills(is_active: bool) -> Element {
    let skills_resource = use_server_future(crate::server::api::list_skills)?;

    // Extract data and error flag BEFORE rsx! — signal borrow discipline
    // per iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let skills_list: Vec<crate::server::api::SkillInfo> = match skills_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let load_error = matches!(skills_resource(), Some(Err(_)));
    let count = skills_list.len();
    let enabled_count = skills_list.iter().filter(|s| s.enabled).count();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-skills",
            "data-screen-label": "04 Skills",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 04" }
                    h1 { class: "screen-title", "Skills" }
                    p { class: "screen-sub",
                        "{count} loaded · {enabled_count} enabled for "
                        code { style: "color:var(--teal)", "default" }
                        "."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "⇣ IMPORT" }
                    button { class: "btn btn--sm", "+ NEW SKILL" }
                }
            }

            div { class: "search",
                span { class: "search-glyph", "⌕" }
                input { placeholder: "Search skills, tags, capabilities…" }
            }

            div { class: "tabs",
                button { class: "tab is-active", "ALL · {count}" }
                button { class: "tab", "BUNDLED · 87" }
                button { class: "tab", "INSTALLED · 55" }
                button { class: "tab", "ENABLED · 57" }
                button { class: "tab", "UPDATES · 3" }
            }

            div { class: "grid",
                if load_error {
                    div {
                        style: "color:var(--danger);font-size:var(--fs-12);",
                        "Could not load skills — check server connection."
                    }
                } else {
                    for skill in skills_list.iter() {
                        SkillCard { key: "{skill.name}", skill: skill.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn SkillCard(skill: crate::server::api::SkillInfo) -> Element {
    rsx! {
        div {
            class: "card",
            class: if skill.enabled { "is-active" },
            div { class: "card-head",
                div {
                    class: if skill.enabled { "card-icon" } else { "card-icon gray" },
                    "⊕"
                }
                div { style: "flex:1",
                    div { class: "card-title", "{skill.name}" }
                    div { class: "card-meta", "{skill.category}" }
                }
                div {
                    class: if skill.enabled { "tgl on" } else { "tgl" },
                    "data-tgl": "true",
                    // No onclick — write ops out of scope (D-09 + deferred list).
                }
            }
            div { class: "card-body", "{skill.description}" }
        }
    }
}
