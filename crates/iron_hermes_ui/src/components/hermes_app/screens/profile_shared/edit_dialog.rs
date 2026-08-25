//! Phase 47.4 Plan 08 (D-02/D-04/D-06/D-07/D-09/D-10/D-11/D-13): the
//! profile detail drawer — the client half of the editing surface Plan 05
//! built server-side. Mounted the way `TaskDrawer` is today (unconditional
//! mount, renders nothing until `profile_id.read().is_some()`); opened
//! from both D-02 entry points (the dropdown's per-row EDIT chip and the
//! MANAGE ALL PROFILES footer link) — see `kanban.rs`.
//!
//! Phase 50.1 Plan 02 (D-10): relocated from `screens/kanban/profile_drawer.rs`
//! into this shared module so both `screens/kanban.rs` and the Agents
//! screen's bot roster consume ONE implementation. A
//! `context: ProfileDialogContext` prop (added by this plan's Task 2)
//! selects bot-flavored vs kanban-flavored copy on the same component; Task
//! 3 mounts this drawer on the roster. `screens/kanban/profile_drawer.rs`
//! is now a thin re-export shim at the old path.
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

use super::advanced::AdvancedProfilePane;
use super::create_dialog::render_verify_doctor_block;
use super::ProfileDialogContext;
use crate::components::hermes_app::screens::bot_roster::delete_confirm::DeleteBotConfirm;
use crate::components::hermes_app::screens::bot_roster::npub_row::BotNpubRow;
use crate::components::hermes_app::screens::bot_roster::routines::BotRoutinesSection;
use crate::components::hermes_app::widgets::avatar_picker::AvatarPicker;
use crate::protocol::{
    BotAvatarDescriptor, DuplicateProfileRequest, KeyRow, KeyStatus, ProfileConfigWritePayload,
    ProfileDetail, ProfileGap, ProfileHealth, VerifyReport,
};
use crate::server::profile_api::{
    fetch_profile_detail, list_profiles, save_profile_key, update_profile_config,
};
use crate::server::profile_verify_api::verify_profile;
use dioxus::prelude::*;

/// Phase 50.1 Plan 06 (D-17): the default profile — the one name
/// `is_deletion_protected` refuses server-side, mirrored client-side so
/// the `DELETE BOT` action renders disabled with its inline note and the
/// confirmation modal is never reachable for it. Deliberately a plain
/// string literal, not a call into the native-only `is_deletion_protected`
/// (server-feature-gated, unreachable from this wasm-compiled file) —
/// mirrors this file's own `is_valid_key_name` client-mirror precedent.
#[allow(dead_code)] // used in ProfileDetailDrawer rsx!; dead_code fires under --all-features (legacy-shell)
const DELETION_PROTECTED_PROFILE_NAME: &str = "default";

/// Phase 50.1 Plan 06 (D-17): the next available `{source}-copy[-N]` name,
/// scanning `existing_names` for a collision and incrementing until one is
/// free. Pure and I/O-free — the caller resolves `existing_names` from a
/// fresh `list_profiles()` read immediately before calling this, so a
/// stale list cannot pick an already-taken name under normal single-
/// operator use; a genuine race with another create/duplicate landing in
/// the same instant still surfaces as `duplicate_profile`'s own
/// "already exists" rejection, which the caller already handles as an
/// error.
#[allow(dead_code)] // used in ProfileDetailDrawer rsx!; dead_code fires under --all-features (legacy-shell)
pub(crate) fn next_duplicate_name(source: &str, existing_names: &[String]) -> String {
    let base = format!("{source}-copy");
    if !existing_names.iter().any(|n| n == &base) {
        return base;
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{source}-copy-{n}");
        if !existing_names.iter().any(|name| name == &candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Phase 50.2 Plan 03 (D-11/D-21, T-50.2-03-01): client-side mirror of
/// `group_chat_store::validate_group_room_name`'s character rules — a group
/// label and a room name must not disagree about what is acceptable.
/// Deliberately duplicated rather than reused: that fn lives behind
/// `#[cfg(feature = "server")]` inside `group_chat_store.rs`, and this file
/// is compiled for wasm too. Rejects empty/whitespace-only, over 64 chars,
/// any character outside `[A-Za-z0-9 ._-]`, and any occurrence of `..`,
/// `/`, `\`, or `$`. Returns the trimmed label.
#[allow(dead_code)] // used in ProfileDetailDrawer rsx!; dead_code fires under --all-features (legacy-shell)
pub(crate) fn validate_group_label(label: &str) -> Result<String, String> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Err("Group label is empty or whitespace-only.".to_string());
    }
    if trimmed.chars().count() > 64 {
        return Err("Group label exceeds 64 characters.".to_string());
    }
    let has_invalid_char = trimmed
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-')));
    if has_invalid_char {
        return Err("Group label contains a character outside [A-Za-z0-9 ._-].".to_string());
    }
    if trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('$')
    {
        return Err("Group label contains a path-traversal-shaped sequence.".to_string());
    }
    Ok(trimmed.to_string())
}

/// Phase 50.2 Plan 03 (D-11): builds the `BotMetaPatch` the GROUP field's
/// commit dispatches — every sibling field `None`, the same field-by-field
/// shape the AVATAR section's `on_save` closure already uses so the merge
/// semantics (`apply_bot_meta_patch`) stay unchanged. Pure and testable
/// without a renderer; `group_label` must already be trimmed/validated by
/// the caller.
#[allow(dead_code)] // used in ProfileDetailDrawer rsx!; dead_code fires under --all-features (legacy-shell)
pub(crate) fn build_group_meta_patch(
    profile_name: &str,
    group_label: &str,
) -> crate::protocol::BotMetaPatch {
    crate::protocol::BotMetaPatch {
        name: profile_name.to_string(),
        title: None,
        description: None,
        avatar: None,
        group: Some(group_label.to_string()),
        preview: None,
        preview_at_ms: None,
    }
}

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
/// Phase 50.1 Plan 02 (D-10): `context` selects bot-flavored vs
/// kanban-flavored presentation on this one shared component — defaults to
/// `Kanban` so an omitted prop preserves the pre-lift behavior exactly.
/// Plan 02 Task 3 appends four bot-context-only sections when `context` is
/// the bot variant; Kanban's section set is unchanged.
#[component]
pub fn ProfileDetailDrawer(
    profile_id: Signal<Option<String>>,
    on_close: EventHandler<()>,
    on_profile_updated: EventHandler<()>,
    #[props(default)] context: ProfileDialogContext,
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

    // Phase 50.1 Plan 04 (D-12): avatar working copy, seeded from the
    // bot-meta store on profile-id change (fetch-on-change effect below).
    // The bot already exists here (unlike the create wizard), so
    // AvatarPicker's on_save fires a real save_bot_meta write on every
    // change rather than deferring to a later submit step.
    let mut avatar_descriptor: Signal<Option<BotAvatarDescriptor>> = use_signal(|| None);

    // Phase 50.2 Plan 03 (D-11/D-21): GROUP working copy — the roster's
    // assignment affordance. Seeded from the bot-meta store on
    // profile-id change (same fetch-on-change effect as
    // `avatar_descriptor`), saved on blur so leaving the field commits
    // the label without a separate submit step. `group_error` renders the
    // client-side `validate_group_label` rejection inline; a blank field
    // is never an error (it means "leave unchanged" per `BotMetaPatch`'s
    // own doc comment).
    let mut group_draft: Signal<String> = use_signal(String::new);
    let mut group_error: Signal<Option<String>> = use_signal(|| None);

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

    // Phase 50.1 Plan 06 (D-17/D-18): Danger zone state — registered
    // unconditionally alongside the drawer's other hooks (Pattern E) even
    // though the section only renders in bot context.
    let mut duplicating: Signal<bool> = use_signal(|| false);
    let mut duplicate_error: Signal<Option<String>> = use_signal(|| None);
    let mut delete_confirm_open: Signal<bool> = use_signal(|| false);

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
                duplicate_error.set(None);
                delete_confirm_open.set(false);
                group_error.set(None);
                spawn(async move {
                    let id_for_meta = id.clone();
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
                    // Phase 50.1 Plan 04 (D-12): seed the avatar working
                    // copy from the bot-meta store — list_bot_meta() is the
                    // roster's own single whole-map read (D-13); reused
                    // here rather than a new per-name getter server fn. A
                    // fetch failure or an absent entry both leave
                    // avatar_descriptor at None — AvatarPicker's own
                    // seeded-default resolution covers that gap in the
                    // live preview.
                    //
                    // Phase 50.2 Plan 03 (D-11): the same `list_bot_meta()`
                    // read also seeds `group_draft` — a fetch failure or an
                    // absent entry both leave it empty, matching the
                    // drawer's own "empty field, no assignment yet" state.
                    if let Ok(response) = crate::server::bot_meta_api::list_bot_meta().await {
                        let meta = response.meta.get(&id_for_meta);
                        avatar_descriptor.set(meta.and_then(|m| m.avatar.clone()));
                        group_draft.set(
                            meta.and_then(|m| m.group.clone()).unwrap_or_default(),
                        );
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
    // D-18: the default profile's DELETE BOT renders disabled regardless
    // of load state — this must not wait on `fetch_profile_detail` to
    // resolve.
    let is_default_profile = name_str == DELETION_PROTECTED_PROFILE_NAME;

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
                skills_disabled: None,
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

    // Phase 50.2 Plan 03 (D-11/D-21, T-50.2-03-01): GROUP field's onblur
    // commit handler. A blank field leaves the label unchanged (no patch
    // dispatched at all — `BotMetaPatch`'s own doc comment: `None` means
    // "leave unchanged" and there is deliberately no clear verb). An
    // invalid label renders inline via `group_error` and is never
    // dispatched. Signals through `on_profile_updated`, the same
    // caller-owned refresh idiom the AVATAR section already uses — this is
    // a lifted shared component (D-10), so it never calls a resource's
    // restart method directly.
    let name_for_group_save = name_str.clone();
    let on_group_blur = move |_| {
        let raw = group_draft.read().clone();
        if raw.trim().is_empty() {
            group_error.set(None);
            return;
        }
        match validate_group_label(&raw) {
            Ok(trimmed) => {
                group_error.set(None);
                let profile_name = name_for_group_save.clone();
                spawn(async move {
                    let patch = build_group_meta_patch(&profile_name, &trimmed);
                    let _ = crate::server::bot_meta_api::save_bot_meta(patch).await;
                });
                on_profile_updated.call(());
            }
            Err(msg) => {
                group_error.set(Some(msg));
            }
        }
    };

    // Phase 50.1 Plan 06 (D-17): DUPLICATE handler — resolves a free
    // `{name}-copy[-N]` target from a fresh `list_profiles()` read, then
    // calls `duplicate_profile`. On success, retargets THIS drawer's own
    // `profile_id` signal at the new bot — the same signal `bot_roster.rs`
    // threads down as `drawer_target`, so this is the drawer opening
    // itself onto the clone rather than a second navigation mechanism.
    let name_for_duplicate = name_str.clone();
    let on_duplicate_click = move |_| {
        let source_name = name_for_duplicate.clone();
        duplicating.set(true);
        duplicate_error.set(None);
        let mut duplicating_sig = duplicating;
        let mut duplicate_error_sig = duplicate_error;
        let mut profile_id_sig = profile_id;
        spawn(async move {
            let existing_names: Vec<String> = list_profiles()
                .await
                .map(|rows| rows.into_iter().map(|r| r.name).collect())
                .unwrap_or_default();
            let target_name = next_duplicate_name(&source_name, &existing_names);
            let req = DuplicateProfileRequest {
                source: source_name,
                target: target_name,
            };
            match crate::server::profile_api::duplicate_profile(req).await {
                Ok(created_name) => {
                    duplicating_sig.set(false);
                    on_profile_updated.call(());
                    profile_id_sig.set(Some(created_name));
                }
                Err(e) => {
                    duplicating_sig.set(false);
                    duplicate_error_sig.set(Some(format!("{e}")));
                }
            }
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
                            // Phase 50.1 Plan 07 (D-14/D-02 scope fence):
                            // the npub row — read-only, inside the existing
                            // Identity section, bot context only. Keyed by
                            // name (npub_row.rs module doc) so switching
                            // which bot the drawer shows re-issues the
                            // fetch against the new profile.
                            if matches!(context, ProfileDialogContext::Bot) {
                                BotNpubRow { key: "{name_str}", bot_name: name_str.clone() }
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
                                {render_verify_doctor_block(&name_str, &verify_report.read().clone(), context)}
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

                        // Phase 50.1 Plan 02 Task 3 (D-10 extension, UI-SPEC
                        // Component Inventory §4): four bot-context-only
                        // sections appended after the pre-lift section set.
                        // Headers only — no controls — plans 50.1-04
                        // (Avatar), 50.1-05 (Advanced) and 50.1-08
                        // (Routines/Danger zone) supply their bodies. Never
                        // rendered for Kanban: opening the Kanban board's
                        // profile drawer shows its pre-lift section set with
                        // none of these four.
                        if matches!(context, ProfileDialogContext::Bot) {
                            // Section 5a — Group (D-11/D-21, this plan): the
                            // roster's assignment affordance — closes the
                            // reachability gap where every UI write site
                            // hard-coded `group: None`. Saves on blur, same
                            // always-live-save discipline as AVATAR below
                            // (the bot already exists, unlike the create
                            // wizard).
                            div { class: "kn-drawer-section",
                                div { class: "kn-drawer-section-label", "GROUP" }
                                input {
                                    class: "kn-modal-input",
                                    value: "{group_draft}",
                                    oninput: move |evt| {
                                        group_draft.set(evt.value());
                                        group_error.set(None);
                                    },
                                    onblur: on_group_blur,
                                }
                                div { class: "kn-modal-hint--info",
                                    "Groups this bot's card under a labeled roster section."
                                }
                                if let Some(err) = group_error.read().clone() {
                                    div { class: "kn-modal-error", "{err}" }
                                }
                            }
                            // Section 6 — Avatar (D-12). Editable any time,
                            // not only at creation — every change here
                            // saves through save_bot_meta immediately,
                            // since (unlike the create wizard) the bot
                            // already exists.
                            div { class: "kn-drawer-section",
                                div { class: "kn-drawer-section-label", "AVATAR" }
                                AvatarPicker {
                                    bot_name: name_str.clone(),
                                    descriptor: avatar_descriptor,
                                    on_save: {
                                        let name_for_avatar_save = name_str.clone();
                                        move |new_descriptor: BotAvatarDescriptor| {
                                            let profile_name = name_for_avatar_save.clone();
                                            spawn(async move {
                                                let patch = crate::protocol::BotMetaPatch {
                                                    name: profile_name,
                                                    title: None,
                                                    description: None,
                                                    avatar: Some(new_descriptor),
                                                    group: None,
                                                    preview: None,
                                                    preview_at_ms: None,
                                                };
                                                let _ = crate::server::bot_meta_api::save_bot_meta(patch).await;
                                            });
                                            on_profile_updated.call(());
                                        }
                                    },
                                }
                            }
                            // Section 7 — Advanced (D-15/D-16). Clone-from is
                            // deliberately absent here — it is a one-time
                            // creation operation, not a live field
                            // (UI-SPEC). Model pin, SOUL.md and Skills only,
                            // the same shared `AdvancedProfilePane` the
                            // create wizard's Verify step mounts.
                            div { class: "kn-drawer-section",
                                div { class: "kn-drawer-section-label", "ADVANCED" }
                                AdvancedProfilePane {
                                    key: "{name_str}",
                                    bot_name: name_str.clone(),
                                    on_saved: move |_| on_profile_updated.call(()),
                                }
                            }
                            // Section 8 — Routines (D-22). Fills the
                            // header-only stub plan 50.1-02 left.
                            div { class: "kn-drawer-section",
                                div { class: "kn-drawer-section-label", "ROUTINES" }
                                BotRoutinesSection { key: "{name_str}", bot_name: name_str.clone() }
                            }
                            // Section 9 — Danger zone (D-17/D-18):
                            // DUPLICATE (warn-tinted) and DELETE BOT
                            // (danger-tinted) side by side — this is the
                            // "real" destination the card's overflow menu
                            // deep-links to, and a drawer opened directly
                            // still needs both reachable.
                            div { class: "kn-drawer-section",
                                div { class: "kn-drawer-section-label", "DANGER ZONE" }
                                div { class: "kn-bot-danger-zone",
                                    button {
                                        class: "kn-modal-btn kn-modal-btn--warn",
                                        disabled: *duplicating.read(),
                                        onclick: on_duplicate_click,
                                        if *duplicating.read() { "DUPLICATING…" } else { "DUPLICATE" }
                                    }
                                    button {
                                        class: "kn-modal-btn kn-modal-btn--danger",
                                        disabled: is_default_profile,
                                        onclick: move |_| delete_confirm_open.set(true),
                                        "DELETE BOT"
                                    }
                                }
                                // D-18: the default profile's DELETE BOT
                                // renders disabled with this inline note —
                                // the confirmation modal is never reachable
                                // for it, so there is no dead-end "delete
                                // blocked" state inside the confirm flow.
                                if is_default_profile {
                                    div { class: "kn-modal-hint--info", "The default profile can't be deleted." }
                                }
                                if let Some(err) = duplicate_error.read().clone() {
                                    div {
                                        class: "kn-modal-error",
                                        "Duplicate failed. Check the source bot's config and try again."
                                        div { style: "font-size: var(--fs-11); margin-top: var(--sp-1);", "{err}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // D-18: mounted outside the `aside` shell — a top-level modal
        // overlay, matching `CreateProfileWizard`'s own `.kn-modal-overlay`
        // mount, not nested inside the drawer's own scroll region. Only
        // ever mounted for a non-default-profile bot (`is_default_profile`
        // already disables the button that would open it, but this is the
        // structural guarantee: the component itself is never instantiated
        // for the default profile, so there is no code path that could
        // open it regardless of the button's disabled state).
        if *delete_confirm_open.read() && !is_default_profile {
            DeleteBotConfirm {
                bot_name: name_str.clone(),
                on_dismiss: move |_| delete_confirm_open.set(false),
                on_deleted: move |_| {
                    delete_confirm_open.set(false);
                    on_profile_updated.call(());
                    on_close.call(());
                },
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

#[cfg(test)]
mod group_label_tests {
    use super::{build_group_meta_patch, validate_group_label};

    /// `<behavior>` bullet 3 / acceptance criteria: a plain label is
    /// accepted and returned trimmed.
    #[test]
    fn accepts_a_plain_label() {
        assert_eq!(validate_group_label("standup"), Ok("standup".to_string()));
        assert_eq!(
            validate_group_label("  standup  "),
            Ok("standup".to_string())
        );
    }

    /// Acceptance criteria: empty is rejected.
    #[test]
    fn rejects_empty() {
        assert!(validate_group_label("").is_err());
    }

    /// Acceptance criteria: whitespace-only is rejected.
    #[test]
    fn rejects_whitespace_only() {
        assert!(validate_group_label("   ").is_err());
    }

    /// Acceptance criteria: 65 characters (one over the 64-char cap) is
    /// rejected.
    #[test]
    fn rejects_over_length() {
        let too_long = "a".repeat(65);
        assert!(validate_group_label(&too_long).is_err());
        // 64 exactly stays valid — the boundary is inclusive.
        let exactly_max = "a".repeat(64);
        assert!(validate_group_label(&exactly_max).is_ok());
    }

    /// Acceptance criteria: a traversal-shaped value is rejected.
    #[test]
    fn rejects_traversal_shaped_value() {
        assert!(validate_group_label("../etc").is_err());
        assert!(validate_group_label("a/b").is_err());
        assert!(validate_group_label("a\\b").is_err());
    }

    /// Acceptance criteria: a value containing a dollar sign is rejected.
    #[test]
    fn rejects_dollar_sign() {
        assert!(validate_group_label("$HOME").is_err());
    }

    /// Acceptance criteria: the constructed patch has `group: Some(...)`
    /// and every sibling field `None`, mirroring the AVATAR section's own
    /// patch shape.
    #[test]
    fn build_group_meta_patch_sets_group_and_leaves_siblings_none() {
        let patch = build_group_meta_patch("scout", "standup");
        assert_eq!(patch.name, "scout");
        assert_eq!(patch.group, Some("standup".to_string()));
        assert_eq!(patch.title, None);
        assert_eq!(patch.description, None);
        assert_eq!(patch.avatar, None);
        assert_eq!(patch.preview, None);
        assert_eq!(patch.preview_at_ms, None);
    }
}
