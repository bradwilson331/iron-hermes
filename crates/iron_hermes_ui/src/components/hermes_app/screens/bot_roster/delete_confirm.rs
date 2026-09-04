//! Phase 50.1 Plan 06 (D-18): the typed destructive confirmation modal for
//! Delete Bot.
//!
//! Supersedes the canonical plugin's own click-to-confirm `ConfirmDialog` —
//! D-18 requires a typed exact-name confirmation, not a single extra click.
//! Built on the crate's existing `.kn-modal-overlay`/`.kn-modal` shell
//! (`kanban.css`) — the same shell `CreateProfileWizard`'s `.kn-wizard`
//! variant composes onto, not a new modal mechanism.
//!
//! State machine (UI-SPEC E6 / State Matrix): an empty or non-matching
//! typed value leaves the confirm button disabled with no error styling —
//! this is a "not yet typed" state, not a validation failure. A matching
//! value enables the danger-tinted confirm button. A delete in flight
//! disables both buttons and shows `DELETING…`. A server-side failure
//! renders a `.kn-modal-error` row above the footer, keeps the modal open,
//! and re-enables the confirm button — symmetric with this phase's other
//! failure treatments (upload, generate, duplicate, handoff).
//!
//! Every string here is the Copywriting Contract's locked wording,
//! verbatim.
//!
//! Signal-borrow discipline (clippy.toml): every value read from a signal
//! is extracted BEFORE the `rsx!` block via `.read()`; `.peek()` is used
//! only inside the confirm handler's spawned closure, never across an
//! `.await`.

use crate::server::profile_api::archive_profile;
use dioxus::prelude::*;

/// Phase 50.1 Plan 06 (UI-SPEC E6 partial): whether the typed confirmation
/// input matches the target bot's name exactly. Pure and disk/DOM-I/O-free
/// so the "not yet typed" vs. "typed and wrong" vs. "matches" state is
/// directly unit-testable without a renderer.
// Under `--all-features`, `legacy-shell` swaps the reachable root component
// to `WarpHermes`, which the native dead-code lint sees as making this fn
// unreferenced from `main()` — even though it is live in the default
// (non-legacy-shell) `HermesApp` build. Same pattern as `NO_CONVERSATIONS_YET`
// in `bot_roster/card.rs` and the rest of this phase's own precedent.
#[allow(dead_code)]
pub(crate) fn confirm_input_matches(bot_name: &str, typed: &str) -> bool {
    !typed.is_empty() && typed == bot_name
}

/// Phase 50.1 Plan 06 (D-18): the typed-confirm modal. Renders unconditionally
/// mounted at the call site — the caller decides when to mount it (only ever
/// for a non-default-profile bot; the default profile's `DELETE BOT` action
/// renders disabled and never opens this component at all).
#[component]
pub fn DeleteBotConfirm(bot_name: String, on_dismiss: EventHandler<()>, on_deleted: EventHandler<()>) -> Element {
    let mut typed_name: Signal<String> = use_signal(String::new);
    let mut deleting: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let typed_val = typed_name.read().clone();
    let is_deleting = *deleting.read();
    let matches = confirm_input_matches(&bot_name, &typed_val);
    let can_confirm = matches && !is_deleting;

    let title = format!("Delete {bot_name}?");
    // Phase 49.4 Plan 08 (D-18): the backend now ARCHIVES rather than
    // permanently removes — `profile_api::delete_profile` was renamed to
    // `archive_profile` and rewritten as a guarded move. This copy is
    // corrected to match the UI-SPEC Copywriting Contract's locked
    // "Destructive confirmation" row, which this modal previously
    // contradicted ("This cannot be undone" was no longer true).
    let body = format!(
        "Type the profile name to confirm. This archives the bot \"{bot_name}\" — its config, memory, sessions, skills, .env, and logs are preserved, not deleted."
    );
    let placeholder = format!("Type \"{bot_name}\" to confirm");

    let on_confirm = move |_| {
        if !can_confirm {
            return;
        }
        let name_to_delete = bot_name.clone();
        deleting.set(true);
        error.set(None);
        let mut deleting_sig = deleting;
        let mut error_sig = error;
        spawn(async move {
            match archive_profile(name_to_delete).await {
                Ok(()) => {
                    deleting_sig.set(false);
                    on_deleted.call(());
                }
                Err(e) => {
                    deleting_sig.set(false);
                    error_sig.set(Some(format!("{e}")));
                }
            }
        });
    };

    rsx! {
        div {
            class: "kn-modal-overlay",
            role: "presentation",
            div {
                class: "kn-modal kn-confirm-delete",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "kn-confirm-delete-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !is_deleting {
                        on_dismiss.call(());
                    }
                },
                div { class: "kn-modal-header",
                    h3 {
                        class: "kn-modal-title",
                        id: "kn-confirm-delete-title",
                        style: "color: var(--danger);",
                        "{title}"
                    }
                }
                div { class: "kn-modal-body",
                    div {
                        class: "kn-modal-error",
                        style: "background: color-mix(in srgb, var(--danger) 10%, transparent);",
                        "{body}"
                    }
                    input {
                        class: "kn-modal-input",
                        placeholder: "{placeholder}",
                        disabled: is_deleting,
                        value: "{typed_val}",
                        oninput: move |evt| typed_name.set(evt.value()),
                    }
                    if let Some(err) = error.read().clone() {
                        div { class: "kn-modal-error", "{err}" }
                    }
                }
                div { class: "kn-modal-actions",
                    button {
                        class: "kn-modal-btn",
                        disabled: is_deleting,
                        onclick: move |_| on_dismiss.call(()),
                        "CANCEL"
                    }
                    button {
                        class: "kn-modal-btn kn-modal-btn--danger",
                        disabled: !can_confirm,
                        onclick: on_confirm,
                        if is_deleting { "DELETING…" } else { "DELETE BOT" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_input_matches_empty_input_does_not_match() {
        assert!(!confirm_input_matches("scout", ""));
    }

    #[test]
    fn confirm_input_matches_partial_typed_does_not_match() {
        assert!(!confirm_input_matches("scout", "sco"));
    }

    #[test]
    fn confirm_input_matches_wrong_name_does_not_match() {
        assert!(!confirm_input_matches("scout", "ranger"));
    }

    #[test]
    fn confirm_input_matches_exact_name_matches() {
        assert!(confirm_input_matches("scout", "scout"));
    }

    #[test]
    fn confirm_input_matches_is_case_sensitive() {
        assert!(!confirm_input_matches("scout", "Scout"));
    }
}
