//! Agents screen — Phase 26.7 Plan 05 (Tasks 2 + 3) + Phase 26.7.1 Plan 01 (Task 4).
//!
//! Phase 50.1 Plan 01 (D-08/D-09/D-11) additions: the `BotRoster` section is
//! mounted directly beneath the screen header, ABOVE the subagent-turn grid
//! documented below — the `+ NEW AGENT` button (previously a dead visual
//! affordance) now opens `CreateProfileWizard`, whose `on_created` callback
//! saves an initial bot-meta entry and bumps the roster's refresh tick. The
//! subagent grid, `AgentCard`, `TurnCard`, PRUNE ENDED, CANCEL and the
//! HOLD-N decay sweep documented below are unchanged by this plan (D-09).
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
//! Phase 39.1 Plan 07 additions (R39.1-09 / R39.1-10):
//! - TurnRegistry section below the agent grid renders all in-flight turns
//!   across every surface (Web, Gateway, CLI, Realtime) via `turns_list`.
//! - TurnCard renders turn_id, surface pill, session_id, elapsed_ms, and a
//!   CANCEL button wired to `turn_cancel`.
//! - Turns list auto-refreshes every 3 s (independent of agent poll loop).
//!
//! Signal-borrow discipline (clippy.toml): all `.read()` calls that produce
//! a `GenerationalRef` are dropped before the `rsx!` block.  Signals
//! captured in `spawn(async move { ... })` closures are read/written via
//! `.set()` / `()` — both of which are value-copy operations returning `bool`
//! / `T: Copy`, never holding a borrow across `.await`.

use dioxus::prelude::*;
use dioxus::CapturedError;

// Phase 50.1 Plan 02 (D-10): the create-profile wizard mounted from this
// screen's `+ NEW AGENT` button, opened in bot context — this is the one
// import Plan 01's tracer deliberately left pointing at the pre-lift kanban
// shim path; Plan 02 repoints it at the shared module directly.
use crate::components::hermes_app::screens::profile_shared::create_dialog::CreateProfileWizard;
use crate::components::hermes_app::screens::profile_shared::ProfileDialogContext;
// Phase 50.1 Plan 01 (D-08/D-09): the bot roster section mounted directly
// beneath the screen header, above the pre-existing subagent-turn grid.
use crate::components::hermes_app::screens::bot_roster::BotRoster;

// ── Screen ───────────────────────────────────────────────────────────────────

/// Agents screen — `<section id="screen-agents">` ported from `app.html`
/// lines 501-591.  Fetches the live agent list from `api_agents_list` and
/// surfaces per-card KILL? / INTERRUPT controls plus a screen-level
/// PRUNE ENDED action.
#[component]
pub fn ScreenAgents(is_active: bool) -> Element {
    // UAT-2 hotfix: SWAP from `use_server_future(...)?` to `use_resource`.
    //
    // Why the `use_server_future(...)?` pattern was broken: the `?` operator
    // propagates a `RenderError` whenever the resource is in the loading
    // (`None`) state, causing the component function to return EARLY before
    // the `use_future` poll loop and decay sweep get a chance to register.
    // In Dioxus 0.7, hooks must register in the same order on every render.
    // After every `.restart()` call, the resource re-enters the loading state,
    // `?` re-fires the early return, and the previously-spawned `use_future`s
    // are not re-issued because they were never registered in this render's
    // hook ordering. UAT-2 confirmed: zero `/api/agents/list` fetches over 30s
    // on a fresh navigation.
    //
    // `use_resource` is the same primitive without the SSR-suspension wrapper.
    // It returns `Resource<Result<T, ServerFnError>>` and its value is read via
    // `()` returning `Option<Result<T, ServerFnError>>` where `None` = loading.
    // All hooks below this line now register on every render regardless of
    // resource state.
    //
    // Note: dropping SSR suspension is acceptable here because (a) ScreenAgents
    // is class-toggle-mounted (always present in DOM, the `is-active` class
    // gates visibility), so first-paint suspension never blocked the user from
    // seeing other screens, and (b) the empty-list fallback during the initial
    // load window is visually indistinguishable from an empty registry — exact
    // same UI as if no agents exist yet.
    let mut agents_resource =
        use_resource(move || async move { crate::server::api::api_agents_list().await });

    // Phase 50.1 Plan 01 (D-08/D-09/D-11): roster signals. `wizard_open`
    // mirrors `create_modal_open`'s existing idiom (`kanban.rs:113`).
    // `bot_roster_refresh_tick` is the crate's proven refresh idiom
    // (`profile_switcher.rs:55-58`) — bumped after `save_bot_meta` resolves
    // so a freshly created bot appears without a page reload.
    let mut wizard_open: Signal<bool> = use_signal(|| false);
    let bot_roster_refresh_tick: Signal<u32> = use_signal(|| 0u32);
    let bot_roster_refresh_tick_ro: ReadSignal<u32> = bot_roster_refresh_tick.into();
    // Phase 50.1 Plan 02 Task 3 (D-10 extension): owned here (mirrors
    // `kanban.rs`'s own `detail_profile` idiom) and threaded down into
    // `BotRoster` as a plain `Signal<Option<String>>` prop — never a
    // context provider — so both a roster card's `EDIT` action and the
    // create wizard's post-create hand-off below can open the same shared
    // drawer instance.
    let drawer_target: Signal<Option<String>> = use_signal(|| None);

    // Phase 26.7.1 Plan 01 — screen-level signals.
    //
    // recently_terminated: ids captured from poll diffs, held for 5 s before
    // removal by the decay sweep. Stores the LAST-OBSERVED AgentInfo snapshot
    // (D-11) alongside the Instant at which termination was detected.
    //
    // UAT-4 hotfix: web_time::Instant instead of std::time::Instant.
    // std::time::Instant::now() panics on wasm32 with "time not implemented on
    // this platform" (rustc's wasm32-unknown-unknown stdlib doesn't have a
    // monotonic clock). web_time::Instant is API-identical (pub use of
    // std::time::Instant on native; Performance.now()-backed shim on wasm32)
    // so .elapsed() and Duration arithmetic in the decay sweep work unchanged.
    let mut recently_terminated = use_signal(|| {
        std::collections::HashMap::<String, (crate::server::api::AgentInfo, web_time::Instant)>::new(
        )
    });
    // prev_live: snapshot of the agent list from the PREVIOUS render, used by
    // diff_terminations to detect newly-absent ids.
    let mut prev_live = use_signal(Vec::<crate::server::api::AgentInfo>::new);
    // rpc_in_flight: counts in-flight kill/interrupt RPCs across all cards.
    // The poll loop checks this before restarting the resource (D-05).
    let rpc_in_flight = use_signal(|| 0u32);

    // Consume context signals provided by Task 1's HermesApp providers.
    // Binding here proves the context is resolvable on first render (including
    // SSR), preventing the "Could not find context" panic.
    let ws_connected_ctx = use_context::<crate::state::WsConnectedContext>().0; // is_ws_connected
    let subagent_events = use_context::<crate::state::SubagentEventsContext>().0; // D-07 — increments on ws SubagentEvent
                                                                                  // Phase 36.3.7.11 D-03 — Agents-toolbar `KANBAN BOARD →` button writes to
                                                                                  // the screen signal published by HermesApp. Same context-access pattern as
                                                                                  // screens/sessions.rs:63 and screens/settings.rs:326.
    let mut active_screen = use_context::<Signal<crate::state::Screen>>();

    // Materialise the list and error flag BEFORE the rsx! block so no
    // GenerationalRef is held across the macro boundary (clippy.toml).
    let agents_list: Vec<crate::server::api::AgentInfo> = match agents_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let load_error = matches!(agents_resource(), Some(Err(_)));

    // D-13: re-running id wins — drop any recently_terminated entry whose id
    // reappeared in the live list.
    //
    // CR-01 fix (Phase 26.7.1 review): collapse N potential per-render writes
    // into 1 (or 0) writes. Previously this loop called `.write()` every time
    // a live agent appeared in the HOLD map; in Dioxus 0.7 `.write()` marks the
    // signal dirty unconditionally and schedules a re-render even when the
    // post-write state is identical. This drove a gratuitous re-render every
    // time the "re-running id wins" path fired, interleaving badly with the
    // poll loop and the push-restart effect. The fix computes the removal set
    // first (a read-only borrow that ends at the closing `}` of the block),
    // then issues exactly one `.write()` only when the set is non-empty. The
    // pattern mirrors the decay sweep at lines 246-263.
    let to_remove: Vec<String> = {
        let map = recently_terminated.read();
        agents_list
            .iter()
            .filter(|a| map.contains_key(&a.id))
            .map(|a| a.id.clone())
            .collect()
    }; // borrow ends at }
    if !to_remove.is_empty() {
        let mut map = recently_terminated.write();
        for id in &to_remove {
            map.remove(id);
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
                // UAT-4 hotfix: web_time::Instant::now() — see comment at the
                // signal declaration. std::time::Instant::now() panics on wasm32.
                (old, web_time::Instant::now()),
            );
        }
    }
    // UAT-3 hotfix: gate prev_live.set() on actual content change, not on every
    // resolved render. Dioxus 0.7 signals do NOT deduplicate identical writes —
    // every `.set()` schedules a re-render, which re-runs the diff block, which
    // calls `.set()` again. UAT-2's swap to `use_resource` removed the suspended-
    // state pacing that had been masking this infinite-loop bug under
    // `use_server_future + ?`, so the page froze on UAT-3 with the diff log
    // firing dozens of times at the same wall-clock timestamp.
    //
    // AgentInfo derives PartialEq (verified in src/server/api.rs line 360), so we
    // can compare prev_snapshot (the owned Vec already cloned at line 112 from
    // prev_live.read()) against agents_list directly. prev_snapshot is the
    // CURRENT value of prev_live at the start of this render — comparing against
    // it is equivalent to checking whether the upcoming .set() would be a no-op.
    // No additional .read() needed (and importantly: no .read() held across the
    // .set() call, which would violate signal-borrow discipline).
    if resource_resolved && prev_snapshot != agents_list {
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
    //
    // UAT-2 instrumentation: per-iteration `[26.7.1-01 poll]` log line so the
    // UAT operator can confirm in DevTools Console that the loop is alive and
    // see exactly which gate (hidden / rpc_in_flight) is suppressing fetches.
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

            // D-05: read in-flight count (Copy — borrow ends at ;).
            let in_flight: u32 = *rpc_in_flight.read();

            // UAT-2 instrumentation: emit a tick every iteration so we can see
            // the loop is alive and which gate (if any) is suppressing the
            // fetch.
            #[cfg(target_arch = "wasm32")]
            web_sys::console::log_1(
                &format!(
                    "[26.7.1-01 poll] tick hidden={} in_flight={}",
                    hidden, in_flight
                )
                .into(),
            );

            if hidden {
                gloo_timers::future::TimeoutFuture::new(500).await;
                continue;
            }
            if in_flight > 0 {
                gloo_timers::future::TimeoutFuture::new(200).await;
                continue;
            }

            agents_resource.restart();
            #[cfg(target_arch = "wasm32")]
            web_sys::console::log_1(&"[26.7.1-01 poll] restart() fired".into());

            // D-08: dynamic cadence reads is_ws_connected from context.
            // Plan 01 ships with the initial value (false) giving 1500 ms
            // baseline. Plan 02 wires the recv-loop .set() calls that promote
            // to 5000 ms while connected.
            let interval_ms: u32 = if *ws_connected_ctx.read() {
                5_000
            } else {
                1_500
            };
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

    // Phase 39.1 Plan 07 (R39.1-09): fetch all in-flight turns from TurnRegistry.
    // Refreshed every 3 s independently of the agent poll loop.
    let mut turns_resource =
        use_resource(move || async move { crate::server::api::turns_list().await });

    // Turns poll loop — 3 s cadence.
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(3_000).await;
            turns_resource.restart();
        }
    });

    // Materialise turns list before rsx! — no GenerationalRef across macro boundary.
    let turns_list: Vec<crate::server::api::TurnSummaryWire> = match turns_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };

    // Phase 26.7.1 Plan 02 D-07: any SubagentEvent from the chat ws bumps the
    // counter; this effect subscribes (call-syntax returns Copy u64 — no
    // borrow held) and triggers a fresh `agents_resource.restart()`. Same code
    // path as the poll, no divergent diff logic. The poll loop is now the
    // safety net for missed events rather than the primary refresh.
    //
    // CR-02 fix (Phase 26.7.1 review): gate restart on strict `cur > prev`
    // against a `last_seen` signal seeded with the CURRENT value of
    // `subagent_events()` at mount time. Two defects in the original effect:
    //
    //   1. Spurious initial-mount restart. `use_resource` already issues an
    //      initial fetch on mount; the effect then fired immediately on the
    //      same mount because `subagent_events()` returned its initial value.
    //      That second restart cancelled the first and raced the poll loop's
    //      first tick, producing 2-3 `/api/agents/list` requests in the first
    //      ~1.5s of mount. Seeding `last_seen` with the current counter value
    //      and gating on `cur > prev` means the initial render observes
    //      `cur == prev` and skips — the resource's own initial fetch is the
    //      only network call.
    //
    //   2. No debounce on burst. A turn that spawned N subagents fired N
    //      events on the chat ws, each incrementing `subagent_events` and
    //      re-firing this effect. Each `.restart()` cancelled the previous.
    //      With the strict gate, the first event still restarts once; the
    //      poll loop is the safety net per the design comment above. A
    //      debounce timer can be layered on later if cancel-churn shows up
    //      in WASM CPU profiles — for now correctness > micro-optimisation.
    //
    // Signal-borrow discipline: `*last_seen.read()` copies a `u64` and the
    // borrow ends at `;` before any `.set()` / `.restart()` call. No borrow
    // is held across the (synchronous) restart.
    // Allow: subagent_events is a Signal<u64>, not a bare fn pointer.
    // Calling subagent_events() reads the current u64 value as initial state;
    // passing &subagent_events would be a wrong type for use_signal's initializer.
    #[allow(clippy::redundant_closure)]
    let mut last_seen = use_signal(|| subagent_events());
    let mut agents_resource_for_effect = agents_resource; // Resource is Copy
    use_effect(move || {
        let cur = subagent_events(); // subscribe; Copy u64 — no borrow
        let prev = *last_seen.read(); // Copy u64 — borrow ends at ;
        if cur > prev {
            last_seen.set(cur);
            agents_resource_for_effect.restart();
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
                }
                div { class: "screen-actions",
                    // Phase 50.1 Plan 01 (D-08): opens the create-profile
                    // wizard — the same component `kanban.rs`'s
                    // `+ NEW PROFILE` button opens.
                    button {
                        class: "btn btn--sm",
                        onclick: move |_| wizard_open.set(true),
                        "+ NEW AGENT"
                    }
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
                    // Phase 36.3.7.11 D-03 — primary navigation entry-point for
                    // operators supervising swarm work. Single-tap to swap the
                    // screen signal to Screen::Kanban; the screen-router fans
                    // out to the placeholder ScreenKanban (Plan 04) until
                    // Plan 01 wires the live KanbanBoard.
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            active_screen.set(crate::state::Screen::Kanban);
                        },
                        "KANBAN BOARD →"
                    }
                }
            }

            // ── ROSTER (D-08/D-09/D-11) ───────────────────────────────
            // Bot roster, mounted directly beneath the screen header and
            // BEFORE the subagent-turn grid below — D-09 ordering. The
            // subagent grid, the `// TURNS` header, In-Flight Turns,
            // `AgentCard`, `TurnCard`, PRUNE ENDED, CANCEL and the HOLD-N
            // decay sweep are unchanged by this plan.
            BotRoster {
                refresh_tick: bot_roster_refresh_tick_ro,
                on_create: move |_| wizard_open.set(true),
                // Phase 50.1 Plan 02 Task 3 (D-10 extension): the shared
                // drawer target — a roster card's `EDIT` action and the
                // create wizard's post-create hand-off below both set it.
                drawer_target,
                on_profile_updated: move |_| {
                    let mut tick = bot_roster_refresh_tick;
                    let cur = *tick.read();
                    tick.set(cur + 1);
                },
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

            // ── In-flight turns (R39.1-09 / R39.1-10) ────────────────
            // All surfaces: Web, Gateway, CLI, Realtime.
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// TURNS" }
                    h2 {
                        style: "font-size:var(--fs-14);margin:0;",
                        "In-Flight Turns"
                    }
                }
                div { class: "screen-actions",
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| { turns_resource.restart(); },
                        "REFRESH"
                    }
                }
            }
            div { class: "grid wide",
                if turns_list.is_empty() {
                    div {
                        style: "color:var(--fg-muted);font-size:var(--fs-12);padding:var(--sp-8) 0;",
                        "No in-flight turns."
                    }
                } else {
                    for turn in turns_list.iter() {
                        TurnCard {
                            key: "{turn.turn_id}",
                            turn: turn.clone(),
                            turns_resource: turns_resource,
                        }
                    }
                }
            }

            // Phase 50.1 Plan 01 (D-08): the create-profile wizard, mounted
            // conditionally exactly like `kanban.rs`'s own
            // `CreateProfileWizard` mount. `on_created` fires after
            // `create_profile` succeeds — this plan additionally calls
            // `save_bot_meta` so the store gains an entry at creation time,
            // then bumps the roster's refresh tick once so the new bot
            // appears without a page reload. Phase 50.1 Plan 02 Task 3
            // (UI-SPEC Component Inventory §3 wizard footer / D-10): after
            // the refresh, also opens the new bot's detail drawer — the
            // operator lands somewhere real, not back on an unchanged grid
            // (matches the 47.4 create-into-edit precedent).
            if *wizard_open.read() {
                CreateProfileWizard {
                    // Phase 50.1 Plan 02 (D-10): explicit at the call site,
                    // not implicit in the default — the same discipline
                    // `kanban.rs` uses for its own `Kanban` mounts.
                    context: ProfileDialogContext::Bot,
                    on_dismiss: move |_| wizard_open.set(false),
                    on_created: move |name: String| {
                        let mut tick = bot_roster_refresh_tick;
                        let mut drawer_target_sig = drawer_target;
                        let name_for_drawer = name.clone();
                        spawn(async move {
                            let patch = crate::protocol::BotMetaPatch {
                                name,
                                title: None,
                                description: None,
                                avatar: None,
                                group: None,
                                preview: None,
                                preview_at_ms: None,
                            };
                            let _ = crate::server::bot_meta_api::save_bot_meta(patch).await;
                            let cur = *tick.read();
                            tick.set(cur + 1);
                            drawer_target_sig.set(Some(name_for_drawer));
                        });
                    },
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

    // Phase 50.2 Plan 16 (G-50.2-2c): the screen-navigation context every
    // sibling component already consumes (`ScreenAgents` itself at
    // agents.rs:134, the roster card's own CHAT button at card.rs:143/191).
    // Context providers live at the `HermesApp` root, so consuming here is
    // safe — a PROVIDER declared in a child is what panics, not a consumer.
    let mut active_screen = use_context::<Signal<crate::state::Screen>>();

    // Clone IDs for use in owned async closures (PATTERNS.md Pitfall 6 /
    // RESEARCH §"Common Pitfalls" #6 — never borrow from component scope
    // inside `spawn(async move { ... })`).
    let agent_id_kill = agent.id.clone();
    let agent_id_arm = agent.id.clone();
    let agent_id_int = agent.id.clone();

    // Derive avatar letter from first char of agent id.
    let avatar_letter = agent.id.chars().next().unwrap_or('S').to_ascii_uppercase();

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

                // Phase 50.2 Plan 16 (G-50.2-2c): CHAT navigates to the LIVE
                // profile's own Chat screen — the correct destination, not a
                // per-bot chat window. `AgentInfo.id` (this card) is a
                // subagent id from the live runtime's `SubagentRegistry`,
                // and `SubagentInfo` (subagent_registry.rs) carries no
                // profile field because every subagent on this screen
                // belongs to the live profile's own in-process runtime —
                // there is no agent-id -> profile-name mapping to invent
                // (this plan's own prohibition). The Chat screen's composer
                // already queues a mid-turn message for the next turn
                // (ws.rs:931-957, R39.1-06), so this button reaches a real
                // steering surface, not a placeholder. No additional
                // status gating is needed here: the ENDED card's whole
                // footer (this button included) is already hidden by the
                // `.card.is-ended .card-footer` CSS rule above.
                button {
                    class: "btn btn--sm",
                    onclick: move |_| active_screen.set(crate::state::Screen::Chat),
                    "CHAT"
                }

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

// ── TurnCard ─────────────────────────────────────────────────────────────────

/// Phase 50.2 Plan 14 (G-50.2-2b): `true` when `s` is shaped like a 36-char
/// UUID string (hyphens at the four canonical positions — 8, 13, 18, 23 —
/// every other character an ASCII hex digit). Pure, dependency-free (no
/// `uuid` import in this wasm-compiled file) — used only to decide the
/// `TurnCard` meta line's rendering, never to validate a real identifier.
///
/// `#[allow(dead_code)]`: called from `turn_session_display` below (live
/// production code, `TurnCard`'s own `card-meta` line) and from this file's
/// `#[cfg(test)] mod tests` — the native "bin" clippy target's dead-code
/// pass does not trace through this crate's Dioxus component tree, the same
/// false-positive `state.rs`'s `WsConnectedContext` et al. already document
/// with an identical annotation.
#[allow(dead_code)]
fn looks_like_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if *b != b'-' {
                    return false;
                }
            }
            _ => {
                if !b.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

/// Phase 50.2 Plan 14 (G-50.2-2b): render a `TurnSummaryWire.session_id` for
/// the `TurnCard` meta line. A UUID-shaped session id (web, gateway and
/// realtime turns today) shortens to its first eight characters followed by
/// a horizontal-ellipsis character — today's unchanged behaviour. Any other
/// session id (a subprocess-transport handoff turn's
/// `crate::server::cli_handoff::handoff_turn_session_id` label, e.g.
/// `"zig · Bot Chat"`) passes through in full, so the operator reads the
/// bot's name and its thread rather than a truncated, meaningless prefix.
///
/// `#[allow(dead_code)]`: see `looks_like_uuid`'s doc comment above — same
/// native-bin-target dead-code false positive despite the live call site in
/// `TurnCard`'s `card-meta` line just below.
#[allow(dead_code)]
pub(crate) fn turn_session_display(session_id: &str) -> String {
    if looks_like_uuid(session_id) {
        format!("{}\u{2026}", session_id.chars().take(8).collect::<String>())
    } else {
        session_id.to_string()
    }
}

/// One in-flight turn card rendered in the turns section (R39.1-09).
///
/// Shows turn_id (truncated to 12 chars), surface pill, session_id, elapsed_ms,
/// and a CANCEL button that calls `turn_cancel` and refreshes the turns list.
///
/// Signal-borrow discipline: no GenerationalRef held across `.await`.
/// `cancelling: Signal<bool>` is set/cleared via value-copy `.set()`.
#[component]
fn TurnCard(
    turn: crate::server::api::TurnSummaryWire,
    turns_resource: Resource<Result<Vec<crate::server::api::TurnSummaryWire>, ServerFnError>>,
) -> Element {
    let mut cancelling = use_signal(|| false);

    // Short display id — first 12 hex chars of the UUID string.
    let short_id: String = turn.turn_id.chars().take(12).collect();

    // Surface pill color class.
    let surface_class = match turn.surface.as_str() {
        "realtime" => "pill green",
        "web" => "pill",
        "gateway" => "pill",
        "cli" => "pill",
        _ => "pill",
    };

    // Elapsed seconds for display.
    let elapsed_s = turn.elapsed_ms / 1_000;

    let turn_id_for_cancel = turn.turn_id.clone();

    rsx! {
        div {
            class: "card",

            // ── Card head ─────────────────────────────────────────────
            div { class: "card-head",
                div { class: "avatar shield",
                    // First char of surface as avatar glyph.
                    "{turn.surface.chars().next().unwrap_or('?').to_ascii_uppercase()}"
                }
                div { style: "flex:1",
                    div { class: "card-title", "{short_id}…" }
                    div { class: "card-meta",
                        "session: {turn_session_display(&turn.session_id)} · {elapsed_s}s"
                    }
                }
                span { class: "{surface_class}", "{turn.surface.to_uppercase()}" }
            }

            // ── Card footer — cancel control ──────────────────────────
            div { class: "card-footer",
                button {
                    class: "btn btn--sm btn--ghost",
                    style: "color:var(--danger)",
                    disabled: cancelling(),
                    onclick: move |_| {
                        cancelling.set(true);
                        let tid = turn_id_for_cancel.clone();
                        spawn(async move {
                            let _ = crate::server::api::turn_cancel(tid).await;
                            // Brief visual hold then refresh turns list.
                            gloo_timers::future::TimeoutFuture::new(300).await;
                            cancelling.set(false);
                            turns_resource.restart();
                        });
                    },
                    if cancelling() { "…" } else { "CANCEL" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // looks_like_uuid (Phase 50.2 Plan 14, Task 3)
    // -------------------------------------------------------------------

    #[test]
    fn looks_like_uuid_true_for_a_real_uuid() {
        assert!(looks_like_uuid("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn looks_like_uuid_false_for_a_short_non_uuid_id() {
        assert!(!looks_like_uuid("zig"));
    }

    #[test]
    fn looks_like_uuid_false_for_the_handoff_label() {
        assert!(!looks_like_uuid("zig \u{b7} Bot Chat"));
    }

    #[test]
    fn looks_like_uuid_false_for_36_chars_with_a_non_hex_character() {
        // Right length, hyphens in the right places, but a non-hex 'g'
        // character in one of the digit positions.
        assert!(!looks_like_uuid("550e8400-e29b-41d4-a716-44665544000g"));
    }

    // -------------------------------------------------------------------
    // turn_session_display (Phase 50.2 Plan 14, Task 3)
    // -------------------------------------------------------------------

    #[test]
    fn turn_session_display_shortens_a_real_uuid() {
        assert_eq!(
            turn_session_display("550e8400-e29b-41d4-a716-446655440000"),
            "550e8400\u{2026}"
        );
    }

    #[test]
    fn turn_session_display_passes_the_handoff_label_through_in_full() {
        let label = "zig \u{b7} Bot Chat";
        assert_eq!(turn_session_display(label), label);
    }

    #[test]
    fn turn_session_display_passes_a_short_non_uuid_id_through_in_full() {
        assert_eq!(turn_session_display("scout"), "scout");
    }

    #[test]
    fn turn_session_display_36_chars_with_a_non_hex_character_passes_through_in_full() {
        let id = "550e8400-e29b-41d4-a716-44665544000g";
        assert_eq!(turn_session_display(id), id);
    }
}
