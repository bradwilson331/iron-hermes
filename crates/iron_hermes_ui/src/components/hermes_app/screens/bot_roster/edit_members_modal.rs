//! Phase 50.2 Plan 17 (G-50.2-2a): the `Edit members` modal — UI-SPEC §6's
//! header overflow has specced `Edit members` / `Delete room` since the
//! phase began (50.2-UI-SPEC.md:316); Plan 06 shipped only `Delete room`
//! ("Edit members is out of this plan's scope",
//! `group_chat_workspace.rs:611-613` before this plan) and no later plan
//! picked it up. This module closes that drop.
//!
//! Composes onto the existing `.kn-modal-overlay`/`.kn-modal`/
//! `.kn-modal-header`/`.kn-modal-title`/`.kn-modal-body`/`.kn-modal-label`/
//! `.kn-room-member-list`/`.kn-modal-checkbox`/`.kn-room-member-row`/
//! `.kn-modal-hint--info`/`.kn-modal-error`/`.kn-modal-actions`/
//! `.kn-modal-btn`/`.kn-modal-btn--submit` shell `create_room_modal.rs`
//! already uses — no new modal shell class is introduced here.
//!
//! **One validator, two modals.** This module imports
//! [`crate::components::hermes_app::screens::bot_roster::create_room_modal::member_selection_is_valid`],
//! `member_selection_hint` and `member_selection_max` rather than
//! re-deriving them — there is exactly one `[2, 6]` bound source and one
//! counter-hint string in this crate, and `CreateRoomModal` and
//! `EditMembersModal` both read it.
//!
//! **No context provider (D-10).** `edit_members_open` is a plain
//! `use_signal` owned by `GroupChatWorkspace`, threaded down as a prop —
//! this module opens no context of its own.

use crate::components::hermes_app::screens::bot_roster::create_room_modal::{
    member_selection_hint, member_selection_is_valid, member_selection_max,
};
use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::{BotRosterEntry, UpdateGroupRoomMembersRequest};
use crate::server::group_members_api::update_group_room_members;
use dioxus::prelude::*;
use std::collections::BTreeSet;

/// Phase 50.2 Plan 17: the room's current membership as the initially
/// checked set. Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn edit_members_selection_seed(current_members: &[String]) -> BTreeSet<String> {
    current_members.iter().cloned().collect()
}

/// Phase 50.2 Plan 17: every offered row — each non-live roster bot's name
/// in roster order, then any current member not already present, appended
/// in current-membership order with no duplicates. The tail is what
/// guarantees a current member that has since become the live profile (or
/// otherwise dropped out of the offered roster) is still visible and
/// removable rather than silently vanishing from the picker, and therefore
/// from the save. Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn edit_members_pick_list(
    current_members: &[String],
    roster_entries: &[BotRosterEntry],
) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut list: Vec<String> = Vec::new();
    for entry in roster_entries.iter().filter(|e| !e.is_live) {
        let name = entry.row.name.clone();
        if seen.insert(name.clone()) {
            list.push(name);
        }
    }
    for member in current_members {
        if seen.insert(member.clone()) {
            list.push(member.clone());
        }
    }
    list
}

/// Phase 50.2 Plan 17 (G-50.2-2a): the `Edit members` modal —
/// `CreateRoomModal`'s picker, pre-seeded with the room's current
/// membership, reused element-for-element. `roster_entries` is the SAME
/// joined roster the caller (`GroupChatWorkspace`, threaded down from
/// `BotRoster`) already computed — this component issues no fetch of its
/// own.
#[component]
pub fn EditMembersModal(
    room_id: String,
    room_name: String,
    current_members: Vec<String>,
    roster_entries: Vec<BotRosterEntry>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let pick_list = edit_members_pick_list(&current_members, &roster_entries);
    let mut selected: Signal<BTreeSet<String>> =
        use_signal(|| edit_members_selection_seed(&current_members));
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    // ---- Derived values (read BEFORE rsx!, clippy.toml discipline). ----
    let selected_val = selected.read().clone();
    let selected_count = selected_val.len();
    let is_saving = *saving.read();
    let can_submit = member_selection_is_valid(selected_count) && !is_saving;
    let hint = member_selection_hint(selected_count);
    let error_val = error.read().clone();
    let max_members = member_selection_max();
    let title = format!("Edit members — \"{room_name}\"");

    let submit = move |_| {
        if !can_submit {
            return;
        }
        let room_id_to_save = room_id.clone();
        let members: Vec<String> = selected.read().iter().cloned().collect();
        saving.set(true);
        error.set(None);
        let mut saving_sig = saving;
        let mut error_sig = error;
        spawn(async move {
            let result = update_group_room_members(UpdateGroupRoomMembersRequest {
                room_id: room_id_to_save,
                members,
            })
            .await;
            // Fresh write-lock, acquired only after the await resolves.
            saving_sig.set(false);
            match result {
                Ok(_room) => on_saved.call(()),
                Err(_) => error_sig.set(Some(
                    "Could not save room membership. Check your selection and try again."
                        .to_string(),
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
                "aria-labelledby": "kn-edit-members-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !is_saving {
                        on_close.call(());
                    }
                },
                div { class: "kn-modal-header",
                    h3 { class: "kn-modal-title", id: "kn-edit-members-title", "{title}" }
                }
                div { class: "kn-modal-body",
                    label { class: "kn-modal-label", "MEMBERS (2–6)" }
                    div { class: "kn-room-member-list",
                        for bot_name in pick_list.iter().cloned() {
                            {
                                let is_selected = selected_val.contains(&bot_name);
                                let entry = roster_entries.iter().find(|e| e.row.name == bot_name);
                                let avatar = entry.and_then(|e| e.meta.as_ref()).and_then(|m| m.avatar.clone());
                                let avatar_shape = avatar.as_ref().and_then(|a| a.shape.clone());
                                let avatar_color = avatar.as_ref().and_then(|a| a.color.clone());
                                let avatar_image_id = avatar.as_ref().and_then(|a| a.image_id.clone());
                                let bot_name_for_toggle = bot_name.clone();
                                rsx! {
                                    label { class: "kn-modal-checkbox", key: "{bot_name}",
                                        input {
                                            r#type: "checkbox",
                                            disabled: is_saving,
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
                        disabled: is_saving,
                        onclick: move |_| on_close.call(()),
                        "CANCEL"
                    }
                    button {
                        class: "kn-modal-btn kn-modal-btn--submit",
                        disabled: !can_submit,
                        onclick: submit,
                        if is_saving { "SAVING…" } else { "SAVE MEMBERS" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ProfileGap, ProfileHealth, ProfileRow};

    fn entry(name: &str, is_live: bool) -> BotRosterEntry {
        BotRosterEntry {
            row: ProfileRow {
                name: name.to_string(),
                health: ProfileHealth::Configured,
                gaps: Vec::<ProfileGap>::new(),
                provider: None,
                model_default: None,
                key_count: 0,
            },
            meta: None,
            is_live,
            is_kanban_worker: false,
        }
    }

    // -------------------------------------------------------------------
    // edit_members_selection_seed
    // -------------------------------------------------------------------

    #[test]
    fn edit_members_selection_seed_equals_the_current_members() {
        let seed = edit_members_selection_seed(&["scout".to_string(), "zig".to_string()]);
        assert_eq!(
            seed,
            BTreeSet::from(["scout".to_string(), "zig".to_string()])
        );
    }

    #[test]
    fn edit_members_selection_seed_empty_members_yields_empty_set() {
        assert!(edit_members_selection_seed(&[]).is_empty());
    }

    // -------------------------------------------------------------------
    // edit_members_pick_list
    // -------------------------------------------------------------------

    #[test]
    fn edit_members_pick_list_contains_every_non_live_roster_bot() {
        let roster = vec![entry("scout", false), entry("zig", false)];
        let list = edit_members_pick_list(&[], &roster);
        assert_eq!(list, vec!["scout".to_string(), "zig".to_string()]);
    }

    #[test]
    fn edit_members_pick_list_excludes_a_live_bot_unless_it_is_a_current_member() {
        let roster = vec![entry("scout", true), entry("zig", false)];
        let list = edit_members_pick_list(&[], &roster);
        assert_eq!(list, vec!["zig".to_string()]);
    }

    #[test]
    fn edit_members_pick_list_a_current_member_absent_from_the_roster_still_appears() {
        let roster = vec![entry("zig", false)];
        let list = edit_members_pick_list(&["ada".to_string()], &roster);
        assert_eq!(list, vec!["zig".to_string(), "ada".to_string()]);
    }

    #[test]
    fn edit_members_pick_list_a_current_member_that_is_now_live_still_appears() {
        // The exact scenario the tail exists for: a current member became
        // the live profile and is therefore filtered from the non-live
        // roster loop — the tail still surfaces it so it can be seen and
        // deliberately removed rather than silently dropped by a save.
        let roster = vec![entry("scout", true)];
        let list = edit_members_pick_list(&["scout".to_string()], &roster);
        assert_eq!(list, vec!["scout".to_string()]);
    }

    #[test]
    fn edit_members_pick_list_has_no_duplicates_and_stable_order() {
        let roster = vec![entry("scout", false), entry("zig", false)];
        let list = edit_members_pick_list(&["scout".to_string(), "ada".to_string()], &roster);
        assert_eq!(
            list,
            vec!["scout".to_string(), "zig".to_string(), "ada".to_string()]
        );
    }
}
