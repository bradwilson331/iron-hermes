//! Phase 50.1 Plan 05 (D-15/D-16): the Advanced pane — model override,
//! persona (SOUL.md) and a per-bot skills checklist, shared verbatim by
//! the create wizard's collapsed disclosure and the detail drawer's
//! Advanced section. One implementation, two mount sites (D-10's "backed
//! by one component" requirement).
//!
//! **Mount precondition: the target profile must already exist.** Every
//! save here writes through the target profile's own `config.yaml` (model
//! override, skills) or its own workspace directory (persona) — neither
//! exists until `create_profile` has already succeeded. The create
//! wizard's Verify step (reached only after a successful create) is
//! therefore this pane's only wizard mount point; it is never mounted
//! against a not-yet-created profile.
//!
//! Callers MUST mount this component keyed by `bot_name`
//! (`AdvancedProfilePane { key: "{bot_name}", ... }`) — `bot_name` is a
//! plain `String` prop, not a `Signal`, so a caller that switches which
//! bot it is showing (the drawer, moving between roster cards) needs the
//! `key` to force a fresh mount; otherwise this pane's `use_resource`
//! fetches would not necessarily re-run against the new name.
//!
//! No context provider, no resource-restart call — matches
//! `create_dialog.rs`'s and `edit_dialog.rs`'s own module-doc discipline.

use crate::protocol::ProfileConfigWritePayload;
use crate::server::profile_api::{
    fetch_profile_detail, fetch_profile_persona, fetch_profile_skills, save_profile_persona,
    update_profile_config,
};
use dioxus::prelude::*;
use std::collections::HashSet;

#[component]
pub fn AdvancedProfilePane(
    bot_name: String,
    #[props(default)] refresh_tick: u32,
    on_saved: EventHandler<()>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E —
    // agents.rs UAT-2 hotfix discipline, matched throughout profile_shared/).

    // ---- Model override working copy ----
    let mut model_wc: Signal<String> = use_signal(String::new);
    let mut model_seeded: Signal<bool> = use_signal(|| false);
    let mut model_saving: Signal<bool> = use_signal(|| false);
    let mut model_error: Signal<Option<String>> = use_signal(|| None);

    // ---- Persona (SOUL.md) working copy ----
    let mut persona_wc: Signal<String> = use_signal(String::new);
    let mut persona_seeded: Signal<bool> = use_signal(|| false);
    let mut persona_saving: Signal<bool> = use_signal(|| false);
    let mut persona_error: Signal<Option<String>> = use_signal(|| None);

    // ---- Skills working copy ----
    // Holds the DISABLED (unchecked) set directly, mirroring
    // `SkillsConfig::disabled`'s own on-disk shape — `None` until the
    // catalog's first resolve seeds it once.
    let mut skills_disabled_wc: Signal<Option<HashSet<String>>> = use_signal(|| None);
    let mut skills_saving: Signal<bool> = use_signal(|| false);
    let mut skills_error: Signal<Option<String>> = use_signal(|| None);

    // ---- Resources — sync prefix reads `refresh_tick`, this crate's
    // proven refresh idiom (never a resource-restart call). ----
    let bot_name_for_detail = bot_name.clone();
    let detail_resource = use_resource(move || {
        let _tick = refresh_tick;
        let name = bot_name_for_detail.clone();
        async move { fetch_profile_detail(name).await }
    });
    let bot_name_for_persona = bot_name.clone();
    let persona_resource = use_resource(move || {
        let _tick = refresh_tick;
        let name = bot_name_for_persona.clone();
        async move { fetch_profile_persona(name).await }
    });
    let bot_name_for_skills = bot_name.clone();
    let skills_resource = use_resource(move || {
        let _tick = refresh_tick;
        let name = bot_name_for_skills.clone();
        async move { fetch_profile_skills(name).await }
    });

    // Seed each working copy exactly once per resolved fetch — never
    // clobber an in-progress edit on a later re-render. `use_effect` (not a
    // render-body mutation) is this crate's sanctioned place to react to a
    // resource resolving, matching `edit_dialog.rs`'s own fetch-then-seed
    // shape.
    use_effect(move || {
        if let Some(Ok(detail)) = detail_resource() {
            if !*model_seeded.peek() {
                model_wc.set(detail.model_default.clone().unwrap_or_default());
                model_seeded.set(true);
            }
        }
    });
    use_effect(move || {
        if let Some(Ok(persona)) = persona_resource() {
            if !*persona_seeded.peek() {
                persona_wc.set(persona.body.clone());
                persona_seeded.set(true);
            }
        }
    });
    use_effect(move || {
        if let Some(Ok(rows)) = skills_resource() {
            if skills_disabled_wc.peek().is_none() {
                let disabled: HashSet<String> = rows
                    .iter()
                    .filter(|r| !r.enabled_for_profile)
                    .map(|r| r.name.clone())
                    .collect();
                skills_disabled_wc.set(Some(disabled));
            }
        }
    });

    // ---- Derived values — read BEFORE rsx! (clippy.toml signal-borrow
    // discipline). ----
    let model_val = model_wc.read().clone();
    let persona_val = persona_wc.read().clone();
    let skills_snapshot = skills_resource();

    // ---- Model override SAVE ----
    let name_for_model_save = bot_name.clone();
    let on_model_save = move |_| {
        let profile_name = name_for_model_save.clone();
        let value = model_wc.peek().clone();
        model_saving.set(true);
        model_error.set(None);
        let mut saving_sig = model_saving;
        let mut error_sig = model_error;
        spawn(async move {
            let payload = ProfileConfigWritePayload {
                name: profile_name,
                provider: None,
                model_default: Some(value),
                skills_disabled: None,
            };
            match update_profile_config(payload).await {
                Ok(()) => {
                    saving_sig.set(false);
                    on_saved.call(());
                }
                Err(e) => {
                    saving_sig.set(false);
                    error_sig.set(Some(format!("{e}")));
                }
            }
        });
    };

    // ---- Persona SAVE ----
    let name_for_persona_save = bot_name.clone();
    let on_persona_save = move |_| {
        let profile_name = name_for_persona_save.clone();
        let body = persona_wc.peek().clone();
        persona_saving.set(true);
        persona_error.set(None);
        let mut saving_sig = persona_saving;
        let mut error_sig = persona_error;
        spawn(async move {
            match save_profile_persona(profile_name, body).await {
                Ok(()) => {
                    saving_sig.set(false);
                    on_saved.call(());
                }
                Err(e) => {
                    saving_sig.set(false);
                    error_sig.set(Some(format!("{e}")));
                }
            }
        });
    };

    rsx! {
        div { class: "kn-advanced-pane",
            // ---- Model override ----
            div { class: "kn-drawer-section",
                label { class: "kn-modal-label", "MODEL OVERRIDE" }
                input {
                    class: "kn-modal-input",
                    placeholder: "inherits from provider default",
                    value: "{model_val}",
                    oninput: move |evt| model_wc.set(evt.value()),
                }
                button {
                    class: "kn-action-btn",
                    disabled: *model_saving.read(),
                    onclick: on_model_save,
                    if *model_saving.read() { "SAVING…" } else { "SAVE" }
                }
                if let Some(err) = model_error.read().clone() {
                    div { class: "kn-modal-error", "{err}" }
                }
            }
            // ---- Persona ----
            div { class: "kn-drawer-section",
                label { class: "kn-modal-label", "PERSONA (SOUL.MD)" }
                textarea {
                    class: "kn-modal-input kn-persona-input",
                    placeholder: "Leave blank to inherit the default identity.",
                    value: "{persona_val}",
                    oninput: move |evt| persona_wc.set(evt.value()),
                }
                button {
                    class: "kn-action-btn",
                    disabled: *persona_saving.read(),
                    onclick: on_persona_save,
                    if *persona_saving.read() { "SAVING…" } else { "SAVE" }
                }
                if let Some(err) = persona_error.read().clone() {
                    div { class: "kn-modal-error", "{err}" }
                }
            }
            // ---- Skills ----
            div { class: "kn-drawer-section",
                label { class: "kn-modal-label", "SKILLS FOR THIS BOT" }
                match &skills_snapshot {
                    None => rsx! {
                        div { class: "kn-drawer-loading", "Loading skills…" }
                    },
                    Some(Err(e)) => rsx! {
                        div { class: "kn-modal-error", "Could not load the skills catalog: {e}" }
                    },
                    Some(Ok(rows)) if rows.is_empty() => rsx! {
                        div { class: "kn-drawer-empty",
                            "No skills installed on this machine yet."
                        }
                    },
                    Some(Ok(rows)) => rsx! {
                        for row in rows.iter().cloned() {
                            {
                                let disabled_val = skills_disabled_wc.read().clone();
                                let is_checked = match disabled_val {
                                    Some(ref disabled) => !disabled.contains(&row.name),
                                    None => row.enabled_for_profile,
                                };
                                let row_name = row.name.clone();
                                let name_for_toggle = bot_name.clone();
                                rsx! {
                                    label {
                                        class: "kn-modal-checkbox kn-skill-row",
                                        key: "{row.name}",
                                        input {
                                            r#type: "checkbox",
                                            checked: is_checked,
                                            disabled: *skills_saving.read(),
                                            onchange: move |evt| {
                                                let checked = evt.checked();
                                                let mut disabled = skills_disabled_wc
                                                    .peek()
                                                    .clone()
                                                    .unwrap_or_default();
                                                if checked {
                                                    disabled.remove(&row_name);
                                                } else {
                                                    disabled.insert(row_name.clone());
                                                }
                                                skills_disabled_wc.set(Some(disabled.clone()));
                                                let profile_name = name_for_toggle.clone();
                                                skills_saving.set(true);
                                                skills_error.set(None);
                                                let mut saving_sig = skills_saving;
                                                let mut error_sig = skills_error;
                                                spawn(async move {
                                                    let payload = ProfileConfigWritePayload {
                                                        name: profile_name,
                                                        provider: None,
                                                        model_default: None,
                                                        skills_disabled: Some(
                                                            disabled.into_iter().collect::<Vec<String>>(),
                                                        ),
                                                    };
                                                    match update_profile_config(payload).await {
                                                        Ok(()) => {
                                                            saving_sig.set(false);
                                                            on_saved.call(());
                                                        }
                                                        Err(e) => {
                                                            saving_sig.set(false);
                                                            error_sig.set(Some(format!("{e}")));
                                                        }
                                                    }
                                                });
                                            },
                                        }
                                        span { style: "font-size: var(--fs-13);", "{row.name}" }
                                        span {
                                            style: "font-size: var(--fs-11); color: var(--fg-dim);",
                                            "{row.category}"
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
                if let Some(err) = skills_error.read().clone() {
                    div { class: "kn-modal-error", "{err}" }
                }
            }
        }
    }
}
