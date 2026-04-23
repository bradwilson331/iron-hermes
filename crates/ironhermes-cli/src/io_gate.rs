//! Non-TTY stdin gate (G-08 / Pitfall 10).
//!
//! Single source of truth for "can I safely read stdin on an
//! agent-reachable code path?" Every stdin read along any path that the
//! agent can reach must consult `is_terminal_stdin()` first so gateway
//! and `hermes -e` (non-interactive batch) runs never deadlock waiting
//! on a TTY that will never exist.
//!
//! The gate is proactive — today's CLI does not have stdin reads on
//! agent paths, but gateway/autonomous modes must stay safe as callers
//! add new prompts. `can_prompt(config_yolo)` composes the yolo bypass
//! with the TTY check so callers get both policies in one branch.

use std::io::IsTerminal;

/// Returns true if the process's stdin is connected to an interactive
/// terminal. Never allocates, never reads, never blocks.
pub fn is_terminal_stdin() -> bool {
    std::io::stdin().is_terminal()
}

/// Composed gate: if yolo is enabled OR stdin is not a TTY, do NOT
/// prompt. This is the canonical check that dangerous-command approval
/// sites and any interactive-only helper should use.
pub fn can_prompt(config_yolo: bool) -> bool {
    !config_yolo && is_terminal_stdin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yolo_disables_prompting_regardless_of_tty() {
        // can_prompt(true) must be false — yolo always suppresses prompts
        // even when stdin is a TTY.
        assert!(!can_prompt(true));
    }

    #[test]
    fn is_terminal_stdin_returns_bool_without_panic() {
        // Under `cargo test` stdin is typically not a TTY, but the
        // important contract is that this returns a bool without side
        // effects. We don't assert the value — it differs by harness.
        let _ = is_terminal_stdin();
    }
}
