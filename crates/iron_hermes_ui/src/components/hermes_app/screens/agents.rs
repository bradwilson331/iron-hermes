//! Agents screen — ported from `app.html` `<section id="screen-agents">`
//! (lines 501-591). Renders 4 agent cards from `stub_data::agents()` per
//! Plan 26.2.1-08. Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{agents, AgentStub};

#[component]
pub fn ScreenAgents(is_active: bool) -> Element {
    let agents_list = agents();
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-agents",
            "data-screen-label": "03 Agents",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 03" }
                    h1 { class: "screen-title", "Agents" }
                    p { class: "screen-sub",
                        "Each profile is an isolated Hermes workspace with its own config, memory, skill set, and persona."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--sm", "+ NEW AGENT" }
                }
            }

            div { class: "grid wide",
                for a in agents_list.iter() {
                    AgentCard { key: "{a.name}", agent: a.clone() }
                }
            }
        }
    }
}

#[component]
fn AgentCard(agent: AgentStub) -> Element {
    // Per CSS in components.css, "card.is-active" highlights the active
    // agent — match the prototype where `default` carries `is-active`.
    let is_active_card = agent.status == "ACTIVE";
    rsx! {
        div {
            class: "card",
            class: if is_active_card { "is-active" },

            div { class: "card-head",
                div {
                    class: "avatar {agent.avatar_color}",
                    "{agent.avatar_letter}"
                }
                div { style: "flex:1",
                    div { class: "card-title", "{agent.name}" }
                    div { class: "card-meta", "{agent.model}" }
                }
                if is_active_card {
                    span { class: "pill teal", "{agent.status}" }
                }
            }
            div { class: "card-body", "{agent.summary}" }
            div { class: "card-footer",
                div { style: "display:flex;gap:14px;font-size:10px;color:var(--gray);letter-spacing:0.06em;",
                    span {
                        span { style: "color:var(--teal);font-weight:700",
                            "{agent.skills_count}"
                        }
                        " SKILLS"
                    }
                    span { "GATEWAY " }
                }
                button {
                    class: if is_active_card { "btn btn--sm" } else { "btn btn--ghost btn--sm" },
                    "▓ CHAT"
                }
            }
        }
    }
}
