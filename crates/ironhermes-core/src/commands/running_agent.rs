//! Phase 36.1 (D-01): extracted from ironhermes-gateway/src/handler.rs:62-103 —
//! single canonical impl for gateway, web UI (Plan 02), and TUI (Plan 03).
//! SeqCst is mandatory on both store sites per RESEARCH.md Pitfall 3 — do NOT downgrade.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// D-02 (Phase 36, locked verbatim): rejection message when a slash command or
/// free-text message arrives while an agent turn is already in flight.
/// All three surfaces (gateway, web UI, TUI) reference this single constant — never inline.
pub const AGENT_RUNNING_REJECT_MSG: &str =
    "Agent is running. Use /stop to interrupt or /queue to send after this turn.";

/// Phase 36 (D-06): RAII guard that atomically sets the per-session running flag
/// to `true` on construction and `false` on `Drop`.
///
/// Drop fires on every exit path — `Ok(...)` return, `Err(...)` propagated via `?`,
/// panic, and future cancellation — covering the "forgot a cleanup branch" bug class.
///
/// Use `Ordering::SeqCst` on both store sites (new + Drop) to guarantee visibility
/// across threads on weakly-ordered architectures (Pitfall 3 mitigation).
pub struct RunningAgentGuard(Arc<AtomicBool>);

impl RunningAgentGuard {
    pub fn new(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self(flag)
    }
}

impl Drop for RunningAgentGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Phase 36 (D-01): returns true for command names that are allowed to dispatch
/// even when an agent turn is in flight. Checks the POST-resolution canonical
/// name (`def.name`), never the raw user input, so `/reset` → `"new"` correctly
/// bypasses (Pitfall 4 mitigation).
///
/// Locked bypass list (D-01): stop, new, status, queue.
/// "approve" and "deny" are intentionally excluded until the approval queue lands.
pub fn is_bypass(name: &str) -> bool {
    // TODO(D-01): add "approve" | "deny" when approval queue lands
    matches!(name, "stop" | "new" | "status" | "queue")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::AssertUnwindSafe;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Test 1: The bypass list contains exactly the four D-01 commands and is case-sensitive.
    #[test]
    fn is_bypass_locked_list() {
        // Members — all must return true
        assert!(is_bypass("stop"), "stop must bypass");
        assert!(is_bypass("new"), "new must bypass");
        assert!(is_bypass("status"), "status must bypass");
        assert!(is_bypass("queue"), "queue must bypass");

        // Non-members — all must return false
        assert!(!is_bypass("model"), "model must not bypass");
        assert!(!is_bypass("retry"), "retry must not bypass");
        assert!(!is_bypass("approve"), "approve must not bypass");
        assert!(!is_bypass("deny"), "deny must not bypass");
        assert!(!is_bypass(""), "empty string must not bypass");
        assert!(!is_bypass("STOP"), "STOP (uppercase) must not bypass — case-sensitive (D-01)");
    }

    /// Test 2: The flag is set to true when the guard is constructed.
    #[test]
    fn guard_sets_flag_on_construction() {
        let flag = Arc::new(AtomicBool::new(false));
        let _guard = RunningAgentGuard::new(flag.clone());
        assert!(
            flag.load(Ordering::SeqCst),
            "Flag must be true while guard is in scope"
        );
    }

    /// Test 3: The flag is cleared to false when the guard is dropped.
    #[test]
    fn guard_clears_flag_on_drop() {
        let flag = Arc::new(AtomicBool::new(false));

        {
            let _guard = RunningAgentGuard::new(flag.clone());
            assert!(flag.load(Ordering::SeqCst), "Flag must be true inside guard scope");
            // Guard drops here
        }

        assert!(
            !flag.load(Ordering::SeqCst),
            "Flag must be false after guard is dropped (D-06)"
        );
    }

    /// Test 4: The flag is cleared even when the scope exits via panic.
    ///
    /// Proves that Drop fires on all exit paths, including panic (D-06 RAII discipline).
    #[test]
    fn guard_clears_flag_on_panic() {
        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _g = RunningAgentGuard::new(flag_clone.clone());
            panic!("simulated");
        }));

        assert!(result.is_err(), "catch_unwind must catch the panic");
        assert!(
            !flag.load(Ordering::SeqCst),
            "Flag must be false after panic — Drop must fire on panic exit (D-06)"
        );
    }

    /// Test 5: The D-02 constant is byte-for-byte identical to the locked contract string.
    #[test]
    fn reject_msg_constant_value() {
        assert_eq!(
            AGENT_RUNNING_REJECT_MSG,
            "Agent is running. Use /stop to interrupt or /queue to send after this turn.",
            "AGENT_RUNNING_REJECT_MSG must match the D-02 locked string byte-for-byte"
        );
    }

    /// Test 6: `approve` and `deny` are NOT in the bypass list.
    ///
    /// The TODO comment inside `is_bypass` is informational only. The bypass list stays
    /// narrow until the approval queue lands. This test locks the non-membership.
    #[test]
    fn is_bypass_keeps_todo_for_approve_deny() {
        assert!(
            !is_bypass("approve"),
            "approve must NOT bypass — tracked by TODO in is_bypass, not yet implemented"
        );
        assert!(
            !is_bypass("deny"),
            "deny must NOT bypass — tracked by TODO in is_bypass, not yet implemented"
        );
    }
}
