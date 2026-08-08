//! Phase 47.4 Plan 01 (D-02 / D-05 / D-11): the board-header PROFILE
//! switcher.
//!
//! Task 1 (tracer) wired the trigger + the populated-row path. Task 2
//! expands this component to the full UI-SPEC State Matrix — loading /
//! empty / error / populated / partial (amber) / the `ALL PROFILES`
//! lens-clear row (rendered at every cardinality) / the `MANAGE ALL
//! PROFILES →` footer link — using the Copywriting Contract copy verbatim,
//! and lands the CSS this file's classes reference
//! (`crates/iron_hermes_ui/assets/kanban.css`, "Phase 47.4 Plan 01 Task 2"
//! block).
//!
//! Data source: `list_profiles()` via a plain `use_resource` that reads a
//! `refresh_tick: ReadSignal<u32>` prop in its SYNC prefix (Phase 47.4 Plan
//! 12, GAP-2 fix) — the codebase's own proven refresh pattern
//! (`models.rs` `roles_refresh_nonce`, Phase 46.9 Plan 15 GAP-6): bumping
//! the tick (from the wizard's `on_created` or the profile drawer's
//! `on_profile_updated`, both in `kanban.rs`) makes `use_resource` re-run
//! and re-fetch — no local snapshot signal, no seeded guard, no
//! resource-restart-method call. This replaces the plan's earlier
//! `use_server_future(...)?` + seed-once-working-copy shape, whose `?`
//! early-return was itself a hook-ordering hazard for any signal declared
//! after the resource line.
//!
//! The client-side `rsx!` here references ONLY `protocol.rs` DTOs and the
//! `#[server]` fn signature — never `ironhermes_core`, `ironhermes_kanban`,
//! or `ironhermes_vault` types, which do not exist on the wasm target.

use crate::protocol::{ProfileHealth, ProfileRow};
use crate::server::profile_api::list_profiles;
use dioxus::prelude::*;

/// Phase 47.4 Plan 01 (D-05): board-header PROFILE dropdown. `active` is
/// the D-05 assignee lens (shared with the parent `ScreenKanban`); `on_edit`
/// is invoked with a profile name when a row's `EDIT` chip — or, per D-02,
/// the "manage all profiles" footer link — is activated. The parent sets
/// its detail-drawer target signal in response.
#[component]
pub fn ProfileSwitcher(
    active: Signal<Option<String>>,
    on_edit: EventHandler<String>,
    refresh_tick: ReadSignal<u32>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).

    // Phase 47.4 Plan 01: menu open/close toggle — the crate's own
    // `*_open` idiom (mirrors `create_modal_open` in `kanban.rs`).
    let mut menu_open: Signal<bool> = use_signal(|| false);

    // Phase 47.4 Plan 12 (GAP-2): `refresh_tick` is READ in the SYNC
    // prefix of this `use_resource` closure (call syntax subscribes),
    // before the `async move` — the same shape as `models.rs`'s
    // `roles_refresh_nonce`. A bump therefore re-runs this fetch.
    let profiles_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { list_profiles().await }
    });

    // Extract data BEFORE rsx! — signal-borrow discipline per
    // iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX). The
    // rendered snapshot comes DIRECTLY from the resource each render —
    // never a seed-once local signal (models.rs's stated rule).
    let is_loading = profiles_resource().is_none();
    let load_error = matches!(profiles_resource(), Some(Err(_)));
    let profiles: Vec<ProfileRow> = match profiles_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };
    let profiles_empty = profiles.is_empty();
    let trigger_label = active
        .read()
        .clone()
        .unwrap_or_else(|| "ALL PROFILES".to_string());
    let is_open = *menu_open.read();
    // D-02: the footer link is not a dead link — it opens the drawer
    // scoped to the currently active profile, or the first row when no
    // lens is active. Not rendered when zero profiles exist (nothing to
    // manage).
    let footer_target: Option<String> = active
        .read()
        .clone()
        .or_else(|| profiles.first().map(|r| r.name.clone()));

    rsx! {
        div { class: "kn-profile-switcher-wrap",
            // Per UI-SPEC: the trigger carries the literal text "PROFILE"
            // plus the active profile name, so it is NOT icon-only and
            // needs no aria-label.
            button {
                class: "kn-profile-trigger",
                onclick: move |_| {
                    let cur = *menu_open.read();
                    menu_open.set(!cur);
                },
                span { class: "kn-profile-trigger-label", "PROFILE " }
                span { class: "kn-profile-trigger-name", "{trigger_label}" }
                span { class: "kn-profile-trigger-caret", "aria-hidden": "true", "▾" }
            }
            if is_open {
                div { class: "kn-profile-menu",
                    div { class: "kn-profile-menu-header", "EXISTING PROFILES" }
                    // ALL PROFILES lens-clear row — rendered above the
                    // profile list at every cardinality, including the
                    // empty state (resolved discretion, D-05: the canvas
                    // has no lens-clear affordance of its own).
                    div {
                        class: "kn-profile-menu-item kn-profile-menu-item--all",
                        onclick: move |_| {
                            active.set(None);
                            menu_open.set(false);
                        },
                        span { class: "kn-profile-menu-item-name", "ALL PROFILES" }
                    }
                    if is_loading {
                        div { class: "kn-profile-menu-loading kn-drawer-loading", "Loading profiles…" }
                    } else if load_error {
                        div { class: "kn-modal-error",
                            "Could not read ~/.ironhermes/profiles/. Check permissions and retry."
                        }
                    } else if profiles_empty {
                        div { class: "kn-drawer-empty",
                            div { "No profiles yet." }
                            div {
                                "Create one to give a kanban worker its own config.yaml and .env — press + NEW PROFILE above."
                            }
                        }
                    } else {
                        for row in profiles.iter().cloned() {
                            {
                                let name_for_row = row.name.clone();
                                let name_for_edit = row.name.clone();
                                let dot_class = if row.health == ProfileHealth::Configured {
                                    "kn-health-dot kn-health-dot--ok"
                                } else {
                                    "kn-health-dot kn-health-dot--gap"
                                };
                                let meta = if row.health == ProfileHealth::Configured {
                                    format!(
                                        "{} · {} · {} keys",
                                        row.provider.clone().unwrap_or_default(),
                                        row.model_default.clone().unwrap_or_default(),
                                        row.key_count,
                                    )
                                } else {
                                    row.gaps
                                        .first()
                                        .map(|g| g.meta_label().to_string())
                                        .unwrap_or_default()
                                };
                                rsx! {
                                    div {
                                        class: "kn-profile-menu-item",
                                        key: "{row.name}",
                                        onclick: move |_| {
                                            active.set(Some(name_for_row.clone()));
                                            menu_open.set(false);
                                        },
                                        span { class: dot_class, "aria-hidden": "true" }
                                        div { class: "kn-profile-menu-item-body",
                                            div { class: "kn-profile-menu-item-name", "{row.name}" }
                                            div { class: "kn-profile-menu-item-meta", "{meta}" }
                                        }
                                        button {
                                            class: "kn-profile-edit-chip",
                                            onclick: move |e| {
                                                e.stop_propagation();
                                                on_edit.call(name_for_edit.clone());
                                            },
                                            "EDIT"
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // D-02: not a dead link — opens the profile detail
                    // drawer scoped to the active (or first) profile. Not
                    // rendered when there is nothing to manage.
                    if let Some(target) = footer_target {
                        button {
                            class: "kn-profile-menu-footer",
                            onclick: move |_| {
                                on_edit.call(target.clone());
                                menu_open.set(false);
                            },
                            "MANAGE ALL PROFILES →"
                        }
                    }
                }
            }
        }
    }
}
