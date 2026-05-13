//! Skills screen — ported from `app.html` `<section id="screen-skills">`
//! (lines 594-688). Renders 6 skill cards from `stub_data::skills()`.
//! Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{skills, SkillStub};

#[component]
pub fn ScreenSkills(is_active: bool) -> Element {
    let skills_list = skills();
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
                        "Composable capabilities the active agent can invoke. 142 loaded · 57 enabled for "
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
                button { class: "tab is-active", "ALL · 142" }
                button { class: "tab", "BUNDLED · 87" }
                button { class: "tab", "INSTALLED · 55" }
                button { class: "tab", "ENABLED · 57" }
                button { class: "tab", "UPDATES · 3" }
            }

            div { class: "grid",
                for s in skills_list.iter() {
                    SkillCard { key: "{s.name}", skill: s.clone() }
                }
            }
        }
    }
}

#[component]
fn SkillCard(skill: SkillStub) -> Element {
    let enabled = skill.status == "ENABLED";
    rsx! {
        div {
            class: "card",
            class: if enabled { "is-active" },

            div { class: "card-head",
                div {
                    class: if enabled { "card-icon" } else { "card-icon gray" },
                    "⊕"
                }
                div { style: "flex:1",
                    div { class: "card-title", "{skill.name}" }
                    div { class: "card-meta", "{skill.version} · {skill.category}" }
                }
                div {
                    class: if enabled { "tgl on" } else { "tgl" },
                    "data-tgl": "true",
                }
            }
            div { class: "card-body", "{skill.summary}" }
        }
    }
}
