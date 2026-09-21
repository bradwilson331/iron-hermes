//! Phase 50.2 Plan 01 (UI-SPEC Component Inventory §6): the room workspace —
//! a full-panel view that REPLACES the roster grid in place (never a modal;
//! a multi-round transcript needs the vertical space a `.kn-modal`'s bounded
//! height cannot comfortably give it). `← Back to roster` returns to the
//! grid.
//!
//! Three flex children: `.kn-room-header` (back control, name, avatar
//! cluster, members chip), `.kn-room-transcript` (scrollable, one
//! `.kn-room-message` per transcript row), `.kn-room-composer` (pinned
//! bottom, matches `chat_window.rs`'s composer shape).
//!
//! **Resource idiom (D-10):** `use_resource` with a `refresh_tick`
//! sync-prefix — never `use_server_future` + `.restart()`. This component
//! owns its OWN local `refresh_tick: Signal<u32>` (rather than a
//! caller-supplied `ReadSignal`) since room creation/round-dispatch both
//! originate from actions taken entirely within this component's own
//! subtree; there is no external caller event that also needs to trigger
//! this refetch.
//!
//! **Signal-borrow discipline (clippy.toml):** [`submit_room_send`] copies
//! `bot_roster/chat_window.rs`'s `submit_send` shape verbatim — every value
//! read from a signal is extracted BEFORE `rsx!`; the write-lock taken to
//! toggle `sending`/clear `draft` is synchronous and released before
//! `spawn`; the round-dispatch result is applied via a FRESH `.set()` call
//! only after `dispatch_group_round(...).await` resolves — never a
//! `Signal::write()` guard held across that `.await`.
//!
//! **No context provider (D-10).** `meta_map` and `room_id` are plain
//! props, threaded down from `BotRoster` — this component opens no context
//! of its own.
//!
//! Phase 50.2 Plan 05 (D-21, UI-SPEC Component Inventory §6/§7): the room
//! header's settings gear mounts the shared `GroupSettingsDrawer` — this
//! component owns its OWN local `settings_open: Signal<bool>` (a plain
//! `use_signal`, never a context provider, D-10). Plan 06 adds the SECOND
//! entry point (the roster section header) when it owns `bot_roster.rs`,
//! mounting its own signal and its own instance of the same drawer
//! component — two entry points onto the same persisted record, not a
//! shared signal.
//!
//! Phase 50.2 Plan 06 (UI-SPEC Component Inventory §6, State Matrix): every
//! state D-21's round driver can produce is now legible in the transcript —
//! round dividers, dimmed pass rows, retrievable failure reasons, the
//! settled line, the dispatching lifecycle, and the needs-you badge.
//!
//! **Round-divider ordering contract.** [`group_messages_into_rounds`]
//! bands consecutive same-`round` messages together; the render side emits
//! exactly ONE `.kn-room-round-divider` per band, labelled with
//! [`round_divider_label`] using the max read from the settings record
//! (never a hardcoded `3`). Under the 2026-08-18 parallel-within-round
//! amendment, several messages can land under one divider in
//! join-completion order with NO serial precedence implied between them —
//! the divider is the transcript's ONLY ordering guarantee.
//!
//! **Pass/failure legibility (State Matrix, Phase 50.2 Plan 11 G-2).** A
//! member turn that failed (missing key, crash, timeout) renders its OWN
//! `data-status="failed"` row carrying the failure reason as visible body
//! text (`{bot} didn't respond this round: {reason}`) — dimmed like a pass
//! (a failure is still low-emphasis, not an alarm) but never byte-identical
//! to one. A `.kn-room-failure-marker` `ⓘ` glyph remains for the hover
//! affordance (`title`), but is `aria-hidden` now that the reason is a
//! real text node. Before this fix, a failed turn rendered the SAME
//! `data-status="pass"`/`(pass)` body a genuine pass did, and the reason
//! was retrievable only via a hover-only accessible label — an operator
//! drive (50.2-UAT-EVIDENCE.md §4 F-2) could not tell a failure from a
//! pass and finished the session with "no idea what's going on".
//!
//! **Dispatching lifecycle, no per-round streaming.** [`RoomDispatchState`]
//! names the workspace's one client-observable dispatch signal, derived from
//! an outstanding-dispatch COUNTER (`in_flight_dispatches`). A single
//! blocking round-trip covers the WHOLE multi-round drive — this crate has
//! no per-round/per-member streaming — so every room member's avatar is
//! treated as potentially in-flight (`data-mood="work"`) for the header
//! cluster's duration, rather than attempting to track a specific
//! member/round pair the client cannot truthfully observe. Message-head
//! avatars never carry this mood: a rendered message row only ever exists
//! for an ALREADY-RESOLVED turn (the transcript only updates once the whole
//! drive's `.await` resolves), so there is no in-flight row to paint it on.
//!
//! **Accept-and-queue composer, not a whole-drive lock (Phase 50.2 Plan 18,
//! G-50.2-2c).** The composer stays live for the ENTIRE duration of a
//! drive — the input is never `readonly`, and SEND disables only for an
//! empty draft. A send submitted while a drive is already running for this
//! room is accepted and QUEUED (`GroupRoundDispatch::Queued`) rather than
//! rejected or blocked at the input: [`crate::server::group_chat_api::dispatch_group_round`]
//! injects it into the room at the running drive's NEXT round boundary, and
//! an `@mention` inside it addresses that member through the room's own
//! existing mention resolution — no second addressing mechanism. Before this
//! fix, the composer's `readonly: is_sending` attribute spanned the whole
//! multi-round drive, and the round chip dispatching indicator was the
//! operator's only feedback with zero remaining control — the exact defect
//! G-50.2-2c was filed against, mirroring the Bot Chat composer's own
//! pre-Plan-16 hard lock.

use crate::components::hermes_app::screens::bot_roster::card::format_relative_timestamp;
use crate::components::hermes_app::screens::bot_roster::chat_window::{
    strip_think_blocks_for_display, BOT_THINK_ONLY_PLACEHOLDER,
};
use crate::components::hermes_app::screens::bot_roster::delete_room_confirm::DeleteRoomConfirm;
use crate::components::hermes_app::screens::bot_roster::edit_members_modal::EditMembersModal;
use crate::components::hermes_app::screens::bot_roster::group_settings::GroupSettingsDrawer;
use crate::components::hermes_app::screens::bot_roster::mention_handoff::{
    MentionHandoffBlock, MentionHandoffEntry,
};
use crate::components::hermes_app::screens::bot_roster::new_conversation_confirm::NewConversationConfirm;
use crate::components::hermes_app::widgets::bot_face::{seeded_color_for, BotFace};
use crate::protocol::{
    BotMeta, BotRosterEntry, DispatchGroupRoundRequest, GroupChatSettings, GroupRoomMessage,
    GroupRoomSpeaker, GroupRoomTranscript, MemberTurnStatus, MentionHandoffRequest,
    MentionHandoffState, TeamPattern, TeamRowKind, WorkerOutcomeKind,
};
use crate::server::group_chat_api::dispatch_group_round;
use crate::server::group_settings_api::load_group_chat_settings;
use crate::server::mention_handoff_api::{dispatch_mention_handoff, resolve_roster_mentions_client};
use dioxus::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// Phase 50.2 Plan 01: current wall-clock time in milliseconds.
/// `web_time::SystemTime` mirrors `bot_roster/card.rs`'s own `now_ms` —
/// duplicated here rather than widened, matching that module's own
/// documented precedent (this file also compiles for the wasm client
/// target, where `web_time` resolves to a `Performance`-backed shim).
//
// Under `--all-features`, `legacy-shell` swaps the reachable root component
// to `WarpHermes`, leaving `HermesApp` (and therefore `BotRoster` and every
// component this module's own `GroupChatWorkspace` is only ever reached
// through) unreferenced from `main()` for dead-code-lint purposes — even
// though this is live in the default (non-legacy-shell) build. Same
// pre-existing pattern as `bot_roster/card.rs`'s `now_ms`/
// `compose_preview_line` and `bot_roster/chat_window.rs`'s `submit_send`.
#[allow(dead_code)]
fn now_ms() -> i64 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 50.2 Plan 06: one round's messages, grouped by their `round`
/// field. Pure and disk/DOM-I/O-free so the grouping rule is directly
/// unit-testable without a renderer.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RoundBand {
    pub(crate) round: u32,
    pub(crate) messages: Vec<GroupRoomMessage>,
}

/// Phase 50.2 Plan 06 (UI-SPEC Component Inventory §6): consecutive
/// messages sharing the same `round` field become one band — the render
/// side emits exactly one `.kn-room-round-divider` per band, never once
/// per message. Grouping is by CONSECUTIVE equality (a plain run-length
/// fold), not by sorting or a global round-number index, so it stays
/// correct even across two separate operator-triggered drives whose round
/// numbering each restarts at 1 (`run_group_rounds`'s own per-call
/// counter, `group_chat_api.rs`). An empty message list returns no bands.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn group_messages_into_rounds(messages: &[GroupRoomMessage]) -> Vec<RoundBand> {
    let mut bands: Vec<RoundBand> = Vec::new();
    for msg in messages {
        match bands.last_mut() {
            Some(band) if band.round == msg.round => band.messages.push(msg.clone()),
            _ => bands.push(RoundBand {
                round: msg.round,
                messages: vec![msg.clone()],
            }),
        }
    }
    bands
}

/// Phase 50.2 Plan 06 (UI-SPEC Copywriting Contract: `Round {n} of {max}`).
/// Pure — `max` is an explicit parameter read from the settings record at
/// the render call site, never baked in here, so a hardcoded `3` can never
/// silently satisfy this fn's own test.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn round_divider_label(round: u32, max: u32) -> String {
    format!("Round {round} of {max}")
}

/// Phase 50.2 Plan 06: whether a round band's own MEMBER messages
/// (excluding the operator's kickoff row, which always shares round 1 with
/// that round's replies) contain zero non-pass, non-failed replies — the
/// render-side mirror of `group_chat_api::score_round`'s `settled` check.
/// Kept as its own fn here (rather than reusing `score_round` directly)
/// because that fn is `#[cfg(feature = "server")]` and this component also
/// compiles for the wasm client target.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn round_band_is_settled(band: &RoundBand) -> bool {
    let mut saw_member = false;
    for msg in &band.messages {
        if matches!(msg.from, GroupRoomSpeaker::Member(_)) {
            saw_member = true;
            if matches!(msg.status, MemberTurnStatus::Replied) {
                return false;
            }
        }
    }
    saw_member
}

/// Phase 52 Plan 09 (D-02/D-17, UI-SPEC Surface Contract 6): the room
/// overflow menu's entries, in their final top-to-bottom order — the menu
/// reads least- to most-consequential, and `New conversation`'s
/// reversibility sits between editing membership and permanent deletion.
/// A pure helper so the ordering is directly unit-testable without a
/// render harness, and the render side loops over it rather than
/// restating the order a second time.
///
/// `pattern` is accepted (a call site passes the room's actual
/// `GroupRoom.pattern`) but never branches this list — D-17 requires
/// `New conversation` present in EVERY room, peer and team alike, the
/// single explicit exception to peer rooms keeping 50.2's behaviour, so
/// there is no room-shape input this list could ever gate on.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn overflow_menu_entries(_pattern: Option<&TeamPattern>) -> [&'static str; 3] {
    ["Edit members", "New conversation", "Delete room"]
}

/// Phase 52 Plan 09 (D-15, UI-SPEC Surface Contract 5): the needs-you
/// advisory sentence — the room's persisted `needs_you_reason`, when and
/// only when `needs_you` is raised. The client composes NOTHING of its
/// own: the four sentences (cycle exhaustion, leader-contract failure,
/// worker failure, unprocessed queue) are server-side consts persisted by
/// the drive that raised the flag (`group_team_api.rs`'s own
/// `*_NEEDS_YOU_COPY` consts), so the copy has exactly one source. A
/// raised flag with no reason (a pre-Phase-52 record, or any future
/// `set_room_needs_you_impl` caller that omits one) yields the badge and
/// no advisory row, rather than rendering an empty row — never
/// `None` -> `Some("")`.
///
/// **Roster-row `title` withdrawn (Round 1 codex MEDIUM).** The reason
/// lives ONLY here, on the opened room — `GroupRoomSummary`
/// (`protocol.rs`) deliberately carries no reason field (Plan 03), and
/// the roster badge lives in `group_row.rs`, a different component in a
/// file this plan does not modify. Do not "restore" a roster-row tooltip
/// for this string; that would require widening `GroupRoomSummary`
/// against Plan 03's own recorded reasoning and would put a per-room
/// advisory string into a payload fetched on every roster paint.
///
/// **Never persisted as a transcript row.**
/// [`crate::server::group_chat_api::conversation_start_index`] treats the
/// LAST `GroupRoomSpeaker::System` row as the conversation boundary, so
/// writing this advisory as a System row would silently truncate every
/// room's replay on every failed drive. This fn returns a render-time
/// string only; no code path here ever constructs a
/// `GroupRoomSpeaker::System` message.
///
/// The helper's output depends only on `needs_you`/the reason — never on
/// `room.pattern` — so it renders identically for a peer room and a team
/// room, the D-15 hard lock (UI-SPEC §5). It also has no loading state of
/// its own: it is derived from already-persisted room state that arrived
/// with the transcript alongside the badge itself.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn needs_you_advisory_text(needs_you: bool, reason: Option<&str>) -> Option<String> {
    if !needs_you {
        return None;
    }
    reason.map(|r| r.to_string())
}

/// Phase 52 Plan 09 (Round 1 codex HIGH): the outcome-kind discriminant
/// [`dispatch_result_refreshes_room_state`] branches on — mirrors
/// `submit_room_send`'s own three match arms
/// (`GroupRoundDispatch::Ran`/`::Queued`/the dispatch's `Err` arm) without
/// requiring a real `Result<GroupRoundDispatch, ServerFnError>` value
/// (`ServerFnError` has no meaningful `PartialEq`/test-constructor), so
/// the branch decision stays directly unit-testable without a render
/// harness or a fabricated server round trip.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoomDispatchOutcomeKind {
    Ran,
    Queued,
    Err,
}

/// Phase 52 Plan 09 (D-15, Round 1 codex HIGH): whether a dispatch outcome
/// should bump `refresh_tick`. ALL THREE arms return `true` — a completed
/// drive (`Ran`) and a queued send (`Queued`) already did before this
/// plan; a FAILED dispatch (`Err`) now must too. A leader-contract failure
/// persists `needs_you`, its reason and any partial rows server-side, and
/// `submit_room_send`'s `Err` arm used to set only `dispatch_error` —
/// leaving every one of them invisible until an unrelated reload, hiding
/// the advisory this whole plan exists to surface on exactly the path it
/// serves (Round 1 codex HIGH, verified at
/// `group_chat_workspace.rs:389-401`).
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn dispatch_result_refreshes_room_state(outcome: RoomDispatchOutcomeKind) -> bool {
    match outcome {
        RoomDispatchOutcomeKind::Ran => true,
        RoomDispatchOutcomeKind::Queued => true,
        RoomDispatchOutcomeKind::Err => true,
    }
}

/// Phase 50.2 Plan 06: the room workspace's one client-observable dispatch
/// signal. A single blocking round-trip (`dispatch_group_round`) covers the
/// WHOLE multi-round drive — this crate has no per-round streaming — so
/// there is exactly one state transition to name, never a per-round enum.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoomDispatchState {
    Idle,
    Dispatching,
}

impl RoomDispatchState {
    #[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
    pub(crate) fn from_sending(is_sending: bool) -> Self {
        if is_sending {
            Self::Dispatching
        } else {
            Self::Idle
        }
    }

    #[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
    pub(crate) fn is_dispatching(self) -> bool {
        matches!(self, Self::Dispatching)
    }
}

/// Phase 50.2 Plan 07 (D-20, T-50.2-07-04): the room composer's handoff
/// targets — mentioned bots that are NOT room members. A mentioned MEMBER
/// is already the group driver's own responder-resolution job (plan 04);
/// dispatching a duplicate handoff here would double-spawn that bot for
/// the same message. Pure — delegates to `resolve_roster_mentions_client`.
///
/// Phase 50.2 Plan 20 (WR-01): there are TWO mention consumers built on top
/// of that same shared resolver, and each excludes the target its own
/// surface has already dispatched — this fn excludes room MEMBERS, because
/// the group driver's own responder resolution covers them; `chat_window.rs`'s
/// `mention_targets_for_bot_chat` excludes its window's own bot, because
/// `submit_send`'s primary path covers it. Neither surface may hand the same
/// message to the same bot twice; a prior version of this comment claimed
/// the Bot Chat consumer was deliberately unfiltered — that claim was the
/// reasoning error WR-01 traced the double-dispatch bug to.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn mention_targets_for_room(
    text: &str,
    roster_names: &[String],
    room_members: &[String],
    speaker: &str,
) -> Vec<String> {
    resolve_roster_mentions_client(text, roster_names, speaker)
        .into_iter()
        .filter(|target| !room_members.iter().any(|m| m.eq_ignore_ascii_case(target)))
        .collect()
}

/// Phase 50.2 Plan 07 (D-20): resolve the MOST RECENT still-pending entry
/// for `target` — same "last pending match" discipline as
/// `chat_window.rs`'s own `resolve_mention_entry`.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn resolve_room_mention_entry(
    entries: &mut [MentionHandoffEntry],
    target: &str,
    outcome: Result<crate::protocol::MentionHandoffResult, String>,
) {
    if let Some(entry) = entries
        .iter_mut()
        .rev()
        .find(|e| e.target == target && e.state == MentionHandoffState::Pending)
    {
        match outcome {
            Ok(result) => {
                entry.state = MentionHandoffState::Resolved;
                entry.reply = Some(result.reply);
            }
            Err(reason) => {
                entry.state = MentionHandoffState::Failed { reason };
            }
        }
    }
}

/// Phase 50.2 Plan 18 (G-50.2-2c): the operator-facing copy for a send
/// accepted while a drive is already running for this room — states that
/// the message will go into the room on the next round, and how many are
/// now waiting, with a singular form for exactly one. Pure, no I/O, directly
/// unit-testable. The room-level twin of `chat_window.rs`'s
/// `queued_for_next_turn_notice` (a room has no single bot name to name, so
/// this copy is depth-only).
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn queued_for_next_round_notice(depth: u32) -> String {
    if depth == 1 {
        "Message queued — it will go into the room on the next round.".to_string()
    } else {
        format!(
            "Message queued — it will go into the room on the next round ({depth} messages waiting)."
        )
    }
}

/// Phase 52 Plan 08 (D-14, Round 1 codex HIGH): mirrors
/// `group_team_api::resolve_cycle_budget`'s room-then-app-wide precedence
/// EXACTLY — duplicated locally rather than called directly, because that
/// fn lives in the `#[cfg(feature = "server")]` `group_team_api` module
/// (`server/mod.rs:274`) and this component also compiles for the wasm
/// client target with only the default `web` feature enabled (same
/// reasoning [`round_band_is_settled`]'s own doc comment already applies
/// to avoiding `group_chat_api::score_round`). A `#[cfg(feature =
/// "server")]` test asserts the two cannot drift — the `--all-features`
/// gate this crate's own verify commands always run under.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn resolve_cycle_budget(room_max_cycles: Option<u32>, settings_max_cycles: u32) -> u32 {
    room_max_cycles
        .unwrap_or(settings_max_cycles)
        .clamp(crate::protocol::TEAM_CYCLE_MIN, crate::protocol::TEAM_CYCLE_MAX)
}

/// Phase 52 Plan 08 (D-08/D-17, UI-SPEC Surface Contract 4): the header
/// chip's in-flight text. `None` whenever no drive is dispatching, for
/// both room kinds. A team room (`is_team_room`) gets the amended template
/// below; a peer room keeps the pre-Phase-52 rounds text byte-for-byte
/// (D-17) — this fn's `else` branch is that exact string, unchanged.
///
/// **Amendment to UI-SPEC Copywriting Contract line 159 (Round 1 codex
/// HIGH).** The contract's literal template is `Working — cycle
/// {n}/{max_cycles}`, but nothing in this crate can supply `{n}`:
/// `run_team_drive` is one blocking await, `transcript_resource` only
/// refetches once the dispatch resolves (module doc, [`RoomDispatchState`]),
/// and this file's own peer-room chip already renders `Round …/{max}` with
/// a comment explaining exactly this gap. Rendering a numeric current-cycle
/// would be the registered-but-fictional failure class D-01 exists to
/// prevent, moved from the schema into the UI. The ellipsis numerator is
/// used instead — the contracted noun, the contracted denominator and the
/// contracted pulse are all retained; only the unsuppliable numerator
/// changes.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn in_flight_chip_text(
    is_team_room: bool,
    is_dispatching: bool,
    cycle_budget: u32,
    settings_max_rounds: u32,
) -> Option<String> {
    if !is_dispatching {
        return None;
    }
    Some(if is_team_room {
        format!("Working — cycle …/{cycle_budget}")
    } else {
        format!("Round …/{settings_max_rounds}")
    })
}

/// Phase 52 Plan 08 (D-15): the composer's SEND-disable predicate — pure,
/// and taking no advisory-state input of any kind, so the invariant "no
/// dispatch gate keyed on a failed drive" is a fact about this fn's own
/// signature, not just its current body. The only input is whether the
/// draft is non-empty.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn composer_send_disabled(can_send: bool) -> bool {
    !can_send
}

/// Phase 52 Plan 08 (Copywriting Contract: `Queued for next cycle`): the
/// SAME notice as [`queued_for_next_round_notice`], room-kind aware —
/// `round` in a peer room, `cycle` in a team room. Reuses the same
/// singular/plural template rather than restating it, so the two can never
/// diverge in wording.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn queued_notice_for_room(depth: u32, is_team_room: bool) -> String {
    let noun = if is_team_room { "cycle" } else { "round" };
    if depth == 1 {
        format!("Message queued — it will go into the room on the next {noun}.")
    } else {
        format!(
            "Message queued — it will go into the room on the next {noun} ({depth} messages waiting)."
        )
    }
}

/// Phase 50.2 Plan 01: the send action, shared by the composer input's
/// Enter-to-send handler and the SEND button's click handler — a free fn
/// taking every dependency as an explicit param, copying
/// `chat_window.rs::submit_send`'s exact shape.
///
/// Phase 50.2 Plan 18 (G-50.2-2c): guards ONLY on an empty trimmed draft —
/// the already-sending term is gone (that term was the whole-drive hard lock
/// G-50.2-2c names). `in_flight_dispatches` is an outstanding-dispatch
/// COUNTER, not a single `bool` — same fix Plan 16 applied to the Bot Chat
/// composer, for the same reason: a queued send resolving must never
/// clobber a still-running drive's own dispatching indicator.
///
/// Phase 50.2 Plan 07 (D-20): `roster_names`/`room_members` feed
/// [`mention_targets_for_room`] — a pending handoff block is appended to
/// `room_mention_entries` for each non-member target, then every target is
/// dispatched in PARALLEL (2026-08-19 operator ruling), independently of
/// the room's own round-dispatch `spawn` below.
// 9 explicit params (vs. a bundled struct): matches this fn's own
// pre-existing shape (`chat_window.rs::submit_send`'s copied signature)
// plus the 3 Phase 50.2 Plan 07 additions (`roster_names`/`room_members`/
// `room_mention_entries`) plus Plan 18's `queued_notice` — introducing a
// params struct here would be a larger refactor than this task's own scope.
#[allow(clippy::too_many_arguments)]
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn submit_room_send(
    room_id: String,
    mut draft: Signal<String>,
    mut in_flight_dispatches: Signal<u32>,
    mut queued_notice: Signal<Option<u32>>,
    mut dispatch_error: Signal<Option<String>>,
    mut refresh_tick: Signal<u32>,
    roster_names: Vec<String>,
    room_members: Vec<String>,
    mut room_mention_entries: Signal<Vec<MentionHandoffEntry>>,
) {
    let text = draft.read().clone();
    if text.trim().is_empty() {
        return;
    }
    draft.set(String::new());
    let current_in_flight = *in_flight_dispatches.read();
    in_flight_dispatches.set(current_in_flight + 1);
    dispatch_error.set(None);

    let mention_targets = mention_targets_for_room(&text, &roster_names, &room_members, "operator");
    {
        let mut entries = room_mention_entries.write();
        for target in &mention_targets {
            entries.push(MentionHandoffEntry {
                target: target.clone(),
                message: text.clone(),
                state: MentionHandoffState::Pending,
                reply: None,
            });
        }
    }
    for target in mention_targets {
        let message = text.clone();
        let mut entries_for_mention = room_mention_entries;
        spawn(async move {
            let outcome = dispatch_mention_handoff(MentionHandoffRequest {
                target: target.clone(),
                message,
            })
            .await
            .map_err(|e| e.to_string());
            let mut entries = entries_for_mention.write();
            resolve_room_mention_entry(&mut entries, &target, outcome);
        });
    }

    spawn(async move {
        let result = dispatch_group_round(DispatchGroupRoundRequest {
            room_id,
            message: text,
        })
        .await;
        // Fresh `.set()` calls only, acquired after the await resolves —
        // never a write-lock guard held across it.
        //
        // Phase 50.2 Plan 18 (G-50.2-2c): `dispatch_group_round` now returns
        // `GroupRoundDispatch`. The `Ran` arm does exactly what the
        // pre-Plan-18 `Ok` arm did (bump `refresh_tick`) and additionally
        // clears `queued_notice` — a completed drive means any prior queued
        // notice's message has already been injected and is now part of the
        // fetched transcript. The `Queued` arm sets `queued_notice` to the
        // returned depth AND ALSO bumps `refresh_tick`, so the transcript
        // refetch picks up the injected row once the still-running round
        // persists it.
        match result {
            Ok(crate::protocol::GroupRoundDispatch::Ran(_)) => {
                let cur = *refresh_tick.read();
                refresh_tick.set(cur + 1);
                queued_notice.set(None);
            }
            Ok(crate::protocol::GroupRoundDispatch::Queued { depth }) => {
                queued_notice.set(Some(depth));
                let cur = *refresh_tick.read();
                refresh_tick.set(cur + 1);
            }
            // Phase 52 Plan 09 (Round 1 codex HIGH): a FAILED dispatch now
            // bumps `refresh_tick` too — the error text explains the
            // immediate failure, and the refresh surfaces the persisted
            // `needs_you`/reason/partial rows a leader-contract failure
            // already wrote server-side before this drive ever returned.
            Err(e) => {
                dispatch_error.set(Some(format!("{e}")));
                if dispatch_result_refreshes_room_state(RoomDispatchOutcomeKind::Err) {
                    let cur = *refresh_tick.read();
                    refresh_tick.set(cur + 1);
                }
            }
        }
        let remaining_in_flight = in_flight_dispatches.read().saturating_sub(1);
        in_flight_dispatches.set(remaining_in_flight);
    });
}

/// Phase 50.2 Plan 01: one transcript row's rendered sender label + BotFace,
/// pure render logic split out only so the `rsx!` block below stays legible
/// (`GroupRoomSpeaker` is a protocol DTO, not a Dioxus-owned type).
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn speaker_display_name(speaker: &GroupRoomSpeaker) -> String {
    match speaker {
        GroupRoomSpeaker::Operator => "You".to_string(),
        GroupRoomSpeaker::Member(name) => name.clone(),
        GroupRoomSpeaker::System => "System".to_string(),
    }
}

/// Phase 50.2 Plan 11 (G-2): one message row's rendered body text. A
/// `Passed` turn still renders the literal Copywriting Contract `(pass)`
/// string. A `Failed { reason }` turn now renders that reason as the row's
/// VISIBLE body — built by [`failure_marker_label`], the same builder that
/// also produces the marker's `title` tooltip, so there is exactly one copy
/// source. Before this fix, both outcomes rendered the SAME `(pass)` body
/// and a failed turn's actual reason was retrievable only via the
/// `.kn-room-failure-marker`'s accessible label — a real operator drive
/// (50.2-UAT-EVIDENCE.md §4 F-2) could not tell a failure from a genuine
/// pass without hovering, and finished the session with "no idea what's
/// going on". `Replied` is unaffected by this task (Plan 11 Task 2 strips
/// reasoning blocks from it).
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn message_display_text(msg: &GroupRoomMessage, sender_name: &str) -> String {
    // Phase 52 Plan 08 (D-10/D-11, UI-SPEC Copywriting Contract): a
    // `Blocked` worker outcome is persisted with `MemberTurnStatus::Replied`
    // (`worker_outcome_row`'s own doc comment — the worker's self-reported
    // summary lives in `msg.text`, not in a `Failed { reason }` variant), so
    // the match below on `msg.status` alone can never distinguish it from
    // an ordinary reply. This early return is the SAME visible-body-text
    // path the `Failed` arm below already takes — wired here rather than
    // duplicated, so blocked can never silently drop to a bare unlabeled
    // reply.
    if let Some(TeamRowKind::WorkerResult { outcome: WorkerOutcomeKind::Blocked }) = &msg.team_row {
        return blocked_marker_label(sender_name, &msg.text);
    }
    match &msg.status {
        MemberTurnStatus::Passed => "(pass)".to_string(),
        MemberTurnStatus::Failed { reason } => failure_marker_label(sender_name, reason),
        // Phase 50.2 Plan 11 (G-3): shares chat_window.rs's
        // strip_think_blocks_for_display/BOT_THINK_ONLY_PLACEHOLDER — the
        // same two-step rule ChatWindowEntry::display_text applies (strip,
        // then fall back to the placeholder when the raw text was
        // non-empty but strips to whitespace) — rather than reimplementing
        // it, so the two surfaces can never drift. Before this fix, the
        // room transcript was the one reply surface in this crate with no
        // think-stripping call, leaking raw chain-of-thought
        // (50.2-VERIFICATION.md's G-3 gap entry).
        MemberTurnStatus::Replied => {
            let stripped = strip_think_blocks_for_display(&msg.text);
            if stripped.trim().is_empty() && !msg.text.trim().is_empty() {
                BOT_THINK_ONLY_PLACEHOLDER.to_string()
            } else {
                stripped
            }
        }
    }
}

/// Phase 50.2 Plan 06 (State Matrix: "renders as a pass-equivalent row"):
/// a true pass and a failed turn both suppress the identity border color —
/// a failed turn is not the bot speaking, so it carries no identity
/// emphasis either. Still drives `border_style` at the render site; the
/// row's `data-status` attribute (which DOES distinguish pass from failed,
/// Phase 50.2 Plan 11 G-2) is decided separately by
/// [`message_status_attr`].
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn message_is_pass_equivalent(msg: &GroupRoomMessage) -> bool {
    matches!(
        msg.status,
        MemberTurnStatus::Passed | MemberTurnStatus::Failed { .. }
    )
}

/// Phase 50.2 Plan 11 (G-2): the row's `data-status` attribute value — a
/// three-way split where `Passed` and `Failed` are now pairwise DISTINCT
/// (previously both were `"pass"`, which is exactly what let a failure
/// render as `(pass)` unnoticed). `Replied` carries no status attribute at
/// all, matching the render site's existing conditional-attribute idiom.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn message_status_attr(msg: &GroupRoomMessage) -> Option<&'static str> {
    match &msg.status {
        MemberTurnStatus::Passed => Some("pass"),
        MemberTurnStatus::Failed { .. } => Some("failed"),
        MemberTurnStatus::Replied => None,
    }
}

/// Phase 50.2 Plan 06 (UI-SPEC Copywriting Contract: failed-turn inline
/// marker tooltip): `{bot name} didn't respond this round: {error
/// summary}`. Pure — the caller supplies the failure's `reason` string,
/// never this fn reaching into the message itself.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn failure_marker_label(bot_name: &str, reason: &str) -> String {
    format!("{bot_name} didn't respond this round: {reason}")
}

/// Phase 50.2 Plan 06: a message's identity border-left color token — the
/// bot's own stored avatar color when set, else the SAME deterministic
/// `seeded_color_for` fallback `BotFace` itself uses, so a message's border
/// always matches that member's own avatar color exactly (50.1's existing
/// 8-swatch identity palette, never a new color).
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn identity_border_token(avatar_color: Option<&str>, member_name: &str) -> String {
    avatar_color
        .map(|c| c.to_string())
        .unwrap_or_else(|| seeded_color_for(member_name).to_string())
}

/// Phase 50.2 Plan 17 (G-50.2-2a, RED stub): the room header's member-name
/// text element — the primary fix for "no member name affordance" (this
/// phase already learned once, in plan 11's G-2 work, that an affordance
/// reachable only by hovering is not an affordance that surfaced). Up to
/// `max_shown` names join in the room's OWN order; beyond that the first
/// `max_shown` join and an overflow marker naming the remaining count is
/// appended. Bounded by the SAME ceiling the avatar cluster's own
/// `.take(6)` uses, so the two elements can never disagree. Pure,
/// disk/DOM-I/O-free.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn room_member_names_label(members: &[String], max_shown: usize) -> String {
    if members.is_empty() {
        return String::new();
    }
    let shown: Vec<&str> = members.iter().take(max_shown).map(String::as_str).collect();
    let mut label = shown.join(", ");
    if members.len() > max_shown {
        let overflow = members.len() - max_shown;
        label.push_str(&format!(" +{overflow} more"));
    }
    label
}

// -------------------------------------------------------------------
// Phase 52 Plan 08 (D-10/D-11): GREEN phase (Task 1). The delegation row,
// the worker-result fold, and their supporting pure fns. Kept together, in
// this order, so the grouping pass and its two readers (the fold summary
// accessor, the worker status/body-text helpers) read as one unit.
// -------------------------------------------------------------------

/// Phase 52 Plan 08 (D-10): a message's `team_row` — a small accessor kept
/// as its own fn only so [`group_transcript_rows`] and every render-site
/// reader share one read path rather than each spelling
/// `msg.team_row.as_ref()` themselves.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn team_row_kind_of(msg: &GroupRoomMessage) -> Option<&TeamRowKind> {
    msg.team_row.as_ref()
}

/// Phase 52 Plan 08 (D-10/D-11, Round 1 codex HIGH): the delegation row's
/// SERVER-COMPOSED fold summary, read from the `TeamRowKind::Delegation`
/// payload and rendered verbatim — never recounted client-side.
/// `GroupRoomMessage` has exactly one text field (`protocol.rs`'s own doc
/// comment) and the delegation row's own body already occupies it, so the
/// fold summary has nowhere else to live; a client-side recount would also
/// silently diverge from `group_team_api::fold_summary_text` the moment a
/// row is trimmed out of the window. `None` for every other row kind.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn delegation_fold_summary_of(msg: &GroupRoomMessage) -> Option<&str> {
    match team_row_kind_of(msg) {
        Some(TeamRowKind::Delegation { fold_summary }) => Some(fold_summary.as_str()),
        _ => None,
    }
}

/// Phase 52 Plan 08 (D-10/D-11): one transcript row after the delegation-
/// grouping pass. `children` is non-empty only for a `TeamRowKind::
/// Delegation` row (its worker sub-rows); every other row kind — including
/// every row a peer room ever produces (D-17) — carries an empty
/// `children`, so a flat pre-Phase-52 transcript renders through the exact
/// same one-row-per-message shape unchanged.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GroupedTranscriptRow {
    pub(crate) message: GroupRoomMessage,
    pub(crate) children: Vec<GroupRoomMessage>,
}

/// Phase 52 Plan 08 (D-10/D-11, Round 1 codex MEDIUM): a pure pre-render
/// pass folding each `TeamRowKind::WorkerResult` row into the children of
/// the NEAREST preceding `TeamRowKind::Delegation` row — mirrors
/// [`group_messages_into_rounds`]'s own shape (a pure fn with its own
/// tests, called once before the render loop rather than branching inside
/// it). It must not re-derive the delegation row's text or fold summary —
/// the server composed both; this fn only regroups what it is handed.
///
/// **Bounded at three boundaries (Round 1 codex suggestion).** "Nearest
/// preceding delegation row" alone would let a malformed, truncated or
/// reordered transcript attach a worker row to a delegation it has nothing
/// to do with — including one from a previous conversation. The open
/// delegation resets to `None` when the scan crosses an Operator row, a
/// `TeamRowKind::Synthesis` row, or a `GroupRoomSpeaker::System` row (D-02's
/// conversation marker). A worker row with no open delegation (none seen
/// yet, or past a boundary) renders at top level rather than being dropped
/// or panicking — `history_limit` can truncate a transcript mid-delegation,
/// so this is a reachable state, not defensive padding.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
pub(crate) fn group_transcript_rows(messages: &[GroupRoomMessage]) -> Vec<GroupedTranscriptRow> {
    let mut rows: Vec<GroupedTranscriptRow> = Vec::new();
    let mut open_delegation: Option<usize> = None;
    for msg in messages {
        let is_boundary = matches!(msg.from, GroupRoomSpeaker::Operator | GroupRoomSpeaker::System)
            || matches!(team_row_kind_of(msg), Some(TeamRowKind::Synthesis));
        if is_boundary {
            open_delegation = None;
        }
        match team_row_kind_of(msg) {
            Some(TeamRowKind::Delegation { .. }) => {
                rows.push(GroupedTranscriptRow { message: msg.clone(), children: Vec::new() });
                open_delegation = Some(rows.len() - 1);
            }
            Some(TeamRowKind::WorkerResult { .. }) => match open_delegation {
                Some(idx) => rows[idx].children.push(msg.clone()),
                None => rows.push(GroupedTranscriptRow { message: msg.clone(), children: Vec::new() }),
            },
            _ => rows.push(GroupedTranscriptRow { message: msg.clone(), children: Vec::new() }),
        }
    }
    rows
}

/// Phase 52 Plan 08 (D-10): a worker sub-row's `data-status` attribute,
/// keyed on the SERVER-CLASSIFIED [`WorkerOutcomeKind`] rather than
/// `MemberTurnStatus` — a `Blocked` row is persisted with
/// `MemberTurnStatus::Replied` (`worker_outcome_row`'s own doc comment in
/// `group_team_api.rs`), so [`message_status_attr`] alone can never
/// distinguish it from an ordinary reply. `Completed` emits no attribute —
/// a worker that produced a usable result is not degraded data and must
/// not be dimmed (UI-SPEC Color table). `Failed` reuses the existing
/// `"failed"` value (Pitfall 4 / D-05: infra failure and unparseable
/// output share ONE "didn't produce usable output" shape). `Blocked` emits
/// the new `"blocked"` value Plan 07's CSS keys the amber border on.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn worker_status_attr(outcome: &WorkerOutcomeKind) -> Option<&'static str> {
    match outcome {
        WorkerOutcomeKind::Completed => None,
        WorkerOutcomeKind::Failed => Some("failed"),
        WorkerOutcomeKind::Blocked => Some("blocked"),
    }
}

/// Phase 52 Plan 08 (D-10/D-11, UI-SPEC Copywriting Contract): mirrors
/// [`failure_marker_label`]'s shape for a `Blocked` worker outcome — `msg`
/// is the worker's OWN self-reported reason it couldn't complete the task
/// (`worker_outcome_row` persists a blocked row's `result.summary` as
/// plain body text, never wrapped further server-side), surfaced here
/// verbatim, never hover-only.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn blocked_marker_label(worker_name: &str, reason: &str) -> String {
    format!("{worker_name} reported it couldn't complete this task: {reason}")
}

/// Phase 52 Plan 08 (D-10/D-11): whether a worker outcome's reason must
/// reach the row's VISIBLE body text rather than staying hover-only — true
/// for `Failed` and `Blocked`, false for `Completed` (a usable result is
/// not degraded data, so it has no "reason" to surface). Directly tested
/// so a future change that special-cases `Blocked` back out of the
/// `Failed`-established visible-body-text path is caught without needing a
/// live drive to notice.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn worker_outcome_reason_is_visible_body_text(outcome: &WorkerOutcomeKind) -> bool {
    !matches!(outcome, WorkerOutcomeKind::Completed)
}

/// Phase 52 Plan 08 (D-10/D-11): renders ONE `.kn-room-message` row — the
/// existing single-message markup (Phase 50.2 Plans 06/11/17), factored
/// into its own fn so the delegation row, an ordinary top-level row and a
/// worker sub-row inside `.kn-room-worker-fold` all render through the
/// SAME markup rather than a second, divergent copy (Round 1 codex HIGH:
/// no client-side re-derivation of anything the server already composed).
/// A plain fn, not a `#[component]` — it uses no hooks and is called like
/// any other Rust fn from inside the transcript `for` loop below, never
/// mounted via `ComponentName { .. }` syntax.
#[allow(dead_code)] // see now_ms's doc note above (legacy-shell reachability)
fn transcript_message_row(
    msg: &GroupRoomMessage,
    meta_map: &BTreeMap<String, BotMeta>,
    now: i64,
    dom_key: String,
) -> Element {
    let GroupRoomMessage { from, at_ms, status, .. } = msg;
    let sender_name = speaker_display_name(from);
    let display_text = message_display_text(msg, &sender_name);
    let is_pass_equivalent = message_is_pass_equivalent(msg)
        || matches!(
            &msg.team_row,
            Some(TeamRowKind::WorkerResult { outcome: WorkerOutcomeKind::Blocked })
        );
    let status_attr = match &msg.team_row {
        Some(TeamRowKind::WorkerResult { outcome }) => worker_status_attr(outcome),
        _ => message_status_attr(msg),
    };
    // Phase 52 Plan 08: the SAME visible-body-text/hover-marker split Plan
    // 06/11 established for a failed peer turn now also covers a blocked
    // worker sub-row — never hover-only (D-10/D-11 must-have).
    let marker_label: Option<String> = if let Some(TeamRowKind::WorkerResult { outcome }) = &msg.team_row {
        if worker_outcome_reason_is_visible_body_text(outcome) {
            match outcome {
                WorkerOutcomeKind::Blocked => Some(blocked_marker_label(&sender_name, &msg.text)),
                _ => match status {
                    MemberTurnStatus::Failed { reason } => Some(failure_marker_label(&sender_name, reason)),
                    _ => None,
                },
            }
        } else {
            None
        }
    } else {
        match status {
            MemberTurnStatus::Failed { reason } => Some(failure_marker_label(&sender_name, reason)),
            _ => None,
        }
    };
    let member_avatar = match from {
        GroupRoomSpeaker::Member(name) => meta_map.get(name).and_then(|m| m.avatar.clone()),
        _ => None,
    };
    let border_style = if is_pass_equivalent {
        String::new()
    } else if let GroupRoomSpeaker::Member(name) = from {
        let token = identity_border_token(
            member_avatar.as_ref().and_then(|a| a.color.as_deref()),
            name,
        );
        format!("border-left-color: var({token});")
    } else {
        String::new()
    };
    rsx! {
        div {
            key: "{dom_key}",
            class: "kn-room-message",
            "data-status": status_attr,
            style: "{border_style}",
            div { class: "kn-room-message-head",
                if let GroupRoomSpeaker::Member(name) = from {
                    BotFace {
                        name: name.clone(),
                        size: 20u32,
                        shape: member_avatar.as_ref().and_then(|a| a.shape.clone()),
                        color_token: member_avatar.as_ref().and_then(|a| a.color.clone()),
                        image_id: member_avatar.as_ref().and_then(|a| a.image_id.clone()),
                    }
                }
                span {
                    style: "font-size: var(--fs-11); font-weight: 700;",
                    "{sender_name}"
                }
                span {
                    style: "font-size: var(--fs-11); color: var(--fg-dim);",
                    "{format_relative_timestamp(*at_ms, now)}"
                }
                if let Some(label) = &marker_label {
                    span {
                        class: "kn-room-failure-marker",
                        "aria-hidden": "true",
                        title: "{label}",
                        "ⓘ"
                    }
                }
            }
            div { "{display_text}" }
        }
    }
}

/// Phase 50.2 Plan 01: the room workspace. `room_id` is expected to be
/// mounted with `key: "{room_id}"` at the call site (`bot_roster.rs`) so a
/// drill-in to a DIFFERENT room fully remounts this component with fresh
/// `use_resource`/`Signal` state, rather than needing this component to
/// track a changing prop itself.
#[component]
pub fn GroupChatWorkspace(
    room_id: String,
    meta_map: BTreeMap<String, BotMeta>,
    // Phase 50.2 Plan 07 (D-20): every actual bot in the roster —
    // `mention_targets_for_room`'s roster filter, so a typo or unrelated
    // `@handle` never dispatches a subprocess.
    roster_names: Vec<String>,
    // Phase 50.2 Plan 17 (G-50.2-2a): the SAME joined roster `BotRoster`
    // already computed — `EditMembersModal`'s picker source. This
    // component issues no `list_profiles`/`list_bot_meta` fetch of its own.
    roster_entries: Vec<BotRosterEntry>,
    on_back: EventHandler<()>,
) -> Element {
    let refresh_tick: Signal<u32> = use_signal(|| 0);
    let room_id_for_fetch = room_id.clone();
    let transcript_resource = use_resource(move || {
        let _tick = refresh_tick();
        let room_id = room_id_for_fetch.clone();
        async move { crate::server::group_chat_api::load_group_room(room_id).await }
    });

    // Phase 52 Plan 08: bumped by `GroupSettingsDrawer`'s `on_saved`
    // callback (added by Plan 07, wired here) so a save made from THIS
    // room's own gear icon refreshes `settings_resource` instead of
    // leaving the header chip's denominator stale until the next
    // unrelated remount.
    let mut settings_tick: Signal<u32> = use_signal(|| 0u32);
    // Phase 50.2 Plan 06 (D-21 integration): the persisted settings record,
    // read only for its `max_rounds` — the round divider's `Round {n} of
    // {max}` label. Falls back to `GroupChatSettings::default().max_rounds`
    // while loading or on a load error, matching the driver's own
    // corrupted-record fallback (never a bare hardcoded `3`).
    let settings_resource = use_resource(move || {
        let _settings_tick = settings_tick();
        async move { load_group_chat_settings().await }
    });

    let mut draft: Signal<String> = use_signal(String::new);
    // Phase 50.2 Plan 18 (G-50.2-2c): an outstanding-dispatch COUNTER, not a
    // single `bool` — two concurrent dispatches (a queued send resolving
    // while a fresh one is submitted) must never clobber each other's
    // state. Same fix Plan 16 applied to the Bot Chat composer's
    // `in_flight_sends`.
    let in_flight_dispatches: Signal<u32> = use_signal(|| 0u32);
    // Phase 50.2 Plan 18 (G-50.2-2c): the most recent queued-for-next-round
    // depth. Cleared when a `Ran` outcome lands — a completed drive means
    // the message that mattered to this notice has already been injected.
    let queued_notice: Signal<Option<u32>> = use_signal(|| None);
    let dispatch_error: Signal<Option<String>> = use_signal(|| None);

    // Phase 50.2 Plan 05 (D-10): the settings drawer's own open flag, owned
    // by THIS component — never a context provider, never threaded from
    // `BotRoster`.
    let mut settings_open: Signal<bool> = use_signal(|| false);

    // Phase 50.2 Plan 06 (UI-SPEC Component Inventory §6/§8): the room
    // header's `⋯` overflow menu (`Delete room`, mirrors `card.rs`'s own
    // per-card overflow idiom — local, per-instance state, never a context
    // provider) and the Delete Room confirmation modal's own open flag.
    let mut overflow_open: Signal<bool> = use_signal(|| false);
    let mut delete_confirm_open: Signal<bool> = use_signal(|| false);
    // Phase 50.2 Plan 17 (G-50.2-2a, D-10): the `Edit members` modal's own
    // open flag — a plain `use_signal`, never a context provider, same
    // ownership placement as `delete_confirm_open` above.
    let mut edit_members_open: Signal<bool> = use_signal(|| false);
    // Phase 52 Plan 09 (D-02): the `New conversation` confirm's own open
    // flag — same ownership placement as `delete_confirm_open`/
    // `edit_members_open` above, never a context provider.
    let mut new_conversation_open: Signal<bool> = use_signal(|| false);

    // Phase 50.2 Plan 07 (D-20): client-side, session-transient handoff
    // blocks for bots mentioned in this room who are NOT room members —
    // rendered supplementally alongside the persisted round-banded
    // transcript (never written into `GroupRoomMessage` itself, which has
    // no handoff-block kind). Owned here, never a context provider, same
    // ownership placement as every other Signal this component holds.
    let room_mention_entries: Signal<Vec<MentionHandoffEntry>> = use_signal(Vec::new);
    // Phase 52 Plan 08 (D-10, Round 1 codex MEDIUM): the fold-toggle
    // expansion state for EVERY delegation row in this transcript — ONE
    // parent-level signal, never a per-row `use_signal` (the transcript
    // loop iterates a variable-length band list, so a per-row hook would
    // register a changing number of hooks between renders, the
    // unconditional-hook rule this crate has already shipped two
    // regressions against). Keyed by a delegation row's position in the
    // flattened, grouped row sequence — a `history_limit` trim can shift
    // that position, so an expanded fold can appear to move after a trim;
    // an accepted cost for a purely presentational toggle with no
    // persisted key.
    let mut expanded_delegations: Signal<BTreeSet<usize>> = use_signal(BTreeSet::new);

    // ---- Derived values (read BEFORE rsx!, clippy.toml discipline). ----
    let is_loading = transcript_resource().is_none();
    let load_failed = matches!(transcript_resource(), Some(Err(_)));
    let transcript: Option<GroupRoomTranscript> = match transcript_resource() {
        Some(Ok(t)) => Some(t),
        _ => None,
    };
    let settings_max_rounds: u32 = match settings_resource() {
        Some(Ok(s)) => s.max_rounds,
        _ => GroupChatSettings::default().max_rounds,
    };
    // Phase 52 Plan 08 (D-14): the app-wide cycle budget, mirroring
    // `settings_max_rounds`'s own loading/error fallback — never a bare
    // hardcoded literal.
    let settings_max_cycles: u32 = match settings_resource() {
        Some(Ok(s)) => s.max_cycles,
        _ => GroupChatSettings::default().max_cycles,
    };
    // Phase 52 Plan 08 (D-17): this room's kind, read once — governs the
    // header chip's wording and the queued-notice noun. `false` while the
    // transcript is still loading, matching the header's own "existing
    // idle chrome" contract (E4) for that window.
    let is_team_room: bool = transcript.as_ref().map(|t| t.room.pattern.is_some()).unwrap_or(false);
    // Phase 52 Plan 09 (D-15): the advisory row's text, computed BEFORE
    // `rsx!` per this crate's signal-borrow discipline — `None` renders
    // nothing (no badge state, or a raised flag with no persisted reason).
    let needs_you_advisory: Option<String> = transcript
        .as_ref()
        .and_then(|t| needs_you_advisory_text(t.room.needs_you, t.room.needs_you_reason.as_deref()));
    // Phase 50.2 Plan 07 (D-20): this room's current membership, for
    // `mention_targets_for_room`'s member-exclusion filter at both
    // `submit_room_send` call sites below.
    let room_members: Vec<String> = transcript
        .as_ref()
        .map(|t| t.room.members.clone())
        .unwrap_or_default();
    // Phase 50.2 Plan 17 (G-50.2-2a): the header's member-name text
    // element — computed BEFORE `rsx!` using the SAME six-member ceiling
    // the avatar cluster's `.take(6)` already uses, so the two elements can
    // never disagree.
    let member_names_label = room_member_names_label(&room_members, 6);
    let draft_val = draft.read().clone();
    let is_sending = *in_flight_dispatches.read() > 0;
    let dispatch_state = RoomDispatchState::from_sending(is_sending);
    // Phase 50.2 Plan 18 (G-50.2-2c): sendable throughout an in-flight
    // drive — only an empty draft disables SEND. The composer input itself
    // is never `readonly` any more (see the input element below).
    let can_send = !draft_val.trim().is_empty();
    let dispatch_error_val = dispatch_error.read().clone();
    let queued_notice_val = *queued_notice.read();
    let send_label = if is_sending { "DISPATCHING…" } else { "SEND" };
    let now = now_ms();
    // Every room member's avatar is treated as potentially in-flight for
    // the header cluster's duration (see module doc: no per-round/
    // per-member streaming exists to distinguish which member is actually
    // mid-turn).
    let header_mood: Option<String> = if dispatch_state.is_dispatching() {
        Some("work".to_string())
    } else {
        None
    };

    let bands: Vec<RoundBand> = transcript
        .as_ref()
        .map(|t| group_messages_into_rounds(&t.messages))
        .unwrap_or_default();
    // A settled drive's LAST band is the one the driver stopped on (only
    // the round that ends a drive can ever score `spoke == 0` — an earlier
    // band would have stopped the drive there instead).
    let last_band_settled = bands.last().map(round_band_is_settled).unwrap_or(false);
    // Phase 52 Plan 08 (D-10/D-11): the delegation-grouping pass, applied
    // PER BAND rather than once over the whole transcript — a delegation,
    // its worker results and its synthesis all share one `round`/cycle
    // number (`group_team_api.rs`'s `worker_outcome_row`/`run_team_drive`),
    // so a delegation's children never span a band boundary in practice,
    // and this keeps the existing round-divider machinery (`bands`,
    // `last_band_settled`) untouched.
    let grouped_bands: Vec<Vec<GroupedTranscriptRow>> = bands
        .iter()
        .map(|band| group_transcript_rows(&band.messages))
        .collect();
    // Phase 52 Plan 08 (D-10): each band's own `(row_i)` turned into the
    // GLOBAL position `expanded_delegations` is keyed on — two different
    // bands' `row_i == 0` would otherwise collide.
    let band_row_offsets: Vec<usize> = {
        let mut offsets = Vec::with_capacity(grouped_bands.len());
        let mut running = 0usize;
        for rows in &grouped_bands {
            offsets.push(running);
            running += rows.len();
        }
        offsets
    };

    rsx! {
        div { class: "kn-room-workspace",
            div { class: "kn-room-header",
                button {
                    class: "kn-action-btn",
                    onclick: move |_| on_back.call(()),
                    "← Back to roster"
                }
                if let Some(t) = &transcript {
                    h2 {
                        style: "font-size: var(--fs-18); font-weight: 700; margin: 0;",
                        "{t.room.name}"
                    }
                    div { class: "kn-room-avatar-cluster",
                        // Bounded at 6 — D-21's own room-size ceiling
                        // (defensive; membership is already validated
                        // [2,6] at creation).
                        for member in t.room.members.iter().take(6).cloned() {
                            {
                                let avatar = meta_map.get(&member).and_then(|m| m.avatar.clone());
                                let avatar_shape = avatar.as_ref().and_then(|a| a.shape.clone());
                                let avatar_color = avatar.as_ref().and_then(|a| a.color.clone());
                                let avatar_image_id = avatar.as_ref().and_then(|a| a.image_id.clone());
                                let mood = header_mood.clone();
                                // Phase 50.2 Plan 17 (G-50.2-2a): a
                                // per-glyph accessible-name/hover
                                // affordance alongside the text element
                                // below (the primary fix).
                                let title = Some(member.clone());
                                rsx! {
                                    BotFace {
                                        key: "{member}",
                                        name: member.clone(),
                                        size: 32u32,
                                        shape: avatar_shape,
                                        color_token: avatar_color,
                                        image_id: avatar_image_id,
                                        mood,
                                        title,
                                    }
                                }
                            }
                        }
                    }
                    // Phase 50.2 Plan 17 (G-50.2-2a): the header's
                    // member-name text — the primary fix (a hover-only
                    // affordance is not an affordance that surfaced, per
                    // plan 11's G-2 lesson). Truncates by CSS rather than
                    // displacing the gear/overflow controls.
                    span { class: "kn-room-member-names", "{member_names_label}" }
                    span {
                        class: "kn-chip",
                        "data-kind": "members",
                        "{t.room.members.len()}/6"
                    }
                    // Round/cycle chip — conditional, dispatching only
                    // (UI-SPEC Component Inventory §6, Surface Contract 4).
                    // `in_flight_chip_text` is the single place the D-17
                    // room-kind split and the Round 1 codex HIGH ellipsis-
                    // numerator amendment both live — see that fn's own
                    // doc comment. `data-live="true"` is the attribute
                    // Plan 07's CSS keys the 2.4s opacity pulse on; it is
                    // only ever present alongside chip text, so it never
                    // needs its own separate condition.
                    if let Some(chip_text) = in_flight_chip_text(
                        is_team_room,
                        dispatch_state.is_dispatching(),
                        resolve_cycle_budget(t.room.max_cycles, settings_max_cycles),
                        settings_max_rounds,
                    ) {
                        span {
                            class: "kn-chip",
                            "data-kind": "round",
                            "data-live": "true",
                            "{chip_text}"
                        }
                    }
                    // Needs-you badge — conditional, absent (not dimmed)
                    // when clear (State Matrix / 50.1 LIVE-badge
                    // convention).
                    if t.room.needs_you {
                        span {
                            class: "kn-badge",
                            "data-kind": "needs-you",
                            "NEEDS YOU"
                        }
                    }
                }
                button {
                    class: "kn-action-btn",
                    "aria-label": "Group chat settings",
                    onclick: move |_| settings_open.set(true),
                    "⚙"
                }
                // Phase 50.2 Plan 06/17 (UI-SPEC Component Inventory §6):
                // the overflow control — both specced entries, `Edit
                // members` and `Delete room`, are now present (G-50.2-2a
                // closed the `Edit members` drop). Reuses `.kn-bot-card-
                // overflow`'s positioning wrapper and `.kn-profile-menu`/
                // `.kn-profile-menu-item` verbatim (kanban.css, loaded
                // unconditionally) — same idiom `card.rs`'s own per-card
                // overflow menu already established, no new CSS needed.
                span { class: "kn-bot-card-overflow",
                    button {
                        class: "kn-action-btn",
                        "aria-label": "More room actions",
                        onclick: move |_| {
                            let cur = *overflow_open.read();
                            overflow_open.set(!cur);
                        },
                        "⋯"
                    }
                    if *overflow_open.read() {
                        div { class: "kn-profile-menu", role: "menu",
                            // Phase 52 Plan 09 (D-02/D-17, UI-SPEC Surface
                            // Contract 6): rendered FROM
                            // `overflow_menu_entries` rather than restating
                            // its order a second time — `New conversation`
                            // lands between `Edit members` and `Delete
                            // room`, present in every room regardless of
                            // `pattern`.
                            for entry in overflow_menu_entries(transcript.as_ref().and_then(|t| t.room.pattern.as_ref())) {
                                {
                                    match entry {
                                        "Edit members" => rsx! {
                                            button {
                                                key: "{entry}",
                                                class: "kn-profile-menu-item",
                                                role: "menuitem",
                                                onclick: move |_| {
                                                    overflow_open.set(false);
                                                    edit_members_open.set(true);
                                                },
                                                "Edit members"
                                            }
                                        },
                                        "New conversation" => rsx! {
                                            button {
                                                key: "{entry}",
                                                class: "kn-profile-menu-item",
                                                role: "menuitem",
                                                onclick: move |_| {
                                                    overflow_open.set(false);
                                                    new_conversation_open.set(true);
                                                },
                                                "New conversation"
                                            }
                                        },
                                        _ => rsx! {
                                            button {
                                                key: "{entry}",
                                                class: "kn-profile-menu-item",
                                                role: "menuitem",
                                                style: "color: var(--danger);",
                                                onclick: move |_| {
                                                    overflow_open.set(false);
                                                    delete_confirm_open.set(true);
                                                },
                                                "Delete room"
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Phase 52 Plan 09 (D-15, UI-SPEC Surface Contract 5): the
            // advisory row — a single-line row directly beneath the
            // header's needs-you badge, reusing the SAME
            // `.kn-modal-hint--info` treatment the composer's queued
            // notice already uses (no new CSS asset). `.kn-room-workspace`
            // is `flex-direction: column`, so this sibling of
            // `.kn-room-header` renders as its own full-width row beneath
            // it without touching the header's own row layout. Renders
            // nothing when there is nothing to advise.
            if let Some(advisory) = &needs_you_advisory {
                div { class: "kn-modal-hint--info", "{advisory}" }
            }
            div { class: "kn-room-transcript",
                if is_loading {
                    div { class: "kn-drawer-loading", "Loading room…" }
                } else if load_failed {
                    div { class: "kn-modal-error", "Could not load this room." }
                } else if let Some(t) = &transcript {
                    if t.messages.is_empty() {
                        div { class: "kn-drawer-empty", "No messages yet — send one to start the room." }
                    } else {
                        for (band_i , band) in bands.iter().enumerate() {
                            {
                                let divider_label = round_divider_label(band.round, settings_max_rounds);
                                let is_last_band = band_i + 1 == bands.len();
                                rsx! {
                                    // ONE divider per band, never per
                                    // message — under the
                                    // parallel-within-round amendment
                                    // several messages can land under one
                                    // divider in join-completion order with
                                    // no serial ordering implied between
                                    // them; the divider is the transcript's
                                    // ONLY ordering guarantee.
                                    div {
                                        key: "divider-{band_i}",
                                        class: "kn-room-round-divider",
                                        "{divider_label}"
                                    }
                                    for (row_i , row) in grouped_bands[band_i].iter().enumerate() {
                                        {
                                            // Phase 52 Plan 08 (D-10/D-11): a delegation row's
                                            // children render via the SAME transcript_message_row
                                            // markup, indented inside .kn-room-worker-fold, only
                                            // while this row's GLOBAL position is expanded.
                                            let global_row_i = band_row_offsets[band_i] + row_i;
                                            let fold_summary = delegation_fold_summary_of(&row.message);
                                            let is_expanded = expanded_delegations.read().contains(&global_row_i);
                                            rsx! {
                                                {transcript_message_row(&row.message, &meta_map, now, format!("msg-{band_i}-{row_i}"))}
                                                if let Some(summary) = fold_summary {
                                                    // Phase 52 Plan 08 (D-10, UI-SPEC Surface
                                                    // Contract 3): the SAME button+signal toggle
                                                    // idiom the "⋯" overflow menu already uses in
                                                    // this file — never `<details>`/`<summary>`.
                                                    // Collapsed by default; its visible text is the
                                                    // server-composed fold summary, never a
                                                    // client-side recount.
                                                    button {
                                                        key: "fold-{band_i}-{row_i}",
                                                        class: "kn-action-btn",
                                                        "aria-label": "Toggle worker results",
                                                        "aria-expanded": if is_expanded { "true" } else { "false" },
                                                        onclick: move |_| {
                                                            let mut set = expanded_delegations.read().clone();
                                                            if set.contains(&global_row_i) {
                                                                set.remove(&global_row_i);
                                                            } else {
                                                                set.insert(global_row_i);
                                                            }
                                                            expanded_delegations.set(set);
                                                        },
                                                        {format!("{} {summary}", if is_expanded { "▾" } else { "▸" })}
                                                    }
                                                    if is_expanded {
                                                        // Phase 52 Plan 08 (D-10): worker sub-rows are
                                                        // excluded from PROMPT replay by the backend
                                                        // (`group_team_api::is_replayable_team_row`) —
                                                        // a backend/prompt-builder concern, not a
                                                        // rendering one. This client renders whatever
                                                        // `row.children` the store returned, with NO
                                                        // filter of its own; do not add a second,
                                                        // divergent filter here.
                                                        div { class: "kn-room-worker-fold",
                                                            for (child_i , child) in row.children.iter().enumerate() {
                                                                {transcript_message_row(child, &meta_map, now, format!("worker-{band_i}-{row_i}-{child_i}"))}
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    // A settled round renders the divider
                                    // followed by a single dim `Settled`
                                    // line, after that band only.
                                    if is_last_band && last_band_settled {
                                        div { class: "kn-room-settled", "Settled" }
                                    }
                                }
                            }
                        }
                    }
                }
                // Phase 50.2 Plan 07 (D-20): handoff blocks for bots
                // mentioned in this room who are NOT room members —
                // rendered supplementally after the persisted round-banded
                // transcript (this room's own send action is what appends
                // them, so they are always the newest content here).
                for (mention_i , mention_entry) in room_mention_entries.read().iter().enumerate() {
                    MentionHandoffBlock { key: "{mention_i}", entry: mention_entry.clone() }
                }
                if let Some(err) = dispatch_error_val {
                    div { class: "kn-modal-error", "{err}" }
                }
            }
            // Phase 50.2 Plan 18 (G-50.2-2c): the queued-for-next-round
            // notice — transient composer chrome, never a persisted
            // transcript row, directly above the composer and below the
            // dispatch-error row above. Reuses `kn-modal-hint--info` (no new
            // CSS asset — plan 50.2-17 owns `bots.css` this round).
            if let Some(depth) = queued_notice_val {
                div { class: "kn-modal-hint--info", "{queued_notice_for_room(depth, is_team_room)}" }
            }
            div { class: "kn-room-composer",
                input {
                    class: "kn-modal-input",
                    r#type: "text",
                    value: "{draft_val}",
                    placeholder: "Message the room…",
                    onkeydown: {
                        let room_id_for_send = room_id.clone();
                        let roster_names_for_send = roster_names.clone();
                        let room_members_for_send = room_members.clone();
                        move |evt| {
                            if evt.key() == Key::Enter {
                                submit_room_send(
                                    room_id_for_send.clone(),
                                    draft,
                                    in_flight_dispatches,
                                    queued_notice,
                                    dispatch_error,
                                    refresh_tick,
                                    roster_names_for_send.clone(),
                                    room_members_for_send.clone(),
                                    room_mention_entries,
                                );
                            }
                        }
                    },
                    oninput: move |evt| draft.set(evt.value()),
                }
                button {
                    class: "kn-action-btn",
                    disabled: composer_send_disabled(can_send),
                    onclick: {
                        let room_id_for_send = room_id.clone();
                        let roster_names_for_send = roster_names.clone();
                        let room_members_for_send = room_members.clone();
                        move |_| {
                            submit_room_send(
                                room_id_for_send.clone(),
                                draft,
                                in_flight_dispatches,
                                queued_notice,
                                dispatch_error,
                                refresh_tick,
                                roster_names_for_send.clone(),
                                room_members_for_send.clone(),
                                room_mention_entries,
                            );
                        }
                    },
                    "{send_label}"
                }
            }
        }
        // Phase 50.2 Plan 05: mounted as a sibling to `.kn-room-workspace`,
        // never nested inside it — same "mounted outside the primary
        // shell" placement `DeleteBotConfirm` uses in the profile drawer.
        // `GroupSettingsDrawer` itself renders nothing while
        // `settings_open` is false (D-10 mount-unconditionally idiom).
        GroupSettingsDrawer {
            open: settings_open,
            // Phase 52 Plan 08: closes Plan 07's Round 1 codex MEDIUM gap —
            // a save made from THIS room's own gear icon now bumps
            // `settings_tick`, which `settings_resource` reads, instead of
            // leaving the header chip's cycle-budget denominator stale
            // until an unrelated remount.
            on_saved: move |_| {
                let cur = *settings_tick.read();
                settings_tick.set(cur + 1);
            },
        }
        // Phase 50.2 Plan 06: conditionally mounted (not always-mounted-
        // render-nothing) — same idiom `bot_roster.rs`'s own
        // `CreateRoomModal` mount already uses, since a target room name
        // is only available once the transcript has resolved. On success,
        // `on_deleted` closes this modal AND delegates to `on_back` —
        // `on_back`'s own handler (owned by `BotRoster`) already clears
        // `open_room` and bumps `rooms_refresh_tick`, exactly "close the
        // modal, return to the roster grid, and the deleted room's row is
        // gone after the roster refresh."
        if *delete_confirm_open.read() {
            if let Some(t) = &transcript {
                DeleteRoomConfirm {
                    room_id: room_id.clone(),
                    room_name: t.room.name.clone(),
                    on_dismiss: move |_| delete_confirm_open.set(false),
                    on_deleted: move |_| {
                        delete_confirm_open.set(false);
                        on_back.call(());
                    },
                }
            }
        }
        // Phase 50.2 Plan 17 (G-50.2-2a): conditionally mounted, same idiom
        // `DeleteRoomConfirm` above already uses — a target room name and
        // current member list are only available once the transcript has
        // resolved. `on_saved` clears the flag AND bumps `refresh_tick`,
        // which is what makes the header re-read the room through its
        // existing `use_resource` sync prefix — never a `.restart()` call.
        if *edit_members_open.read() {
            if let Some(t) = &transcript {
                EditMembersModal {
                    room_id: room_id.clone(),
                    room_name: t.room.name.clone(),
                    current_members: t.room.members.clone(),
                    roster_entries: roster_entries.clone(),
                    pattern: t.room.pattern.clone(),
                    roles: t.room.roles.clone(),
                    max_cycles: t.room.max_cycles,
                    leader_prompt_override: t.room.leader_prompt_override.clone(),
                    worker_prompt_override: t.room.worker_prompt_override.clone(),
                    on_close: move |_| edit_members_open.set(false),
                    on_saved: move |_| {
                        edit_members_open.set(false);
                        let mut tick = refresh_tick;
                        let cur = *tick.read();
                        tick.set(cur + 1);
                    },
                }
            }
        }
        // Phase 52 Plan 09 (D-02): conditionally mounted, same idiom
        // `DeleteRoomConfirm`/`EditMembersModal` above already use — a
        // target room name is only available once the transcript has
        // resolved. `on_reset` bumps `refresh_tick`, the same reload
        // `EditMembersModal`'s own `on_saved` already triggers, so the
        // appended System marker row is visible without a page reload.
        if *new_conversation_open.read() {
            if let Some(t) = &transcript {
                NewConversationConfirm {
                    room_id: room_id.clone(),
                    room_name: t.room.name.clone(),
                    on_dismiss: move |_| new_conversation_open.set(false),
                    on_reset: move |_| {
                        new_conversation_open.set(false);
                        let mut tick = refresh_tick;
                        let cur = *tick.read();
                        tick.set(cur + 1);
                    },
                }
            }
        }
    }
}

// Module named `room_transcript_tests` (not the crate's usual bare `tests`)
// so every test's fully-qualified name contains the substring
// `room_transcript` — this plan's own `<verify>` filter
// (`cargo nextest run ... room_transcript`) targets exactly this module.
#[cfg(test)]
mod room_transcript_tests {
    use super::*;

    fn msg(round: u32, from: GroupRoomSpeaker, status: MemberTurnStatus, text: &str) -> GroupRoomMessage {
        GroupRoomMessage {
            from,
            text: text.to_string(),
            at_ms: 0,
            round,
            status,
            team_row: None,
        }
    }

    fn member_msg(round: u32, name: &str, status: MemberTurnStatus, text: &str) -> GroupRoomMessage {
        msg(round, GroupRoomSpeaker::Member(name.to_string()), status, text)
    }

    // Phase 52 Plan 08 (D-10/D-11): team-row constructors, mirroring `msg`/
    // `member_msg`'s own shape.

    fn operator_msg(round: u32, text: &str) -> GroupRoomMessage {
        msg(round, GroupRoomSpeaker::Operator, MemberTurnStatus::Replied, text)
    }

    fn system_marker_msg(round: u32) -> GroupRoomMessage {
        msg(round, GroupRoomSpeaker::System, MemberTurnStatus::Replied, "")
    }

    fn delegation_msg(round: u32, leader: &str, fold_summary: &str, text: &str) -> GroupRoomMessage {
        let mut m = member_msg(round, leader, MemberTurnStatus::Replied, text);
        m.team_row = Some(TeamRowKind::Delegation { fold_summary: fold_summary.to_string() });
        m
    }

    fn worker_result_msg(
        round: u32,
        worker: &str,
        outcome: WorkerOutcomeKind,
        status: MemberTurnStatus,
        text: &str,
    ) -> GroupRoomMessage {
        let mut m = member_msg(round, worker, status, text);
        m.team_row = Some(TeamRowKind::WorkerResult { outcome });
        m
    }

    fn synthesis_msg(round: u32, leader: &str, text: &str) -> GroupRoomMessage {
        let mut m = member_msg(round, leader, MemberTurnStatus::Replied, text);
        m.team_row = Some(TeamRowKind::Synthesis);
        m
    }

    // -------------------------------------------------------------------
    // group_messages_into_rounds
    // -------------------------------------------------------------------

    #[test]
    fn group_messages_into_rounds_bands_consecutive_equal_rounds() {
        let messages = vec![
            member_msg(1, "a", MemberTurnStatus::Replied, "hi"),
            member_msg(1, "b", MemberTurnStatus::Replied, "hi"),
            member_msg(1, "c", MemberTurnStatus::Replied, "hi"),
            member_msg(2, "a", MemberTurnStatus::Replied, "hi"),
            member_msg(2, "b", MemberTurnStatus::Replied, "hi"),
        ];
        let bands = group_messages_into_rounds(&messages);
        assert_eq!(bands.len(), 2);
        assert_eq!(bands[0].round, 1);
        assert_eq!(bands[0].messages.len(), 3);
        assert_eq!(bands[1].round, 2);
        assert_eq!(bands[1].messages.len(), 2);
    }

    #[test]
    fn group_messages_into_rounds_empty_list_returns_no_bands() {
        let bands = group_messages_into_rounds(&[]);
        assert!(bands.is_empty());
    }

    // -------------------------------------------------------------------
    // round_divider_label
    // -------------------------------------------------------------------

    #[test]
    fn round_divider_label_uses_the_settings_max_not_a_hardcoded_three() {
        assert_eq!(round_divider_label(1, 5), "Round 1 of 5");
        assert_eq!(round_divider_label(2, 5), "Round 2 of 5");
    }

    // -------------------------------------------------------------------
    // round_band_is_settled
    // -------------------------------------------------------------------

    #[test]
    fn round_band_is_settled_true_when_every_member_message_is_pass_or_failed() {
        let band = RoundBand {
            round: 1,
            messages: vec![
                msg(1, GroupRoomSpeaker::Operator, MemberTurnStatus::Replied, "kickoff"),
                member_msg(1, "a", MemberTurnStatus::Passed, "(pass)"),
                member_msg(
                    1,
                    "b",
                    MemberTurnStatus::Failed { reason: "boom".to_string() },
                    "",
                ),
            ],
        };
        assert!(round_band_is_settled(&band));
    }

    #[test]
    fn round_band_is_settled_false_when_any_member_replied() {
        let band = RoundBand {
            round: 1,
            messages: vec![
                msg(1, GroupRoomSpeaker::Operator, MemberTurnStatus::Replied, "kickoff"),
                member_msg(1, "a", MemberTurnStatus::Replied, "hello"),
            ],
        };
        assert!(!round_band_is_settled(&band));
    }

    #[test]
    fn round_band_is_settled_false_when_band_has_no_member_messages() {
        let band = RoundBand {
            round: 1,
            messages: vec![msg(1, GroupRoomSpeaker::Operator, MemberTurnStatus::Replied, "kickoff")],
        };
        assert!(!round_band_is_settled(&band));
    }

    // -------------------------------------------------------------------
    // message_display_text / message_is_pass_equivalent
    // -------------------------------------------------------------------

    #[test]
    fn message_display_text_passed_renders_literal_pass_regardless_of_raw_text() {
        let m = member_msg(1, "a", MemberTurnStatus::Passed, "( PASS ).");
        assert_eq!(message_display_text(&m, "a"), "(pass)");
    }

    // Phase 50.2 Plan 11 (G-2): replaces
    // `message_display_text_failed_renders_literal_pass_not_the_raw_reason`,
    // which asserted the defect (50.2-UAT-EVIDENCE.md §4 F-2 — an operator
    // could not tell a failed turn from a genuine pass). This is the D-24
    // mutation-bar test: run against the pre-fix `Failed` arm (still
    // returning the literal `"(pass)"` string) it fails, proving the
    // assertion actually exercises the defect before the fix lands.
    #[test]
    fn message_display_text_failed_renders_the_visible_failure_reason() {
        let m = member_msg(
            1,
            "zig",
            MemberTurnStatus::Failed { reason: "bot subprocess exited with code 1".to_string() },
            "",
        );
        assert_eq!(
            message_display_text(&m, "zig"),
            "zig didn't respond this round: bot subprocess exited with code 1"
        );
        assert_ne!(message_display_text(&m, "zig"), "(pass)");
    }

    #[test]
    fn message_display_text_replied_renders_the_actual_text() {
        let m = member_msg(1, "a", MemberTurnStatus::Replied, "hello room");
        assert_eq!(message_display_text(&m, "a"), "hello room");
    }

    // Phase 50.2 Plan 11 (G-3): the room transcript's Replied arm must
    // strip reasoning blocks, sharing chat_window.rs's implementation
    // (50.2-VERIFICATION.md: message_display_text was the one reply
    // surface with zero think-stripping references). This is the D-24
    // mutation-bar test: run against the pre-fix `Replied` arm (still
    // `msg.text.clone()`) it fails, proving the assertion actually
    // exercises the leak before the fix lands.
    #[test]
    fn message_display_text_replied_strips_a_leading_reasoning_block() {
        let m = member_msg(
            1,
            "a",
            MemberTurnStatus::Replied,
            "<think>internal musing</think>the real answer",
        );
        assert_eq!(message_display_text(&m, "a"), "the real answer");
    }

    #[test]
    fn message_display_text_replied_entirely_reasoning_renders_the_shared_placeholder() {
        let m = member_msg(1, "a", MemberTurnStatus::Replied, "<think>only reasoning</think>");
        assert_eq!(message_display_text(&m, "a"), BOT_THINK_ONLY_PLACEHOLDER);
    }

    #[test]
    fn message_display_text_replied_no_reasoning_tags_is_byte_identical() {
        let m = member_msg(1, "a", MemberTurnStatus::Replied, "plain reply, no tags here");
        assert_eq!(message_display_text(&m, "a"), "plain reply, no tags here");
    }

    #[test]
    fn message_display_text_replied_unclosed_leading_tag_renders_the_placeholder() {
        let m = member_msg(1, "a", MemberTurnStatus::Replied, "<think>dangling, never closed");
        assert_eq!(message_display_text(&m, "a"), BOT_THINK_ONLY_PLACEHOLDER);
    }

    // Confirms the shared fn/const are reachable and behave identically
    // from this module — a direct call, not routed through
    // message_display_text, so a future regression that only breaks the
    // Replied ARM (not the shared fn itself) still gets caught above.
    #[test]
    fn shared_strip_think_blocks_for_display_is_reachable_from_group_chat_workspace() {
        assert_eq!(
            strip_think_blocks_for_display("<think>x</think>visible"),
            "visible"
        );
    }

    #[test]
    fn message_is_pass_equivalent_true_for_passed_and_failed_false_for_replied() {
        assert!(message_is_pass_equivalent(&member_msg(
            1,
            "a",
            MemberTurnStatus::Passed,
            ""
        )));
        assert!(message_is_pass_equivalent(&member_msg(
            1,
            "a",
            MemberTurnStatus::Failed { reason: "x".to_string() },
            ""
        )));
        assert!(!message_is_pass_equivalent(&member_msg(
            1,
            "a",
            MemberTurnStatus::Replied,
            "hi"
        )));
    }

    // -------------------------------------------------------------------
    // message_status_attr
    // -------------------------------------------------------------------

    #[test]
    fn message_status_attr_passed_returns_pass() {
        let m = member_msg(1, "a", MemberTurnStatus::Passed, "");
        assert_eq!(message_status_attr(&m), Some("pass"));
    }

    #[test]
    fn message_status_attr_failed_returns_failed() {
        let m = member_msg(1, "a", MemberTurnStatus::Failed { reason: "x".to_string() }, "");
        assert_eq!(message_status_attr(&m), Some("failed"));
    }

    #[test]
    fn message_status_attr_replied_returns_none() {
        let m = member_msg(1, "a", MemberTurnStatus::Replied, "hi");
        assert_eq!(message_status_attr(&m), None);
    }

    // Phase 50.2 Plan 11 (G-2): the failed value must not equal the pass
    // value — this is the assertion that goes red if someone re-fuses the
    // two arms back into a shared "pass" status.
    #[test]
    fn message_status_attr_outcomes_are_pairwise_distinct() {
        let passed = member_msg(1, "a", MemberTurnStatus::Passed, "");
        let failed = member_msg(1, "a", MemberTurnStatus::Failed { reason: "x".to_string() }, "");
        let replied = member_msg(1, "a", MemberTurnStatus::Replied, "hi");

        let passed_attr = message_status_attr(&passed);
        let failed_attr = message_status_attr(&failed);
        let replied_attr = message_status_attr(&replied);

        assert_eq!(passed_attr, Some("pass"));
        assert_eq!(failed_attr, Some("failed"));
        assert_eq!(replied_attr, None);
        assert_ne!(passed_attr, failed_attr);
        assert_ne!(passed_attr, replied_attr);
        assert_ne!(failed_attr, replied_attr);
    }

    // -------------------------------------------------------------------
    // failure_marker_label
    // -------------------------------------------------------------------

    #[test]
    fn failure_marker_label_carries_bot_name_and_reason() {
        assert_eq!(
            failure_marker_label("scout", "missing provider key"),
            "scout didn't respond this round: missing provider key"
        );
    }

    // -------------------------------------------------------------------
    // identity_border_token
    // -------------------------------------------------------------------

    #[test]
    fn identity_border_token_prefers_the_stored_avatar_color() {
        assert_eq!(identity_border_token(Some("--danger"), "scout"), "--danger");
    }

    #[test]
    fn identity_border_token_falls_back_to_seeded_color_for_when_unset() {
        assert_eq!(identity_border_token(None, "scout"), seeded_color_for("scout"));
    }

    // -------------------------------------------------------------------
    // room_member_names_label — Phase 50.2 Plan 17 (G-50.2-2a, tdd="true")
    // -------------------------------------------------------------------

    #[test]
    fn room_member_names_label_empty_list_returns_empty_string() {
        assert_eq!(room_member_names_label(&[], 6), "");
    }

    #[test]
    fn room_member_names_label_one_member_returns_that_name_alone() {
        assert_eq!(room_member_names_label(&["scout".to_string()], 6), "scout");
    }

    #[test]
    fn room_member_names_label_several_members_join_in_room_order() {
        let members = vec!["scout".to_string(), "zig".to_string(), "ada".to_string()];
        assert_eq!(room_member_names_label(&members, 6), "scout, zig, ada");
    }

    #[test]
    fn room_member_names_label_bounded_at_max_shown_appends_overflow_marker() {
        let members = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        assert_eq!(room_member_names_label(&members, 2), "a, b +2 more");
    }

    #[test]
    fn room_member_names_label_exactly_at_max_shown_has_no_overflow_marker() {
        let members = vec!["a".to_string(), "b".to_string()];
        assert_eq!(room_member_names_label(&members, 2), "a, b");
    }

    // -------------------------------------------------------------------
    // RoomDispatchState
    // -------------------------------------------------------------------

    #[test]
    fn room_dispatch_state_from_sending_maps_correctly() {
        assert_eq!(RoomDispatchState::from_sending(true), RoomDispatchState::Dispatching);
        assert_eq!(RoomDispatchState::from_sending(false), RoomDispatchState::Idle);
        assert!(RoomDispatchState::Dispatching.is_dispatching());
        assert!(!RoomDispatchState::Idle.is_dispatching());
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 07 (D-20, T-50.2-07-04): mention_targets_for_room —
    // a mentioned ROOM MEMBER is the group driver's own job (plan 04); a
    // mentioned non-member gets an inline handoff.
    // -------------------------------------------------------------------

    fn room_roster() -> Vec<String> {
        vec!["scout".to_string(), "zig".to_string(), "ada".to_string()]
    }

    #[test]
    fn mention_targets_for_room_excludes_a_mentioned_member() {
        let members = vec!["scout".to_string(), "zig".to_string()];
        assert_eq!(
            mention_targets_for_room("@scout status", &room_roster(), &members, "operator"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mention_targets_for_room_includes_a_mentioned_non_member() {
        let members = vec!["scout".to_string(), "zig".to_string()];
        assert_eq!(
            mention_targets_for_room("@ada status", &room_roster(), &members, "operator"),
            vec!["ada".to_string()]
        );
    }

    #[test]
    fn mention_targets_for_room_mix_of_member_and_non_member() {
        let members = vec!["scout".to_string()];
        assert_eq!(
            mention_targets_for_room("@scout and @ada", &room_roster(), &members, "operator"),
            vec!["ada".to_string()]
        );
    }

    #[test]
    fn mention_targets_for_room_no_mentions_returns_empty() {
        let members = vec!["scout".to_string()];
        assert_eq!(
            mention_targets_for_room("just a normal message", &room_roster(), &members, "operator"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mention_targets_for_room_two_non_members_in_mention_order() {
        let members = vec!["scout".to_string()];
        assert_eq!(
            mention_targets_for_room("@zig and @ada", &room_roster(), &members, "operator"),
            vec!["zig".to_string(), "ada".to_string()]
        );
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 07: the SHARED resolver (`resolve_roster_mentions_client`)
    // has no member/self filtering of its own — room-membership exclusion is
    // `mention_targets_for_room`'s own job, applied on top of this fn's
    // output. (Corrected by Phase 50.2 Plan 20 / WR-01: this test pins ONLY
    // the shared resolver's own unfiltered behavior — it does NOT mean the
    // Bot Chat consumer is unfiltered. `mention_targets_for_bot_chat`
    // applies its own self-exclusion filter on top of this same resolver;
    // see `both_mention_consumers_exclude_an_already_dispatched_bot` below.)
    // -------------------------------------------------------------------

    #[test]
    fn resolve_roster_mentions_client_itself_applies_no_member_or_self_filter() {
        let roster = room_roster();
        let raw_targets = crate::server::mention_handoff_api::resolve_roster_mentions_client(
            "@scout and @ada",
            &roster,
            "operator",
        );
        assert_eq!(
            raw_targets,
            vec!["scout".to_string(), "ada".to_string()],
            "the shared resolver applies no member or self filter — that filtering is each \
             mention consumer's own job, layered on top"
        );
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 20 (WR-01): both mention consumers guard the same
    // hazard class — double-dispatching a bot the caller's own surface has
    // already dispatched — the same way. This test defends against the two
    // filters drifting apart again: for the same text, the room filter (with
    // the bot as a room member) and the Bot Chat filter (with the bot as the
    // window's own bot) must both exclude it and both keep any other
    // mentioned bot.
    // -------------------------------------------------------------------

    #[test]
    fn both_mention_consumers_exclude_an_already_dispatched_bot() {
        let roster = room_roster();
        let text = "@scout and @ada";

        let room_members = vec!["scout".to_string()];
        let room_result = mention_targets_for_room(text, &roster, &room_members, "operator");
        assert_eq!(
            room_result,
            vec!["ada".to_string()],
            "the room filter must exclude a mentioned room member and keep the other target"
        );

        let bot_chat_result =
            super::super::chat_window::mention_targets_for_bot_chat(text, &roster, "scout");
        assert_eq!(
            bot_chat_result,
            vec!["ada".to_string()],
            "the Bot Chat filter must exclude the window's own bot and keep the other target"
        );

        assert_eq!(
            room_result, bot_chat_result,
            "both mention consumers must agree on the single-element target set for a text \
             mentioning one already-dispatched bot and one other roster bot"
        );
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 18 (G-50.2-2c): queued_for_next_round_notice
    // -------------------------------------------------------------------

    #[test]
    fn queued_for_next_round_notice_depth_one_uses_singular_form() {
        let notice = queued_for_next_round_notice(1);
        assert_eq!(
            notice,
            "Message queued — it will go into the room on the next round."
        );
    }

    #[test]
    fn queued_for_next_round_notice_depth_three_states_the_count() {
        let notice = queued_for_next_round_notice(3);
        assert_eq!(
            notice,
            "Message queued — it will go into the room on the next round (3 messages waiting)."
        );
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 08, Task 1 (D-10/D-11): group_transcript_rows
    // -------------------------------------------------------------------

    #[test]
    fn group_transcript_rows_groups_worker_sub_rows_under_their_delegation_row() {
        let messages = vec![
            operator_msg(1, "kick it off"),
            delegation_msg(1, "leader", "3 workers — 2 completed, 1 failed", "Delegating 3 tasks: ..."),
            worker_result_msg(1, "alpha", WorkerOutcomeKind::Completed, MemberTurnStatus::Replied, "done"),
            worker_result_msg(1, "beta", WorkerOutcomeKind::Completed, MemberTurnStatus::Replied, "done"),
            worker_result_msg(
                1,
                "gamma",
                WorkerOutcomeKind::Failed,
                MemberTurnStatus::Failed { reason: "dispatch-failed".to_string() },
                "",
            ),
            synthesis_msg(1, "leader", "final answer"),
        ];
        let rows = group_transcript_rows(&messages);
        assert_eq!(rows.len(), 3, "operator, delegation, synthesis — three top-level rows");
        assert_eq!(rows[0].message.from, GroupRoomSpeaker::Operator);
        assert!(rows[0].children.is_empty());
        assert!(delegation_fold_summary_of(&rows[1].message).is_some());
        assert_eq!(rows[1].children.len(), 3);
        assert!(matches!(rows[2].message.team_row, Some(TeamRowKind::Synthesis)));
        assert!(rows[2].children.is_empty());
    }

    #[test]
    fn group_transcript_rows_leaves_a_peer_room_transcript_flat() {
        let messages = vec![
            member_msg(1, "a", MemberTurnStatus::Replied, "hi"),
            member_msg(1, "b", MemberTurnStatus::Passed, "(pass)"),
            member_msg(2, "a", MemberTurnStatus::Replied, "hi2"),
        ];
        let rows = group_transcript_rows(&messages);
        assert_eq!(rows.len(), messages.len());
        for (row, original) in rows.iter().zip(messages.iter()) {
            assert_eq!(&row.message, original);
            assert!(row.children.is_empty());
        }
    }

    #[test]
    fn group_transcript_rows_handles_worker_rows_with_no_preceding_delegation_row() {
        let messages = vec![worker_result_msg(
            1,
            "alpha",
            WorkerOutcomeKind::Completed,
            MemberTurnStatus::Replied,
            "done",
        )];
        let rows = group_transcript_rows(&messages);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].children.is_empty());
        assert_eq!(rows[0].message.from, GroupRoomSpeaker::Member("alpha".to_string()));
    }

    #[test]
    fn group_transcript_rows_stops_grouping_at_an_operator_row() {
        let messages = vec![
            delegation_msg(1, "leader", "1 worker — 1 completed", "Delegating 1 task: ..."),
            operator_msg(2, "follow up"),
            worker_result_msg(2, "alpha", WorkerOutcomeKind::Completed, MemberTurnStatus::Replied, "done"),
        ];
        let rows = group_transcript_rows(&messages);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].children.is_empty(), "the operator row closed the delegation before any worker row arrived");
        assert!(rows[2].children.is_empty());
        assert_eq!(rows[2].message.from, GroupRoomSpeaker::Member("alpha".to_string()));
    }

    #[test]
    fn group_transcript_rows_stops_grouping_at_a_synthesis_row() {
        let messages = vec![
            delegation_msg(1, "leader", "1 worker — 1 completed", "Delegating 1 task: ..."),
            synthesis_msg(1, "leader", "final"),
            worker_result_msg(2, "alpha", WorkerOutcomeKind::Completed, MemberTurnStatus::Replied, "done"),
        ];
        let rows = group_transcript_rows(&messages);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].children.is_empty());
        assert!(rows[2].children.is_empty());
    }

    #[test]
    fn group_transcript_rows_stops_grouping_at_a_conversation_marker_row() {
        let messages = vec![
            delegation_msg(1, "leader", "1 worker — 1 completed", "Delegating 1 task: ..."),
            system_marker_msg(2),
            worker_result_msg(2, "alpha", WorkerOutcomeKind::Completed, MemberTurnStatus::Replied, "done"),
        ];
        let rows = group_transcript_rows(&messages);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].children.is_empty());
        assert!(rows[2].children.is_empty());
    }

    #[test]
    fn all_five_worker_sub_rows_render_with_no_truncation() {
        let mut messages = vec![delegation_msg(1, "leader", "5 workers — 5 completed", "Delegating 5 tasks: ...")];
        for i in 0..5 {
            messages.push(worker_result_msg(
                1,
                &format!("worker{i}"),
                WorkerOutcomeKind::Completed,
                MemberTurnStatus::Replied,
                "done",
            ));
        }
        let rows = group_transcript_rows(&messages);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].children.len(), 5, "no pagination, no show-more — all 5 render");
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 08, Task 1 (D-10/D-11): delegation_fold_summary_of
    // -------------------------------------------------------------------

    #[test]
    fn delegation_fold_summary_of_reads_the_server_composed_payload() {
        let d = delegation_msg(1, "leader", "2 workers — 1 completed, 1 failed", "Delegating 2 tasks: ...");
        assert_eq!(delegation_fold_summary_of(&d), Some("2 workers — 1 completed, 1 failed"));

        let w = worker_result_msg(1, "alpha", WorkerOutcomeKind::Completed, MemberTurnStatus::Replied, "done");
        assert_eq!(delegation_fold_summary_of(&w), None);

        let plain = member_msg(1, "a", MemberTurnStatus::Replied, "hi");
        assert_eq!(delegation_fold_summary_of(&plain), None);
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 08, Task 1 (D-10): worker_status_attr / blocked_marker_label
    // -------------------------------------------------------------------

    #[test]
    fn worker_status_attr_maps_completed_failed_and_blocked_to_distinct_values() {
        assert_eq!(worker_status_attr(&WorkerOutcomeKind::Completed), None);
        assert_eq!(worker_status_attr(&WorkerOutcomeKind::Failed), Some("failed"));
        assert_eq!(worker_status_attr(&WorkerOutcomeKind::Blocked), Some("blocked"));
        assert_ne!(
            worker_status_attr(&WorkerOutcomeKind::Failed),
            worker_status_attr(&WorkerOutcomeKind::Blocked)
        );
    }

    #[test]
    fn blocked_marker_label_carries_the_reason_as_visible_body_text() {
        let label = blocked_marker_label("scout", "needs the API key rotated");
        assert!(label.contains("scout"));
        assert!(label.contains("needs the API key rotated"));

        assert!(worker_outcome_reason_is_visible_body_text(&WorkerOutcomeKind::Blocked));
        assert_eq!(
            worker_outcome_reason_is_visible_body_text(&WorkerOutcomeKind::Blocked),
            worker_outcome_reason_is_visible_body_text(&WorkerOutcomeKind::Failed)
        );
        assert!(!worker_outcome_reason_is_visible_body_text(&WorkerOutcomeKind::Completed));

        let blocked_row = worker_result_msg(
            1,
            "scout",
            WorkerOutcomeKind::Blocked,
            MemberTurnStatus::Replied,
            "needs the API key rotated",
        );
        assert_eq!(
            message_display_text(&blocked_row, "scout"),
            label,
            "the visible body text is the SAME label the hover marker would carry — never hover-only"
        );
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 08, Task 3 (D-08/D-14/D-17): in_flight_chip_text / resolve_cycle_budget
    // -------------------------------------------------------------------

    #[test]
    fn in_flight_chip_text_reads_cycles_in_a_team_room_and_rounds_in_a_peer_room() {
        assert_eq!(in_flight_chip_text(true, true, 3, 5), Some("Working — cycle …/3".to_string()));
        assert_eq!(in_flight_chip_text(false, true, 3, 5), Some("Round …/5".to_string()));
    }

    #[test]
    fn in_flight_chip_text_is_absent_when_no_drive_is_dispatching() {
        assert_eq!(in_flight_chip_text(true, false, 3, 5), None);
        assert_eq!(in_flight_chip_text(false, false, 3, 5), None);
    }

    #[test]
    fn in_flight_chip_text_carries_no_numeric_current_cycle() {
        let text = in_flight_chip_text(true, true, 3, 5).unwrap();
        // Exactly one contiguous digit run (the denominator) — a
        // regression that reintroduces a fabricated `{n}` numerator would
        // add a SECOND digit run and fail this count.
        let digit_runs = text.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).count();
        assert_eq!(digit_runs, 1, "exactly one integer (the denominator) may appear: {text}");
    }

    #[test]
    fn in_flight_chip_denominator_matches_the_resolved_cycle_budget() {
        assert_eq!(resolve_cycle_budget(Some(3), 1), 3, "a room override wins over the app-wide default");
        assert_eq!(resolve_cycle_budget(None, 1), 1, "absent a room override, the app-wide default applies");
    }

    // Phase 52 Plan 08 (Round 1 codex HIGH): asserted against the REAL
    // server-side resolver so the two implementations cannot silently
    // drift apart. `#[cfg(feature = "server")]` — this crate's own verify
    // commands always run `--all-features`, so this test is always live
    // there; it is compiled out of a default-feature (`web`-only) test
    // build, where `group_team_api` (`server/mod.rs:274`) does not exist.
    #[cfg(feature = "server")]
    #[test]
    fn resolve_cycle_budget_matches_the_server_resolver() {
        use crate::protocol::{GroupRoom, MemberRole};
        use std::collections::BTreeMap;

        fn bare_room(max_cycles: Option<u32>) -> GroupRoom {
            GroupRoom {
                id: "r1".to_string(),
                name: "room".to_string(),
                members: vec!["a".to_string(), "b".to_string()],
                group: None,
                needs_you: false,
                needs_you_reason: None,
                preview: None,
                preview_at_ms: None,
                created_at_ms: 0,
                updated_at_ms: 0,
                pattern: None,
                roles: BTreeMap::<String, MemberRole>::new(),
                max_cycles,
                leader_prompt_override: None,
                worker_prompt_override: None,
                conversation_epoch: 1,
            }
        }

        let settings = GroupChatSettings::default();
        for room_max in [Some(3u32), None, Some(99u32)] {
            let room = bare_room(room_max);
            let server_result = crate::server::group_team_api::resolve_cycle_budget(&room, &settings);
            let client_result = resolve_cycle_budget(room_max, settings.max_cycles);
            assert_eq!(
                client_result, server_result,
                "client and server cycle-budget resolution must never drift apart (room_max={room_max:?})"
            );
        }
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 08, Task 3 (D-15): composer_send_disabled
    // -------------------------------------------------------------------

    #[test]
    fn composer_is_never_disabled() {
        // D-15: needs_you raised has zero bearing here — the predicate's
        // only input is whether the draft is sendable, by construction.
        assert!(!composer_send_disabled(true));
        assert!(composer_send_disabled(false));
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 08, Task 3 (Copywriting Contract): queued_notice_for_room
    // -------------------------------------------------------------------

    #[test]
    fn the_queued_notice_says_cycle_in_a_team_room_and_round_in_a_peer_room() {
        assert_eq!(
            queued_notice_for_room(1, true),
            "Message queued — it will go into the room on the next cycle."
        );
        assert_eq!(queued_notice_for_room(1, false), queued_for_next_round_notice(1));
        assert_eq!(
            queued_notice_for_room(3, true),
            "Message queued — it will go into the room on the next cycle (3 messages waiting)."
        );
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 09, Task 2 (D-02/D-17): overflow_menu_entries
    // -------------------------------------------------------------------

    #[test]
    fn overflow_menu_order_is_edit_members_then_new_conversation_then_delete_room() {
        assert_eq!(
            overflow_menu_entries(None),
            ["Edit members", "New conversation", "Delete room"]
        );
    }

    #[test]
    fn the_overflow_menu_offers_new_conversation_in_a_peer_room() {
        assert!(overflow_menu_entries(None).contains(&"New conversation"));
    }

    #[test]
    fn the_overflow_menu_offers_new_conversation_in_a_team_room() {
        assert!(overflow_menu_entries(Some(&TeamPattern::OrchestratorWorkers))
            .contains(&"New conversation"));
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 09, Task 3 (D-15): needs_you_advisory_text
    // -------------------------------------------------------------------

    // `GroupRoom` is used only by this test module (`bare_room_for_advisory`
    // below) — a local import here, not a file-level one, mirroring
    // `resolve_cycle_budget_matches_the_server_resolver`'s own local
    // `use crate::protocol::{GroupRoom, MemberRole};` precedent above.
    use crate::protocol::GroupRoom;

    fn bare_room_for_advisory(
        pattern: Option<TeamPattern>,
        needs_you: bool,
        reason: Option<&str>,
    ) -> GroupRoom {
        GroupRoom {
            id: "r1".to_string(),
            name: "room".to_string(),
            members: vec!["a".to_string(), "b".to_string()],
            group: None,
            needs_you,
            needs_you_reason: reason.map(|r| r.to_string()),
            preview: None,
            preview_at_ms: None,
            created_at_ms: 0,
            updated_at_ms: 0,
            pattern,
            roles: std::collections::BTreeMap::new(),
            max_cycles: None,
            leader_prompt_override: None,
            worker_prompt_override: None,
            conversation_epoch: 1,
        }
    }

    #[test]
    fn needs_you_advisory_text_is_none_when_the_flag_is_clear() {
        assert_eq!(needs_you_advisory_text(false, Some("some reason")), None);
        assert_eq!(needs_you_advisory_text(false, None), None);
    }

    #[test]
    fn needs_you_advisory_text_is_the_persisted_reason_when_the_flag_is_raised() {
        assert_eq!(
            needs_you_advisory_text(true, Some("Send another message to try again.")),
            Some("Send another message to try again.".to_string())
        );
    }

    #[test]
    fn needs_you_advisory_text_is_none_when_the_flag_is_raised_with_no_reason() {
        // A pre-Phase-52 record: the badge still renders (`t.room.needs_you`
        // alone), but there is nothing to say — never `Some("")`.
        assert_eq!(needs_you_advisory_text(true, None), None);
    }

    #[test]
    fn needs_you_advisory_renders_identically_for_a_peer_room_and_a_team_room() {
        let reason = "The leader couldn't produce a valid task breakdown after a retry. Send another message to try again.";
        let peer_room = bare_room_for_advisory(None, true, Some(reason));
        let team_room = bare_room_for_advisory(Some(TeamPattern::OrchestratorWorkers), true, Some(reason));
        assert_eq!(
            needs_you_advisory_text(peer_room.needs_you, peer_room.needs_you_reason.as_deref()),
            needs_you_advisory_text(team_room.needs_you, team_room.needs_you_reason.as_deref())
        );
    }

    #[test]
    fn exactly_one_advisory_is_produced() {
        // `needs_you` is a single flag with a single reason — the helper's
        // return type (`Option<String>`) is itself the proof there is no
        // multi-advisory stacking case to render.
        assert_eq!(
            needs_you_advisory_text(true, Some("only one reason")),
            Some("only one reason".to_string())
        );
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 09, Task 3 (Round 1 codex HIGH): dispatch_result_refreshes_room_state
    // -------------------------------------------------------------------

    #[test]
    fn a_failed_dispatch_still_refreshes_persisted_room_state() {
        assert!(dispatch_result_refreshes_room_state(RoomDispatchOutcomeKind::Ran));
        assert!(dispatch_result_refreshes_room_state(RoomDispatchOutcomeKind::Queued));
        assert!(dispatch_result_refreshes_room_state(RoomDispatchOutcomeKind::Err));
    }
}
