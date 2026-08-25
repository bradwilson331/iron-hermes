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

use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::{BotMeta, GroupRoomSummary};
use dioxus::prelude::*;
use std::collections::BTreeMap;

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
