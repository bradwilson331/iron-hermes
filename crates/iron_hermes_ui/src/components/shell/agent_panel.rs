use crate::state::{SessionMemory, TokenBudget};
use dioxus::prelude::*;

const IH_SHIELD_PNG: Asset = asset!("/assets/ih-shield.png");
const PAGE_SIZE: usize = 6;

#[component]
pub fn AgentPanel(
    sessions: ReadSignal<Vec<SessionMemory>>,
    active_side_tab: ReadSignal<usize>,
    on_side_tab_click: EventHandler<usize>,
    session_id: ReadSignal<String>,
    token_budget: ReadSignal<TokenBudget>,
    model_label: String,
    provider_label: String,
    context_length: u32,
    memory_enabled: bool,
) -> Element {
    let sid = session_id();
    let session_display =
        if sid.is_empty() || sid == "pending" { "—".to_string() } else { sid };

    let mut page = use_signal(|| 0_usize);

    // Filter to sessions that have data or are currently live.
    // Sessions with no activity (ghost sessions from past runs) are hidden.
    let filtered: Vec<SessionMemory> = sessions
        .read()
        .iter()
        .filter(|s| s.is_live || s.exchange_count > 0)
        .cloned()
        .collect();

    let total = filtered.len();
    let total_pages = if total == 0 { 0 } else { (total + PAGE_SIZE - 1) / PAGE_SIZE };
    let cur_page = page().min(if total_pages > 0 { total_pages - 1 } else { 0 });
    let start = cur_page * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(total);
    let page_items: Vec<SessionMemory> = filtered[start..end].to_vec();
    let show_nav = total_pages > 1;

    rsx! {
        aside { class: "wh-side",
            div { class: "wh-side-head",
                img {
                    src: IH_SHIELD_PNG,
                    alt: "IronHermes",
                    style: "height: 24px; width: auto; opacity: 0.85;",
                }
            }
            div {
                class: "wh-side-tabs",
                role: "tablist",
                "aria-label": "Agent panel views",
                button {
                    class: "wh-side-tab",
                    class: if active_side_tab() == 0 { "is-active" },
                    role: "tab",
                    "aria-selected": if active_side_tab() == 0 { "true" } else { "false" },
                    "aria-controls": "side-panel-memory",
                    onclick: move |_| on_side_tab_click.call(0),
                    "MEMORY"
                }
                button {
                    class: "wh-side-tab",
                    class: if active_side_tab() == 1 { "is-active" },
                    role: "tab",
                    "aria-selected": if active_side_tab() == 1 { "true" } else { "false" },
                    "aria-controls": "side-panel-info",
                    onclick: move |_| on_side_tab_click.call(1),
                    "INFO"
                }
            }
            if active_side_tab() == 0 {
                div {
                    class: "wh-mem-panel",
                    role: "tabpanel",
                    id: "side-panel-memory",
                    div { class: "wh-side-scroll",
                        if page_items.is_empty() {
                            div { class: "wh-mem-empty", "no sessions" }
                        } else {
                            for (i, sess) in page_items.iter().enumerate() {
                                MemoryCard { key: "{start + i}", session: sess.clone() }
                            }
                        }
                    }
                    if show_nav {
                        div { class: "wh-mem-nav",
                            button {
                                class: "wh-mem-nav-btn",
                                disabled: cur_page == 0,
                                onclick: move |_| page.set(cur_page.saturating_sub(1)),
                                "‹"
                            }
                            span { class: "wh-mem-nav-pos",
                                "{cur_page + 1} / {total_pages}"
                            }
                            button {
                                class: "wh-mem-nav-btn",
                                disabled: cur_page + 1 >= total_pages,
                                onclick: move |_| page.set((cur_page + 1).min(total_pages - 1)),
                                "›"
                            }
                        }
                    }
                }
            } else {
                div {
                    class: "wh-side-info",
                    role: "tabpanel",
                    id: "side-panel-info",
                    div { class: "wh-side-info-card",
                        div { class: "wh-side-info-heading", "SESSION" }
                        div { class: "wh-side-info-row",
                            span { class: "wh-side-info-key", "id" }
                            span { class: "wh-side-info-val", "{session_display}" }
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

#[component]
fn MemoryCard(session: SessionMemory) -> Element {
    let time_range = match (session.first_time.is_empty(), session.last_time.is_empty()) {
        (true, _) => String::new(),
        (false, true) => session.first_time.clone(),
        (false, false) if session.first_time == session.last_time => session.first_time.clone(),
        (false, false) => format!("{}–{}", session.first_time, session.last_time),
    };

    let n = session.exchange_count;
    let t = session.token_count;
    let p = session.personality.as_str();
    let stats = match (n, t, p.is_empty() || p == "default") {
        (0, 0, true) => "new".to_string(),
        (0, 0, false) => p.to_string(),
        (n, 0, true) => format!("{n} msg{}", if n == 1 { "" } else { "s" }),
        (n, 0, false) => format!("{n} msg{} · {p}", if n == 1 { "" } else { "s" }),
        (n, t, true) => format!("{n} msg{} · {t} tokens", if n == 1 { "" } else { "s" }),
        (n, t, false) => format!("{n} msg{} · {t} tokens · {p}", if n == 1 { "" } else { "s" }),
    };

    let quoted = if session.last_input.is_empty() {
        String::new()
    } else {
        format!("\u{201c}{}\u{201d}", session.last_input)
    };

    rsx! {
        div {
            class: "wh-mem-card",
            class: if session.is_live { "is-live" },
            div { class: "wh-mem-card-head",
                if session.is_live {
                    div { class: "wh-mem-live-dot" }
                }
                span { class: "wh-mem-card-label", "{session.label}" }
                if !time_range.is_empty() {
                    span { class: "wh-mem-card-time", "{time_range}" }
                }
            }
            div { class: "wh-mem-card-stats", "{stats}" }
            if !quoted.is_empty() {
                div { class: "wh-mem-card-excerpt", "{quoted}" }
            }
        }
    }
}
