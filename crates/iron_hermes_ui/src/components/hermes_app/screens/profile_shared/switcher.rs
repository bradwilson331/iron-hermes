//! Phase 47.4 Plan 01 (D-02 / D-05 / D-11): the board-header PROFILE
//! switcher.
//!
//! Phase 50.1 Plan 02 (D-10): relocated from
//! `screens/kanban/profile_switcher.rs` into this shared module alongside
//! `create_dialog.rs`/`edit_dialog.rs` — `screens/kanban/profile_switcher.rs`
//! is now a thin re-export shim at the old path. This file's own signature
//! is unchanged by the lift; Kanban is its only caller today.
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

use crate::protocol::{ActiveProfileRecord, ProfileHealth, ProfileRow};
use crate::server::profile_api::list_profiles;
use dioxus::prelude::*;

/// Phase 49.4 Plan 11 (D-19): whether `health` renders the OK (green) dot
/// vs the GAP (amber) dot. Extracted from `ProfileSwitcher`'s own inline
/// per-row check below so the topbar quick-switch
/// (`crate::components::hermes_app::profile_topbar::TopbarProfileSwitch`)
/// can reuse the identical ok/gap classification without a second
/// implementation (D-19 prohibition: "Do not build a second
/// profile-switcher component — extend or reuse the existing shared
/// one."). `ProfileSwitcher`'s rendering below now calls this instead of
/// repeating the check inline — rendered output is unchanged.
pub(crate) fn profile_health_is_ok(health: &ProfileHealth) -> bool {
    *health == ProfileHealth::Configured
}

/// Phase 49.4 Plan 11 (D-19): the row's meta line — provider/model/key
/// count when configured, the first health gap's label otherwise.
/// Extracted verbatim from `ProfileSwitcher`'s own per-row computation for
/// the same reuse reason as [`profile_health_is_ok`] above.
pub(crate) fn profile_meta_line(row: &ProfileRow) -> String {
    if profile_health_is_ok(&row.health) {
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
    }
}

/// Phase 49.4 Plan 11 (D-19 / UI-SPEC E9-equivalent loading & error rows):
/// the exact loading/error copy `ProfileSwitcher`'s dropdown renders below,
/// reused verbatim by the topbar quick-switch so a profile-list fetch
/// failure or an unresolved fetch never grows a second error/loading
/// surface ("List fetch failure reuses the existing switcher error state
/// from profile_shared — no new error surface").
pub(crate) const PROFILE_LIST_LOADING_TEXT: &str = "Loading profiles…";
pub(crate) const PROFILE_LIST_ERROR_TEXT: &str =
    "Could not read ~/.ironhermes/profiles/. Check permissions and retry.";

/// Phase 49.4 Plan 11 (D-19): the topbar's closed-state label — the
/// currently active profile's name when a persisted activation record
/// exists and resolved successfully, the environment-derived fallback name
/// for every other outcome (no record persisted, a fetch failure, or the
/// fetch not yet resolved). Never returns an empty label when
/// `fallback_name` is non-empty — the topbar control must never render a
/// blank control or a placeholder implying nothing is active
/// (`must_haves.truths`, D-19).
pub(crate) fn topbar_closed_label(
    active_record_fetch: Option<&Result<Option<ActiveProfileRecord>, String>>,
    fallback_name: &str,
) -> String {
    match active_record_fetch {
        Some(Ok(Some(record))) => record.name.clone(),
        _ => fallback_name.to_string(),
    }
}

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
                        div { class: "kn-profile-menu-loading kn-drawer-loading", "{PROFILE_LIST_LOADING_TEXT}" }
                    } else if load_error {
                        div { class: "kn-modal-error", "{PROFILE_LIST_ERROR_TEXT}" }
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
                                let dot_class = if profile_health_is_ok(&row.health) {
                                    "kn-health-dot kn-health-dot--ok"
                                } else {
                                    "kn-health-dot kn-health-dot--gap"
                                };
                                let meta = profile_meta_line(&row);
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

// =============================================================================
// Pure-fn tests (Phase 49.4 Plan 11, D-19): the shared helpers extracted
// for the topbar quick-switch to reuse without a second implementation.
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ActivationScope;

    fn configured_row(name: &str) -> ProfileRow {
        ProfileRow {
            name: name.to_string(),
            health: ProfileHealth::Configured,
            gaps: Vec::new(),
            provider: Some("anthropic".to_string()),
            model_default: Some("claude-opus".to_string()),
            key_count: 2,
        }
    }

    fn incomplete_row(name: &str) -> ProfileRow {
        ProfileRow {
            name: name.to_string(),
            health: ProfileHealth::Incomplete,
            gaps: vec![crate::protocol::ProfileGap::NoResolvableKey],
            provider: None,
            model_default: None,
            key_count: 0,
        }
    }

    #[test]
    fn profile_health_is_ok_true_for_configured() {
        assert!(profile_health_is_ok(&ProfileHealth::Configured));
    }

    #[test]
    fn profile_health_is_ok_false_for_incomplete() {
        assert!(!profile_health_is_ok(&ProfileHealth::Incomplete));
    }

    #[test]
    fn profile_meta_line_configured_shows_provider_model_and_key_count() {
        let row = configured_row("chat-bot");
        assert_eq!(profile_meta_line(&row), "anthropic · claude-opus · 2 keys");
    }

    #[test]
    fn profile_meta_line_incomplete_shows_first_gap_label() {
        let row = incomplete_row("gap-bot");
        assert_eq!(profile_meta_line(&row), "no resolvable key");
    }

    // -------------------------------------------------------------------
    // topbar_closed_label — record-present, record-absent, fetch-failed.
    // -------------------------------------------------------------------

    #[test]
    fn topbar_closed_label_record_present_returns_the_record_name() {
        let record = ActiveProfileRecord {
            name: "activated-bot".to_string(),
            scope: ActivationScope::ChatOnly,
            updated_at_ms: 1,
        };
        let fetch: Result<Option<ActiveProfileRecord>, String> = Ok(Some(record));
        assert_eq!(
            topbar_closed_label(Some(&fetch), "env-derived-bot"),
            "activated-bot"
        );
    }

    #[test]
    fn topbar_closed_label_record_absent_returns_the_fallback_name() {
        let fetch: Result<Option<ActiveProfileRecord>, String> = Ok(None);
        assert_eq!(
            topbar_closed_label(Some(&fetch), "env-derived-bot"),
            "env-derived-bot"
        );
    }

    #[test]
    fn topbar_closed_label_fetch_failed_returns_the_fallback_name() {
        let fetch: Result<Option<ActiveProfileRecord>, String> = Err("boom".to_string());
        assert_eq!(
            topbar_closed_label(Some(&fetch), "env-derived-bot"),
            "env-derived-bot"
        );
    }

    #[test]
    fn topbar_closed_label_still_loading_returns_the_fallback_name() {
        assert_eq!(
            topbar_closed_label(None, "env-derived-bot"),
            "env-derived-bot"
        );
    }

    #[test]
    fn topbar_closed_label_never_blank_when_fallback_is_non_empty() {
        let fetch: Result<Option<ActiveProfileRecord>, String> = Ok(None);
        let label = topbar_closed_label(Some(&fetch), "default");
        assert!(!label.is_empty());
    }
}
