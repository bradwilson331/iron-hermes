//! Skills screen — wired to the live `api::list_skills()` server fn
//! (Phase 26.7 Plan 03 / D-09, R-1, R-4).
//!
//! Renders the full SkillRegistry catalog with per-skill enabled state
//! derived from `runtime_bundle.active_skills`. Dynamic count in sub-copy
//! and ALL tab. SkillCard reflects enabled state via `.tgl on` class;
//! toggle is wired via optimistic HashMap signal (Phase 26.7.3 Plan 03).

use dioxus::prelude::*;
use std::collections::HashMap;
use ironhermes_core::skills::{tab_predicate, search_matches};

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

    // Tab and search signals — let mut so event handlers can .set()
    let mut active_tab = use_signal(|| "all");
    let mut search_query = use_signal(|| String::new());

    // Live tab counts — computed from skills_list (server-sourced per Pitfall 6)
    let count_all = skills_list.len();
    let count_bundled = skills_list.iter().filter(|s| s.category == "bundled").count();
    let count_installed = skills_list.iter().filter(|s| s.category != "bundled").count();
    let count_enabled = skills_list.iter().filter(|s| s.enabled).count();

    // Extract owned values BEFORE rsx! — no GenerationalRef crossing the macro boundary
    let tab_val = active_tab();     // &'static str — Copy
    let _query_val = search_query(); // String — Clone (declared for completeness; used via signal in memo)

    // Clone skills_list before use_memo — closure must own its data ('static capture, Pitfall 5)
    let skills_for_memo = skills_list.clone();
    let filtered_skills = use_memo(move || {
        let tab = active_tab();
        let query = search_query();
        skills_for_memo
            .iter()
            .filter(|s| tab_predicate(&s.category, s.enabled, tab) && search_matches(&s.name, &s.description, &query))
            .cloned()
            .collect::<Vec<crate::server::api::SkillInfo>>()
    });

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
                        "{count_all} loaded · {count_enabled} enabled for "
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
                input {
                    placeholder: "Search skills, tags, capabilities…",
                    oninput: move |e| search_query.set(e.value()),
                }
            }

            div { class: "tabs",
                button {
                    class: if tab_val == "all" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("all"),
                    "ALL · {count_all}"
                }
                button {
                    class: if tab_val == "bundled" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("bundled"),
                    "BUNDLED · {count_bundled}"
                }
                button {
                    class: if tab_val == "installed" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("installed"),
                    "INSTALLED · {count_installed}"
                }
                button {
                    class: if tab_val == "enabled" { "tab is-active" } else { "tab" },
                    onclick: move |_| active_tab.set("enabled"),
                    "ENABLED · {count_enabled}"
                }
                button {
                    class: "tab",
                    style: "opacity:0.5; pointer-events:none;",
                    disabled: true,
                    "UPDATES · 0"
                }
            }

            div { class: "grid",
                if load_error {
                    div {
                        style: "color:var(--danger);font-size:var(--fs-12);",
                        "Could not load skills — check server connection."
                    }
                } else if filtered_skills.read().is_empty() && !skills_list.is_empty() {
                    div {
                        style: "color:var(--gray);font-size:var(--fs-12);",
                        "No skills match."
                    }
                } else {
                    for skill in filtered_skills.read().iter() {
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
                    // No onclick — wired in Task 2 (Phase 26.7.3 Plan 03).
                }
            }
            div { class: "card-body", "{skill.description}" }
        }
    }
}
