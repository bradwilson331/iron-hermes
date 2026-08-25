//! Phase 50.2 Plan 06 (UI-SPEC Component Inventory §8): Delete Room's
//! confirmation modal — reuses 50.1's `.kn-confirm-delete` shell
//! (`delete_confirm.rs`'s title/body/footer layout, its in-flight and
//! failure handling) WITHOUT the typed-name confirmation gate.
//!
//! **Deliberate divergence from D-18 (rationale, not an omission).** D-18's
//! typed-confirm bar exists because deleting a BOT destroys a whole
//! profile — its config, memory, sessions, skills and credential-adjacent
//! state. Deleting a ROOM destroys only a group-chat transcript and its
//! membership record; no credential, config or per-profile data is
//! touched at all. Applying D-18's heavier gate here would overstate this
//! action's actual blast radius, so this modal is a plain two-button
//! `.kn-modal` (`CANCEL` / danger-tinted `DELETE ROOM`) — the body copy
//! below carries that same distinction to the operator in plain language
//! rather than leaving it implicit.
//!
//! State machine (UI-SPEC E8 / State Matrix), mirroring `delete_confirm.rs`
//! exactly: a delete in flight disables both buttons and shows
//! `DELETING…`; a server-side failure renders a `.kn-modal-error` row
//! above the footer, keeps the modal open, and re-enables the buttons.
//!
//! Signal-borrow discipline (clippy.toml): every value read from a signal
//! is extracted BEFORE the `rsx!` block; the confirm handler's fresh
//! `.set()` calls only happen AFTER the `delete_group_room(...).await`
//! resolves — never a write-lock guard held across it.

use crate::server::group_chat_api::delete_group_room;
use dioxus::prelude::*;

/// Phase 50.2 Plan 06 (UI-SPEC Copywriting Contract, verbatim): the confirm
/// modal's title. Pure and disk/DOM-I/O-free so the exact quoting is
/// directly unit-testable without a renderer.
#[allow(dead_code)] // see delete_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
pub(crate) fn delete_room_confirm_title(room_name: &str) -> String {
    format!("Delete room \"{room_name}\"?")
}

/// Phase 50.2 Plan 06 (UI-SPEC Copywriting Contract, verbatim): the confirm
/// modal's body — states the blast-radius distinction from Delete Bot
/// explicitly, in the operator-facing copy, not only in this module's own
/// doc comment.
#[allow(dead_code)] // see delete_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
pub(crate) fn delete_room_confirm_body(room_name: &str) -> String {
    format!(
        "This permanently deletes the room \"{room_name}\" and its conversation history. Members are unaffected — this does not delete any bot or its profile. This cannot be undone."
    )
}

/// Phase 50.2 Plan 06: Delete Room's confirmation modal. Mounted
/// conditionally by the room workspace's overflow control — the caller
/// (`group_chat_workspace.rs`) decides when to mount it and supplies the
/// target room's id/name; `on_deleted` is expected to close this modal AND
/// return to the roster grid (the caller's own `on_back` already does
/// both, per its existing shape).
#[component]
pub fn DeleteRoomConfirm(
    room_id: String,
    room_name: String,
    on_dismiss: EventHandler<()>,
    on_deleted: EventHandler<()>,
) -> Element {
    let mut deleting: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let is_deleting = *deleting.read();
    let error_val = error.read().clone();

    let title = delete_room_confirm_title(&room_name);
    let body = delete_room_confirm_body(&room_name);

    let on_confirm = move |_| {
        if is_deleting {
            return;
        }
        let room_id_to_delete = room_id.clone();
        deleting.set(true);
        error.set(None);
        let mut deleting_sig = deleting;
        let mut error_sig = error;
        spawn(async move {
            match delete_group_room(room_id_to_delete).await {
                Ok(()) => {
                    // Fresh `.set()` calls only, acquired after the await
                    // resolves — never a write-lock guard held across it.
                    deleting_sig.set(false);
                    on_deleted.call(());
                }
                Err(_e) => {
                    // Phase 50.2 Plan 06 (UI-SPEC Copywriting Contract,
                    // verbatim): the failure copy is the LOCKED generic
                    // string, never the raw server error — same precedent
                    // `create_room_modal.rs`'s own failure path already
                    // established for this room-workspace surface.
                    deleting_sig.set(false);
                    error_sig.set(Some(
                        "Could not delete the room. Try again.".to_string(),
                    ));
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
                "aria-labelledby": "kn-confirm-delete-room-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !is_deleting {
                        on_dismiss.call(());
                    }
                },
                div { class: "kn-modal-header",
                    h3 {
                        class: "kn-modal-title",
                        id: "kn-confirm-delete-room-title",
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
                    if let Some(err) = error_val {
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
                        disabled: is_deleting,
                        onclick: on_confirm,
                        if is_deleting { "DELETING…" } else { "DELETE ROOM" }
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
    fn delete_room_confirm_title_matches_the_locked_copy() {
        assert_eq!(
            delete_room_confirm_title("standup"),
            "Delete room \"standup\"?"
        );
    }

    #[test]
    fn delete_room_confirm_title_uses_the_room_name_verbatim() {
        assert_eq!(
            delete_room_confirm_title("ops-triage"),
            "Delete room \"ops-triage\"?"
        );
    }

    #[test]
    fn delete_room_confirm_body_contains_the_blast_radius_distinction() {
        let body = delete_room_confirm_body("standup");
        assert!(body.contains("Members are unaffected"));
        assert!(body.contains("This cannot be undone."));
        assert!(body.contains("standup"));
    }
}
