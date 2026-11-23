//! Dangerous-command approval gate (D-11 skip-under-bypass).
//!
//! Phase 21.7 Plan 08 (D-14): the full approval queue is deferred to a
//! later phase. This module implements only the bypass decision, so every
//! approval call site has a single constant-time helper to consult before
//! prompting.
//!
//! Phase 57 (D-21): the decision follows a [`DefconLevel`], not the
//! `autonomous.yolo` boolean. Levels 1-3 prompt; levels 4-5 bypass.
//!
//! Usage pattern at a dangerous-command approval site:
//!
//! ```ignore
//! use ironhermes_tools::approval::should_prompt_for_approval;
//!
//! // `level` is the session's live level (or the process default where the
//! // caller has no session id).
//! if should_prompt_for_approval(level) {
//!     // ...show the approval UI / prompt the operator...
//! } else {
//!     // Level 4-5: bypass and proceed with the dangerous command.
//! }
//! ```
//!
//! The CLI banner (printed once per session by `ironhermes_cli::yolo`)
//! is the operator-facing record that bypass is active (T-21.7-08-04).

use ironhermes_core::config::DefconLevel;

/// Returns true if an approval prompt SHOULD be shown for a dangerous
/// command. At DEFCON 4-5 (`DefconLevel::yolo_bypass_active`) it returns
/// false (bypass, D-21); at 1-3 it returns true.
///
/// Budget 100% / fatal error / user interrupt (G-01/G-04/G-09) are
/// enforced upstream and are NOT affected by this gate — bypass has no
/// path to override those hard stops.
pub fn should_prompt_for_approval(level: DefconLevel) -> bool {
    !level.yolo_bypass_active()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(n: u8) -> DefconLevel {
        DefconLevel::try_from(n).expect("1..=5 is a valid DEFCON level")
    }

    /// D-21: levels 1, 2 and 3 prompt; 4 and 5 bypass. Every level is checked so a
    /// boundary shift (say, bypass starting at 3) fails here.
    #[test]
    fn should_prompt_follows_level() {
        for n in 1..=3 {
            assert!(
                should_prompt_for_approval(level(n)),
                "DEFCON {n} must prompt for approval"
            );
        }
        for n in 4..=5 {
            assert!(
                !should_prompt_for_approval(level(n)),
                "DEFCON {n} bypasses the approval prompt (D-21)"
            );
        }
    }
}
