//! Phase 50.4 Plan 01 (D-08/D-09/D-11/D-14): the shared "APPLY CONFIG NOW"
//! banner. Supersedes the old per-screen "Restart required" banner — D-11
//! says the amber apply-config banner appears after a successful save and
//! replaces the previous dismissible restart-required banner on that screen,
//! rather than the two coexisting.
//!
//! Renders nothing when `visible` is false (UI-SPEC apply-config-banner/empty:
//! an absence state, not a visible empty slot). While the `apply_config_now`
//! call is in flight the button reads "APPLYING…" and is disabled, and the
//! banner stays visible — it never auto-dismisses mid-call
//! (apply-config-banner/loading). On the D-09 failure path the banner shows
//! "Apply failed — the previous config is still active. {reason}" and
//! re-enables APPLY NOW for retry (apply-config-banner/error) — failure is
//! never silent and the banner never just vanishes. There is no fractional-
//! progress render state (apply-config-banner/partial): this component is
//! only ever pending, applying, failed, or gone, matching D-09's all-or-
//! nothing backend guarantee.

use dioxus::prelude::*;

use crate::server::api::apply_config_now;

/// Phase 50.4 Plan 07 (D-15 Side A): root-provided tick bumped once per
/// successful apply-now. Read in the SYNC prefix of the `get_config_summary`
/// `use_resource` closure in `hermes_app/mod.rs`, which is what makes the
/// topbar's model/provider/context-window readout refetch without a page
/// reload and without `.restart()` on a suspended `use_server_future`
/// (D-15's named trap).
///
/// This banner is mounted on three surfaces (Providers, Models, and Memory
/// screens per Plans 01/03/05), all under `HermesApp`, but consumed here
/// with `try_consume_context` rather than `use_context` on purpose: making a
/// shared component panic on mount unless one specific root provided a
/// context is a trap for the next caller, and this crate's own rule that
/// every `*Ctx` provider lives only at the `HermesApp` root means a caller
/// outside that root has no legitimate way to satisfy it locally. Without a
/// `ConfigAppliedCtx` in scope, the banner still applies config correctly —
/// the readout just doesn't auto-refresh and catches up on the next page
/// load instead.
#[allow(dead_code)] // field read via `.0` at the one provide site (mod.rs) and the one consume site (this file)
#[derive(Clone, Copy)]
pub struct ConfigAppliedCtx(pub Signal<u32>);

/// `visible` is the SAME `show_restart_banner`-style signal a screen already
/// flips to `true` after a successful config-affecting save — mounting this
/// component in that banner's old position keeps the mount point and trigger
/// unchanged while replacing what renders there.
#[component]
pub fn ApplyConfigBanner(visible: Signal<bool>) -> Element {
    let mut applying = use_signal(|| false);
    let mut failure: Signal<Option<String>> = use_signal(|| None);

    // Opportunistic, not a hook: `try_consume_context` is a plain fn
    // (dioxus-core, re-exported via dioxus::prelude), so calling it here
    // adds no hook to this component's sequence — unlike `use_context`,
    // which IS a hook. Called once at the top into a local `Option`, never
    // per-branch or inside `rsx!`, since it walks the tree on each call.
    let applied_tick: Option<ConfigAppliedCtx> = try_consume_context::<ConfigAppliedCtx>();

    // Read every needed signal into an owned local BEFORE rsx! (this crate's
    // signal-borrow rule — never hold a GenerationalRef across `.await` or
    // across `rsx!`).
    let visible_val = *visible.read();
    let applying_val = *applying.read();
    let failure_val = failure.read().clone();

    if !visible_val {
        // UI-SPEC apply-config-banner/empty: absence, not a visible empty slot.
        return rsx! {};
    }

    rsx! {
        div {
            class: "panel",
            style: "border-color:rgba(210,153,34,0.45);background:rgba(210,153,34,0.06);flex-direction:row;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;padding:10px 16px;",
            span { style: "color:var(--amber);font-size:11px;line-height:1.5;",
                "Config changed — apply now to use it on your next message."
            }
            if let Some(ref reason) = failure_val {
                // UI-SPEC apply-config-banner/long-text: the {reason} string
                // wraps inside this flex row rather than truncating — a
                // config error reason is diagnostic information the operator
                // needs in full. --danger/--fs-12 (not --red): this phase's
                // bespoke inline failure messages use the same token pair
                // memory.rs already uses; --red is reserved for screens.css
                // class rules.
                span {
                    style: "color:var(--danger);font-size:var(--fs-12);flex:1 1 240px;",
                    "Apply failed — the previous config is still active. {reason}"
                }
            }
            button {
                class: "btn btn--sm",
                disabled: applying_val,
                onclick: move |_| {
                    applying.set(true);
                    failure.set(None);
                    spawn(async move {
                        match apply_config_now().await {
                            Ok(outcome) => {
                                applying.set(false);
                                match outcome.failure_reason {
                                    Some(reason) => failure.set(Some(reason)),
                                    None => {
                                        failure.set(None);
                                        visible.set(false);
                                        // D-15 Side A: bump the applied tick so the
                                        // config-summary resource refetches. Read
                                        // into an owned local before .set(), same
                                        // shape as the subagent_events counter in
                                        // mod.rs — the borrow must not span the
                                        // rest of this async block.
                                        if let Some(mut ctx) = applied_tick {
                                            let cur = *ctx.0.read();
                                            ctx.0.set(cur + 1);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                applying.set(false);
                                failure.set(Some(e.to_string()));
                            }
                        }
                    });
                },
                if applying_val { "APPLYING…" } else { "APPLY NOW" }
            }
        }
    }
}
