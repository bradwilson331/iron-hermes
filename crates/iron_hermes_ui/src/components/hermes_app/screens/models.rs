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

    // Phase 46.9 Plan 02 (D-10): restart-required banner — appears after a
    // successful write this session, dismissible, reappears after the next
    // write. Starts hidden (not shown on a plain read-only page load).
    let mut restart_banner_visible = use_signal(|| false);

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

            // Phase 46.9 Plan 02 (D-10): restart-required banner, Providers/Models only.
            if *restart_banner_visible.read() {
                div {
                    class: "panel",
                    style: "border-color:var(--amber);flex-direction:row;align-items:center;justify-content:space-between;gap:14px;flex-wrap:wrap;",
                    p {
                        style: "color:var(--amber);font-size:12px;margin:0;flex:1;min-width:280px;",
                        "Restart required — provider and model changes take effect after restart. Schedule changes apply immediately."
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| restart_banner_visible.set(false),
                        "DISMISS"
                    }
                }
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
                        on_saved: move |_| {
                            restart_banner_visible.set(true);
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
                    for view in role_row_views.iter() {
                        RolePickerRow {
                            key: "{view.role_key}",
                            view: view.clone(),
                            provider_options: provider_names.clone(),
                            write_enabled,
                            on_saved: move |_| {
                                restart_banner_visible.set(true);
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
    on_saved: EventHandler<()>,
) -> Element {
    let mut selected_provider = use_signal(|| initial_provider.clone());
    let mut selected_model = use_signal(|| initial_model.clone());
    let mut saving = use_signal(|| false);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);
    // Phase 49.4 hotfix: stable unique `<datalist>` id per cascade instance —
    // the Models screen mounts one cascade per role (~7-8), and a native
    // `<select>` of a large provider's full model list (300+ for OpenRouter)
    // rendered that many times locked the single-threaded WASM client. The
    // model picker below is a filterable input + capped datalist instead.
    let list_id = use_hook(|| {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CASCADE_SEQ: AtomicU64 = AtomicU64::new(0);
        format!(
            "cascade-models-{}",
            CASCADE_SEQ.fetch_add(1, Ordering::Relaxed)
        )
    });

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
    // Phase 49.4 hotfix: render at most a bounded window of `<option>`s into
    // the datalist, filtered by whatever is currently typed, so a 300+-model
    // provider never renders 300 nodes per cascade. The assigned value is
    // always kept in the list (compute_model_options already prepends it when
    // absent), and typing narrows toward any model id.
    const MODEL_DATALIST_CAP: usize = 50;
    let model_filter = model_val.trim().to_ascii_lowercase();
    let datalist_options: Vec<String> = model_options
        .iter()
        .filter(|id| model_filter.is_empty() || id.to_ascii_lowercase().contains(&model_filter))
        .take(MODEL_DATALIST_CAP)
        .cloned()
        .collect();
    let model_total = model_options.len();

    let can_save = write_enabled && !is_saving;
    let gate_title = if !write_enabled {
        "Config writes are disabled"
    } else {
        ""
    };

    rsx! {
        div { style: "display:flex;flex-direction:column;gap:8px;width:100%;",
            div { style: "display:flex;gap:10px;align-items:center;flex-wrap:wrap;",
                // PROVIDER select FIRST — its onchange drives the dependent model list.
                select {
                    class: "voice-settings-select",
                    disabled: !write_enabled || is_saving,
                    title: "{gate_title}",
                    onchange: move |evt| {
                        error_msg.set(None);
                        selected_provider.set(evt.value());
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
                // MODEL picker — a filterable input + capped datalist rather
                // than a native <select> of the provider's entire model list.
                // The datalist renders at most MODEL_DATALIST_CAP matches for
                // the current text (see `datalist_options`), so a 300+-model
                // provider can't lock the client; typing narrows to any id.
                input {
                    class: "voice-settings-select",
                    list: "{list_id}",
                    value: "{model_val}",
                    disabled: !write_enabled || is_saving || models_loading,
                    title: "{gate_title}",
                    placeholder: if allow_unset { "— uses default — (type to search)" } else { "type to search models…" },
                    oninput: move |evt| {
                        error_msg.set(None);
                        selected_model.set(evt.value());
                    },
                }
                datalist { id: "{list_id}",
                    for id in datalist_options.iter() {
                        option { key: "{id}", value: "{id}" }
                    }
                }
                if model_total > datalist_options.len() {
                    span { style: "color:var(--gray);font-size:10px;white-space:nowrap;",
                        "{datalist_options.len()}/{model_total} — type to filter"
                    }
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
                        let kind_local = kind.clone();
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
                            let payload = match kind_local {
                                CascadeKind::Default => crate::server::api::ModelsRolesWritePayload {
                                    default_model: model_opt,
                                    provider: provider_opt,
                                    roles: Vec::new(),
                                },
                                CascadeKind::Role(role_key) => {
                                    crate::server::api::ModelsRolesWritePayload {
                                        default_model: None,
                                        provider: None,
                                        roles: vec![crate::server::api::ModelRoleAssignment {
                                            role_key,
                                            provider: provider_opt,
                                            model: model_opt,
                                        }],
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
                    on_saved,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{compute_model_options, compute_role_row_views};
    use crate::server::api::{ModelRoleAssignment, ProviderModelsSnapshot};

    fn role(key: &str, provider: Option<&str>, model: Option<&str>) -> ModelRoleAssignment {
        ModelRoleAssignment {
            role_key: key.to_string(),
            provider: provider.map(|p| p.to_string()),
            model: model.map(|m| m.to_string()),
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
}
