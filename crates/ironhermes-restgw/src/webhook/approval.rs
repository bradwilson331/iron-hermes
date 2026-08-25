//! O-02: fail-closed, immediate-deny approval gate for webhook-originated
//! turns.
//!
//! D-12 makes webhook lanes autonomous — the sender already received its
//! 202 and disconnected, and there is no chat surface to prompt. Routing a
//! gated tool operation through `ironhermes_gateway::approval::ApprovalCoordinator`
//! would send an approval prompt to nobody, wait its full configured
//! timeout (120s by default), and then deny anyway — holding a concurrency
//! slot the whole time for a foregone conclusion.
//! [`ImmediateDenyApprovalGate`] reaches the exact same closed door
//! synchronously: no wait, no coordinator, denied by construction. It is
//! fail closed in both directions — denial is the outcome whether the wait
//! happens or not.
//!
//! **The failure mode this must avoid (research Pitfall 4):** a route that
//! silently denies every gated operation looks exactly like an agent that
//! simply chose not to attempt it. An operator debugging "why does my route
//! never do X" has no signal at all. The mitigation is two-fold and BOTH
//! halves are required: every denial logs at `warn!` with the tool name and
//! reason (mirroring `ApprovalCoordinator`'s own audit discipline), AND
//! [`format_denial`] produces the text a webhook route's delivered output
//! should carry so the operator reads "approval automatically denied for
//! this tool" rather than a plausible-looking answer that quietly omitted
//! the action.
//!
//! **Scope:** this gate is constructed and used only on the webhook
//! turn-construction path — it is not a global default and must never
//! become one. The REST API surface (Plan 06) has a real client that can
//! answer and wires to the coordinator's resolve path instead.
//!
//! **Residual integration note (documented, not silently claimed as done):**
//! `ApprovalGate::request_approval` is consulted by `AgentLoop`
//! (`ironhermes-agent::agent_loop`) via `self.approval_gate:
//! Option<Arc<dyn ApprovalGate>>`, which is threaded in per-turn by
//! `ironhermes-gateway::handler.rs` (`GatewayMessageHandler`'s per-turn
//! `approval_gate_for_turn` construction, currently derived only from
//! `self.approval_coordinator` — `None` for both the webhook and REST API
//! server handlers, since neither calls `set_approval_coordinator`). Wiring
//! `ImmediateDenyApprovalGate` into that per-turn construction for a
//! `Platform::Webhook` event requires a change to `ironhermes-gateway`,
//! which is outside this crate and outside this plan's declared
//! `files_modified`. Until that follow-up lands, a webhook turn's
//! `NeedsApproval` outcome still fails closed today (`ApprovalOutcome::GateUnavailable`,
//! the trait's own documented default when no gate is configured) but
//! without this module's loud `warn!`/tool-naming or the delivered-output
//! surfacing O-02 requires — see this plan's SUMMARY.md for the full
//! rationale and the precedented fix shape (mirrors the existing Buzz
//! channel-reply `StreamConsumer::with_reply_to` wiring).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::approval_gate::{ApprovalGate, ApprovalOutcome};

/// Bound on a sanitized tool name or reason, in `char`s. Matches the
/// treatment `ApprovalCoordinator` already applies to interpolated text
/// (`crates/ironhermes-gateway/src/approval.rs:244-263`) and
/// `webhook::template`'s own interpolation sanitizer.
const MAX_SANITIZED_CHARS: usize = 200;

/// Strip newlines/carriage-returns and truncate — the injection-mitigation
/// treatment applied to every attacker-influenced string that reaches
/// logged or delivered output in this crate (mirrors
/// `ApprovalCoordinator`'s `safe_tool`/`safe_reason` construction and
/// `webhook::template::sanitize`).
fn sanitize(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
        .chars()
        .take(MAX_SANITIZED_CHARS)
        .collect()
}

/// Build the sanitized denial text a webhook route's delivered output
/// should carry when [`ImmediateDenyApprovalGate`] denies a gated
/// operation. Exposed as a standalone `pub fn` (rather than folded silently
/// into `request_approval`, which the `ApprovalGate` trait constrains to
/// return only an [`ApprovalOutcome`] enum with no message payload) so a
/// caller building the turn's final delivered text — and this crate's own
/// tests — can produce the exact same string `request_approval` logs.
pub fn format_denial(tool_name: &str, reason: &str) -> String {
    let safe_tool = sanitize(tool_name);
    let safe_reason = sanitize(reason);
    format!(
        "\u{26a0}\u{fe0f} Approval automatically denied for tool `{safe_tool}` \
         (autonomous webhook lane — no human is attached to approve or deny): {safe_reason}"
    )
}

/// Shared, per-turn record of the denial texts this gate produced.
///
/// `ApprovalGate::request_approval` may only return an [`ApprovalOutcome`] —
/// the trait carries no message payload — and what `AgentLoop` does with a
/// refusal is feed a terse string back to the model as a tool result. So the
/// operator-facing text has to travel out of band, and this is the channel:
/// the caller constructing the gate holds a handle, the gate appends to it,
/// and the caller drains it once the turn ends and appends the contents to the
/// delivered output (T-36.7.1-37's second half — without it the operator reads
/// only the model's paraphrase of a refusal it may not mention at all).
pub type DenialLog = Arc<Mutex<Vec<String>>>;

/// Create an empty [`DenialLog`].
pub fn new_denial_log() -> DenialLog {
    Arc::new(Mutex::new(Vec::new()))
}

/// Fail-closed, synchronous-deny [`ApprovalGate`] for autonomous webhook
/// turns. See this module's doc comment for the full O-02 rationale.
#[derive(Debug, Clone, Default)]
pub struct ImmediateDenyApprovalGate {
    /// `None` keeps the log-only behaviour; `Some` additionally records each
    /// denial's operator-facing text for the caller to deliver.
    denials: Option<DenialLog>,
}

impl ImmediateDenyApprovalGate {
    /// Construct a gate that only logs. Every call to `request_approval`
    /// denies independently; there is nothing to share across turns.
    pub fn new() -> Self {
        Self { denials: None }
    }

    /// Construct a gate that also records each denial's text into `log`, so
    /// the caller can append it to the turn's delivered output.
    pub fn with_denial_log(log: DenialLog) -> Self {
        Self {
            denials: Some(log),
        }
    }
}

#[async_trait]
impl ApprovalGate for ImmediateDenyApprovalGate {
    /// Denies immediately — no `tokio::time::timeout`, no
    /// `ApprovalCoordinator` call, no wait of any kind. Every firing logs at
    /// `warn!` with the sanitized tool name and reason (T-36.7.1-37 —
    /// Research Pitfall 4: a silent auto-denial is indistinguishable from
    /// an agent that simply did not attempt the action).
    async fn request_approval(
        &self,
        _session_id: &str,
        tool_name: &str,
        reason: &str,
        _args: &serde_json::Value,
    ) -> ApprovalOutcome {
        let safe_tool = sanitize(tool_name);
        let safe_reason = sanitize(reason);
        tracing::warn!(
            tool = %safe_tool,
            reason = %safe_reason,
            "webhook turn: approval automatically denied (O-02 immediate-deny gate — \
             autonomous lane, no human attached to approve or deny)"
        );
        // T-36.7.1-37's second half: record the operator-facing text so the
        // caller can append it to this turn's delivered output. Both signals
        // are required — a log line alone is invisible to whoever is reading
        // the route's response, and a delivered line alone is invisible to
        // whoever is reading the process logs.
        if let Some(log) = &self.denials {
            log.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(format_denial(tool_name, reason));
        }
        ApprovalOutcome::Denied
    }

    /// Nothing is ever pending for this gate — `request_approval` never
    /// parks a waiting task, so there is never anything to resolve.
    /// Idempotent no-op, matching `ApprovalGate::resolve`'s documented
    /// "safe to call when nothing is pending" contract.
    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_newlines_and_truncates() {
        let long = format!("line1\nline2\r\n{}", "x".repeat(500));
        let out = sanitize(&long);
        assert!(!out.contains('\n'));
        assert!(!out.contains('\r'));
        assert_eq!(out.chars().count(), MAX_SANITIZED_CHARS);
    }

    #[test]
    fn format_denial_names_the_tool_and_reason() {
        let text = format_denial("delete_repository", "destructive git operation");
        assert!(text.contains("delete_repository"));
        assert!(text.contains("destructive git operation"));
        assert!(text.to_lowercase().contains("denied"));
    }

    #[test]
    fn format_denial_sanitizes_newline_injection() {
        let text = format_denial("tool\nname", "reason\nwith\nbreaks");
        assert!(!text.contains('\n'));
    }

    #[tokio::test]
    async fn request_approval_denies_immediately_with_no_wait() {
        let gate = ImmediateDenyApprovalGate::new();
        let start = std::time::Instant::now();
        let outcome = gate
            .request_approval("session-1", "shell_exec", "rm -rf /", &serde_json::json!({}))
            .await;
        let elapsed = start.elapsed();
        assert_eq!(outcome, ApprovalOutcome::Denied);
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "request_approval took {elapsed:?} — must return immediately, never wait"
        );
    }

    #[tokio::test]
    async fn resolve_is_a_safe_no_op() {
        let gate = ImmediateDenyApprovalGate::new();
        assert!(!gate.resolve("session-1", true).await);
        assert!(!gate.resolve("session-1", false).await);
    }
}
