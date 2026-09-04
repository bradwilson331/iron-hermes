//! Phase 49.4 Plan 07 (D-09): the SKILL.md editor — opened from a
//! skills-list row action. A BUNDLED skill's SAVE forks to a prefilled
//! derived name via plan 05's `fork_bundled_skill` `#[server]` fn instead of
//! ever offering an overwrite-in-place option; an INSTALLED skill opens
//! read/write with no fork banner. Loading mirrors `soul.rs`'s persona
//! editor exactly: a read-only textarea plus a loading chip, never a
//! flash-of-empty.
//!
//! Reading an existing skill's body has no analog among plan 05's four
//! entry points (all four are write-shaped or preview-shaped) — this file
//! also adds the one read-only `fetch_skill_body` `#[server]` fn the editor
//! needs to open anything at all (documented as a plan deviation; see the
//! plan 07 SUMMARY).

use dioxus::prelude::*;

use crate::server::skills_import_api::{fetch_skill_body, fork_bundled_skill};

/// Which skill the editor is open for, and whether it is BUNDLED (forks on
/// save) or INSTALLED (no fork banner). Owned by `screens/skills.rs`,
/// mirroring the `ReadSignal<Option<T>>` / `EventHandler` ownership split
/// `import_wizard.rs`/`new_skill_wizard.rs` already establish for `open`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorTarget {
    pub name: String,
    pub is_bundled: bool,
}

/// D-09: derive the fork-on-save target name for a bundled skill. Trims
/// input whitespace first. Idempotent in the sense the task requires: a
/// name that already carries the `-custom` suffix produces a DISTINCT name
/// rather than doubling it (`polymarket-custom` -> `polymarket-custom-2`,
/// never `polymarket-custom-custom`).
pub fn derive_fork_name(original: &str) -> String {
    let trimmed = original.trim();
    match trimmed.strip_suffix("-custom") {
        Some(base) => format!("{base}-custom-2"),
        None => format!("{trimmed}-custom"),
    }
}

#[component]
pub fn SkillMdEditor(
    target: ReadSignal<Option<EditorTarget>>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let mut body: Signal<String> = use_signal(String::new);
    let mut dirty: Signal<bool> = use_signal(|| false);
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut save_error: Signal<Option<String>> = use_signal(|| None);
    let mut fork_name: Signal<String> = use_signal(String::new);
    // Guards the editable buffer against clobbering in-progress edits on an
    // unrelated re-render — reseeds only when the TARGET NAME changes,
    // mirroring `soul.rs`'s `seeded_for` keyed-seed-once idiom.
    let mut seeded_for: Signal<Option<String>> = use_signal(|| None);

    // Fetch the selected skill's body — re-runs whenever `target` changes
    // (a new skill opened, or closed to None). Never restarts a
    // `use_server_future`; this is a plain `use_resource` keyed off the
    // reactive read below, matching `soul.rs`'s persona_resource shape.
    let body_resource: Resource<Option<Result<String, ServerFnError>>> = use_resource(move || {
        let t = target.read().clone();
        async move {
            match t {
                Some(t) => Some(fetch_skill_body(t.name.clone()).await),
                None => None,
            }
        }
    });

    let target_val = target.read().clone();
    let is_open = target_val.is_some();
    let is_bundled = target_val.as_ref().map(|t| t.is_bundled).unwrap_or(false);
    let target_name = target_val.as_ref().map(|t| t.name.clone());

    let loading = is_open && matches!(body_resource(), None | Some(None));
    let load_error: Option<String> = match body_resource() {
        Some(Some(Err(e))) => Some(e.to_string()),
        _ => None,
    };
    let loaded_body: Option<String> = match body_resource() {
        Some(Some(Ok(b))) => Some(b),
        _ => None,
    };

    // Reset the seed guard once the editor closes so reopening the SAME
    // skill later reliably reseeds from the fresh fetch rather than
    // reusing a stale in-memory buffer.
    {
        use_effect(move || {
            if target_name.is_none() {
                seeded_for.set(None);
            }
        });
    }

    // Seed the editable buffer + the fork-name field once per target name.
    {
        let name_key = target_val.as_ref().map(|t| t.name.clone());
        let body_for_seed = loaded_body.clone();
        use_effect(move || {
            let (Some(name), Some(b)) = (name_key.clone(), body_for_seed.clone()) else {
                return;
            };
            if seeded_for.read().as_ref() == Some(&name) {
                return;
            }
            body.set(b);
            dirty.set(false);
            save_error.set(None);
            fork_name.set(derive_fork_name(&name));
            seeded_for.set(Some(name));
        });
    }

    if !is_open {
        return rsx! {};
    }

    let body_val = body.read().clone();
    let saving_val = *saving.read();
    let save_error_val = save_error.read().clone();
    let fork_name_val = fork_name.read().clone();
    let line_count = if body_val.is_empty() {
        0
    } else {
        body_val.split('\n').count()
    };

    let original_name = target_val.map(|t| t.name).unwrap_or_default();
    let can_save_bundled = !fork_name_val.trim().is_empty() && !saving_val;

    let mut close_editor = move || {
        save_error.set(None);
        dirty.set(false);
        on_close.call(());
    };

    rsx! {
        div {
            class: "kn-modal-overlay",
            onclick: move |_| close_editor(),
            div {
                class: "skill-wizard",
                onclick: move |e| e.stop_propagation(),
                div { class: "kn-modal-header",
                    h2 { class: "kn-modal-title", "SKILL.md — {original_name}" }
                }
                div { class: "kn-modal-body",
                    if is_bundled {
                        div { class: "skill-fork-banner",
                            "Editing a bundled skill. Saving creates "
                            input {
                                class: "kn-modal-input",
                                style: "display:inline-block;width:auto;min-width:160px;",
                                value: "{fork_name_val}",
                                oninput: move |e| fork_name.set(e.value()),
                            }
                            " — the bundled skill is never changed."
                        }
                    }
                    div { style: "display:flex;justify-content:space-between;align-items:center;",
                        span { class: "kn-modal-label", "SKILL.md" }
                        div { style: "display:flex;gap:8px;font-size:10px;color:var(--gray);letter-spacing:0.12em;",
                            if loading {
                                span { "LOADING" }
                            } else {
                                span { "{line_count} LINES" }
                            }
                        }
                    }
                    if let Some(ref reason) = load_error {
                        div { class: "kn-modal-error", "Couldn't load skill — {reason}." }
                    } else if let Some(ref reason) = save_error_val {
                        div { class: "kn-modal-error", "Couldn't save — {reason}. Your edits are still in the editor." }
                    }
                    textarea {
                        class: "kn-modal-textarea kn-modal-textarea--mono skill-editor-textarea",
                        spellcheck: "false",
                        readonly: loading,
                        value: "{body_val}",
                        oninput: move |e| {
                            save_error.set(None);
                            body.set(e.value());
                            dirty.set(true);
                        },
                    }
                }
                div { class: "kn-modal-actions",
                    button {
                        class: "kn-modal-btn",
                        disabled: saving_val,
                        onclick: move |_| close_editor(),
                        "CLOSE"
                    }
                    if is_bundled {
                        button {
                            class: "kn-modal-btn kn-modal-btn--submit",
                            disabled: !can_save_bundled,
                            onclick: move |_| {
                                let original = original_name.clone();
                                let body_arg = body.read().clone();
                                let new_name = fork_name.read().clone();
                                saving.set(true);
                                save_error.set(None);
                                spawn(async move {
                                    match fork_bundled_skill(original, body_arg, new_name).await {
                                        Ok(_) => {
                                            saving.set(false);
                                            dirty.set(false);
                                            on_saved.call(());
                                            on_close.call(());
                                        }
                                        Err(e) => {
                                            saving.set(false);
                                            save_error.set(Some(e.to_string()));
                                        }
                                    }
                                });
                            },
                            if saving_val { "FORKING…" } else { "▓ SAVE (FORK)" }
                        }
                    } else {
                        // D-09 scopes fork-on-save to BUNDLED skills only —
                        // plan 05 ships no in-place update entry point for an
                        // already-installed skill's body, so in-place SAVE is
                        // deliberately unavailable here (documented in the
                        // plan 07 SUMMARY, not a silent stub).
                        button {
                            class: "kn-modal-btn",
                            disabled: true,
                            style: "opacity:0.5;",
                            title: "In-place editing for installed skills isn't available yet.",
                            "▓ SAVE"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod derive_fork_name_tests {
    use super::*;

    #[test]
    fn simple_name_gets_custom_suffix() {
        assert_eq!(derive_fork_name("polymarket"), "polymarket-custom");
    }

    #[test]
    fn already_forked_name_does_not_double_the_suffix() {
        let derived = derive_fork_name("polymarket-custom");
        assert_ne!(derived, "polymarket-custom");
        assert_ne!(derived, "polymarket-custom-custom");
        assert_eq!(derived, "polymarket-custom-2");
    }

    #[test]
    fn trailing_whitespace_is_trimmed_before_deriving() {
        assert_eq!(derive_fork_name("polymarket  \n"), "polymarket-custom");
    }
}
