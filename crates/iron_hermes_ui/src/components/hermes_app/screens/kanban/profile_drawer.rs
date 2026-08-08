//! Phase 47.4 Plan 08 (D-02/D-04/D-06/D-07/D-09/D-10/D-11/D-13): the
//! profile detail drawer — the client half of the editing surface Plan 05
//! built server-side. Mounted the way `TaskDrawer` is today (unconditional
//! mount, renders nothing until `profile_id.read().is_some()`); opened
//! from both D-02 entry points (the dropdown's per-row EDIT chip and the
//! MANAGE ALL PROFILES footer link) — see `kanban.rs`.
//!
//! Five sections in fixed order (UI-SPEC Component Inventory §3): Identity
//! (dir path + health dot + honest D-11 status line), Provider/Model
//! (editable, D-04), Keys (editable table, D-04/D-07/D-13 — a `NOT IN
//! ROOT .ENV` row grows a `ProfileKeyRowEditor`), Verify (on-demand D-09
//! probe, reusing the wizard's shared doctor-block renderer), Save.
//!
//! Load-state discipline (E5/loading backstop): a profile-id change resets
//! `DetailLoadState` to `Loading` BEFORE the new fetch is issued, so the
//! drawer can never render a previously opened profile's values under a
//! new profile's title.
//!
//! D-13: key material is write-only across the HTTP boundary. No signal in
//! this file holds a typed key value past the moment its save call
//! returns `Ok` — the masked/status display flips only after the write
//! confirms (`providers.rs`'s "no optimistic mask flip" discipline), and
//! the input is cleared in the same step.
//!
//! This file must never call Dioxus's resource-restart method — doing so
//! after a resource-driven early return breaks hook ordering for every
//! signal declared afterward (see `providers.rs`'s own module doc). It
//! must never introduce a shared-context provider scoped to this
//! component either — such a provider compiles green and panics its
//! consumers at runtime; shared state belongs only at the `HermesApp`
//! root.

use super::wizard::render_verify_doctor_block;
use crate::protocol::{
    KeyRow, KeyStatus, ProfileConfigWritePayload, ProfileDetail, ProfileGap, ProfileHealth,
    VerifyReport,
};
use crate::server::profile_api::{fetch_profile_detail, save_profile_key, update_profile_config};
use crate::server::profile_verify_api::verify_profile;
use dioxus::prelude::*;

/// Phase 47.4 Plan 08 (E5/loading backstop): explicit fetch-state model —
/// resolves the backstop by construction. `Loading` is set BEFORE every
/// new fetch is issued (including on a profile-id change), so the drawer
/// can never render stale values under a new title.
#[allow(dead_code)] // used in ProfileDetailDrawer rsx!; dead_code fires under --all-features (legacy-shell swaps the reachable root component, mirrors Plan 01's identical precedent)
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DetailLoadState {
    Loading,
    Loaded(ProfileDetail),
    Failed(String),
}

/// Collapses `KeyStatus::ManuallySet` into the same `INHERITED` label as
/// `Inherited` — the locked Copywriting Contract defines exactly two key
/// status labels (`INHERITED` / `NOT IN ROOT .ENV`); mirrors `wizard.rs`'s
/// own collapsing rule so the two surfaces never present divergent
/// vocabulary for the same underlying per-source classification.
#[allow(dead_code)] // used in ProfileDetailDrawer rsx!; dead_code fires under --all-features (legacy-shell swaps the reachable root component, mirrors Plan 01's identical precedent)
fn key_row_status_label_and_class(status: &KeyStatus) -> (&'static str, &'static str) {
    match status {
        KeyStatus::Inherited | KeyStatus::ManuallySet => ("INHERITED", "kn-key-status--inherited"),
        KeyStatus::Missing => ("NOT IN ROOT .ENV", "kn-key-status--missing"),
    }
}

/// Phase 47.4 Plan 13 (GAP-5, Task 2 edge case): pure derivation of the
/// PROVIDER select's option list. Mirrors `screens/models.rs`'s
/// `compute_model_options` prepend-when-absent rule so a stale stored
/// provider (one no longer present in the operator's configured provider
/// list) never vanishes from its own dropdown. An empty or whitespace-only
/// `assigned` prepends nothing, matching `compute_model_options`' own
/// `trim()` rule.
///
/// `cfg_attr(not(wasm), allow(dead_code))`: web-live (called from
/// `ProfileDetailDrawer`'s rsx!); native `--all-features` bin build does not
/// reach the component tree. Mirrors `compute_model_options`'s own
/// attribute.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn provider_select_options(configured: &[String], assigned: &str) -> Vec<String> {
    let mut options: Vec<String> = configured.to_vec();
    let assigned = assigned.trim();
    if !assigned.is_empty() && !options.iter().any(|o| o == assigned) {
        options.insert(0, assigned.to_string());
    }
    options
}

/// Phase 47.4 Plan 15 (GAP-9, T-47.4-15-01): client-side mirror of the
/// server's `validate_key_name` (`server/profile_api.rs:1126`) — non-empty,
/// first character ASCII uppercase, every remaining character ASCII
/// uppercase, ASCII digit, or underscore. UX-only: the server predicate is
/// the authoritative security boundary, this exists so SAVE is never
/// enabled for a name the server will reject. Pure and disk-I/O-free.
///
/// `cfg_attr(not(wasm), allow(dead_code))`: web-live (called from
/// `ProfileKeyAddForm`'s rsx!); native `--all-features` bin build does not
/// reach the component tree. Mirrors `provider_select_options`'s own
/// attribute.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn is_valid_key_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_uppercase());
    let rest_ok = name
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    first_ok && rest_ok
}

/// Phase 47.4 Plan 08: the profile detail drawer. Mounted unconditionally
/// from `kanban.rs`; renders nothing until `profile_id.read().is_some()`.
#[component]
pub fn ProfileDetailDrawer(
    profile_id: Signal<Option<String>>,
    on_close: EventHandler<()>,
    on_profile_updated: EventHandler<()>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E —
    // agents.rs UAT-2 hotfix discipline, matches TaskDrawer's own
    // early-return-after-hooks shape).

    let mut load_state: Signal<DetailLoadState> = use_signal(|| DetailLoadState::Loading);
    // Tracks which profile the CURRENT load_state belongs to, so a
    // profile-id change can be detected and the reset-before-fetch rule
    // enforced exactly once per change, not on every render.
    let mut loaded_for: Signal<Option<String>> = use_signal(|| None);

    // Provider/Model working copy (D-04), seeded from the loaded detail.
    let mut provider_wc: Signal<String> = use_signal(String::new);
    let mut model_wc: Signal<String> = use_signal(String::new);

    // Save row (Section 5) state.
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut save_hint: Signal<Option<String>> = use_signal(|| None);

    // Verify section (Section 4) state — the probe runs ONLY on the
    // explicit VERIFY click (D-11/D-09), never on open, profile change, or
    // render. `verify_started` distinguishes "never clicked" (render
    // nothing) from the shared doctor block's own Pending/`None` state.
    let mut verify_started: Signal<bool> = use_signal(|| false);
    let mut verifying: Signal<bool> = use_signal(|| false);
    let mut verify_report: Signal<Option<Result<VerifyReport, String>>> = use_signal(|| None);

    // Phase 47.4 Plan 15 (GAP-9): the name of the single key row whose
    // editor is currently revealed via an OVERRIDE/SET toggle. One at a
    // time, per D-13 — at most one typed value can exist at any moment.
    // Registered unconditionally alongside the drawer's other hooks
    // (Pattern E).
    let mut expanded_key: Signal<Option<String>> = use_signal(|| None);

    // Phase 47.4 Plan 15 (GAP-9, Task 2): whether the `+ ADD KEY` form is
    // open. Registered unconditionally alongside the drawer's other hooks.
    let mut add_form_open: Signal<bool> = use_signal(|| false);

    // Phase 47.4 Plan 13 (GAP-5): PROVIDER/MODEL dependent cascade, mirroring
    // `screens/models.rs`'s `ProviderModelCascade`. Registered unconditionally
    // alongside the drawer's other hooks (Pattern E). `provider_wc` is read
    // with CALL syntax in the model resource's SYNC prefix so a provider
    // change re-runs the model fetch — `.peek()` here would not subscribe.
    let provider_options_resource = use_resource(move || async move {
        crate::server::provider_config_api::get_provider_config().await
    });
    let model_options_resource = use_resource(move || {
        let provider = provider_wc();
        async move { crate::server::api::list_provider_models(provider).await }
    });

    // Fetch-on-change: resolves the E5/loading backstop. Resets to
    // Loading synchronously on a profile-id change, THEN issues the new
    // fetch as a plain async call.
    use_effect(move || {
        let id_opt = profile_id.read().clone();
        if id_opt != *loaded_for.read() {
            loaded_for.set(id_opt.clone());
            if let Some(id) = id_opt {
                load_state.set(DetailLoadState::Loading);
                save_hint.set(None);
                save_error.set(None);
                verify_started.set(false);
                verify_report.set(None);
                expanded_key.set(None);
                add_form_open.set(false);
                spawn(async move {
                    match fetch_profile_detail(id).await {
                        Ok(detail) => {
                            provider_wc.set(detail.provider.clone().unwrap_or_default());
                            model_wc.set(detail.model_default.clone().unwrap_or_default());
                            load_state.set(DetailLoadState::Loaded(detail));
                        }
                        Err(_e) => {
                            load_state.set(DetailLoadState::Failed(
                                "Could not read this profile. Check permissions and retry."
                                    .to_string(),
                            ));
                        }
                    }
                });
            }
        }
    });

    // Snapshot BEFORE any conditional RSX — clippy.toml signal-borrow
    // discipline (no GenerationalRef held across rsx!).
    let id_snapshot = profile_id.read().clone();
    let is_open = id_snapshot.is_some();

    if !is_open {
        // Hooks above stay registered across opens/closes; only the
        // visible subtree is skipped.
        return rsx! {};
    }
    let name_str = id_snapshot.clone().unwrap_or_default();

    let state_snapshot = load_state.read().clone();

    let aria_label = format!("Profile detail: {name_str}");

    // Handler: a key row's SAVE resolved — patch that row in place and
    // notify the parent so the board-header dropdown's health dot can
    // refresh without a page reload (Task 3).
    let on_key_saved = move |row: KeyRow| {
        if let DetailLoadState::Loaded(ref mut detail) = *load_state.write() {
            if let Some(existing) = detail.keys.iter_mut().find(|k| k.name == row.name) {
                *existing = row;
            }
        }
        on_profile_updated.call(());
    };

    // Phase 47.4 Plan 15 (GAP-9, Task 2): a new key was added via
    // `ProfileKeyAddForm`. Deliberately does NOT reuse `on_key_saved` —
    // that handler patches an existing row in place and silently drops a
    // name that is not already in the list (source fact 3). A new row can
    // only be surfaced by a full refetch, since the `extra` row set is
    // computed server-side from the profile .env
    // (`server/profile_api.rs:929-940`).
    let name_for_add = name_str.clone();
    let on_key_added = move |()| {
        add_form_open.set(false);
        on_profile_updated.call(());
        let profile_name = name_for_add.clone();
        let mut load_state_sig = load_state;
        spawn(async move {
            if let Ok(detail) = fetch_profile_detail(profile_name).await {
                load_state_sig.set(DetailLoadState::Loaded(detail));
            }
        });
    };

    // Section 5 SAVE handler.
    let name_for_save = name_str.clone();
    let on_save_click = move |_| {
        let provider_val = provider_wc.read().clone();
        let model_val = model_wc.read().clone();
        let profile_name = name_for_save.clone();
        saving.set(true);
        save_error.set(None);
        let mut saving_sig = saving;
        let mut save_error_sig = save_error;
        let mut save_hint_sig = save_hint;
        let mut load_state_sig = load_state;
        spawn(async move {
            let payload = ProfileConfigWritePayload {
                name: profile_name.clone(),
                provider: Some(provider_val),
                model_default: Some(model_val),
            };
            match update_profile_config(payload).await {
                Ok(()) => {
                    saving_sig.set(false);
                    save_hint_sig.set(Some("Saved.".to_string()));
                    on_profile_updated.call(());
                    if let Ok(detail) = fetch_profile_detail(profile_name).await {
                        load_state_sig.set(DetailLoadState::Loaded(detail));
                    }
                }
                Err(e) => {
                    saving_sig.set(false);
                    save_error_sig.set(Some(format!("{e}")));
                }
            }
        });
    };

    // Section 4 VERIFY handler — the sole trigger for the real probe.
    let name_for_verify = name_str.clone();
    let on_verify_click = move |_| {
        let profile_name = name_for_verify.clone();
        verify_started.set(true);
        verifying.set(true);
        verify_report.set(None);
        let mut verifying_sig = verifying;
        let mut verify_report_sig = verify_report;
        spawn(async move {
            let result = verify_profile(profile_name).await.map_err(|e| format!("{e}"));
            verify_report_sig.set(Some(result));
            verifying_sig.set(false);
        });
    };

    rsx! {
        aside {
            class: "kn-drawer",
            "data-open": "true",
            role: "complementary",
            "aria-label": "{aria_label}",
            "aria-modal": "false",
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    on_close.call(());
                }
            },
            div { class: "kn-drawer-header",
                div { class: "kn-drawer-header-row",
                    h2 { class: "kn-drawer-title", "{name_str}" }
                    button {
                        class: "kn-drawer-close",
                        "aria-label": "Close profile detail",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
            }
            match &state_snapshot {
                DetailLoadState::Loading => rsx! {
                    div { class: "kn-drawer-section",
                        div { class: "kn-drawer-loading", "Loading…" }
                    }
                },
                DetailLoadState::Failed(msg) => rsx! {
                    div { class: "kn-drawer-section",
                        div { class: "kn-modal-error", "{msg}" }
                    }
                },
                DetailLoadState::Loaded(detail) => {
                    // ---- Section 1: Identity ----
                    let dot_class = if detail.health == ProfileHealth::Configured {
                        "kn-health-dot kn-health-dot--ok"
                    } else {
                        "kn-health-dot kn-health-dot--gap"
                    };
                    let dir_str = detail.dir.clone();
                    let is_configured = detail.health == ProfileHealth::Configured;
                    let gaps = detail.gaps.clone();

                    // ---- Section 2: Provider/Model ----
                    // E5/error backstop: provider AND model_default both
                    // absent alongside a MissingConfigYaml gap is the
                    // signature of a config.yaml Plan 05's read degraded
                    // rather than failed on (a live, documented failure —
                    // the entire subject of Phase 48) — as distinct from a
                    // fresh profile that legitimately has none yet.
                    let config_parse_hint = detail.provider.is_none()
                        && detail.model_default.is_none()
                        && detail.gaps.contains(&ProfileGap::MissingConfigYaml);
                    let provider_val = provider_wc.read().clone();
                    let model_val = model_wc.read().clone();
                    let write_enabled = detail.web_config_write_enabled;

                    // GAP-5: provider option list (read-only, ungated — mirrors
                    // the Models page's own provider dropdown source).
                    let provider_names: Vec<String> = match provider_options_resource() {
                        Some(Ok(snap)) => snap.providers.iter().map(|p| p.name.clone()).collect(),
                        _ => Vec::new(),
                    };
                    // GAP-5: dependent model list, provider-sourced. `fell_back`
                    // means the provider has no `/models` endpoint and the full
                    // catalog is shown instead (mirrors the Models page's note).
                    let model_snapshot = match model_options_resource() {
                        Some(Ok(snap)) => Some(snap),
                        _ => None,
                    };
                    let models_loading = model_options_resource().is_none();
                    let fell_back = model_snapshot
                        .as_ref()
                        .map(|s| s.fell_back)
                        .unwrap_or(false);
                    let assigned_model_ref = if model_val.trim().is_empty() {
                        None
                    } else {
                        Some(model_val.as_str())
                    };
                    let model_options = crate::components::hermes_app::screens::models::compute_model_options(
                        model_snapshot.as_ref(),
                        assigned_model_ref,
                    );

                    // ---- Section 3: Keys ----
                    let keys = detail.keys.clone();
                    let keys_empty = keys.is_empty();

                    rsx! {
                        // Section 1 — Identity.
                        div { class: "kn-drawer-section",
                            div { class: "kn-drawer-section-label", "IDENTITY" }
                            div {
                                style: "overflow-wrap: anywhere; color: var(--fg-dim); font-size: var(--fs-13);",
                                "{dir_str}"
                            }
                            div { style: "display: flex; align-items: center; gap: var(--sp-2); padding-top: var(--sp-2);",
                                span { class: dot_class, "aria-hidden": "true" }
                                if is_configured {
                                    span { style: "font-size: var(--fs-11); color: var(--fg-dim);", "CONFIGURED" }
                                } else {
                                    div { style: "display: flex; flex-direction: column; gap: 2px;",
                                        for gap in gaps.iter() {
                                            span { style: "font-size: var(--fs-11); color: var(--warn);", "{gap.meta_label()}" }
                                        }
                                    }
                                }
                            }
                        }

                        // Section 2 — Provider / Model (editable, D-04).
                        div { class: "kn-drawer-section",
                            div { class: "kn-drawer-section-label", "PROVIDER / MODEL" }
                            if config_parse_hint {
                                div { class: "kn-modal-hint--info",
                                    "config.yaml could not be parsed — saving will rewrite it from these values."
                                }
                            }
                            label { class: "kn-modal-label", "PROVIDER" }
                            select {
                                class: "voice-settings-select",
                                disabled: !write_enabled,
                                onchange: move |evt| {
                                    provider_wc.set(evt.value());
                                    save_hint.set(None);
                                },
                                if provider_val.trim().is_empty() {
                                    option { value: "", selected: true, "— select a provider —" }
                                }
                                for name in provider_select_options(&provider_names, &provider_val).iter() {
                                    option {
                                        key: "{name}",
                                        value: "{name}",
                                        selected: name == &provider_val,
                                        "{name}"
                                    }
                                }
                            }
                            label { class: "kn-modal-label", "MODEL" }
                            select {
                                class: "voice-settings-select",
                                style: "overflow: hidden; text-overflow: ellipsis; white-space: nowrap;",
                                disabled: !write_enabled || models_loading,
                                onchange: move |evt| {
                                    model_wc.set(evt.value());
                                    save_hint.set(None);
                                },
                                if model_val.trim().is_empty() {
                                    option { value: "", selected: true, "— select a model —" }
                                }
                                for id in model_options.iter() {
                                    option {
                                        key: "{id}",
                                        value: "{id}",
                                        selected: id == &model_val,
                                        "{id}"
                                    }
                                }
                            }
                            if fell_back && !models_loading {
                                div { class: "kn-modal-hint--info",
                                    "This provider exposes no model list — showing the full catalog."
                                }
                            }
                            if !write_enabled {
                                div { class: "kn-modal-hint--info", "Config writes are disabled." }
                            }
                        }

                        // Section 3 — Keys (editable, D-04/D-07/D-13).
                        div { class: "kn-drawer-section",
                            div { class: "kn-drawer-section-label", "KEYS" }
                            div { class: "kn-key-table",
                                if keys_empty {
                                    div { class: "kn-drawer-empty",
                                        "No provider keys in this profile's .env — a kanban worker assigned here will crash at judge build."
                                    }
                                } else {
                                    for row in keys.iter().cloned() {
                                        {
                                            let (status_label, status_class) = key_row_status_label_and_class(&row.status);
                                            let is_missing = row.status == KeyStatus::Missing;
                                            // GAP-9: read with `.read()` (not `.peek()`) in the
                                            // render body so a toggle click re-renders this row.
                                            let is_expanded = expanded_key.read().as_deref() == Some(row.name.as_str());
                                            let show_editor = is_missing || is_expanded;
                                            let toggle_label = if is_missing { "SET" } else { "OVERRIDE" };
                                            let key_name_for_editor = row.name.clone();
                                            let profile_name_for_editor = name_str.clone();
                                            let key_name_for_toggle = row.name.clone();
                                            rsx! {
                                                div { key: "{row.name}",
                                                    div { class: "kn-key-row",
                                                        span {
                                                            style: "font-size: var(--fs-13); color: var(--accent-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1 1 auto;",
                                                            "{row.name}"
                                                        }
                                                        span { style: "font-size: var(--fs-13); color: var(--fg-faint);", "{row.masked}" }
                                                        span { class: status_class, "{status_label}" }
                                                        if !is_missing {
                                                            button {
                                                                class: "kn-key-edit-toggle",
                                                                disabled: !write_enabled,
                                                                onclick: move |_| {
                                                                    if is_expanded {
                                                                        expanded_key.set(None);
                                                                    } else {
                                                                        expanded_key.set(Some(key_name_for_toggle.clone()));
                                                                    }
                                                                },
                                                                "{toggle_label}"
                                                            }
                                                        }
                                                    }
                                                    if show_editor {
                                                        ProfileKeyRowEditor {
                                                            profile_name: profile_name_for_editor,
                                                            key_name: key_name_for_editor,
                                                            write_enabled: write_enabled,
                                                            on_saved: on_key_saved,
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // GAP-9 Task 2: outside `.kn-key-table` — that
                            // element is a 280px inner scroll region
                            // (source fact 9) and a control placed inside
                            // it scrolls out of reach.
                            if *add_form_open.read() {
                                ProfileKeyAddForm {
                                    profile_name: name_str.clone(),
                                    write_enabled: write_enabled,
                                    existing_key_names: keys.iter().map(|k| k.name.clone()).collect::<Vec<String>>(),
                                    on_added: on_key_added,
                                    on_cancel: move |()| add_form_open.set(false),
                                }
                            } else {
                                button {
                                    class: "kn-action-btn",
                                    disabled: !write_enabled,
                                    onclick: move |_| add_form_open.set(true),
                                    "+ ADD KEY"
                                }
                            }
                        }

                        // Section 4 — Verify (on-demand D-09 probe).
                        div { class: "kn-drawer-section",
                            div { class: "kn-drawer-section-label", "VERIFY" }
                            button {
                                class: "kn-action-btn",
                                disabled: *verifying.read(),
                                onclick: on_verify_click,
                                if *verifying.read() { "VERIFYING…" } else { "VERIFY" }
                            }
                            if *verify_started.read() {
                                {render_verify_doctor_block(&name_str, &verify_report.read().clone())}
                            }
                        }

                        // Section 5 — Save row.
                        div { class: "kn-drawer-section",
                            button {
                                class: "kn-modal-btn kn-modal-btn--submit",
                                disabled: !write_enabled || *saving.read(),
                                onclick: on_save_click,
                                if *saving.read() { "SAVING…" } else { "SAVE" }
                            }
                            if let Some(hint) = save_hint.read().clone() {
                                div { class: "kn-modal-hint--info", "{hint}" }
                            }
                            if let Some(err) = save_error.read().clone() {
                                div { class: "kn-modal-error", "{err}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Phase 47.4 Plan 08 (D-04/D-07/D-13): a single `NOT IN ROOT .ENV` row's
/// manual-entry affordance. Copies `providers.rs`'s masked-key state
/// machine discipline: no optimistic mask flip (the parent only patches
/// the row after `Ok`), and the typed value is cleared in the same step
/// as the successful write — never redisplayed, not even immediately
/// after.
#[component]
fn ProfileKeyRowEditor(
    profile_name: String,
    key_name: String,
    write_enabled: bool,
    on_saved: EventHandler<KeyRow>,
) -> Element {
    let mut input_value: Signal<String> = use_signal(String::new);
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let saving_val = *saving.read();
    let input_val = input_value.read().clone();
    let can_save = write_enabled && !saving_val && !input_val.trim().is_empty();

    let on_save = move |_| {
        // D-13/clippy.toml: read the signal value BEFORE spawn — a signal
        // borrow held across `.await` panics at runtime.
        let value_to_save = input_value.read().clone();
        let name_owned = profile_name.clone();
        let key_owned = key_name.clone();
        saving.set(true);
        error.set(None);
        let mut saving_sig = saving;
        let mut error_sig = error;
        let mut input_sig = input_value;
        spawn(async move {
            match save_profile_key(name_owned, key_owned, value_to_save).await {
                Ok(row) => {
                    saving_sig.set(false);
                    // D-13: clear in the same step as the confirmed
                    // write — never redisplayed.
                    input_sig.set(String::new());
                    on_saved.call(row);
                }
                Err(e) => {
                    saving_sig.set(false);
                    error_sig.set(Some(format!("{e}")));
                }
            }
        });
    };

    rsx! {
        div {
            div { class: "kn-key-row",
                input {
                    class: "kn-key-input",
                    r#type: "password",
                    disabled: !write_enabled || saving_val,
                    value: "{input_val}",
                    oninput: move |evt| input_value.set(evt.value()),
                }
                button {
                    class: "kn-action-btn",
                    disabled: !can_save,
                    onclick: on_save,
                    if saving_val { "SAVING…" } else { "SAVE" }
                }
            }
            if let Some(err) = error.read().clone() {
                div { class: "kn-modal-error", "{err}" }
            }
        }
    }
}

/// Phase 47.4 Plan 15 (GAP-9, Task 2): the `+ ADD KEY` form — an
/// operator-named key that is NOT already in the row set. Modelled
/// directly on `ProfileKeyRowEditor`'s state machine (D-13: typed value
/// lives only in a component-local signal, cleared in the same step as the
/// confirmed write, never redisplayed). SAVE calls the same
/// `save_profile_key` server fn; the parent's `on_added` handler is what
/// differs — it triggers a full `fetch_profile_detail` refetch rather than
/// an in-place patch, because the row this form creates is not yet in the
/// loaded detail (source fact 3/6).
#[component]
fn ProfileKeyAddForm(
    profile_name: String,
    write_enabled: bool,
    existing_key_names: Vec<String>,
    on_added: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut name_value: Signal<String> = use_signal(String::new);
    let mut value_value: Signal<String> = use_signal(String::new);
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let saving_val = *saving.read();
    let name_val = name_value.read().clone();
    let value_val = value_value.read().clone();
    let name_trimmed = name_val.trim();
    let name_is_valid = is_valid_key_name(name_trimmed);
    let name_has_content = !name_trimmed.is_empty();
    let is_duplicate = existing_key_names.iter().any(|n| n.as_str() == name_trimmed);
    let can_save =
        write_enabled && !saving_val && name_is_valid && !value_val.trim().is_empty();

    let on_save = move |_| {
        // D-13/clippy.toml: read signal values BEFORE spawn — a signal
        // borrow held across `.await` panics at runtime.
        let name_to_save = name_value.read().trim().to_string();
        let value_to_save = value_value.read().clone();
        let profile_name_owned = profile_name.clone();
        saving.set(true);
        error.set(None);
        let mut saving_sig = saving;
        let mut error_sig = error;
        let mut name_sig = name_value;
        let mut value_sig = value_value;
        spawn(async move {
            match save_profile_key(profile_name_owned, name_to_save, value_to_save).await {
                Ok(_row) => {
                    saving_sig.set(false);
                    // D-13: clear BOTH inputs in the same step as the
                    // confirmed write — never redisplayed.
                    name_sig.set(String::new());
                    value_sig.set(String::new());
                    error_sig.set(None);
                    on_added.call(());
                }
                Err(e) => {
                    saving_sig.set(false);
                    error_sig.set(Some(format!("{e}")));
                }
            }
        });
    };

    let on_cancel_click = move |_| {
        name_value.set(String::new());
        value_value.set(String::new());
        on_cancel.call(());
    };

    rsx! {
        div { class: "kn-key-add-row",
            label { class: "kn-modal-label", "KEY NAME" }
            input {
                class: "kn-key-input",
                r#type: "text",
                disabled: !write_enabled || saving_val,
                value: "{name_val}",
                oninput: move |evt| name_value.set(evt.value()),
            }
            if name_has_content && !name_is_valid {
                div { class: "kn-modal-error",
                    "Uppercase letters, digits and underscores only; must start with a letter."
                }
            }
            if name_is_valid && is_duplicate {
                div { class: "kn-modal-hint--info",
                    "This key already exists — saving replaces its value."
                }
            }
            label { class: "kn-modal-label", "VALUE" }
            input {
                class: "kn-key-input",
                r#type: "password",
                disabled: !write_enabled || saving_val,
                value: "{value_val}",
                oninput: move |evt| value_value.set(evt.value()),
            }
            div { class: "kn-key-add-actions",
                button {
                    class: "kn-action-btn",
                    disabled: !can_save,
                    onclick: on_save,
                    if saving_val { "SAVING…" } else { "SAVE" }
                }
                button {
                    class: "kn-action-btn",
                    disabled: saving_val,
                    onclick: on_cancel_click,
                    "CANCEL"
                }
            }
            if let Some(err) = error.read().clone() {
                div { class: "kn-modal-error", "{err}" }
            }
        }
    }
}

#[cfg(test)]
mod provider_select_options_tests {
    use super::provider_select_options;

    /// `<behavior>` bullet 1: an assigned provider already present in the
    /// configured list is not duplicated.
    #[test]
    fn assigned_provider_already_configured_is_not_duplicated() {
        let configured = vec!["openrouter".to_string(), "moonshot".to_string()];
        let result = provider_select_options(&configured, "moonshot");
        assert_eq!(
            result,
            vec!["openrouter".to_string(), "moonshot".to_string()]
        );
    }

    /// `<behavior>` bullet 2: a stored provider absent from the configured
    /// list is prepended so it stays selectable.
    #[test]
    fn assigned_provider_absent_from_config_is_prepended() {
        let configured = vec!["openrouter".to_string()];
        let result = provider_select_options(&configured, "moonshot");
        assert_eq!(result, vec!["moonshot".to_string(), "openrouter".to_string()]);
    }

    /// `<behavior>` bullet 3: an empty assignment prepends nothing.
    #[test]
    fn empty_assignment_prepends_nothing() {
        let configured = vec!["openrouter".to_string()];
        let result = provider_select_options(&configured, "");
        assert_eq!(result, vec!["openrouter".to_string()]);
    }

    /// `<behavior>` bullet 5: whitespace-only is treated as empty, matching
    /// `compute_model_options`' own `trim()` rule.
    #[test]
    fn whitespace_only_assignment_prepends_nothing() {
        let configured = vec!["openrouter".to_string()];
        let result = provider_select_options(&configured, "  ");
        assert_eq!(result, vec!["openrouter".to_string()]);
    }

    /// `<behavior>` bullet 4: an empty configured list yields just the
    /// assignment.
    #[test]
    fn empty_configured_list_yields_just_the_assignment() {
        let configured: Vec<String> = Vec::new();
        let result = provider_select_options(&configured, "moonshot");
        assert_eq!(result, vec!["moonshot".to_string()]);
    }
}

#[cfg(test)]
mod is_valid_key_name_tests {
    use super::is_valid_key_name;

    /// Test 1: a well-formed name returns true.
    #[test]
    fn well_formed_name_is_valid() {
        assert!(is_valid_key_name("MOONSHOT_API_KEY"));
    }

    /// Test 2: an empty string returns false.
    #[test]
    fn empty_string_is_invalid() {
        assert!(!is_valid_key_name(""));
    }

    /// Test 3: a lowercase first character returns false.
    #[test]
    fn lowercase_first_char_is_invalid() {
        assert!(!is_valid_key_name("moonshot_API_KEY"));
    }

    /// Test 4: a leading digit and a leading underscore both return false.
    #[test]
    fn leading_digit_or_underscore_is_invalid() {
        assert!(!is_valid_key_name("1KEY"));
        assert!(!is_valid_key_name("_KEY"));
    }

    /// Test 5: a hyphen or a space anywhere in the name returns false.
    #[test]
    fn hyphen_or_space_is_invalid() {
        assert!(!is_valid_key_name("MOONSHOT-KEY"));
        assert!(!is_valid_key_name("MOONSHOT KEY"));
    }

    /// Test 6: an embedded newline returns false — this is the injection
    /// case `validate_key_name`'s own doc comment names (a forged second
    /// `VAR=value` line).
    #[test]
    fn embedded_newline_is_invalid() {
        assert!(!is_valid_key_name("KEY\nFORGED=value"));
    }

    /// Test 7: a single uppercase letter is the shortest legal name.
    #[test]
    fn single_uppercase_letter_is_valid() {
        assert!(is_valid_key_name("K"));
    }
}
