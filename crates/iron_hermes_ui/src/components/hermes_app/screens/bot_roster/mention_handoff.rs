//! Phase 50.2 Plan 07 (UI-SPEC Component Inventory §2): the shared
//! `@mention` handoff quote block — ONE component, THREE call sites (the
//! Bot Chat window, a room transcript, and the live Chat screen). Never a
//! per-surface variant.
//!
//! Renders as an extra transcript row, inserted immediately after the
//! sender's message that contained the `@mention` — the mention is
//! detected, never consumed or edited; the original message stays intact.
//!
//! **State Matrix (verbatim):**
//! - Pending: accent-tinted border, head `→ @{bot}`, body shows only the
//!   delegated (quoted) message.
//! - Resolved: border fades to `var(--fg-dim)`, head `@{bot} replied`, the
//!   reply appends below the quote at full weight.
//! - Failed: danger-tinted border, head `@{bot} — no reply`, the fixed
//!   failure copy renders in place of a reply. The block never silently
//!   disappears in any state.
//!
//! The quoted delegated message is ALWAYS visible, in every state,
//! regardless of length — the reader never scrolls back to find what was
//! asked. A bot's reply passes through the SAME think-stripping convention
//! `chat_window.rs`'s `ChatWindowEntry::display_text` applies — a
//! reasoning model's preamble must not surface here any more than it does
//! in a Bot Chat row (this module's own `mention_reply_display_text` is an
//! isomorphic duplicate of `strip_think_blocks_for_display`, following the
//! SAME "each `#[cfg(test)]`/feature-boundary module is its own namespace"
//! duplication precedent already used throughout this crate).

use crate::protocol::MentionHandoffState;
use dioxus::prelude::*;

// Under `--all-features`, `legacy-shell` swaps the reachable root component
// to `WarpHermes`, leaving `HermesApp` (and therefore every `bot_roster/`
// component this module is only ever reached through) unreferenced from
// `main()` for dead-code-lint purposes — even though it is live in the
// default (non-legacy-shell) build. Same pre-existing pattern as
// `chat_window.rs`'s `ChatWindowRole`/`submit_send` and
// `group_chat_workspace.rs`'s `now_ms`.
#[allow(dead_code)]
const THINK_OPEN: &str = "<think>";
#[allow(dead_code)]
const THINK_CLOSE: &str = "</think>";

/// Phase 50.2 Plan 07: an isomorphic duplicate of `chat_window.rs`'s
/// `strip_think_blocks_for_display` — this module compiles for the wasm
/// client target too, so it cannot call that module-private fn directly.
/// Removes every `<think>…</think>` block; an unclosed leading `<think>`
/// strips to the end of the string rather than being left dangling.
#[allow(dead_code)] // see THINK_OPEN's doc note above (legacy-shell reachability)
fn strip_think_blocks_for_mention_display(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        match rest.find(THINK_OPEN) {
            None => {
                out.push_str(rest);
                break;
            }
            Some(open_idx) => {
                out.push_str(&rest[..open_idx]);
                let after_open = &rest[open_idx + THINK_OPEN.len()..];
                match after_open.find(THINK_CLOSE) {
                    Some(close_idx) => {
                        rest = &after_open[close_idx + THINK_CLOSE.len()..];
                    }
                    None => break,
                }
            }
        }
    }
    out
}

/// Phase 50.2 Plan 07: the reply text actually rendered once a handoff is
/// resolved — think-stripped, matching a Bot Chat row's own convention. A
/// reply that is ENTIRELY a think block still renders a labeled
/// placeholder rather than a blank reply row.
#[allow(dead_code)] // see THINK_OPEN's doc note above (legacy-shell reachability)
fn mention_reply_display_text(raw_reply: &str) -> String {
    let stripped = strip_think_blocks_for_mention_display(raw_reply);
    if stripped.trim().is_empty() && !raw_reply.trim().is_empty() {
        "(no visible reply — response was reasoning only)".to_string()
    } else {
        stripped
    }
}

/// Phase 50.2 Plan 07: the transport-agnostic value each surface stores in
/// its OWN transcript collection — target, delegated message, state, and
/// optional (raw, un-stripped) reply. Each surface owns its own
/// `Vec<MentionHandoffEntry>` (or equivalent field set); this component
/// itself is stateless.
#[allow(dead_code)] // see THINK_OPEN's doc note above (legacy-shell reachability)
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MentionHandoffEntry {
    pub(crate) target: String,
    pub(crate) message: String,
    pub(crate) state: MentionHandoffState,
    pub(crate) reply: Option<String>,
}

/// Phase 50.2 Plan 07 (UI-SPEC Copywriting Contract): the block's head line
/// text per state, exact strings including the arrow and em dash
/// characters — a copy drift fails the build rather than reaching the
/// operator (this module's own test suite pins these verbatim).
#[allow(dead_code)] // see THINK_OPEN's doc note above (legacy-shell reachability)
pub(crate) fn mention_head_text(target: &str, state: &MentionHandoffState) -> String {
    match state {
        MentionHandoffState::Pending => format!("→ @{target}"),
        MentionHandoffState::Resolved => format!("@{target} replied"),
        MentionHandoffState::Failed { .. } => format!("@{target} — no reply"),
    }
}

/// Phase 50.2 Plan 07 (UI-SPEC Copywriting Contract, Mention-handoff
/// failure body): the fixed failure copy rendered in place of a reply —
/// never the raw `BotHandoffError` text.
#[allow(dead_code)] // see THINK_OPEN's doc note above (legacy-shell reachability)
pub(crate) fn mention_failure_body(target: &str) -> String {
    format!("{target} didn't respond. Try again from its Bot Chat.")
}

/// Phase 50.2 Plan 07 (UI-SPEC Component Inventory §2): the shared block.
/// Renders `.kn-mention-handoff` with `data-state` set to
/// `pending`/`resolved`/`failed`, containing `.kn-mention-handoff-head`
/// (the state-driven copy above) and `.kn-mention-handoff-body` (the
/// always-visible quoted delegated message, dimmed; once resolved the
/// reply appends below at full weight; once failed the fixed failure copy
/// appends instead). Never disappears in any state.
#[allow(dead_code)] // see THINK_OPEN's doc note above (legacy-shell reachability)
#[component]
pub fn MentionHandoffBlock(entry: MentionHandoffEntry) -> Element {
    let data_state = match &entry.state {
        MentionHandoffState::Pending => "pending",
        MentionHandoffState::Resolved => "resolved",
        MentionHandoffState::Failed { .. } => "failed",
    };
    let head = mention_head_text(&entry.target, &entry.state);
    let message = entry.message.clone();

    rsx! {
        div { class: "kn-mention-handoff", "data-state": data_state,
            div { class: "kn-mention-handoff-head", "{head}" }
            div { class: "kn-mention-handoff-body",
                // The delegated message: always visible, dimmed — a
                // quote, not the payload.
                div { style: "color: var(--fg-dim);", "{message}" }
                match &entry.state {
                    MentionHandoffState::Resolved => {
                        let reply = mention_reply_display_text(
                            entry.reply.as_deref().unwrap_or_default(),
                        );
                        rsx! {
                            div {
                                style: "color: var(--fg); margin-top: var(--sp-2);",
                                "{reply}"
                            }
                        }
                    }
                    MentionHandoffState::Failed { .. } => {
                        let body = mention_failure_body(&entry.target);
                        rsx! {
                            div {
                                style: "color: var(--fg); margin-top: var(--sp-2);",
                                "{body}"
                            }
                        }
                    }
                    MentionHandoffState::Pending => rsx! {},
                }
            }
        }
    }
}

// Module named `mention_block_tests` (not the crate's usual bare `tests`)
// so every test's fully-qualified name contains the substring
// `mention_block` — this plan's own `<verify>` filter
// (`cargo nextest run ... mention_block`) targets exactly this module,
// matching `group_chat_workspace.rs`'s own `room_transcript_tests` naming
// precedent for the same filter-targeting reason.
#[cfg(test)]
mod mention_block_tests {
    use super::*;

    // -------------------------------------------------------------------
    // mention_head_text — behavior block
    // -------------------------------------------------------------------

    #[test]
    fn mention_head_text_pending_is_arrow_at_bot() {
        assert_eq!(
            mention_head_text("zig", &MentionHandoffState::Pending),
            "→ @zig"
        );
    }

    #[test]
    fn mention_head_text_resolved_is_bot_replied() {
        assert_eq!(
            mention_head_text("zig", &MentionHandoffState::Resolved),
            "@zig replied"
        );
    }

    #[test]
    fn mention_head_text_failed_is_bot_em_dash_no_reply() {
        assert_eq!(
            mention_head_text(
                "zig",
                &MentionHandoffState::Failed { reason: "timeout".to_string() }
            ),
            "@zig — no reply"
        );
    }

    // -------------------------------------------------------------------
    // mention_failure_body — behavior block
    // -------------------------------------------------------------------

    #[test]
    fn mention_failure_body_is_the_locked_copy() {
        assert_eq!(
            mention_failure_body("zig"),
            "zig didn't respond. Try again from its Bot Chat."
        );
    }

    // -------------------------------------------------------------------
    // mention_reply_display_text — think-stripping parity with chat_window.rs
    // -------------------------------------------------------------------

    #[test]
    fn mention_reply_display_text_strips_a_closed_think_block() {
        assert_eq!(
            mention_reply_display_text("<think>internal reasoning</think>the real reply"),
            "the real reply"
        );
    }

    #[test]
    fn mention_reply_display_text_think_only_falls_back_to_placeholder() {
        assert_eq!(
            mention_reply_display_text("<think>only reasoning</think>"),
            "(no visible reply — response was reasoning only)"
        );
    }

    #[test]
    fn mention_reply_display_text_plain_reply_passes_through() {
        assert_eq!(
            mention_reply_display_text("a perfectly ordinary reply"),
            "a perfectly ordinary reply"
        );
    }
}
