use dioxus::prelude::*;
use crate::state::{Message, ShellSettings};
use super::sigil::Sigil;
use super::tool_call::ToolCall;

/// AgentPanel — right-side `.wh-side` agent panel: Sigil + HERMES title +
/// personality pill + scrollable message list.
///
/// Phase 4 (per CONTEXT D-02): the `personality: String` prop is REMOVED.
/// Personality is now read via `use_context::<ShellSettings>()` so any
/// `/personality` palette pick (Plan 04-05) reactively re-renders this panel
/// without parent prop-drilling (KBD-06 reactivity).
///
/// Phase 4 (per CONTEXT D-01): `messages` is now `ReadSignal<Vec<Message>>`
/// so writes in `mocks::run_agent_steps` (Plan 04-03) trigger re-render here.
///
/// `aside` semantics + 360px width preserved from Phase 3. `cursor: default`
/// on the personality pill kept (Phase 4 doesn't add a click handler;
/// TweaksPanel in Phase 5 will).
#[component]
pub fn AgentPanel(messages: ReadSignal<Vec<Message>>) -> Element {
    let settings = use_context::<ShellSettings>();
    let personality = settings.personality.read().label();

    rsx! {
        aside { class: "wh-side",
            div { class: "wh-side-head",
                Sigil { size: 20_u16 }
                span { class: "wh-side-title", "HERMES" }
                span {
                    class: "wh-personality",
                    style: "cursor: default;",
                    "/{personality}"
                }
            }
            div { class: "wh-side-scroll",
                for (i, m) in messages.read().iter().enumerate() {
                    div {
                        key: "{i}",
                        class: "wh-msg",
                        class: if m.who == "user" { "is-user" } else { "is-hermes" },
                        div { class: "wh-msg-meta",
                            b { if m.who == "user" { "You" } else { "Hermes" } }
                            span { "{m.time}" }
                        }
                        if let Some(tool) = &m.tool {
                            ToolCall {
                                name: tool.name.clone(),
                                args_summary: tool.args_summary.clone(),
                                status: tool.status.clone(),
                            }
                        } else {
                            div { class: "wh-msg-body", "{m.body}" }
                        }
                    }
                }
            }
        }
    }
}
