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
use crate::components::hermes_app::widgets::bot_face::{seeded_color_for, BotFace};
use crate::protocol::{
    BotMeta, BotRosterEntry, DispatchGroupRoundRequest, GroupChatSettings, GroupRoomMessage,
    GroupRoomSpeaker, GroupRoomTranscript, MemberTurnStatus, MentionHandoffRequest,
    MentionHandoffState,
};
use crate::server::group_chat_api::dispatch_group_round;
use crate::server::group_settings_api::load_group_chat_settings;
use crate::server::mention_handoff_api::{dispatch_mention_handoff, resolve_roster_mentions_client};
use dioxus::prelude::*;
use std::collections::BTreeMap;

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
            Err(e) => dispatch_error.set(Some(format!("{e}"))),
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

    // Phase 50.2 Plan 06 (D-21 integration): the persisted settings record,
    // read only for its `max_rounds` — the round divider's `Round {n} of
    // {max}` label. Falls back to `GroupChatSettings::default().max_rounds`
    // while loading or on a load error, matching the driver's own
    // corrupted-record fallback (never a bare hardcoded `3`).
    let settings_resource =
        use_resource(move || async move { load_group_chat_settings().await });

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

    // Phase 50.2 Plan 07 (D-20): client-side, session-transient handoff
    // blocks for bots mentioned in this room who are NOT room members —
    // rendered supplementally alongside the persisted round-banded
    // transcript (never written into `GroupRoomMessage` itself, which has
    // no handoff-block kind). Owned here, never a context provider, same
    // ownership placement as every other Signal this component holds.
    let room_mention_entries: Signal<Vec<MentionHandoffEntry>> = use_signal(Vec::new);

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
                    // Round chip — conditional, dispatching only (UI-SPEC
                    // Component Inventory §6). No per-round streaming
                    // exists to name a specific round mid-drive (see module
                    // doc), so the chip states the bound rather than
                    // fabricating a false-precision round count.
                    if dispatch_state.is_dispatching() {
                        span {
                            class: "kn-chip",
                            "data-kind": "round",
                            "Round …/{settings_max_rounds}"
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
                            button {
                                class: "kn-profile-menu-item",
                                role: "menuitem",
                                onclick: move |_| {
                                    overflow_open.set(false);
                                    edit_members_open.set(true);
                                },
                                "Edit members"
                            }
                            button {
                                class: "kn-profile-menu-item",
                                role: "menuitem",
                                style: "color: var(--danger);",
                                onclick: move |_| {
                                    overflow_open.set(false);
                                    delete_confirm_open.set(true);
                                },
                                "Delete room"
                            }
                        }
                    }
                }
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
                                    for (msg_i , msg) in band.messages.iter().enumerate() {
                                        {
                                            let GroupRoomMessage { from, at_ms, status, .. } = msg;
                                            // Phase 50.2 Plan 11 (G-2): sender_name is now computed
                                            // BEFORE display_text so the reason can flow through it.
                                            let sender_name = speaker_display_name(from);
                                            let display_text = message_display_text(msg, &sender_name);
                                            let is_pass_equivalent = message_is_pass_equivalent(msg);
                                            let status_attr = message_status_attr(msg);
                                            let failure_reason = match status {
                                                MemberTurnStatus::Failed { reason } => Some(reason.clone()),
                                                _ => None,
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
                                                    key: "msg-{band_i}-{msg_i}",
                                                    class: "kn-room-message",
                                                    // Phase 50.2 Plan 11 (G-2): data-status now comes from
                                                    // message_status_attr, which distinguishes Failed from
                                                    // Passed — the previous shared "pass" value is exactly
                                                    // what made a failed row indistinguishable from a real
                                                    // pass (50.2-UAT-EVIDENCE.md §4 F-2).
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
                                                        // Phase 50.2 Plan 11 (G-2): the reason is now visible
                                                        // BODY text (message_display_text), which assistive
                                                        // tech already reads via the row's own text node — so
                                                        // this marker is aria-hidden rather than aria-label'd,
                                                        // to avoid a screen reader announcing the same
                                                        // sentence twice. `title` stays for the hover
                                                        // affordance.
                                                        if let Some(reason) = &failure_reason {
                                                            span {
                                                                class: "kn-room-failure-marker",
                                                                "aria-hidden": "true",
                                                                title: "{failure_marker_label(&sender_name, reason)}",
                                                                "ⓘ"
                                                            }
                                                        }
                                                    }
                                                    div { "{display_text}" }
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
                div { class: "kn-modal-hint--info", "{queued_for_next_round_notice(depth)}" }
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
                    disabled: !can_send,
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
        GroupSettingsDrawer { open: settings_open }
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
        }
    }

    fn member_msg(round: u32, name: &str, status: MemberTurnStatus, text: &str) -> GroupRoomMessage {
        msg(round, GroupRoomSpeaker::Member(name.to_string()), status, text)
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
}
