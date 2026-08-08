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
///
/// # Test-support override (WR-06, Phase 36.3.12 review round 2)
///
/// Under the `test-support` feature only, an integration test may force this
/// function's return value via [`set_force_tty_for_test`]. This exists because
/// `CliApprovalGate::request_approval` (`approval_gate.rs`) hardcodes a call to
/// the production `prompt_for_approval`, which consults `can_prompt(false)` —
/// i.e. this function — BEFORE its session/always-cache checks
/// (`prompt_for_approval_with_reader`'s D-12 non-TTY check runs first). A
/// `cargo test` process's stdin is never a real TTY, so without this override
/// that D-12 check would unconditionally short-circuit to `Deny` on every
/// dispatch regardless of the approval store's state — making a regression in
/// the session-tier's cross-dispatch persistence (WR-01) unobservable through
/// `build_gated_terminal_intercept`'s real closure in any headless test
/// process. The override is compiled out entirely — and therefore cannot
/// affect ANY behavior, including under `--features test-support` unless a
/// test explicitly calls `set_force_tty_for_test` — outside of a
/// `test-support` build; production builds never enable this feature.
pub fn is_terminal_stdin() -> bool {
    #[cfg(feature = "test-support")]
    {
        if let Some(forced) = force_tty_override() {
            return forced;
        }
    }
    std::io::stdin().is_terminal()
}

/// Process-global override for [`is_terminal_stdin`], `test-support`-gated.
///
/// A process-global `AtomicI8` (not a thread-local) because
/// `CliApprovalGate::request_approval` bridges its non-`Send` prompt future
/// onto a `tokio::task::spawn_blocking` worker thread (see `approval_gate.rs`'s
/// "Send bridging" doc) — a thread-local set on the test's own task would not
/// be visible from that worker thread. `-1` = no override (fall through to the
/// real `is_terminal()` check); `0` = force false; `1` = force true.
#[cfg(feature = "test-support")]
static FORCE_TTY_FOR_TEST: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);

#[cfg(feature = "test-support")]
fn force_tty_override() -> Option<bool> {
    match FORCE_TTY_FOR_TEST.load(std::sync::atomic::Ordering::SeqCst) {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

/// Test-only: force [`is_terminal_stdin`]'s return value for the remainder of
/// the process, or clear the override with `None`. `test-support`-gated —
/// unavailable (does not exist as a symbol) in any build that does not enable
/// the `test-support` feature, so it cannot leak into production behavior.
///
/// Callers MUST reset this to `None` when done (e.g. via an RAII guard) so the
/// override does not leak into other tests sharing the same test binary
/// process.
#[cfg(feature = "test-support")]
pub fn set_force_tty_for_test(value: Option<bool>) {
    let v = match value {
        None => -1,
        Some(false) => 0,
        Some(true) => 1,
    };
    FORCE_TTY_FOR_TEST.store(v, std::sync::atomic::Ordering::SeqCst);
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
