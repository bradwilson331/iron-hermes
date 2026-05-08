use super::sigil::Sigil;
use super::tool_call::ToolCall;
use crate::state::{Message, ShellSettings, TokenBudget};
use dioxus::prelude::*;

#[component]
pub fn AgentPanel(
    messages: ReadSignal<Vec<Message>>,
    active_side_tab: ReadSignal<usize>,
    on_side_tab_click: EventHandler<usize>,
    session_id: ReadSignal<String>,
    token_budget: ReadSignal<TokenBudget>,
    model_label: String,
    provider_label: String,
    context_length: u32,
    memory_enabled: bool,
) -> Element {
    let settings = use_context::<ShellSettings>();
    let personality = settings.personality.read().label();
    let sid = session_id();
    let session_display = if sid.is_empty() || sid == "pending" { "—".to_string() } else { sid };

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
            div { class: "wh-side-tabs",
                button {
                    class: "wh-side-tab",
                    class: if active_side_tab() == 0 { "is-active" },
                    onclick: move |_| on_side_tab_click.call(0),
                    "AGENT"
                }
                button {
                    class: "wh-side-tab",
                    class: if active_side_tab() == 1 { "is-active" },
                    onclick: move |_| on_side_tab_click.call(1),
                    "INFO"
                }
            }
            if active_side_tab() == 0 {
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
            } else {
                div { class: "wh-side-info",
                    div { class: "wh-side-info-card",
                        div { class: "wh-side-info-heading", "SESSION" }
                        div { class: "wh-side-info-row",
                            span { class: "wh-side-info-key", "id" }
                            span { class: "wh-side-info-val", "{session_display}" }
                        }
                        div { class: "wh-side-info-row",
                            span { class: "wh-side-info-key", "messages" }
                            span { class: "wh-side-info-val", "{messages.read().len()}" }
                        }
                        div { class: "wh-side-info-row",
                            span { class: "wh-side-info-key", "tokens" }
                            span { class: "wh-side-info-val",
                                "{token_budget.read().used} / {token_budget.read().max}"
                            }
                        }
                    }
                    div { class: "wh-side-info-card",
                        div { class: "wh-side-info-heading", "CONFIG" }
                        div { class: "wh-side-info-row",
                            span { class: "wh-side-info-key", "model" }
                            span { class: "wh-side-info-val", "{model_label}" }
                        }
                        div { class: "wh-side-info-row",
                            span { class: "wh-side-info-key", "provider" }
                            span { class: "wh-side-info-val", "{provider_label}" }
                        }
                        div { class: "wh-side-info-row",
                            span { class: "wh-side-info-key", "context" }
                            span { class: "wh-side-info-val", "{context_length}" }
                        }
                        div { class: "wh-side-info-row",
                            span { class: "wh-side-info-key", "memory" }
                            span { class: "wh-side-info-val",
                                if memory_enabled { "yes" } else { "no" }
                            }
                        }
                    }
                }
            }
        }
    }
}
