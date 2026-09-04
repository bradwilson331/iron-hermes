//! Phase 49.4 Plan 07 (D-08): the NEW SKILL form wizard — name, description,
//! tags, and a body textarea, wired to plan 05's `create_skill` `#[server]`
//! fn. Reuses `import_wizard.rs`'s dialog shell (`.kn-modal-overlay` /
//! `.skill-wizard`) and `.kn-modal-*` form-control classes — no second
//! dialog vocabulary.
//!
//! Client-side validation mirrors, but does not call, the server's own
//! `ironhermes_hub::to_skill_slug` rule (this file compiles to wasm, which
//! cannot link that native-only crate) — duplicated deliberately, per
//! `profile_shared::create_dialog`'s own precedent for this exact tradeoff.
//! Duplicate-name detection is left entirely to the server: the loaded
//! skills list may be stale, so this file never pre-checks against it.

use dioxus::prelude::*;

use crate::server::skills_import_api::create_skill;

/// D-08 client-side create-gate: mirrors (does not call) the server's
/// `ironhermes_hub::to_skill_slug` empty-check ("must contain at least one
/// letter or number") plus a non-empty body. Description and tags are
/// optional and play no part in this predicate.
pub(crate) fn can_create_skill(name: &str, body: &str) -> bool {
    !body.trim().is_empty() && name.trim().chars().any(|c| c.is_ascii_alphanumeric())
}

/// Parse the comma-separated tags field into a vector, trimming each entry
/// and dropping empties — tags are optional (D-08 partial rule).
pub(crate) fn parse_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

#[component]
pub fn NewSkillWizard(
    open: ReadSignal<bool>,
    on_close: EventHandler<()>,
    on_created: EventHandler<()>,
) -> Element {
    let mut name: Signal<String> = use_signal(String::new);
    let mut description: Signal<String> = use_signal(String::new);
    let mut tags: Signal<String> = use_signal(String::new);
    let mut body: Signal<String> = use_signal(String::new);
    let mut creating: Signal<bool> = use_signal(|| false);
    let mut create_error: Signal<Option<String>> = use_signal(|| None);

    // Pattern B: read every signal into an owned local before rsx!.
    let open_val = open();
    let name_val = name.read().clone();
    let description_val = description.read().clone();
    let tags_val = tags.read().clone();
    let body_val = body.read().clone();
    let creating_val = *creating.read();
    let create_error_val = create_error.read().clone();

    if !open_val {
        return rsx! {};
    }

    let can_create = can_create_skill(&name_val, &body_val) && !creating_val;

    let mut reset_and_close = move || {
        name.set(String::new());
        description.set(String::new());
        tags.set(String::new());
        body.set(String::new());
        create_error.set(None);
        on_close.call(());
    };

    rsx! {
        div {
            class: "kn-modal-overlay",
            onclick: move |_| reset_and_close(),
            div {
                class: "skill-wizard",
                onclick: move |e| e.stop_propagation(),
                div { class: "kn-modal-header",
                    h2 { class: "kn-modal-title", "New skill" }
                }
                div { class: "kn-modal-body",
                    label { class: "kn-modal-label", "NAME" }
                    input {
                        class: "kn-modal-input",
                        placeholder: "my-new-skill",
                        value: "{name_val}",
                        oninput: move |e| name.set(e.value()),
                    }
                    label { class: "kn-modal-label", "DESCRIPTION (optional)" }
                    input {
                        class: "kn-modal-input",
                        placeholder: "What this skill does",
                        value: "{description_val}",
                        oninput: move |e| description.set(e.value()),
                    }
                    label { class: "kn-modal-label", "TAGS (optional, comma-separated)" }
                    input {
                        class: "kn-modal-input",
                        placeholder: "research, web, automation",
                        value: "{tags_val}",
                        oninput: move |e| tags.set(e.value()),
                    }
                    label { class: "kn-modal-label", "BODY" }
                    textarea {
                        class: "kn-modal-textarea kn-modal-textarea--mono skill-body-editor",
                        placeholder: "# My New Skill\n\nDescribe what the agent should do…",
                        value: "{body_val}",
                        oninput: move |e| body.set(e.value()),
                    }
                    if let Some(ref err) = create_error_val {
                        div { class: "kn-modal-error", "{err}" }
                    }
                }
                div { class: "kn-modal-actions",
                    button {
                        class: "kn-modal-btn",
                        disabled: creating_val,
                        onclick: move |_| reset_and_close(),
                        "CANCEL"
                    }
                    button {
                        class: "kn-modal-btn kn-modal-btn--submit",
                        disabled: !can_create,
                        onclick: move |_| {
                            let name_arg = name.read().clone();
                            let description_arg = description.read().clone();
                            let tags_arg = parse_tags(&tags.read());
                            let body_arg = body.read().clone();
                            creating.set(true);
                            create_error.set(None);
                            spawn(async move {
                                match create_skill(name_arg, description_arg, tags_arg, body_arg).await {
                                    Ok(_) => {
                                        creating.set(false);
                                        name.set(String::new());
                                        description.set(String::new());
                                        tags.set(String::new());
                                        body.set(String::new());
                                        on_created.call(());
                                        on_close.call(());
                                    }
                                    Err(e) => {
                                        creating.set(false);
                                        create_error.set(Some(format!("Couldn't create skill — {e}.")));
                                    }
                                }
                            });
                        },
                        if creating_val { "CREATING…" } else { "CREATE SKILL" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod new_skill_validation_tests {
    use super::*;

    #[test]
    fn blank_form_is_disabled() {
        assert!(!can_create_skill("", ""));
    }

    #[test]
    fn name_alone_is_disabled() {
        assert!(!can_create_skill("my-skill", ""));
    }

    #[test]
    fn name_and_body_enables_create() {
        assert!(can_create_skill("my-skill", "do the thing"));
    }

    #[test]
    fn whitespace_only_name_is_disabled() {
        assert!(!can_create_skill("   ", "do the thing"));
    }

    #[test]
    fn name_that_sanitizes_to_empty_slug_is_disabled() {
        // No letters/digits at all — mirrors the server's
        // to_skill_slug "must contain at least one letter or number" rule.
        assert!(!can_create_skill("---", "do the thing"));
    }

    #[test]
    fn whitespace_only_body_is_disabled() {
        assert!(!can_create_skill("my-skill", "   "));
    }

    #[test]
    fn very_long_name_is_accepted() {
        let long_name = "a".repeat(500);
        assert!(can_create_skill(&long_name, "do the thing"));
    }

    #[test]
    fn tags_parse_trims_and_drops_empties() {
        assert_eq!(
            parse_tags("research, web,, automation , "),
            vec!["research", "web", "automation"]
        );
    }
}
