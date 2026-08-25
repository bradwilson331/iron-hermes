//! Phase 48.2 Plan 01 (D-14/D-16/D-20) — Tools page catalog components:
//! `ToolsetSection` (a display group's header + master toggle + card grid),
//! `ToolCard` (Task 2: per-tool override toggle + amber unmet-prerequisite
//! treatment), `AvailabilityPill`, `CatalogSkeleton`, `CatalogErrorBanner`,
//! and the zero-match `EmptyFilterState` (reserved for Plan 06's search/
//! favorites toolbar — not yet rendered from anywhere in this plan).
//!
//! Phase 48.2 Plan 04 Task 3 (D-20): [`is_mcp_group`]/[`mcp_group_label`]
//! key ONLY on the server-supplied `ToolsetKind` discriminant carried in
//! `ToolCatalogRow`/`ToolsetGroup` — never on a client-side parse of the
//! group name. `sanitize_server_name` runs server-side only (Plan 01's
//! `mcp_group_server_key`, Plan 02's `McpServerRow.toolset_group`); this
//! file must never re-derive that mapping.
//!
//! Phase 48.2 Plan 11 (G-48.2-6 slice a): [`should_show_gateway_annotation`]
//! is the render-decision predicate for `ToolCard`'s gateway-down
//! annotation — a pure function of (declared `runtime_dependency`, the
//! live `GatewayRuntimeStatus`), unit-tested across every combination
//! below. The annotation never gates, hides, or disables a card — `cronjob`
//! stays AVAILABLE and its toggle stays live no matter what the gateway is
//! doing.

use super::credential_form::ToolCredentialForm;
use crate::server::gateway_status_api::GatewayRuntimeStatus;
use crate::server::tools_config_api::{
    ConfigScope, MissingPrereq, ToolAvailability, ToolCatalogEntry, ToolsetGroup, ToolsetKind,
};
use dioxus::prelude::*;

/// D-20: true for an MCP display group, false for a built-in one. Keys
/// ONLY on the `ToolsetKind` discriminant — takes no group-name argument at
/// all, so it structurally cannot consult the group-name string.
#[allow(dead_code)] // used inside ToolsetSection rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn is_mcp_group(kind: &ToolsetKind) -> bool {
    matches!(kind, ToolsetKind::Mcp { .. })
}

/// D-20: the group-header label pairing the server name (from
/// `ToolsetKind::Mcp { server }`, never a substring parsed from
/// `group_name`) with the raw display-group name, so the operator can
/// connect this group to its row in the MCP SERVERS section.
#[allow(dead_code)] // used inside ToolsetSection rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn mcp_group_label(kind: &ToolsetKind, group_name: &str) -> String {
    match kind {
        ToolsetKind::Mcp { server } => format!("Imported server: {server} ({group_name})"),
        ToolsetKind::BuiltIn => group_name.to_string(),
    }
}

/// D-20 (Phase 48.2 Plan 10/G-48.2-4): the provenance-badge text for a
/// card's or group's origin. Keys ONLY on `&ToolsetKind` — same discipline
/// as `is_mcp_group`/`mcp_group_label` above, never a group-name parse.
/// Used at BOTH render sites — the per-card badge in `ToolCard` and the
/// built-in group-header badge in `ToolsetSection`'s builtin branch — so
/// there is one derivation, not a second hardcoded "BUILT-IN" string. The
/// MCP group header does NOT call this: it already shows `mcp_group_label`'s
/// text as its provenance label, and this fn's own MCP phrasing would be a
/// second, competing phrasing of the same fact.
#[allow(dead_code)] // used inside ToolCard/ToolsetSection rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn origin_badge_label(origin: &ToolsetKind) -> String {
    match origin {
        ToolsetKind::BuiltIn => "BUILT-IN".to_string(),
        ToolsetKind::Mcp { server } => format!("IMPORTED · {server}"),
    }
}

/// D-16/G-48.2-3 (Task 3b): the credential-fixable half of a
/// missing-prerequisites list — feeds `.tool-fix-mount`'s
/// `ToolCredentialForm`s, one per entry, unchanged from before this plan.
#[allow(dead_code)] // used inside ToolCard rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn missing_fixable_by_credential(missing: &[MissingPrereq]) -> Vec<&MissingPrereq> {
    missing.iter().filter(|m| m.kind == "env_var").collect()
}

/// D-16/G-48.2-3 (Task 3b): the LITERAL COMPLEMENT of
/// [`missing_fixable_by_credential`]'s filter — every missing prerequisite
/// no credential form can satisfy (`runtime`, `config_field`, and any
/// future non-`env_var` kind). Written as `!=` against the exact predicate
/// the fixable half filters on, so a fourth prerequisite kind added later
/// lands here automatically rather than rendering nothing — the exact
/// failure mode that produced G-48.2-3 (both messaging tools' `runtime`
/// prerequisites were unreportable because no branch existed for a
/// non-`env_var` kind).
#[allow(dead_code)] // used inside ToolCard rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn missing_needing_explanation(missing: &[MissingPrereq]) -> Vec<&MissingPrereq> {
    missing.iter().filter(|m| m.kind != "env_var").collect()
}

/// Phase 48.2 Plan 11 (G-48.2-6 slice a): the render-decision predicate for
/// `ToolCard`'s gateway-down annotation, as a pure function of (declared
/// `runtime_dependency`, the live `GatewayRuntimeStatus`) — every branch
/// unit-tested below.
///
/// `true` only when the tool declares a dependency AND the live status is
/// resolved AND NOT confirmed running. `status: None` (the RUNTIME resource
/// has not resolved yet) is treated the same as "don't know" — no
/// annotation, rather than a flash of an unwarranted claim before the first
/// real read. No dependency means no annotation regardless of status
/// (Task 3 must_have: `delegate_task` never gets one).
#[allow(dead_code)] // used inside ToolCard rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn should_show_gateway_annotation(
    dependency: Option<&str>,
    status: Option<&GatewayRuntimeStatus>,
) -> bool {
    dependency.is_some() && status.is_some_and(|s| !s.is_confirmed_running())
}

/// Phase 48.2 Plan 11: the annotation copy for a non-running `status`. Only
/// ever called when [`should_show_gateway_annotation`] is `true` — the
/// `Running`/`RunningOtherUser` arms are unreachable through that call
/// path, but stay exhaustive and honest (a neutral fallback line, never a
/// panic) rather than assuming the predicate was checked correctly at every
/// future call site.
///
/// `NotRunning` states plainly that the gateway is down. `StalePidfile` and
/// `Unknown`/`UnsupportedPlatform` deliberately do NOT claim the gateway is
/// definitely down — an overconfident annotation here is the same class of
/// defect as the overconfident AVAILABLE pill this plan exists to fix; they
/// say only what is actually known (a leftover pidfile / an undetermined
/// state) and that scheduled work is not CONFIRMED running.
#[allow(dead_code)] // used inside ToolCard rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn gateway_annotation_text(status: &GatewayRuntimeStatus) -> String {
    match status {
        GatewayRuntimeStatus::NotRunning => {
            "The gateway is not running — what this tool creates will not fire until it is."
                .to_string()
        }
        GatewayRuntimeStatus::StalePidfile { .. } => {
            "A gateway pidfile is present but its process is gone — scheduled work is not confirmed to be running."
                .to_string()
        }
        GatewayRuntimeStatus::UnsupportedPlatform | GatewayRuntimeStatus::Unknown { .. } => {
            "The gateway's state could not be determined — scheduled work is not confirmed to be running."
                .to_string()
        }
        GatewayRuntimeStatus::Running { .. } | GatewayRuntimeStatus::RunningOtherUser { .. } => {
            // Unreachable via should_show_gateway_annotation (both are
            // is_confirmed_running() == true), kept exhaustive rather than
            // unreachable!() so a future direct call never panics a card.
            "Gateway status confirmed running.".to_string()
        }
    }
}

/// While the page's `use_resource` is unresolved: 6 dimmed placeholder
/// `.tool-card` outlines, not a spinner.
#[component]
pub fn CatalogSkeleton() -> Element {
    rsx! {
        div { class: "tools-skeleton-grid",
            for i in 0..6 {
                div { key: "{i}", class: "tool-card tools-skeleton-card" }
            }
        }
    }
}

/// A real catalog fetch failure — the grid must not silently render empty.
#[component]
pub fn CatalogErrorBanner(reason: String, on_retry: EventHandler<()>) -> Element {
    rsx! {
        div { class: "tools-error-banner",
            span { "COULD NOT LOAD TOOLS — {reason}" }
            button {
                class: "btn btn--ghost btn--sm",
                onclick: move |_| on_retry.call(()),
                "RETRY"
            }
        }
    }
}

/// Zero-match search/favorites filter state (D-15). Mounted from
/// `ScreenTools` (Plan 06) when a search query or the favorites-only chip
/// filters the catalog down to zero groups.
#[component]
pub fn EmptyFilterState() -> Element {
    rsx! {
        div { class: "tools-empty-state",
            div { class: "tools-empty-heading", "NO TOOLS MATCH" }
            div { class: "tools-empty-body",
                "Try a different search term or clear the filter to see all tools."
            }
        }
    }
}

/// One display-group section: a header (name, tool count, master toggle)
/// plus its member cards. For a `ToolsetKind::BuiltIn` group the master
/// toggle calls `toggle_toolset` and bumps `on_changed` on success. For a
/// `ToolsetKind::Mcp` group (Plan 04 Task 3) the master toggle is LIVE — it
/// calls `set_mcp_server_enabled`, the server's own `mcp_servers.<name>.enabled`
/// lifecycle field — and NEVER `toggle_toolset`, which Plan 01 makes reject
/// `mcp__`-prefixed names precisely so this mistake fails loudly server-side
/// rather than persisting an inert key.
#[component]
pub fn ToolsetSection(
    group: ToolsetGroup,
    scope: ConfigScope,
    writable: bool,
    pinned: Signal<crate::ui_prefs::ToolFavorites>,
    on_changed: EventHandler<()>,
    // Phase 48.2 Plan 11 (G-48.2-6 slice a): the live gateway status for
    // THIS scope, threaded down to every member `ToolCard` unchanged. An
    // owned value (not a signal) — the section already takes owned
    // `group`/`scope` props, and this follows that shape (Task 3 action).
    runtime_status: Option<GatewayRuntimeStatus>,
) -> Element {
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let is_mcp = is_mcp_group(&group.kind);
    let enabled = group.enabled;
    let group_name = group.name.clone();
    let tool_count = group.tools.len();
    let scope_for_toggle = scope.clone();
    let group_label = mcp_group_label(&group.kind, &group.name);
    let mcp_server_name = match &group.kind {
        ToolsetKind::Mcp { server } => server.clone(),
        ToolsetKind::BuiltIn => String::new(),
    };

    rsx! {
        div { class: "toolset-section",
            div { class: "toolset-section-header",
                div { class: "section-label",
                    "{group.name.to_uppercase()}"
                    span { class: "count", "({tool_count})" }
                }
                if is_mcp {
                    div { class: "toolset-master-toggle toolset-master-toggle--mcp",
                        div {
                            class: if enabled { "tgl on" } else { "tgl" },
                            "aria-label": "Toggle {mcp_server_name} MCP server enabled",
                            onclick: {
                                let scope_for_mcp = scope_for_toggle.clone();
                                let server_for_mcp = mcp_server_name.clone();
                                move |_| {
                                    if !writable {
                                        return;
                                    }
                                    let scope_value = scope_for_mcp.clone();
                                    let server_value = server_for_mcp.clone();
                                    let next_enabled = !enabled;
                                    spawn(async move {
                                        match crate::server::mcp_admin_api::set_mcp_server_enabled(
                                            scope_value,
                                            server_value,
                                            next_enabled,
                                        )
                                            .await
                                        {
                                            Ok(_row) => {
                                                save_error.set(None);
                                                on_changed.call(());
                                            }
                                            Err(e) => {
                                                save_error.set(Some(e.to_string()));
                                            }
                                        }
                                    });
                                }
                            },
                        }
                    }
                    a {
                        class: "toolset-mcp-group-link",
                        href: "#mcp-servers-section",
                        "aria-label": "View {mcp_server_name} in the MCP SERVERS section",
                        "{group_label}"
                    }
                    if !enabled {
                        span { class: "toolset-mcp-note toolset-mcp-note--disabled",
                            "SERVER DISABLED — enable it to reconnect and expose these tools."
                        }
                    }
                } else {
                    div { class: "toolset-master-toggle",
                        div {
                            class: if enabled { "tgl on" } else { "tgl" },
                            "aria-label": "Toggle {group_name} toolset",
                            onclick: move |_| {
                                if !writable {
                                    return;
                                }
                                let scope_value = scope_for_toggle.clone();
                                let name = group_name.clone();
                                let next_enabled = !enabled;
                                spawn(async move {
                                    match crate::server::tools_config_api::toggle_toolset(
                                        scope_value,
                                        name,
                                        next_enabled,
                                    )
                                        .await
                                    {
                                        Ok(()) => {
                                            save_error.set(None);
                                            on_changed.call(());
                                        }
                                        Err(e) => {
                                            save_error.set(Some(e.to_string()));
                                        }
                                    }
                                });
                            },
                        }
                    }
                    // Phase 48.2 Plan 10 (D-14/G-48.2-4): built-in group
                    // headers were the silent half — MCP groups already show
                    // `group_label` as their provenance text; this badge
                    // gives built-in groups the same treatment, driven by
                    // the same `origin_badge_label` fn `ToolCard` uses.
                    span {
                        class: "toolset-origin-badge toolset-origin-badge--builtin",
                        "{origin_badge_label(&group.kind)}"
                    }
                }
            }
            if let Some(err) = save_error() {
                span { class: "pill red tools-save-failed-chip", "SAVE FAILED — {err}." }
            }
            div { class: "grid",
                for tool in group.tools.iter().cloned() {
                    ToolCard {
                        key: "{tool.name}",
                        tool,
                        scope: scope.clone(),
                        writable,
                        toolset_enabled: enabled,
                        pinned,
                        on_changed,
                        runtime_status: runtime_status.clone(),
                    }
                }
            }
        }
    }
}

/// FAVORITES row (D-15): every pinned tool that survives the toolbar's
/// current filter, rendered as ordinary cards above the toolset groups.
/// `groups` is the ALREADY-filtered (favorites-only) group list — reuses
/// `toolbar::filter_groups(groups, query, true, pinned)` rather than a
/// separate helper, so there is exactly one definition of "which tools
/// match". Renders nothing when `groups` carries no tools (the pinned set
/// is empty, or every pinned tool was filtered out by the search query) —
/// never an empty placeholder row.
#[component]
pub fn FavoritesRow(
    groups: Vec<ToolsetGroup>,
    scope: ConfigScope,
    writable: bool,
    pinned: Signal<crate::ui_prefs::ToolFavorites>,
    on_changed: EventHandler<()>,
    // Phase 48.2 Plan 11 (G-48.2-6 slice a): same owned-value threading as
    // `ToolsetSection` — a pinned card renders the same annotation its
    // toolset-grid counterpart would.
    runtime_status: Option<GatewayRuntimeStatus>,
) -> Element {
    let entries: Vec<(ToolCatalogEntry, bool)> = groups
        .iter()
        .flat_map(|g| g.tools.iter().cloned().map(|t| (t, g.enabled)))
        .collect();

    if entries.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "favorites-row",
            div { class: "section-label", "FAVORITES" }
            div { class: "grid",
                for (tool, toolset_enabled) in entries {
                    ToolCard {
                        key: "{tool.name}",
                        tool,
                        scope: scope.clone(),
                        writable,
                        toolset_enabled,
                        pinned,
                        on_changed,
                        runtime_status: runtime_status.clone(),
                    }
                }
            }
        }
    }
}

/// Phase 48.2 Plan 10 Task 3b (D-16/G-48.2-3) / Plan 11 Task 3
/// (G-48.2-6): a runtime fact the operator cannot fix by typing a
/// credential — `role="note"` prose, never a control, so assistive tech
/// announces it as an explanation to read rather than something to click.
/// Originally the missing-prerequisite explanation's own inline markup;
/// widened here from a fixed `&[MissingPrereq]` render to a plain
/// `class`/`aria_label`/`lines` shape so the gateway-down annotation
/// (Task 3) reuses this exact component rather than forking a second
/// `role="note"` block — the composition IS the point (Task 3 action).
#[component]
fn ExplanatoryNote(class: &'static str, aria_label: String, lines: Vec<String>) -> Element {
    rsx! {
        div {
            class: "{class}",
            role: "note",
            "aria-label": "{aria_label}",
            for (i, line) in lines.into_iter().enumerate() {
                div { class: "tool-prereq-explanation", key: "{i}", "{line}" }
            }
        }
    }
}

/// One tool card: icon, name, description, availability pill, and the
/// per-tool disabled-override toggle (D-16/D-20). A card whose availability
/// is `Unavailable` renders amber and names every missing prerequisite; a
/// toggle-write failure shows an inline `SAVE FAILED` chip with a retry
/// affordance and never advances the rendered toggle state until the write
/// actually succeeds (the render always reflects last-confirmed server
/// state, never an optimistic guess).
///
/// `toolset_enabled` links the card to its parent toolset's master toggle:
/// a card in a disabled toolset stays in the grid (D-16) but renders dimmed
/// with its toggle OFF and visibly disabled — the tool is inactive no matter
/// what its per-tool override says, and the toggle is inert-by-declaration
/// (guarded click + aria-disabled), never a live-but-silent no-op (D-03,
/// same treatment as the MCP master toggle).
#[component]
fn ToolCard(
    tool: ToolCatalogEntry,
    scope: ConfigScope,
    writable: bool,
    toolset_enabled: bool,
    pinned: Signal<crate::ui_prefs::ToolFavorites>,
    on_changed: EventHandler<()>,
    // Phase 48.2 Plan 11 (G-48.2-6 slice a): the live gateway status for
    // the page's current scope. `None` while the RUNTIME resource has not
    // resolved yet — `should_show_gateway_annotation` treats that the same
    // as "don't know", never flashing a claim before the first real read.
    runtime_status: Option<GatewayRuntimeStatus>,
) -> Element {
    let save_error: Signal<Option<String>> = use_signal(|| None);
    let mut pending_disable: Signal<Option<bool>> = use_signal(|| None);

    let is_amber = matches!(tool.availability, ToolAvailability::Unavailable { .. });
    let is_enabled = toolset_enabled && !tool.disabled_override;
    let tool_name_for_toggle = tool.name.clone();
    let tool_name_for_retry = tool.name.clone();
    let scope_for_toggle = scope.clone();
    let scope_for_retry = scope.clone();

    let do_toggle = move |next_disable: bool, mut save_error: Signal<Option<String>>, scope_value: ConfigScope, name: String| {
        spawn(async move {
            match crate::server::tools_config_api::toggle_tool_disabled(
                scope_value,
                name,
                next_disable,
            )
            .await
            {
                Ok(()) => {
                    save_error.set(None);
                    on_changed.call(());
                }
                Err(e) => {
                    save_error.set(Some(e.to_string()));
                }
            }
        });
    };

    rsx! {
        div {
            class: "tool-card",
            class: if is_amber { "tool-card--amber" },
            class: if !toolset_enabled { "tool-card--toolset-off" },
            div { class: "tool-top",
                div { class: "tool-icon", "aria-hidden": "true", "⊕" }
                FavoritesStar { tool_name: tool.name.clone(), pinned }
                AvailabilityPill { availability: tool.availability.clone() }
                div {
                    class: if is_enabled { "tgl on" } else { "tgl" },
                    class: if !toolset_enabled { "tgl--disabled" },
                    "aria-label": "Toggle {tool.name} enabled state",
                    "aria-disabled": if !toolset_enabled { "true" },
                    title: if !toolset_enabled { "Toolset disabled — enable the toolset master toggle first" },
                    onclick: {
                        let scope_for_toggle = scope_for_toggle.clone();
                        let tool_name_for_toggle = tool_name_for_toggle.clone();
                        move |_| {
                            if !writable || !toolset_enabled {
                                return;
                            }
                            let next_disable = is_enabled;
                            pending_disable.set(Some(next_disable));
                            do_toggle(
                                next_disable,
                                save_error,
                                scope_for_toggle.clone(),
                                tool_name_for_toggle.clone(),
                            );
                        }
                    },
                }
            }
            div { class: "tool-name", title: "{tool.name}", "{tool.name}" }
            div { class: "tool-desc", "{tool.description}" }
            span {
                class: if matches!(tool.origin, ToolsetKind::Mcp { .. }) { "tool-origin-badge tool-origin-badge--mcp" } else { "tool-origin-badge tool-origin-badge--builtin" },
                "{origin_badge_label(&tool.origin)}"
            }
            // Phase 48.2 Plan 11 (G-48.2-6 slice a): independent of
            // `tool.availability` — `cronjob` stays AVAILABLE with the
            // gateway down, so this annotation is additive truth, not a
            // recolor of the pill above it (D-16 prohibition: never gate,
            // hide, or disable the card on this fact).
            if should_show_gateway_annotation(tool.runtime_dependency.as_deref(), runtime_status.as_ref()) {
                if let Some(status) = runtime_status.as_ref() {
                    ExplanatoryNote {
                        class: "tool-runtime-annotation",
                        aria_label: format!("Gateway dependency status for {}", tool.name),
                        lines: vec![gateway_annotation_text(status)],
                    }
                }
            }
            if let ToolAvailability::Unavailable { missing } = &tool.availability {
                // Task 3a: each missing prerequisite renders its
                // `description` alongside its `name` — a bare identifier is
                // not a reason. Name stays visible (what an operator greps
                // for); description is the prominent line.
                div { class: "tool-missing-prereqs",
                    for m in missing.iter() {
                        div { class: "tool-missing-prereq", key: "{m.name}",
                            div { class: "tool-missing-prereq-name", "{m.name}" }
                            div { class: "tool-missing-prereq-desc", "{m.description}" }
                        }
                    }
                }
                // Task 3b: explanatory (non-credential-fixable) treatment
                // for a kind no credential form can satisfy (`runtime`,
                // `config_field`, and any future non-`env_var` kind) —
                // text, not a control, so `role`/`aria` announce it as an
                // explanation an operator reads rather than something to
                // click. `missing_needing_explanation` is the literal
                // complement of the filter feeding `.tool-fix-mount` below.
                if !missing_needing_explanation(missing).is_empty() {
                    ExplanatoryNote {
                        class: "tool-prereq-explanations",
                        aria_label: format!("Why {} cannot be fixed here", tool.name),
                        lines: missing_needing_explanation(missing)
                            .iter()
                            .map(|m| m.description.clone())
                            .collect::<Vec<String>>(),
                    }
                }
                // D-16 inline key-entry fix form, routed per D-07 (root)
                // / D-09 (profile) — one ToolCredentialForm per env_var
                // prerequisite ONLY. A `runtime` or `config_field`
                // prerequisite never reaches this branch — no credential
                // can satisfy either.
                div {
                    class: "tool-fix-mount",
                    "aria-label": "Fix {tool.name} prerequisite",
                    for m in missing_fixable_by_credential(missing) {
                        ToolCredentialForm {
                            key: "{m.name}",
                            scope: scope.clone(),
                            key_name: m.name.clone(),
                            writable,
                            on_saved: on_changed,
                        }
                    }
                }
            }
            if let Some(err) = save_error() {
                div { class: "tool-save-failed-row",
                    span { class: "pill red tools-save-failed-chip", "SAVE FAILED — {err}." }
                    button {
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Retry saving {tool.name}",
                        onclick: {
                            move |_| {
                                if let Some(next_disable) = pending_disable() {
                                    do_toggle(
                                        next_disable,
                                        save_error,
                                        scope_for_retry.clone(),
                                        tool_name_for_retry.clone(),
                                    );
                                }
                            }
                        },
                        "RETRY"
                    }
                }
            }
        }
    }
}

/// D-15: per-tool favorites star. A per-browser UI preference only — the
/// click handler mutates the shell-level `pinned` signal and persists it
/// through `write_tool_favorites`; nothing here ever calls a `#[server]`
/// fn or touches config.yaml. Filled in accent (`--teal`) when pinned,
/// outlined when not (UI-SPEC `## Color` — filled/pinned star is one of
/// the accent-reserved uses).
#[component]
fn FavoritesStar(tool_name: String, mut pinned: Signal<crate::ui_prefs::ToolFavorites>) -> Element {
    let is_pinned = pinned.read().is_pinned(&tool_name);
    let name_for_click = tool_name.clone();
    let action_label = if is_pinned {
        format!("Unpin {tool_name} from favorites")
    } else {
        format!("Pin {tool_name} to favorites")
    };

    rsx! {
        button {
            r#type: "button",
            class: if is_pinned { "favorites-star favorites-star--on" } else { "favorites-star" },
            "aria-label": "{action_label}",
            "aria-pressed": if is_pinned { "true" } else { "false" },
            onclick: move |_| {
                let mut favorites = pinned.write();
                if is_pinned {
                    favorites.unpin(&name_for_click);
                } else {
                    favorites.pin(&name_for_click);
                }
                let snapshot = favorites.clone();
                drop(favorites);
                crate::ui_prefs::write_tool_favorites(&snapshot);
            },
            if is_pinned { "★" } else { "☆" }
        }
    }
}

/// D-16: availability pill — `Available` renders green, `Unavailable`
/// renders amber (missing-prerequisite names render separately in the
/// card body), `Unknown` renders neutral (never falsely claiming either
/// state).
#[component]
fn AvailabilityPill(availability: ToolAvailability) -> Element {
    match availability {
        ToolAvailability::Available => rsx! {
            span { class: "pill green", "AVAILABLE" }
        },
        ToolAvailability::Unavailable { .. } => rsx! {
            span {
                class: "pill amber",
                // Task 3d: the enable toggle records configuration intent;
                // this pill reports a runtime fact. A card showing ON and
                // UNAVAILABLE together is correct by design (D-16), not a
                // contradiction — say so rather than leaving it to read as
                // a bug.
                title: "The toggle records whether you want this tool on; this pill reports whether it can run right now — the two are independent, so ON + UNAVAILABLE together is expected.",
                "UNAVAILABLE"
            }
        },
        ToolAvailability::Unknown { reason } => rsx! {
            span { class: "pill", "Availability unknown — {reason}" }
        },
    }
}

// =============================================================================
// Pure-fn tests — Phase 48.2 Plan 04 Task 3 (D-20)
// =============================================================================
#[cfg(test)]
mod mcp_group_tests {
    use super::*;

    #[test]
    fn is_mcp_group_true_for_mcp_false_for_builtin() {
        assert!(is_mcp_group(&ToolsetKind::Mcp { server: "github".to_string() }));
        assert!(!is_mcp_group(&ToolsetKind::BuiltIn));
    }

    #[test]
    fn mcp_group_label_uses_the_server_name_from_toolset_kind_not_the_group_name() {
        let kind = ToolsetKind::Mcp { server: "github".to_string() };
        // Deliberately mismatched group_name — a client-side parse of the
        // group name would produce a DIFFERENT server name than the one
        // ToolsetKind::Mcp carries; the label must use the latter.
        let label = mcp_group_label(&kind, "mcp__totally_different_group_name");
        assert!(
            label.contains("github"),
            "label must contain the server name carried by ToolsetKind::Mcp; got: {label}"
        );
    }

    #[test]
    fn mcp_group_label_falls_through_to_the_group_name_for_builtin() {
        assert_eq!(mcp_group_label(&ToolsetKind::BuiltIn, "code"), "code");
    }
}

// =============================================================================
// Pure-fn tests — Phase 48.2 Plan 10 Task 3 (D-14/D-16/G-48.2-3/G-48.2-4)
// =============================================================================
#[cfg(test)]
mod origin_and_prereq_split_tests {
    use super::*;

    #[test]
    fn origin_badge_label_builtin_case() {
        assert_eq!(origin_badge_label(&ToolsetKind::BuiltIn), "BUILT-IN");
    }

    #[test]
    fn origin_badge_label_mcp_case_names_the_server() {
        let label = origin_badge_label(&ToolsetKind::Mcp {
            server: "github".to_string(),
        });
        assert!(
            label.contains("github"),
            "label must name the server; got: {label}"
        );
        assert!(
            label.contains("IMPORTED"),
            "label must say IMPORTED; got: {label}"
        );
    }

    fn prereq(kind: &str, name: &str) -> MissingPrereq {
        MissingPrereq {
            kind: kind.to_string(),
            name: name.to_string(),
            description: format!("{name} description"),
        }
    }

    /// Given a mixed `missing` list (`env_var`, `runtime`, `config_field`),
    /// the fixable half receives exactly the `env_var` entries and the
    /// explanatory half receives exactly the complement — every non-
    /// `env_var` entry, not just `runtime`. Together the two lists cover
    /// every input entry exactly once (true partition, no drops, no
    /// duplicates).
    #[test]
    fn fixable_and_explanation_split_is_the_literal_env_var_complement() {
        let missing = vec![
            prereq("env_var", "SOME_API_KEY"),
            prereq("runtime", "clarify_channel"),
            prereq("config_field", "search.brave_api_key"),
        ];

        let fixable = missing_fixable_by_credential(&missing);
        let explain = missing_needing_explanation(&missing);

        assert_eq!(fixable.len(), 1, "exactly one env_var entry");
        assert_eq!(fixable[0].name, "SOME_API_KEY");

        assert_eq!(
            explain.len(),
            2,
            "both non-env_var entries land in the explanatory half"
        );
        assert!(explain.iter().any(|m| m.name == "clarify_channel"));
        assert!(explain.iter().any(|m| m.name == "search.brave_api_key"));

        // A fourth, hypothetical kind lands in the explanatory half too —
        // the complement is `!= "env_var"`, not an enumerated allowlist.
        let with_future_kind = {
            let mut v = missing.clone();
            v.push(prereq("some_future_kind", "future_thing"));
            v
        };
        let future_explain = missing_needing_explanation(&with_future_kind);
        assert!(
            future_explain.iter().any(|m| m.name == "future_thing"),
            "a fourth kind must land in the explanatory half automatically"
        );
        let future_fixable = missing_fixable_by_credential(&with_future_kind);
        assert!(
            !future_fixable.iter().any(|m| m.name == "future_thing"),
            "a fourth kind must never reach the credential-form half"
        );
    }
}

// =============================================================================
// Pure-fn tests — Phase 48.2 Plan 11 Task 3 (G-48.2-6 slice a)
// =============================================================================
#[cfg(test)]
mod gateway_annotation_tests {
    use super::*;

    fn running() -> GatewayRuntimeStatus {
        GatewayRuntimeStatus::Running {
            pid: 1,
            started_at: "t".to_string(),
            profile: "default".to_string(),
        }
    }

    fn running_other_user() -> GatewayRuntimeStatus {
        GatewayRuntimeStatus::RunningOtherUser {
            pid: 1,
            started_at: "t".to_string(),
            profile: "default".to_string(),
        }
    }

    /// Every (dependency, status) combination named by the Task 3 done
    /// criteria: dependency + running (either user) gives no annotation;
    /// dependency + each non-running state gives one; no dependency gives
    /// none regardless of status; an unresolved status (`None`) gives none.
    #[test]
    fn every_dependency_status_combination() {
        let non_running_states = [
            GatewayRuntimeStatus::NotRunning,
            GatewayRuntimeStatus::StalePidfile { pid: 42 },
            GatewayRuntimeStatus::UnsupportedPlatform,
            GatewayRuntimeStatus::Unknown {
                reason: "test".to_string(),
            },
        ];

        // dependency + running (either user) -> no annotation.
        assert!(!should_show_gateway_annotation(
            Some("gateway"),
            Some(&running())
        ));
        assert!(!should_show_gateway_annotation(
            Some("gateway"),
            Some(&running_other_user())
        ));

        // dependency + each non-running state -> an annotation.
        for status in &non_running_states {
            assert!(
                should_show_gateway_annotation(Some("gateway"), Some(status)),
                "dependency + {status:?} must show an annotation"
            );
        }

        // no dependency + any status (including running/non-running/None)
        // -> never an annotation.
        assert!(!should_show_gateway_annotation(None, Some(&running())));
        assert!(!should_show_gateway_annotation(
            None,
            Some(&GatewayRuntimeStatus::NotRunning)
        ));
        assert!(!should_show_gateway_annotation(None, None));

        // dependency + unresolved status (None) -> no annotation yet, not a
        // flashed false claim before the first real read.
        assert!(!should_show_gateway_annotation(Some("gateway"), None));
    }

    #[test]
    fn not_running_text_says_the_gateway_is_down() {
        let text = gateway_annotation_text(&GatewayRuntimeStatus::NotRunning);
        assert!(text.contains("not running"));
    }

    /// T-48.2-11-06: stale/unknown wording must not overclaim that the
    /// gateway is definitely down — only that scheduled work is not
    /// CONFIRMED running.
    #[test]
    fn stale_and_unknown_text_never_claims_definitely_down() {
        let stale = gateway_annotation_text(&GatewayRuntimeStatus::StalePidfile { pid: 1 });
        assert!(!stale.to_lowercase().contains("is not running"));
        assert!(stale.contains("not confirmed"));

        let unknown = gateway_annotation_text(&GatewayRuntimeStatus::Unknown {
            reason: "test".to_string(),
        });
        assert!(!unknown.to_lowercase().contains("is not running"));
        assert!(unknown.contains("not confirmed"));
    }
}
