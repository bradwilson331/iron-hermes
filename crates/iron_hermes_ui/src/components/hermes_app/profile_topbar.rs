//! Phase 49.4 Plan 11 (D-19): the topbar active-profile indicator and
//! chat-scope quick switch.
//!
//! Mounted inside `hermes_app/mod.rs`'s `.topbar-row` (Phase 49.4 Plan 03,
//! D-22) as a flex child between `Breadcrumb` and `SysMeta` — never with a
//! `position` of its own, so the D-22 overlap fix survives this addition.
//!
//! # Closed state
//!
//! The currently active profile's name, read through
//! `profile_activation_api::get_active_profile` (Phase 49.4 Plan 08). When
//! no activation record exists — or the fetch fails, or it has not
//! resolved yet — the label falls back to `bot_meta_api::live_profile_name`,
//! the SAME environment-derived name `ironhermes_core::current_profile()`
//! resolves, so the control is never blank and never implies "nothing is
//! active" (`profile_shared::switcher::topbar_closed_label` owns this
//! selection logic and is unit-tested there).
//!
//! # Open state
//!
//! Every profile from `profile_api::list_profiles`, reusing
//! `profile_shared::switcher`'s own health-dot classification, meta line,
//! and loading/error copy (`profile_health_is_ok` / `profile_meta_line` /
//! `PROFILE_LIST_LOADING_TEXT` / `PROFILE_LIST_ERROR_TEXT`) rather than a
//! second profile-switcher implementation (D-19 prohibition: "Do not build
//! a second profile-switcher component — extend or reuse the existing
//! shared one."). The markup itself is new (`.topbar-profile-*` classes,
//! `site.css`) since the topbar's flex-row constraints and closed/open
//! shape differ from the kanban board's dropdown — matching the
//! `tools/profile_bar.rs` precedent of copying the INTERACTION pattern and
//! reusing the DATA path, while the presentation classes are net-new.
//!
//! # Chat-only scope, unconditionally
//!
//! Selecting a row calls `activate_profile` with `ActivationScope::ChatOnly`
//! UNCONDITIONALLY — the topbar never offers the everywhere scope; that
//! choice lives on the Soul page (`must_haves.prohibitions`, D-19).
//!
//! # Refresh signal — provided at the root, never here
//!
//! A successful activation bumps `TopbarProfileRefreshCtx`, a
//! `Signal<u32>` provided at the `HermesApp` root (`hermes_app/mod.rs`) —
//! never inside this module, which is itself mounted on every screen. This
//! component and the Soul page's own activation surface are siblings under
//! that root, not ancestor/descendant, so a child-level provider would
//! panic whichever consumer didn't provide it (Dioxus context-panic rule,
//! `crates/iron_hermes_ui/CLAUDE.md`). Regardless of whether a future
//! consumer subscribes to this same context, the two surfaces can never
//! disagree about which profile is active: both call the identical
//! `activate_profile` server fn, which writes the one persisted
//! `Config.active_profile` record every reader resolves through.

use crate::components::hermes_app::screens::profile_shared::switcher::{
    profile_health_is_ok, profile_meta_line, topbar_closed_label, PROFILE_LIST_ERROR_TEXT,
    PROFILE_LIST_LOADING_TEXT,
};
use crate::protocol::{ActivationScope, ProfileRow};
use crate::server::bot_meta_api::live_profile_name;
use crate::server::profile_activation_api::{activate_profile, get_active_profile};
use dioxus::prelude::*;

/// Phase 49.4 Plan 11 (D-19): shared refresh signal so the topbar's own
/// closed-state label and any other consumer (the Soul page's activation
/// surface) re-derive from the same persisted record after either surface
/// activates a profile. MUST be provided at the `HermesApp` root — see
/// this module's doc comment for the context-panic rationale.
#[allow(dead_code)] // field read via `.0` at the one provide site (mod.rs) and the one consume site (this file's TopbarProfileSwitch) — dead_code fires on isolated compilation units, same class as AuthedContext above it in mod.rs
#[derive(Clone, Copy)]
pub struct TopbarProfileRefreshCtx(pub Signal<u32>);

/// Phase 49.4 hotfix: the ONE root-provided `list_profiles` resource every
/// boot-path consumer reads (topbar, @mention roster, gateway scope
/// selector) instead of each mounting its own. Each `list_profiles` call
/// does a full `Config::load_from` per profile, so mounting several at once
/// on boot saturated the WASM client and froze the UI. Provided at the
/// `HermesApp` root and refreshed by the same [`TopbarProfileRefreshCtx`]
/// tick, so a profile activation still re-derives the list for every reader.
#[allow(dead_code)] // read via `.0` at the provide site (mod.rs) and consume sites — dead_code fires on isolated compilation units
#[derive(Clone, Copy)]
pub struct SharedProfilesCtx(pub Resource<Result<Vec<ProfileRow>, ServerFnError>>);

/// The topbar's always-visible active-profile indicator plus its
/// chat-scope quick-switch dropdown (D-19).
#[component]
pub fn TopbarProfileSwitch() -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).
    let mut menu_open: Signal<bool> = use_signal(|| false);
    let refresh_ctx = use_context::<TopbarProfileRefreshCtx>();
    let mut refresh_tick = refresh_ctx.0;

    // Phase 47.4 Plan 12 (GAP-2) idiom, mirrored from
    // `profile_shared/switcher.rs`: `refresh_tick` is READ in the SYNC
    // prefix of each `use_resource` closure (call syntax subscribes)
    // before the `async move` — a bump re-runs the fetch. Never a
    // resource-restart method call.
    let active_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { get_active_profile().await.map_err(|e| e.to_string()) }
    });
    // The environment-derived fallback name — the same value
    // `ironhermes_core::current_profile()` resolves — used whenever no
    // activation record covers the closed-state label.
    let fallback_resource = use_resource(|| async move { live_profile_name().await });
    // Phase 49.4 hotfix: read the ONE shared root-level profiles resource
    // instead of firing another `list_profiles` here — this component is
    // always mounted, so its own fetch was a permanent boot-herd member.
    // The shared resource refreshes on the same `TopbarProfileRefreshCtx`
    // tick this component bumps on activation, so the list stays current.
    let profiles_resource = use_context::<SharedProfilesCtx>().0;

    // Extract data BEFORE rsx! — signal-borrow discipline per
    // iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let is_loading = profiles_resource().is_none();
    let load_error = matches!(profiles_resource(), Some(Err(_)));
    let profiles: Vec<ProfileRow> = match profiles_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };

    // Never blank, even in the sub-second window before EITHER resource
    // has resolved — an empty label here would violate the same
    // never-blank truth this whole component exists to satisfy. Mirrors
    // SysMeta's own pre-poll/unavailable em-dash placeholder (Phase 49.4
    // Plan 03, D-22 / UI-SPEC E16) rather than inventing a second
    // "unavailable" vocabulary.
    let fallback_name = match fallback_resource() {
        Some(Ok(name)) if !name.is_empty() => name,
        _ => "\u{2014}".to_string(),
    };
    let closed_label = topbar_closed_label(active_resource().as_ref(), &fallback_name);
    let is_open = *menu_open.read();

    rsx! {
        div { class: "topbar-profile",
            button {
                class: "topbar-profile-trigger",
                "aria-label": "Active profile — {closed_label}. Open quick switch.",
                title: "{closed_label}",
                onclick: move |_| {
                    let cur = *menu_open.read();
                    menu_open.set(!cur);
                },
                span { class: "topbar-profile-name", "{closed_label}" }
                span { class: "topbar-profile-caret", "aria-hidden": "true", "▾" }
            }
            if is_open {
                div { class: "topbar-profile-menu",
                    if is_loading {
                        div { class: "topbar-profile-menu-loading", "{PROFILE_LIST_LOADING_TEXT}" }
                    } else if load_error {
                        div { class: "topbar-profile-menu-error", "{PROFILE_LIST_ERROR_TEXT}" }
                    } else {
                        for row in profiles.iter().cloned() {
                            {
                                let name_for_click = row.name.clone();
                                let dot_class = if profile_health_is_ok(&row.health) {
                                    "topbar-profile-health-dot topbar-profile-health-dot--ok"
                                } else {
                                    "topbar-profile-health-dot topbar-profile-health-dot--gap"
                                };
                                let meta = profile_meta_line(&row);
                                rsx! {
                                    div {
                                        class: "topbar-profile-menu-item",
                                        key: "{row.name}",
                                        title: "{row.name}",
                                        onclick: move |_| {
                                            let selected = name_for_click.clone();
                                            // Chat-only scope, UNCONDITIONALLY — the topbar
                                            // never offers the everywhere scope
                                            // (must_haves.prohibitions, D-19).
                                            spawn(async move {
                                                if activate_profile(selected, ActivationScope::ChatOnly)
                                                    .await
                                                    .is_ok()
                                                {
                                                    refresh_tick.set(refresh_tick() + 1);
                                                }
                                            });
                                            menu_open.set(false);
                                        },
                                        span { class: dot_class, "aria-hidden": "true" }
                                        div { class: "topbar-profile-menu-item-body",
                                            div { class: "topbar-profile-menu-item-name", "{row.name}" }
                                            div { class: "topbar-profile-menu-item-meta", "{meta}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
