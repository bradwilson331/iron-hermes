//! Phase 52 Plan 09 (D-02, UI-SPEC Surface Contract 6): New Conversation's
//! confirmation modal — reuses `delete_room_confirm.rs`'s light two-button
//! `.kn-modal` shell (title + body + `CANCEL` / affirmative button) and its
//! in-flight/failure state machine.
//!
//! **Own blast-radius argument (distinct from `delete_room_confirm.rs`'s
//! own).** This action resets conversational continuity only. It touches
//! no credential, no room record beyond one integer
//! (`GroupRoom.conversation_epoch`), and no transcript content — the
//! transcript is APPENDED to (one `System` marker row), never rewritten or
//! deleted. That is why this modal takes the SAME light gate room
//! deletion uses rather than D-18's typed-name confirmation gate (reserved
//! for destroying a whole bot profile — this destroys nothing), and
//! equally why its affirmative button is NOT danger-tinted the way
//! `DELETE ROOM` is: painting a state reset red would overstate it exactly
//! as much as the typed gate would.
//!
//! **The pending treatment, described accurately (UI-SPEC E6 backstop,
//! corrected by Round 1 codex and re-synced across both this plan and
//! Plan 05's own SUMMARY at the plan-checker's iteration-1 pass).** The
//! action is an asynchronous `#[server]` round trip over the network
//! boundary. Its handler (`reset_room_conversation_impl`) takes the room's
//! drive slot (`handoff_steering::try_acquire_room_drive`) and then
//! `GROUP_CHAT_LOCK`, and performs two atomic file writes on a
//! `spawn_blocking` thread — a blocking THREAD, not a subprocess, unlike a
//! bot dispatch. It spawns NO child process. It also rewrites NO child
//! session: it bumps one integer, and the child-side break (each member's
//! own CLI subprocess resuming a DIFFERENT session) is a DERIVED
//! consequence, computed at the NEXT dispatch, when
//! `group_chat_api::group_session_title` renders a different title from
//! the newly persisted epoch. The pending treatment below is warranted by
//! the round trip and the two locks, not by subprocess work and not by any
//! child-session mutation.
//!
//! **Three distinguishable failure outcomes, not one (Round 1 codex
//! MEDIUM).** The reset's persistence is two separate writes — the
//! transcript (authoritative) and the roster's index projection — so a
//! single generic failure treatment would invite a retry after a reset
//! that had already landed, bumping the epoch a second time and orphaning
//! another child session for nothing. [`reset_outcome_treatment`] maps the
//! server error's text into two arms:
//! - **Retryable** — a refused-while-busy reset
//!   (`GroupChatError::ResetRefusedDriveInFlight`) or any store I/O error
//!   raised BEFORE the transcript write. Genuine no-ops. Treated exactly
//!   like `DeleteRoomConfirm`'s own failure path — a `.kn-modal-error` row,
//!   modal stays open, both buttons re-enable. No reset-specific wording is
//!   invented for these.
//! - **Landed** — `GroupChatError::IndexProjectionStale`. The transcript
//!   write SUCCEEDED; only the roster's own index projection failed. The
//!   conversation HAS been reset. This modal transitions to a
//!   `.kn-modal-hint--info` notice and runs the caller's reload
//!   immediately — the alternative is a modal that tells the operator a
//!   completed action failed, which is worse than telling them nothing.
//!
//! The `#[server]` boundary (`ServerFnError`) erases the structured
//! `GroupChatError` into its `Display` text before this always-client-
//! reachable component ever sees it (`GroupChatError` itself is
//! `#[cfg(feature = "server")]`, `pub(crate)`, and not constructible from
//! here) — so [`reset_outcome_treatment`] matches on that text rather than
//! on the enum. Its own unit tests, gated `#[cfg(feature = "server")]`,
//! build the REAL `GroupChatError` variants and assert against their own
//! `Display` output, so the two representations can never silently drift
//! apart.
//!
//! Signal-borrow discipline (clippy.toml): every value read from a signal
//! is extracted BEFORE the `rsx!` block; the confirm handler's fresh
//! `.set()` calls only happen AFTER the
//! `reset_group_room_conversation(...).await` resolves — never a
//! write-lock guard held across it.

use crate::server::group_chat_api::reset_group_room_conversation;
use dioxus::prelude::*;

/// Phase 52 Plan 09 (UI-SPEC Copywriting Contract, verbatim): the confirm
/// modal's title. Pure and disk/DOM-I/O-free so the exact quoting is
/// directly unit-testable without a renderer. Bounded by the room-name cap
/// alone — the room name is this surface's only variable-length input
/// (`validate_group_room_name`'s 64-character bound).
#[allow(dead_code)] // see delete_room_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
pub(crate) fn new_conversation_confirm_title(room_name: &str) -> String {
    format!("Start a new conversation in \"{room_name}\"?")
}

/// Phase 52 Plan 09 (UI-SPEC Copywriting Contract, verbatim): the confirm
/// modal's body — a fixed sentence pair, unchanged by the room name (the
/// room name interpolates only into the title above, never here).
#[allow(dead_code)] // see delete_room_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
pub(crate) fn new_conversation_confirm_body() -> &'static str {
    "The bots in this room — including any team leader and workers — won't recall anything said before this point. This does not delete the room or its history."
}

/// Phase 52 Plan 09: the affirmative button's class list — the crate's
/// established non-danger submit shape (`create_room_modal.rs`,
/// `edit_members_modal.rs`, `group_settings.rs`), never the danger-tinted
/// class room deletion applies to its own affirmative button. This is a
/// state reset, not a data-destroying action.
#[allow(dead_code)] // see delete_room_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
pub(crate) const NEW_CONVERSATION_CONFIRM_SUBMIT_CLASS: &str = "kn-modal-btn kn-modal-btn--submit";

/// Phase 52 Plan 09 (UI-SPEC Copywriting Contract, verbatim): the two
/// button labels.
#[allow(dead_code)] // see delete_room_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
pub(crate) const NEW_CONVERSATION_CONFIRM_CANCEL_LABEL: &str = "CANCEL";
#[allow(dead_code)] // see delete_room_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
pub(crate) const NEW_CONVERSATION_CONFIRM_SUBMIT_LABEL: &str = "START NEW CONVERSATION";

/// Phase 52 Plan 09 (Round 1 codex MEDIUM): the two treatments a failed
/// reset can receive — see this module's own doc comment for the full
/// rationale.
#[allow(dead_code)] // see delete_room_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResetTreatment {
    /// Nothing was written. Safe, correct to retry.
    Retryable,
    /// The transcript write already landed; a retry would bump the epoch
    /// a second time and orphan another child session for nothing.
    Landed,
}

/// Phase 52 Plan 09 (Round 1 codex MEDIUM): maps a reset failure's
/// `Display` text to its [`ResetTreatment`]. `IndexProjectionStale` is the
/// ONLY outcome where the reset has already landed ("the conversation
/// reset landed" is unique to its `Display` text in `group_chat_store.rs`)
/// — every other outcome (`ResetRefusedDriveInFlight`, a pre-write
/// `StoreIo`, `ConversationEpochExhausted`) mutated nothing, so they fall
/// through to the safe `Retryable` default rather than being enumerated
/// one by one.
#[allow(dead_code)] // see delete_room_confirm.rs's own reachability note (legacy-shell) for this crate's precedent
pub(crate) fn reset_outcome_treatment(message: &str) -> ResetTreatment {
    if message.contains("reset landed") {
        ResetTreatment::Landed
    } else {
        ResetTreatment::Retryable
    }
}

/// Phase 52 Plan 09 (D-02): the "New conversation" confirm modal. Mounted
/// conditionally by the room workspace's overflow control (mirrors
/// `DeleteRoomConfirm`'s own mount idiom) — the caller decides when to
/// mount it and supplies the target room's id/name; `on_reset` is expected
/// to trigger the same transcript reload the workspace's other room
/// mutations already use, so the appended marker row is visible without a
/// page reload.
#[component]
pub fn NewConversationConfirm(
    room_id: String,
    room_name: String,
    on_dismiss: EventHandler<()>,
    on_reset: EventHandler<()>,
) -> Element {
    let mut resetting: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    // Phase 52 Plan 09 (Round 1 codex MEDIUM): the Landed arm's own
    // state — once true, the modal stops asking a question (the reset
    // already happened) and shows a dismissible informational notice
    // instead of a modal that tells the operator a completed action
    // failed.
    let landed: Signal<bool> = use_signal(|| false);

    let is_resetting = *resetting.read();
    let error_val = error.read().clone();
    let is_landed = *landed.read();

    let title = new_conversation_confirm_title(&room_name);
    let body = new_conversation_confirm_body();

    let on_confirm = move |_| {
        if is_resetting {
            return;
        }
        let room_id_to_reset = room_id.clone();
        resetting.set(true);
        error.set(None);
        let mut resetting_sig = resetting;
        let mut error_sig = error;
        let mut landed_sig = landed;
        spawn(async move {
            match reset_group_room_conversation(room_id_to_reset).await {
                Ok(_room) => {
                    // Fresh `.set()` calls only, acquired after the await
                    // resolves — never a write-lock guard held across it.
                    resetting_sig.set(false);
                    on_reset.call(());
                }
                Err(e) => match reset_outcome_treatment(&e.to_string()) {
                    ResetTreatment::Retryable => {
                        resetting_sig.set(false);
                        error_sig.set(Some(
                            "Could not start a new conversation. Try again.".to_string(),
                        ));
                    }
                    ResetTreatment::Landed => {
                        // The reset HAS landed — retrying here would bump
                        // the epoch a second time and orphan another
                        // child session for nothing. Run the caller's
                        // reload now; the notice below is what tells the
                        // operator this succeeded rather than failed.
                        resetting_sig.set(false);
                        landed_sig.set(true);
                        on_reset.call(());
                    }
                },
            }
        });
    };

    rsx! {
        div {
            class: "kn-modal-overlay",
            role: "presentation",
            div {
                class: "kn-modal",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "kn-new-conversation-confirm-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !is_resetting {
                        on_dismiss.call(());
                    }
                },
                div { class: "kn-modal-header",
                    h3 {
                        class: "kn-modal-title",
                        id: "kn-new-conversation-confirm-title",
                        "{title}"
                    }
                }
                div { class: "kn-modal-body",
                    div { "{body}" }
                    if is_landed {
                        div {
                            class: "kn-modal-hint--info",
                            "The reset already landed — the room list may show stale details until the next reload."
                        }
                    } else if let Some(err) = error_val {
                        div { class: "kn-modal-error", "{err}" }
                    }
                }
                div { class: "kn-modal-actions",
                    if is_landed {
                        button {
                            class: NEW_CONVERSATION_CONFIRM_SUBMIT_CLASS,
                            onclick: move |_| on_dismiss.call(()),
                            "OK"
                        }
                    } else {
                        button {
                            class: "kn-modal-btn",
                            disabled: is_resetting,
                            onclick: move |_| on_dismiss.call(()),
                            "{NEW_CONVERSATION_CONFIRM_CANCEL_LABEL}"
                        }
                        button {
                            class: NEW_CONVERSATION_CONFIRM_SUBMIT_CLASS,
                            disabled: is_resetting,
                            onclick: on_confirm,
                            if is_resetting { "STARTING…" } else { "{NEW_CONVERSATION_CONFIRM_SUBMIT_LABEL}" }
                        }
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
    fn new_conversation_confirm_body_interpolates_the_room_name() {
        let title = new_conversation_confirm_title("standup");
        assert_eq!(title, "Start a new conversation in \"standup\"?");
        // The body is the fixed contracted sentence pair, unchanged by the
        // room name — it takes no room-name parameter at all.
        assert_eq!(
            new_conversation_confirm_body(),
            "The bots in this room — including any team leader and workers — won't recall anything said before this point. This does not delete the room or its history."
        );
    }

    #[test]
    fn new_conversation_confirm_body_is_bounded_by_the_room_name_cap() {
        // The 64-character room-name cap (`validate_group_room_name`) is
        // the only variable-length input to this surface, so the composed
        // title's length is bounded by it plus the fixed wrapper text.
        let longest_room_name = "a".repeat(64);
        let title = new_conversation_confirm_title(&longest_room_name);
        assert_eq!(title.len(), "Start a new conversation in \"\"?".len() + 64);
    }

    #[test]
    fn new_conversation_confirm_affirmative_button_is_not_danger_tinted() {
        assert!(NEW_CONVERSATION_CONFIRM_SUBMIT_CLASS.contains("kn-modal-btn"));
        assert!(!NEW_CONVERSATION_CONFIRM_SUBMIT_CLASS.contains("danger"));
    }

    #[test]
    fn new_conversation_confirm_labels_match_the_copywriting_contract() {
        assert_eq!(NEW_CONVERSATION_CONFIRM_CANCEL_LABEL, "CANCEL");
        assert_eq!(NEW_CONVERSATION_CONFIRM_SUBMIT_LABEL, "START NEW CONVERSATION");
    }
}

#[cfg(feature = "server")]
#[cfg(test)]
mod reset_outcome_treatment_tests {
    use super::*;
    use crate::server::group_chat_store::GroupChatError;

    #[test]
    fn reset_outcome_treatment_offers_retry_for_a_refused_or_pre_write_failure() {
        let refused = GroupChatError::ResetRefusedDriveInFlight.to_string();
        let io = GroupChatError::StoreIo {
            reason: "disk full".to_string(),
        }
        .to_string();
        assert_eq!(reset_outcome_treatment(&refused), ResetTreatment::Retryable);
        assert_eq!(reset_outcome_treatment(&io), ResetTreatment::Retryable);
    }

    #[test]
    fn reset_outcome_treatment_does_not_offer_retry_for_a_landed_reset() {
        let stale = GroupChatError::IndexProjectionStale.to_string();
        assert_eq!(reset_outcome_treatment(&stale), ResetTreatment::Landed);
    }
}
