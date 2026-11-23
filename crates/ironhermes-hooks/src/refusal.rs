//! Model-facing refusal text for every approval refusal and policy Block (Phase 57 G-1, D-07).
//!
//! The text is built where the outcome is decided, so every surface (gateway, web, CLI,
//! ACP) relays the same words. Each refusal says who refused (the operator or security
//! policy), that the call did not run, and carries [`NO_CIRCUMVENTION_NOTICE`]: the model
//! must not retry the action or reach the same result another way.
//!
//! Wording rules: plain ASCII, no double quotes and no backslashes, so the text survives
//! JSON encoding byte-for-byte and a request-body assertion can find it verbatim.
//!
//! Only the model-facing text lives here. Audit rows keep their own `decision`,
//! `resolution` and `reason` fields; nothing in this module touches them.

use ironhermes_core::ApprovalOutcome;

/// Appended to every refusal. Fixed wording; pinned by tests.
pub const NO_CIRCUMVENTION_NOTICE: &str = "Do not retry this action and do not try to get the same result another way, such as a different tool, a rewritten command, code, or a web fetch. Tell the user it was not allowed and wait for their instructions.";

/// Trailing periods and whitespace come off a reason before it is parenthesized, so the
/// sentence around it keeps one full stop.
fn tidy_reason(reason: &str) -> &str {
    reason.trim_end_matches(|c: char| c == '.' || c.is_whitespace())
}

/// Text for an approval that did not end in `Approved`. `None` for `Approved`.
///
/// Only the operator's own denial says the operator refused; a timeout, a competing
/// approval and a missing channel each say what actually happened.
pub fn approval_refusal_text(tool_name: &str, outcome: &ApprovalOutcome) -> Option<String> {
    let lead = match outcome {
        ApprovalOutcome::Approved => return None,
        ApprovalOutcome::Denied => {
            format!("The operator denied this {tool_name} call, so it did not run.")
        }
        ApprovalOutcome::TimedOut => format!(
            "This {tool_name} call needed the operator's approval and the request timed out without an answer, so it was denied and did not run."
        ),
        ApprovalOutcome::AlreadyPending => format!(
            "Another approval is already waiting for the operator in this session, so this {tool_name} call was refused and did not run."
        ),
        ApprovalOutcome::GateUnavailable => format!(
            "This {tool_name} call needs the operator's approval and no approval channel is available here, so security policy refused it and it did not run."
        ),
    };
    Some(format!("{lead} {NO_CIRCUMVENTION_NOTICE}"))
}

/// Text for a call that needs approval when no approval channel exists at all.
pub fn no_approval_channel_text(tool_name: &str, reason: &str) -> String {
    format!(
        "This {tool_name} call needs the operator's approval ({}) and no approval channel is available here, so security policy refused it and it did not run. {NO_CIRCUMVENTION_NOTICE}",
        tidy_reason(reason)
    )
}

/// Text for a hard policy Block (Tier-2, a Critical rule, a reach block, a sidecar Block).
pub fn policy_block_text(tool_name: &str, reason: &str) -> String {
    format!(
        "Security policy blocked this {tool_name} call ({}), so it did not run. {NO_CIRCUMVENTION_NOTICE}",
        tidy_reason(reason)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTICE: &str = "Do not retry this action and do not try to get the same result another way, such as a different tool, a rewritten command, code, or a web fetch. Tell the user it was not allowed and wait for their instructions.";

    #[test]
    fn refusal_text_is_pinned() {
        assert_eq!(NO_CIRCUMVENTION_NOTICE, NOTICE);
        assert_eq!(
            approval_refusal_text("terminal", &ApprovalOutcome::Denied).unwrap(),
            format!("The operator denied this terminal call, so it did not run. {NOTICE}")
        );
        assert_eq!(
            approval_refusal_text("terminal", &ApprovalOutcome::TimedOut).unwrap(),
            format!(
                "This terminal call needed the operator's approval and the request timed out without an answer, so it was denied and did not run. {NOTICE}"
            )
        );
        assert_eq!(
            approval_refusal_text("terminal", &ApprovalOutcome::AlreadyPending).unwrap(),
            format!(
                "Another approval is already waiting for the operator in this session, so this terminal call was refused and did not run. {NOTICE}"
            )
        );
        assert_eq!(
            approval_refusal_text("terminal", &ApprovalOutcome::GateUnavailable).unwrap(),
            format!(
                "This terminal call needs the operator's approval and no approval channel is available here, so security policy refused it and it did not run. {NOTICE}"
            )
        );
        assert_eq!(
            approval_refusal_text("terminal", &ApprovalOutcome::Approved),
            None
        );
        assert_eq!(
            no_approval_channel_text("terminal", "network tool"),
            format!(
                "This terminal call needs the operator's approval (network tool) and no approval channel is available here, so security policy refused it and it did not run. {NOTICE}"
            )
        );
        assert_eq!(
            policy_block_text("terminal", "rm -rf on /."),
            format!(
                "Security policy blocked this terminal call (rm -rf on /), so it did not run. {NOTICE}"
            )
        );
    }

    #[test]
    fn every_refusal_ends_with_notice() {
        let mut texts = vec![
            no_approval_channel_text("terminal", "network tool"),
            policy_block_text("execute_code", "reach block  "),
        ];
        for o in [
            ApprovalOutcome::Denied,
            ApprovalOutcome::TimedOut,
            ApprovalOutcome::AlreadyPending,
            ApprovalOutcome::GateUnavailable,
        ] {
            texts.push(approval_refusal_text("terminal", &o).expect("refusal for non-approved"));
        }
        for t in texts {
            assert!(t.ends_with(NO_CIRCUMVENTION_NOTICE), "{t}");
            assert!(!t.contains('"'), "no double quote: {t}");
            assert!(!t.contains('\\'), "no backslash: {t}");
        }
    }
}
