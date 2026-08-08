//! Approval gate trait — async cross-crate seam for the gateway approval engine.
//!
//! Lives in `ironhermes-core` so `AgentLoop` (in `ironhermes-agent`, which depends on
//! core) can hold `Option<Arc<dyn ApprovalGate>>` while the gateway provides the
//! implementation, preserving the agent→core direction without introducing a cycle.
//!
//! # Phase 45 decision anchors
//!
//! - D-02: one active pending per session → [`ApprovalOutcome::AlreadyPending`]
//! - D-07: pending approvals are in-memory only; no disk persistence seam here
//! - Fail-closed: anything other than `Approved` is treated as a denial

use async_trait::async_trait;

/// The final decision returned by an approval gate after a `request_approval` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// The operator approved the action in time.
    Approved,
    /// The operator explicitly denied the action.
    Denied,
    /// The configured timeout elapsed before a decision was made (D-04/D-05).
    TimedOut,
    /// Another approval is already pending for this session (D-02).
    AlreadyPending,
    /// No gate is configured for this turn — treated as denied (fail-closed default).
    GateUnavailable,
}

/// Async trait for approval gate implementations.
///
/// The gateway implements this via `GatewayApprovalGate` (phase 45 plan 03); the agent
/// loop holds it as `Option<Arc<dyn ApprovalGate>>` so it can operate without a gate
/// in non-gateway contexts (e.g. CLI, test harness).
///
/// **Fail-closed rule:** callers MUST treat any outcome other than `Approved` as a
/// denial — never proceed with a destructive action on `GateUnavailable`.
#[async_trait]
pub trait ApprovalGate: Send + Sync {
    /// Park the caller until the operator approves/denies or the configured timeout elapses.
    ///
    /// Implementations must:
    /// - Return `AlreadyPending` immediately if this session already has a pending request (D-02).
    /// - Return `TimedOut` when `approvals.timeout_secs` elapses without a decision (D-04).
    /// - Notify the chat on expiry (D-05) before returning `TimedOut`.
    ///
    /// The `reason` string is surfaced to the operator in the approval prompt.
    ///
    /// `args` (Phase 46 D-03) is the raw tool-call/command arguments, threaded through
    /// so an audit-log implementation can record redacted, truncated arguments
    /// alongside the resolution. Implementations that do not audit may ignore it.
    async fn request_approval(
        &self,
        session_id: &str,
        tool_name: &str,
        reason: &str,
        args: &serde_json::Value,
    ) -> ApprovalOutcome;

    /// Resolve a pending approval for the given session.
    ///
    /// - `approved: true`  → unparks the waiting task with `ApprovalOutcome::Approved`.
    /// - `approved: false` → unparks the waiting task with `ApprovalOutcome::Denied`.
    ///
    /// Returns `false` if no pending approval exists for `session_id` (idempotent: safe
    /// to call on `/deny` when there is no pending request).
    async fn resolve(&self, session_id: &str, approved: bool) -> bool;
}
