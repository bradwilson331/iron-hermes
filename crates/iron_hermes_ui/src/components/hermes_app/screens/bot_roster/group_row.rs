//! Phase 50.2 Plan 01 (UI-SPEC Component Inventory §4): one group-chat
//! room's row in the roster area — the same card formula `card.rs`'s
//! `BotCard` uses (a `.kn-bot-card-head` flex row, a preview line below),
//! content differs: a stacked/overlapping member-avatar cluster instead of
//! one avatar, and the whole row is the single click affordance (a room has
//! exactly one destination, unlike a bot card's CHAT/EDIT/⋯ split).
//!
//! Room name and preview text compose onto `.kn-bot-card-title`/
//! `.kn-bot-card-preview` verbatim — the exact ellipsis-truncation rule the
//! plan calls for ("the room's preview line truncated by the same rule
//! `.kn-bot-card-preview` already uses") — rather than redeclaring an
//! identical rule under a new selector.
//!
//! Phase 50.2 Plan 06 (State Matrix, 50.1 LIVE-badge absence convention):
//! `.kn-badge[data-kind="needs-you"]` renders when `room.needs_you` is set
//! and NO element at all otherwise — absence, never a dimmed placeholder.
//! `.kn-chip[data-kind="round"]` renders only while `room.active_round` is
//! `Some` — idle rows omit it entirely. `active_round` is populated by a
//! server-side write path outside this component's own scope (no code path
//! in this phase sets it to `Some` yet, since the roster grid this row
//! lives in is always hidden by the room workspace's own drill-in replace
//! while THAT room's own dispatch is in flight — UI-SPEC's "drill into a
//! room" navigation rule means this component can currently only ever
//! observe `active_round: None`); the render path is nonetheless written
//! against the DTO's full contract, matching `GroupRoomSummary.needs_you`'s
//! own precedent of shipping a field's read path ahead of its writer.
//!
//! Phase 52 Plan 07 (D-09, UI-SPEC Surface Contracts §1): `.kn-chip
//! [data-kind="pattern"]` renders only when `room.pattern` is `Some` — same
//! absence-not-dimmed convention as the two chips above, sourced via
//! [`pattern_chip_for`]. The chip has no loading state and no error state of
//! its own — it is derived from the already-fetched `GroupRoomSummary`, so a
//! summary that fails to load takes the whole row with it, and there is
//! nothing left to paint independently once the summary itself is in hand.

use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::{BotMeta, GroupRoomSummary, TeamPattern};
use dioxus::prelude::*;
use std::collections::BTreeMap;

/// Phase 52 Plan 07 (D-09, UI-SPEC Surface Contracts §1): the roster row's
/// `TEAM` pattern chip decision, pure and unit-testable without a render
/// harness. Returns `(label, tooltip)` when `summary.pattern` is `Some`,
/// `None` for a peer room — the caller renders nothing at all in the `None`
/// case (absence, never a dimmed placeholder, matching this file's existing
/// `needs-you`/`round` convention above).
///
/// The label is always the fixed 4-character literal `TEAM` regardless of
/// the pattern arm — a second pattern in a later phase changes only the
/// tooltip returned here, never the visible chip text, so the chip can
/// never overflow its container. Reads only `pattern`: a summary whose
/// `pattern` is `Some` but whose `roles` map is empty (the D-07 invariant a
/// hand-edited or pre-demotion record can still violate) still yields a
/// chip here and never panics — the roster is a read surface and cannot be
/// made unloadable by bad data.
pub(crate) fn pattern_chip_for(summary: &GroupRoomSummary) -> Option<(&'static str, &'static str)> {
    summary.pattern.as_ref().map(|pattern| {
        let tooltip = match pattern {
            TeamPattern::OrchestratorWorkers => "Orchestrator-workers",
        };
        ("TEAM", tooltip)
    })
}

/// Phase 50.2 Plan 01: one `.kn-room-row`. `meta_map` is the SAME bot-name
/// -> `BotMeta` map the caller (`BotRoster`) already built from
/// `list_bot_meta()` — this component never issues its own fetch, and never
/// per-room fans out a lookup (D-13 fence, mirrors the roster's own
/// single-whole-map-read discipline).
#[component]
pub fn GroupRow(
    room: GroupRoomSummary,
    meta_map: BTreeMap<String, BotMeta>,
    on_open: EventHandler<String>,
) -> Element {
    let room_id = room.id.clone();
    let members_chip = format!("{}/6", room.members.len());
    let pattern_chip = pattern_chip_for(&room);

    rsx! {
        div {
            class: "kn-room-row",
            onclick: move |_| on_open.call(room_id.clone()),
            div { class: "kn-bot-card-head",
                div { class: "kn-room-avatar-cluster",
                    // Bounded at 6 — D-21's own room-size ceiling
                    // (defensive; membership is already validated [2,6] at
                    // creation).
                    for member in room.members.iter().take(6).cloned() {
                        {
                            let avatar = meta_map.get(&member).and_then(|m| m.avatar.clone());
                            let avatar_shape = avatar.as_ref().and_then(|a| a.shape.clone());
                            let avatar_color = avatar.as_ref().and_then(|a| a.color.clone());
                            let avatar_image_id = avatar.as_ref().and_then(|a| a.image_id.clone());
                            rsx! {
                                BotFace {
                                    key: "{member}",
                                    name: member.clone(),
                                    size: 20u32,
                                    shape: avatar_shape,
                                    color_token: avatar_color,
                                    image_id: avatar_image_id,
                                }
                            }
                        }
                    }
                }
                div { class: "kn-bot-card-title", "{room.name}" }
                span { class: "kn-chip", "data-kind": "members", "{members_chip}" }
                // Pattern chip (D-09) — conditional, TEAM rooms only; a
                // peer room omits it entirely (absence, not a dimmed
                // placeholder, matching this file's existing convention).
                if let Some((label, tooltip)) = pattern_chip {
                    span { class: "kn-chip", "data-kind": "pattern", title: "{tooltip}", "{label}" }
                }
                // Round chip — conditional, dispatching only; idle rows
                // omit it entirely (absence, not a dimmed placeholder).
                if let Some(n) = room.active_round {
                    span { class: "kn-chip", "data-kind": "round", "Round {n}" }
                }
                // Needs-you badge — conditional, absent (not dimmed) when
                // clear.
                if room.needs_you {
                    span { class: "kn-badge", "data-kind": "needs-you", "NEEDS YOU" }
                }
            }
            // A brand-new room with an empty transcript renders NO preview
            // line at all — absence, not placeholder text (UI-SPEC E4).
            if let Some(preview) = room.preview.clone() {
                div { class: "kn-bot-card-preview", "{preview}" }
            }
        }
    }
}

#[cfg(test)]
mod pattern_chip_for_tests {
    use super::*;

    fn summary(pattern: Option<TeamPattern>) -> GroupRoomSummary {
        GroupRoomSummary {
            id: "room-1".to_string(),
            name: "Room One".to_string(),
            members: vec!["alpha".to_string(), "beta".to_string()],
            group: None,
            needs_you: false,
            preview: None,
            preview_at_ms: None,
            active_round: None,
            pattern,
        }
    }

    /// `<behavior>` bullet 1: a team room's summary yields the chip, a peer
    /// room's summary yields `None` — the caller renders nothing at all in
    /// that case.
    #[test]
    fn a_team_room_summary_renders_the_pattern_chip_and_a_peer_room_renders_none() {
        let team = summary(Some(TeamPattern::OrchestratorWorkers));
        assert!(pattern_chip_for(&team).is_some());

        let peer = summary(None);
        assert!(pattern_chip_for(&peer).is_none());
    }

    /// `<behavior>` bullet 2: the label is the fixed literal regardless of
    /// pattern arm; the tooltip carries the full human-readable name.
    #[test]
    fn the_pattern_chip_label_is_the_fixed_literal_and_the_tooltip_carries_the_full_name() {
        let (label, tooltip) =
            pattern_chip_for(&summary(Some(TeamPattern::OrchestratorWorkers))).unwrap();
        assert_eq!(label, "TEAM");
        assert_eq!(tooltip, "Orchestrator-workers");
    }

    /// `<behavior>` bullet 3: a summary carrying `pattern: Some(..)` with no
    /// leader is the state a hand-edited or pre-demotion `group-rooms.json`
    /// can produce even though the write-path invariant (D-07) forbids
    /// writing one — `GroupRoomSummary` has no `roles` field of its own for
    /// this projection to depend on, so `pattern_chip_for` structurally
    /// cannot read (or panic on) roles data; this test pins that a bare
    /// `Some(pattern)` alone is sufficient.
    #[test]
    fn a_summary_with_a_pattern_and_no_leader_still_yields_a_chip() {
        let malformed = summary(Some(TeamPattern::OrchestratorWorkers));
        assert!(pattern_chip_for(&malformed).is_some());
    }
}
