//! Phase 50.2 Plan 01 (D-21/UI-SPEC Component Inventory §5): the Create Room
//! modal — single-step `.kn-modal`, not a wizard. Composes onto the existing
//! `.kn-modal-overlay`/`.kn-modal`/`.kn-modal-header`/`.kn-modal-title`/
//! `.kn-modal-body`/`.kn-modal-label`/`.kn-modal-input`/`.kn-modal-checkbox`/
//! `.kn-modal-hint--info`/`.kn-modal-error`/`.kn-modal-actions`/
//! `.kn-modal-btn`/`.kn-modal-btn--submit` shell (`kanban.css`, loaded
//! unconditionally) — the same disabled-until-valid submit pattern and
//! `CANCEL` dismiss precedent `profile_shared/create_dialog.rs`'s wizard
//! already establishes. Every string is the UI-SPEC Copywriting Contract's
//! locked wording, verbatim.
//!
//! Member selection validity ([`member_selection_is_valid`]) and the counter
//! hint text ([`member_selection_hint`]) are pure, disk/DOM-I/O-free
//! functions so the UI-SPEC State Matrix's 0/1 (invalid) vs. 2-6 (valid)
//! behavior is directly unit-testable without a renderer. Selection counts
//! above 6 are unreachable through the UI itself — the member checkbox's
//! `onchange` handler refuses to grow the selection set past
//! `GroupChatSettings::default().max_members` — the "disabled-until-valid"
//! pattern the plan calls for.
//!
//! Signal-borrow discipline (clippy.toml): every value read from a signal is
//! extracted BEFORE the `rsx!` block; the write-lock taken to toggle a
//! member checkbox is synchronous and released well before any `spawn`; the
//! submit handler's write-lock (`creating`/`error`) is only ever taken
//! fresh, after `create_group_room(...).await` resolves — never held across
//! the `.await` itself (mirrors `bot_roster/chat_window.rs`'s `submit_send`
//! and `profile_shared/create_dialog.rs`'s `submit_create`).

use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::{BotRosterEntry, CreateGroupRoomRequest, GroupChatSettings};
use crate::server::group_chat_api::create_group_room;
use dioxus::prelude::*;
use std::collections::BTreeSet;

/// Phase 50.2 Plan 01 (UI-SPEC E5): `true` for a selection count within
/// `[GroupChatSettings::default().min_members, ...max_members]` — 2..=6 for
/// this plan's shipped defaults. Pure, disk/DOM-I/O-free.
//
// Under `--all-features`, `legacy-shell` swaps the reachable root component
// to `WarpHermes`, leaving `HermesApp` (and therefore `BotRoster`,
// `CreateRoomModal`, and this whole module) unreferenced from `main()` for
// dead-code-lint purposes — even though this is live in the default
// (non-legacy-shell) build. Same pre-existing pattern as
// `bot_roster/delete_confirm.rs`'s `confirm_input_matches`.
#[allow(dead_code)]
pub(crate) fn member_selection_is_valid(count: usize) -> bool {
    let settings = GroupChatSettings::default();
    count >= settings.min_members as usize && count <= settings.max_members as usize
}

/// Phase 50.2 Plan 01 (UI-SPEC Copywriting Contract): the live counter hint
/// below the member checklist — `{n} of 6 selected — need at least 2` below
/// the minimum, `{n} of 6 selected` once valid. Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see member_selection_is_valid's doc note above
pub(crate) fn member_selection_hint(count: usize) -> String {
    let max = GroupChatSettings::default().max_members;
    if member_selection_is_valid(count) {
        format!("{count} of {max} selected")
    } else {
        format!("{count} of {max} selected — need at least 2")
    }
}

/// Phase 50.2 Plan 17 (G-50.2-2a): the shared selection ceiling — extracted
/// from this file's own inline `GroupChatSettings::default().max_members`
/// read so `EditMembersModal` has a bound source that is not a second
/// `GroupChatSettings` read. `CreateRoomModal`'s toggle guard below now
/// calls this fn instead of reading the settings record inline; the
/// observable behavior is unchanged (same default value, same source of
/// truth). Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see member_selection_is_valid's doc note above
pub(crate) fn member_selection_max() -> usize {
    GroupChatSettings::default().max_members as usize
}

/// Phase 50.2 Plan 01: the Create Room modal. `roster_entries` is the SAME
/// joined roster the caller (`BotRoster`) already computed — this component
/// never issues its own `list_profiles`/`list_bot_meta` fetch. Only
/// non-live bots are offered as room members (`entry.is_live` filters the
/// live profile out of the checklist) — a room whose members list somehow
/// contained the live profile would still be safely handled server-side by
/// `run_bot_handoff`'s own D-04 guard, but the picker never offers it as a
/// choice in the first place.
#[component]
pub fn CreateRoomModal(
    roster_entries: Vec<BotRosterEntry>,
    on_close: EventHandler<()>,
    on_created: EventHandler<String>,
) -> Element {
    let mut room_name: Signal<String> = use_signal(String::new);
    let mut selected: Signal<BTreeSet<String>> = use_signal(BTreeSet::new);
    let mut creating: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // ---- Derived values (read BEFORE rsx!, clippy.toml discipline). ----
    let name_val = room_name.read().clone();
    let selected_val = selected.read().clone();
    let selected_count = selected_val.len();
    let is_creating = *creating.read();
    let can_submit = member_selection_is_valid(selected_count) && !is_creating;
    let hint = member_selection_hint(selected_count);
    let error_val = error.read().clone();
    let max_members = member_selection_max();

    let submit = move |_| {
        if !can_submit {
            return;
        }
        let name = room_name.read().clone();
        let members: Vec<String> = selected.read().iter().cloned().collect();
        creating.set(true);
        error.set(None);
        let mut creating_sig = creating;
        let mut error_sig = error;
        spawn(async move {
            let result = create_group_room(CreateGroupRoomRequest { name, members }).await;
            // Fresh write-lock, acquired only after the await resolves.
            creating_sig.set(false);
            match result {
                Ok(room) => on_created.call(room.id),
                Err(_) => error_sig.set(Some(
                    "Could not create the room. Check your selection and try again.".to_string(),
                )),
            }
        });
    };

    rsx! {
        div { class: "kn-modal-overlay", role: "presentation",
            div {
                class: "kn-modal",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "kn-create-room-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !is_creating {
                        on_close.call(());
                    }
                },
                div { class: "kn-modal-header",
                    h3 { class: "kn-modal-title", id: "kn-create-room-title", "Create Room" }
                }
                div { class: "kn-modal-body",
                    label { class: "kn-modal-label", "ROOM NAME" }
                    input {
                        class: "kn-modal-input",
                        disabled: is_creating,
                        value: "{name_val}",
                        oninput: move |evt| room_name.set(evt.value()),
                    }
                    label { class: "kn-modal-label", "MEMBERS (2–6)" }
                    div { class: "kn-room-member-list",
                        for entry in roster_entries.iter().filter(|e| !e.is_live).cloned() {
                            {
                                let bot_name = entry.row.name.clone();
                                let is_selected = selected_val.contains(&bot_name);
                                let avatar = entry.meta.as_ref().and_then(|m| m.avatar.clone());
                                let avatar_shape = avatar.as_ref().and_then(|a| a.shape.clone());
                                let avatar_color = avatar.as_ref().and_then(|a| a.color.clone());
                                let avatar_image_id = avatar.as_ref().and_then(|a| a.image_id.clone());
                                let bot_name_for_toggle = bot_name.clone();
                                rsx! {
                                    label { class: "kn-modal-checkbox", key: "{bot_name}",
                                        input {
                                            r#type: "checkbox",
                                            disabled: is_creating,
                                            checked: is_selected,
                                            onchange: move |_| {
                                                let mut set = selected.write();
                                                if set.contains(&bot_name_for_toggle) {
                                                    set.remove(&bot_name_for_toggle);
                                                } else if set.len() < max_members {
                                                    set.insert(bot_name_for_toggle.clone());
                                                }
                                            },
                                        }
                                        BotFace {
                                            name: bot_name.clone(),
                                            size: 20u32,
                                            shape: avatar_shape,
                                            color_token: avatar_color,
                                            image_id: avatar_image_id,
                                        }
                                        span { class: "kn-room-member-row", "{bot_name}" }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "kn-modal-hint--info", "{hint}" }
                    if let Some(err) = error_val {
                        div { class: "kn-modal-error", "{err}" }
                    }
                }
                div { class: "kn-modal-actions",
                    button {
                        class: "kn-modal-btn",
                        disabled: is_creating,
                        onclick: move |_| on_close.call(()),
                        "CANCEL"
                    }
                    button {
                        class: "kn-modal-btn kn-modal-btn--submit",
                        disabled: !can_submit,
                        onclick: submit,
                        if is_creating { "CREATING…" } else { "CREATE ROOM" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // member_selection_is_valid
    // -------------------------------------------------------------------

    #[test]
    fn member_selection_is_valid_rejects_zero() {
        assert!(!member_selection_is_valid(0));
    }

    #[test]
    fn member_selection_is_valid_rejects_one() {
        assert!(!member_selection_is_valid(1));
    }

    #[test]
    fn member_selection_is_valid_accepts_two() {
        assert!(member_selection_is_valid(2));
    }

    #[test]
    fn member_selection_is_valid_accepts_six() {
        assert!(member_selection_is_valid(6));
    }

    #[test]
    fn member_selection_is_valid_rejects_seven() {
        assert!(!member_selection_is_valid(7));
    }

    // -------------------------------------------------------------------
    // member_selection_hint — both Copywriting Contract strings
    // -------------------------------------------------------------------

    #[test]
    fn member_selection_hint_below_minimum_states_the_constraint() {
        assert_eq!(member_selection_hint(0), "0 of 6 selected — need at least 2");
        assert_eq!(member_selection_hint(1), "1 of 6 selected — need at least 2");
    }

    #[test]
    fn member_selection_hint_valid_selection_has_no_constraint_suffix() {
        assert_eq!(member_selection_hint(2), "2 of 6 selected");
        assert_eq!(member_selection_hint(6), "6 of 6 selected");
    }

    // -------------------------------------------------------------------
    // member_selection_max — Phase 50.2 Plan 17 (G-50.2-2a): the extracted
    // bound source. Asserting it equals the settings default's maximum
    // means a future settings-default change cannot leave this constant
    // stale.
    // -------------------------------------------------------------------

    #[test]
    fn member_selection_max_equals_the_settings_defaults_maximum() {
        assert_eq!(
            member_selection_max(),
            GroupChatSettings::default().max_members as usize
        );
    }
}
