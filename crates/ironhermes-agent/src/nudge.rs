//! Phase 32 Plan 01 (LEARN-01 / LEARN-02): periodic memory nudge.
//!
//! At every `memory.nudge_interval` user turns, the REPL or gateway loop calls
//! [`spawn_nudge_review`] with a clone of the current message history. The
//! function builds a *narrow* [`ToolRegistry`] containing only [`MemoryTool`]
//! (no `session_search`, `web_read`, `execute_code`, browser, or skill tools)
//! and runs an internal [`AgentLoop`] with the [`MEMORY_REVIEW_PROMPT`]
//! appended as a user message. The internal agent decides per-item which
//! memory layer fits (LEARN-02 two-tier judgment) and calls `memory_add`,
//! `memory_replace`, or `memory_remove` as appropriate. Writes go through
//! the standard `MemoryManager` path — they fan out to the optional mirror
//! provider and respect the 3,575 char cap (2,200 MEMORY.md + 1,375 USER.md).
//!
//! The frozen-snapshot invariant (PRMT-06 / MEM-06) is honored automatically:
//! the active session's cached system prompt is not reloaded mid-session.
//! New entries written here take effect at the next session start.
//!
//! ## Threat model
//!
//! - T-32-01 (Tampering): the registry intentionally excludes `session_search`,
//!   `web_read`, `execute_code`, browser tools, and skills — the nudge agent
//!   cannot exfiltrate data or run code.
//! - T-32-04 (Prompt injection via memory review): writes flow through
//!   `MemoryManager::handle_tool_call` -> `MemoryStore::scan_content` (Phase
//!   17 security scanner); no bypass path.
//! - T-32-05 (Availability): callers must `tokio::spawn` the future so the
//!   REPL / gateway loop is never blocked on nudge completion. This module
//!   does not spawn internally — the spawn boundary stays at the call site so
//!   tests can `.await` the function directly.

use std::sync::Arc;

use ironhermes_core::{ChatMessage, Config};
use ironhermes_tools::ToolRegistry;
use tokio::sync::{Mutex, RwLock};

use crate::agent_loop::AgentLoop;
use crate::any_client::AnyClient;
use crate::memory::MemoryManager;

/// Internal review prompt fed to the nudge AgentLoop after the conversation
/// snapshot. Encodes the LEARN-02 two-tier judgment: persistent memory
/// (MEMORY.md / USER.md) vs. session-search archive.
///
/// Source: Python `run_agent.py` `AIAgent._MEMORY_REVIEW_PROMPT`
/// (lines 3984-3996), adapted verbatim.
pub const MEMORY_REVIEW_PROMPT: &str =
    "Review the conversation above and consider saving to memory if appropriate.\n\n\
     Focus on:\n\
     1. Has the user revealed things about themselves — their persona, desires, \
     preferences, or personal details worth remembering?\n\
     2. Has the user expressed expectations about how you should behave, their work \
     style, or ways they want you to operate?\n\n\
     Decide per-item which memory layer fits:\n\
     - \"Important enough to be present in every future conversation\" → use the \
     memory tool (persists to MEMORY.md/USER.md, present in every session).\n\
     - \"Useful only when topic comes up\" → leave in session history (searchable \
     via session_search when needed). Do NOT force these into prompt memory.\n\n\
     The total memory cap is 3,575 chars (2,200 MEMORY.md + 1,375 USER.md). \
     Be selective — only persist what genuinely improves every future conversation.\n\n\
     If nothing is worth saving, just say 'Nothing to save.' and stop.";

/// Run the periodic memory-review nudge.
///
/// The caller is responsible for `tokio::spawn`-ing this future so the REPL /
/// gateway loop is not blocked on nudge completion (T-32-05 mitigation). The
/// function consumes a clone of the active message history and the shared
/// `MemoryManager` handle; it does NOT mutate the caller's `messages` vec.
///
/// Errors from the internal `AgentLoop::run` are caught and logged via
/// `tracing::warn!` — nudge failures must never abort the user session.
///
/// ## Tool surface
///
/// The internal [`ToolRegistry`] is built fresh with **only** [`MemoryTool`]
/// registered. `session_search`, `web_read`, `execute_code`, browser_*, and
/// skill tools are deliberately excluded (T-32-01 mitigation). The single
/// registration call in this function is asserted by the plan's
/// acceptance grep — see the in-body SECURITY comment.
///
/// ## Iteration cap
///
/// `max_iterations = 8` mirrors the Python `_spawn_background_review` cap —
/// the review agent should converge in 1-3 turns; the 8 ceiling is a safety
/// net for pathological loops, not a target.
pub async fn spawn_nudge_review(
    messages_snapshot: Vec<ChatMessage>,
    memory_manager: Arc<Mutex<MemoryManager>>,
    client: AnyClient,
    _config: &Config,
) {
    // Build a narrow registry: MemoryTool only.
    //
    // SECURITY (T-32-01): no `session_search`, `web_read`, `execute_code`,
    // browser_*, or skill tools may be registered here. The single
    // registration call below is the only one in this module — the strict
    // acceptance grep counts the open-paren form, so comments deliberately
    // avoid that form to keep the count at exactly 1.
    let mut nudge_registry = ToolRegistry::new();
    let shared: ironhermes_tools::memory_tool::SharedMemoryManager = memory_manager.clone();
    nudge_registry.register(Box::new(ironhermes_tools::memory_tool::MemoryTool::new(
        shared,
    )));
    let nudge_registry = Arc::new(RwLock::new(nudge_registry));

    // Build the internal AgentLoop. max_iterations=8 mirrors the Python
    // `_spawn_background_review` cap — convergence is typically 1-3 turns.
    let mut agent = AgentLoop::new(client, nudge_registry, 8).with_memory_manager(memory_manager);

    // Append the review prompt as a final user message. We clone the snapshot
    // first so the caller's vec is never mutated (defense in depth — the
    // signature already takes Vec<ChatMessage> by value, but readers should
    // not have to reason about move semantics to verify isolation).
    let mut augmented = messages_snapshot;
    augmented.push(ChatMessage::user(MEMORY_REVIEW_PROMPT));

    match agent.run(augmented).await {
        Ok(_result) => {
            tracing::info!("nudge: memory review complete");
        }
        Err(e) => {
            // Phase 32 LEARN-01: nudge failures must never abort the user
            // session. Log and swallow — the next nudge will retry.
            tracing::warn!("nudge: review agent error: {:#}", e);
        }
    }
}

/// Returns true when the nudge should fire. Increments `*counter`; resets to 0 on fire.
///
/// Returns `false` immediately when `interval == 0` (disabled — documented sentinel
/// across run_chat + gateway callers).
///
/// Extracted from the inline post-turn fire logic so the counter behavior can be
/// unit-tested independently of `AgentLoop`, `MemoryManager`, or the surrounding
/// REPL / gateway machinery. Both call sites (CLI `run_chat`, gateway
/// `run_agent`) still inline the same logic — this helper is the canonical
/// reference for the counter contract.
///
/// TDD RED stub — Plan 32-02 Task 1. Real body lands in the GREEN commit.
pub(crate) fn should_nudge(_interval: u32, _counter: &mut u32) -> bool {
    // Intentionally wrong: lets RED tests fail at runtime before the GREEN fix.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEARN-02: prompt must encode the two-tier persistence judgment.
    #[test]
    fn prompt_contains_tier_guidance() {
        assert!(
            MEMORY_REVIEW_PROMPT.contains("every future conversation"),
            "MEMORY_REVIEW_PROMPT must mention 'every future conversation' \
             (persistent tier framing)"
        );
        assert!(
            MEMORY_REVIEW_PROMPT.contains("session_search"),
            "MEMORY_REVIEW_PROMPT must mention 'session_search' \
             (session-archive tier framing)"
        );
    }

    // LEARN-02: agent must be reminded of the 3,575 char total cap so it
    // stays selective. The MemoryStore enforces the cap structurally, but
    // surfacing it in the prompt keeps choices in scope.
    #[test]
    fn prompt_contains_cap_info() {
        assert!(
            MEMORY_REVIEW_PROMPT.contains("3,575"),
            "MEMORY_REVIEW_PROMPT must surface the 3,575 char total cap"
        );
    }

    // LEARN-01: explicit short-circuit phrase the agent emits when nothing
    // is worth saving — keeps nudge turns cheap.
    #[test]
    fn prompt_contains_nothing_to_save_signal() {
        assert!(
            MEMORY_REVIEW_PROMPT.contains("Nothing to save"),
            "MEMORY_REVIEW_PROMPT must include the 'Nothing to save.' \
             short-circuit signal"
        );
    }

    // LEARN-01 (Plan 32-02 Task 1): counter-logic invariants for the
    // turn-based nudge fire predicate. These tests live in the agent crate so
    // both CLI run_chat and gateway run_agent callers can rely on a single
    // canonical contract.

    /// interval=3: turns 1/2 return false; turn 3 fires (true) and resets the
    /// counter to 0; turn 1 of the next cycle is back to false.
    #[test]
    fn fires_at_interval() {
        let mut c = 0u32;
        assert!(!should_nudge(3, &mut c)); // turn 1: c=1
        assert!(!should_nudge(3, &mut c)); // turn 2: c=2
        assert!(should_nudge(3, &mut c)); // turn 3: fires, c=0
        assert_eq!(c, 0, "counter must reset to 0 after firing");
        assert!(!should_nudge(3, &mut c)); // turn 1 of next cycle
    }

    /// interval=0 is the documented disable sentinel: should_nudge must
    /// return false on every call AND leave the counter at 0 (no side effect).
    #[test]
    fn disabled_when_zero() {
        let mut c = 0u32;
        for _ in 0..20 {
            assert!(
                !should_nudge(0, &mut c),
                "should_nudge must return false when interval==0"
            );
        }
        assert_eq!(c, 0, "counter must stay at 0 when interval==0 (no side effect)");
    }

    /// interval=2 fires at turn 2, resets, and fires again at turn 2 of the
    /// next cycle — proving the reset is reusable, not a one-shot.
    #[test]
    fn counter_resets_after_fire() {
        let mut c = 0u32;
        assert!(!should_nudge(2, &mut c)); // turn 1: c=1
        assert!(should_nudge(2, &mut c)); // turn 2: fires, c=0
        assert_eq!(c, 0);
        assert!(!should_nudge(2, &mut c)); // turn 1 of cycle 2: c=1
        assert!(should_nudge(2, &mut c)); // turn 2: fires again, c=0
        assert_eq!(c, 0);
    }
}
