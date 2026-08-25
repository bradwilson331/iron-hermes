//! Phase 48.2 Plan 05 (D-08/D-18/D-22): the Tools page header PROFILE
//! selector plus the editing-a-profile banner.
//!
//! # Interaction pattern copied, CSS not (UI-SPEC "Kanban-pattern reuse
//! note", D-18/D-19)
//!
//! The trigger + name + caret, dropdown menu, health dots, and the
//! `use_resource` sync-prefix `refresh_tick` refetch idiom are copied from
//! `profile_shared/switcher.rs`'s `ProfileSwitcher` — the SAME behavior. The
//! CSS is net-new (`tools-profile-*` classes styled against this page's
//! `site.css`/`screens.css` token set) — the kanban board's ANSI-layer
//! profile-switcher classes (see that module's own prefix) are never
//! referenced here.
//!
//! # `list_profiles()` is reused, not reimplemented
//!
//! `crate::server::profile_api::list_profiles()` (Phase 47.4) already scans
//! `$IRONHERMES_HOME/profiles/*` and covers every kanban worker AND
//! bot-mode profile — both are just directories under `profiles/`. This
//! plan adds no second profile-enumeration path (plan prohibition).
//!
//! # ROOT never blocks on a fetch
//!
//! The trigger's label is read directly from the `scope` signal — never
//! from `profiles_resource` — so `ROOT` renders immediately and the trigger
//! never shows a loading state. A failed profile-listing fetch still lets
//! the menu open with `ROOT` selectable; the failure renders as a
//! constructed message below the (possibly empty) profile list, never as a
//! reason the menu refuses to open (T-48.2-05-04: no raw error text is
//! forwarded).

use crate::protocol::{ProfileHealth, ProfileRow};
use crate::server::profile_api::list_profiles;
use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

/// D-08: map a `ConfigScope` to its display label — `ROOT` for the root
/// scope, the bare profile name otherwise. Pure and unit-tested so the
/// mapping is verifiable without a rendered component.
#[allow(dead_code)] // called from ToolsProfileBar's rsx!; dead_code fires on test target (settings_panel.rs precedent)
pub fn scope_label_for(scope: &ConfigScope) -> String {
    match scope {
        ConfigScope::Root => "ROOT".to_string(),
        ConfigScope::Profile(name) => name.clone(),
    }
}

/// D-08: map a menu-row selection — `None` for the ROOT row, `Some(name)`
/// for a profile row — to the `ConfigScope` that selecting it should set.
/// Pure and unit-tested; the inverse of `scope_label_for` for the profile
/// case.
#[allow(dead_code)] // called from ToolsProfileBar's rsx!; dead_code fires on test target (settings_panel.rs precedent)
pub fn scope_from_selection(selection: Option<&str>) -> ConfigScope {
    match selection {
        None => ConfigScope::Root,
        Some(name) => ConfigScope::Profile(name.to_string()),
    }
}

/// Header PROFILE selector (D-08/D-18): ROOT first, then every enumerated
/// profile with a health dot. Selecting a row sets `scope` directly — the
/// shell (`tools.rs`) owns bumping `refresh_tick` on an observed scope
/// change (Task 2), since this component only receives a `ReadSignal` for
/// the tick, never a mutable one.
#[component]
pub fn ToolsProfileBar(scope: Signal<ConfigScope>, refresh_tick: ReadSignal<u32>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).
    let mut menu_open: Signal<bool> = use_signal(|| false);

    // Phase 47.4 Plan 12 (GAP-2) idiom, mirrored from
    // `profile_shared/switcher.rs`: `refresh_tick` is read in the SYNC
    // prefix (call syntax subscribes) before the `async move` — a bump
    // re-runs this fetch. Never a resource-restart method call.
    let profiles_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { list_profiles().await }
    });

    // Extract every value out of the resource BEFORE the rsx! block — no
    // signal borrow held across the macro (iron_hermes_ui/clippy.toml).
    let is_loading = profiles_resource().is_none();
    let load_error: Option<String> = match profiles_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let profiles: Vec<ProfileRow> = match profiles_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };

    // The trigger label comes DIRECTLY from `scope` — never from
    // `profiles_resource` — so ROOT is always resolvable with no fetch.
    let trigger_label = scope_label_for(&scope.read());
    let is_open = *menu_open.read();

    rsx! {
        div { class: "tools-profile-bar",
            button {
                class: "tools-profile-trigger",
                "aria-label": "Select config scope — currently {trigger_label}",
                title: "{trigger_label}",
                onclick: move |_| {
                    let cur = *menu_open.read();
                    menu_open.set(!cur);
                },
                span { class: "tools-profile-trigger-label", "SCOPE " }
                span { class: "tools-profile-trigger-name", "{trigger_label}" }
                span { class: "tools-profile-trigger-caret", "aria-hidden": "true", "▾" }
            }
            if is_open {
                div { class: "tools-profile-menu",
                    // ROOT is always selectable, first, and never gated on
                    // the enumeration outcome — a failed or in-flight fetch
                    // must never make ROOT unreachable.
                    div {
                        class: "tools-profile-menu-item",
                        "aria-label": "Select ROOT config scope",
                        onclick: move |_| {
                            scope.set(scope_from_selection(None));
                            menu_open.set(false);
                        },
                        span { class: "tools-profile-menu-item-name", "ROOT" }
                    }
                    if is_loading {
                        div { class: "tools-profile-menu-loading", "Loading profiles…" }
                    } else {
                        for row in profiles.iter().cloned() {
                            {
                                let name_for_click = row.name.clone();
                                let dot_class = if row.health == ProfileHealth::Configured {
                                    "tools-profile-health-dot tools-profile-health-dot--ok"
                                } else {
                                    "tools-profile-health-dot tools-profile-health-dot--gap"
                                };
                                rsx! {
                                    div {
                                        class: "tools-profile-menu-item",
                                        key: "{row.name}",
                                        "aria-label": "Select profile {row.name} config scope",
                                        title: "{row.name}",
                                        onclick: move |_| {
                                            scope.set(scope_from_selection(Some(&name_for_click)));
                                            menu_open.set(false);
                                        },
                                        span { class: dot_class, "aria-hidden": "true" }
                                        span { class: "tools-profile-menu-item-name", "{row.name}" }
                                    }
                                }
                            }
                        }
                    }
                    // T-48.2-05-04: a constructed message only, never raw
                    // filesystem/parser error text — the menu still opened
                    // with ROOT selectable above.
                    if let Some(reason) = load_error {
                        div { class: "tools-profile-menu-error",
                            "Could not list profiles — {reason}"
                        }
                    }
                }
            }
        }
    }
}

/// Editing-a-profile banner (D-18): rendered by the shell whenever — and
/// only whenever — `scope` is a profile. Its presence at root scope is
/// literally nothing, so its presence at all is itself the signal
/// (T-48.2-05-03).
#[component]
pub fn ProfileScopeBanner(scope: Signal<ConfigScope>) -> Element {
    let profile_name: Option<String> = match &*scope.read() {
        ConfigScope::Root => None,
        ConfigScope::Profile(name) => Some(name.clone()),
    };

    rsx! {
        if let Some(name) = profile_name {
            div {
                class: "tools-profile-scope-banner",
                "aria-label": "Editing profile {name} — changes do not affect the root agent",
                span { class: "tools-profile-scope-banner-text",
                    "EDITING PROFILE "
                    strong { "{name}" }
                    " — changes here do not affect the root agent."
                }
                button {
                    class: "tools-profile-scope-banner-return",
                    "aria-label": "Return to ROOT config scope",
                    onclick: move |_| scope.set(ConfigScope::Root),
                    "RETURN TO ROOT"
                }
            }
        }
    }
}

// =============================================================================
// Pure-fn tests
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_label_for_root_is_root() {
        assert_eq!(scope_label_for(&ConfigScope::Root), "ROOT");
    }

    #[test]
    fn scope_label_for_profile_is_the_bare_name() {
        assert_eq!(
            scope_label_for(&ConfigScope::Profile("worker-1".to_string())),
            "worker-1"
        );
    }

    #[test]
    fn scope_from_selection_none_is_root() {
        assert_eq!(scope_from_selection(None), ConfigScope::Root);
    }

    #[test]
    fn scope_from_selection_some_is_the_named_profile() {
        assert_eq!(
            scope_from_selection(Some("worker-1")),
            ConfigScope::Profile("worker-1".to_string())
        );
    }
}
