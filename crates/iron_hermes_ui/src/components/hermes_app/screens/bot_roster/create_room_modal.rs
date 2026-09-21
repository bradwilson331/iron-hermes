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
//! behavior is directly unit-testable without a renderer — the caller now
//! passes in the resolved `min_members`/`max_members` bound (see the review
//! fix note on [`CreateRoomModal`] below) rather than these functions
//! reading a hardcoded default themselves. Selection counts above the
//! resolved `max_members` are unreachable through the UI itself — the
//! member checkbox's `onchange` handler refuses to grow the selection set
//! past it — the "disabled-until-valid" pattern the plan calls for.
//!
//! Signal-borrow discipline (clippy.toml): every value read from a signal is
//! extracted BEFORE the `rsx!` block; the write-lock taken to toggle a
//! member checkbox is synchronous and released well before any `spawn`; the
//! submit handler's write-lock (`creating`/`error`) is only ever taken
//! fresh, after `create_group_room(...).await` resolves — never held across
//! the `.await` itself (mirrors `bot_roster/chat_window.rs`'s `submit_send`
//! and `profile_shared/create_dialog.rs`'s `submit_create`).

use crate::components::hermes_app::screens::bot_roster::team_pattern_fields::{
    parse_cycles_input, reconcile_leader_draft, save_is_blocked_by_missing_leader,
    seed_leader_prompt_text, seed_worker_prompt_text, team_setup_from_draft, CyclesInput,
    LeaderRadioCell, TeamPatternFields,
};
use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::{BotRosterEntry, CreateGroupRoomRequest, GroupChatSettings};
use crate::server::group_chat_api::create_group_room;
use crate::server::group_settings_api::load_group_chat_settings;
use dioxus::prelude::*;
use std::collections::BTreeSet;

/// Phase 50.2 Plan 01 (UI-SPEC E5), updated by the phase 52 review fix
/// (WR-01): `true` for a selection count within `[min_members,
/// max_members]` — 2..=6 for the shipped defaults, but the caller now
/// passes in the OPERATOR-PERSISTED bound (loaded via
/// [`load_group_chat_settings`], falling back to
/// `GroupChatSettings::default()` while loading/on error) rather than this
/// fn reading the hardcoded default itself. This mirrors the server's own
/// `validate_room_members`/`group_chat_settings_for_drive` fallback policy
/// (`group_chat_store.rs`), which phase 52's D-18 fix made authoritative —
/// before that fix client and server both read the hardcoded default, so
/// they silently agreed; this fn no longer reads a default of its own so it
/// cannot drift out of sync with the server again. Pure, disk/DOM-I/O-free.
//
// Under `--all-features`, `legacy-shell` swaps the reachable root component
// to `WarpHermes`, leaving `HermesApp` (and therefore `BotRoster`,
// `CreateRoomModal`, and this whole module) unreferenced from `main()` for
// dead-code-lint purposes — even though this is live in the default
// (non-legacy-shell) build. Same pre-existing pattern as
// `bot_roster/delete_confirm.rs`'s `confirm_input_matches`.
#[allow(dead_code)]
pub(crate) fn member_selection_is_valid(count: usize, min_members: usize, max_members: usize) -> bool {
    count >= min_members && count <= max_members
}

/// Phase 50.2 Plan 01 (UI-SPEC Copywriting Contract), updated by the phase
/// 52 review fix (WR-01): the live counter hint below the member
/// checklist — `{n} of {max} selected — need at least {min}` below the
/// minimum, `{n} of {max} selected` once valid. `min_members`/`max_members`
/// are the same operator-persisted bound [`member_selection_is_valid`] now
/// takes — see its doc note above. Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see member_selection_is_valid's doc note above
pub(crate) fn member_selection_hint(count: usize, min_members: usize, max_members: usize) -> String {
    if member_selection_is_valid(count, min_members, max_members) {
        format!("{count} of {max_members} selected")
    } else {
        format!("{count} of {max_members} selected — need at least {min_members}")
    }
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
    // Phase 52 (D-09 create half): a brand-new room's team draft — a fresh
    // room has no persisted leader, so `reconcile_leader_draft`'s
    // persisted-leader parameter is always `None` at every call site below.
    let mut team_toggle: Signal<bool> = use_signal(|| false);
    let mut leader: Signal<Option<String>> = use_signal(|| None);
    let cycles_text: Signal<String> = use_signal(String::new);
    let leader_prompt_text: Signal<String> = use_signal(|| seed_leader_prompt_text(None));
    let worker_prompt_text: Signal<String> = use_signal(|| seed_worker_prompt_text(None));
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
    let name_val = room_name.read().clone();
    let selected_val = selected.read().clone();
    let selected_count = selected_val.len();
    let is_creating = *creating.read();
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
        && !is_creating
        && !save_is_blocked_by_missing_leader(is_team, leader_val.as_deref())
        && !matches!(cycles_parsed, CyclesInput::Invalid);
    let hint = member_selection_hint(selected_count, min_members, max_members);
    let error_val = error.read().clone();

    let submit = move |_| {
        if !can_submit {
            return;
        }
        let name = room_name.read().clone();
        let members: Vec<String> = selected.read().iter().cloned().collect();
        let team = team_setup_from_draft(
            *team_toggle.read(),
            leader.read().as_deref(),
            parse_cycles_input(&cycles_text.read()),
            &leader_prompt_text.read(),
            &worker_prompt_text.read(),
        );
        creating.set(true);
        error.set(None);
        let mut creating_sig = creating;
        let mut error_sig = error;
        spawn(async move {
            // Phase 52 (D-09): `team` carries the operator's draft team
            // setup, or `None` for a plain peer room — TeamPatternFields
            // (Plan 06) is what fills it in.
            let result =
                create_group_room(CreateGroupRoomRequest { name, members, team }).await;
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
                    if is_team {
                        div { style: "display: flex; flex-direction: row; align-items: center; gap: var(--sp-2);",
                            label { class: "kn-modal-label", style: "flex: 1; margin-top: 0;", "MEMBERS (2–6)" }
                            label { class: "kn-modal-label", style: "margin-top: 0;", "LEADER" }
                        }
                    } else {
                        label { class: "kn-modal-label", "MEMBERS (2–6)" }
                    }
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
                                                {
                                                    let mut set = selected.write();
                                                    if set.contains(&bot_name_for_toggle) {
                                                        set.remove(&bot_name_for_toggle);
                                                    } else if set.len() < max_members {
                                                        set.insert(bot_name_for_toggle.clone());
                                                    }
                                                }
                                                // Phase 52 (D-09/Round 1 codex HIGH): a
                                                // brand-new room has no persisted leader
                                                // (`None`), so unchecking the draft leader
                                                // only ever clears the stale pointer — it
                                                // never turns the toggle off here (that is
                                                // EditMembersModal's D-07 demotion case).
                                                let selection_now = selected.read().clone();
                                                let current_leader = leader.read().clone();
                                                let current_toggle = *team_toggle.read();
                                                let (new_leader, new_toggle) = reconcile_leader_draft(
                                                    None,
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
                        disabled: is_creating,
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
        assert!(!member_selection_is_valid(0, 2, 6));
    }

    #[test]
    fn member_selection_is_valid_rejects_one() {
        assert!(!member_selection_is_valid(1, 2, 6));
    }

    #[test]
    fn member_selection_is_valid_accepts_two() {
        assert!(member_selection_is_valid(2, 2, 6));
    }

    #[test]
    fn member_selection_is_valid_accepts_six() {
        assert!(member_selection_is_valid(6, 2, 6));
    }

    #[test]
    fn member_selection_is_valid_rejects_seven() {
        assert!(!member_selection_is_valid(7, 2, 6));
    }

    // Phase 52 review fix (WR-01): the bound is now the CALLER's resolved
    // value, not a hardcoded default — a non-default persisted bound must
    // be honoured exactly like the server's `validate_room_members` does.
    #[test]
    fn member_selection_is_valid_honours_a_non_default_persisted_bound() {
        assert!(!member_selection_is_valid(4, 2, 3), "4 exceeds a persisted max_members of 3");
        assert!(member_selection_is_valid(3, 2, 3), "3 is within a persisted max_members of 3");
    }

    // -------------------------------------------------------------------
    // member_selection_hint — both Copywriting Contract strings
    // -------------------------------------------------------------------

    #[test]
    fn member_selection_hint_below_minimum_states_the_constraint() {
        assert_eq!(
            member_selection_hint(0, 2, 6),
            "0 of 6 selected — need at least 2"
        );
        assert_eq!(
            member_selection_hint(1, 2, 6),
            "1 of 6 selected — need at least 2"
        );
    }

    #[test]
    fn member_selection_hint_valid_selection_has_no_constraint_suffix() {
        assert_eq!(member_selection_hint(2, 2, 6), "2 of 6 selected");
        assert_eq!(member_selection_hint(6, 2, 6), "6 of 6 selected");
    }

    // Phase 52 review fix (WR-01): the hint text reflects the CALLER's
    // resolved bound, not a hardcoded default.
    #[test]
    fn member_selection_hint_reflects_a_non_default_persisted_bound() {
        assert_eq!(
            member_selection_hint(1, 2, 3),
            "1 of 3 selected — need at least 2"
        );
        assert_eq!(member_selection_hint(3, 2, 3), "3 of 3 selected");
    }
}
