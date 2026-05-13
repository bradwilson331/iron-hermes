//! Soul screen — ported from `app.html` `<section id="screen-soul">`
//! (lines 885-957). The prototype shows a single SOUL.md editor; the
//! `stub_data::soul_personas()` factory surfaces the multi-persona
//! picker that 26.2.6 will wire. Pure visual stub (D-04) — zero
//! server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{soul_personas, SoulPersonaStub};

const SOUL_BODY: &str = "# Hermes — default profile\n\n## Identity\nYou are Hermes, an operator-aligned intelligence shell. Calm, direct, technically literate.\nSpeak in short paragraphs. Lead with the recommendation, then the evidence.\n\n## Voice\n- No hedging without justification.\n- No filler (\"As an AI…\", \"Certainly!\", \"Let me help you with that…\").\n- Cite when claims are non-obvious.\n- When uncertain, say so in one sentence and proceed.\n\n## Behavior\n- Always run tools when the question is empirical.\n- Always show progress when a tool chain takes more than ~2 seconds.\n- Refuse only when continuing would violate operator policy — and say which clause.\n\n## Style\n- Monospace for code, paths, identifiers, IPs.\n- ISO-8601 timestamps. SI units. UTC.\n- Tables for ≥3 comparable rows.";

#[component]
pub fn ScreenSoul(is_active: bool) -> Element {
    let personas = soul_personas();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-soul",
            "data-screen-label": "07 Soul",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 07" }
                    h1 { class: "screen-title", "Soul" }
                    p { class: "screen-sub",
                        "The persona for the active profile — "
                        code { style: "color:var(--teal)", "SOUL.md" }
                        " for "
                        code { style: "color:var(--teal)", "default" }
                        ". Edits apply on next message."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "REVERT" }
                    button { class: "btn btn--sm", "▓ SAVE" }
                }
            }

            // Persona picker (multi-profile surface for 26.2.6).
            div { class: "tabs",
                for p in personas.iter() {
                    button {
                        key: "{p.id}",
                        class: "tab",
                        class: if p.active { "is-active" },
                        "data-persona-id": "{p.id}",
                        "{p.label}"
                    }
                }
            }

            div { class: "soul-grid",
                div { class: "soul-editor",
                    div { style: "display:flex;justify-content:space-between;align-items:center;",
                        div { class: "panel-title", "SOUL.md" }
                        div { style: "display:flex;gap:8px;font-size:10px;color:var(--gray);letter-spacing:0.12em;",
                            span { "312 LINES" }
                            span { "·" }
                            span { "8.4 KB" }
                            span { "·" }
                            span { style: "color:var(--green)", "SAVED" }
                        }
                    }
                    textarea {
                        spellcheck: "false",
                        "{SOUL_BODY}"
                    }
                }

                div { class: "soul-preview",
                    div { class: "panel-title", "Active Personas" }
                    div { class: "soul-preview-body",
                        for p in personas.iter() {
                            PersonaCard { key: "{p.id}", persona: p.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PersonaCard(persona: SoulPersonaStub) -> Element {
    rsx! {
        div {
            class: "card",
            class: if persona.active { "is-active" },
            "data-persona-id": "{persona.id}",
            div { class: "card-head",
                div { style: "flex:1",
                    div { class: "card-title", "{persona.label}" }
                }
                if persona.active {
                    span { class: "pill teal", "ACTIVE" }
                }
            }
            div { class: "card-body", "{persona.blurb}" }
        }
    }
}
