//! Gateway screen — ported from `app.html` `<section id="screen-gateway">`
//! (lines 1150-1263).
//!
//! Phase 49.3 Plan 01 (tracer slice, D-06/D-07/D-08/D-09): restructured
//! from the prior 87-line `gateway.rs` stub (which rendered the crate's
//! mock platform-card helper — "Pure visual stub (D-04) — zero server
//! calls") into this `gateway/` submodule. The Telegram card is now LIVE
//! end-to-end (browser -> `#[server]` fn -> `config.gateway.platforms` ->
//! DTO -> card); Discord/Slack/Buzz/webhook-route/REST-API cards land in
//! later plans of this phase (03/04/05/06), each owning its own disjoint
//! child file so they can run in parallel without re-touching this file.
//!
//! # Component skeleton (Task 1)
//!
//! `mod.rs` owns the screen shell (header, D-07 scope selector, `.grid.wide`,
//! RESTART ALL, + ADD PLATFORM) and calls one child per grid section:
//! [`chat_platform_cards::ChatPlatformCards`],
//! [`webhook_route_cards::WebhookRouteCards`],
//! [`api_server_card::ApiServerCard`], [`teaser_cards::TeaserCards`], plus
//! [`webhook_wizard::AddRouteWizard`] as an always-mounted modal bound to a
//! `wizard_open: Signal<bool>`. Every child's prop SIGNATURE established
//! here is the CONTRACT expansion plans 03/04/05/06 fill the BODY of
//! without changing — see each child module's own doc comment.
//! `chat_config_form.rs`/`whitelist_editor.rs` are nested children of
//! `chat_platform_cards.rs` (not called from this file); they exist as
//! compiling empty stubs starting Plan 01, wired starting Plan 03.
//!
//! # D-09 — no auto-restart on save
//!
//! A config write (the Telegram ENABLED toggle, Task 2) never triggers a
//! restart. The write path shows "SAVED — RESTART TO APPLY" on the card;
//! the operator restarts explicitly via RESTART ALL (Task 2), which calls
//! the existing gated/cooled-down/audited
//! `gateway_control_api::restart_gateway` — no second process-control path
//! is introduced anywhere in this phase.
//!
//! # D-08 status source (Plan 06: heartbeat-first, pidfile fallback)
//!
//! Per-card status comes from `gateway_platform_status_api::read_platform_status`
//! — heartbeat-first (real per-platform `connected` + `session_count` from
//! the gateway process's periodic status file), falling back to the SAME
//! `gateway_status_api::get_gateway_runtime_status` 6-state pidfile
//! liveness the Tools page RUNTIME section uses when the heartbeat is
//! absent/stale. This is the ONE call site in this phase that reads
//! gateway liveness.

mod api_server_card;
mod chat_config_form;
mod chat_platform_cards;
mod schedules_card;
mod teaser_cards;
mod webhook_route_cards;
// pub(crate), unlike every sibling above: CR-02's combined-contract test
// (`webhook_route_api.rs`'s `mod tests`) must call `save_intent`/`SaveIntent`
// directly to prove the client predicate's own output is accepted by the
// server impl — the test class whose absence let CR-02 ship.
pub(crate) mod webhook_wizard;
mod whitelist_editor;

use api_server_card::ApiServerCard;
use chat_platform_cards::ChatPlatformCards;
use schedules_card::GatewaySchedulesCard;
use teaser_cards::TeaserCards;
use webhook_route_cards::WebhookRouteCards;
use webhook_wizard::AddRouteWizard;

use crate::server::gateway_control_api::{restart_gateway, GatewayLifecycleOutcome};
use crate::server::gateway_platform_status_api::PlatformStatusMap;
use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

/// Client-side mirror of `gateway_control_api::ACTION_COOLDOWN` (3s) — that
/// constant is private to its own module and this screen has no server-
/// truthful "seconds remaining" read (Task 2's action text leaves the
/// choice to Claude's discretion; this plan uses the fixed-countdown
/// option). If the server-side cooldown value ever changes, this constant
/// must be updated to match — it is NOT derived from the server.
#[allow(dead_code)] // consumed in ScreenGateway's cooldown closure (mod.rs ~210); dead_code fires under --all-features (mutually-exclusive renderer features cfg the call site out)
const RESTART_COOLDOWN_SECS: u32 = 3;

#[component]
pub fn ScreenGateway(is_active: bool) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).

    // D-07: the same root/profile ConfigScope selector as the Tools page —
    // lifted idiom (tools.rs Signal<ConfigScope> + refresh_tick +
    // last_scope_for_tick), not a shared component import (tools.rs's
    // `profile_bar` submodule is private to that screen).
    let scope: Signal<ConfigScope> = use_signal(|| ConfigScope::Root);
    let mut refresh_tick: Signal<u32> = use_signal(|| 0);
    let mut last_scope_for_tick: Signal<Option<ConfigScope>> = use_signal(|| None);
    use_effect(move || {
        let current = scope();
        let previous = last_scope_for_tick.peek().clone();
        last_scope_for_tick.set(Some(current.clone()));
        if let Some(prev) = previous {
            if prev != current {
                let next = *refresh_tick.peek() + 1;
                refresh_tick.set(next);
            }
        }
    });

    // + ADD PLATFORM opens the (currently empty) webhook route wizard —
    // Plan 04 fills the wizard body; this signal is the established
    // contract this plan wires end to end.
    let mut wizard_open: Signal<bool> = use_signal(|| false);

    // D-08 (Plan 06): per-card status assembly — heartbeat-first,
    // pidfile-fallback (`read_platform_status`), re-fetched on scope/tick
    // change (47.4 Plan 12 GAP-2 sync-prefix idiom).
    let status_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move {
            crate::server::gateway_platform_status_api::read_platform_status(scope_value).await
        }
    });
    let platform_status: Option<PlatformStatusMap> = match status_resource() {
        Some(Ok(s)) => Some(s),
        _ => None,
    };

    // Explicit `Signal` -> `ReadSignal` conversions as named bindings —
    // calling `.into()` inline inside the rsx! prop position is ambiguous
    // under dioxus 0.7's `SuperInto` (multiple candidate impls), so the
    // conversion happens once here instead of at every call site below.
    let scope_ro: ReadSignal<ConfigScope> = scope.into();

    // Task 2: RESTART ALL — calls the EXISTING gated/cooled-down/audited
    // `gateway_control_api::restart_gateway`; no second process-control
    // path. `cooldown_remaining` drives the "RESTART ALL — COOLING DOWN
    // ({N}s)" disabled state via a fixed client-side countdown from
    // `RESTART_COOLDOWN_SECS` (D-09/E9).
    let restarting: Signal<bool> = use_signal(|| false);
    let cooldown_remaining: Signal<u32> = use_signal(|| 0);
    let restart_outcome: Signal<Option<GatewayLifecycleOutcome>> = use_signal(|| None);

    let restarting_val = *restarting.read();
    let cooldown_val = *cooldown_remaining.read();
    let restart_disabled = restarting_val || cooldown_val > 0;
    let restart_label = if restarting_val {
        "RESTARTING…".to_string()
    } else if cooldown_val > 0 {
        format!("RESTART ALL — COOLING DOWN ({cooldown_val}s)")
    } else {
        "↻ RESTART ALL".to_string()
    };
    // E9 error: a gate-denied/blocked response surfaces as a `--red`
    // banner — never silent (D-09's "blocked attempts are audited AND
    // surfaced" carry-forward from 48.2).
    let restart_outcome_val = restart_outcome.read().clone();
    let restart_is_refusal = matches!(
        restart_outcome_val,
        Some(
            GatewayLifecycleOutcome::GateClosed { .. }
                | GatewayLifecycleOutcome::CooldownActive
                | GatewayLifecycleOutcome::RefusedOtherUser { .. }
                | GatewayLifecycleOutcome::RefusedInvalidTarget
                | GatewayLifecycleOutcome::RefusedAlreadyRunning { .. }
                | GatewayLifecycleOutcome::InvalidScope { .. }
                | GatewayLifecycleOutcome::UnsupportedPlatform
                | GatewayLifecycleOutcome::InternalError { .. }
                | GatewayLifecycleOutcome::StopNotConfirmed { .. }
                | GatewayLifecycleOutcome::StoppedButNotRestarted { .. }
                | GatewayLifecycleOutcome::StartNotConfirmed { .. }
                | GatewayLifecycleOutcome::SpawnFailed
        )
    );
    let restart_outcome_line: Option<String> =
        restart_outcome_val.as_ref().map(restart_outcome_text);

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-gateway",
            "data-screen-label": "10 Gateway",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 10" }
                    h1 { class: "screen-title", "Gateway" }
                    GatewayScopeSelector { scope }
                }
                div { class: "screen-actions",
                    button {
                        class: "btn btn--ghost btn--sm",
                        disabled: restart_disabled,
                        "aria-label": "Restart all gateway platforms",
                        onclick: move |_| {
                            if *restarting.peek() || *cooldown_remaining.peek() > 0 {
                                return;
                            }
                            let scope_value = scope.read().clone();
                            let mut restarting_sig = restarting;
                            let mut outcome_sig = restart_outcome;
                            let mut cooldown_sig = cooldown_remaining;
                            let mut refresh_tick_sig = refresh_tick;
                            restarting_sig.set(true);
                            spawn(async move {
                                let outcome = restart_gateway(scope_value).await.unwrap_or(
                                    GatewayLifecycleOutcome::InternalError {
                                        reason: "the restart request itself failed".to_string(),
                                    },
                                );
                                restarting_sig.set(false);
                                outcome_sig.set(Some(outcome));
                                let cur = *refresh_tick_sig.read();
                                refresh_tick_sig.set(cur + 1);
                                // Client-side cooldown countdown — 1Hz tick
                                // (app_footer.rs's cross-platform sleep
                                // idiom: gloo_timers on wasm, tokio off it).
                                cooldown_sig.set(RESTART_COOLDOWN_SECS);
                                loop {
                                    let remaining = *cooldown_sig.peek();
                                    if remaining == 0 {
                                        break;
                                    }
                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        gloo_timers::future::TimeoutFuture::new(1000).await;
                                    }
                                    #[cfg(not(target_arch = "wasm32"))]
                                    {
                                        tokio::time::sleep(std::time::Duration::from_millis(1000))
                                            .await;
                                    }
                                    cooldown_sig.set(remaining.saturating_sub(1));
                                }
                            });
                        },
                        "{restart_label}"
                    }
                    button {
                        class: "btn btn--sm",
                        "aria-label": "Add a new gateway platform or webhook route",
                        onclick: move |_| {
                            let cur = *wizard_open.read();
                            wizard_open.set(!cur);
                        },
                        "+ ADD PLATFORM"
                    }
                }
            }

            if let Some(line) = restart_outcome_line {
                div {
                    class: if restart_is_refusal { "pill red" } else { "pill" },
                    "{line}"
                }
            }

            div { class: "grid wide",
                // Schedules leads the grid as a full-width banner
                // (`.plat-card--full`) — operator request, 2026-09-01. It
                // stays INSIDE `.grid.wide` rather than being hoisted above
                // it so it scrolls with the other cards rather than pinning
                // to the screen.
                GatewaySchedulesCard { scope: scope_ro, refresh_tick }
                ChatPlatformCards {
                    scope: scope_ro,
                    refresh_tick,
                    platform_status: platform_status.clone(),
                }
                WebhookRouteCards { scope: scope_ro, refresh_tick }
                ApiServerCard { scope: scope_ro, refresh_tick }
                TeaserCards { scope: scope_ro }
            }

            AddRouteWizard {
                open: wizard_open,
                scope: scope_ro,
                refresh_tick,
            }
        }
    }
}

/// D-07 header scope selector — a native `<select>` of ROOT + every
/// enumerated profile (E8: overflow scrolls natively, long names truncate
/// via CSS, never a custom menu). Lifted idiom from `tools.rs`'s
/// `Signal<ConfigScope>`/`refresh_tick` pattern; `tools::profile_bar` is a
/// private submodule of the Tools screen so its component is not directly
/// importable here — this is a from-scratch component over the SAME
/// `ConfigScope` type and the same `list_profiles()` read, not a second
/// profile-enumeration path.
#[component]
fn GatewayScopeSelector(mut scope: Signal<ConfigScope>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    // Phase 49.4 hotfix: read the ONE shared root-level profiles resource
    // rather than firing another `list_profiles` on mount. The profile LIST
    // does not change on a scope switch (only the selected scope does), so a
    // scope-tick-driven refetch here was pure duplicate boot-herd cost.
    let profiles_resource =
        use_context::<crate::components::hermes_app::profile_topbar::SharedProfilesCtx>().0;

    // E8 loading: the selector is disabled while its own profile listing
    // fetch is in flight (a scope-switch bump re-triggers this same
    // resource via `refresh_tick`, keeping the disabled window aligned
    // with "a scope-switch fetch is in flight").
    let is_loading = profiles_resource().is_none();
    let profiles: Vec<crate::protocol::ProfileRow> = match profiles_resource() {
        Some(Ok(rows)) => rows,
        // E8 error: a failed fetch still renders ROOT + whatever the
        // selection already was — never a broken/blank control.
        _ => Vec::new(),
    };

    let current_value = match &*scope.read() {
        ConfigScope::Root => "ROOT".to_string(),
        ConfigScope::Profile(name) => name.clone(),
    };

    rsx! {
        select {
            class: "gw-scope-select",
            "aria-label": "Select config scope — currently {current_value}",
            title: "{current_value}",
            disabled: is_loading,
            value: "{current_value}",
            onchange: move |evt| {
                let v = evt.value();
                if v == "ROOT" {
                    scope.set(ConfigScope::Root);
                } else {
                    scope.set(ConfigScope::Profile(v));
                }
            },
            option { value: "ROOT", "ROOT" }
            for row in profiles.iter() {
                option { key: "{row.name}", value: "{row.name}", "{row.name}" }
            }
        }
    }
}

/// RESTART ALL's result-line text for `outcome` — never a raw OS error
/// (the outcome DTO itself carries none, `gateway_control_api`'s own
/// T-48.2-13-11 contract). Mirrors `tools/runtime_section.rs`'s
/// `runtime_outcome_line` fn (not imported directly — that fn is private
/// to the Tools screen's `runtime_section` submodule).
#[allow(dead_code)] // consumed in ScreenGateway's outcome memo (mod.rs ~164); dead_code fires under --all-features (mutually-exclusive renderer features cfg the call site out)
fn restart_outcome_text(outcome: &GatewayLifecycleOutcome) -> String {
    use GatewayLifecycleOutcome::*;
    match outcome {
        Stopped { pid } => format!("STOPPED — pid {pid} confirmed gone."),
        StopNotConfirmed { pid } => {
            format!("SIGTERM sent to pid {pid}, but death was not confirmed in time.")
        }
        NotRunning => "Nothing was running — nothing to restart.".to_string(),
        RefusedOtherUser { pid } => format!("REFUSED — pid {pid} is owned by another user."),
        RefusedInvalidTarget => "REFUSED — the recorded pid failed validation.".to_string(),
        RefusedAlreadyRunning { pid } => {
            format!("REFUSED — a gateway is already running (pid {pid}).")
        }
        Started { pid, log_path } => format!("STARTED — pid {pid} confirmed live. Log: {log_path}"),
        StartNotConfirmed { log_path } => {
            format!("Spawned, but not confirmed live in time. Log: {log_path}")
        }
        SpawnFailed => "The gateway process could not be spawned.".to_string(),
        StoppedButNotRestarted { pid } => format!(
            "STOPPED but NOT restarted — pid {pid}'s death was not confirmed, so start was not attempted."
        ),
        GateClosed { message } => format!("REFUSED — {message}."),
        CooldownActive => "REFUSED — please wait a moment before trying again.".to_string(),
        InvalidScope { message } => format!("REFUSED — {message}."),
        UnsupportedPlatform => {
            "REFUSED — this platform cannot control the gateway process.".to_string()
        }
        InternalError { reason } => format!("REFUSED — {reason}."),
    }
}
