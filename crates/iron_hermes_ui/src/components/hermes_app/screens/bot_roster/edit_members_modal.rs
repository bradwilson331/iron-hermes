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
//! [`crate::components::hermes_app::screens::bot_roster::create_room_modal::member_selection_is_valid`]
//! and `member_selection_hint` rather than re-deriving them — there is
//! exactly one counter-hint string in this crate, and `CreateRoomModal` and
//! `EditMembersModal` both read it. Phase 52 review fix (WR-01): both
//! modals independently resolve the operator-persisted `min_members`/
//! `max_members` bound via `load_group_chat_settings()` (the same
//! `use_resource` + fallback-to-default pattern `TeamPatternFields` already
//! uses for `max_cycles`) and pass it into these shared functions — this
//! module no longer imports a `member_selection_max()` constant-bound
//! helper, since the resolved value now lives in each modal's own derived
//! state.
//!
//! **No context provider (D-10).** `edit_members_open` is a plain
//! `use_signal` owned by `GroupChatWorkspace`, threaded down as a prop —
//! this module opens no context of its own.

use crate::components::hermes_app::screens::bot_roster::create_room_modal::{
    member_selection_hint, member_selection_is_valid,
};
use crate::components::hermes_app::screens::bot_roster::team_pattern_fields::{
    leader_removal_warning_text, parse_cycles_input, reconcile_leader_draft,
    save_is_blocked_by_missing_leader, seed_leader_prompt_text, seed_worker_prompt_text,
    team_setup_from_draft, CyclesInput, LeaderRadioCell, TeamPatternFields,
};
use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::{
    BotRosterEntry, GroupChatSettings, GroupRoomTeamSetup, MemberRole, TeamPattern,
    UpdateGroupRoomTeamRequest,
};
use crate::server::group_members_api::update_group_room_team;
use crate::server::group_settings_api::load_group_chat_settings;
use dioxus::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

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

/// Phase 52 (D-06/D-09): seeds the team draft's toggle/leader/cycles-text
/// from the room's currently persisted `pattern`/`roles`/`max_cycles` —
/// `(team_toggle, leader, cycles_text)`. A blank cycles text means "no
/// per-room override," matching [`parse_cycles_input`]'s own `Blank`
/// contract. Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn edit_members_team_draft_seed(
    pattern: &Option<TeamPattern>,
    roles: &BTreeMap<String, MemberRole>,
    max_cycles: Option<u32>,
) -> (bool, Option<String>, String) {
    let toggle = pattern.is_some();
    let leader = roles
        .iter()
        .find(|(_, role)| matches!(role, MemberRole::Leader))
        .map(|(name, _)| name.clone());
    let cycles_text = max_cycles.map(|n| n.to_string()).unwrap_or_default();
    (toggle, leader, cycles_text)
}

/// Phase 52 (D-07): `true` exactly when the room is CURRENTLY a team and
/// the pending selection excludes its persisted leader — the trigger for
/// both the inline removal warning and the SAVE relabel. Pure,
/// disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn edit_members_leader_is_being_removed(
    room_is_team: bool,
    persisted_leader: Option<&str>,
    pending_selection: &BTreeSet<String>,
) -> bool {
    room_is_team
        && persisted_leader
            .map(|name| !pending_selection.contains(name))
            .unwrap_or(false)
}

/// Phase 52 (D-07, Round 1 codex HIGH): the SAVE button's label — the
/// UI-SPEC Copywriting Contract relabel only when
/// [`edit_members_leader_is_being_removed`] is true, plain wording in
/// every other case (peer room, team room with the leader retained, team
/// room with a different member removed). Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn edit_members_save_label(
    room_is_team: bool,
    persisted_leader: Option<&str>,
    pending_selection: &BTreeSet<String>,
) -> &'static str {
    if edit_members_leader_is_being_removed(room_is_team, persisted_leader, pending_selection) {
        "SAVE & DEMOTE TO PEER ROOM"
    } else {
        "SAVE MEMBERS"
    }
}

/// Phase 50.2 Plan 17 (G-50.2-2a) / Phase 52 Plan 06 (D-06/D-07/D-09/D-16):
/// the `Edit members` modal — `CreateRoomModal`'s picker, pre-seeded with
/// the room's current membership, reused element-for-element, plus the
/// shared team controls pre-seeded from the room's own
/// `pattern`/`roles`/`max_cycles`/role-prompt overrides. `roster_entries`
/// is the SAME joined roster the caller (`GroupChatWorkspace`, threaded
/// down from `BotRoster`) already computed — this component issues no
/// fetch of its own.
#[component]
pub fn EditMembersModal(
    room_id: String,
    room_name: String,
    current_members: Vec<String>,
    roster_entries: Vec<BotRosterEntry>,
    pattern: Option<TeamPattern>,
    roles: BTreeMap<String, MemberRole>,
    max_cycles: Option<u32>,
    leader_prompt_override: Option<String>,
    worker_prompt_override: Option<String>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<()>,
) -> Element {
    let pick_list = edit_members_pick_list(&current_members, &roster_entries);
    let room_is_team = pattern.is_some();
    let (team_toggle_seed, leader_seed, cycles_text_seed) =
        edit_members_team_draft_seed(&pattern, &roles, max_cycles);
    // The room's PERSISTED leader (pre-any-edit) — distinct from the
    // `leader` draft signal below, which the operator can change. D-07's
    // demotion detection always compares against this frozen snapshot,
    // never against the live draft.
    let persisted_leader = leader_seed.clone();

    let mut selected: Signal<BTreeSet<String>> =
        use_signal(|| edit_members_selection_seed(&current_members));
    let mut saving: Signal<bool> = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut team_toggle: Signal<bool> = use_signal(move || team_toggle_seed);
    let mut leader: Signal<Option<String>> = use_signal(move || leader_seed);
    let cycles_text: Signal<String> = use_signal(move || cycles_text_seed);
    let leader_prompt_text: Signal<String> =
        use_signal(move || seed_leader_prompt_text(leader_prompt_override.as_deref()));
    let worker_prompt_text: Signal<String> =
        use_signal(move || seed_worker_prompt_text(worker_prompt_override.as_deref()));
    // Phase 52 review fix (WR-01): resolve the operator-persisted
    // member-count bound the same way `TeamPatternFields` already resolves
    // `max_cycles` — via `use_resource` + `load_group_chat_settings()`,
    // falling back to `GroupChatSettings::default()` while loading/on
    // error, matching the server's own fallback policy in
    // `validate_room_members`/`group_chat_settings_for_drive`. ALL hooks
    // register unconditionally on every render (GatewayScopeSelector
    // discipline).
    let settings_resource = use_resource(move || async move { load_group_chat_settings().await });

    // ---- Derived values (read BEFORE rsx!, clippy.toml discipline). ----
    let selected_val = selected.read().clone();
    let selected_count = selected_val.len();
    let is_saving = *saving.read();
    let is_team = *team_toggle.read();
    let leader_val = leader.read().clone();
    let cycles_parsed = parse_cycles_input(&cycles_text.read());
    let resolved_settings = match settings_resource() {
        Some(Ok(settings)) => settings,
        _ => GroupChatSettings::default(),
    };
    let min_members = resolved_settings.min_members as usize;
    let max_members = resolved_settings.max_members as usize;
    let can_submit = member_selection_is_valid(selected_count, min_members, max_members)
        && !is_saving
        && !save_is_blocked_by_missing_leader(is_team, leader_val.as_deref())
        && !matches!(cycles_parsed, CyclesInput::Invalid);
    let hint = member_selection_hint(selected_count, min_members, max_members);
    let error_val = error.read().clone();
    let title = format!("Edit members — \"{room_name}\"");
    let leader_being_removed =
        edit_members_leader_is_being_removed(room_is_team, persisted_leader.as_deref(), &selected_val);
    let removal_warning_text = persisted_leader
        .as_deref()
        .map(leader_removal_warning_text)
        .unwrap_or_default();
    let save_label = edit_members_save_label(room_is_team, persisted_leader.as_deref(), &selected_val);

    let submit = move |_| {
        if !can_submit {
            return;
        }
        let room_id_to_save = room_id.clone();
        let members: Vec<String> = selected.read().iter().cloned().collect();
        // D-06/D-09: membership and team composition ride the SAME
        // request so they cannot half-save. `UpdateGroupRoomTeamRequest`'s
        // `team` field is a required `GroupRoomTeamSetup`, not an
        // `Option` — a `None` draft (toggle off) maps to the explicit
        // peer-room shape rather than omitting the field.
        let team = team_setup_from_draft(
            *team_toggle.read(),
            leader.read().as_deref(),
            parse_cycles_input(&cycles_text.read()),
            &leader_prompt_text.read(),
            &worker_prompt_text.read(),
        )
        .unwrap_or_else(|| GroupRoomTeamSetup {
            pattern: None,
            roles: BTreeMap::new(),
            max_cycles: None,
            leader_prompt_override: None,
            worker_prompt_override: None,
        });
        saving.set(true);
        error.set(None);
        let mut saving_sig = saving;
        let mut error_sig = error;
        spawn(async move {
            let result = update_group_room_team(UpdateGroupRoomTeamRequest {
                room_id: room_id_to_save,
                members,
                team,
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
                    if is_team {
                        div { style: "display: flex; flex-direction: row; align-items: center; gap: var(--sp-2);",
                            label { class: "kn-modal-label", style: "flex: 1; margin-top: 0;", "MEMBERS (2–6)" }
                            label { class: "kn-modal-label", style: "margin-top: 0;", "LEADER" }
                        }
                    } else {
                        label { class: "kn-modal-label", "MEMBERS (2–6)" }
                    }
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
                                let persisted_leader_for_toggle = persisted_leader.clone();
                                rsx! {
                                    label { class: "kn-modal-checkbox", key: "{bot_name}",
                                        input {
                                            r#type: "checkbox",
                                            disabled: is_saving,
                                            checked: is_selected,
                                            onchange: move |_| {
                                                {
                                                    let mut set = selected.write();
                                                    if set.contains(&bot_name_for_toggle) {
                                                        set.remove(&bot_name_for_toggle);
                                                    } else if set.len() < max_members {
                                                        set.insert(bot_name_for_toggle.clone());
                                                    }
                                                }
                                                // D-07 (Round 1 codex HIGH): removing the
                                                // room's CURRENTLY PERSISTED leader clears
                                                // the draft AND turns the team toggle off,
                                                // so the missing-leader guard cannot
                                                // disable the Save this demotion requires.
                                                let selection_now = selected.read().clone();
                                                let current_leader = leader.read().clone();
                                                let current_toggle = *team_toggle.read();
                                                let (new_leader, new_toggle) = reconcile_leader_draft(
                                                    persisted_leader_for_toggle.as_deref(),
                                                    &selection_now,
                                                    current_leader.as_deref(),
                                                    current_toggle,
                                                );
                                                leader.set(new_leader);
                                                team_toggle.set(new_toggle);
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
                                        LeaderRadioCell {
                                            bot_name: bot_name.clone(),
                                            leader,
                                            team_toggle,
                                            checked: is_selected,
                                        }
                                    }
                                }
                            }
                        }
                    }
                    TeamPatternFields {
                        team_toggle,
                        cycles_text,
                        leader_prompt_text,
                        worker_prompt_text,
                        disabled: is_saving,
                    }
                    if leader_being_removed {
                        div { class: "kn-modal-hint--info", "{removal_warning_text}" }
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
                        if is_saving { "SAVING…" } else { "{save_label}" }
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

    // -------------------------------------------------------------------
    // edit_members_team_draft_seed
    // -------------------------------------------------------------------

    #[test]
    fn edit_members_seeds_the_team_draft_from_the_rooms_current_values() {
        let mut roles = BTreeMap::new();
        roles.insert("scout".to_string(), MemberRole::Leader);
        let (toggle, leader, cycles_text) = edit_members_team_draft_seed(
            &Some(TeamPattern::OrchestratorWorkers),
            &roles,
            Some(3),
        );
        assert!(toggle);
        assert_eq!(leader, Some("scout".to_string()));
        assert_eq!(cycles_text, "3".to_string());
    }

    #[test]
    fn edit_members_team_draft_seed_a_peer_room_has_no_leader_and_a_blank_cycles_text() {
        let (toggle, leader, cycles_text) =
            edit_members_team_draft_seed(&None, &BTreeMap::new(), None);
        assert!(!toggle);
        assert_eq!(leader, None);
        assert_eq!(cycles_text, String::new());
    }

    // -------------------------------------------------------------------
    // seed_leader_prompt_text / seed_worker_prompt_text (re-used, not
    // re-derived, from team_pattern_fields.rs)
    // -------------------------------------------------------------------

    #[test]
    fn edit_members_seeds_the_prompt_textareas_with_the_room_override_or_the_shared_default() {
        use crate::protocol::{DEFAULT_LEADER_DECOMPOSE_TEMPLATE, DEFAULT_WORKER_TEMPLATE};

        assert_eq!(seed_leader_prompt_text(None), DEFAULT_LEADER_DECOMPOSE_TEMPLATE);
        assert_eq!(seed_worker_prompt_text(None), DEFAULT_WORKER_TEMPLATE);
        assert_eq!(
            seed_leader_prompt_text(Some("room-specific leader instructions")),
            "room-specific leader instructions".to_string()
        );
        assert_eq!(
            seed_worker_prompt_text(Some("room-specific worker instructions")),
            "room-specific worker instructions".to_string()
        );
    }

    // -------------------------------------------------------------------
    // edit_members_save_label (Round 1 codex HIGH)
    // -------------------------------------------------------------------

    #[test]
    fn edit_members_save_label_switches_to_demote_only_when_the_leader_is_being_removed() {
        let full_selection: BTreeSet<String> =
            BTreeSet::from(["scout".to_string(), "zig".to_string(), "ada".to_string()]);
        let without_leader: BTreeSet<String> = BTreeSet::from(["zig".to_string()]);
        // Same leader ("scout") retained, but a DIFFERENT member ("ada")
        // has been removed relative to `full_selection`.
        let different_member_removed: BTreeSet<String> =
            BTreeSet::from(["scout".to_string(), "zig".to_string()]);

        // Peer room: never relabelled, regardless of selection.
        assert_eq!(
            edit_members_save_label(false, None, &without_leader),
            "SAVE MEMBERS"
        );
        // Team room, leader retained: plain wording.
        assert_eq!(
            edit_members_save_label(true, Some("scout"), &full_selection),
            "SAVE MEMBERS"
        );
        // Team room, a DIFFERENT member removed (leader untouched): plain
        // wording.
        assert_eq!(
            edit_members_save_label(true, Some("scout"), &different_member_removed),
            "SAVE MEMBERS"
        );
        // Team room, the leader itself excluded from the pending
        // selection: the demote relabel.
        assert_eq!(
            edit_members_save_label(true, Some("scout"), &without_leader),
            "SAVE & DEMOTE TO PEER ROOM"
        );
    }

    // -------------------------------------------------------------------
    // Composed: reconcile_leader_draft + save_is_blocked_by_missing_leader
    // + edit_members_save_label (Round 1 codex HIGH)
    // -------------------------------------------------------------------

    #[test]
    fn edit_members_removing_the_leader_leaves_the_submit_enabled_and_relabelled() {
        let persisted_leader = Some("scout");
        // The pending selection excludes the persisted leader.
        let selection: BTreeSet<String> = BTreeSet::from(["zig".to_string()]);

        let (new_leader, new_toggle) =
            reconcile_leader_draft(persisted_leader, &selection, persisted_leader, true);
        assert!(
            !save_is_blocked_by_missing_leader(new_toggle, new_leader.as_deref()),
            "demotion must leave SAVE enabled, never disabled"
        );
        assert_eq!(
            edit_members_save_label(true, persisted_leader, &selection),
            "SAVE & DEMOTE TO PEER ROOM"
        );
    }
}
