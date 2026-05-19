//! Agents screen — Phase 26.7 Plan 05 (Tasks 2 + 3) + Phase 26.7.1 Plan 01 (Task 4).
//!
//! Replaces the Plan 26.2.1-08 visual stub with a live
//! `api_agents_list` resource backed by the SubagentRegistry.  Per-card
//! KILL? inline confirm (two-click, 3-second timeout via gloo_timers) and
//! INTERRUPT (500ms `...` feedback) are wired in Task 3.  PRUNE ENDED calls
//! `api_agents_prune(None)` and restarts the list.
//!
//! Phase 26.7.1 Plan 01 additions:
//! - Poll loop: 1500 ms baseline (D-03), pauses on tab hidden (D-04) and
//!   while rpc_in_flight > 0 (D-05).
//! - recently_terminated map with 1 s decay sweep, 5 s hold window (D-01/D-11).
//! - diff_terminations integration (D-02/D-13).
//! - PRUNE click now also clears recently_terminated synchronously (D-12).
//! - AgentCard extended with is_ended + rpc_in_flight props (D-09/D-10/D-14).
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

    // Phase 26.7.1 Plan 01 — screen-level signals.
    //
    // recently_terminated: ids captured from poll diffs, held for 5 s before
    // removal by the decay sweep. Stores the LAST-OBSERVED AgentInfo snapshot
    // (D-11) alongside the Instant at which termination was detected.
    let mut recently_terminated = use_signal(||
        std::collections::HashMap::<String, (crate::server::api::AgentInfo, std::time::Instant)>::new()
    );
    // prev_live: snapshot of the agent list from the PREVIOUS render, used by
    // diff_terminations to detect newly-absent ids.
    let mut prev_live = use_signal(|| Vec::<crate::server::api::AgentInfo>::new());
    // rpc_in_flight: counts in-flight kill/interrupt RPCs across all cards.
    // The poll loop checks this before restarting the resource (D-05).
    let mut rpc_in_flight = use_signal(|| 0u32);

    // Consume context signals provided by Task 1's HermesApp providers.
    // Binding here proves the context is resolvable on first render (including
    // SSR), preventing the "Could not find context" panic.
    let ws_connected_ctx = use_context::<Signal<bool>>();      // is_ws_connected
    let _subagent_events_ctx = use_context::<Signal<u64>>();   // subagent_events — Plan 02 wires use_effect

    // Materialise the list and error flag BEFORE the rsx! block so no
    // GenerationalRef is held across the macro boundary (clippy.toml).
    let agents_list: Vec<crate::server::api::AgentInfo> = match agents_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let load_error = matches!(agents_resource(), Some(Err(_)));

    // D-13: re-running id wins — drop any recently_terminated entry whose id
    // reappeared in the live list. Read into bool (Copy) so borrow is released
    // before the write call on the same signal.
    for agent in agents_list.iter() {
        let in_map = recently_terminated.read().contains_key(&agent.id); // bool — borrow ends at ;
        if in_map {
            recently_terminated.write().remove(&agent.id);
        }
    }

    // D-02 + D-11: ids in prev_live absent from agents_list → snapshot into recently_terminated.
    // FIX (UAT-1 hotfix): only update prev_live when the resource has RESOLVED (i.e. we have a
    // Some(Ok(...)) value). During the loading/suspended window between `.restart()` and the
    // new fetch, `agents_resource()` returns `None` and the fallback `Vec::new()` at line 64
    // would make us snapshot an EMPTY list as "the new state" and immediately overwrite
    // `prev_live` to []. That causes the eventual real diff to compare [] vs [] → 0
    // terminations, even though A WAS just removed by kill/interrupt. We must distinguish
    // "list is genuinely empty" from "list is suspended" — only the former should drive
    // the diff and prev_live update.
    let resource_resolved: bool = matches!(agents_resource(), Some(Ok(_)));
    let prev_snapshot: Vec<crate::server::api::AgentInfo> = prev_live.read().clone(); // owned — borrow ends at ;
    let newly_terminated = if resource_resolved {
        crate::components::hermes_app::screens::agents_diff::diff_terminations(
            &prev_snapshot,
            &agents_list,
        )
    } else {
        Vec::new()
    };
    for old in newly_terminated.into_iter() {
        let already = recently_terminated.read().contains_key(&old.id); // bool — borrow ends at ;
        if !already {
            recently_terminated.write().insert(
                old.id.clone(),
                (old, std::time::Instant::now()),
            );
        }
    }
    // Only snapshot prev_live when the resource has resolved — otherwise we'd be overwriting
    // a valid [A] snapshot with the suspended-state [] fallback and losing the diff signal.
    if resource_resolved {
        prev_live.set(agents_list.clone());
    }

    // Instrumentation (wasm only): trace diff outcomes to the browser console so the UAT
    // operator can see exactly what the diff path observes per render. Kept gated to wasm32
    // so native cargo test stays quiet.
    #[cfg(target_arch = "wasm32")]
    {
        let ended_len = recently_terminated.read().len();
        web_sys::console::log_1(
            &format!(
                "[26.7.1-01 diff] resolved={} prev={} curr={} recently_terminated={}",
                resource_resolved,
                prev_snapshot.len(),
                agents_list.len(),
                ended_len,
            )
            .into(),
        );
    }

    // Materialise HOLD-card list before rsx! — collect into owned Vec so no
    // GenerationalRef is held during the rsx! macro expansion (clippy.toml).
    let ended_cards: Vec<crate::server::api::AgentInfo> = {
        let map = recently_terminated.read();
        map.values().map(|(info, _ts)| info.clone()).collect()
    }; // borrow ends at }

    // D-03 / D-04 / D-05 / D-08: Poll loop.
    // Checks visibility and in-flight RPC count before each restart.
    // Dynamic cadence: 1500 ms while ws disconnected (Plan 01 ships with
    // is_ws_connected = false), 5000 ms while connected (Plan 02 wires .set()).
    use_future(move || async move {
        loop {
            // D-04: skip while tab is hidden. document.hidden() is synchronous
            // bool — no JsCast needed, no borrow held across await.
            #[cfg(target_arch = "wasm32")]
            let hidden: bool = web_sys::window()
                .and_then(|w| w.document())
                .map(|d| d.hidden())
                .unwrap_or(false);
            #[cfg(not(target_arch = "wasm32"))]
            let hidden: bool = false;

            if hidden {
                gloo_timers::future::TimeoutFuture::new(500).await;
                continue;
            }

            // D-05: skip while a kill/interrupt RPC is in flight.
            let in_flight: u32 = *rpc_in_flight.read(); // Copy — borrow ends at ;
            if in_flight > 0 {
                gloo_timers::future::TimeoutFuture::new(200).await;
                continue;
            }

            agents_resource.restart();

            // D-08: dynamic cadence reads is_ws_connected from context.
            // Plan 01 ships with the initial value (false) giving 1500 ms
            // baseline. Plan 02 wires the recv-loop .set() calls that promote
            // to 5000 ms while connected.
            let interval_ms: u32 = if *ws_connected_ctx.read() { 5_000 } else { 1_500 };
            // ws_connected_ctx borrow ends at ; — interval_ms is Copy.
            gloo_timers::future::TimeoutFuture::new(interval_ms).await;
        }
    });

    // Decay sweep — runs every 1 s, removes entries older than 5 s (D-01).
    // Collect expired ids into an owned Vec (borrow released at }) before any write.
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(1_000).await;
            let expired: Vec<String> = {
                let map = recently_terminated.read();
                map.iter()
                    .filter_map(|(id, (_info, ts))| {
                        if ts.elapsed() >= std::time::Duration::from_secs(5) {
                            Some(id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            }; // borrow ends at }
            if !expired.is_empty() {
                let mut map = recently_terminated.write();
                for id in &expired {
                    map.remove(id);
                }
            }
        }
    });

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
                    // PRUNE ENDED: D-12 — clear client-side HOLD state synchronously,
                    // then call server prune and restart the resource.
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            // D-12: clear client-side HOLD state synchronously before
                            // the async prune call so ENDED cards disappear immediately.
                            recently_terminated.write().clear();
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
                    // Live agents
                    for agent in agents_list.iter() {
                        AgentCard {
                            key: "{agent.id}",
                            agent: agent.clone(),
                            agents_resource: agents_resource,
                            is_ended: false,
                            rpc_in_flight: rpc_in_flight,
                        }
                    }
                    // HOLD-N cards for recently terminated agents (D-01/D-09/D-10)
                    for agent in ended_cards.iter() {
                        AgentCard {
                            key: "ended-{agent.id}",
                            agent: agent.clone(),
                            agents_resource: agents_resource,
                            is_ended: true,
                            rpc_in_flight: rpc_in_flight,
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
/// - `agent` — live or snapshot `AgentInfo` from `api_agents_list` / `recently_terminated`.
/// - `agents_resource` — the screen-level resource handle; `.restart()` is
///   called after a successful kill or prune to refresh the list.
/// - `is_ended` — true when this card represents a terminated agent in HOLD-N state.
///   Renders with `.card.is-ended` + `.pill.ended` and hides the card-footer via CSS.
/// - `rpc_in_flight` — screen-level counter; incremented when an RPC is dispatched,
///   decremented when it resolves. The poll loop consults this to avoid racing (D-05).
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
    is_ended: bool,
    rpc_in_flight: Signal<u32>,
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
        div {
            class: "card",
            class: if is_ended { "is-ended" },

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
                // D-09: single ENDED pill for all terminations (no per-kind differentiation).
                // D-14: .pill.ended is the gray neutral variant from screens.css.
                span {
                    class: if is_ended { "pill ended" } else { "pill green" },
                    if is_ended { "ENDED" } else { "{agent.status.to_uppercase()}" }
                }
            }

            // ── Card body ─────────────────────────────────────────────
            div { class: "card-body", "{agent.task_summary}" }

            // ── Card footer — action buttons ──────────────────────────
            // The .card.is-ended .card-footer { display: none } CSS rule from
            // screens.css hides this block for ENDED cards — no RSX conditional needed.
            div { class: "card-footer",

                // CHAT — static visual affordance (deferred per UI-SPEC).
                button { class: "btn btn--sm", "CHAT" }

                // INTERRUPT — 500 ms visual `...` feedback + list refresh.
                // D-05: increments rpc_in_flight while the RPC is in flight.
                // UAT-1 hotfix: parity with KILL — interrupt now also calls
                // agents_resource.restart() so the HOLD card can render via the diff path
                // without waiting up to 1.5s for the next periodic poll. The restart fires
                // BEFORE rpc_in_flight decrement so the periodic poll doesn't sneak a
                // racing restart in between (which would coalesce, but the ordering also
                // lets us hold rpc_in_flight > 0 until the registry's Drop has run).
                button {
                    class: "btn btn--sm btn--ghost",
                    style: "color:var(--warning)",
                    disabled: interrupting(),
                    onclick: move |_| {
                        // D-05: increment before dispatch, decrement after resolve.
                        let cur = *rpc_in_flight.read(); // Copy — borrow ends at ;
                        rpc_in_flight.set(cur + 1);
                        interrupting.set(true);
                        let id = agent_id_int.clone();
                        spawn(async move {
                            let _ = crate::server::api::api_agents_interrupt(id).await;
                            // 500 ms visual hold per UI-SPEC §"Interrupt button".
                            gloo_timers::future::TimeoutFuture::new(500).await;
                            interrupting.set(false);
                            // UAT-1 hotfix: trigger refresh so the diff path captures the
                            // termination (server's RegistrationGuard::drop has run by now —
                            // the 500ms visual hold gave it plenty of time).
                            agents_resource.restart();
                            // Decrement AFTER restart is queued so the periodic poll
                            // doesn't race the restart-driven re-render.
                            let cur2 = *rpc_in_flight.read(); // Copy — borrow ends at ;
                            rpc_in_flight.set(cur2.saturating_sub(1));
                        });
                    },
                    if interrupting() { "..." } else { "INTERRUPT" }
                }

                // KILL — two-click inline confirm with 3-second armed timeout.
                // D-05: increments rpc_in_flight on second click (actual kill POST).
                button {
                    class: "btn btn--sm btn--ghost",
                    style: "color:var(--danger)",
                    disabled: killing(),
                    onclick: move |_| {
                        if armed() {
                            // Second click within 3 s — execute kill.
                            armed.set(false);
                            killing.set(true);
                            // D-05: increment when the kill RPC is dispatched.
                            let cur = *rpc_in_flight.read(); // Copy — borrow ends at ;
                            rpc_in_flight.set(cur + 1);
                            let id = agent_id_kill.clone();
                            spawn(async move {
                                let _ = crate::server::api::api_agents_kill(id).await;
                                killing.set(false);
                                // UAT-1 hotfix: give the server's RegistrationGuard::drop a
                                // beat to actually remove the agent from the registry before
                                // we restart() — otherwise the refetch can return the
                                // still-present-but-doomed agent, and by the time it IS gone
                                // a subsequent render will have already set prev_live = [A]
                                // again and the diff signal is lost.
                                gloo_timers::future::TimeoutFuture::new(100).await;
                                // Restart BEFORE decrement so the periodic poll doesn't race
                                // the restart-driven re-render. The restart's refetch will
                                // produce the new (empty) list, the diff path will capture
                                // the termination, and the HOLD card will render.
                                agents_resource.restart();
                                let cur2 = *rpc_in_flight.read(); // Copy — borrow ends at ;
                                rpc_in_flight.set(cur2.saturating_sub(1));
                            });
                        } else {
                            // First click — arm and start 3 s timeout.
                            // D-05: first click is NOT an RPC, so rpc_in_flight is NOT incremented.
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
