//! `WebApprovalGate` — fail-closed `ironhermes_core::ApprovalGate` for the
//! web/desktop surface (Phase 36.3.12 D-08, GAP 2).
//!
//! # Decision: fail-closed DENY, not a silent absence, not an interactive UX
//!
//! This is a **decided, documented** posture, not a placeholder or a "v1" of an
//! interactive approval flow.
//!
//! - **Why fail-closed instead of silently leaving `TurnRequest.approval_gate`
//!   unset (the prior state):** an absent gate means `AgentLoop`'s NeedsApproval
//!   arm degrades to `ApprovalOutcome::GateUnavailable`, which the trait's own
//!   fail-closed contract already treats as a denial — but "no gate wired" reads,
//!   to a future maintainer, as an oversight rather than a decision. Supplying an
//!   explicit gate that documents *why* it always denies turns an implicit gap
//!   into a visible, intentional control. `request_approval` here is exactly as
//!   safe as `GateUnavailable` (both deny), but it is now impossible to mistake
//!   for "nobody got around to wiring this yet."
//! - **Why not an interactive web approval UX:** the web/desktop surface has no
//!   approval prompt component, no pending-approval awaiter registry, no
//!   WebSocket round-trip for an operator decision, and no timeout policy. Building
//!   one is a genuine feature (comparable in scope to the gateway's chat-native
//!   `/approve` bridge or the CLI's `[o/s/a/d]` prompt) — it is not in scope for a
//!   gap-closure pass. This gate exists so `terminal`/`execute_code` are gated
//!   *today*, on the surface the project actually runs, without waiting on that
//!   larger feature.
//! - **Mirrors this codebase's own precedent:** the gateway's `GatewayApprovalGate`
//!   fail-closed on a non-TTY caller before Phase 45 added the chat-native
//!   `/approve` bridge (see `ironhermes-cli::approval_gate`'s D-12 doc: "Gateway,
//!   pipe, and non-interactive contexts have no operator to respond to the
//!   prompt. Deny immediately."). This gate applies the identical reasoning to
//!   the web/desktop surface, which likewise has no synchronous operator to
//!   answer a prompt today.
//!
//! # Practical consequence
//!
//! - **Tier-2** (`DangerousCommandGuardrail::Block`) commands were, and remain,
//!   unconditionally blocked before this gate is ever consulted — this gate
//!   changes nothing about the Tier-2 floor.
//! - **Tier-1 / `NeedsApproval`** commands, and any `Allow`-classified command
//!   forced through the approval branch by D-08 (remote backend / non-empty
//!   `forward_env`), are now refused on the web/desktop surface — where before
//!   they ran with no guardrail, no approval check, and no audit entry at all.
//!   Denying here is strictly safer than that prior no-checks-at-all behavior.
//! - **`Allow`-classified commands** (the common case) are unaffected — they
//!   still run, and are now audited (Task 1's second half, wired in
//!   `state.rs::run_web_turn`).
//!
//! # Follow-up
//!
//! An interactive web/desktop approval UX (prompt surfaced in the chat UI, an
//! awaiter registry, a resolve path analogous to the gateway's `/approve`) is the
//! tracked follow-up — see Plan 13's recorded open question and
//! `36.3.12-VERIFICATION.md` gap 2. Until it ships, this gate's DENY is the
//! correct, safer default.
//!
//! Cites: D-08 (approval posture / forced-approval-on-remote), VERIFICATION gap 2
//! (the web surface bypass this plan closes).

use async_trait::async_trait;
use ironhermes_core::{ApprovalGate, ApprovalOutcome};

/// Non-interactive, fail-closed `ApprovalGate` for the web/desktop surface.
///
/// Stateless by design (a unit struct) — there is no pending-approval store to
/// hold because `request_approval` never parks the caller; it denies
/// synchronously and immediately, every time.
#[derive(Debug, Clone, Copy, Default)]
pub struct WebApprovalGate;

#[async_trait]
impl ApprovalGate for WebApprovalGate {
    /// Unconditionally denies. Never approves, never blocks awaiting a human
    /// (there is no human to await on this surface today), never panics.
    ///
    /// Emits a `tracing::warn!` naming the session, tool, and reason so an
    /// operator inspecting logs understands *why* a command was refused —
    /// this is a decided control acting, not a silent swallow.
    async fn request_approval(
        &self,
        session_id: &str,
        tool_name: &str,
        reason: &str,
        _args: &serde_json::Value,
    ) -> ApprovalOutcome {
        tracing::warn!(
            target: "ironhermes::security::web_approval_gate",
            session_id = %session_id,
            tool = %tool_name,
            reason = %reason,
            "web/desktop surface has no interactive approval UX yet — denying by \
             fail-closed decision (D-08, VERIFICATION gap 2); see \
             web_approval_gate.rs module docs for rationale and follow-up"
        );
        ApprovalOutcome::Denied
    }

    /// No pending-approval store exists on this gate — `request_approval` never
    /// parks a caller, so there is never anything to resolve out-of-band.
    /// Returns `false` unconditionally (idempotent no-op).
    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate's `request_approval` returns the Denied outcome for a
    /// representative input — it never returns `Approved`, and (per the second
    /// test below) never panics regardless of input shape.
    #[tokio::test]
    async fn request_approval_denies() {
        let gate = WebApprovalGate;
        let outcome = gate
            .request_approval(
                "sess-1",
                "terminal",
                "Tier-1 command requires approval",
                &serde_json::json!({ "command": "curl https://example.com" }),
            )
            .await;
        assert_eq!(
            outcome,
            ApprovalOutcome::Denied,
            "WebApprovalGate must fail closed with Denied, never Approved"
        );
    }

    /// Never returns `Approved` and never panics across a variety of inputs,
    /// including empty strings and an empty/opaque args value (the shape
    /// `execute_code`'s D-11 empty `classify_arg` produces).
    #[tokio::test]
    async fn request_approval_never_approves_across_varied_input() {
        let gate = WebApprovalGate;
        let cases: &[(&str, &str, &str, serde_json::Value)] = &[
            ("", "", "", serde_json::json!({})),
            (
                "sess-2",
                "execute_code",
                "",
                serde_json::json!({ "command": "" }),
            ),
            (
                "sess-3",
                "terminal",
                "remote backend — approval required regardless of guardrail classification (D-08)",
                serde_json::json!({ "command": "ls -la" }),
            ),
        ];
        for (session_id, tool_name, reason, args) in cases {
            let outcome = gate
                .request_approval(session_id, tool_name, reason, args)
                .await;
            assert_ne!(
                outcome,
                ApprovalOutcome::Approved,
                "must never approve for input ({session_id}, {tool_name}, {reason}, {args})"
            );
        }
    }

    /// `resolve` is an idempotent no-op that always reports nothing was pending.
    #[tokio::test]
    async fn resolve_is_idempotent_noop() {
        let gate = WebApprovalGate;
        assert!(!gate.resolve("sess-1", true).await);
        assert!(!gate.resolve("sess-1", false).await);
    }
}
