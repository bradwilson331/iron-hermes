//! Dangerous-command approval gate (D-11 skip-under-yolo).
//!
//! Phase 21.7 Plan 08 (D-14): the full approval queue is deferred to a
//! later phase. This module implements only the skip-under-yolo bypass,
//! so every future approval call site has a single constant-time helper
//! to consult before prompting.
//!
//! Usage pattern at a future dangerous-command approval site:
//!
//! ```ignore
//! use ironhermes_tools::approval::should_prompt_for_approval;
//!
//! if should_prompt_for_approval(config.autonomous.yolo) {
//!     // ...show the approval UI / prompt the operator...
//! } else {
//!     // Under yolo: bypass and proceed with the dangerous command.
//! }
//! ```
//!
//! The CLI banner (printed once per session by `ironhermes_cli::yolo`)
//! is the operator-facing record that yolo is active (T-21.7-08-04).

/// Returns true if an approval prompt SHOULD be shown for a dangerous
/// command. Under `--yolo` / `autonomous.yolo = true`, returns false
/// (blanket bypass — D-11).
///
/// Budget 100% / fatal error / user interrupt (G-01/G-04/G-09) are
/// enforced upstream and are NOT affected by this gate — yolo has no
/// path to override those hard stops.
pub fn should_prompt_for_approval(config_yolo: bool) -> bool {
    !config_yolo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_yolo_prompts() {
        assert!(
            should_prompt_for_approval(false),
            "With yolo disabled, the approval prompt MUST fire"
        );
    }

    #[test]
    fn yolo_bypasses() {
        assert!(
            !should_prompt_for_approval(true),
            "With yolo enabled, the approval prompt MUST be bypassed (D-11)"
        );
    }
}
