//! Models screen — wired to the live `api::list_models()` server fn
//! (Phase 26.7 Plan 04 / D-10, R-1).
//!
//! Renders the full ModelRegistry catalog grouped by inferred family.
//! The configured default model (state.config.model.default) renders with
//! status `DEFAULT`; all others show `AVAILABLE`. Family grouping uses
//! owned `Vec<String>` (not `Vec<&'static str>`) per PATTERNS.md gotcha.
//! Context window formatted as human-readable string ("200k", "1M", etc.).
//!
//! Phase 46.9 Plan 02 (D-05/D-10): Above the catalog grid, a default-model
//! selector card + the six fixed `config.model.roles` picker rows turn this
//! screen from read-only into read+write (`get_models_roles_config`/
//! `update_models_roles_config`, api.rs). Also fixes the pre-existing
//! error-token bug (the load-error state used design-tokens.css/tokens.css
//! variables that don't resolve against this screen's stylesheet — now
//! real `--red` / `12px` from site.css/screens.css).
//!
//! ── Phase 46.9 Plan 07 (Gap 1/CR-01) ───────────────────────────────────
//! `models_resource`/`roles_resource` were declared via `use_server_future`
//! chained with the `?` operator, which early-returns while the resource is
//! loading — the exact hook-ordering trap documented and fixed in
//! `agents.rs` (~41-58, "UAT-2 hotfix"). Swapped to plain `use_resource(...)`
//! (no `?`, loading read via `.is_none()` every render).
//!
//! ── Phase 46.9 Plan 15 (GAP-6, GAP-1 round-2) ──────────────────────────
//! Round-1 shipped green unit tests but the live round-2 UAT still failed on
//! two counts:
//!
//! GAP-6 — the six role rows rendered EMPTY on a fresh load. Root cause: the
//! rows were seeded into a local snapshot signal by a **seed-once
//! `use_effect`** (guarded by a seeded-boolean flag) that did not reliably
//! fire in a live browser. Fix: the seed-once effect + its seeded guard + the
//! local snapshot signal are GONE. The rendered snapshot is derived DIRECTLY
//! from `roles_resource` on every render (loading via `.is_none()`, never a
//! `?` early-return upstream of any later hook). Post-write refresh bumps a
//! monotonic `roles_refresh_nonce` that the resource closure reads, so
//! `use_resource` re-runs and re-fetches — no seed effect that can miss the
//! live resolution, and no resource restart-method call (the CR-01 hook-order
//! trap stays closed). The role-row view models (label, provider/model, and
//! the stale-`MISSING` flag) come from a pure fn (`compute_role_row_views`)
//! unit-tested against a stale-distractor fixture WITHOUT a VirtualDom.
//!
//! GAP-1 — the model dropdown was a flat all-catalog list, and the selects
//! were controlled `<select value=...>` inputs that snapped back / looked
//! frozen live. Fix: `ProviderModelCascade` presents the PROVIDER select
//! FIRST; changing it re-fetches `list_provider_models(provider)` (Plan 13)
//! and repopulates the DEPENDENT model select from that provider's own list
//! (falling back to the full catalog with a dim note when the provider
//! exposes no `/models` endpoint). The frozen-control fix binds no `value:`
//! on the `<select>` — each `<option>` carries an explicit `selected` state
//! derived from a `.read()` signal (the interactive precedent), so the shown
//! option follows the signal every render. The same cascade backs the global
//! default card and all six role rows, so they cannot diverge. The
//! model-option-list derivation (`compute_model_options`) is a pure fn,
//! unit-tested with a provider list that excludes a catalog-only distractor
//! to prove the options are provider-sourced, not catalog-sourced.

use dioxus::prelude::*;

/// Phase 46.9 Plan 15 (GAP-6): pure, VirtualDom-free view model for one of
/// the six fixed Models role rows. Extracted from the `#[component]` so the
/// stale-assignment (`is_missing`) decision can be unit-tested directly
/// (mirrors `agents_diff.rs`).
#[derive(Clone, Debug, PartialEq)]
pub struct RoleRowView {
    /// Raw `config.model.roles` key (e.g. `kanban_decomposer`).
    pub role_key: String,
    /// Human display label — underscores to spaces, upper-cased.
    pub display_label: String,
    /// The role's configured provider, if any (drives the row's cascade).
    pub assigned_provider: Option<String>,
    /// The role's assigned model id — `None` means "— uses default".
    pub assigned_model: Option<String>,
    /// `true` when `assigned_model` is `Some(id)` but `id` is absent from the
    /// live catalog (a stale/removed model) — the row renders the amber
    /// `MISSING` pill and stays re-assignable.
    pub is_missing: bool,
}

/// Phase 46.9 Plan 15 (GAP-6): compute the rendered role-row view models from
/// the server-truth `roles` list + the live catalog id list. One view per
/// input row, in input order (the snapshot delivers exactly the six fixed
/// role keys). A role whose assigned model id is absent from `catalog_ids` is
/// flagged `is_missing` (still shown / re-assignable, never dropped).
///
/// `cfg_attr(not(wasm), allow(dead_code))`: live on the web (wasm) render
/// target (called from `ScreenModels`), but the native `--all-features` bin
/// build enters through the server path where the component tree is not
/// reachable — the established sibling-screen pattern (voice_mode.rs,
/// kanban/card.rs) for web-live helpers.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn compute_role_row_views(
    roles: &[crate::server::api::ModelRoleAssignment],
    catalog_ids: &[String],
) -> Vec<RoleRowView> {
    roles
        .iter()
        .map(|role| {
            let assigned_model = role.model.clone();
            let is_missing = assigned_model
                .as_ref()
                .map(|m| !catalog_ids.iter().any(|c| c == m))
                .unwrap_or(false);
            RoleRowView {
                role_key: role.role_key.clone(),
                display_label: role.role_key.replace('_', " ").to_uppercase(),
                assigned_provider: role.provider.clone(),
                assigned_model,
                is_missing,
            }
        })
        .collect()
}

/// Phase 46.9 Plan 15 (GAP-1): pure derivation of the model-select option
/// list from a provider-sourced `ProviderModelsSnapshot`. The options are the
/// snapshot's models (provider-sourced, NOT the flat global catalog). The
/// currently-assigned id is prepended when it is non-empty and absent from
/// the provider list, so a stale assignment stays selectable / re-assignable
/// instead of vanishing from its own dropdown.
///
/// `cfg_attr(not(wasm), allow(dead_code))`: web-live (called from
/// `ProviderModelCascade`); native `--all-features` bin sees the component
/// tree as unreachable (server entry). See `compute_role_row_views`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn compute_model_options(
    snapshot: Option<&crate::server::api::ProviderModelsSnapshot>,
    assigned_id: Option<&str>,
) -> Vec<String> {
    let mut options: Vec<String> = snapshot.map(|s| s.models.clone()).unwrap_or_default();
    if let Some(id) = assigned_id {
        let id = id.trim();
        if !id.is_empty() && !options.iter().any(|o| o == id) {
            options.insert(0, id.to_string());
        }
    }
    options
}

/// Phase 50.5 (D-14/D-15): group an integer's digits with `,` for the
/// CONTEXT WINDOW placeholder — an explicit digit walk, not a formatting
/// crate (this file's Artifacts note: `10_000_000` is well within `u32`
/// range and needs no external dependency to render with separators).
///
/// `cfg_attr(not(wasm), allow(dead_code))`: web-live helper, see
/// `compute_role_row_views`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn format_thousands(n: u32) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(bytes.len() + bytes.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Phase 50.5 (D-14/D-15, UI-SPEC Copywriting Contract): the CONTEXT WINDOW
/// field's placeholder — states the resolved value and which tier produced
/// it, so an empty field never leaves the operator guessing what "use
/// resolved" currently means. `None` (the value has not resolved yet, e.g.
/// mid-fetch) renders as an em dash with no provenance word, per UI-SPEC
/// E2/E3's loading state. Consumes `provenance_word` from Plan 05 rather
/// than duplicating its four words.
///
/// `cfg_attr(not(wasm), allow(dead_code))`: web-live helper, see
/// `compute_role_row_views`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn window_placeholder(
    resolved: Option<u32>,
    source: crate::server::api::ContextWindowSource,
) -> String {
    match resolved {
        None => "resolved: —".to_string(),
        Some(n) => format!(
            "resolved: {} ({})",
            format_thousands(n),
            crate::server::api::provenance_word(source)
        ),
    }
}

/// Phase 50.5 (D-14/D-15, UI-SPEC Copywriting Contract): the CONTEXT WINDOW
/// field's validation error string — verbatim, so both the client-side
/// gate and the server's `validate_models_roles_payload` rejection (Plan 05)
/// read as the same operator-facing message.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const WINDOW_VALIDATION_ERROR: &str =
    "Enter a whole number of tokens (e.g. 200000), or leave blank to use the resolved value.";

/// Phase 50.5 (D-14/D-15): parse the CONTEXT WINDOW input. Blank (post-trim)
/// means "clear the override, use the resolved value" — `Ok(None)`, never an
/// error. Anything unparseable, `0`, or above the 10,000,000 ceiling Plan 05
/// enforces server-side is rejected here too, so the operator gets an inline
/// message before a round trip to the server.
///
/// `cfg_attr(not(wasm), allow(dead_code))`: web-live helper, see
/// `compute_role_row_views`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn parse_window_input(raw: &str) -> Result<Option<u32>, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match trimmed.parse::<u32>() {
        Ok(0) => Err(WINDOW_VALIDATION_ERROR),
        Ok(n) if n > 10_000_000 => Err(WINDOW_VALIDATION_ERROR),
        Ok(n) => Ok(Some(n)),
        Err(_) => Err(WINDOW_VALIDATION_ERROR),
    }
}

/// Phase 50.5 (D-11, UI-SPEC Copywriting Contract): decide whether a
/// configured-model-id drift note should render, and if so, its two copy
/// strings (`(body, title)`). Returns `None` whenever any suppression
/// condition holds — `is_missing` (the `MISSING` pill already owns this
/// row), `loading` (the served-id list is unresolved — UI-SPEC E4 loading),
/// `fell_back` (a degraded catalog is not the provider's own list — UI-SPEC
/// E4 error), an empty `served_ids` list, a blank `configured_id`, or
/// `served_ids` already containing `configured_id` (no drift). `None` here
/// is deliberately NOT an assertion that no drift exists — it only means
/// this call site cannot currently prove one.
///
/// `cfg_attr(not(wasm), allow(dead_code))`: web-live helper, see
/// `compute_role_row_views`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn drift_note(
    served_ids: &[String],
    configured_id: &str,
    provider: &str,
    fell_back: bool,
    loading: bool,
    is_missing: bool,
) -> Option<(String, String)> {
    if is_missing || loading || fell_back {
        return None;
    }
    let configured_id = configured_id.trim();
    if configured_id.is_empty() || served_ids.is_empty() {
        return None;
    }
    if served_ids.iter().any(|s| s == configured_id) {
        return None;
    }
    let served_id = served_ids.first()?;
    let body = format!("served as \"{served_id}\" by {provider} — your config names \"{configured_id}\"");
    let title = format!(
        "{provider} does not list \"{configured_id}\" in its model catalog, but it IS serving \
         \"{served_id}\" — the assigned model still works. This is a naming mismatch, not a \
         missing model."
    );
    Some((body, title))
}

/// Phase 46.9 Plan 15 (GAP-1): which config slot a `ProviderModelCascade`
/// writes on ASSIGN. `Default` writes `config.model.default` + `.provider`;
/// `Role` upserts one `config.model.roles` entry.
///
/// `cfg_attr(not(wasm), allow(dead_code))`: constructed only in the web-live
/// component tree (`ScreenModels`/`RolePickerRow`), which the native
/// `--all-features` bin build does not reach. See `compute_role_row_views`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Clone, PartialEq)]
enum CascadeKind {
    Default,
    Role(String),
}

#[component]
pub fn ScreenModels(is_active: bool) -> Element {
    // Gap 1 / CR-01 fix: `use_resource` instead of `use_server_future` + `?`
    // — see module doc + agents.rs ~41-58. No `?` early-return can precede
    // any hook declared later in this render.
    let models_resource =
        use_resource(move || async move { crate::server::api::list_models().await });

    // Phase 46.9 Plan 15 (GAP-6): monotonic refresh nonce. The roles resource
    // closure READS it synchronously, so bumping it (in an `on_saved`
    // callback) makes `use_resource` re-run and re-fetch — this is the
    // seed-effect-free refresh path (no local snapshot signal, no seeded
    // guard, no resource restart-method call).
    let mut roles_refresh_nonce = use_signal(|| 0u32);
    let roles_resource = use_resource(move || {
        // Subscribe to the nonce in the SYNC prefix so a bump re-runs us.
        let _nonce = roles_refresh_nonce();
        async move { crate::server::api::get_models_roles_config().await }
    });
    // GAP-1: read-only source of the provider dropdown's option list. Never
    // written from this screen (provider create/edit stays out of scope —
    // Models scope is default + roles + provider *selection* only).
    let provider_options_resource = use_resource(move || async move {
        crate::server::provider_config_api::get_provider_config().await
    });

    // Phase 50.4 Plan 01 follow-up (team-lead, 2026-09-08): the Phase 46.9
    // Plan 02 dismissible "Restart required" banner + its `restart_banner_
    // visible` signal are REMOVED here — D-11 states the shared
    // `ApplyConfigBanner` supersedes it. This screen now reads/writes the
    // same root-provided `ApplyConfigPendingCtx` flag Providers uses (see
    // state.rs doc comment), so a Default-model save here — the ONLY
    // control that changes the ACTIVE provider — can raise the identical
    // APPLY NOW banner Providers raises after its own save.
    let mut apply_config_pending = use_context::<crate::state::ApplyConfigPendingCtx>().0;

    // Phase 49.4 hotfix: `list_models()` returns the WHOLE model registry —
    // for a large provider (e.g. OpenRouter, 300+ models) that is hundreds of
    // read-only ModelCards. Rendering them all at once locked the
    // single-threaded WASM client. The cards are informational (their EDIT
    // button is inert; the interactive selects are the role cascades above),
    // so each family renders at most CATALOG_CARD_CAP cards until the operator
    // opts into the full list.
    let mut show_all_models = use_signal(|| false);

    // Extract data and error flags BEFORE rsx! — signal borrow discipline
    // per iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let models_list: Vec<crate::server::api::ModelInfo> = match models_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let models_load_error = matches!(models_resource(), Some(Err(_)));
    let models_loading = models_resource().is_none();

    // GAP-6: the rendered snapshot comes DIRECTLY from the resource each
    // render — never a seed-once local signal.
    let roles_snapshot: Option<crate::server::api::ModelsRolesSnapshot> = match roles_resource() {
        Some(Ok(snap)) => Some(snap),
        _ => None,
    };
    let roles_load_error = matches!(roles_resource(), Some(Err(_)));
    let roles_loading = roles_resource().is_none();

    let provider_names: Vec<String> = match provider_options_resource() {
        Some(Ok(snap)) => snap.providers.iter().map(|p| p.name.clone()).collect(),
        _ => Vec::new(),
    };

    // Phase 46.9 Plan 02: token bug fix — the error state used to reference
    // design-tokens.css/tokens.css color/font-size variables that don't
    // resolve against this screen's site.css/screens.css vocabulary. Real
    // error state now, in `--red`/`12px`.
    let load_error = models_load_error || roles_load_error;
    // Distinct loading state (ghost rows), never conflated with "no data yet"
    // (the models.rs `None`==empty bug this phase is also fixing).
    let is_loading = !load_error && (models_loading || roles_loading);

    // Family dedup loop — owned Vec<String>, source order preserved.
    // &'static str would fail because ModelInfo.family is a String.
    let mut families: Vec<String> = Vec::new();
    for m in models_list.iter() {
        if !families.contains(&m.family) {
            families.push(m.family.clone());
        }
    }

    let catalog_ids: Vec<String> = models_list.iter().map(|m| m.id.clone()).collect();
    let write_enabled = roles_snapshot
        .as_ref()
        .map(|s| s.web_config_write_enabled)
        .unwrap_or(false);
    let default_model_value = roles_snapshot
        .as_ref()
        .map(|s| s.default_model.clone())
        .unwrap_or_default();
    let provider_value = roles_snapshot
        .as_ref()
        .map(|s| s.provider.clone())
        .unwrap_or_default();
    let role_rows: Vec<crate::server::api::ModelRoleAssignment> = roles_snapshot
        .as_ref()
        .map(|s| s.roles.clone())
        .unwrap_or_default();
    // GAP-6: the six rendered role rows come from the pure derivation fn,
    // called from the RSX below.
    let role_row_views = compute_role_row_views(&role_rows, &catalog_ids);

    let default_is_missing = {
        let d = default_model_value.trim();
        !d.is_empty() && !catalog_ids.iter().any(|c| c == &default_model_value)
    };

    // Phase 50.5 (D-14/D-15): the default card's resolved window, provenance,
    // and stored override — from Plan 05's snapshot fields, one shared
    // evaluation with the topbar marker.
    let default_resolved_window: Option<u32> =
        roles_snapshot.as_ref().map(|s| s.default_resolved_context_length);
    let default_resolved_source = roles_snapshot
        .as_ref()
        .map(|s| s.default_resolved_context_source)
        .unwrap_or_default();
    let default_window_override =
        roles_snapshot.as_ref().and_then(|s| s.default_context_override);

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-models",
            "data-screen-label": "05 Models",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 05" }
                    h1 { class: "screen-title", "Models" }
                }
                div { class: "screen-actions",
                    // Phase 46.9 Plan 02 (UI-SPEC Models CTA note): D-05 wires the
                    // global default + six role pickers only. This button has no
                    // backing action — deliberately unwired, not fabricated.
                    button { class: "btn btn--sm", "+ NEW CONFIG" }
                }
            }

            // Phase 50.4 Plan 01 follow-up (D-11): the shared apply-config
            // banner supersedes the old dismissible "Restart required"
            // banner on this screen — same mount position, same trigger
            // signal Providers uses.
            crate::components::hermes_app::screens::apply_config_banner::ApplyConfigBanner {
                visible: apply_config_pending,
            }

            if load_error {
                div {
                    style: "color:var(--red);font-size:12px;",
                    p { style: "margin:0 0 2px 0;font-weight:700;", "Could not load models." }
                    p { style: "margin:0;", "Check the server connection and retry." }
                }
            } else if is_loading {
                div { class: "section-label", "DEFAULT MODEL" }
                div { class: "card", style: "opacity:0.35;", div { class: "card-title", "···" } }

                div { class: "section-label", "ROLE ASSIGNMENTS" }
                div { class: "grid wide",
                    for i in 0..6 {
                        div { key: "{i}", class: "card", style: "opacity:0.35;",
                            div { class: "card-title", "···" }
                            div { class: "card-meta", "loading" }
                        }
                    }
                }
            } else {
                div { class: "panel",
                    div { class: "section-label", "DEFAULT MODEL" }
                    if default_model_value.trim().is_empty() {
                        div { style: "margin-bottom:10px;",
                            p { style: "color:var(--text);font-size:13px;font-weight:700;margin:0 0 4px 0;",
                                "No default model set."
                            }
                            p { style: "color:var(--gray);font-size:11px;margin:0;",
                                "Choose a provider, then a model — role assignments fall back to this default."
                            }
                        }
                    }
                    if default_is_missing {
                        div { style: "margin-bottom:10px;display:flex;align-items:center;gap:8px;flex-wrap:wrap;",
                            span { class: "pill amber", "MISSING" }
                            p { style: "color:var(--gray);font-size:11px;margin:0;",
                                "Stored default \"{default_model_value}\" is no longer in the catalog — choose a replacement."
                            }
                        }
                    }
                    // GAP-1: provider-first dependent cascade (shared with the role rows).
                    ProviderModelCascade {
                        kind: CascadeKind::Default,
                        initial_provider: provider_value.clone(),
                        initial_model: default_model_value.clone(),
                        provider_options: provider_names.clone(),
                        write_enabled,
                        allow_unset: false,
                        resolved_window: default_resolved_window,
                        resolved_source: default_resolved_source,
                        initial_window: default_window_override,
                        is_missing: default_is_missing,
                        on_saved: move |_| {
                            // Phase 50.4 Plan 01 follow-up: this is the ONLY
                            // control that changes config.model.provider (the
                            // ACTIVE provider) — raise the shared apply-config
                            // banner so the operator has a path to make it
                            // live without a restart.
                            apply_config_pending.set(true);
                            // GAP-6: refresh by bumping the nonce so the roles
                            // resource re-runs — never a resource restart method.
                            let next = roles_refresh_nonce.peek().wrapping_add(1);
                            roles_refresh_nonce.set(next);
                        },
                    }
                }

                div { class: "section-label",
                    "ROLE ASSIGNMENTS "
                    span { class: "count", "· {role_row_views.len()} configs" }
                }
                div { class: "grid wide",
                    // Phase 50.5 (D-14/D-15): `role_row_views` and `role_rows`
                    // are both derived from `roles_snapshot.roles` in the same
                    // fixed order (`compute_role_row_views` preserves input
                    // order) — zip rather than re-deriving a lookup so each
                    // row's resolved-window fields come from its own entry.
                    for (view, role) in role_row_views.iter().zip(role_rows.iter()) {
                        RolePickerRow {
                            key: "{view.role_key}",
                            view: view.clone(),
                            provider_options: provider_names.clone(),
                            write_enabled,
                            resolved_window: Some(role.resolved_context_length),
                            resolved_source: role.resolved_context_source,
                            initial_window: role.context_length,
                            on_saved: move |_| {
                                // Phase 50.4 Plan 01 follow-up: a role-model
                                // save changes `config.model.roles`, which
                                // D-08's config swap also republishes — raise
                                // the same shared banner as the Default card
                                // above rather than leaving role saves with
                                // no apply-now affordance now that the old
                                // restart banner is gone.
                                apply_config_pending.set(true);
                                let next = roles_refresh_nonce.peek().wrapping_add(1);
                                roles_refresh_nonce.set(next);
                            },
                        }
                    }
                }

                {
                    // Phase 49.4 hotfix: single toggle gating the read-only
                    // catalog card list (see `show_all_models`). Only shown
                    // when the catalog is large enough to have been capped.
                    const CATALOG_CARD_CAP: usize = 12;
                    let total_models = models_list.len();
                    let show_all = *show_all_models.read();
                    let any_capped = !show_all
                        && families.iter().any(|f| {
                            models_list.iter().filter(|m| &m.family == f).count() > CATALOG_CARD_CAP
                        });
                    rsx! {
                        if total_models > CATALOG_CARD_CAP {
                            div {
                                style: "display:flex;align-items:center;gap:10px;margin:14px 0 6px;flex-wrap:wrap;",
                                span { class: "section-label", "MODEL CATALOG · {total_models} models" }
                                button {
                                    class: "btn btn--ghost btn--sm",
                                    onclick: move |_| {
                                        let cur = *show_all_models.read();
                                        show_all_models.set(!cur);
                                    },
                                    if show_all { "SHOW FEWER" } else { "SHOW ALL" }
                                }
                                if any_capped {
                                    span { style: "color:var(--gray);font-size:11px;",
                                        "showing {CATALOG_CARD_CAP} per family — SHOW ALL to render the full catalog"
                                    }
                                }
                            }
                        }
                        for family in families.iter() {
                            {
                                // Snapshot rows for this family — owned Vec, no borrow into RSX.
                                let family_name = family.clone();
                                let rows: Vec<crate::server::api::ModelInfo> = models_list
                                    .iter()
                                    .filter(|m| m.family == family_name)
                                    .cloned()
                                    .collect();
                                let count = rows.len();
                                let hidden = if show_all {
                                    0
                                } else {
                                    count.saturating_sub(CATALOG_CARD_CAP)
                                };
                                let shown: Vec<crate::server::api::ModelInfo> = if show_all {
                                    rows.clone()
                                } else {
                                    rows.iter().take(CATALOG_CARD_CAP).cloned().collect()
                                };
                                rsx! {
                                    div { key: "{family_name}", class: "model-family-group",
                                        div { class: "section-label",
                                            "{family_name} "
                                            span { class: "count", "· {count} configs" }
                                        }
                                        div { class: "grid wide",
                                            for m in shown.iter() {
                                                ModelCard { key: "{m.id}", model: m.clone() }
                                            }
                                        }
                                        if hidden > 0 {
                                            div { style: "color:var(--gray);font-size:11px;margin:4px 0 2px;",
                                                "· {hidden} more hidden"
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
}

#[component]
fn ModelCard(model: crate::server::api::ModelInfo) -> Element {
    let is_default = model.status == "DEFAULT";
    rsx! {
        div {
            class: "card",
            class: if is_default { "is-active" },
            div { class: "card-head",
                div { class: "card-icon", "◉" }
                div { style: "flex:1",
                    div { class: "card-title", "{model.id}" }
                    div { class: "card-meta",
                        "{model.family} · {model.context_window} context"
                    }
                }
                if is_default {
                    span { class: "pill teal", "{model.status}" }
                }
            }
            div { class: "card-footer",
                div { style: "display:flex;gap:14px;font-size:10px;color:var(--gray);letter-spacing:0.06em;",
                    span { "CTX " span { style: "color:var(--teal);font-weight:700", "{model.context_window}" } }
                    span { "STATE " span { style: "color:var(--teal);font-weight:700", "{model.status}" } }
                }
                button { class: "btn btn--ghost btn--sm", "EDIT" }   // no onclick — out of scope
            }
        }
    }
}

/// Phase 46.9 Plan 15 (GAP-1): the shared provider-first → dependent-model
/// cascade. Backs BOTH the global default card and the six role rows so they
/// cannot diverge. The provider select is rendered FIRST; changing it
/// re-fetches `list_provider_models(provider)` and the model select's options
/// come from that provider-sourced snapshot (`compute_model_options`),
/// falling back to the full catalog (with a dim note) when the provider has
/// no `/models` endpoint.
///
/// Frozen-control fix: NO `value:` is bound on either `<select>` — each
/// `<option>` carries an explicit `selected` state derived from a `.read()`
/// signal, so the shown option follows the signal on every render (the
/// controlled-`value:` snap-back that made the round-1 selects look frozen is
/// gone). The gated ASSIGN write rides `update_models_roles_config`; when the
/// write gate is closed the selects + button disable and surface the
/// 'Config writes are disabled' reason.
#[component]
fn ProviderModelCascade(
    kind: CascadeKind,
    initial_provider: String,
    initial_model: String,
    provider_options: Vec<String>,
    write_enabled: bool,
    allow_unset: bool,
    // Phase 50.5 (D-14/D-15): this row's resolved context window + which
    // tier produced it (feeds the placeholder), the currently stored
    // per-(provider, model) override (seeds the input), and whether the
    // MISSING pill already owns this row (suppresses the D-11 drift note).
    resolved_window: Option<u32>,
    resolved_source: crate::server::api::ContextWindowSource,
    initial_window: Option<u32>,
    is_missing: bool,
    on_saved: EventHandler<()>,
) -> Element {
    let mut selected_provider = use_signal(|| initial_provider.clone());
    let selected_model = use_signal(|| initial_model.clone());
    let mut saving = use_signal(|| false);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);
    // Phase 50.5 (D-14): seeded from the STORED override, not the resolved
    // value — an empty field means "use resolved," and pre-filling it with
    // the resolved number would turn every row into an override the moment
    // ASSIGN was pressed.
    let mut window_input = use_signal(|| initial_window.map(|n| n.to_string()).unwrap_or_default());

    // GAP-1: the DEPENDENT model list — re-fetched whenever the provider
    // signal changes (read in the sync prefix so `use_resource` re-runs).
    let models_resource = use_resource(move || {
        let provider = selected_provider();
        async move { crate::server::api::list_provider_models(provider).await }
    });

    let provider_val = selected_provider.read().clone();
    let model_val = selected_model.read().clone();
    let is_saving = *saving.read();
    let error_val = error_msg.read().clone();
    let window_text = window_input.read().clone();

    let snapshot = match models_resource() {
        Some(Ok(s)) => Some(s),
        _ => None,
    };
    let models_loading = models_resource().is_none();
    let fell_back = snapshot.as_ref().map(|s| s.fell_back).unwrap_or(false);
    let assigned_ref = if model_val.trim().is_empty() {
        None
    } else {
        Some(model_val.as_str())
    };
    let model_options = compute_model_options(snapshot.as_ref(), assigned_ref);

    // Phase 50.5 (D-14/D-15): parse the window input, derive the placeholder,
    // and compute the D-11 drift note — all owned locals read before `rsx!`.
    let window_parsed = parse_window_input(&window_text);
    let window_error: Option<&'static str> = window_parsed.as_ref().err().copied();
    let window_placeholder_text = window_placeholder(resolved_window, resolved_source);
    let served_ids: Vec<String> = snapshot.as_ref().map(|s| s.models.clone()).unwrap_or_default();
    let note = drift_note(&served_ids, &model_val, &provider_val, fell_back, models_loading, is_missing);

    let can_save = write_enabled && !is_saving && window_error.is_none();
    let gate_title = if !write_enabled {
        "Config writes are disabled"
    } else {
        ""
    };

    rsx! {
        div { style: "display:flex;flex-direction:column;gap:8px;width:100%;",
            // Phase 50.5 (D-11): the drift note is the FIRST child — unclipped,
            // wraps freely, sits adjacent to the control where the model is
            // chosen. `is_missing` is enforced inside `drift_note` itself, so
            // the two conditions never render together.
            if let Some((body, title)) = note {
                p {
                    title: "{title}",
                    style: "color:var(--gray);font-size:11px;margin:0 0 10px 0;",
                    "{body}"
                }
            }
            div { style: "display:flex;gap:10px;align-items:center;flex-wrap:wrap;",
                // PROVIDER select FIRST — its onchange drives the dependent model list.
                select {
                    class: "voice-settings-select",
                    disabled: !write_enabled || is_saving,
                    title: "{gate_title}",
                    onchange: move |evt| {
                        error_msg.set(None);
                        selected_provider.set(evt.value());
                        // Phase 50.5 CR-01 fix: `providers.<p>.models.<m>.context_length`
                        // is keyed on the (provider, model) PAIR — switching
                        // the provider changes that key even when the model
                        // id string stays the same, so the window text seeded
                        // for the OLD pair no longer describes anything real.
                        // Reset to blank (== "use resolved", D-14) rather than
                        // guessing the new pair's stored override: the client
                        // has no per-model override data for an
                        // arbitrary/not-yet-selected model (`ProviderModelsSnapshot`
                        // carries only served ids, not their overrides), so a
                        // silent reseed would just be a different guess. A
                        // blank field is visibly "no override" and the
                        // placeholder's "resolved: N (tier)" text still shows
                        // the pair's real resolved window.
                        window_input.set(String::new());
                    },
                    if provider_val.trim().is_empty() {
                        option { value: "", selected: true, "— select a provider —" }
                    }
                    for name in provider_options.iter() {
                        option {
                            key: "{name}",
                            value: "{name}",
                            selected: name == &provider_val,
                            "{name}"
                        }
                    }
                }
                crate::components::hermes_app::screens::model_picker::ModelPickerField {
                    value: selected_model,
                    all_options: model_options.clone(),
                    seeded: initial_model.clone(),
                    disabled: !write_enabled || is_saving || models_loading,
                    title: gate_title.to_string(),
                    placeholder: if allow_unset { "— uses default — (type to search)".to_string() } else { "type to search models…".to_string() },
                    // Phase 50.5 CR-01 fix: a confirmed row pick is a new
                    // (provider, model) pair — same rationale as the provider
                    // `onchange` reset above, fired on the OTHER half of that
                    // pair. Only the row `onclick` inside `ModelPickerField`
                    // calls this (never `oninput`), so filtering-while-typing
                    // does not clear the field out from under an operator who
                    // has not actually changed models yet.
                    on_change: move |_new_model: String| {
                        window_input.set(String::new());
                    },
                }
                // Phase 50.5 (D-14/D-15): CONTEXT WINDOW — styled to match the
                // read-only `CTX` stat on `ModelCard` (font-size:10px;
                // color:var(--teal);font-weight:700), not `.section-label`
                // (whose `::before` dot bullet and `margin-bottom` are a
                // section-heading affordance this inline row does not want).
                // Never gated on `models_loading` (UI-SPEC E2): a slow catalog
                // fetch must not block an override.
                span {
                    style: "font-size:10px;color:var(--gray);letter-spacing:0.06em;text-transform:uppercase;",
                    "CONTEXT WINDOW"
                }
                input {
                    r#type: "number",
                    min: "0",
                    class: "voice-settings-select",
                    style: "font-size:10px;color:var(--teal);font-weight:700;width:110px;",
                    placeholder: "{window_placeholder_text}",
                    value: "{window_text}",
                    disabled: !write_enabled || is_saving,
                    oninput: move |evt| window_input.set(evt.value()),
                }
                button {
                    class: "btn btn--sm",
                    disabled: !can_save,
                    title: "{gate_title}",
                    onclick: move |_| {
                        // Pattern B: owned locals read before spawn (clippy.toml —
                        // no signal borrow across .await).
                        let provider_id = selected_provider.read().clone();
                        let model_raw = selected_model.read().clone();
                        let window_raw = window_input.read().clone();
                        let kind_local = kind.clone();
                        // Belt-and-braces: `can_save` already blocks this click
                        // when the parse errors, so this is a defensive re-check,
                        // not the primary gate.
                        let Ok(window_parsed) = parse_window_input(&window_raw) else {
                            return;
                        };
                        saving.set(true);
                        error_msg.set(None);
                        spawn(async move {
                            let provider_opt = if provider_id.trim().is_empty() {
                                None
                            } else {
                                Some(provider_id)
                            };
                            let model_opt = if model_raw.trim().is_empty() {
                                None
                            } else {
                                Some(model_raw)
                            };
                            // Phase 50.5 (D-14/D-15/D-16): always send
                            // `apply_context_length: true` — this control
                            // always reports its current state, which is what
                            // makes a blank field mean "clear" rather than
                            // "unchanged" (three-state semantics, api.rs).
                            let payload = match kind_local {
                                CascadeKind::Default => crate::server::api::ModelsRolesWritePayload {
                                    default_model: model_opt,
                                    provider: provider_opt,
                                    roles: Vec::new(),
                                    context_length: window_parsed,
                                    apply_context_length: true,
                                },
                                CascadeKind::Role(role_key) => {
                                    crate::server::api::ModelsRolesWritePayload {
                                        default_model: None,
                                        provider: None,
                                        roles: vec![crate::server::api::ModelRoleAssignment {
                                            role_key,
                                            provider: provider_opt,
                                            model: model_opt,
                                            context_length: window_parsed,
                                            apply_context_length: true,
                                            ..Default::default()
                                        }],
                                        ..Default::default()
                                    }
                                }
                            };
                            match crate::server::api::update_models_roles_config(payload).await {
                                Ok(()) => {
                                    saving.set(false);
                                    on_saved.call(());
                                }
                                Err(_e) => {
                                    saving.set(false);
                                    error_msg.set(Some("Save failed. Check server logs.".to_string()));
                                }
                            }
                        });
                    },
                    if is_saving { "SAVING…" } else { "ASSIGN" }
                }
            }
            if fell_back && !models_loading {
                p { style: "color:var(--gray);font-size:11px;margin:0;",
                    "This provider exposes no model list — showing the full catalog."
                }
            }
            if let Some(err) = error_val {
                p { style: "color:var(--red);font-size:11px;margin:0;", "{err}" }
            }
            // Phase 50.5 (D-14/D-15): the CONTEXT WINDOW validation error —
            // sibling of the save-failure paragraph above, identical style,
            // no new error surface.
            if let Some(err) = window_error {
                p { style: "color:var(--red);font-size:11px;margin:0;", "{err}" }
            }
        }
    }
}

/// Phase 46.9 Plan 02 (D-05) / Plan 15 (GAP-6/GAP-1): one of the six fixed
/// role rows. The display label + stale-`MISSING` decision arrive precomputed
/// in `view` (from `compute_role_row_views`); the interactive part is the
/// shared `ProviderModelCascade`, so the row is provider-first → dependent
/// model just like the global default card.
#[component]
fn RolePickerRow(
    view: RoleRowView,
    provider_options: Vec<String>,
    write_enabled: bool,
    resolved_window: Option<u32>,
    resolved_source: crate::server::api::ContextWindowSource,
    initial_window: Option<u32>,
    on_saved: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "card",
            div { class: "card-head",
                div { style: "flex:1;min-width:0;",
                    div { class: "card-title", "{view.display_label}" }
                    div {
                        class: "card-meta",
                        title: "{view.assigned_model.clone().unwrap_or_default()}",
                        style: "max-width:280px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
                        if let Some(ref m) = view.assigned_model {
                            "{m}"
                        } else {
                            span { style: "color:var(--gray);", "— uses default" }
                        }
                    }
                }
                if view.is_missing {
                    span { class: "pill amber", "MISSING" }
                }
            }
            div { class: "card-footer", style: "flex-wrap:wrap;gap:8px;",
                ProviderModelCascade {
                    kind: CascadeKind::Role(view.role_key.clone()),
                    initial_provider: view.assigned_provider.clone().unwrap_or_default(),
                    initial_model: view.assigned_model.clone().unwrap_or_default(),
                    provider_options,
                    write_enabled,
                    allow_unset: true,
                    resolved_window,
                    resolved_source,
                    initial_window,
                    is_missing: view.is_missing,
                    on_saved,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        compute_model_options, compute_role_row_views, drift_note, format_thousands,
        parse_window_input, window_placeholder,
    };
    use crate::server::api::{ContextWindowSource, ModelRoleAssignment, ProviderModelsSnapshot};

    fn role(key: &str, provider: Option<&str>, model: Option<&str>) -> ModelRoleAssignment {
        ModelRoleAssignment {
            role_key: key.to_string(),
            provider: provider.map(|p| p.to_string()),
            model: model.map(|m| m.to_string()),
            ..Default::default()
        }
    }

    /// GAP-6 stale-distractor test. The fixture is MULTI-ENTRY and includes a
    /// deliberate stale distractor (`kanban_judge` → `retired-model-x`, an id
    /// absent from the catalog). Expected values are written out LITERALLY —
    /// none are computed by calling `compute_role_row_views` again — so a
    /// broken derivation cannot pass by agreeing with itself.
    #[test]
    fn stale_assignment_flags_exactly_the_missing_row() {
        // Catalog deliberately EXCLUDES "retired-model-x".
        let catalog = vec![
            "gpt-4o".to_string(),
            "claude-sonnet".to_string(),
            "llama-70b".to_string(),
        ];
        let roles = vec![
            role("fast", Some("openai"), Some("gpt-4o")), // live
            role("kanban_decomposer", None, None),        // uses default
            role("kanban_judge", Some("legacy"), Some("retired-model-x")), // STALE distractor
        ];

        let views = compute_role_row_views(&roles, &catalog);

        assert_eq!(views.len(), 3, "one view per input row, input order preserved");

        // Row 0: live assignment — literal expectations.
        assert_eq!(views[0].role_key, "fast");
        assert_eq!(views[0].display_label, "FAST");
        assert_eq!(views[0].assigned_provider, Some("openai".to_string()));
        assert_eq!(views[0].assigned_model, Some("gpt-4o".to_string()));
        assert!(!views[0].is_missing, "a live catalog id is NOT missing");

        // Row 1: uses-default — no model, never missing.
        assert_eq!(views[1].role_key, "kanban_decomposer");
        assert_eq!(views[1].display_label, "KANBAN DECOMPOSER");
        assert_eq!(views[1].assigned_provider, None);
        assert_eq!(views[1].assigned_model, None);
        assert!(!views[1].is_missing, "an unassigned role is NOT missing");

        // Row 2: the STALE distractor — the only row flagged missing.
        assert_eq!(views[2].role_key, "kanban_judge");
        assert_eq!(views[2].display_label, "KANBAN JUDGE");
        assert_eq!(views[2].assigned_provider, Some("legacy".to_string()));
        assert_eq!(views[2].assigned_model, Some("retired-model-x".to_string()));
        assert!(
            views[2].is_missing,
            "the stale distractor (retired-model-x absent from catalog) MUST be flagged missing"
        );

        // Exactly one row is missing — the assertion is over the whole set,
        // not a re-run of the derivation.
        let missing_count = views.iter().filter(|v| v.is_missing).count();
        assert_eq!(missing_count, 1, "exactly one row is missing");
    }

    /// GAP-1 provider-sourced test. The provider snapshot lists ONLY
    /// `prov-a`/`prov-b`. `catalog-only` is a distractor that WOULD appear in
    /// the flat global catalog but is NOT in this provider's list — the
    /// derived options must therefore NOT contain it, proving the list is
    /// provider-sourced rather than catalog-sourced. A stale assigned id
    /// (`stale-x`, absent from the provider list) is prepended so it stays
    /// selectable. Expected values are literal.
    #[test]
    fn model_options_are_provider_sourced_not_catalog_sourced() {
        let snap = ProviderModelsSnapshot {
            models: vec!["prov-a".to_string(), "prov-b".to_string()],
            fell_back: false,
        };

        // Stale assignment absent from the provider list → prepended, still
        // selectable; the catalog-only distractor never appears.
        let opts = compute_model_options(Some(&snap), Some("stale-x"));
        assert_eq!(
            opts,
            vec![
                "stale-x".to_string(),
                "prov-a".to_string(),
                "prov-b".to_string()
            ],
            "stale assigned id is prepended; options are exactly the provider list otherwise"
        );
        assert!(
            !opts.iter().any(|o| o == "catalog-only"),
            "a catalog-only distractor must NOT appear — options are provider-sourced"
        );

        // An assigned id already in the provider list is NOT duplicated.
        let opts_present = compute_model_options(Some(&snap), Some("prov-a"));
        assert_eq!(
            opts_present,
            vec!["prov-a".to_string(), "prov-b".to_string()],
            "an already-present assigned id is not duplicated"
        );

        // No snapshot + no assignment → empty option list (loading state).
        assert!(
            compute_model_options(None, None).is_empty(),
            "no snapshot and no assignment yields no options"
        );
    }

    /// Phase 50.5 Task 1: `format_thousands` literal examples from `<behavior>`.
    #[test]
    fn format_thousands_matches_the_spec_examples() {
        assert_eq!(format_thousands(1_000_000), "1,000,000");
        assert_eq!(format_thousands(128_000), "128,000");
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
    }

    /// Phase 50.5 Task 1: `window_placeholder`'s loading (no provenance word)
    /// and resolved (provenance word per tier) cases, literal expectations.
    #[test]
    fn window_placeholder_states_resolved_and_provenance() {
        assert_eq!(window_placeholder(None, ContextWindowSource::Metadata), "resolved: —");
        assert_eq!(
            window_placeholder(Some(1_000_000), ContextWindowSource::Metadata),
            "resolved: 1,000,000 (cache)"
        );
        assert_eq!(
            window_placeholder(Some(256_000), ContextWindowSource::GlobalPin),
            "resolved: 256,000 (pin)"
        );
        assert_eq!(
            window_placeholder(Some(128_000), ContextWindowSource::Fallback),
            "resolved: 128,000 (default)"
        );
    }

    /// Phase 50.5 Task 1: blank means clear (`Ok(None)`), a valid number
    /// parses, and garbage / zero / above-ceiling all carry the Copywriting
    /// Contract's validation string.
    #[test]
    fn parse_window_input_blank_means_clear() {
        assert_eq!(parse_window_input(""), Ok(None));
        assert_eq!(parse_window_input("   "), Ok(None));
        assert_eq!(parse_window_input("200000"), Ok(Some(200_000)));

        let expected_err =
            "Enter a whole number of tokens (e.g. 200000), or leave blank to use the resolved value.";
        assert_eq!(parse_window_input("abc"), Err(expected_err));
        assert_eq!(parse_window_input("-5"), Err(expected_err));
        assert_eq!(parse_window_input("0"), Err(expected_err));
        assert_eq!(parse_window_input("10000001"), Err(expected_err));
    }

    /// Phase 50.5 Task 1 (D-11 VALIDATION Wave 0 gap): every guard branch in
    /// `<behavior>` covered in one test body so a partial implementation
    /// cannot pass.
    #[test]
    fn drift_note_fires_on_the_moonshot_case_only() {
        let served = vec!["k3".to_string(), "k3-256k".to_string()];

        // The moonshot-shaped case: configured id absent from served list.
        let (body, title) = drift_note(&served, "kimi-k3", "moonshot", false, false, false)
            .expect("a genuine drift must produce a note");
        assert_eq!(
            body,
            "served as \"k3\" by moonshot — your config names \"kimi-k3\""
        );
        assert!(title.contains("This is a naming mismatch, not a missing model."));

        // is_missing wins — the MISSING pill already owns this row.
        assert_eq!(
            drift_note(&served, "kimi-k3", "moonshot", false, false, true),
            None,
            "is_missing must suppress the drift note"
        );

        // loading — the served-id list is unresolved.
        assert_eq!(
            drift_note(&served, "kimi-k3", "moonshot", false, true, false),
            None,
            "loading must suppress the drift note"
        );

        // fell_back — a degraded catalog is not the provider's own list.
        assert_eq!(
            drift_note(&served, "kimi-k3", "moonshot", true, false, false),
            None,
            "fell_back must suppress the drift note"
        );

        // empty served list — nothing to compare against.
        assert_eq!(
            drift_note(&[], "kimi-k3", "moonshot", false, false, false),
            None,
            "an empty served list must suppress the drift note"
        );

        // configured id present in the served list — no drift.
        assert_eq!(
            drift_note(&served, "k3", "moonshot", false, false, false),
            None,
            "a configured id present in the served list is not drift"
        );
    }
}
