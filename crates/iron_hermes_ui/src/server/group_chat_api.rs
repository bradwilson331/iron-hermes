//! Phase 50.2 Plan 01 (D-20/D-21/D-05/D-04/D-06): the group-chat round
//! driver.
//!
//! **This module is a NEW CALLER of [`run_bot_handoff`], never a
//! re-derivation of it.** It defines no second env-scrub allowlist and no
//! second reply sanitizer — [`dispatch_member_turn`] is the single seam
//! every member turn funnels through, and it delegates straight to
//! [`crate::server::cli_handoff::run_bot_handoff`], the exact fn 50.1's own
//! `dispatch_bot_message` calls. The live-profile identity guard (D-04), the
//! `.env_clear()`-before-`.envs(...)` isolation (D-05), and the D-06
//! workspace default all come free through that one call — nothing here
//! duplicates them.
//!
//! **Parallel-within-round dispatch implements the 2026-08-18 operator
//! amendment to D-21.** [`run_group_rounds_with_settings`] fans every
//! member's turn out concurrently via a `tokio::task::JoinSet` of
//! `spawn_blocking(dispatch_member_turn)` calls, then joins all of them
//! before the round is scored. A member's failed turn (a `BotHandoffError`)
//! or a task join failure (a `JoinError`, e.g. a panicked blocking closure)
//! is recorded as DATA — that ONE member's `MemberTurnStatus::Failed` row —
//! never an early `?` that aborts the round. This mirrors upstream's own
//! per-member "a failed turn is a pass, never a room error"
//! (`plugin.js:3868-3872`) and is exactly what
//! [`crate::protocol::BotHandoffResult::error`]'s doc comment was added for.
//! Because a round now produces SEVERAL messages in join-completion order
//! with no precedence between them, the NEXT round's `@mention` and
//! self-exclusion semantics are resolved from that WHOLE set —
//! [`resolve_round_responders`] over the previous round's own messages,
//! never a single trailing transcript element — making resolution
//! independent of which subprocess exited last (Phase 50.2 Plan 13,
//! CR-02).
//!
//! **Frozen-delta-per-round (RESEARCH Pitfall 3, option (a)).** Every member
//! in a round receives the SAME snapshot of the room's message log, taken
//! BEFORE the fan-out starts — never a live mutable handle read during the
//! join loop. The 2026-08-18 amendment made turns parallel, which makes
//! same-round visibility between members structurally impossible: a member
//! reacts to its peers on the NEXT round, never within the current one.
//!
//! **`GROUP_TURN_HARD_CAP_MS` is deliberately NOT ported (Phase 50.2 Plan
//! 04, D-21 re-derivation, RESEARCH Pitfall 4).** Upstream's 20-minute
//! ceiling exists to extend a deadline while a session is *visibly still
//! working* — a signal [`run_bot_handoff`] does not have, since it blocks
//! until child exit or its own 180-second `BOT_HANDOFF_TIMEOUT_SECONDS`.
//! Porting the number without its poll/extend/harvest machinery would copy
//! a constant away from the mechanism that gives it meaning. 180s stands as
//! a genuine hard per-member cap, and a timed-out member's turn reads as
//! that member's failure — silence for settle purposes, never a room error.
//!
//! **Phase 50.2 Plan 04: multi-round settle/needs-you/mention-resolution is
//! real.** [`run_group_rounds_with_settings`] drives up to
//! `settings.max_rounds` serial rounds, settling the first time a round's
//! every responder passes or fails, bounded drive-wide (not per round) by
//! `settings.max_messages`. `dispatch_member_turn` is written for keeps:
//! plan 02 extended it once (a session-title parameter) and no call site
//! changes here — plan 04 only changes how many times, and with what
//! prompt, it gets called.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use crate::protocol::{
    GroupChatSettings, GroupMemberFailure, GroupRoomMessage, GroupRoomSpeaker, GroupRoundOutcome,
    MemberTurnStatus,
};

/// Phase 50.2 Plan 01: current wall-clock time in milliseconds. Duplicated
/// from `group_chat_store::now_ms`/`bot_meta_api::now_ms`/`cli_handoff::now_ms`
/// (each module-private) — the crate's own sanctioned "duplicate the trivial
/// helper" precedent.
#[cfg(feature = "server")]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 50.2 Plan 01: a message's sender rendered as a plain display label
/// for [`build_group_turn_prompt`]'s delta lines. Pure, no I/O.
#[cfg(feature = "server")]
fn speaker_label(speaker: &GroupRoomSpeaker) -> String {
    match speaker {
        GroupRoomSpeaker::Operator => "Operator".to_string(),
        GroupRoomSpeaker::Member(name) => name.clone(),
        GroupRoomSpeaker::System => "System".to_string(),
    }
}

/// Phase 50.2 Plan 02 (D-21): a room's per-member session title —
/// upstream's own naming convention (`Group: <room name>`), ported verbatim.
/// Pure, no I/O; `run_group_rounds_with_settings` is the sole caller.
#[cfg(feature = "server")]
pub(crate) fn group_session_title(room_name: &str) -> String {
    format!("Group: {room_name}")
}

/// Phase 50.2 Plan 01 (D-20/D-05) / Phase 50.2 Plan 02 (D-21) / Phase 50.2
/// Plan 14 (G-50.2-2b): **the single seam every member turn funnels
/// through.** Its body is a direct, un-widened call to
/// [`crate::server::cli_handoff::run_bot_handoff_tracked_in`] — `None` for
/// the workspace override is D-06's default (the target profile's own
/// `workspace/`). `session_title` is Plan 02's ONE extension of this seam —
/// the session-continuity gap RESEARCH finding 2 uncovered — threaded
/// straight through. Plan 14 makes the seam TRACKED: `registry` is an owned
/// `TurnRegistry` handle (a clone is another handle to the same inner map),
/// so a caller can pass either the production registry
/// (`crate::server::cli_handoff::handoff_turn_registry()`) or a fresh
/// per-test registry. Now `async` — callers awaiting a subprocess-transport
/// turn through this seam get register-before-spawn / deregister-on-exit
/// for free, and never need their own `spawn_blocking`.
///
/// Phase 50.2 Plan 16 (G-50.2-2c): `run_bot_handoff_tracked_in` now takes an
/// owned `InFlightGuard`. This fn acquires its own, unconditionally, via
/// `mark_bot_in_flight` — a round member turn always RUNS and never queues
/// (queueing a member's own round turn would deadlock the drive, this
/// plan's own prohibition), so it marks itself busy directly rather than
/// going through `begin_turn_or_queue`'s run-or-queue decision.
#[cfg(feature = "server")]
pub(crate) async fn dispatch_member_turn(
    registry: ironhermes_core::TurnRegistry,
    bot_name: &str,
    prompt: &str,
    session_title: Option<&str>,
) -> (
    String,
    Result<crate::protocol::BotHandoffResult, crate::server::cli_handoff::BotHandoffError>,
) {
    let in_flight = crate::server::handoff_steering::mark_bot_in_flight(bot_name);
    (
        bot_name.to_string(),
        crate::server::cli_handoff::run_bot_handoff_tracked_in(
            &registry,
            in_flight,
            bot_name,
            prompt,
            None,
            session_title,
        )
        .await,
    )
}

/// Phase 50.2 Plan 01: pure, I/O-free prompt builder — renders the frozen
/// room delta as the plain-text lines `dispatch_member_turn`'s child
/// receives as its `-q` message. Follows upstream's own sender-attribution
/// shape (`Message from <sender>: <text>` per line, RESEARCH's transcribed
/// reference) and ends with a brief instruction that the bot may reply or
/// pass. Deliberately does NOT teach the bot a shell command to run
/// (RESEARCH Pitfall 1 — upstream's SOUL-taught async CLI form is the WRONG
/// mechanism here; this phase is host-orchestrated, per D-20).
#[cfg(feature = "server")]
pub(crate) fn build_group_turn_prompt(
    room_name: &str,
    member_name: &str,
    delta_lines: &[(String, String)],
) -> String {
    let mut out = format!(
        "You are \"{member_name}\" in the group chat room \"{room_name}\".\n\n"
    );
    for (sender, text) in delta_lines {
        out.push_str(&format!("Message from {sender}: {text}\n"));
    }
    out.push_str(
        "\nReply to the room, or reply with exactly \"(pass)\" if you have nothing to add this round.",
    );
    out
}

// ---------------------------------------------------------------------
// Phase 50.2 Plan 04 (D-21, D-03): pure, I/O-free orchestration rules,
// transcribed verbatim from `plugin.js` (the canonical source cited per
// function below) and from the transcribed test suite `group-chat.test.mjs`.
// None of these take `tokio`, `Signal`, or store access — every one is
// callable from a plain `#[test]` with no async runtime, exactly the
// isolation upstream's own `vm.runInNewContext` harness gives its JS
// originals (RESEARCH Pattern 1).
// ---------------------------------------------------------------------

/// Phase 50.2 Plan 04 (D-21, `plugin.js:3342`): a message's @-mentions.
/// `handles` is lowercased, first-seen-ordered, and de-duplicated. The
/// literal `user` handle is never collected here — it is the operator-
/// escalation signal, handled independently by [`message_addresses_user`],
/// never a responder-resolution target.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct GroupMentionSet {
    pub(crate) handles: Vec<String>,
    pub(crate) everyone: bool,
}

/// Phase 50.2 Plan 04 (D-21, `plugin.js:3342`): ported verbatim from
/// `/@([a-z0-9][a-z0-9._-]*)/gi`. `@everyone`/`@all` set `everyone: true`
/// with no handle collected for either literal keyword; `@user` is skipped
/// (see [`message_addresses_user`] for the escalation channel it belongs
/// to). The regex is intentionally unanchored to word boundaries, exactly
/// as upstream's — an email address like `a@b.com` matches `b.com` as a
/// mention, ported verbatim rather than "fixed" (the truth table's own
/// explicit case); non-member matches are filtered later by
/// [`resolve_group_responders`], not here.
#[cfg(feature = "server")]
pub(crate) fn parse_group_mentions(text: &str) -> GroupMentionSet {
    static MENTION_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = MENTION_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)@([a-z0-9][a-z0-9._-]*)").expect("static mention regex")
    });

    let mut handles = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut everyone = false;
    for cap in re.captures_iter(text) {
        let handle = cap[1].to_lowercase();
        match handle.as_str() {
            "everyone" | "all" => everyone = true,
            "user" => {}
            _ => {
                if seen.insert(handle.clone()) {
                    handles.push(handle);
                }
            }
        }
    }
    GroupMentionSet { handles, everyone }
}

/// Phase 50.2 Plan 04 (D-21, `plugin.js:3301-3309`): ported verbatim from
/// `/^\(?\s*pass\s*\)?\.?$/i`, applied to the trimmed text (an empty or
/// whitespace-only trimmed text is a pass by upstream's own short-circuit,
/// before the regex ever runs).
#[cfg(feature = "server")]
pub(crate) fn is_group_pass_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    static PASS_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = PASS_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)^\(?\s*pass\s*\)?\.?$").expect("static pass regex")
    });
    re.is_match(trimmed)
}

/// Phase 50.2 Plan 04 (D-21, `plugin.js:3342` + `resolveGroupResponders`):
/// no mentions, or `everyone`/`all`, means every member except `speaker`
/// responds. Named mentions resolve to exactly those members present in
/// `members`, in `members`' own order — never the mention list's order. A
/// mention naming no actual member falls back to everyone (a message
/// mentioning only outsiders is not a silent no-op). The speaker never
/// responds to itself, in every branch.
///
/// Phase 50.2 Plan 13 (CR-02): [`resolve_round_responders`] is the caller
/// above this fn in the round loop, and supplies an empty `speaker` string
/// for a multi-participant round — `speaker_label` never produces an empty
/// string for any real speaker, so an empty `speaker` here excludes no
/// one. This fn's own semantics, called with a real speaker, are
/// unchanged.
#[cfg(feature = "server")]
pub(crate) fn resolve_group_responders(
    members: &[String],
    mentions: &GroupMentionSet,
    speaker: &str,
) -> Vec<String> {
    let everyone_except_speaker = |members: &[String]| -> Vec<String> {
        members
            .iter()
            .filter(|m| m.as_str() != speaker)
            .cloned()
            .collect()
    };

    if mentions.everyone || mentions.handles.is_empty() {
        return everyone_except_speaker(members);
    }

    let named: Vec<String> = members
        .iter()
        .filter(|m| {
            m.as_str() != speaker
                && mentions.handles.iter().any(|h| h.eq_ignore_ascii_case(m))
        })
        .cloned()
        .collect();

    if named.is_empty() {
        // A message mentioning only outsiders is not a silent no-op.
        everyone_except_speaker(members)
    } else {
        named
    }
}

/// Phase 50.2 Plan 13 (CR-02 fix): folds [`parse_group_mentions`] over
/// every message's `text` in a round. `everyone` is the logical OR across
/// rows; `handles` accumulate each row's handles in encounter order,
/// de-duplicated with the same `HashSet` guard [`parse_group_mentions`]
/// uses internally. Does NOT re-implement the mention regex — it calls
/// [`parse_group_mentions`] once per row.
#[cfg(feature = "server")]
pub(crate) fn union_group_mentions(round_messages: &[GroupRoomMessage]) -> GroupMentionSet {
    let mut handles = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut everyone = false;
    for msg in round_messages {
        let mentions = parse_group_mentions(&msg.text);
        if mentions.everyone {
            everyone = true;
        }
        for handle in mentions.handles {
            if seen.insert(handle.clone()) {
                handles.push(handle);
            }
        }
    }
    GroupMentionSet { handles, everyone }
}

/// Phase 50.2 Plan 13 (CR-02 fix): the single seam the round loop calls to
/// resolve the NEXT round's responders. Before the 2026-08-18 parallel
/// amendment a round always produced exactly one message, so reading
/// `transcript.messages.last()` and feeding it to
/// [`resolve_group_responders`] was correct. A round now produces several
/// messages in `tokio::task::JoinSet` completion order with no precedence
/// between them, so resolving from a single trailing element let
/// `@mention` addressing and self-exclusion be decided by whichever
/// member's subprocess happened to exit last (CR-02) — a member's
/// `@mention` was silently discarded whenever another member's task
/// finished after it.
///
/// This fn makes resolution a pure function of `prev_round_messages` that
/// is INVARIANT under permutation of that slice, which is what retires the
/// completion-order dependence:
///
/// - Exactly ONE message (round 1's operator kickoff, or any
///   single-responder round): delegates to [`resolve_group_responders`]
///   with that message's own mentions and speaker — byte-for-byte today's
///   semantics.
/// - Zero or two-plus messages: delegates to [`resolve_group_responders`]
///   with [`union_group_mentions`] of the whole slice and an empty speaker
///   string, so nobody is excluded. Excluding EVERY speaker of a
///   multi-participant round would leave a 2-member room with zero
///   responders and deadlock the drive; excluding one arbitrarily is
///   exactly the CR-02 defect. An empty `prev_round_messages` falls back
///   to every member so a bookkeeping miss can never silently produce an
///   empty responder set.
#[cfg(feature = "server")]
pub(crate) fn resolve_round_responders(
    members: &[String],
    prev_round_messages: &[GroupRoomMessage],
) -> Vec<String> {
    if let [msg] = prev_round_messages {
        resolve_group_responders(
            members,
            &parse_group_mentions(&msg.text),
            &speaker_label(&msg.from),
        )
    } else {
        resolve_group_responders(members, &union_group_mentions(prev_round_messages), "")
    }
}

/// Phase 50.2 Plan 04 (D-21, `plugin.js:3582-3593`): ported verbatim from
/// `/@user\b/i`. Word-boundary anchored, so `@username` (a member's own
/// name, not the operator) never matches. This is the operator-escalation
/// trigger, checked on every appended member message INDEPENDENTLY of
/// whether the round settles — never folded into the settle branch.
#[cfg(feature = "server")]
pub(crate) fn message_addresses_user(text: &str) -> bool {
    static USER_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = USER_RE.get_or_init(|| regex::Regex::new(r"(?i)@user\b").expect("static user regex"));
    re.is_match(text)
}

/// Phase 50.2 Plan 04 (D-21): returns at most `limit` most-recent messages,
/// preserving order. A log shorter than `limit` is returned whole.
/// `limit == 0` returns empty.
#[cfg(feature = "server")]
pub(crate) fn trim_room_history(
    messages: &[GroupRoomMessage],
    limit: usize,
) -> Vec<GroupRoomMessage> {
    if limit == 0 || messages.is_empty() {
        return Vec::new();
    }
    let start = messages.len().saturating_sub(limit);
    messages[start..].to_vec()
}

/// Phase 50.2 Plan 04 (D-21, upstream's per-member `watermarks` map):
/// returns only the messages at indices at or after `watermark`. A
/// watermark past the end of `messages` returns empty; a watermark of `0`
/// returns everything.
#[cfg(feature = "server")]
pub(crate) fn member_delta(messages: &[GroupRoomMessage], watermark: usize) -> Vec<GroupRoomMessage> {
    if watermark >= messages.len() {
        return Vec::new();
    }
    messages[watermark..].to_vec()
}

/// Phase 50.2 Plan 04 (D-21, `plugin.js:3896-3898` settle +
/// `plugin.js:3582-3593` needs-you): the result of scoring one round's
/// member messages.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct RoundScore {
    /// Count of this round's messages that are neither a pass nor a
    /// failure — upstream's `spokeThisRound`.
    pub(crate) spoke: u32,
    /// `spoke == 0` — a full round of silence (every responder passed or
    /// failed) ends the conversation (`plugin.js:3896-3898`).
    pub(crate) settled: bool,
    /// `true` when any non-pass message in this round satisfies
    /// [`message_addresses_user`]. A `Passed`/`Failed` message's text is
    /// never checked, even if it happens to contain `@user`.
    pub(crate) needs_you: bool,
}

/// Phase 50.2 Plan 04 (D-21): scores one round's already-appended member
/// messages. Pure — takes exactly the round's own `GroupRoomMessage` rows,
/// nothing else.
#[cfg(feature = "server")]
pub(crate) fn score_round(round_messages: &[GroupRoomMessage]) -> RoundScore {
    let mut spoke = 0u32;
    let mut needs_you = false;
    for msg in round_messages {
        if matches!(msg.status, MemberTurnStatus::Replied) {
            spoke += 1;
            if message_addresses_user(&msg.text) {
                needs_you = true;
            }
        }
    }
    RoundScore {
        spoke,
        settled: spoke == 0,
        needs_you,
    }
}

/// Phase 50.2 Plan 06 (D-21 integration): sources the round loop's bounds
/// from the persisted, operator-editable record —
/// [`crate::server::group_settings_api::load_group_settings_impl`] (Phase
/// 50.2 Plan 05, `pub(crate)` and fully tested). This is the ONE line D-21's
/// full round trip depends on: "lower MAX ROUNDS in the drawer, the next
/// drive runs fewer rounds." A record that fails to parse (a corrupted
/// settings file) falls back to [`GroupChatSettings::default`] rather than
/// aborting the room's drive — a corrupted settings file must never prevent
/// a room from running; it runs on the shipped defaults and the settings
/// drawer surfaces the corruption on its own next load, independently of
/// this fn. Isolating the read to this one-line helper is what plan 04 set
/// up this fn to do — this plan changes only its body, never the driver's
/// own loop.
#[cfg(feature = "server")]
fn group_chat_settings_for_drive() -> GroupChatSettings {
    crate::server::group_settings_api::load_group_settings_impl()
        .unwrap_or_else(|_| GroupChatSettings::default())
}

/// Phase 50.2 Plan 01/04: the round driver's real work, factored out of the
/// `#[server]` wrapper below so it is directly unit-testable inside a
/// `#[tokio::test]` without going through the Dioxus server-fn machinery
/// (no precedent in this crate for testing a `#[server]` fn directly — every
/// existing test module tests the impl layer, e.g. `run_bot_handoff` rather
/// than `dispatch_bot_message`). `dispatch_group_round` below is a thin
/// four-step wrapper over this fn, exactly as `dispatch_bot_message`
/// (`cli_handoff.rs:740`) wraps `run_bot_handoff`.
///
/// Sources its bounds from [`group_chat_settings_for_drive`] — see that
/// fn's doc for the plan-05/plan-06-style handoff this defers.
#[cfg(feature = "server")]
pub(crate) async fn run_group_rounds(
    room_id: &str,
    message: &str,
) -> Result<GroupRoundOutcome, crate::server::group_chat_store::GroupChatError> {
    run_group_rounds_with_settings(
        room_id,
        message,
        group_chat_settings_for_drive(),
        crate::server::cli_handoff::handoff_turn_registry(),
    )
    .await
}

/// Phase 50.2 Plan 04 (D-21 + the 2026-08-18 amendment): up to
/// `settings.max_rounds` SERIAL rounds of PARALLEL member turns, settling
/// on a full round of silence, bounded drive-wide (not per round) by
/// `settings.max_messages`.
///
/// **Frozen-delta-per-round (RESEARCH Pitfall 3, option (a)).** Every
/// member in a round receives the SAME snapshot of the room's message log,
/// taken BEFORE that round's fan-out starts — every responder's prompt is
/// built into an owned `String` before the `JoinSet` is spawned, so no room
/// state is read after the fan-out begins. The 2026-08-18 amendment made
/// turns parallel, which makes same-round visibility between members
/// structurally impossible: two concurrent subprocesses cannot read each
/// other's not-yet-produced replies. A member reacts to its peers on the
/// NEXT round, never within the current one.
///
/// **`GROUP_TURN_HARD_CAP_MS` is deliberately NOT ported (D-21
/// re-derivation, RESEARCH Pitfall 4).** Upstream's 20-minute ceiling
/// exists to extend a deadline while a session is *visibly still working* —
/// a signal [`crate::server::cli_handoff::run_bot_handoff`] does not have,
/// since it blocks until child exit or its own 180-second timeout
/// (`BOT_HANDOFF_TIMEOUT_SECONDS`). Porting the number without its
/// poll/extend/harvest machinery would copy a constant away from the
/// mechanism that gives it meaning. 180s stands as a genuine hard
/// per-member cap, and a timed-out member's [`BotHandoffError::Timeout`]
/// reads as that member's failed turn — silence for settle purposes, never
/// a room error — consistent with upstream's own "a failed turn is a pass,
/// never a room error."
///
/// **Per-member watermarks are local to this one drive.** Every member
/// starts this call's round 1 at watermark `0` (the full persisted room
/// history, bounded by `settings.history_limit`) and advances only across
/// THIS call's own rounds — no protocol or store field exists yet to carry
/// a watermark across two separate operator-triggered drives, so that is
/// out of this plan's scope, not a silent gap.
///
/// **The drive-wide message cap is checked at round boundaries, not
/// mid-fan-out.** Upstream's own serial driver can check `posted` before
/// every member's turn; the 2026-08-18 amendment's parallel-within-round
/// dispatch already has every responder's turn in flight before any of
/// them completes, so there is no serial point to interrupt. The cap is
/// still drive-wide (the counter is declared outside the round loop,
/// exactly as upstream's), just enforced once per round rather than once
/// per member.
///
/// **Post-condition (Phase 50.2 Plan 13, CR-02 fix): round-to-round
/// resolution is order-independent.** At the end of every round this fn
/// assigns `prev_round_messages` from that round's own `round_messages` —
/// the same set `score_round` just scored — and the NEXT round's
/// responders are resolved from that whole set via
/// [`resolve_round_responders`], never from `transcript.messages.last()`.
/// A member's `@mention` in a multi-reply round reaches its target on the
/// next round regardless of which subprocess exits last.
///
/// [`BotHandoffError::Timeout`]: crate::server::cli_handoff::BotHandoffError::Timeout
#[cfg(feature = "server")]
pub(crate) async fn run_group_rounds_with_settings(
    room_id: &str,
    message: &str,
    settings: GroupChatSettings,
    registry: ironhermes_core::TurnRegistry,
) -> Result<GroupRoundOutcome, crate::server::group_chat_store::GroupChatError> {
    use crate::server::group_chat_store::GroupChatError;

    // Step 1: append the operator's message so it is durable BEFORE any
    // dispatch — a crash mid-drive never loses the operator's own input.
    let operator_msg = GroupRoomMessage {
        from: GroupRoomSpeaker::Operator,
        text: message.to_string(),
        at_ms: now_ms(),
        round: 1,
        status: MemberTurnStatus::Replied,
    };
    // Phase 50.2 Plan 13 (CR-02 fix): seeded here, before `operator_msg` is
    // moved into the persist closure below, so round 1's responders are
    // resolved from the operator's own message via
    // `resolve_round_responders`'s single-message branch — byte-for-byte
    // today's semantics.
    let mut prev_round_messages: Vec<GroupRoomMessage> = vec![operator_msg.clone()];
    let room_id_owned = room_id.to_string();
    let mut transcript = tokio::task::spawn_blocking(move || {
        crate::server::group_chat_store::append_room_messages_impl(
            &room_id_owned,
            vec![operator_msg],
        )
    })
    .await
    .map_err(|e| GroupChatError::StoreIo {
        reason: format!("spawn_blocking join: {e}"),
    })??;

    // Phase 50.2 Plan 06 (D-03): upstream's escalate-until-addressed
    // semantics — the operator sending a message INTO the room is the
    // attention the needs-you badge was requesting, so the flag clears
    // here, immediately after the operator's message is durable and BEFORE
    // this round's (or any round's) responders are resolved below. A
    // member reply containing `@user` later in THIS SAME drive re-raises it
    // independently (the `score.needs_you` branch further down, unchanged
    // from plan 04) — clear-then-possibly-reraise is what lets both
    // behaviors coexist in one drive.
    let room_id_for_clear = room_id.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        crate::server::group_chat_store::set_room_needs_you_impl(&room_id_for_clear, false)
    })
    .await;

    let room_name = transcript.room.name.clone();
    let members = transcript.room.members.clone();
    // Phase 50.2 Plan 02 (D-21): every member turn in this room carries the
    // SAME `Group: <room name>` session title across every round of this
    // drive, so each bot resumes its own persistent per-room session
    // rather than starting from a blank slate on every dispatch.
    let session_title = group_session_title(&room_name);
    let history_limit = settings.history_limit as usize;

    let mut watermarks: std::collections::HashMap<String, usize> =
        members.iter().map(|m| (m.clone(), 0usize)).collect();

    let mut rounds_run = 0u32;
    let mut posted = 0u32; // drive-wide, declared OUTSIDE the round loop.
    let mut settled = false;
    let mut needs_you = false; // sticky: once raised, stays raised.
    let mut failures = Vec::new();
    let mut total_member_rows = 0u32;

    'rounds: for _round in 1..=settings.max_rounds {
        rounds_run += 1;
        let round_number = rounds_run;

        // Phase 50.2 Plan 18 (G-50.2-2c): drain this room's steering queue
        // at the TOP of every round, BEFORE `round_start_len` is read and
        // BEFORE `resolve_round_responders` runs — this ordering is the
        // whole point, because it is what puts an injected row inside
        // `union_group_mentions`'s view of `prev_round_messages` for THIS
        // round's resolution, and inside every responder's `member_delta`
        // for their prompt. An empty drain leaves the round's control flow
        // byte-for-byte unchanged from before this plan: no persist call,
        // no `prev_round_messages` mutation.
        let drained_steering = crate::server::handoff_steering::drain_room_steering(room_id);
        if !drained_steering.is_empty() {
            let injected: Vec<GroupRoomMessage> = drained_steering
                .into_iter()
                .map(|text| GroupRoomMessage {
                    from: GroupRoomSpeaker::Operator,
                    text,
                    at_ms: now_ms(),
                    round: round_number,
                    status: MemberTurnStatus::Replied,
                })
                .collect();
            prev_round_messages.extend(injected.iter().cloned());
            let room_id_owned = room_id.to_string();
            transcript = tokio::task::spawn_blocking(move || {
                crate::server::group_chat_store::append_room_messages_impl(
                    &room_id_owned,
                    injected,
                )
            })
            .await
            .map_err(|e| GroupChatError::StoreIo {
                reason: format!("spawn_blocking join: {e}"),
            })??;
        }

        // Step 2 (Phase 50.2 Plan 13, CR-02 fix): this round's responders
        // are resolved from the PREVIOUS round's own message set — the
        // same set `score_round` scores — never from a single trailing
        // transcript element. `resolve_round_responders` is
        // permutation-invariant over `prev_round_messages`, which is what
        // makes resolution independent of which subprocess exited last.
        // Every responder's delta is snapshotted as a VALUE at the round's
        // starting length — the frozen-delta guarantee this fn's own doc
        // comment describes.
        let round_start_len = transcript.messages.len();
        let responders = resolve_round_responders(&members, &prev_round_messages);

        // Step 3: build every responder's prompt BEFORE the JoinSet is
        // spawned — no room state is read after the fan-out begins.
        let mut set = tokio::task::JoinSet::new();
        let mut id_to_name: std::collections::HashMap<tokio::task::Id, String> =
            std::collections::HashMap::new();
        for member in &responders {
            let watermark = *watermarks.get(member).unwrap_or(&0);
            let delta = member_delta(&transcript.messages, watermark);
            let delta = trim_room_history(&delta, history_limit);
            let delta_lines: Vec<(String, String)> = delta
                .iter()
                .map(|m| (speaker_label(&m.from), m.text.clone()))
                .collect();
            let prompt = build_group_turn_prompt(&room_name, member, &delta_lines);
            let member_owned = member.clone();
            let session_title_owned = session_title.clone();
            let registry_for_task = registry.clone();
            let handle = set.spawn(async move {
                dispatch_member_turn(
                    registry_for_task,
                    &member_owned,
                    &prompt,
                    Some(&session_title_owned),
                )
                .await
            });
            id_to_name.insert(handle.id(), member.clone());
        }

        // Step 4: a JoinError and a BotHandoffError are BOTH treated as
        // that ONE member's failed turn; neither aborts the round nor
        // propagates through `?` (D-20's "a failed turn is data" contract).
        let mut turn_results: Vec<(
            String,
            Result<crate::protocol::BotHandoffResult, crate::server::cli_handoff::BotHandoffError>,
        )> = Vec::with_capacity(responders.len());
        while let Some(joined) = set.join_next_with_id().await {
            match joined {
                Ok((_, (name, result))) => turn_results.push((name, result)),
                Err(join_err) => {
                    let name = id_to_name
                        .get(&join_err.id())
                        .cloned()
                        .unwrap_or_else(|| "unknown member".to_string());
                    turn_results.push((
                        name,
                        Err(crate::server::cli_handoff::BotHandoffError::SpawnFailed {
                            reason: join_err.to_string(),
                        }),
                    ));
                }
            }
        }

        // Step 5: classify each result. A successful reply that reads as a
        // pass (`is_group_pass_text`) is `Passed`, never `Replied` — this
        // is what makes `score_round`'s `spoke` count correct. A failure is
        // `Failed { reason }` carrying only the BotHandoffError's Display
        // string — never raw child output, never an env value, never a raw
        // `.env` line (the CR-05/CR-06 discipline `cli_handoff.rs`
        // records).
        let now = now_ms();
        let mut round_messages = Vec::with_capacity(turn_results.len());
        for (name, result) in turn_results {
            match result {
                Ok(handoff) => {
                    let status = if is_group_pass_text(&handoff.reply) {
                        MemberTurnStatus::Passed
                    } else {
                        MemberTurnStatus::Replied
                    };
                    round_messages.push(GroupRoomMessage {
                        from: GroupRoomSpeaker::Member(name),
                        text: handoff.reply,
                        at_ms: now,
                        round: round_number,
                        status,
                    });
                }
                Err(err) => {
                    let reason = err.to_string();
                    failures.push(GroupMemberFailure {
                        member: name.clone(),
                        reason: reason.clone(),
                    });
                    round_messages.push(GroupRoomMessage {
                        from: GroupRoomSpeaker::Member(name),
                        text: String::new(),
                        at_ms: now,
                        round: round_number,
                        status: MemberTurnStatus::Failed { reason },
                    });
                }
            }
        }

        // Step 6: advance each participating member's watermark to the
        // round's STARTING room length — it has now seen everything up to
        // and including that point.
        for member in &responders {
            watermarks.insert(member.clone(), round_start_len);
        }

        // Score THIS round's messages only (never the persisted whole
        // transcript) — settle and needs-you are both round-scoped.
        let score = score_round(&round_messages);

        // needs-you is checked on every appended member message
        // INDEPENDENTLY of the settle branch (plugin.js:3582-3593) — not
        // folded into "the room settled and asked for the user." Persisted
        // the moment it's raised, so the roster row badges even if the
        // drive is still running.
        if score.needs_you && !needs_you {
            needs_you = true;
            let room_id_for_flag = room_id.to_string();
            let _ = tokio::task::spawn_blocking(move || {
                crate::server::group_chat_store::set_room_needs_you_impl(&room_id_for_flag, true)
            })
            .await;
        }

        // Step 7: increment the drive-wide posted counter for each
        // non-pass, non-failed reply — `posted` bounds the ENTIRE drive
        // across all rounds, not each round individually.
        total_member_rows += round_messages.len() as u32;
        for msg in &round_messages {
            if matches!(msg.status, MemberTurnStatus::Replied) {
                posted += 1;
            }
        }

        // Phase 50.2 Plan 13 (CR-02 fix): seed the NEXT round's resolution
        // from THIS round's own message set, assigned from `round_messages`
        // before it is moved into the persist closure below — it can never
        // diverge from what `score_round` just scored.
        prev_round_messages = round_messages.clone();

        // Persist this round's messages.
        let room_id_owned = room_id.to_string();
        transcript = tokio::task::spawn_blocking(move || {
            crate::server::group_chat_store::append_room_messages_impl(
                &room_id_owned,
                round_messages,
            )
        })
        .await
        .map_err(|e| GroupChatError::StoreIo {
            reason: format!("spawn_blocking join: {e}"),
        })??;

        // Step 8: stop early the first time a round scores settled.
        if score.settled {
            settled = true;
            break 'rounds;
        }

        // Stop when the drive-wide cap is reached — checked at this round
        // boundary (see this fn's own doc comment for why not mid-round).
        if posted >= settings.max_messages {
            break 'rounds;
        }
    }

    Ok(GroupRoundOutcome {
        rounds_run,
        messages_appended: total_member_rows,
        settled,
        needs_you,
        failures,
    })
}

/// Phase 50.2 Plan 19 (CR-01): the group-round drive's detached wrapper —
/// rides the SAME [`crate::server::cli_handoff::await_detached`] seam the
/// Bot Chat and `@mention` dispatch paths use. The `async move` block MOVES
/// `guard` in and drops it explicitly only after [`run_group_rounds`]
/// itself resolves — the same explicit-drop-after-the-whole-drive
/// discipline `dispatch_group_round` used to apply inline, now inside the
/// task that actually owns it, so the room reopens only when the drive
/// itself ends, never merely because the request that started it went
/// away. A `JoinError` (the detached task itself panicking) flattens into
/// [`crate::server::group_chat_store::GroupChatError::StoreIo`], reusing
/// the variant this module already uses for `spawn_blocking` join failures
/// rather than adding a new one.
#[cfg(feature = "server")]
pub(crate) async fn run_group_rounds_detached(
    guard: crate::server::handoff_steering::RoomDriveGuard,
    room_id: String,
    message: String,
) -> Result<GroupRoundOutcome, crate::server::group_chat_store::GroupChatError> {
    let fut = async move {
        let outcome = run_group_rounds(&room_id, &message).await;
        // Release the room only after the whole drive has finished —
        // success or error alike — never before.
        drop(guard);
        outcome
    };
    match crate::server::cli_handoff::await_detached(fut).await {
        Ok(outcome) => outcome,
        Err(e) => Err(crate::server::group_chat_store::GroupChatError::StoreIo {
            reason: format!("detached round-drive task: {e}"),
        }),
    }
}

/// Phase 50.2 Plan 01/04: dispatch a group-chat room's round drive.
/// Four-step protocol, exactly as `dispatch_bot_message` (`cli_handoff.rs:
/// 740`) does it: `Config::load` -> `profile_api::check_profile_write_gate`
/// -> the real work ([`run_group_rounds`]) -> map errors to
/// `ServerFnError::new`.
///
/// Phase 50.2 Plan 18 (G-50.2-2c): the write gate is checked BEFORE
/// [`crate::server::handoff_steering::begin_room_drive_or_queue`] is ever
/// called, so the room's steering queue is never a back door around it
/// (T-50.2-18-06).
///
/// Phase 50.2 Plan 19 (CR-01): on the run-now branch, the returned
/// [`crate::server::handoff_steering::RoomDriveGuard`] no longer lives on
/// THIS fn's own await — it moves into [`run_group_rounds_detached`], which
/// runs the whole drive on its own task and holds the guard there for the
/// WHOLE drive, including its error path, dropping it only once the drive
/// is fully finished. That guard living on a detached task — not on the
/// one-shot `#[server]` fn's own request future — is what makes two
/// concurrent drives on the same room structurally impossible
/// (T-50.2-18-01) even when the first drive's own request is dropped. On
/// the queued branch, no drive is started here at all.
#[server]
pub async fn dispatch_group_round(
    req: crate::protocol::DispatchGroupRoundRequest,
) -> Result<crate::protocol::GroupRoundDispatch, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        match crate::server::handoff_steering::begin_room_drive_or_queue(
            &req.room_id,
            &req.message,
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?
        {
            crate::server::handoff_steering::BeginRoomDriveOrQueue::Queued { depth } => {
                Ok(crate::protocol::GroupRoundDispatch::Queued { depth })
            }
            crate::server::handoff_steering::BeginRoomDriveOrQueue::Begin(guard) => {
                let outcome =
                    run_group_rounds_detached(guard, req.room_id, req.message).await;
                outcome
                    .map(crate::protocol::GroupRoundDispatch::Ran)
                    .map_err(|e| ServerFnError::new(e.to_string()))
            }
        }
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "dispatch_group_round unavailable without `server` feature",
        ))
    }
}

/// Phase 50.2 Plan 01: create a group-chat room. Follows the crate's
/// four-step write protocol.
#[server]
pub async fn create_group_room(
    req: crate::protocol::CreateGroupRoomRequest,
) -> Result<crate::protocol::GroupRoom, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || {
            crate::server::group_chat_store::create_room_impl(&req.name, &req.members)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = req;
        Err(ServerFnError::new(
            "create_group_room unavailable without `server` feature",
        ))
    }
}

/// Phase 50.2 Plan 01: list every group-chat room — a read, so it is
/// ungated by `security.web_config_write_enabled` (mirrors `list_bot_meta`'s
/// own read-path exemption).
#[server]
pub async fn list_group_rooms() -> Result<Vec<crate::protocol::GroupRoomSummary>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(crate::server::group_chat_store::list_rooms_impl)
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "list_group_rooms unavailable without `server` feature",
        ))
    }
}

/// Phase 50.2 Plan 01: load one room's full transcript — a read, ungated by
/// the write gate (mirrors `list_group_rooms` above).
#[server]
pub async fn load_group_room(
    room_id: String,
) -> Result<crate::protocol::GroupRoomTranscript, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(move || {
            crate::server::group_chat_store::load_room_impl(&room_id)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = room_id;
        Err(ServerFnError::new(
            "load_group_room unavailable without `server` feature",
        ))
    }
}

/// Phase 50.2 Plan 01: delete a group-chat room. Follows the crate's
/// four-step write protocol.
#[server]
pub async fn delete_group_room(room_id: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || {
            crate::server::group_chat_store::delete_room_impl(&room_id)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = room_id;
        Err(ServerFnError::new(
            "delete_group_room unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// RAII guard, duplicated from `cli_handoff.rs`'s / `group_chat_store.rs`'s
    /// own `ScopedEnv` — each `#[cfg(test)]` module is its own namespace.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; env_lock() below
            // serializes every test in this crate that mutates process env.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    /// Writes a `#!/bin/sh` stub at `dir/name`, `chmod +x` on unix.
    /// Duplicated from `cli_handoff.rs`'s own `write_stub_script`.
    fn write_stub_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write stub script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("set_permissions 0755");
        }
        path
    }

    // -------------------------------------------------------------------
    // group_session_title (Phase 50.2 Plan 02)
    // -------------------------------------------------------------------

    #[test]
    fn group_session_title_formats_with_group_prefix() {
        assert_eq!(group_session_title("standup"), "Group: standup");
    }

    // -------------------------------------------------------------------
    // build_group_turn_prompt
    // -------------------------------------------------------------------

    #[test]
    fn build_group_turn_prompt_is_deterministic_for_a_fixed_input() {
        let delta = vec![("Operator".to_string(), "hello room".to_string())];
        let a = build_group_turn_prompt("Ops Room", "scout", &delta);
        let b = build_group_turn_prompt("Ops Room", "scout", &delta);
        assert_eq!(a, b, "same input must always render the same prompt");
        assert!(a.contains("Ops Room"));
        assert!(a.contains("scout"));
        assert!(a.contains("Message from Operator: hello room"));
    }

    // -------------------------------------------------------------------
    // run_group_rounds / run_group_rounds_with_settings — the multi-round
    // driver (Phase 50.2 Plan 04)
    // -------------------------------------------------------------------

    // `env_lock()`'s std::sync::MutexGuard is held across this test's
    // `.await` points deliberately — this async test body is the only thing
    // in the current tokio runtime touching process env, and this module's
    // own `<verify>` command runs `--test-threads=1`, so there is no
    // concurrent thread this guard could deadlock against. Same precedent
    // `ironhermes-vault/tests/rusty_vault_spike.rs:97` already established
    // for exactly this shape.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_one_member_fails_others_pass_and_round_settles() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig", "badbot"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(
            dir.path(),
            "group-stub.sh",
            "if [ \"$2\" = \"badbot\" ]; then exit 3; else echo \"(pass)\"; fi",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string(), "badbot".to_string()],
        )
        .expect("create_room_impl should succeed");

        let outcome = run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must return Ok even with one failing member");

        // A failed turn counts as silence for settle purposes exactly like a
        // pass — the round still completes and settles after round 1.
        assert_eq!(outcome.rounds_run, 1);
        assert!(outcome.settled);
        assert_eq!(
            outcome.failures.len(),
            1,
            "exactly one member's turn must be recorded as a failure"
        );
        assert_eq!(outcome.failures[0].member, "badbot");

        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");
        // operator message (round 1) + 3 member turns = 4 rows.
        assert_eq!(transcript.messages.len(), 4);

        let member_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| matches!(m.from, crate::protocol::GroupRoomSpeaker::Member(_)))
            .collect();
        assert_eq!(member_rows.len(), 3, "every member must produce a transcript row");

        let passed = member_rows
            .iter()
            .filter(|m| matches!(m.status, MemberTurnStatus::Passed))
            .count();
        let failed = member_rows
            .iter()
            .filter(|m| matches!(m.status, MemberTurnStatus::Failed { .. }))
            .count();
        assert_eq!(passed, 2, "the two healthy members must be Passed, not Replied");
        assert_eq!(failed, 1, "the failing member must be Failed, not aborting the round");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_full_round_of_passes_settles_after_one_round() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig", "ada"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(dir.path(), "all-pass-stub.sh", "echo \"(pass)\"");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string(), "ada".to_string()],
        )
        .expect("create_room_impl should succeed");

        let outcome = run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        assert_eq!(outcome.rounds_run, 1);
        assert!(outcome.settled);
        assert!(outcome.failures.is_empty());
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_talkative_room_runs_exactly_max_rounds_and_does_not_settle() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig", "ada"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(dir.path(), "always-reply-stub.sh", "echo \"keep talking\"");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string(), "ada".to_string()],
        )
        .expect("create_room_impl should succeed");

        // Default settings: max_rounds = 3, max_messages = 10. Three members
        // replying non-pass every round posts 3/round — never hits the
        // drive-wide cap before the round ceiling does.
        let outcome = run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        assert_eq!(outcome.rounds_run, GroupChatSettings::default().max_rounds);
        assert!(!outcome.settled);
    }

    // `env_lock()`'s std::sync::MutexGuard is held across this test's
    // `.await` points deliberately — same precedent as the driver tests
    // above; this module's own `<verify>` command runs `--test-threads=1`.
    //
    // CR-02 regression (D-24 mutation bar). Forces zig's task to be the
    // LAST to complete in round 1 by sleeping 1s in its stub, so
    // `transcript.messages.last()` is deterministically zig's mention-free
    // reply — the worst case for the pre-fix driver, which resolved round
    // 2's responders from that single trailing element instead of the
    // whole round's own message set.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_mention_from_a_multi_reply_round_reaches_its_target_next_round() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig", "nova"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        // `$2` is the profile name (build_bot_handoff_argv's `--profile
        // <name>` positional). scout mentions @nova; zig sleeps 1s so its
        // task is provably the last to finish; every other member (nova)
        // replies with a third, distinct, mention-free message. None of
        // the three texts match `is_group_pass_text` or contain `@user`.
        let stub = write_stub_script(
            dir.path(),
            "mention-order-stub.sh",
            "if [ \"$2\" = \"scout\" ]; then\n  echo \"hey @nova please check the deploy log\"\nelif [ \"$2\" = \"zig\" ]; then\n  sleep 1\n  echo \"still triaging the incident timeline\"\nelse\n  echo \"watching the rollout dashboard\"\nfi",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string(), "nova".to_string()],
        )
        .expect("create_room_impl should succeed");

        let settings = GroupChatSettings {
            max_rounds: 2,
            ..GroupChatSettings::default()
        };
        let outcome = run_group_rounds_with_settings(
            &room.id,
            "operator kickoff",
            settings,
            ironhermes_core::TurnRegistry::new(),
        )
            .await
            .expect("run_group_rounds_with_settings must succeed");

        assert_eq!(outcome.rounds_run, 2);

        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");

        // Fixture self-check: round 1 really was a multi-reply round.
        let round1_member_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| m.round == 1 && matches!(m.from, GroupRoomSpeaker::Member(_)))
            .collect();
        assert_eq!(
            round1_member_rows.len(),
            3,
            "fixture self-check: round 1 must be a genuine multi-reply round"
        );

        // Fixture self-check: the mention was actually emitted.
        let mentioning_rows = round1_member_rows
            .iter()
            .filter(|m| m.text.contains("@nova"))
            .count();
        assert_eq!(
            mentioning_rows, 1,
            "fixture self-check: exactly one round-1 member must mention @nova"
        );

        let round2_member_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| m.round == 2 && matches!(m.from, GroupRoomSpeaker::Member(_)))
            .collect();
        assert_eq!(
            round2_member_rows.len(),
            1,
            "CR-02: round 2's responder set must be resolved from the whole \
             previous round's messages, not from whichever subprocess \
             happened to exit last"
        );
        match &round2_member_rows[0].from {
            GroupRoomSpeaker::Member(name) => assert_eq!(
                name.as_str(),
                "nova",
                "CR-02: the @nova mention from round 1 must reach nova in \
                 round 2 regardless of completion order"
            ),
            other => panic!("expected a Member row for round 2, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 18 (G-50.2-2c): round-boundary steering injection
    // -------------------------------------------------------------------

    // `env_lock()`'s std::sync::MutexGuard is held across this test's
    // `.await` points deliberately — same precedent as the driver tests
    // above; this module's own `<verify>` command runs `--test-threads=1`.
    //
    // Mutation bar (D-24): a message queued for a room BEFORE its drive
    // even starts is drained at round 1's OWN top-of-loop, before round 1's
    // own responders are resolved (`run_group_rounds_with_settings`'s doc
    // comment on the injection step). Its `@zig` mention therefore narrows
    // round 1 itself down to zig alone — the round the message lands in is
    // round 1, so that is the round this test asserts on. Force-emptying
    // the round-boundary drain (the D-24 mutation) makes round 1 dispatch
    // to BOTH scout and zig again, turning this test RED.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_message_queued_before_the_drive_is_injected_and_addresses_its_target() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        // Both bots reply with a distinct, non-pass, mention-free message —
        // WHICH of them actually runs round 1 is decided entirely by the
        // pre-queued steering message's own @zig mention, not by anything
        // either stub says.
        let stub = write_stub_script(
            dir.path(),
            "queued-before-drive-stub.sh",
            "if [ \"$2\" = \"scout\" ]; then\n  echo \"scout must never run this round\"\nelse\n  echo \"zig is triaging the deploy log\"\nfi",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Steering Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        let steering_text = "@zig focus on the deploy log";
        crate::server::handoff_steering::enqueue_room_steering(&room.id, steering_text)
            .expect("enqueue_room_steering should succeed before the drive starts");
        assert_eq!(
            crate::server::handoff_steering::room_steering_depth(&room.id),
            1,
            "fixture self-check: the steering message must be queued before the drive starts"
        );

        let settings = GroupChatSettings {
            max_rounds: 2,
            ..GroupChatSettings::default()
        };
        let outcome = run_group_rounds_with_settings(
            &room.id,
            "operator kickoff",
            settings,
            ironhermes_core::TurnRegistry::new(),
        )
        .await
        .expect("run_group_rounds_with_settings must succeed");
        // A single-responder round 1 falls back to "everyone except that
        // responder" for round 2 (unchanged, pre-existing semantics) — this
        // test's own claim is about round 1's addressing, asserted below.
        assert_eq!(outcome.rounds_run, 2);

        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");

        // The queued message must land as an operator row at round 1,
        // injected before round 1's own responders were resolved.
        let round1_operator_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| m.round == 1 && matches!(m.from, GroupRoomSpeaker::Operator))
            .collect();
        assert!(
            round1_operator_rows.iter().any(|m| m.text == steering_text),
            "the queued steering message must appear as an operator row at round 1"
        );

        // Round 1's own responder set is narrowed by that injected @zig
        // mention — zig alone, addressed through the room's existing
        // mention resolution, no second addressing mechanism.
        let round1_member_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| m.round == 1 && matches!(m.from, GroupRoomSpeaker::Member(_)))
            .collect();
        assert_eq!(
            round1_member_rows.len(),
            1,
            "a message queued before the drive starts is injected at round one, before \
             that round's responders are resolved, so its @mention must narrow round 1 itself"
        );
        match &round1_member_rows[0].from {
            GroupRoomSpeaker::Member(name) => assert_eq!(
                name.as_str(),
                "zig",
                "the @zig mention in the pre-queued steering message must address zig \
                 through the room's existing mention resolution"
            ),
            other => panic!("expected a Member row for round 1, got {other:?}"),
        }

        assert_eq!(
            crate::server::handoff_steering::room_steering_depth(&room.id),
            0,
            "the room queue must be drained after injection"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn group_round_message_queued_mid_drive_is_injected_at_the_next_round_boundary() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        // Both bots sleep ~2s before replying — generous by an order of
        // magnitude relative to the enqueue below, which lands a few
        // hundred ms after the drive starts, well inside round 1's
        // dispatch window.
        let stub = write_stub_script(
            dir.path(),
            "queued-mid-drive-stub.sh",
            "sleep 2\nif [ \"$2\" = \"scout\" ]; then\n  echo \"scout round one reply\"\nelse\n  echo \"zig round one reply\"\nfi",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Mid Drive Steering Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        let settings = GroupChatSettings {
            max_rounds: 2,
            ..GroupChatSettings::default()
        };
        let room_id_for_task = room.id.clone();
        let drive = tokio::spawn(async move {
            run_group_rounds_with_settings(
                &room_id_for_task,
                "operator kickoff",
                settings,
                ironhermes_core::TurnRegistry::new(),
            )
            .await
        });

        // The stub's 2s sleep is generous by an order of magnitude
        // relative to this enqueue — it lands well inside round 1's
        // dispatch window, which is what makes the enqueue land inside
        // round 1 rather than racing the round boundary.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let steering_text = "@zig keep watching the rollout";
        crate::server::handoff_steering::enqueue_room_steering(&room.id, steering_text)
            .expect("enqueue_room_steering should succeed while round 1 is in flight");

        let outcome = drive
            .await
            .expect("drive task must not panic")
            .expect("run_group_rounds_with_settings must succeed");
        assert_eq!(outcome.rounds_run, 2);

        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");

        let round2_operator_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| m.round == 2 && matches!(m.from, GroupRoomSpeaker::Operator))
            .collect();
        assert!(
            round2_operator_rows.iter().any(|m| m.text == steering_text),
            "a message queued while round 1 is dispatching must be injected at the round-2 boundary"
        );

        let round2_member_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| m.round == 2 && matches!(m.from, GroupRoomSpeaker::Member(_)))
            .collect();
        assert_eq!(
            round2_member_rows.len(),
            1,
            "round 2 must be narrowed to exactly the @mention named in the mid-drive steering message"
        );
        match &round2_member_rows[0].from {
            GroupRoomSpeaker::Member(name) => assert_eq!(name.as_str(), "zig"),
            other => panic!("expected a Member row for round 2, got {other:?}"),
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_empty_steering_queue_leaves_the_drive_unchanged() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(dir.path(), "no-steering-stub.sh", "echo \"(pass)\"");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "No Steering Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        assert_eq!(
            crate::server::handoff_steering::room_steering_depth(&room.id),
            0,
            "fixture self-check: this room's steering queue must be empty"
        );

        let outcome = run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        // Byte-for-byte the pre-existing "full round of passes settles
        // after one round" behavior (see
        // `group_round_full_round_of_passes_settles_after_one_round` above)
        // — an empty steering queue must not perturb it.
        assert_eq!(outcome.rounds_run, 1);
        assert!(outcome.settled);
        assert!(outcome.failures.is_empty());

        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");
        let operator_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| matches!(m.from, GroupRoomSpeaker::Operator))
            .collect();
        assert_eq!(
            operator_rows.len(),
            1,
            "an empty steering queue must produce no operator row beyond the kickoff"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dispatch_group_round_while_a_drive_is_running_returns_queued() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        // Config::load() reads <IRONHERMES_HOME>/config.yaml fresh from
        // disk (never a test-injected Config value) — write the
        // write-gate-open record this #[server] fn's own
        // check_profile_write_gate call requires, following
        // mention_handoff_api.rs's own precedent for exercising a gated
        // #[server] fn end to end.
        std::fs::write(
            dir.path().join("config.yaml"),
            "security:\n  web_config_write_enabled: true\n",
        )
        .expect("write config.yaml");
        let stub = write_stub_script(
            dir.path(),
            "queued-endpoint-stub.sh",
            "sleep 2\necho \"slow reply\"",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Endpoint Steering Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        let room_id_for_task = room.id.clone();
        let first = tokio::spawn(async move {
            dispatch_group_round(crate::protocol::DispatchGroupRoundRequest {
                room_id: room_id_for_task,
                message: "operator kickoff".to_string(),
            })
            .await
        });

        // Poll the production TurnRegistry — an observable side effect of
        // the drive's own dispatch, not this module's internal steering
        // state — until the round's member turns are genuinely registered.
        // Same pattern `cli_handoff.rs`'s
        // `tracked_handoff_is_listed_while_the_child_runs_and_gone_after`
        // already established for polling a real in-flight signal rather
        // than sleeping a fixed guess.
        let registry = crate::server::cli_handoff::handoff_turn_registry();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if !registry.list_all().await.is_empty() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the round's member turns to register"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let second = dispatch_group_round(crate::protocol::DispatchGroupRoundRequest {
            room_id: room.id.clone(),
            message: "@zig keep going".to_string(),
        })
        .await
        .expect("dispatch_group_round should not error while a drive is running");

        match second {
            crate::protocol::GroupRoundDispatch::Queued { depth } => assert_eq!(depth, 1),
            crate::protocol::GroupRoundDispatch::Ran(_) => {
                panic!("a second dispatch while a drive is running must be queued, not run")
            }
        }

        let first_result = first
            .await
            .expect("first dispatch task must not panic")
            .expect("first dispatch_group_round must succeed");
        match first_result {
            crate::protocol::GroupRoundDispatch::Ran(outcome) => {
                // "slow reply" never reads as a pass, so the drive never
                // settles early and runs to the persisted default
                // `max_rounds` (3) — what matters here is that round 2 ran
                // at all, carrying the queued steering message (asserted
                // via the transcript below), not the exact round count.
                assert!(
                    outcome.rounds_run >= 2,
                    "round 2 must run, carrying the queued steering message"
                );
            }
            crate::protocol::GroupRoundDispatch::Queued { .. } => {
                panic!("the first dispatch on an idle room must Begin, not Queued")
            }
        }

        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");
        let kickoff_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| matches!(m.from, GroupRoomSpeaker::Operator) && m.text == "operator kickoff")
            .collect();
        assert_eq!(
            kickoff_rows.len(),
            1,
            "the transcript must not gain a second kickoff row from the queued dispatch"
        );
        let round2_operator_rows: Vec<_> = transcript
            .messages
            .iter()
            .filter(|m| m.round == 2 && matches!(m.from, GroupRoomSpeaker::Operator))
            .collect();
        assert!(
            round2_operator_rows
                .iter()
                .any(|m| m.text == "@zig keep going"),
            "the queued message must land as an operator row at round 2"
        );
    }

    // -------------------------------------------------------------------
    // run_group_rounds_detached (Phase 50.2 Plan 19, CR-01)
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropping_the_group_round_future_keeps_the_room_marked_in_flight() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        std::fs::write(
            dir.path().join("config.yaml"),
            "security:\n  web_config_write_enabled: true\n",
        )
        .expect("write config.yaml");
        let stub = write_stub_script(
            dir.path(),
            "group-round-detached-drop-stub.sh",
            "sleep 5\necho 'should not be observed by the dropped future'",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Detached Drop Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        let guard = match crate::server::handoff_steering::begin_room_drive_or_queue(
            &room.id,
            "operator kickoff",
        )
        .expect("begin_room_drive_or_queue should not error on an idle room")
        {
            crate::server::handoff_steering::BeginRoomDriveOrQueue::Begin(guard) => guard,
            crate::server::handoff_steering::BeginRoomDriveOrQueue::Queued { .. } => {
                panic!("an idle room's first drive must Begin, not Queued")
            }
        };

        // Simulated client disconnect: this caller's own future (standing
        // in for the HTTP request future axum/hyper would drop) is wrapped
        // in a short timeout and DROPPED when it elapses, well before the
        // member turns' 5s sleep finishes.
        let dropped = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            run_group_rounds_detached(guard, room.id.clone(), "operator kickoff".to_string()),
        )
        .await;
        assert!(
            dropped.is_err(),
            "the timeout must elapse — the round's member turns must still be running"
        );

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        match crate::server::handoff_steering::begin_room_drive_or_queue(
            &room.id,
            "second send",
        ) {
            Ok(crate::server::handoff_steering::BeginRoomDriveOrQueue::Queued { .. }) => {}
            other => panic!(
                "the room must still read as in-flight after the dropped future — got {other:?}"
            ),
        }

        let registry = crate::server::cli_handoff::handoff_turn_registry();
        let observed = registry.list_all().await;
        assert!(
            !observed.is_empty(),
            "the round's member turns must still be tracked in the registry after the dropped future"
        );

        for summary in &observed {
            registry.cancel_one(summary.turn_id).await;
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            match crate::server::handoff_steering::begin_room_drive_or_queue(
                &room.id,
                "poll send",
            ) {
                Ok(crate::server::handoff_steering::BeginRoomDriveOrQueue::Begin(guard)) => {
                    drop(guard);
                    break;
                }
                _ => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the room must reopen within the bounded poll window once its member turns are cancelled"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_drive_wide_message_cap_stops_the_whole_drive() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(dir.path(), "always-reply-stub.sh", "echo \"keep talking\"");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        // 2 members x 3 default rounds, uncapped, would post 6 member
        // messages. max_messages: 2 must stop the drive after round 1 (2
        // posted == cap) — proving the cap is drive-wide, not per round.
        let settings = GroupChatSettings {
            max_messages: 2,
            ..GroupChatSettings::default()
        };

        let outcome = run_group_rounds_with_settings(
            &room.id,
            "operator kickoff",
            settings,
            ironhermes_core::TurnRegistry::new(),
        )
            .await
            .expect("run_group_rounds_with_settings must succeed");

        assert_eq!(
            outcome.rounds_run, 1,
            "the drive-wide cap must stop further rounds, not just further messages"
        );
        assert_eq!(outcome.messages_appended, 2);
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_needs_you_true_and_persisted_in_a_round_that_did_not_settle() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig", "ada"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        // 3 members. Round 1 always includes every member (the round's
        // starting message is the operator's single-element
        // `prev_round_messages`, never a member's). Phase 50.2 Plan 13
        // (CR-02 fix): later rounds now ALSO include every member, because
        // "scout" and "zig" never mention anyone by name, so no mention
        // narrows `resolve_round_responders`'s multi-message union — no
        // member is arbitrarily excluded by join-completion order anymore.
        // "scout" and "zig" both always talk (non-pass) every round they
        // participate in, so spoke > 0 for the whole drive regardless, and
        // "scout" addressing the operator guarantees needs_you is raised
        // in round 1.
        let stub = write_stub_script(
            dir.path(),
            "needs-you-stub.sh",
            "if [ \"$2\" = \"ada\" ]; then echo \"(pass)\"; else echo \"hey @user check this\"; fi",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string(), "ada".to_string()],
        )
        .expect("create_room_impl should succeed");

        let outcome = run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        // scout and zig both keep talking (non-pass) every round they
        // participate in, so spoke > 0 and the drive never settles — it
        // runs the full default max_rounds.
        assert!(!outcome.settled);
        assert_eq!(outcome.rounds_run, GroupChatSettings::default().max_rounds);
        assert!(
            outcome.needs_you,
            "needs_you must be raised even though the drive never settled"
        );

        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");
        assert!(
            transcript.room.needs_you,
            "the persisted room record must carry the escalation flag"
        );
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 06 (D-21 integration + D-03): the settings round trip
    // and the operator-clears/member-reraises needs_you sequence.
    // -------------------------------------------------------------------

    /// `group_chat_settings_for_drive` is a direct (non-`#[tokio::test]`)
    /// unit test — a missing settings file must still yield D-21's shipped
    /// defaults, exactly as it did before this plan's integration flip.
    #[test]
    fn group_chat_settings_for_drive_missing_file_returns_d21_defaults() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        let settings = group_chat_settings_for_drive();
        assert_eq!(settings, GroupChatSettings::default());
    }

    /// Proves the round driver actually READS the persisted record through
    /// the production zero-arg `run_group_rounds` entry point — never a
    /// settings param the test itself supplies. Persists `max_rounds: 1`
    /// (differs from the default 3) against a room whose members keep
    /// talking every round (would otherwise run the default 3 rounds); this
    /// test fails if the settings read is reverted to `Default::default()`.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_persisted_max_rounds_one_stops_the_drive_after_one_round() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig", "ada"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(dir.path(), "always-reply-stub.sh", "echo \"keep talking\"");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let settings = GroupChatSettings {
            max_rounds: 1,
            ..GroupChatSettings::default()
        };
        crate::server::group_settings_api::save_group_settings_impl(&settings)
            .expect("save_group_settings_impl should succeed");

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string(), "ada".to_string()],
        )
        .expect("create_room_impl should succeed");

        let outcome = run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        assert_eq!(
            outcome.rounds_run, 1,
            "a persisted max_rounds of 1 must stop the drive after round 1, even though \
             every member keeps talking and would otherwise run the default 3 rounds"
        );
    }

    /// Same proof for `max_messages` — persists `max_messages: 2` (differs
    /// from the default 10) against a two-member room whose members never
    /// pass; the drive-wide cap must stop the whole drive after round 1
    /// (2 members x 1 round == the cap), not just further messages within a
    /// round.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_persisted_max_messages_two_caps_the_whole_drive() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(dir.path(), "always-reply-stub.sh", "echo \"keep talking\"");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let settings = GroupChatSettings {
            max_messages: 2,
            ..GroupChatSettings::default()
        };
        crate::server::group_settings_api::save_group_settings_impl(&settings)
            .expect("save_group_settings_impl should succeed");

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        let outcome = run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        assert_eq!(
            outcome.rounds_run, 1,
            "the drive-wide cap must stop further rounds, not just further messages"
        );
        assert_eq!(outcome.messages_appended, 2);
    }

    /// A pre-existing `needs_you == true` must be cleared the moment the
    /// operator's message lands, BEFORE any member turn is dispatched.
    /// Every member passes in this drive (no `@user` anywhere), so the
    /// ONLY thing that can clear the pre-existing flag is the operator
    /// message's own clear-on-send call — never a member reply.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_operator_message_clears_a_preexisting_needs_you_flag() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(dir.path(), "all-pass-stub.sh", "echo \"(pass)\"");
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");
        crate::server::group_chat_store::set_room_needs_you_impl(&room.id, true)
            .expect("set_room_needs_you_impl should succeed");

        run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");
        assert!(
            !transcript.room.needs_you,
            "an operator message must clear a pre-existing needs_you flag before any \
             member turn is dispatched"
        );
    }

    /// A member reply addressing `@user` re-raises needs_you WITHIN the
    /// same drive that just cleared it on the operator's own message —
    /// proving clear-then-reraise coexist rather than the clear
    /// permanently suppressing the escalation channel.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_member_at_user_reply_reraises_needs_you_after_operator_clear() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let stub = write_stub_script(
            dir.path(),
            "always-at-user-stub.sh",
            "echo \"hey @user check this\"",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");
        crate::server::group_chat_store::set_room_needs_you_impl(&room.id, true)
            .expect("set_room_needs_you_impl should succeed");

        let outcome = run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        assert!(
            outcome.needs_you,
            "a member reply addressing @user must re-raise needs_you within the same drive \
             that already cleared it on the operator's own message"
        );
        let transcript = crate::server::group_chat_store::load_room_impl(&room.id)
            .expect("load_room_impl should succeed");
        assert!(
            transcript.room.needs_you,
            "the re-raised flag must be persisted on the room record"
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_all_members_in_one_round_receive_identical_delta() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let capture_dir = dir.path().to_path_buf();
        // argv layout is `--profile <name> chat -q <message> --session
        // <title>` (build_bot_handoff_argv) — $5 is the message positional.
        let stub = write_stub_script(
            dir.path(),
            "delta-capture-stub.sh",
            &format!(
                "echo \"$5\" > \"{}/delta-$2.txt\"\necho \"(pass)\"",
                capture_dir.display()
            ),
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        let mut deltas = Vec::new();
        for name in ["scout", "zig"] {
            let captured = std::fs::read_to_string(capture_dir.join(format!("delta-{name}.txt")))
                .unwrap_or_else(|e| panic!("read captured message for {name}: {e}"));
            // Isolate the delta-lines section — between the member-specific
            // header's own trailing blank line and the trailing reply
            // instruction, which is the only part every responder in a
            // round must share byte-for-byte.
            let after_header = captured
                .split_once("\n\n")
                .map(|(_, rest)| rest)
                .unwrap_or(captured.as_str());
            let delta_only = after_header
                .split("\nReply to the room")
                .next()
                .unwrap_or(after_header)
                .to_string();
            deltas.push(delta_only);
        }
        assert_eq!(
            deltas[0], deltas[1],
            "every member in one round must receive an identical frozen delta"
        );
    }

    /// Phase 50.2 Plan 02 (D-21): proves the session title actually reaches
    /// the spawned child's argv over the real `run_group_rounds` fan-out —
    /// not just at the pure `build_bot_handoff_argv` unit-test level. The
    /// stub script records its own argv to a per-bot file; this test reads
    /// that file back and asserts `--session "Group: <room>"` landed on
    /// every member's dispatch.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn group_round_threads_group_session_title_into_child_argv() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        let argv_log_dir = dir.path().to_path_buf();
        let stub = write_stub_script(
            dir.path(),
            "argv-capture-stub.sh",
            &format!(
                "echo \"$@\" > \"{}/argv-$2.txt\"\necho \"(pass)\"",
                argv_log_dir.display()
            ),
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        run_group_rounds(&room.id, "operator kickoff")
            .await
            .expect("run_group_rounds must succeed");

        for name in ["scout", "zig"] {
            let captured =
                std::fs::read_to_string(argv_log_dir.join(format!("argv-{name}.txt")))
                    .unwrap_or_else(|e| panic!("read captured argv for {name}: {e}"));
            assert!(
                captured.contains("--session"),
                "{name}'s captured argv must contain --session: {captured}"
            );
            assert!(
                captured.contains("Group: Ops Room"),
                "{name}'s captured argv must carry the room's Group: session title: {captured}"
            );
        }
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 04 (D-21, D-03): the orchestration truth table,
    // transcribed as failing assertions (RED). Every test name is prefixed
    // `group_round_` so it stays reachable under this module's own
    // `<verify>` filter, and named after the RULE it protects rather than
    // the function it calls.
    // -------------------------------------------------------------------

    /// Test-only `GroupRoomMessage` builder shared by the trim/delta/score
    /// test groups below.
    fn group_round_test_msg_with_status(text: &str, status: MemberTurnStatus) -> GroupRoomMessage {
        GroupRoomMessage {
            from: GroupRoomSpeaker::Member("m".to_string()),
            text: text.to_string(),
            at_ms: 0,
            round: 1,
            status,
        }
    }

    fn group_round_test_msg(text: &str) -> GroupRoomMessage {
        group_round_test_msg_with_status(text, MemberTurnStatus::Replied)
    }

    // --- parse_group_mentions (plugin.js:3342) ---------------------------

    #[test]
    fn group_round_mention_parse_single_lowercased() {
        let set = parse_group_mentions("@zig look at this");
        assert_eq!(set.handles, vec!["zig".to_string()]);
        assert!(!set.everyone);
    }

    #[test]
    fn group_round_mention_parse_uppercase_input_lowers_handle() {
        let set = parse_group_mentions("@ZIG");
        assert_eq!(set.handles, vec!["zig".to_string()]);
    }

    #[test]
    fn group_round_mention_parse_everyone_keyword_sets_everyone_true() {
        let set = parse_group_mentions("@everyone ship it");
        assert!(set.everyone);
        assert!(set.handles.is_empty());
    }

    #[test]
    fn group_round_mention_parse_all_keyword_sets_everyone_true() {
        let set = parse_group_mentions("@all ship it");
        assert!(set.everyone);
        assert!(set.handles.is_empty());
    }

    #[test]
    fn group_round_mention_parse_user_handle_is_skipped() {
        let set = parse_group_mentions("@user please confirm");
        assert!(set.handles.is_empty());
        assert!(!set.everyone);
    }

    #[test]
    fn group_round_mention_parse_no_mentions_returns_empty() {
        let set = parse_group_mentions("no mentions here");
        assert!(set.handles.is_empty());
        assert!(!set.everyone);
    }

    #[test]
    fn group_round_mention_parse_two_mentions_preserve_first_seen_order() {
        let set = parse_group_mentions("@zig and @ada");
        assert_eq!(set.handles, vec!["zig".to_string(), "ada".to_string()]);
    }

    #[test]
    fn group_round_mention_parse_email_matches_verbatim_regex_behavior() {
        // Pinned, not "fixed" — the verbatim-ported regex matches `b.com`
        // inside an email address. Non-member matches are filtered later
        // by resolve_group_responders, not here.
        let set = parse_group_mentions("email me at a@b.com");
        assert_eq!(set.handles, vec!["b.com".to_string()]);
    }

    // --- is_group_pass_text (plugin.js:3301-3309) ------------------------

    #[test]
    fn group_round_pass_text_empty_string_is_pass() {
        assert!(is_group_pass_text(""));
    }

    #[test]
    fn group_round_pass_text_whitespace_only_is_pass() {
        assert!(is_group_pass_text("   "));
    }

    #[test]
    fn group_round_pass_text_bare_paren_pass_is_pass() {
        assert!(is_group_pass_text("(pass)"));
    }

    #[test]
    fn group_round_pass_text_bare_pass_is_pass() {
        assert!(is_group_pass_text("pass"));
    }

    #[test]
    fn group_round_pass_text_uppercase_pass_is_pass() {
        assert!(is_group_pass_text("PASS"));
    }

    #[test]
    fn group_round_pass_text_spaced_paren_pass_is_pass() {
        assert!(is_group_pass_text("( pass )"));
    }

    #[test]
    fn group_round_pass_text_trailing_period_is_pass() {
        assert!(is_group_pass_text("pass."));
    }

    #[test]
    fn group_round_pass_text_paren_trailing_period_is_pass() {
        assert!(is_group_pass_text("(pass)."));
    }

    #[test]
    fn group_round_pass_text_substring_is_not_pass() {
        assert!(!is_group_pass_text("I'll pass on this one"));
    }

    #[test]
    fn group_round_pass_text_passing_word_is_not_pass() {
        assert!(!is_group_pass_text("passing the ball"));
    }

    // --- resolve_group_responders (plugin.js:3342 + resolveGroupResponders) ---

    #[test]
    fn group_round_responders_no_mentions_everyone_but_speaker_responds() {
        let members = vec!["scout".to_string(), "zig".to_string(), "ada".to_string()];
        let mentions = GroupMentionSet::default();
        let responders = resolve_group_responders(&members, &mentions, "scout");
        assert_eq!(responders, vec!["zig".to_string(), "ada".to_string()]);
    }

    #[test]
    fn group_round_responders_everyone_keyword_everyone_but_speaker_responds() {
        let members = vec!["scout".to_string(), "zig".to_string()];
        let mentions = GroupMentionSet {
            handles: vec![],
            everyone: true,
        };
        let responders = resolve_group_responders(&members, &mentions, "scout");
        assert_eq!(responders, vec!["zig".to_string()]);
    }

    #[test]
    fn group_round_responders_two_named_members_in_member_order() {
        let members = vec!["scout".to_string(), "zig".to_string(), "ada".to_string()];
        let mentions = GroupMentionSet {
            handles: vec!["ada".to_string(), "zig".to_string()],
            everyone: false,
        };
        let responders = resolve_group_responders(&members, &mentions, "scout");
        assert_eq!(responders, vec!["zig".to_string(), "ada".to_string()]);
    }

    #[test]
    fn group_round_responders_non_member_mention_falls_back_to_everyone() {
        let members = vec!["scout".to_string(), "zig".to_string()];
        let mentions = GroupMentionSet {
            handles: vec!["outsider".to_string()],
            everyone: false,
        };
        let responders = resolve_group_responders(&members, &mentions, "scout");
        assert_eq!(responders, vec!["zig".to_string()]);
    }

    #[test]
    fn group_round_responders_speaker_never_responds_to_self() {
        let members = vec!["scout".to_string(), "zig".to_string()];
        let mentions = GroupMentionSet {
            handles: vec!["scout".to_string(), "zig".to_string()],
            everyone: false,
        };
        let responders = resolve_group_responders(&members, &mentions, "scout");
        assert_eq!(responders, vec!["zig".to_string()]);
    }

    // --- resolve_round_responders / union_group_mentions (Phase 50.2 Plan
    // 13, CR-02 fix) --------------------------------------------------

    /// Test-only `GroupRoomMessage` builder for the `resolve_round_responders`
    /// / `union_group_mentions` test group.
    fn member_row(name: &str, text: &str) -> GroupRoomMessage {
        GroupRoomMessage {
            from: GroupRoomSpeaker::Member(name.to_string()),
            text: text.to_string(),
            at_ms: 0,
            round: 1,
            status: MemberTurnStatus::Replied,
        }
    }

    #[test]
    fn group_round_responders_multi_reply_round_is_order_independent() {
        let members = vec!["scout".to_string(), "zig".to_string(), "nova".to_string()];
        let rows = vec![
            member_row("scout", "hey @nova please check the deploy log"),
            member_row("zig", "no mentions in this one"),
        ];
        let forward = resolve_round_responders(&members, &rows);

        let mut reversed = rows.clone();
        reversed.reverse();
        let backward = resolve_round_responders(&members, &reversed);

        assert_eq!(
            forward, backward,
            "resolution must be invariant under permutation of the round's messages"
        );
        assert_eq!(forward, vec!["nova".to_string()]);
    }

    #[test]
    fn group_round_responders_multi_reply_round_does_not_self_exclude_an_arbitrary_member() {
        let members = vec!["scout".to_string(), "zig".to_string(), "nova".to_string()];
        let rows = vec![
            member_row("scout", "just chatting, no mentions"),
            member_row("zig", "also just chatting"),
        ];
        let responders = resolve_round_responders(&members, &rows);
        assert_eq!(
            responders, members,
            "a multi-participant round with no mentions must exclude no one"
        );
    }

    #[test]
    fn group_round_responders_single_message_round_keeps_speaker_exclusion() {
        let members = vec!["scout".to_string(), "zig".to_string(), "nova".to_string()];
        let rows = vec![member_row("scout", "no mention here")];
        let responders = resolve_round_responders(&members, &rows);
        assert_eq!(responders, vec!["zig".to_string(), "nova".to_string()]);
    }

    #[test]
    fn group_round_responders_empty_previous_round_falls_back_to_every_member() {
        let members = vec!["scout".to_string(), "zig".to_string(), "nova".to_string()];
        let rows: Vec<GroupRoomMessage> = vec![];
        let responders = resolve_round_responders(&members, &rows);
        assert_eq!(responders, members);
    }

    #[test]
    fn group_round_union_mentions_folds_every_message_in_the_round() {
        let rows = vec![
            member_row("a", "hey @zig can you take this"),
            member_row("b", "@everyone please check in"),
            member_row("c", "just chatting, no mentions"),
        ];
        let mentions = union_group_mentions(&rows);
        assert!(
            mentions.handles.contains(&"zig".to_string()),
            "must collect the named handle from one row"
        );
        assert!(
            mentions.everyone,
            "must set everyone when ANY row in the round says @everyone"
        );
    }

    // --- message_addresses_user (plugin.js:3582-3593) --------------------

    #[test]
    fn group_round_needs_you_at_user_lowercase_true() {
        assert!(message_addresses_user("@user please decide"));
    }

    #[test]
    fn group_round_needs_you_at_user_uppercase_true() {
        assert!(message_addresses_user("@USER"));
    }

    #[test]
    fn group_round_needs_you_cc_prefix_true() {
        assert!(message_addresses_user("cc @user, thanks"));
    }

    #[test]
    fn group_round_needs_you_username_word_boundary_false() {
        assert!(!message_addresses_user("@username is not the operator"));
    }

    #[test]
    fn group_round_needs_you_no_escalation_false() {
        assert!(!message_addresses_user("no escalation here"));
    }

    // --- trim_room_history -------------------------------------------------

    #[test]
    fn group_round_trim_history_returns_at_most_limit_most_recent() {
        let messages: Vec<_> = (0..5).map(|i| group_round_test_msg(&i.to_string())).collect();
        let trimmed = trim_room_history(&messages, 2);
        assert_eq!(trimmed.len(), 2);
        assert_eq!(trimmed[0].text, "3");
        assert_eq!(trimmed[1].text, "4");
    }

    #[test]
    fn group_round_trim_history_shorter_log_returned_whole() {
        let messages: Vec<_> = (0..2).map(|i| group_round_test_msg(&i.to_string())).collect();
        let trimmed = trim_room_history(&messages, 10);
        assert_eq!(trimmed.len(), 2);
    }

    #[test]
    fn group_round_trim_history_limit_zero_returns_empty() {
        let messages: Vec<_> = (0..3).map(|i| group_round_test_msg(&i.to_string())).collect();
        let trimmed = trim_room_history(&messages, 0);
        assert!(trimmed.is_empty());
    }

    #[test]
    fn group_round_trim_history_preserves_order() {
        let messages: Vec<_> = (0..4).map(|i| group_round_test_msg(&i.to_string())).collect();
        let trimmed = trim_room_history(&messages, 3);
        assert_eq!(
            trimmed.iter().map(|m| m.text.clone()).collect::<Vec<_>>(),
            vec!["1".to_string(), "2".to_string(), "3".to_string()]
        );
    }

    // --- member_delta (upstream's per-member watermarks map) --------------

    #[test]
    fn group_round_member_delta_returns_from_watermark() {
        let messages: Vec<_> = (0..5).map(|i| group_round_test_msg(&i.to_string())).collect();
        let delta = member_delta(&messages, 3);
        assert_eq!(
            delta.iter().map(|m| m.text.clone()).collect::<Vec<_>>(),
            vec!["3".to_string(), "4".to_string()]
        );
    }

    #[test]
    fn group_round_member_delta_watermark_past_end_returns_empty() {
        let messages: Vec<_> = (0..3).map(|i| group_round_test_msg(&i.to_string())).collect();
        let delta = member_delta(&messages, 10);
        assert!(delta.is_empty());
    }

    #[test]
    fn group_round_member_delta_watermark_zero_returns_everything() {
        let messages: Vec<_> = (0..3).map(|i| group_round_test_msg(&i.to_string())).collect();
        let delta = member_delta(&messages, 0);
        assert_eq!(delta.len(), 3);
    }

    // --- score_round (plugin.js:3896-3898 settle + 3582-3593 needs-you) ---

    #[test]
    fn group_round_score_spoke_counts_non_pass_non_failure() {
        let messages = vec![
            group_round_test_msg_with_status("hello", MemberTurnStatus::Replied),
            group_round_test_msg_with_status("(pass)", MemberTurnStatus::Passed),
            group_round_test_msg_with_status(
                "",
                MemberTurnStatus::Failed {
                    reason: "boom".to_string(),
                },
            ),
        ];
        let score = score_round(&messages);
        assert_eq!(score.spoke, 1);
    }

    #[test]
    fn group_round_score_settled_true_when_spoke_zero() {
        let messages = vec![
            group_round_test_msg_with_status("(pass)", MemberTurnStatus::Passed),
            group_round_test_msg_with_status(
                "",
                MemberTurnStatus::Failed {
                    reason: "boom".to_string(),
                },
            ),
        ];
        let score = score_round(&messages);
        assert_eq!(score.spoke, 0);
        assert!(score.settled);
    }

    #[test]
    fn group_round_score_needs_you_true_on_non_pass_addressing_user() {
        let messages = vec![group_round_test_msg_with_status(
            "hey @user check this",
            MemberTurnStatus::Replied,
        )];
        let score = score_round(&messages);
        assert!(score.needs_you);
        assert!(!score.settled);
    }

    #[test]
    fn group_round_score_pass_and_failure_excluded_from_needs_you_and_spoke() {
        let messages = vec![group_round_test_msg_with_status(
            "@user in a passed message",
            MemberTurnStatus::Passed,
        )];
        let score = score_round(&messages);
        assert_eq!(score.spoke, 0);
        assert!(
            !score.needs_you,
            "a Passed status message must not trigger needs_you even if its text mentions @user"
        );
        assert!(score.settled);
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 14 (G-50.2-2b): group-room member turns are listed in
    // the TurnRegistry while dispatching
    // -------------------------------------------------------------------

    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn group_round_member_turns_are_listed_in_the_turn_registry_while_dispatching() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _home = ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        );
        for name in ["scout", "zig"] {
            std::fs::create_dir_all(
                crate::server::profile_api::profile_dir_for(name).join("workspace"),
            )
            .expect("mkdir workspace");
        }
        // `$2` is the profile name. Both members sleep ~2s then echo a
        // distinct, non-pass reply, giving the poller a window to observe
        // both entries in the registry before either child exits.
        let stub = write_stub_script(
            dir.path(),
            "group-turn-registry-stub.sh",
            "sleep 2\nif [ \"$2\" = \"scout\" ]; then echo \"scout checking in\"; else echo \"zig checking in\"; fi",
        );
        let _bin_guard = ScopedEnv::set(
            "IRONHERMES_WORKER_BIN",
            stub.to_str().expect("utf8 stub path"),
        );

        let room = crate::server::group_chat_store::create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");

        let registry = ironhermes_core::TurnRegistry::new();
        let registry_for_poll = registry.clone();
        let settings = GroupChatSettings {
            max_rounds: 1,
            ..GroupChatSettings::default()
        };
        let room_id = room.id.clone();
        let drive = tokio::spawn(async move {
            run_group_rounds_with_settings(&room_id, "operator kickoff", settings, registry).await
        });

        // Poll while the drive runs, collecting every distinct session_id
        // observed — the drive is fast enough (~2s) that a fixed sleep
        // could miss the window entirely, so this polls on a short
        // interval for up to the drive's own generous ceiling.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen_session_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_surfaces: std::collections::HashSet<String> = std::collections::HashSet::new();
        while std::time::Instant::now() < deadline && !drive.is_finished() {
            for summary in registry_for_poll.list_all().await {
                seen_session_ids.insert(summary.session_id);
                seen_surfaces.insert(summary.surface.to_string());
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // One last poll in case the drive finished between the last
        // iteration's check and its own final teardown.
        for summary in registry_for_poll.list_all().await {
            seen_session_ids.insert(summary.session_id);
            seen_surfaces.insert(summary.surface.to_string());
        }

        let outcome = drive
            .await
            .expect("drive task join must succeed")
            .expect("run_group_rounds_with_settings must succeed");
        assert_eq!(outcome.rounds_run, 1);

        let expected_title = group_session_title("Ops Room");
        assert!(
            seen_session_ids.contains(&crate::server::cli_handoff::handoff_turn_session_id(
                "scout",
                Some(&expected_title)
            )),
            "G-50.2-2b: scout's member turn must be listed in the TurnRegistry while dispatching, observed={seen_session_ids:?}"
        );
        assert!(
            seen_session_ids.contains(&crate::server::cli_handoff::handoff_turn_session_id(
                "zig",
                Some(&expected_title)
            )),
            "G-50.2-2b: zig's member turn must be listed in the TurnRegistry while dispatching, observed={seen_session_ids:?}"
        );
        assert_eq!(
            seen_surfaces,
            std::collections::HashSet::from([ironhermes_core::Surface::Cli.to_string()]),
            "G-50.2-2b: every observed group-room member turn summary must be Surface::Cli"
        );
        assert!(
            registry_for_poll.list_all().await.is_empty(),
            "G-50.2-2b: the registry must be empty once the drive resolves"
        );
    }
}
