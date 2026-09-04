//! Soul screen — ported from `app.html` `<section id="screen-soul">`
//! (lines 885-957).
//!
//! Phase 49.4 Plan 01 (D-17) tracer: replaces the prior mock-persona visual
//! stub with a real screen wired to `crate::server::profile_api`:
//! `list_profiles` drives the persona tabs, `fetch_profile_persona` /
//! `save_profile_persona` drive the editor. Follows the
//! `profile_shared::switcher` idiom — a `ReadSignal<u32>`-shaped
//! `refresh_tick` read in the SYNC prefix of each `use_resource` closure,
//! bumped after a successful SAVE to re-fetch both the profile list and the
//! persona. Never restarts a `use_server_future` mid-hook-tree — the
//! documented hook-order crash trap in this crate (`PROJECT.md`
//! Constraints).
//!
//! The editor buffer is an owned `Signal<String>` seeded once per
//! `(profile, refresh_tick)` pair — the same seed-once + `use_effect` guard
//! shape `schedules.rs` uses for `schedule_list_sig`/`seeded`, extended to a
//! keyed variant because the buffer must reseed on every tab switch and on
//! REVERT, not just once ever.
//!
//! Phase 49.4 Plan 10 (D-14/D-15/D-18/D-19): turns this screen into the full
//! profile-management surface. `ProfileActivatePrompt` drives
//! `profile_activation_api::activate_profile` with an explicit scope choice
//! — never a bare click-to-activate. `ArchiveProfileConfirm` drives
//! `profile_api::archive_profile` (never the old hard-removal fn) behind a
//! typed-name gate. The add-profile control mounts the shared
//! `CreateProfileWizard` with its new `show_entry_step` opt-in (template vs
//! clone). The persona-card grid's "ACTIVE" pill/accent now reflects the
//! REAL persisted activation record (`active_resource`, falling back to the
//! environment-derived `live_profile_name` when no record has ever been
//! written) rather than merely "the tab currently open for editing" — those
//! are two different questions this plan deliberately stops conflating.

use dioxus::prelude::*;

use crate::components::hermes_app::screens::profile_shared::ProfileDialogContext;
use crate::components::hermes_app::screens::profile_shared::create_dialog::CreateProfileWizard;
use crate::protocol::{
    ActivationScope, ActiveProfileRecord, BotBinding, ProfileHealth, ProfilePersona, ProfileRow,
};
use crate::server::bot_binding_api::{list_bot_bindings, set_bot_binding};
use crate::server::bot_meta_api::live_profile_name;
use crate::server::profile_activation_api::{activate_profile, get_active_profile};
use crate::server::profile_api::{
    archive_profile, fetch_profile_persona, fetch_root_persona, list_profiles, save_profile_persona,
    save_root_persona, ROOT_PERSONA_NAME,
};

/// Phase 49.4 Plan 12 (D-16): the fixed, closed platform-adapter key set —
/// duplicated from `bot_binding_api`'s server-side module doc per this
/// crate's own "each module owns its tiny constants" precedent (see
/// `bot_roster.rs`'s identical copy for the same wasm-client-reachability
/// reason).
const PLATFORM_KEYS: [&str; 6] = ["telegram", "discord", "slack", "buzz", "webhook", "api_server"];

/// Phase 49.4 Plan 12 (UI-SPEC E14 partial): the literal profile name an
/// unbound bot is considered NOT assigned to — duplicated from
/// `bot_binding_api::DEFAULT_BOUND_PROFILE` for the same reason.
const DEFAULT_PROFILE_LABEL: &str = "default";

/// Phase 49.4 Plan 01 (D-21): the Soul header's live inline status string —
/// `"{N} profiles · {active} active"` for N != 1, `"1 profile · {active}
/// active"` for exactly one. Pure and unit-tested; the long-name case is
/// intentionally NOT truncated here — the CSS ellipsis on `.screen-status`
/// (screens.css) handles overflow, not Rust string manipulation.
pub(crate) fn soul_status_line(profile_count: usize, active_name: &str) -> String {
    if profile_count == 1 {
        format!("1 profile · {active_name} active")
    } else {
        format!("{profile_count} profiles · {active_name} active")
    }
}

/// Phase 49.4 Plan 10 (D-14): maps the activate prompt's two option ids to
/// the wire `ActivationScope` — pure and unit-tested (Task 1's own
/// instruction) so the two buttons share one mapping rather than each
/// hand-rolling the `ActivationScope` literal inline.
pub(crate) fn activation_scope_for_option(option_id: &str) -> Option<ActivationScope> {
    match option_id {
        "chat-only" => Some(ActivationScope::ChatOnly),
        "everywhere" => Some(ActivationScope::Everywhere),
        _ => None,
    }
}

/// Phase 49.4 Plan 10 (D-18): whether a typed archive-confirmation name
/// exactly matches `target`, once surrounding whitespace on the typed side
/// is trimmed. Case-sensitive, exact match only — this is the single source
/// of truth for the archive confirm button's disabled state; nowhere else
/// duplicates this comparison inline.
pub(crate) fn name_match_enables_confirm(typed: &str, target: &str) -> bool {
    typed.trim() == target
}

#[component]
pub fn ScreenSoul(is_active: bool) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).

    // Phase 49.4 Plan 01: bumped after a successful SAVE to re-fetch both
    // the profile list (health can change) and the persona. Phase 49.4
    // Plan 10 additionally bumps this after a successful activate, archive,
    // or profile creation — every one of those changes what this page must
    // re-derive.
    let mut refresh_tick: Signal<u32> = use_signal(|| 0);
    // Phase 49.4: default to the ROOT/master persona so the Soul screen opens on
    // the default profile's `~/.ironhermes/SOUL.md` (the only persona with
    // content on a typical install — the per-profile SOUL.md files are usually
    // empty). The DEFAULT tab and every profile tab switch this.
    let mut selected_profile: Signal<Option<String>> =
        use_signal(|| Some(ROOT_PERSONA_NAME.to_string()));
    let mut editor_body: Signal<String> = use_signal(String::new);
    let mut dirty: Signal<bool> = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut just_saved: Signal<bool> = use_signal(|| false);
    // CR-03: armed when a persona-tab click is blocked because the editor
    // has unsaved edits (`dirty`) — surfaces an inline warning instead of
    // silently discarding the edit, matching this screen's own REVERT
    // (explicit discard) and ARCHIVE (typed-confirm) precedent for every
    // other destructive-to-unsaved-work action.
    let mut tab_switch_blocked: Signal<bool> = use_signal(|| false);
    // Tracks which (profile, refresh_tick) pair the editor buffer was last
    // seeded from — the keyed seed-once guard.
    let mut seeded_for: Signal<Option<(String, u32)>> = use_signal(|| None);

    // Phase 49.4 Plan 10 (D-14): activate-with-scope prompt state. `None`
    // means the prompt is not armed — arming happens from a persona card's
    // ACTIVATE control.
    let mut activate_target: Signal<Option<String>> = use_signal(|| None);
    let mut activating: Signal<bool> = use_signal(|| false);
    let mut activate_error: Signal<Option<String>> = use_signal(|| None);

    // Phase 49.4 Plan 10 (D-15): add-new-profile dialog toggle.
    let mut add_profile_open: Signal<bool> = use_signal(|| false);

    // Phase 49.4 Plan 10 (D-18): archive-confirm state, mirroring the
    // activate-prompt arming shape above.
    let mut archive_target: Signal<Option<String>> = use_signal(|| None);
    let mut archiving: Signal<bool> = use_signal(|| false);
    let mut archive_error: Signal<Option<String>> = use_signal(|| None);
    let mut archive_typed_name: Signal<String> = use_signal(String::new);

    // Profile list — the persona tab strip.
    let profiles_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { list_profiles().await }
    });

    let profiles_loading = profiles_resource().is_none();
    let profiles_load_error = matches!(profiles_resource(), Some(Err(_)));
    let profiles: Vec<ProfileRow> = match profiles_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };

    // Phase 49.4 Plan 10 (D-14): the real persisted activation record for
    // this page's "which profile is ACTUALLY active" question — falls back
    // to the environment-derived live profile when no record has ever been
    // written, matching `profile_activation_api`'s own documented fallback
    // contract ("an un-activated install is byte-for-byte unaffected").
    // Keyed on the same `refresh_tick` every other resource on this screen
    // uses, so an activate/archive/create all invalidate it together.
    let active_resource: Resource<(Option<ActiveProfileRecord>, String)> = use_resource(move || {
        let _tick = refresh_tick();
        async move {
            let record = get_active_profile().await.unwrap_or(None);
            let live = live_profile_name().await.unwrap_or_default();
            (record, live)
        }
    });
    let (active_record, live_fallback_name) = active_resource().unwrap_or_default();

    // Phase 49.4 Plan 12 (D-16): the Soul-side half of the profile-to-bot
    // binding editor — reads the SAME store `bot_roster.rs`'s roster-side
    // selector writes through, via the SAME `set_bot_binding` entry point.
    // Keyed on the same `refresh_tick` so any change on either side (this
    // page's own bind/unbind control, or a roster-side change from a
    // sibling browser tab reloading this one) is picked up on the next
    // natural refresh — never a parallel store.
    let bindings_resource = use_resource(move || {
        let _tick = refresh_tick();
        async move { list_bot_bindings().await }
    });
    let bindings: Vec<BotBinding> = match bindings_resource() {
        Some(Ok(v)) => v,
        _ => Vec::new(),
    };
    let mut binding_saving: Signal<Option<String>> = use_signal(|| None);
    let mut binding_error: Signal<Option<(String, String)>> = use_signal(|| None);
    let binding_saving_val = binding_saving.read().clone();
    let binding_error_val = binding_error.read().clone();
    // T-49.4-10-04 / Task 3's own "re-read so the page does not keep
    // showing a stale active profile": `archive_profile` has no fn that
    // clears a persisted `Config.active_profile` record pointing at the
    // profile just archived (plan 08 built no such primitive, and it is
    // out of this plan's declared `files_modified`). Guard client-side
    // instead — only trust the record when its name still appears in the
    // freshly re-fetched profile list; a record for an archived (now
    // absent) profile falls back to the environment-derived name rather
    // than displaying a profile that no longer exists.
    let active_name: String = active_record
        .as_ref()
        .filter(|r| profiles.iter().any(|p| p.name == r.name))
        .map(|r| r.name.clone())
        .unwrap_or(live_fallback_name);

    // Auto-select the first profile once the list resolves, if nothing is
    // selected yet. Never overrides an existing selection.
    //
    // Phase 49.4 hotfix: the profiles resource is read INSIDE this effect (not
    // captured into a snapshot outside it) so the list ARRIVING actually
    // re-runs the effect — that read is the effect's reactive dependency.
    // `selected_profile` is `.peek()`ed, never `.read()`ed: reading it would
    // subscribe the effect to the very signal it writes, pinning re-runs to
    // selection changes so it would never fire when the list loads (the exact
    // reason SOUL.md sat on LOADING forever — persona resolved to Some(None)
    // because nothing was ever selected). This is the crate's rule: `.read()`
    // in render, `.peek()` in effects.
    use_effect(move || {
        let first_name = match profiles_resource() {
            Some(Ok(rows)) => rows.first().map(|r| r.name.clone()),
            _ => None,
        };
        if selected_profile.peek().is_none() {
            if let Some(name) = first_name {
                selected_profile.set(Some(name));
            }
        }
    });

    // Persona body for the selected tab — keyed on `selected_profile` +
    // `refresh_tick` (both read in the SYNC prefix, so either change
    // re-runs the fetch). `None` selection (only transient, before the
    // effect above assigns the first profile) yields `Some(None)`, folded
    // into the loading state below.
    let persona_resource: Resource<Option<Result<ProfilePersona, ServerFnError>>> =
        use_resource(move || {
            let name = selected_profile.read().clone();
            let _tick = refresh_tick();
            async move {
                match name {
                    Some(n) if n == ROOT_PERSONA_NAME => Some(fetch_root_persona().await),
                    Some(n) => Some(fetch_profile_persona(n).await),
                    None => None,
                }
            }
        });

    let persona_loading = matches!(persona_resource(), None | Some(None));
    let persona_error: Option<String> = match persona_resource() {
        Some(Some(Err(e))) => Some(e.to_string()),
        _ => None,
    };
    // (The resolved persona is read directly inside the seed effect below —
    // see the comment there for why it must not be captured out here.)

    // Seed the editor buffer once per (profile, tick) — never on every
    // render, so operator keystrokes are never clobbered by a re-render.
    // Phase 49.4 fix: read `persona_resource` INSIDE the effect so the arriving
    // fetch is the effect's reactive dependency. Previously the resolved persona
    // was captured OUTSIDE (`let resolved = persona_ok.clone()`), so the effect
    // never subscribed to the resource and the editor was never seeded when the
    // body arrived — SOUL.md stayed at "0 LINES". Every per-profile SOUL.md on a
    // typical install is empty, which masked this; the ROOT/master persona (the
    // one file with real content) is what exposed it. Everything the effect
    // writes — and the selection/tick it keys on — is `.peek()`ed, never
    // `.read()`ed, so writing them cannot re-arm the effect (crate rule:
    // `.read()` in render, `.peek()` in effects). The resource itself already
    // re-runs on a selection or `refresh_tick` change, so subscribing to it
    // alone covers every case the old captures were trying to cover.
    use_effect(move || {
        let persona = match persona_resource() {
            Some(Some(Ok(p))) => p,
            _ => return,
        };
        let Some(name) = selected_profile.peek().clone() else {
            return;
        };
        if persona.name != name {
            return;
        }
        let key = (name.clone(), *refresh_tick.peek());
        if seeded_for.peek().as_ref() == Some(&key) {
            return;
        }
        editor_body.set(persona.body.clone());
        dirty.set(false);
        just_saved.set(false);
        tab_switch_blocked.set(false);
        seeded_for.set(Some(key));
    });

    let editor_body_val = editor_body.read().clone();
    let dirty_val = *dirty.read();
    let saving_val = *saving.read();
    let save_error_val = save_error.read().clone();
    let just_saved_val = *just_saved.read();
    let selected_val = selected_profile.read().clone();
    let tab_switch_blocked_val = *tab_switch_blocked.read();

    let line_count = if editor_body_val.is_empty() {
        0
    } else {
        editor_body_val.split('\n').count()
    };
    let kb_size = editor_body_val.len() as f64 / 1024.0;

    // D-20/D-21: one-line header — tag + title + live status, no
    // explanatory subtitle paragraph. Phase 49.4 Plan 10: the status line's
    // active name now comes from `active_name` (the real activation
    // record), not the first profile in the list.
    let status_line = soul_status_line(profiles.len(), &active_name);

    // Phase 49.4 Plan 10: read all arming/in-flight/error signals into
    // owned locals before rsx! (Pattern B — no borrow held into render or
    // across a later `.await`).
    let activate_target_val = activate_target.read().clone();
    let activating_val = *activating.read();
    let activate_error_val = activate_error.read().clone();
    let add_profile_open_val = *add_profile_open.read();
    let archive_target_val = archive_target.read().clone();
    let archiving_val = *archiving.read();
    let archive_error_val = archive_error.read().clone();
    let archive_typed_name_val = archive_typed_name.read().clone();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-soul",
            "data-screen-label": "07 Soul",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 07" }
                    h1 { class: "screen-title", "Soul" }
                    span { class: "screen-status", "{status_line}" }
                }
                div { class: "screen-actions",
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| add_profile_open.set(true),
                        "+ ADD PROFILE"
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        disabled: saving_val || persona_loading,
                        onclick: move |_| {
                            // REVERT: force a re-fetch (bumps refresh_tick,
                            // which also invalidates the seed key), clears
                            // dirty and any error/confirmation state.
                            save_error.set(None);
                            just_saved.set(false);
                            dirty.set(false);
                            tab_switch_blocked.set(false);
                            refresh_tick.set(refresh_tick() + 1);
                        },
                        "REVERT"
                    }
                    button {
                        class: "btn btn--sm",
                        disabled: saving_val || persona_loading || selected_val.is_none(),
                        onclick: move |_| {
                            let Some(name) = selected_profile.read().clone() else {
                                return;
                            };
                            // Pattern B: read all signal values into owned
                            // locals BEFORE spawn — no borrow across .await.
                            let body_local = editor_body.read().clone();

                            saving.set(true);
                            save_error.set(None);
                            just_saved.set(false);

                            spawn(async move {
                                let save_result = if name == ROOT_PERSONA_NAME {
                                    save_root_persona(body_local).await
                                } else {
                                    save_profile_persona(name, body_local).await
                                };
                                match save_result {
                                    Ok(()) => {
                                        saving.set(false);
                                        dirty.set(false);
                                        just_saved.set(true);
                                        tab_switch_blocked.set(false);
                                        refresh_tick.set(refresh_tick() + 1);
                                    }
                                    Err(e) => {
                                        saving.set(false);
                                        save_error.set(Some(e.to_string()));
                                    }
                                }
                            });
                        },
                        if saving_val { "SAVING…" } else { "▓ SAVE" }
                    }
                }
            }

            // Phase 49.4 Plan 10 (D-14): activate-with-scope prompt — armed
            // from a persona card's ACTIVATE control below. Rendered above
            // the tab strip, mirroring `schedules.rs`'s own inline-confirm
            // placement (immediately under the header, before the list it
            // acts on).
            if let Some(ref target_name) = activate_target_val {
                ProfileActivatePrompt {
                    profile_name: target_name.clone(),
                    activating: activating_val,
                    error: activate_error_val.clone(),
                    on_choose: {
                        let target_name = target_name.clone();
                        move |scope: ActivationScope| {
                            let name_local = target_name.clone();
                            activating.set(true);
                            activate_error.set(None);
                            spawn(async move {
                                match activate_profile(name_local, scope).await {
                                    Ok(()) => {
                                        activating.set(false);
                                        activate_target.set(None);
                                        refresh_tick.set(refresh_tick() + 1);
                                    }
                                    Err(e) => {
                                        activating.set(false);
                                        activate_error.set(Some(e.to_string()));
                                    }
                                }
                            });
                        }
                    },
                    on_dismiss: move |_| {
                        activate_target.set(None);
                        activate_error.set(None);
                    },
                }
            }

            // Phase 49.4 Plan 10 (D-18): archive confirmation — armed from a
            // persona card's ARCHIVE control below.
            if let Some(ref target_name) = archive_target_val {
                ArchiveProfileConfirm {
                    profile_name: target_name.clone(),
                    typed_name: archive_typed_name_val.clone(),
                    archiving: archiving_val,
                    error: archive_error_val.clone(),
                    on_typed: move |v: String| archive_typed_name.set(v),
                    on_confirm: {
                        let target_name = target_name.clone();
                        move |_| {
                            let name_local = target_name.clone();
                            archiving.set(true);
                            archive_error.set(None);
                            spawn(async move {
                                match archive_profile(name_local).await {
                                    Ok(()) => {
                                        archiving.set(false);
                                        archive_target.set(None);
                                        archive_typed_name.set(String::new());
                                        // D-14/T-49.4-10-04: an archived
                                        // profile might have been the active
                                        // one — re-read activation state
                                        // (via refresh_tick) rather than
                                        // keep showing it as active.
                                        refresh_tick.set(refresh_tick() + 1);
                                    }
                                    Err(e) => {
                                        archiving.set(false);
                                        archive_error.set(Some(e.to_string()));
                                    }
                                }
                            });
                        }
                    },
                    on_cancel: move |_| {
                        archive_target.set(None);
                        archive_error.set(None);
                        archive_typed_name.set(String::new());
                    },
                }
            }

            // Persona picker — one tab per real profile (D-17/E9). Loading
            // and error states mirror `profile_shared::switcher` rather
            // than inventing a new surface.
            if profiles_load_error {
                div {
                    class: "kn-modal-error",
                    style: "margin-bottom:10px;",
                    "Could not read ~/.ironhermes/profiles/. Check permissions and retry."
                }
            } else if profiles_loading {
                div { class: "tabs", "aria-hidden": "true",
                    button { class: "tab", style: "opacity:0.35;", disabled: true, "…" }
                    button { class: "tab", style: "opacity:0.35;", disabled: true, "…" }
                    button { class: "tab", style: "opacity:0.35;", disabled: true, "…" }
                }
            } else {
                div { class: "tabs",
                    // Phase 49.4: DEFAULT tab for the ROOT/master persona
                    // (`~/.ironhermes/SOUL.md`). Uses the SAME proven tab render
                    // as the profile tabs below — deliberately NOT a `<select>`,
                    // which panicked the whole Soul render when tried inline.
                    {
                        let root_editing = selected_val.as_deref() == Some(ROOT_PERSONA_NAME);
                        rsx! {
                            button {
                                key: "{ROOT_PERSONA_NAME}",
                                class: "tab",
                                class: if root_editing { "is-editing" },
                                "data-persona-id": "{ROOT_PERSONA_NAME}",
                                title: "DEFAULT — the master ~/.ironhermes/SOUL.md",
                                onclick: move |_| {
                                    if *dirty.peek() {
                                        tab_switch_blocked.set(true);
                                        return;
                                    }
                                    selected_profile.set(Some(ROOT_PERSONA_NAME.to_string()));
                                },
                                span { style: "overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:160px;display:inline-block;vertical-align:bottom;",
                                    "DEFAULT"
                                }
                            }
                        }
                    }
                    for p in profiles.iter().cloned() {
                        {
                            let name_for_click = p.name.clone();
                            let is_editing = selected_val.as_deref() == Some(p.name.as_str());
                            let is_live = p.name == active_name;
                            rsx! {
                                button {
                                    key: "{p.name}",
                                    class: "tab",
                                    class: if is_live { "is-active" },
                                    class: if is_editing && !is_live { "is-editing" },
                                    "data-persona-id": "{p.name}",
                                    title: "{p.name}",
                                    onclick: move |_| {
                                        // CR-03: never silently discard an
                                        // unsaved edit by switching the seed
                                        // key out from under it — block the
                                        // switch and surface an inline
                                        // warning until SAVE or REVERT
                                        // resolves `dirty`.
                                        if *dirty.peek() {
                                            tab_switch_blocked.set(true);
                                            return;
                                        }
                                        selected_profile.set(Some(name_for_click.clone()));
                                    },
                                    span { style: "overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:160px;display:inline-block;vertical-align:bottom;",
                                        "{p.name}"
                                    }
                                    if p.health == ProfileHealth::Incomplete {
                                        span {
                                            style: "display:inline-block;width:6px;height:6px;border-radius:50%;background:var(--amber);margin-left:6px;",
                                            "aria-hidden": "true",
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "soul-grid",
                div { class: "soul-editor",
                    div { style: "display:flex;justify-content:space-between;align-items:center;",
                        div { class: "panel-title", "SOUL.md" }
                        div { style: "display:flex;gap:8px;font-size:10px;color:var(--gray);letter-spacing:0.12em;",
                            span { "{line_count} LINES" }
                            span { "·" }
                            span { "{kb_size:.1} KB" }
                            span { "·" }
                            if persona_loading {
                                span { "LOADING" }
                            } else if dirty_val {
                                span { style: "color:var(--amber)", "UNSAVED" }
                            } else {
                                span { style: "color:var(--green)", "SAVED" }
                            }
                        }
                    }
                    if let Some(ref reason) = save_error_val {
                        div {
                            style: "color:var(--red);font-size:12px;",
                            "Couldn't save SOUL.md — {reason}. Your edits are still in the editor; retry SAVE."
                        }
                    } else if let Some(ref reason) = persona_error {
                        div {
                            style: "color:var(--red);font-size:12px;",
                            "Couldn't load SOUL.md — {reason}."
                        }
                    } else if tab_switch_blocked_val && dirty_val {
                        div {
                            style: "color:var(--amber);font-size:12px;",
                            "You have unsaved changes — SAVE or REVERT first before switching personas."
                        }
                    } else if just_saved_val {
                        div {
                            style: "color:var(--green);font-size:12px;",
                            "Saved. Takes effect on the profile's next message."
                        }
                    }
                    textarea {
                        spellcheck: "false",
                        readonly: persona_loading,
                        value: "{editor_body_val}",
                        oninput: move |e| {
                            just_saved.set(false);
                            save_error.set(None);
                            editor_body.set(e.value());
                            dirty.set(true);
                        },
                    }
                }

                div { class: "soul-preview",
                    div { class: "panel-title", "Active Personas" }
                    div { class: "soul-preview-body",
                        for p in profiles.iter().cloned() {
                            {
                                let is_active_flag = p.name == active_name;
                                // Phase 49.4 Plan 12 (D-16): every platform-adapter
                                // key currently bound to THIS profile — the
                                // Soul-side half of the binding editor. Derived
                                // from the SAME `bindings` list the roster's own
                                // selector reads, never a second store.
                                let bound_bots: Vec<String> = bindings
                                    .iter()
                                    .filter(|b| b.profile_name == p.name)
                                    .map(|b| b.bot_key.0.clone())
                                    .collect();
                                let card_saving = binding_saving_val.clone();
                                let card_error = binding_error_val.clone();
                                rsx! {
                                    PersonaCard {
                                        key: "{p.name}",
                                        is_active: is_active_flag,
                                        profile: p,
                                        bound_bots,
                                        saving_key: card_saving,
                                        bind_error: card_error,
                                        on_activate: move |name: String| {
                                            activate_error.set(None);
                                            activate_target.set(Some(name));
                                        },
                                        on_archive: move |name: String| {
                                            archive_error.set(None);
                                            archive_typed_name.set(String::new());
                                            archive_target.set(Some(name));
                                        },
                                        on_bind: move |(bot_key, target_profile): (String, String)| {
                                            binding_error.set(None);
                                            binding_saving.set(Some(bot_key.clone()));
                                            spawn(async move {
                                                match set_bot_binding(bot_key.clone(), target_profile).await {
                                                    Ok(()) => {
                                                        binding_saving.set(None);
                                                        refresh_tick.set(refresh_tick() + 1);
                                                    }
                                                    Err(e) => {
                                                        binding_saving.set(None);
                                                        binding_error.set(Some((bot_key, e.to_string())));
                                                    }
                                                }
                                            });
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Phase 49.4 Plan 10 (D-15): the add-new-profile dialog. Reuses
            // the shared `CreateProfileWizard` (never a second wizard) with
            // its new `show_entry_step` opt-in — the two pre-existing
            // mounts (Kanban board, Agents roster) omit this prop and keep
            // landing on their previous first step unchanged.
            if add_profile_open_val {
                CreateProfileWizard {
                    context: ProfileDialogContext::Bot,
                    show_entry_step: true,
                    on_dismiss: move |_| add_profile_open.set(false),
                    on_created: move |_name: String| {
                        add_profile_open.set(false);
                        refresh_tick.set(refresh_tick() + 1);
                    },
                }
            }
        }
    }
}

/// Phase 49.4 Plan 10 (D-14): the two-choice activate-with-scope prompt.
/// Never activates on its own — the caller's `on_choose` handler is the
/// only thing that ever calls `activate_profile`. No modal overlay; this is
/// the same inline arming idiom `schedules.rs:94`'s delete confirm uses.
#[component]
fn ProfileActivatePrompt(
    profile_name: String,
    activating: bool,
    error: Option<String>,
    on_choose: EventHandler<ActivationScope>,
    on_dismiss: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "panel profile-activate-prompt",
            style: "margin-bottom:14px;",
            div { class: "panel-title", "Activate profile" }
            h3 {
                style: "font-size:14px;font-weight:700;color:var(--text);margin:0;",
                "Activate \"{profile_name}\" for…"
            }
            if let Some(ref reason) = error {
                div { style: "color:var(--red);font-size:12px;", "{reason}" }
            }
            div { style: "display:flex;gap:10px;flex-wrap:wrap;",
                button {
                    class: "btn btn--sm",
                    disabled: activating,
                    onclick: move |_| {
                        if let Some(scope) = activation_scope_for_option("chat-only") {
                            on_choose.call(scope);
                        }
                    },
                    "This chat only"
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    disabled: activating,
                    onclick: move |_| {
                        if let Some(scope) = activation_scope_for_option("everywhere") {
                            on_choose.call(scope);
                        }
                    },
                    "Everywhere (chat, editor, gateway/workers)"
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| on_dismiss.call(()),
                    "CANCEL"
                }
            }
        }
    }
}

/// Phase 49.4 Plan 10 (D-18): the typed-name-gated archive confirmation.
/// The confirm button's enabled state is derived entirely from
/// `name_match_enables_confirm` — never an inline comparison scattered
/// through the markup. Calls `profile_api::archive_profile` only, never the
/// pre-existing hard-removal fn.
#[component]
fn ArchiveProfileConfirm(
    profile_name: String,
    typed_name: String,
    archiving: bool,
    error: Option<String>,
    on_typed: EventHandler<String>,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let confirm_enabled = !archiving && name_match_enables_confirm(&typed_name, &profile_name);
    rsx! {
        div {
            class: "panel profile-archive-confirm",
            style: "margin-bottom:14px;border-color:rgba(248,81,73,0.45);background:rgba(248,81,73,0.06);",
            div { class: "panel-title", style: "color:var(--red);", "Archive profile" }
            p {
                style: "color:var(--text);font-size:12px;margin:0;",
                "Delete \"{profile_name}\"? This archives the profile (state, .env, logs preserved) — it does not permanently delete it."
            }
            label {
                style: "font-size:10px;letter-spacing:0.12em;color:var(--gray);text-transform:uppercase;",
                "Type the profile name to confirm."
            }
            input {
                class: "field-input",
                value: "{typed_name}",
                disabled: archiving,
                oninput: move |e| on_typed.call(e.value()),
            }
            if let Some(ref reason) = error {
                div { style: "color:var(--red);font-size:12px;", "{reason}" }
            }
            div { style: "display:flex;gap:10px;",
                button {
                    class: "btn btn--sm btn--danger",
                    disabled: !confirm_enabled,
                    onclick: move |_| on_confirm.call(()),
                    if archiving { "ARCHIVING…" } else { "ARCHIVE" }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    disabled: archiving,
                    onclick: move |_| on_cancel.call(()),
                    "CANCEL"
                }
            }
        }
    }
}

#[component]
fn PersonaCard(
    profile: ProfileRow,
    is_active: bool,
    // Phase 49.4 Plan 12 (D-16): every platform-adapter key currently bound
    // to this profile.
    bound_bots: Vec<String>,
    // Which bot key (if any) has a set-binding call in flight, and the last
    // (bot_key, message) failure — both owned by `ScreenSoul` and passed
    // down, mirroring `is_active`'s own "caller computes, card renders"
    // split.
    saving_key: Option<String>,
    bind_error: Option<(String, String)>,
    on_activate: EventHandler<String>,
    on_archive: EventHandler<String>,
    // (bot_key, target_profile) — the SAME `set_bot_binding` entry point
    // `bot_roster.rs`'s roster-side selector calls, never a parallel path.
    on_bind: EventHandler<(String, String)>,
) -> Element {
    let name_for_activate = profile.name.clone();
    let name_for_archive = profile.name.clone();
    let profile_name = profile.name.clone();
    rsx! {
        div {
            class: "card",
            class: if is_active { "is-active" },
            "data-persona-id": "{profile.name}",
            div { class: "card-head",
                div { style: "flex:1",
                    div { class: "card-title", "{profile.name}" }
                }
                if is_active {
                    span { class: "pill teal", "ACTIVE" }
                } else if profile.health == ProfileHealth::Incomplete {
                    span { class: "pill amber", "INCOMPLETE" }
                }
            }
            div { class: "card-body",
                if let Some(ref provider) = profile.provider {
                    "{provider} · {profile.key_count} keys"
                } else {
                    "{profile.key_count} keys"
                }
            }
            // Phase 49.4 Plan 12 (D-16): the Soul-side bot-assignment
            // control — one toggle per fixed platform-adapter key. Bound
            // keys render highlighted; clicking a bound key unbinds it (an
            // explicit set to the default profile, per this module's own
            // "no separate clear call" simplification); clicking an
            // unbound key binds it to THIS card's profile.
            div { style: "margin-top:8px;",
                div {
                    style: "font-size:10px;letter-spacing:0.1em;color:var(--gray);text-transform:uppercase;",
                    "Bots"
                }
                div { style: "display:flex;flex-wrap:wrap;gap:4px;margin-top:4px;",
                    for key in PLATFORM_KEYS {
                        {
                            let is_bound = bound_bots.iter().any(|b| b == key);
                            let is_saving = saving_key.as_deref() == Some(key);
                            let key_s = key.to_string();
                            let target_profile = profile_name.clone();
                            rsx! {
                                button {
                                    key: "{key}",
                                    class: "pill",
                                    class: if is_bound { "teal" },
                                    disabled: is_saving,
                                    title: if is_bound { "Bound — click to unbind" } else { "Not bound — click to bind" },
                                    onclick: move |_| {
                                        let next = if is_bound {
                                            DEFAULT_PROFILE_LABEL.to_string()
                                        } else {
                                            target_profile.clone()
                                        };
                                        on_bind.call((key_s.clone(), next));
                                    },
                                    "{key}"
                                }
                            }
                        }
                    }
                }
                // A bind failure is screen-scoped (one `(bot_key, message)`
                // pair, not per-profile) — surfaced on every card so the
                // operator sees it regardless of which card's control they
                // clicked, matching this control's card-agnostic PLATFORM_KEYS
                // rendering above.
                if let Some((ref err_key, ref reason)) = bind_error {
                    div { style: "color:var(--red);font-size:11px;margin-top:4px;", "{err_key}: {reason}" }
                }
            }
            div { style: "display:flex;gap:8px;margin-top:8px;",
                if !is_active {
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| on_activate.call(name_for_activate.clone()),
                        "ACTIVATE"
                    }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| on_archive.call(name_for_archive.clone()),
                    "ARCHIVE"
                }
            }
        }
    }
}

#[cfg(test)]
mod soul_status_line_tests {
    use super::soul_status_line;

    #[test]
    fn singular_profile() {
        assert_eq!(soul_status_line(1, "default"), "1 profile · default active");
    }

    #[test]
    fn plural_profiles() {
        assert_eq!(soul_status_line(4, "default"), "4 profiles · default active");
    }

    #[test]
    fn long_active_name_passes_through_unmodified() {
        // The ellipsis is CSS (.screen-status), not Rust truncation — a
        // long name must pass through the helper byte-for-byte.
        let long_name =
            "a-very-long-profile-name-that-would-need-css-ellipsis-not-rust-truncation";
        assert_eq!(
            soul_status_line(3, long_name),
            format!("3 profiles · {long_name} active")
        );
    }
}

#[cfg(test)]
mod activation_scope_for_option_tests {
    use super::*;

    #[test]
    fn chat_only_option_maps_to_chat_only_scope() {
        assert_eq!(
            activation_scope_for_option("chat-only"),
            Some(ActivationScope::ChatOnly)
        );
    }

    #[test]
    fn everywhere_option_maps_to_everywhere_scope() {
        assert_eq!(
            activation_scope_for_option("everywhere"),
            Some(ActivationScope::Everywhere)
        );
    }

    #[test]
    fn unknown_option_maps_to_none() {
        assert_eq!(activation_scope_for_option("bogus"), None);
    }
}

#[cfg(test)]
mod name_match_enables_confirm_tests {
    use super::name_match_enables_confirm;

    #[test]
    fn empty_typed_is_false() {
        assert!(!name_match_enables_confirm("", "alpha"));
    }

    #[test]
    fn partial_typed_is_false() {
        assert!(!name_match_enables_confirm("alph", "alpha"));
    }

    #[test]
    fn case_mismatch_is_false() {
        assert!(!name_match_enables_confirm("ALPHA", "alpha"));
    }

    #[test]
    fn exact_match_is_true() {
        assert!(name_match_enables_confirm("alpha", "alpha"));
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert!(name_match_enables_confirm(" alpha ", "alpha"));
    }
}
