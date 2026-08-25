//! Phase 50.1 OF-5 (gap task): the floating one-on-one bot chat window.
//!
//! Replaces plan 03's bare inline `BotHandoffComposer` (deleted by this gap
//! task) as the non-live bot's `CHAT` destination. Reference screenshots
//! (`operator-reference/hermes-one-on-one-agent-chat.png`,
//! `operator-reference/buzz-chat-inbox-example.png`) are WORKFLOW
//! references only — this renders in Iron Hermes's own design language
//! (System B tokens, `.kn-modal-overlay`/`.kn-modal`/`.kn-drawer-close`
//! shell, same idiom `delete_confirm.rs` already established in this
//! directory), never Hermes/desktop-app styling (same restyle rule as
//! OF-2/OF-3).
//!
//! **Scope guard (OF-5):** thread state here is session-transient client
//! state, owned by the caller (`bot_roster.rs`, mirroring the `drawer_target`
//! idiom — `Signal` state owned at the roster screen level, passed down by
//! props) — never a context provider outside `HermesApp`'s root, never
//! written to disk, never a new store. A PERSISTENT canonical per-bot
//! conversation is Phase 50.2 scope (D-01); this window's thread disappears
//! the moment `BotRoster` unmounts (e.g. navigating away from the Agents
//! screen), which is the intended "session-transient" behavior.
//!
//! Every dispatch reuses [`dispatch_bot_message`] — the SAME `#[server]` fn
//! plan 03's composer called — so the OF-5 reply-cleanup fix
//! (`server/cli_handoff.rs`'s reply-scrubbing helper) applies here for
//! free; this module never re-derives extraction logic.
//!
//! Signal-borrow discipline (clippy.toml): every value read from a signal is
//! extracted BEFORE the `rsx!` block; the write-lock taken for the
//! synchronous "append the operator's own message" step is dropped before
//! `spawn` is called, and the write-lock taken to append the reply/error is
//! acquired only AFTER `dispatch_bot_message(...).await` resolves — no
//! `GenerationalRefMut`/`WriteLock` is ever held across an `.await` point.
//!
//! # Phase 50.2 Plan 16 (G-50.2-2c): accept-and-queue, not a hard lock
//!
//! **Superseded claim:** earlier phases recorded that this window's
//! composer is `readonly`/disabled for the ENTIRE duration of an in-flight
//! turn (up to 180s). That is no longer true. `dispatch_bot_message` now
//! returns `BotDispatchOutcome` — a busy bot ACCEPTS a mid-flight send and
//! QUEUES it for its own next turn (`server::handoff_steering`) rather than
//! racing a second concurrent subprocess. This window's composer stays
//! typable and sendable for the whole duration of a turn; `in_flight_sends`
//! (an outstanding-send counter, not a single `bool`) drives the pending `…`
//! row, and a `queued` signal drives an operator-visible notice naming the
//! bot and how many messages are waiting (`queued_for_next_turn_notice`).
//!
//! # OF-9 (gap task): think-block hiding
//!
//! `dispatch_bot_message`'s reply (via `run_bot_handoff`'s own
//! reply-scrubbing step) keeps any `<think>…</think>` reasoning preamble
//! intact — only the D-13
//! preview text strips it server-side (`cli_handoff.rs`'s
//! `strip_think_blocks` doc comment). This module hides it at RENDER time
//! instead, for `ChatWindowRole::Bot` rows only, via
//! [`ChatWindowEntry::display_text`]. Chosen over a collapsible disclosure
//! widget: no cheap, reusable disclosure/`<details>` idiom already exists in
//! this crate (would mean introducing a new one for a single row type), so
//! full removal is the simplest implementation that satisfies the operator
//! directive ("yes strip the think blocks"). A reply that is ENTIRELY a
//! think block still renders a labeled placeholder row rather than a blank
//! one — the dispatch succeeded and something WAS delivered (module doc,
//! `cli_handoff.rs`'s OF-9 section: "a think-only reply is still a
//! delivered reply for the chat window").
//!
//! [`strip_think_blocks_for_display`] is a deliberate, isomorphic duplicate
//! of `cli_handoff.rs`'s server-gated `strip_think_blocks` (same "each
//! `#[cfg(test)]`/feature-boundary module is its own namespace" duplication
//! precedent this crate already uses for its `ScopedEnv` test guards) — this
//! module has no `server` feature gate (it must also compile for the wasm
//! client build, which never enables `server`), so it cannot call the
//! server-only original.
//!
//! Phase 50.2 Plan 11 (G-3): `strip_think_blocks_for_display` and
//! `BOT_THINK_ONLY_PLACEHOLDER` are `pub(super)` so the room transcript
//! (`group_chat_workspace.rs`, a sibling `bot_roster` child module, also
//! `server`-gate-free) can share this exact implementation rather than
//! adding a THIRD isomorphic copy. This does not weaken the duplication
//! precedent above — that precedent explains why `cli_handoff.rs`'s
//! server-gated original cannot be called from here; it says nothing about
//! sibling wasm-compiling modules sharing ONE copy instead of each growing
//! its own.

use crate::components::hermes_app::screens::bot_roster::mention_handoff::{
    MentionHandoffBlock, MentionHandoffEntry,
};
use crate::components::hermes_app::widgets::bot_face::BotFace;
use crate::protocol::{
    BotChatEntry, BotChatEntryKind, BotHandoffRequest, BotRosterEntry, MentionHandoffRequest,
    MentionHandoffState,
};
use crate::server::bot_chat_store::{append_bot_chat_entry, load_bot_chat_thread};
use crate::server::cli_handoff::dispatch_bot_message;
use crate::server::mention_handoff_api::{dispatch_mention_handoff, resolve_roster_mentions_client};
use dioxus::prelude::*;
use std::collections::BTreeMap;

/// Phase 50.1 OF-5: who authored a chat window row.
//
// Under `--all-features`, `legacy-shell` swaps the reachable root component
// to `WarpHermes`, leaving `HermesApp` (and therefore `BotRoster`,
// `BotCard`, and this whole module) unreferenced from `main()` for
// dead-code-lint purposes — even though it is live in the default
// (non-legacy-shell) build. Same pre-existing pattern as
// `bot_roster.rs`'s own `build_roster_entries`/`BOTS_CSS` and
// `bot_roster/card.rs`'s `NO_CONVERSATIONS_YET`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatWindowRole {
    Operator,
    Bot,
    Error,
}

#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
impl ChatWindowRole {
    /// CSS modifier suffix (`.kn-chat-row--{class}`).
    fn css_class(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Bot => "bot",
            Self::Error => "error",
        }
    }

    /// Row label rendered above the message text.
    fn label(self) -> &'static str {
        match self {
            Self::Operator => "YOU",
            Self::Bot => "BOT",
            Self::Error => "ERROR",
        }
    }
}

/// Phase 50.1 OF-9 (gap task): render-time think-block stripping — a
/// deliberate, isomorphic duplicate of `server::cli_handoff`'s
/// server-gated `strip_think_blocks` (module doc). Removes every
/// `<think>…</think>` block; an unclosed leading `<think>` strips to the
/// end of the string rather than being left dangling.
///
/// Phase 50.2 Plan 11 (G-3): `pub(super)` so `group_chat_workspace.rs` (a
/// sibling `bot_roster` child, also `server`-gate-free) can share this
/// exact implementation for the room transcript's `Replied` arm, rather
/// than adding a third isomorphic copy (module doc above).
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(super) fn strip_think_blocks_for_display(input: &str) -> String {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        match rest.find(OPEN) {
            None => {
                out.push_str(rest);
                break;
            }
            Some(open_idx) => {
                out.push_str(&rest[..open_idx]);
                let after_open = &rest[open_idx + OPEN.len()..];
                match after_open.find(CLOSE) {
                    Some(close_idx) => {
                        rest = &after_open[close_idx + CLOSE.len()..];
                    }
                    None => break,
                }
            }
        }
    }
    out
}

/// Phase 50.1 OF-9 (gap task): shown in place of a Bot row's text when the
/// reply was ENTIRELY a `<think>` block — the dispatch still succeeded and
/// delivered something, so the row is never left blank (module doc).
///
/// Phase 50.2 Plan 11 (G-3): `pub(super)` for the same reason as
/// `strip_think_blocks_for_display` above — the room transcript's
/// think-only fallback must match this exact placeholder, not a
/// hand-written copy of its text.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(super) const BOT_THINK_ONLY_PLACEHOLDER: &str = "(no visible reply — response was reasoning only)";

/// Phase 50.1 OF-5: one rendered row in a bot's chat thread.
///
/// Phase 50.2 Plan 07 (D-20): `mention` distinguishes a handoff block from
/// a normal text row — a distinct row kind, never a text row with markup
/// in it. When `Some`, `role`/`text` are unused placeholders (the shared
/// `MentionHandoffBlock` component reads `mention` instead).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChatWindowEntry {
    pub role: ChatWindowRole,
    pub text: String,
    pub mention: Option<MentionHandoffEntry>,
}

#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
impl ChatWindowEntry {
    /// Phase 50.1 OF-9 (gap task): the text to actually render for this
    /// row. Only `ChatWindowRole::Bot` rows are ever think-stripped —
    /// `Operator`/`Error` text passes through unchanged, matching
    /// `dispatch_bot_message`'s own scope (only a bot's reply can carry a
    /// `<think>` block). A think-only reply (non-empty raw text that
    /// strips to nothing) falls back to [`BOT_THINK_ONLY_PLACEHOLDER`]
    /// rather than rendering an empty row.
    fn display_text(&self) -> String {
        if self.role != ChatWindowRole::Bot {
            return self.text.clone();
        }
        let stripped = strip_think_blocks_for_display(&self.text);
        if stripped.trim().is_empty() && !self.text.trim().is_empty() {
            BOT_THINK_ONLY_PLACEHOLDER.to_string()
        } else {
            stripped
        }
    }
}

/// Phase 50.1 OF-5: append the operator's own message. Pure and
/// disk/DOM-I/O-free — directly unit-testable without a renderer.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(crate) fn append_operator_message(thread: &mut Vec<ChatWindowEntry>, text: &str) {
    thread.push(ChatWindowEntry {
        role: ChatWindowRole::Operator,
        text: text.to_string(),
        mention: None,
    });
}

/// Phase 50.2 Plan 07 (D-20): append a PENDING `@mention` handoff row —
/// `target` is the mentioned bot, `message` is the delegated (quoted)
/// text, always the full sent message (the mention is detected, never
/// consumed or edited).
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(crate) fn append_mention_pending(thread: &mut Vec<ChatWindowEntry>, target: &str, message: &str) {
    thread.push(ChatWindowEntry {
        role: ChatWindowRole::Bot,
        text: String::new(),
        mention: Some(MentionHandoffEntry {
            target: target.to_string(),
            message: message.to_string(),
            state: MentionHandoffState::Pending,
            reply: None,
        }),
    });
}

/// Phase 50.2 Plan 07 (D-20): resolve the MOST RECENT still-pending entry
/// for `target` to `Resolved`/`Failed` — the last-appended pending match
/// (rather than the first) is correct even if the operator sends a second
/// `@{target}` mention before the first one resolves.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(crate) fn resolve_mention_entry(
    thread: &mut [ChatWindowEntry],
    target: &str,
    outcome: Result<crate::protocol::MentionHandoffResult, String>,
) {
    if let Some(entry) = thread.iter_mut().rev().find(|e| {
        e.mention
            .as_ref()
            .is_some_and(|m| m.target == target && m.state == MentionHandoffState::Pending)
    }) {
        if let Some(mention) = entry.mention.as_mut() {
            match outcome {
                Ok(result) => {
                    mention.state = MentionHandoffState::Resolved;
                    mention.reply = Some(result.reply);
                }
                Err(reason) => {
                    mention.state = MentionHandoffState::Failed { reason };
                }
            }
        }
    }
}

/// Phase 50.1 OF-5: append the bot's (already OF-5-cleaned, via
/// `dispatch_bot_message` -> `run_bot_handoff`'s own reply-scrubbing step)
/// reply.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(crate) fn append_bot_reply(thread: &mut Vec<ChatWindowEntry>, text: &str) {
    thread.push(ChatWindowEntry {
        role: ChatWindowRole::Bot,
        text: text.to_string(),
        mention: None,
    });
}

/// Phase 50.1 OF-5: append a failed-dispatch error row. The thread keeps the
/// operator's own message (appended before the dispatch was attempted) —
/// only the bot's side of the exchange failed.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(crate) fn append_error(thread: &mut Vec<ChatWindowEntry>, text: &str) {
    thread.push(ChatWindowEntry {
        role: ChatWindowRole::Error,
        text: text.to_string(),
        mention: None,
    });
}

// -------------------------------------------------------------------
// Phase 50.2 Plan 08 (D-13/D-23/D-11/D-04/D-21): backing this window's
// thread with `server::bot_chat_store` — the OPERATOR half of "every bot
// has a persistent Bot Chat" (plan 02 gave the BOT half). The window's
// visual shell and its `append_operator_message`/`append_bot_reply`/
// `append_error` helpers above are UNCHANGED (UI-SPEC Component
// Inventory §1) — this section only adds the conversion to/from the
// persisted `BotChatEntry` shape and a persistence write alongside each
// existing append, never a new append shape. `@mention` handoff rows
// (`ChatWindowEntry.mention`) are session-transient, client-side state per
// plan 07's own recorded scope boundary — they are never converted to or
// from `BotChatEntry` and are never persisted by this store.
// -------------------------------------------------------------------

/// Phase 50.2 Plan 08: `chat_window.rs`'s render-time role ->
/// `bot_chat_store`'s persisted kind. Exhaustive, no wildcard arm — a
/// future variant added to either enum without updating this fn fails to
/// compile rather than silently dropping a row kind.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
fn chat_window_role_to_bot_chat_entry_kind(role: ChatWindowRole) -> BotChatEntryKind {
    match role {
        ChatWindowRole::Operator => BotChatEntryKind::Operator,
        ChatWindowRole::Bot => BotChatEntryKind::Bot,
        ChatWindowRole::Error => BotChatEntryKind::Error,
    }
}

/// Phase 50.2 Plan 08: the inverse of
/// [`chat_window_role_to_bot_chat_entry_kind`] — a persisted kind back to
/// this window's render-time role.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
fn bot_chat_entry_kind_to_chat_window_role(kind: BotChatEntryKind) -> ChatWindowRole {
    match kind {
        BotChatEntryKind::Operator => ChatWindowRole::Operator,
        BotChatEntryKind::Bot => ChatWindowRole::Bot,
        BotChatEntryKind::Error => ChatWindowRole::Error,
    }
}

/// Phase 50.2 Plan 08: build the `BotChatEntry` to persist for one append —
/// `at_ms` is always overwritten server-side by
/// `bot_chat_store::append_bot_chat_entries_impl`, so `0` here is a safe
/// placeholder, never trusted.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
fn bot_chat_entry_for_send(role: ChatWindowRole, text: &str) -> BotChatEntry {
    BotChatEntry {
        kind: chat_window_role_to_bot_chat_entry_kind(role),
        text: text.to_string(),
        at_ms: 0,
    }
}

/// Phase 50.2 Plan 08: one persisted row converted to this window's render
/// shape. `mention` is always `None` — a persisted `BotChatEntry` has no
/// mention-handoff kind (mention rows are never persisted, module doc).
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
fn bot_chat_entry_to_window_entry(entry: BotChatEntry) -> ChatWindowEntry {
    ChatWindowEntry {
        role: bot_chat_entry_kind_to_chat_window_role(entry.kind),
        text: entry.text,
        mention: None,
    }
}

/// Phase 50.2 Plan 08 (UI-SPEC State Matrix): the window's loading/empty/
/// populated render fork, pinned by a dedicated unit test rather than left
/// to render-only UAT — the "no loading flash on a miss" rule depends on
/// this fn treating a resolved-but-empty thread distinctly from an
/// unresolved one. `loaded_len` is `None` while the fetch is unresolved,
/// `Some(0)` once resolved to an empty thread (a fast local lookup — a
/// miss is never re-treated as still-loading), `Some(n)` once resolved to
/// `n` entries.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadRenderState {
    Loading,
    Empty,
    Rows(usize),
}

#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(crate) fn thread_render_state(loaded_len: Option<usize>) -> ThreadRenderState {
    match loaded_len {
        None => ThreadRenderState::Loading,
        Some(0) => ThreadRenderState::Empty,
        Some(n) => ThreadRenderState::Rows(n),
    }
}

/// Phase 50.2 Plan 16 (G-50.2-2c): the operator-facing copy for a queued
/// send — names the bot and states how many messages are now waiting for
/// its next turn, with a singular form for exactly one. Pure, no I/O,
/// directly unit-testable.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
pub(crate) fn queued_for_next_turn_notice(bot_name: &str, depth: u32) -> String {
    if depth == 1 {
        format!(
            "Message queued — {bot_name} will see it on its next turn."
        )
    } else {
        format!(
            "Message queued — {bot_name} will see it on its next turn ({depth} messages waiting)."
        )
    }
}

/// Phase 50.2 Plan 20 (WR-01, gap-round-3): the Bot Chat composer's own
/// mention-handoff target set — [`resolve_roster_mentions_client`]'s output
/// with the window's own bot filtered out. The window's own bot is ALREADY
/// dispatched by `submit_send`'s primary path through
/// [`dispatch_bot_message`], so letting it also match as a mention target
/// fires a second, concurrent [`dispatch_mention_handoff`] at the same bot
/// under the same `Bot Chat` session title — two separately-launched child
/// processes racing `resolve_or_create_titled_session` in that bot's
/// `state.db`, which can leave two sessions sharing one title. Mirrors
/// `group_chat_workspace.rs`'s `mention_targets_for_room` exactly: same
/// hazard class (a mention consumer double-dispatching a target its own
/// surface already covers), same layer (a pure filter over the shared
/// resolver's output), same `eq_ignore_ascii_case` comparison — that
/// function excludes room MEMBERS because the group driver's own responder
/// resolution covers them; this one excludes the window's own bot because
/// `submit_send`'s primary path covers it.
pub(crate) fn mention_targets_for_bot_chat(
    text: &str,
    roster_names: &[String],
    bot_name: &str,
) -> Vec<String> {
    resolve_roster_mentions_client(text, roster_names, "operator")
        .into_iter()
        .filter(|target| !target.eq_ignore_ascii_case(bot_name))
        .collect()
}

/// Phase 50.1 OF-5: the send action, shared by the input's Enter-to-send
/// handler and the SEND button's click handler. A free fn taking every
/// dependency as an explicit param — not a closure captured by one rsx!
/// event handler — precisely so BOTH handlers can call it without fighting
/// over ownership of a single `FnMut` closure value (`Signal`/`EventHandler`
/// are `Copy`; `bot_name` is cloned once per call site by the caller).
///
/// Phase 50.2 Plan 16 (G-50.2-2c): guards ONLY on an empty trimmed draft —
/// the already-sending term is gone (that term was the hard lock G-50.2-2c
/// names). `in_flight_sends` is an outstanding-send COUNTER, not a single
/// `bool`, so two concurrent sends (e.g. a queued send resolving while a
/// fresh one is submitted) can never clobber each other's state:
/// incremented synchronously before the dispatch task spawns, decremented
/// inside that task's own resolve block on every exit path (success,
/// queued, and error).
///
/// Phase 50.2 Plan 07 (D-20): `roster_names` is every actual bot in the
/// roster — `resolve_roster_mentions_client` (the client-safe twin of the
/// server's `resolve_roster_mentions`) resolves `@mention` targets on the
/// outbound text BEFORE dispatch. A pending handoff block is appended for
/// EACH target, then every target is dispatched in PARALLEL (`spawn` one
/// task per target, never awaited in sequence, per the 2026-08-19 operator
/// ruling) — each block resolves independently as its own reply lands,
/// never blocking on the others.
///
/// Phase 50.2 Plan 20 (WR-01): the resolved target set now excludes the
/// window's OWN bot — see [`mention_targets_for_bot_chat`]'s doc comment for
/// why. The `roster_names` prop stays the FULL, unfiltered roster (needed so
/// a typo can still be told apart from a real bot); the self-exclusion lives
/// entirely in the filter, not in what's passed to it.
#[allow(dead_code)] // see ChatWindowRole's doc note above (legacy-shell reachability)
fn submit_send(
    bot_name: String,
    mut draft: Signal<String>,
    mut in_flight_sends: Signal<u32>,
    mut queued: Signal<Option<(u32, String)>>,
    mut chat_threads: Signal<BTreeMap<String, Vec<ChatWindowEntry>>>,
    roster_names: Vec<String>,
    on_message_sent: EventHandler<()>,
) {
    let text = draft.read().clone();
    if text.trim().is_empty() {
        return;
    }
    draft.set(String::new());
    let current_in_flight = *in_flight_sends.read();
    in_flight_sends.set(current_in_flight + 1);

    let mention_targets = mention_targets_for_bot_chat(&text, &roster_names, &bot_name);

    // Append the operator's own message, then one pending handoff block per
    // mentioned target, synchronously — the write-lock is acquired and
    // dropped entirely within this block, well before `spawn` below, never
    // held across an `.await`.
    {
        let mut map = chat_threads.write();
        let thread = map.entry(bot_name.clone()).or_default();
        append_operator_message(thread, &text);
        for target in &mention_targets {
            append_mention_pending(thread, target, &text);
        }
    }

    // Phase 50.2 Plan 08: persist the operator's own message BEFORE the
    // dispatch below is awaited, so a failure or a mid-flight navigation
    // can never lose what the operator typed — a persistence write NEXT TO
    // the append above, never inside `append_operator_message` itself
    // (which stays pure and directly unit-testable). Fire-and-forget: the
    // write is serialized server-side by its own `BOT_CHAT_LOCK`, and does
    // not need to block this event handler or the dispatch below.
    {
        let bot_name_for_persist = bot_name.clone();
        let persisted_entry = bot_chat_entry_for_send(ChatWindowRole::Operator, &text);
        spawn(async move {
            let _ = append_bot_chat_entry(bot_name_for_persist, persisted_entry).await;
        });
    }

    for target in mention_targets {
        let message = text.clone();
        let bot_name_for_mention = bot_name.clone();
        let mut chat_threads_for_mention = chat_threads;
        spawn(async move {
            let outcome = dispatch_mention_handoff(MentionHandoffRequest {
                target: target.clone(),
                message,
            })
            .await
            .map_err(|e| e.to_string());
            let mut map = chat_threads_for_mention.write();
            if let Some(thread) = map.get_mut(&bot_name_for_mention) {
                resolve_mention_entry(thread, &target, outcome);
            }
        });
    }

    spawn(async move {
        let result = dispatch_bot_message(BotHandoffRequest {
            name: bot_name.clone(),
            message: text,
        })
        .await;
        // Fresh write-lock, acquired only after the await resolves, and
        // scoped to this block — dropped by the block's own exit, well
        // before the second `.await` below (`append_bot_chat_entry`),
        // never the same lock instance carried across either await.
        //
        // Phase 50.2 Plan 16 (G-50.2-2c): `dispatch_bot_message` returns
        // `BotDispatchOutcome`. The `Replied` arm does exactly what the
        // pre-Plan-16 `Ok` arm did (append the bot reply, persist it, call
        // `on_message_sent`) and additionally clears `queued` — a reply
        // means the turn that mattered to any prior queued notice has now
        // resolved. The `Queued` arm appends NOTHING to the thread (the
        // operator's own row is already durable from the pre-dispatch
        // write — this is not a second send) and does not call
        // `on_message_sent`; it sets `queued` to the returned depth and
        // bot name so the composer can render the notice. The pending `…`
        // bot row stays driven by `is_sending`
        // (`in_flight_sends`-derived) — an unrelated queued send resolving
        // must never clear the STILL-in-flight turn's own pending
        // indicator, which is exactly why this decrements the counter
        // rather than setting a single `bool` to `false`.
        {
            let mut map = chat_threads.write();
            let bot_thread = map.entry(bot_name.clone()).or_default();
            match &result {
                Ok(crate::protocol::BotDispatchOutcome::Replied(res)) => {
                    append_bot_reply(bot_thread, &res.reply);
                    queued.set(None);
                }
                Ok(crate::protocol::BotDispatchOutcome::Queued { depth }) => {
                    queued.set(Some((*depth, bot_name.clone())));
                }
                Err(e) => append_error(bot_thread, &format!("{e}")),
            }
        }
        let remaining_in_flight = in_flight_sends.read().saturating_sub(1);
        in_flight_sends.set(remaining_in_flight);
        if !matches!(result, Ok(crate::protocol::BotDispatchOutcome::Queued { .. })) {
            on_message_sent.call(());
        }

        // Phase 50.2 Plan 08: persist the reply/error row alongside the
        // in-memory append above — mirrors the operator-message
        // persistence write earlier in this fn. The bot's own message
        // (appended before this dispatch was attempted) is already
        // persisted, so a failed dispatch here keeps it, matching
        // `append_error`'s own doc contract. A `Queued` outcome persists
        // NOTHING here — the operator's own message row is already durable
        // from the pre-dispatch write, and there is no reply yet to persist
        // (this IS the "no second write" the operator's own send already
        // satisfies — this plan's own recorded truth).
        let persisted_entry = match &result {
            Ok(crate::protocol::BotDispatchOutcome::Replied(res)) => {
                Some(bot_chat_entry_for_send(ChatWindowRole::Bot, &res.reply))
            }
            Ok(crate::protocol::BotDispatchOutcome::Queued { .. }) => None,
            Err(e) => Some(bot_chat_entry_for_send(ChatWindowRole::Error, &format!("{e}"))),
        };
        if let Some(entry) = persisted_entry {
            let _ = append_bot_chat_entry(bot_name.clone(), entry).await;
        }
    });
}

/// Phase 50.1 OF-5: the floating one-on-one chat window. Mounted
/// unconditionally by `BotRoster` (same "renders nothing until targeted"
/// idiom as `ProfileDetailDrawer`) — `entry` is the roster row for the
/// targeted bot, resolved by the caller. `chat_threads` is the SAME
/// `Signal<BTreeMap<String, Vec<ChatWindowEntry>>>` the caller owns — a
/// direct signal prop (mirrors `drawer_target: Signal<Option<String>>`
/// already flowing through this same screen), not a callback round-trip,
/// since this map is per-bot session-transient UI state with no server
/// counterpart to reconcile.
#[component]
pub fn BotChatWindow(
    entry: BotRosterEntry,
    chat_threads: Signal<BTreeMap<String, Vec<ChatWindowEntry>>>,
    // Phase 50.2 Plan 07 (D-20): every actual bot in the roster — passed to
    // `resolve_roster_mentions_client` at send time so a typo or an
    // unrelated `@handle` never dispatches a subprocess.
    roster_names: Vec<String>,
    on_close: EventHandler<()>,
    on_message_sent: EventHandler<()>,
) -> Element {
    let bot_name = entry.row.name.clone();
    let title = entry
        .meta
        .as_ref()
        .and_then(|m| m.title.clone())
        .unwrap_or_else(|| bot_name.clone());
    let avatar = entry.meta.as_ref().and_then(|m| m.avatar.clone());
    let avatar_shape = avatar.as_ref().and_then(|a| a.shape.clone());
    let avatar_color = avatar.as_ref().and_then(|a| a.color.clone());
    let avatar_image_id = avatar.as_ref().and_then(|a| a.image_id.clone());

    let mut draft: Signal<String> = use_signal(String::new);
    // Phase 50.2 Plan 16 (G-50.2-2c): an outstanding-send COUNTER, not a
    // single `bool` — two concurrent sends (a queued send resolving while a
    // fresh one is in flight) must never clobber each other's state.
    let in_flight_sends: Signal<u32> = use_signal(|| 0u32);
    // Phase 50.2 Plan 16 (G-50.2-2c): depth and bot name for the most
    // recent queued-for-next-turn notice. Cleared when a `Replied` outcome
    // lands — a reply means the turn that mattered to this notice resolved.
    let queued: Signal<Option<(u32, String)>> = use_signal(|| None);

    // Phase 50.2 Plan 08: resume the persisted thread on open. `use_resource`
    // (this crate's binding discipline: never the sibling resource hook
    // paired with an explicit relaunch call) — the closure captures
    // `bot_name` by value, so it runs
    // once per mount, which is exactly the semantics this window needs:
    // `BotChatWindow` is freshly mounted every time `chat_target` flips
    // from `None` to `Some(name)` (bot_roster.rs's "renders nothing until
    // targeted" idiom), so a fresh mount IS a fresh resume.
    let thread_resource = use_resource({
        let bot_name = bot_name.clone();
        move || {
            let bot_name = bot_name.clone();
            async move { load_bot_chat_thread(bot_name).await }
        }
    });

    // Phase 50.2 Plan 08: seed the session-transient `chat_threads` map
    // from the resolved thread — but only ONCE per bot (a subsequent send
    // already writes through `chat_threads` directly, and re-seeding on
    // every resource poll would clobber those in-session appends). A load
    // failure degrades to an empty seeded thread, matching 50.1's original
    // "no prior history" render rather than a stuck spinner.
    use_effect({
        let bot_name = bot_name.clone();
        move || {
            if let Some(result) = thread_resource() {
                let mut map = chat_threads;
                let already_seeded = map.peek().contains_key(&bot_name);
                if !already_seeded {
                    let converted: Vec<ChatWindowEntry> = match result {
                        Ok(thread) => thread
                            .entries
                            .into_iter()
                            .map(bot_chat_entry_to_window_entry)
                            .collect(),
                        Err(_) => Vec::new(),
                    };
                    map.write().insert(bot_name.clone(), converted);
                }
            }
        }
    });

    // ---- Derived values (read BEFORE rsx!, clippy.toml discipline). ----
    let draft_val = draft.read().clone();
    let is_sending = *in_flight_sends.read() > 0;
    // Phase 50.2 Plan 16 (G-50.2-2c): sendable throughout an in-flight
    // turn — only an empty draft disables SEND. The composer input itself
    // is never `readonly` any more (see the input element below).
    let can_send = !draft_val.trim().is_empty();
    let queued_val = queued.read().clone();
    // Phase 50.2 Plan 08: `chat_threads` is the source of truth once the
    // effect above has seeded it (or a send has appended to it). Before
    // that first seed lands, fall back to rendering directly from the
    // just-resolved resource so there is no one-frame empty flash between
    // "resource resolved" and "effect committed the seed."
    let session_thread: Option<Vec<ChatWindowEntry>> = chat_threads.read().get(&bot_name).cloned();
    let thread_for_render: Vec<ChatWindowEntry> = match session_thread {
        Some(rows) => rows,
        None => match thread_resource() {
            Some(Ok(thread)) => thread
                .entries
                .into_iter()
                .map(bot_chat_entry_to_window_entry)
                .collect(),
            _ => Vec::new(),
        },
    };
    // Phase 50.2 Plan 08 (UI-SPEC State Matrix): `Loading conversation…`
    // while the resource is unresolved; once resolved, the empty/populated
    // fork below never shows the loading line again, even for a miss.
    let loaded_len: Option<usize> = match thread_resource() {
        None => None,
        Some(Ok(thread)) => Some(thread.entries.len()),
        Some(Err(_)) => Some(0),
    };
    let render_state = thread_render_state(loaded_len);
    let placeholder = format!("Message {bot_name}…");
    let send_label = if is_sending { "SENDING…" } else { "SEND" };

    rsx! {
        div { class: "kn-modal-overlay", role: "presentation",
            div {
                class: "kn-modal kn-chat-window",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "kn-chat-window-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        on_close.call(());
                    }
                },
                div { class: "kn-modal-header kn-chat-header",
                    BotFace {
                        name: bot_name.clone(),
                        size: 32u32,
                        shape: avatar_shape,
                        color_token: avatar_color,
                        image_id: avatar_image_id,
                    }
                    h3 { class: "kn-modal-title", id: "kn-chat-window-title", "{title}" }
                    button {
                        class: "kn-drawer-close",
                        "aria-label": "Close chat with {bot_name}",
                        onclick: move |_| on_close.call(()),
                        "×"
                    }
                }
                div { class: "kn-chat-thread",
                    if render_state == ThreadRenderState::Loading {
                        // Phase 50.2 Plan 08 (UI-SPEC Component Inventory
                        // §1 / Copywriting Contract): shown ONLY while the
                        // fetch is unresolved — never re-shown for a
                        // resolved-but-empty thread (the miss case below).
                        div { class: "kn-drawer-loading", "Loading conversation…" }
                    } else if thread_for_render.is_empty() {
                        div { class: "kn-drawer-empty", "No messages yet. Say hello." }
                    } else {
                        for (i , item) in thread_for_render.iter().enumerate() {
                            {
                                if let Some(mention) = item.mention.clone() {
                                    rsx! {
                                        div { key: "{i}",
                                            MentionHandoffBlock { entry: mention }
                                        }
                                    }
                                } else {
                                    rsx! {
                                        div {
                                            key: "{i}",
                                            class: "kn-chat-row kn-chat-row--{item.role.css_class()}",
                                            div { class: "kn-chat-row-label", "{item.role.label()}" }
                                            div { class: "kn-chat-row-text", "{item.display_text()}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if is_sending {
                        div { class: "kn-chat-row kn-chat-row--bot kn-chat-row--pending",
                            div { class: "kn-chat-row-label", "BOT" }
                            div { class: "kn-chat-row-text", "…" }
                        }
                    }
                }
                // Phase 50.2 Plan 16 (G-50.2-2c): the queued-for-next-turn
                // notice — transient composer chrome, never a persisted
                // thread row (module doc, this plan's own prohibition on a
                // new `BotChatEntryKind`). Reuses `kn-modal-hint--info`
                // (no new CSS asset).
                if let Some((depth, notice_bot_name)) = queued_val.as_ref() {
                    div { class: "kn-modal-hint--info",
                        "{queued_for_next_turn_notice(notice_bot_name, *depth)}"
                    }
                }
                div { class: "kn-chat-input-row",
                    input {
                        class: "kn-modal-input kn-chat-input",
                        r#type: "text",
                        value: "{draft_val}",
                        placeholder: "{placeholder}",
                        onkeydown: {
                            let bot_name_for_send = bot_name.clone();
                            let roster_names_for_send = roster_names.clone();
                            move |evt| {
                                if evt.key() == Key::Enter {
                                    submit_send(
                                        bot_name_for_send.clone(),
                                        draft,
                                        in_flight_sends,
                                        queued,
                                        chat_threads,
                                        roster_names_for_send.clone(),
                                        on_message_sent,
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
                            let bot_name_for_send = bot_name.clone();
                            let roster_names_for_send = roster_names.clone();
                            move |_| {
                                submit_send(
                                    bot_name_for_send.clone(),
                                    draft,
                                    in_flight_sends,
                                    queued,
                                    chat_threads,
                                    roster_names_for_send.clone(),
                                    on_message_sent,
                                );
                            }
                        },
                        "{send_label}"
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
    // queued_for_next_turn_notice (Phase 50.2 Plan 16, Task 2, G-50.2-2c)
    // -------------------------------------------------------------------

    #[test]
    fn queued_for_next_turn_notice_depth_one_uses_singular_form() {
        let notice = queued_for_next_turn_notice("zig", 1);
        assert!(notice.contains("zig"), "must name the bot, got={notice:?}");
        assert!(
            !notice.contains("1 messages") && !notice.contains("(1 "),
            "depth one must not render a parenthetical count, got={notice:?}"
        );
    }

    #[test]
    fn queued_for_next_turn_notice_depth_three_states_the_count() {
        let notice = queued_for_next_turn_notice("zig", 3);
        assert!(notice.contains("zig"), "must name the bot, got={notice:?}");
        assert!(
            notice.contains('3'),
            "must state how many messages are waiting, got={notice:?}"
        );
    }

    #[test]
    fn queued_for_next_turn_notice_handles_a_bot_name_containing_a_space() {
        let notice = queued_for_next_turn_notice("Ops Bot", 2);
        assert!(
            notice.contains("Ops Bot"),
            "must name the bot verbatim even with an embedded space, got={notice:?}"
        );
        assert!(notice.contains('2'), "got={notice:?}");
    }

    // -------------------------------------------------------------------
    // thread-state logic: append user msg -> in-flight -> append reply/error
    // -------------------------------------------------------------------

    #[test]
    fn append_operator_message_pushes_operator_role_entry() {
        let mut thread = Vec::new();
        append_operator_message(&mut thread, "hello there");
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].role, ChatWindowRole::Operator);
        assert_eq!(thread[0].text, "hello there");
    }

    #[test]
    fn append_bot_reply_pushes_bot_role_entry() {
        let mut thread = Vec::new();
        append_bot_reply(&mut thread, "hi, how can I help?");
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].role, ChatWindowRole::Bot);
        assert_eq!(thread[0].text, "hi, how can I help?");
    }

    #[test]
    fn append_error_pushes_error_role_entry() {
        let mut thread = Vec::new();
        append_error(&mut thread, "dispatch failed: timeout");
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].role, ChatWindowRole::Error);
        assert_eq!(thread[0].text, "dispatch failed: timeout");
    }

    #[test]
    fn send_sequence_operator_message_then_bot_reply_appends_in_order() {
        let mut thread = Vec::new();
        append_operator_message(&mut thread, "what's the weather?");
        // In-flight: no third-party pure fn to call here — `sending` is
        // Signal-based render state, exercised only via UAT (same
        // discipline `bot_roster/handoff.rs`'s own doc note establishes:
        // interactive/render-only behavior is left to UAT, only pure
        // thread-mutation logic is unit-tested here).
        append_bot_reply(&mut thread, "sunny and 72°F");

        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].role, ChatWindowRole::Operator);
        assert_eq!(thread[0].text, "what's the weather?");
        assert_eq!(thread[1].role, ChatWindowRole::Bot);
        assert_eq!(thread[1].text, "sunny and 72°F");
    }

    #[test]
    fn send_sequence_operator_message_then_error_appends_in_order_and_keeps_operator_message() {
        let mut thread = Vec::new();
        append_operator_message(&mut thread, "run the deploy script");
        append_error(&mut thread, "profile \"scout\" is missing its API key");

        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].role, ChatWindowRole::Operator);
        assert_eq!(thread[1].role, ChatWindowRole::Error);
        assert_eq!(thread[1].text, "profile \"scout\" is missing its API key");
    }

    #[test]
    fn multiple_send_cycles_accumulate_in_one_thread() {
        let mut thread = Vec::new();
        append_operator_message(&mut thread, "first message");
        append_bot_reply(&mut thread, "first reply");
        append_operator_message(&mut thread, "second message");
        append_bot_reply(&mut thread, "second reply");

        assert_eq!(thread.len(), 4);
        assert_eq!(
            thread.iter().map(|e| e.role).collect::<Vec<_>>(),
            vec![
                ChatWindowRole::Operator,
                ChatWindowRole::Bot,
                ChatWindowRole::Operator,
                ChatWindowRole::Bot,
            ]
        );
    }

    // -------------------------------------------------------------------
    // ChatWindowRole rendering helpers
    // -------------------------------------------------------------------

    #[test]
    fn role_css_class_and_label_are_distinct_per_variant() {
        assert_eq!(ChatWindowRole::Operator.css_class(), "operator");
        assert_eq!(ChatWindowRole::Bot.css_class(), "bot");
        assert_eq!(ChatWindowRole::Error.css_class(), "error");
        assert_eq!(ChatWindowRole::Operator.label(), "YOU");
        assert_eq!(ChatWindowRole::Bot.label(), "BOT");
        assert_eq!(ChatWindowRole::Error.label(), "ERROR");
    }

    // -------------------------------------------------------------------
    // OF-9 (gap task): think-block hiding
    // -------------------------------------------------------------------

    #[test]
    fn strip_think_blocks_for_display_removes_a_closed_block() {
        let raw = "<think>internal reasoning</think>the real reply";
        assert_eq!(strip_think_blocks_for_display(raw), "the real reply");
    }

    #[test]
    fn strip_think_blocks_for_display_removes_multiple_blocks() {
        let raw = "<think>a</think>Part one. <think>b</think>Part two.";
        assert_eq!(strip_think_blocks_for_display(raw), "Part one. Part two.");
    }

    #[test]
    fn strip_think_blocks_for_display_unclosed_leading_tag_strips_to_end() {
        let raw = "<think>reasoning that never closes";
        assert_eq!(strip_think_blocks_for_display(raw), "");
    }

    #[test]
    fn strip_think_blocks_for_display_no_op_on_plain_text() {
        let raw = "just an ordinary reply";
        assert_eq!(strip_think_blocks_for_display(raw), raw);
    }

    #[test]
    fn display_text_bot_row_strips_think_block() {
        let entry = ChatWindowEntry {
            role: ChatWindowRole::Bot,
            text: "<think>reasoning</think>the visible answer".to_string(),
            mention: None,
        };
        assert_eq!(entry.display_text(), "the visible answer");
    }

    #[test]
    fn display_text_bot_row_think_only_reply_falls_back_to_placeholder() {
        let entry = ChatWindowEntry {
            role: ChatWindowRole::Bot,
            text: "<think>only reasoning, nothing else</think>".to_string(),
            mention: None,
        };
        assert_eq!(entry.display_text(), BOT_THINK_ONLY_PLACEHOLDER);
    }

    #[test]
    fn display_text_bot_row_plain_reply_passes_through_unchanged() {
        let entry = ChatWindowEntry {
            role: ChatWindowRole::Bot,
            text: "a perfectly ordinary reply".to_string(),
            mention: None,
        };
        assert_eq!(entry.display_text(), "a perfectly ordinary reply");
    }

    #[test]
    fn display_text_bot_row_genuinely_empty_reply_stays_empty_not_placeholder() {
        // A legitimately empty reply (unrelated to think-blocks) must not
        // be relabeled as "reasoning only" — the placeholder is only for
        // text that WAS non-empty before stripping.
        let entry = ChatWindowEntry {
            role: ChatWindowRole::Bot,
            text: String::new(),
            mention: None,
        };
        assert_eq!(entry.display_text(), "");
    }

    #[test]
    fn display_text_operator_and_error_rows_never_think_stripped() {
        let operator = ChatWindowEntry {
            role: ChatWindowRole::Operator,
            text: "<think>not actually a think block, just text I typed</think>".to_string(),
            mention: None,
        };
        let error = ChatWindowEntry {
            role: ChatWindowRole::Error,
            text: "<think>same here</think>".to_string(),
            mention: None,
        };
        assert_eq!(operator.display_text(), operator.text);
        assert_eq!(error.display_text(), error.text);
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 08: BotChatEntry <-> ChatWindowEntry round trip
    // -------------------------------------------------------------------

    #[test]
    fn round_trip_conversion_preserves_all_three_kinds_and_their_text() {
        for (role, text) in [
            (ChatWindowRole::Operator, "hello there"),
            (ChatWindowRole::Bot, "hi, how can I help?"),
            (ChatWindowRole::Error, "dispatch failed: timeout"),
        ] {
            let persisted = bot_chat_entry_for_send(role, text);
            assert_eq!(persisted.text, text);
            let window_entry = bot_chat_entry_to_window_entry(persisted);
            assert_eq!(window_entry.role, role);
            assert_eq!(window_entry.text, text);
            assert_eq!(window_entry.mention, None);
        }
    }

    #[test]
    fn kind_role_conversions_are_exact_inverses() {
        for role in [
            ChatWindowRole::Operator,
            ChatWindowRole::Bot,
            ChatWindowRole::Error,
        ] {
            let kind = chat_window_role_to_bot_chat_entry_kind(role);
            assert_eq!(bot_chat_entry_kind_to_chat_window_role(kind), role);
        }
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 08: thread_render_state — the no-loading-flash-on-a-
    // miss rule
    // -------------------------------------------------------------------

    #[test]
    fn thread_render_state_unresolved_resource_is_loading() {
        assert_eq!(thread_render_state(None), ThreadRenderState::Loading);
    }

    #[test]
    fn thread_render_state_resolved_empty_is_empty_not_loading() {
        assert_eq!(thread_render_state(Some(0)), ThreadRenderState::Empty);
    }

    #[test]
    fn thread_render_state_resolved_two_entries_is_rows_two() {
        assert_eq!(thread_render_state(Some(2)), ThreadRenderState::Rows(2));
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 20 (WR-01): mention_targets_for_bot_chat — the
    // window's own bot is excluded from the mention-handoff target set,
    // because `submit_send`'s primary path already dispatches it. Mirrors
    // `group_chat_workspace.rs`'s own `mention_targets_for_room_*` tests.
    // -------------------------------------------------------------------

    fn bot_chat_roster() -> Vec<String> {
        vec!["scout".to_string(), "zig".to_string(), "ada".to_string()]
    }

    #[test]
    fn mention_targets_for_bot_chat_excludes_the_window_bot() {
        assert_eq!(
            mention_targets_for_bot_chat("@zig ping", &bot_chat_roster(), "zig"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mention_targets_for_bot_chat_excludes_the_window_bot_case_insensitively() {
        assert_eq!(
            mention_targets_for_bot_chat("@ZIG ping", &bot_chat_roster(), "zig"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mention_targets_for_bot_chat_keeps_another_roster_bot() {
        assert_eq!(
            mention_targets_for_bot_chat("@scout ping", &bot_chat_roster(), "zig"),
            vec!["scout".to_string()]
        );
    }

    #[test]
    fn mention_targets_for_bot_chat_mixed_mention_drops_only_the_self_target() {
        assert_eq!(
            mention_targets_for_bot_chat("@zig and @scout, sync up", &bot_chat_roster(), "zig"),
            vec!["scout".to_string()]
        );
    }

    #[test]
    fn mention_targets_for_bot_chat_no_mentions_returns_empty() {
        assert_eq!(
            mention_targets_for_bot_chat("no mentions here", &bot_chat_roster(), "zig"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mention_targets_for_bot_chat_unknown_handle_returns_empty() {
        assert_eq!(
            mention_targets_for_bot_chat("@nobody ping", &bot_chat_roster(), "zig"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn mention_targets_for_bot_chat_everyone_literal_matches_the_underlying_resolver() {
        let roster = bot_chat_roster();
        let expected: Vec<String> = resolve_roster_mentions_client("@everyone ping", &roster, "operator")
            .into_iter()
            .filter(|target| !target.eq_ignore_ascii_case("zig"))
            .collect();
        assert_eq!(
            mention_targets_for_bot_chat("@everyone ping", &roster, "zig"),
            expected
        );
    }
}
