//! Agents screen — Phase 26.7 Plan 05 (Tasks 2 + 3).
//!
//! Replaces the Plan 26.2.1-08 visual stub with a live
//! `api_agents_list` resource backed by the SubagentRegistry.  Per-card
//! KILL? inline confirm (two-click, 3-second timeout via gloo_timers) and
//! INTERRUPT (500ms `...` feedback) are wired in Task 3.  PRUNE ENDED calls
//! `api_agents_prune(None)` and restarts the list.
//!
//! Signal-borrow discipline (clippy.toml): all `.read()` calls that produce
//! a `GenerationalRef` are dropped before the `rsx!` block.  Signals
//! captured in `spawn(async move { ... })` closures are read/written via
//! `.set()` / `()` — both of which are value-copy operations returning `bool`
//! / `T: Copy`, never holding a borrow across `.await`.

use dioxus::prelude::*;
use dioxus::CapturedError;

// ── Screen ───────────────────────────────────────────────────────────────────

/// Agents screen — `<section id="screen-agents">` ported from `app.html`
/// lines 501-591.  Fetches the live agent list from `api_agents_list` and
/// surfaces per-card KILL? / INTERRUPT controls plus a screen-level
/// PRUNE ENDED action.
#[component]
pub fn ScreenAgents(is_active: bool) -> Element {
    // `use_server_future` suspends on first render until the future resolves,
    // then re-evaluates on `.restart()`.  The `?` operator propagates the
    // `RenderError` (Dioxus 0.7 — same idiom as sessions.rs:33).
    let mut agents_resource = use_server_future(crate::server::api::api_agents_list)?;

    // Materialise the list and error flag BEFORE the rsx! block so no
    // GenerationalRef is held across the macro boundary (clippy.toml).
    let agents_list: Vec<crate::server::api::AgentInfo> = match agents_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let load_error = matches!(agents_resource(), Some(Err(_)));

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-agents",
            "data-screen-label": "03 Agents",

            // ── Header ────────────────────────────────────────────────
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 03" }
                    h1 { class: "screen-title", "Agents" }
                    p { class: "screen-sub",
                        "Each profile is an isolated Hermes workspace with its own config, memory, skill set, and persona."
                    }
                }
                div { class: "screen-actions",
                    // Static visual affordance — write op deferred per out-of-scope.
                    button { class: "btn btn--sm", "+ NEW AGENT" }
                    // PRUNE ENDED: calls api_agents_prune(None) then restarts the list.
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            spawn(async move {
                                let _ = crate::server::api::api_agents_prune(None).await;
                                agents_resource.restart();
                            });
                        },
                        "PRUNE ENDED"
                    }
                }
            }

            // ── Agent grid ────────────────────────────────────────────
            div { class: "grid wide",
                if load_error {
                    div {
                        style: "color:var(--danger);font-size:var(--fs-12);",
                        "Could not load agents — check server connection."
                    }
                } else {
                    for agent in agents_list.iter() {
                        AgentCard {
                            key: "{agent.id}",
                            agent: agent.clone(),
                            agents_resource: agents_resource,
                        }
                    }
                }
            }
        }
    }
}

// ── AgentCard ────────────────────────────────────────────────────────────────

/// One agent card in the live grid.
///
/// # Props
/// - `agent` — live `AgentInfo` from `api_agents_list`.
/// - `agents_resource` — the screen-level resource handle; `.restart()` is
///   called after a successful kill or prune to refresh the list.
///
/// # Per-card signals
/// - `armed: Signal<bool>` — true while the KILL button is in the "KILL?"
///   armed state waiting for a second click within 3 seconds.
/// - `killing: Signal<bool>` — true while the kill POST is in flight
///   (disables the button to prevent double-fire).
/// - `interrupting: Signal<bool>` — true while the interrupt POST is in
///   flight; label shows `...` for 500 ms then reverts.
#[component]
fn AgentCard(
    agent: crate::server::api::AgentInfo,
    agents_resource: Resource<Result<Vec<crate::server::api::AgentInfo>, CapturedError>>,
) -> Element {
    // Per-card local state — independent across rows.
    let mut armed = use_signal(|| false);
    let mut killing = use_signal(|| false);
    let mut interrupting = use_signal(|| false);

    // Clone IDs for use in owned async closures (PATTERNS.md Pitfall 6 /
    // RESEARCH §"Common Pitfalls" #6 — never borrow from component scope
    // inside `spawn(async move { ... })`).
    let agent_id_kill = agent.id.clone();
    let agent_id_arm = agent.id.clone();
    let agent_id_int = agent.id.clone();

    // Derive avatar letter from first char of agent id.
    let avatar_letter = agent
        .id
        .chars()
        .next()
        .unwrap_or('S')
        .to_ascii_uppercase();

    rsx! {
        div { class: "card",

            // ── Card head ─────────────────────────────────────────────
            div { class: "card-head",
                div { class: "avatar shield",
                    "{avatar_letter}"
                }
                div { style: "flex:1",
                    div { class: "card-title", "{agent.id}" }
                    div { class: "card-meta",
                        "{agent.status} · {agent.uptime_secs}s"
                    }
                }
                span { class: "pill green", "{agent.status.to_uppercase()}" }
            }

            // ── Card body ─────────────────────────────────────────────
            div { class: "card-body", "{agent.task_summary}" }

            // ── Card footer — action buttons ──────────────────────────
            div { class: "card-footer",

                // CHAT — static visual affordance (deferred per UI-SPEC).
                button { class: "btn btn--sm", "CHAT" }

                // INTERRUPT — 500 ms visual `...` feedback, no list refresh.
                button {
                    class: "btn btn--sm btn--ghost",
                    style: "color:var(--warning)",
                    disabled: interrupting(),
                    onclick: move |_| {
                        interrupting.set(true);
                        let id = agent_id_int.clone();
                        spawn(async move {
                            let _ = crate::server::api::api_agents_interrupt(id).await;
                            // 500 ms visual hold per UI-SPEC §"Interrupt button".
                            gloo_timers::future::TimeoutFuture::new(500).await;
                            interrupting.set(false);
                        });
                    },
                    if interrupting() { "..." } else { "INTERRUPT" }
                }

                // KILL — two-click inline confirm with 3-second armed timeout.
                button {
                    class: "btn btn--sm btn--ghost",
                    style: "color:var(--danger)",
                    disabled: killing(),
                    onclick: move |_| {
                        if armed() {
                            // Second click within 3 s — execute kill.
                            armed.set(false);
                            killing.set(true);
                            let id = agent_id_kill.clone();
                            spawn(async move {
                                let _ = crate::server::api::api_agents_kill(id).await;
                                killing.set(false);
                                agents_resource.restart();
                            });
                        } else {
                            // First click — arm and start 3 s timeout.
                            armed.set(true);
                            let _id = agent_id_arm.clone();
                            spawn(async move {
                                gloo_timers::future::TimeoutFuture::new(3_000).await;
                                armed.set(false);
                            });
                        }
                    },
                    if armed() { "KILL?" } else { "KILL" }
                }
            }
        }
    }
}
