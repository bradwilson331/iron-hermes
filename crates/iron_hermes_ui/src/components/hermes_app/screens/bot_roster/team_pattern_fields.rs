//! Phase 52 (D-07/D-09/D-16): the shared team-composition controls — a
//! `Make this a team` toggle, a per-row leader radio, a per-room cycle
//! budget and two role-prompt overrides. ONE component, TWO mounts
//! (`SecretsSourcePicker`'s own discipline, `profile_shared/
//! secrets_source_picker.rs:1-20`, for this exact class of problem):
//! `CreateRoomModal` mounts this to let a room start as a team (D-09 create
//! half), `EditMembersModal` mounts it to let an existing peer room convert
//! and a team room demote (D-09 convert half, D-07's client mirror).
//!
//! **Signal-backed props, never a context provider.** Draft state
//! (`team_toggle`, `cycles_text`, `leader_prompt_text`, `worker_prompt_text`
//! below, plus each row's own `leader` signal) is threaded down from each
//! modal's own `use_signal` calls, mirroring `SecretsSourcePicker`'s prop
//! shape exactly — UI-SPEC Surface Contract 2 locks this: "not a shared
//! context — D-10's 'no context provider' rule applies here too."
//!
//! **Every hook registers unconditionally on every render, never behind an
//! `if`.** This crate has shipped both of the failure modes that rule
//! exists to prevent: an inline dropdown mounted per-row that rendered
//! blank, and a `use_effect` that read and unconditionally wrote the same
//! signal, freezing the screen with no console error
//! (`GatewayScopeSelector`, `screens/gateway/mod.rs:290`).
//!
//! **The leader control is a native `input[type=radio]` per member row,
//! never a `select`.** The failure class both precedents above share is a
//! control mounted per-row inside a list with something to lazily
//! populate. A radio group has nothing to lazily populate — its options
//! are exactly the currently-checked members, known synchronously — so it
//! cannot hit that failure class structurally (UI-SPEC Surface Contract
//! 2's locked resolution).

use crate::protocol::{
    GroupChatSettings, GroupRoomTeamSetup, MemberRole, TeamPattern, DEFAULT_LEADER_DECOMPOSE_TEMPLATE,
    DEFAULT_WORKER_TEMPLATE, TEAM_CYCLE_MAX, TEAM_CYCLE_MIN,
};
use crate::server::group_settings_api::load_group_chat_settings;
use dioxus::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// Phase 52 (Round 1 codex MEDIUM): a `CYCLES (this room)` textarea's
/// parsed content — three distinguishable states, not two. Collapsing "I
/// typed nonsense" into the same value as "I deliberately left it blank to
/// inherit the app-wide default" is the silent-data-loss shape this phase
/// is otherwise careful about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CyclesInput {
    /// Empty or whitespace-only — valid, means inherit the app-wide
    /// default.
    Blank,
    /// A decimal integer inside `TEAM_CYCLE_MIN..=TEAM_CYCLE_MAX`.
    Valid(u32),
    /// Anything else: non-numeric, negative, or an in-range-looking
    /// integer outside the shared bound. Renders a field error and blocks
    /// submit; never silently coerced into `Blank`.
    Invalid,
}

/// Phase 52 (Round 1 codex MEDIUM): parses a `CYCLES (this room)` field.
/// Bounded by the shared [`TEAM_CYCLE_MIN`]/[`TEAM_CYCLE_MAX`] consts
/// rather than restated literals, so this client-side mirror cannot drift
/// from Plan 03's server-side `validate_team_room_shape` check. Pure,
/// disk/DOM-I/O-free, never panics.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn parse_cycles_input(raw: &str) -> CyclesInput {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return CyclesInput::Blank;
    }
    match trimmed.parse::<u32>() {
        Ok(n) if (TEAM_CYCLE_MIN..=TEAM_CYCLE_MAX).contains(&n) => CyclesInput::Valid(n),
        _ => CyclesInput::Invalid,
    }
}

/// Phase 52 (D-16): seeds the `LEADER PROMPT` textarea — an override
/// present seeds the override text; an override absent seeds
/// [`DEFAULT_LEADER_DECOMPOSE_TEMPLATE`] directly, the SAME const the
/// server-side prompt builder reads. One string, two readers — there is no
/// second copy to drift from. Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn seed_leader_prompt_text(override_text: Option<&str>) -> String {
    override_text.unwrap_or(DEFAULT_LEADER_DECOMPOSE_TEMPLATE).to_string()
}

/// Phase 52 (D-16): seeds the `WORKER PROMPT` textarea — same contract as
/// [`seed_leader_prompt_text`], against [`DEFAULT_WORKER_TEMPLATE`]. Pure,
/// disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn seed_worker_prompt_text(override_text: Option<&str>) -> String {
    override_text.unwrap_or(DEFAULT_WORKER_TEMPLATE).to_string()
}

/// Phase 52 (D-06/D-09/D-14/D-16): builds the request-shaped team setup
/// from one modal's draft signals, or `None` when `Make this a team` is
/// toggled off — regardless of what leader or cycles values are still
/// sitting in the draft, a peer room is a peer room. Never called with an
/// `Invalid` cycles input: submit is blocked before this runs, asserted
/// here rather than silently coerced. A textarea whose content equals the
/// shared `protocol` default maps to `None` rather than persisting a
/// redundant copy of the default. Pure, disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn team_setup_from_draft(
    team_toggle: bool,
    leader: Option<&str>,
    cycles: CyclesInput,
    leader_prompt_text: &str,
    worker_prompt_text: &str,
) -> Option<GroupRoomTeamSetup> {
    if !team_toggle {
        return None;
    }
    debug_assert!(
        !matches!(cycles, CyclesInput::Invalid),
        "team_setup_from_draft must never be called with an Invalid cycles input — submit is \
         blocked before this runs"
    );
    let mut roles = BTreeMap::new();
    if let Some(name) = leader {
        roles.insert(name.to_string(), MemberRole::Leader);
    }
    let max_cycles = match cycles {
        CyclesInput::Valid(n) => Some(n),
        _ => None,
    };
    let leader_prompt_override = (leader_prompt_text != DEFAULT_LEADER_DECOMPOSE_TEMPLATE)
        .then(|| leader_prompt_text.to_string());
    let worker_prompt_override =
        (worker_prompt_text != DEFAULT_WORKER_TEMPLATE).then(|| worker_prompt_text.to_string());
    Some(GroupRoomTeamSetup {
        pattern: Some(TeamPattern::OrchestratorWorkers),
        roles,
        max_cycles,
        leader_prompt_override,
        worker_prompt_override,
    })
}

/// Phase 52 (UI-SPEC Surface Contract 2, Round 1 codex HIGH): `true` exactly
/// when `Make this a team` is on and no leader has been chosen — the
/// genuinely incomplete state. This is the ONE disabling condition this
/// phase adds to SAVE/CREATE; it is deliberately FALSE once
/// [`reconcile_leader_draft`] has turned the toggle off in response to a
/// demotion, so the client can never submit `pattern: Some(...)` with an
/// empty `roles` map, even though the server validates it too. Pure,
/// disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn save_is_blocked_by_missing_leader(team_toggle: bool, leader: Option<&str>) -> bool {
    team_toggle && leader.is_none()
}

/// Phase 52 (D-07, Round 1 codex HIGH): the client-side mirror of
/// `group_chat_store::normalize_team_setup_for_write`'s server-side split —
/// called on every member-selection change. When the just-removed member
/// is the room's CURRENTLY PERSISTED leader, clears BOTH the leader draft
/// AND the team toggle, making demotion an explicit, visible draft state
/// rather than an implicit one: clearing only the leader would leave the
/// toggle on, which makes [`save_is_blocked_by_missing_leader`] return
/// true and disable the very Save that D-07 demotion requires — the
/// original acceptance criteria demanded both outcomes for that one state
/// and were self-contradictory until this split existed. In every OTHER
/// case — the draft leader is still selected, or the draft leader was
/// never the room's persisted leader (a brand-new room in `CreateRoomModal`,
/// or a leader chosen but never saved) — only the stale leader pointer is
/// cleared and the toggle is left exactly as the operator set it, so a
/// genuinely incomplete team ([`save_is_blocked_by_missing_leader`]) stays
/// blocked rather than being silently normalized into a demotion. Pure,
/// disk/DOM-I/O-free. Returns `(new_leader, new_team_toggle)`.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn reconcile_leader_draft(
    persisted_leader: Option<&str>,
    selection: &BTreeSet<String>,
    leader_draft: Option<&str>,
    team_toggle: bool,
) -> (Option<String>, bool) {
    let Some(current_leader) = leader_draft else {
        return (None, team_toggle);
    };
    if selection.contains(current_leader) {
        return (Some(current_leader.to_string()), team_toggle);
    }
    if persisted_leader == Some(current_leader) {
        (None, false)
    } else {
        (None, team_toggle)
    }
}

/// Phase 52 (D-07): the UI-SPEC Copywriting Contract's inline
/// leader-removal warning sentence, naming the leader being removed. Pure,
/// disk/DOM-I/O-free.
#[allow(dead_code)] // see create_room_modal.rs's member_selection_is_valid doc note (legacy-shell reachability)
pub(crate) fn leader_removal_warning_text(leader: &str) -> String {
    format!(
        "Removing {leader} will turn this back into a peer room — the team pattern and leader \
         role are cleared."
    )
}

/// Phase 52 (UI-SPEC Surface Contract 2): one member row's leader radio.
/// All radios across both modals share `name="leader"`, so native
/// radio-group semantics give single-selection with no extra state. Has no
/// hooks of its own — every prop is `Signal`-backed state the caller
/// already owns, so there is nothing here that could register
/// conditionally in the first place. Renders nothing at all when the team
/// toggle is off, so a peer-room edit looks exactly as it did before this
/// phase.
#[component]
pub fn LeaderRadioCell(
    bot_name: String,
    leader: Signal<Option<String>>,
    team_toggle: Signal<bool>,
    checked: bool,
) -> Element {
    let team_is_on = *team_toggle.read();
    if !team_is_on {
        return rsx! {};
    }
    let is_leader = leader.read().as_deref() == Some(bot_name.as_str());
    let name_for_click = bot_name.clone();

    rsx! {
        input {
            r#type: "radio",
            name: "leader",
            // A non-member cannot be leader (UI-SPEC Surface Contract 2).
            disabled: !checked,
            checked: is_leader,
            // `value` is load-bearing, not decoration: `dioxus-web`'s form
            // handler special-cases ONLY `type="checkbox"` when building the
            // event's value (`dioxus-web/src/events/form.rs`, whose own
            // comment reads `todo: special case more input types`). Every
            // other input type — radio included — reports `input.value()`,
            // which defaults to the DOM's `"on"`. `FormData::checked()` is
            // `self.value().parse().unwrap_or(false)`, so `"on"` never
            // parses and a radio's `checked()` is FALSE FOREVER.
            value: "true",
            // Deliberately NOT gated on `evt.checked()`. An HTML radio fires
            // `change` only on the element that BECOMES selected — there is
            // no deselect event — so the guard bought nothing and, via the
            // path above, silently discarded every selection: the leader
            // draft stayed `None`, `save_is_blocked_by_missing_leader` stayed
            // true, and CREATE ROOM was permanently disabled. This mirrors
            // the member checkbox in `create_room_modal.rs`, which likewise
            // ignores the event and writes from its own state.
            onchange: move |_| {
                leader.set(Some(name_for_click.clone()));
            },
        }
    }
}

/// Phase 52 (UI-SPEC Surface Contract 2/Copywriting Contract): the `Make
/// this a team` toggle plus, only when it is on, `CYCLES (this room)` and
/// the two role-prompt textareas with their `RESET TO DEFAULT` actions.
/// Reuses `.kn-modal-label`/`.kn-modal-input`/`.kn-modal-textarea`/
/// `.kn-modal-hint--info`/`.kn-modal-error` verbatim — no new modal chrome,
/// no new spacing value, no new color literal. Mounted directly below the
/// member list in both `CreateRoomModal` and `EditMembersModal`; the
/// `LEADER` column header and the per-row [`LeaderRadioCell`] live inside
/// each modal's own member-list markup, not here, since that column sits
/// ABOVE and INSIDE the member list rather than below it.
#[component]
pub fn TeamPatternFields(
    team_toggle: Signal<bool>,
    cycles_text: Signal<String>,
    leader_prompt_text: Signal<String>,
    worker_prompt_text: Signal<String>,
    disabled: bool,
) -> Element {
    // ALL hooks register unconditionally on every render — the
    // GatewayScopeSelector discipline this module's own doc comment cites.
    let app_default_resource = use_resource(move || async move { load_group_chat_settings().await });

    // ---- Derived values (read BEFORE rsx!, clippy.toml discipline). ----
    let is_on = *team_toggle.read();
    let cycles_val = cycles_text.read().clone();
    let leader_prompt_val = leader_prompt_text.read().clone();
    let worker_prompt_val = worker_prompt_text.read().clone();
    // Planner assumption (52-06-PLAN.md): the hint renders the shipped
    // default synchronously and is replaced in place once the resource
    // resolves — no skeleton, no spinner, no blank — mirroring
    // `create_room_modal.rs`'s `member_selection_max` synchronous-default
    // precedent for this exact class of hint.
    let app_default_cycles = match app_default_resource() {
        Some(Ok(settings)) => settings.max_cycles,
        _ => GroupChatSettings::default().max_cycles,
    };
    let cycles_invalid = matches!(parse_cycles_input(&cycles_val), CyclesInput::Invalid);

    rsx! {
        label { class: "kn-modal-checkbox",
            input {
                r#type: "checkbox",
                disabled,
                checked: is_on,
                onchange: move |evt| team_toggle.set(evt.checked()),
            }
            "Make this a team"
        }
        if is_on {
            label { class: "kn-modal-label", "CYCLES (this room)" }
            input {
                class: "kn-modal-input",
                disabled,
                value: "{cycles_val}",
                oninput: move |evt| cycles_text.set(evt.value()),
            }
            div { class: "kn-modal-hint--info",
                "Leave blank to use the app-wide default ({app_default_cycles})."
            }
            if cycles_invalid {
                div { class: "kn-modal-error",
                    "CYCLES (this room) must be blank or a whole number from {TEAM_CYCLE_MIN} to {TEAM_CYCLE_MAX}."
                }
            }
            label { class: "kn-modal-label", "LEADER PROMPT" }
            textarea {
                class: "kn-modal-textarea",
                disabled,
                value: "{leader_prompt_val}",
                oninput: move |evt| leader_prompt_text.set(evt.value()),
            }
            div { class: "kn-modal-hint--info", "Pre-filled with the default. Edits apply to this room only." }
            button {
                class: "kn-modal-btn kn-settings-reset",
                r#type: "button",
                disabled,
                onclick: move |_| leader_prompt_text.set(DEFAULT_LEADER_DECOMPOSE_TEMPLATE.to_string()),
                "RESET TO DEFAULT"
            }
            label { class: "kn-modal-label", "WORKER PROMPT" }
            textarea {
                class: "kn-modal-textarea",
                disabled,
                value: "{worker_prompt_val}",
                oninput: move |evt| worker_prompt_text.set(evt.value()),
            }
            div { class: "kn-modal-hint--info", "Pre-filled with the default. Edits apply to this room only." }
            button {
                class: "kn-modal-btn kn-settings-reset",
                r#type: "button",
                disabled,
                onclick: move |_| worker_prompt_text.set(DEFAULT_WORKER_TEMPLATE.to_string()),
                "RESET TO DEFAULT"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // team_setup_from_draft
    // -------------------------------------------------------------------

    #[test]
    fn team_setup_from_draft_is_none_when_the_team_toggle_is_off() {
        let setup = team_setup_from_draft(
            false,
            Some("scout"),
            CyclesInput::Valid(3),
            DEFAULT_LEADER_DECOMPOSE_TEMPLATE,
            DEFAULT_WORKER_TEMPLATE,
        );
        assert!(setup.is_none());
    }

    #[test]
    fn team_setup_from_draft_carries_exactly_one_leader_entry() {
        let setup = team_setup_from_draft(
            true,
            Some("scout"),
            CyclesInput::Blank,
            DEFAULT_LEADER_DECOMPOSE_TEMPLATE,
            DEFAULT_WORKER_TEMPLATE,
        )
        .expect("toggle is on, so a setup must be produced");
        assert_eq!(setup.pattern, Some(TeamPattern::OrchestratorWorkers));
        assert_eq!(setup.roles.len(), 1);
        assert_eq!(setup.roles.get("scout"), Some(&MemberRole::Leader));
    }

    #[test]
    fn an_unedited_prompt_override_maps_to_none() {
        let setup = team_setup_from_draft(
            true,
            Some("scout"),
            CyclesInput::Blank,
            DEFAULT_LEADER_DECOMPOSE_TEMPLATE,
            DEFAULT_WORKER_TEMPLATE,
        )
        .expect("toggle is on");
        assert_eq!(setup.leader_prompt_override, None);
        assert_eq!(setup.worker_prompt_override, None);

        let edited = team_setup_from_draft(
            true,
            Some("scout"),
            CyclesInput::Blank,
            "custom leader instructions",
            DEFAULT_WORKER_TEMPLATE,
        )
        .expect("toggle is on");
        assert_eq!(
            edited.leader_prompt_override,
            Some("custom leader instructions".to_string())
        );
        assert_eq!(edited.worker_prompt_override, None);
    }

    // -------------------------------------------------------------------
    // save_is_blocked_by_missing_leader
    // -------------------------------------------------------------------

    #[test]
    fn save_is_blocked_by_missing_leader_when_team_is_on_and_no_radio_is_checked() {
        assert!(save_is_blocked_by_missing_leader(true, None));
    }

    #[test]
    fn save_is_not_blocked_when_the_team_toggle_is_off() {
        assert!(!save_is_blocked_by_missing_leader(false, None));
    }

    #[test]
    fn save_is_not_blocked_when_team_is_on_and_a_leader_is_chosen() {
        assert!(!save_is_blocked_by_missing_leader(true, Some("scout")));
    }

    #[test]
    fn save_is_blocked_when_the_toggle_is_on_and_no_leader_was_ever_chosen() {
        // The discriminating counterpart to the persisted-leader-removal
        // tests below: a genuinely incomplete team stays blocked.
        assert!(save_is_blocked_by_missing_leader(true, None));
    }

    // -------------------------------------------------------------------
    // reconcile_leader_draft (Round 1 codex HIGH)
    // -------------------------------------------------------------------

    #[test]
    fn removing_the_persisted_leader_clears_the_draft_and_turns_the_team_toggle_off() {
        let selection: BTreeSet<String> = BTreeSet::from(["zig".to_string()]);
        let (new_leader, new_toggle) =
            reconcile_leader_draft(Some("scout"), &selection, Some("scout"), true);
        assert_eq!(new_leader, None);
        assert!(!new_toggle);
    }

    #[test]
    fn removing_the_persisted_leader_leaves_save_enabled() {
        // The composed test: after reconciliation, the missing-leader
        // guard must read FALSE, because the toggle is off — this is the
        // test that would have caught the original contradiction (Round 1
        // codex HIGH).
        let selection: BTreeSet<String> = BTreeSet::from(["zig".to_string()]);
        let (new_leader, new_toggle) =
            reconcile_leader_draft(Some("scout"), &selection, Some("scout"), true);
        assert!(!save_is_blocked_by_missing_leader(new_toggle, new_leader.as_deref()));
    }

    #[test]
    fn removing_an_unpersisted_draft_leader_clears_only_the_leader() {
        // A brand-new room (CreateRoomModal): no persisted leader exists
        // yet, so removing the draft leader clears the stale pointer only
        // — the toggle stays exactly as the operator left it.
        let selection: BTreeSet<String> = BTreeSet::from(["zig".to_string()]);
        let (new_leader, new_toggle) = reconcile_leader_draft(None, &selection, Some("scout"), true);
        assert_eq!(new_leader, None);
        assert!(new_toggle);
    }

    #[test]
    fn a_leader_still_in_the_selection_is_left_untouched() {
        let selection: BTreeSet<String> = BTreeSet::from(["scout".to_string(), "zig".to_string()]);
        let (new_leader, new_toggle) =
            reconcile_leader_draft(Some("scout"), &selection, Some("scout"), true);
        assert_eq!(new_leader, Some("scout".to_string()));
        assert!(new_toggle);
    }

    // -------------------------------------------------------------------
    // parse_cycles_input
    // -------------------------------------------------------------------

    #[test]
    fn parse_cycles_input_distinguishes_blank_valid_and_invalid() {
        assert_eq!(parse_cycles_input(""), CyclesInput::Blank);
        assert_eq!(parse_cycles_input("   "), CyclesInput::Blank);
        assert_eq!(parse_cycles_input("3"), CyclesInput::Valid(3));
        // A non-numeric string must NOT yield Blank — inherit-the-default
        // and you-typed-something-wrong are different states the operator
        // must be able to tell apart.
        assert_eq!(parse_cycles_input("abc"), CyclesInput::Invalid);
    }

    #[test]
    fn parse_cycles_input_rejects_a_value_outside_the_shared_team_cycle_range() {
        assert_eq!(parse_cycles_input("0"), CyclesInput::Invalid);
        assert_eq!(parse_cycles_input("6"), CyclesInput::Invalid);
        assert_eq!(parse_cycles_input(&TEAM_CYCLE_MIN.to_string()), CyclesInput::Valid(TEAM_CYCLE_MIN));
        assert_eq!(parse_cycles_input(&TEAM_CYCLE_MAX.to_string()), CyclesInput::Valid(TEAM_CYCLE_MAX));
    }

    #[test]
    fn parse_cycles_input_rejects_a_negative_value_without_panicking() {
        assert_eq!(parse_cycles_input("-1"), CyclesInput::Invalid);
    }

    // -------------------------------------------------------------------
    // seed_leader_prompt_text / seed_worker_prompt_text
    // -------------------------------------------------------------------

    #[test]
    fn the_prompt_textareas_seed_from_the_shared_protocol_template_consts() {
        assert_eq!(seed_leader_prompt_text(None), DEFAULT_LEADER_DECOMPOSE_TEMPLATE);
        assert_eq!(seed_worker_prompt_text(None), DEFAULT_WORKER_TEMPLATE);
        assert_eq!(seed_leader_prompt_text(Some("custom")), "custom".to_string());
        assert_eq!(seed_worker_prompt_text(Some("custom")), "custom".to_string());
    }

    // -------------------------------------------------------------------
    // leader_removal_warning_text
    // -------------------------------------------------------------------

    #[test]
    fn leader_removal_warning_text_names_the_leader_being_removed() {
        let text = leader_removal_warning_text("scout");
        assert!(text.contains("scout"));
        assert!(text.contains("peer room"));
    }

    /// Phase 52 UAT regression: the leader radio silently discarded every
    /// selection, leaving CREATE ROOM permanently disabled.
    ///
    /// Root cause, and why this is a source assertion rather than a DOM
    /// test: `dioxus-web`'s form handler
    /// (`dioxus-web/src/events/form.rs`) special-cases ONLY
    /// `type="checkbox"` when building a form event's value; every other
    /// input type falls through to `input.value()`. A radio with no
    /// `value` attribute reports the DOM default `"on"`, and
    /// `FormData::checked()` is `self.value().parse().unwrap_or(false)`,
    /// so `"on"` never parses and `checked()` is false on every single
    /// change event. Gating the write on it meant `leader` stayed `None`,
    /// `save_is_blocked_by_missing_leader` stayed true, and the submit
    /// button never enabled. There is no headless-DOM harness in this
    /// crate, so this pins the two structural properties that make the
    /// control work at all.
    ///
    /// This test FAILS against the pre-fix source, which read
    /// `onchange: move |evt| { if evt.checked() { ... } }`.
    #[test]
    fn the_leader_radio_write_does_not_depend_on_evt_checked() {
        let source = include_str!("team_pattern_fields.rs");
        let start = source
            .find("pub fn LeaderRadioCell(")
            .expect("LeaderRadioCell must exist");
        let body = &source[start..];
        let end = body
            .find("\npub fn TeamPatternFields(")
            .or_else(|| body.find("\n#[component]"))
            .unwrap_or(body.len());
        // Strip comment lines before scanning. The doc comment above and the
        // inline comments inside the control both legitimately MENTION
        // `checked()` while explaining why it must not be used — scanning raw
        // text would match the explanation and fail against correct code.
        // Phase 52 hit this exact shape seven times in its own plans'
        // `<verify>` greps; do not reintroduce it here.
        let cell: String = body[..end]
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//") && !t.starts_with("///")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let cell = cell.as_str();

        assert!(
            !cell.contains(".checked()"),
            "LeaderRadioCell must not gate its write on a form event's \
             checked() — dioxus-web only special-cases type=\"checkbox\", so \
             a radio's checked() is always false. Write unconditionally: an \
             HTML radio fires `change` only on the element becoming selected."
        );
        assert!(
            cell.contains("leader.set(Some("),
            "LeaderRadioCell must write the selected bot into the leader draft"
        );
        assert!(
            cell.contains("value: \"true\""),
            "the radio must carry value=\"true\" so any future reader calling \
             FormData::checked() on it parses a real bool rather than \"on\""
        );
    }
}
