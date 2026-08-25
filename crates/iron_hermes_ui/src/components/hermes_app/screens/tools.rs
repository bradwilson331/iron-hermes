//! Tools screen — Phase 48.2 Plan 01 tracer: rebuilt as a live, scope-aware,
//! write-gated control surface for the `tools:` section of config.yaml.
//!
//! Replaces the pre-48.2 read-only card grid (`list_tools()` +
//! deliberately-inert `.tgl` toggles). This shell owns the page's
//! `ConfigScope` signal and the `refresh_tick` re-fetch idiom
//! (`profile_shared/switcher.rs` precedent — `use_resource`'s sync prefix
//! reads both signals before the `async move` so a bump re-fetches without
//! calling a resource-restart method).
//!
//! Card rendering + the state matrix (loading/error/populated/amber/
//! zero-match) live in `tools::catalog`.
//!
//! Phase 48.2 Plan 05 (D-08/D-18): `tools::profile_bar` mounts the header
//! PROFILE selector and the editing-a-profile banner, both bound to this
//! shell's `scope` signal — every mounted section already read `scope` in
//! its own `use_resource` sync prefix (Plans 01/02/03/04), so selecting a
//! profile here re-scopes the whole page without a page reload.

mod bulk_confirm;
mod buzz_section;
mod catalog;
mod credential_form;
mod import_wizard;
mod mcp_section;
mod profile_bar;
mod runtime_section;
mod settings_panel;
mod toolbar;

use crate::components::hermes_app::screens::tools::bulk_confirm::{
    BulkAction, BulkConfirmDialog, BulkOutcomeReport, BulkResultBanner,
};
use crate::components::hermes_app::screens::tools::buzz_section::BuzzSection;
use crate::components::hermes_app::screens::tools::catalog::{
    CatalogErrorBanner, CatalogSkeleton, EmptyFilterState, FavoritesRow, ToolsetSection,
};
use crate::components::hermes_app::screens::tools::mcp_section::McpSection;
use crate::components::hermes_app::screens::tools::profile_bar::{
    ProfileScopeBanner, ToolsProfileBar,
};
use crate::components::hermes_app::screens::tools::runtime_section::RuntimeSection;
use crate::components::hermes_app::screens::tools::settings_panel::ToolsSettingsPanel;
use crate::components::hermes_app::screens::tools::toolbar::{filter_groups, ToolsToolbar};
use crate::server::gateway_status_api::GatewayRuntimeStatus;
use crate::server::tools_config_api::{ConfigScope, ToolAvailability, ToolsPageState};
use crate::ui_prefs::ToolFavorites;
use dioxus::prelude::*;

/// Phase 48.2 Plan 01: stylesheet for the Tools page — extends the
/// `site.css`/`screens.css` token layer in place (UI-SPEC "Which CSS layer
/// this phase lives in"), mirroring `kanban.rs`'s `KANBAN_CSS` per-screen
/// registration pattern.
#[allow(dead_code)] // used in ScreenTools rsx! document::Link; dead_code fires on test target
const TOOLS_CSS: Asset = asset!("/assets/tools.css");

/// UI-SPEC Copywriting Contract — verbatim, never truncated (D-10).
#[allow(dead_code)] // used in ScreenTools rsx!; dead_code fires on test target (KANBAN_CSS precedent)
const READ_ONLY_BANNER: &str =
    "READ-ONLY — set security.web_config_write_enabled: true in config.yaml to edit this page.";

#[component]
pub fn ScreenTools(is_active: bool) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).
    let scope: Signal<ConfigScope> = use_signal(|| ConfigScope::Root);
    let mut refresh_tick: Signal<u32> = use_signal(|| 0);

    // Phase 48.2 Plan 06 (D-15): search/favorites toolbar state. `pinned`
    // is seeded ONCE from localStorage at mount — every subsequent read/
    // write goes through the signal, never a second `read_tool_favorites`
    // call, so the shell stays the single source of truth for the pinned
    // set during this page's lifetime.
    let query: Signal<String> = use_signal(String::new);
    let favorites_only: Signal<bool> = use_signal(|| false);
    let pinned: Signal<ToolFavorites> = use_signal(crate::ui_prefs::read_tool_favorites);

    // Phase 48.2 Plan 06 Task 3 (D-17): ENABLE ALL / DISABLE ALL bulk
    // toggle state. `bulk_dialog` holds which confirm is open (if any);
    // `bulk_report` holds the post-submission result, rendered AFTER the
    // dialog closes (must_haves: the dialog closes and the PAGE shows the
    // per-toolset result).
    let mut bulk_dialog: Signal<Option<BulkAction>> = use_signal(|| None);
    let mut bulk_report: Signal<Option<BulkOutcomeReport>> = use_signal(|| None);

    // Phase 48.2 Plan 05: bump `refresh_tick` whenever `scope` actually
    // changes (never on initial mount), so every resource that ALSO reads
    // the tick in its sync prefix re-runs exactly once instead of showing
    // one frame of the previous scope's data paired with the new tick.
    // `ToolsProfileBar` only receives a `ReadSignal<u32>` for the tick (it
    // mutates `scope` directly), so the shell — not the profile bar — owns
    // this bump. `last_scope_for_tick` is read via `.peek()` inside the
    // effect so setting it never re-triggers this same effect (models.rs
    // `roles_refresh_nonce` precedent: explicit bump at the point of
    // change, not a seed-once-guarded effect).
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

    // Phase 47.4 Plan 12 (GAP-2) idiom, mirrored from
    // `profile_shared/switcher.rs`: read both signals in the SYNC prefix
    // (call syntax subscribes) before the `async move` — a bump on either
    // re-runs this fetch. Never call a resource-restart method.
    let page_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { crate::server::tools_config_api::get_tools_page_state(scope_value).await }
    });

    // Extract every value out of the resource BEFORE the rsx! block — no
    // signal borrow held across the macro (iron_hermes_ui/clippy.toml).
    let is_loading = page_resource().is_none();
    let load_error: Option<String> = match page_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let page_state: Option<ToolsPageState> = match page_resource() {
        Some(Ok(s)) => Some(s),
        _ => None,
    };
    let gate_open = page_state.as_ref().map(|s| s.gate_open).unwrap_or(false);
    let writable = gate_open;

    // Phase 48.2 Plan 03: flat list of every registered tool name, fed to
    // the settings panel's per-tool timeout-override selector. Derived
    // from the already-loaded catalog rather than a second fetch.
    let known_tool_names: Vec<String> = page_state
        .as_ref()
        .map(|s| {
            s.toolsets
                .iter()
                .flat_map(|g| g.tools.iter())
                .map(|t| t.name.clone())
                .collect()
        })
        .unwrap_or_default();
    // Phase 48.2 Plan 07 (D-05/D-16): every env_var-shaped credential key
    // named by a currently-unmet prerequisite anywhere in the (unfiltered)
    // catalog, deduped and sorted — fed to the settings panel's
    // credentials section alongside CANONICAL_TOOL_CREDENTIAL_KEYS so an
    // operator can fix a missing key away from its specific card too.
    // Derived from the already-loaded catalog, never a second fetch.
    let missing_credential_keys: Vec<String> = {
        let mut keys: Vec<String> = page_state
            .as_ref()
            .map(|s| {
                s.toolsets
                    .iter()
                    .flat_map(|g| g.tools.iter())
                    .filter_map(|t| match &t.availability {
                        ToolAvailability::Unavailable { missing } => Some(
                            missing
                                .iter()
                                .filter(|m| m.kind == "env_var")
                                .map(|m| m.name.clone())
                                .collect::<Vec<String>>(),
                        ),
                        _ => None,
                    })
                    .flatten()
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        keys.sort();
        keys.dedup();
        keys
    };
    // Phase 48.2 Plan 06 (D-15): apply the toolbar filter to the fetched
    // catalog BEFORE it reaches `ToolsetSection` — filtering is entirely
    // client-side over the already-fetched groups (no server call per
    // keystroke). `pinned_snapshot` releases the signal borrow before the
    // rsx! block (clippy.toml: no signal borrow spans the macro).
    let pinned_snapshot: ToolFavorites = pinned.read().clone();
    let query_value = query();
    let favorites_only_value = favorites_only();
    let filter_active = !query_value.is_empty() || favorites_only_value;
    let filtered_toolsets: Vec<crate::server::tools_config_api::ToolsetGroup> = page_state
        .as_ref()
        .map(|s| filter_groups(&s.toolsets, &query_value, favorites_only_value, &pinned_snapshot))
        .unwrap_or_default();
    // The FAVORITES row is independent of the `favorites_only` chip — it
    // always shows every pinned tool that survives the search query, so
    // toggling the chip narrows the main grid without hiding the row.
    let favorites_toolsets: Vec<crate::server::tools_config_api::ToolsetGroup> = page_state
        .as_ref()
        .map(|s| filter_groups(&s.toolsets, &query_value, true, &pinned_snapshot))
        .unwrap_or_default();

    // Phase 48.2 Plan 06 Task 3 (D-17): the FULL bulk target list — always
    // computed from the unfiltered catalog (never `filtered_toolsets`), so
    // the confirm dialog names every affected toolset regardless of the
    // toolbar's current search/favorites filter.
    let bulk_targets_snapshot: Vec<String> = page_state
        .as_ref()
        .map(|s| s.bulk_targets.clone())
        .unwrap_or_default();
    let live_toolsets_snapshot: Vec<crate::server::tools_config_api::ToolsetGroup> = page_state
        .as_ref()
        .map(|s| s.toolsets.clone())
        .unwrap_or_default();

    // Captured before `load_error` is potentially moved out of by the
    // `else if let Some(reason) = load_error` arm below.
    let load_error_is_none = load_error.is_none();
    // Explicit type annotations resolve `SuperInto` ambiguity between the
    // `Signal -> ReadSignal` and `dioxus_stores::Store -> ReadSignal` impls
    // that a bare `.into()` call inside the props builder cannot infer.
    let scope_ro: ReadSignal<ConfigScope> = scope.into();
    let refresh_tick_ro: ReadSignal<u32> = refresh_tick.into();

    // Phase 48.2 Plan 11 (D-03/D-14/D-16, G-48.2-6 slice a): the gateway
    // RUNTIME status resource. Same sync-prefix idiom as `page_resource`
    // above — reads BOTH `scope` and `refresh_tick` before the `async
    // move`, so a bump on either re-fetches. Deliberately no separate
    // timer: see `RuntimeSection`'s own doc comment for why a pidfile does
    // not get a background poll (T-48.2-11-08).
    let runtime_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { crate::server::gateway_status_api::get_gateway_runtime_status(scope_value).await }
    });
    let runtime_status: Option<GatewayRuntimeStatus> = match runtime_resource() {
        Some(Ok(s)) => Some(s),
        _ => None,
    };

    rsx! {
        document::Link { rel: "stylesheet", href: TOOLS_CSS }

        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-tools",
            "data-screen-label": "08 Tools",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 08" }
                    h1 { class: "screen-title", "Tools" }
                    p { class: "screen-sub",
                        "Enable or disable toolsets and individual tools available to the agent during conversations."
                    }
                }
                // Phase 48.2 Plan 05 (D-08/D-18): the header PROFILE
                // selector — ROOT first, then every enumerated profile.
                // Bound to the shell's own `scope` signal; `refresh_tick`
                // is passed read-only so the bar's own resource re-fetches
                // on an external refresh without a remount.
                div { class: "screen-actions",
                    // Phase 48.2 Plan 06 Task 3 (D-17): the header's
                    // ENABLE ALL / DISABLE ALL buttons — both open a
                    // confirm naming exactly what flips before anything is
                    // written; both render disabled when the page is
                    // read-only.
                    button {
                        class: "btn btn--sm",
                        "aria-label": "Enable all toolsets",
                        disabled: !writable,
                        onclick: move |_| {
                            if writable {
                                bulk_dialog.set(Some(BulkAction::EnableAll));
                            }
                        },
                        "ENABLE ALL"
                    }
                    button {
                        class: "btn btn--danger btn--sm",
                        "aria-label": "Disable all toolsets",
                        disabled: !writable,
                        onclick: move |_| {
                            if writable {
                                bulk_dialog.set(Some(BulkAction::DisableAll));
                            }
                        },
                        "DISABLE ALL"
                    }
                    ToolsProfileBar { scope, refresh_tick: refresh_tick_ro }
                }
            }

            if let Some(action) = bulk_dialog() {
                BulkConfirmDialog {
                    action,
                    targets: bulk_targets_snapshot,
                    scope: scope(),
                    refresh_tick,
                    on_result: move |report| bulk_report.set(Some(report)),
                    on_close: move |_| bulk_dialog.set(None),
                }
            }

            if let Some(report) = bulk_report() {
                BulkResultBanner {
                    report,
                    live_toolsets: live_toolsets_snapshot,
                    on_dismiss: move |_| bulk_report.set(None),
                }
            }

            // Phase 48.2 Plan 05 (D-18/T-48.2-05-03): present ONLY at
            // profile scope — its presence is itself the "you are not
            // editing root" signal.
            ProfileScopeBanner { scope }

            // Phase 48.2 Plan 06 (D-15): search/favorites toolbar, mounted
            // beneath the profile bar. Shown whenever the catalog has
            // loaded — filtering only makes sense once there is something
            // to filter.
            if !is_loading && load_error_is_none {
                ToolsToolbar { query, favorites_only }
            }

            if !gate_open && !is_loading && load_error.is_none() {
                div { class: "tools-gate-banner", "{READ_ONLY_BANNER}" }
            }

            if is_loading {
                CatalogSkeleton {}
            } else if let Some(reason) = load_error {
                CatalogErrorBanner {
                    reason,
                    on_retry: move |_| {
                        let cur = *refresh_tick.read();
                        refresh_tick.set(cur + 1);
                    },
                }
            } else if page_state.is_some() {
                FavoritesRow {
                    groups: favorites_toolsets,
                    scope: scope(),
                    writable,
                    pinned,
                    on_changed: move |_| {
                        let cur = *refresh_tick.read();
                        refresh_tick.set(cur + 1);
                    },
                    // Phase 48.2 Plan 11 (G-48.2-6 slice a): a pinned card
                    // gets the same gateway annotation its toolset-grid
                    // counterpart would.
                    runtime_status: runtime_status.clone(),
                }
                if filter_active && filtered_toolsets.is_empty() {
                    EmptyFilterState {}
                } else {
                    div { class: "tools-sections",
                        for group in filtered_toolsets {
                            ToolsetSection {
                                key: "{group.name}",
                                group,
                                scope: scope(),
                                writable,
                                pinned,
                                on_changed: move |_| {
                                    let cur = *refresh_tick.read();
                                    refresh_tick.set(cur + 1);
                                },
                                runtime_status: runtime_status.clone(),
                            }
                        }
                    }
                }
            }

            // Phase 48.2 Plan 11 (G-48.2-6 slice a) + Plan 13 (slice b): the
            // RUNTIME section mounts above MCP SERVERS regardless of the
            // catalog's own load state — it has its own resource(s) and its
            // own loading treatment (a neutral CHECKING pill), and does not
            // depend on `page_state` at all. `scope` is threaded through so
            // Plan 13's lifecycle actions know which gateway to act on.
            RuntimeSection { status: runtime_status.clone(), scope: scope(), refresh_tick }

            if !is_loading && load_error_is_none {
                McpSection { scope: scope(), writable, refresh_tick }
            }

            // Phase 48.2 Plan 12 (G-48.2-7): the BUZZ section, mounted after
            // MCP SERVERS. `runtime_status` is threaded through here (the
            // already-fetched 48.2-11 resource, never a second fetch of the
            // same fact) so a later task inside `buzz_section.rs` alone can
            // render the live-apply honesty statement without this file
            // needing a second edit (`files_modified` scopes that task to
            // `buzz_section.rs` only).
            if !is_loading && load_error_is_none {
                BuzzSection {
                    scope: scope(),
                    writable,
                    refresh_tick,
                    runtime_status: runtime_status.clone(),
                }
            }

            if !is_loading && load_error_is_none {
                ToolsSettingsPanel {
                    scope: scope_ro,
                    writable,
                    refresh_tick: refresh_tick_ro,
                    known_tool_names,
                    missing_credential_keys,
                }
            }
        }
    }
}
