//! Channel-based approval gate for the `tui_rata` REPL (Phase 36.6.2 Plan 03).
//!
//! # Why this exists (RESEARCH Pitfall 2, HIGH severity)
//!
//! The headless [`CliApprovalGate`](crate::approval_gate::CliApprovalGate) resolves an
//! approval by BLOCKING on a synchronous stdin read (`spawn_blocking` +
//! `prompt_for_approval`). Inside `tui_rata` the terminal is owned by crossterm's
//! raw-mode `EventStream`, so a blocking stdin read there conflicts with the event
//! loop and hangs the whole UI. This gate replaces it: `request_approval` (called from
//! WITHIN the spawned agent-turn task) sends an [`ApprovalRequest`] carrying a
//! `oneshot::Sender` down an `UnboundedSender` that the main loop drains via a
//! `tokio::select!` arm, surfaces the approval overlay, and — when the operator
//! answers — `App::handle_key` fires the stashed `oneshot::Sender` back with the
//! decision. The awaiting `request_approval` then returns it.
//!
//! # Fail-closed (V4 access control)
//!
//! Every non-approval resolution is a denial. `rx.await.unwrap_or(Denied)` is the
//! load-bearing line: a dropped `oneshot::Sender` (main loop panicked, overlay
//! force-closed by unrelated code) resolves to `Denied`, never `Approved`.
//!
//! # D-04 session reuse
//!
//! The `[s]ession` tier and the pre-surface short-circuit both go through the SAME
//! process-lifetime [`ApprovalsStore`](ironhermes_core::ApprovalsStore) the headless
//! flow uses, with the SAME `normalize_command` cache-key scope — no parallel store.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use ironhermes_core::{ApprovalGate, ApprovalOutcome, ApprovalsStore};

use crate::tui_rata::overlay::OverlayKind;

/// An in-flight approval request surfaced from a spawned agent-turn task to the
/// TUI decision surface. Carries the `oneshot::Sender` the resolving keypress
/// fires the [`ApprovalOutcome`] back on.
pub struct ApprovalRequest {
    /// The session the blocked tool call belongs to (forwarded from the gate).
    pub session_id: String,
    /// The tool being gated (`"terminal"` / `"execute_code"`).
    pub tool_name: String,
    /// Human-readable reason from the guardrail (shown as the overlay detail).
    pub reason: String,
    /// The literal command string (empty for opaque `execute_code`), used for the
    /// overlay label and the sudo-vs-approval derivation.
    pub command: String,
    /// The `normalize_command`-derived cache key — the SAME scope the headless
    /// flow uses, threaded so the `[s]ession` grant reuses `ApprovalsStore`.
    pub cache_key: String,
    /// The reply channel: `App::handle_key` sends the operator's decision here.
    pub resolve: oneshot::Sender<ApprovalOutcome>,
}

impl ApprovalRequest {
    /// Derive which overlay to surface for this request. A `sudo`-prefixed
    /// `terminal` command surfaces the [`OverlayKind::Sudo`] "Confirm Privileged
    /// Action" modal; everything else surfaces [`OverlayKind::Approval`]. (The
    /// [`OverlayKind::Secret`] variant is not gate-derived in this phase — it is a
    /// scaffold for a future secret-input flow.)
    pub fn to_overlay_kind(&self) -> OverlayKind {
        let trimmed = self.command.trim_start();
        let is_sudo = trimmed == "sudo" || trimmed.starts_with("sudo ");
        let label = if self.command.trim().is_empty() {
            self.tool_name.clone()
        } else {
            self.command.clone()
        };
        if is_sudo {
            OverlayKind::Sudo {
                cache_key: self.cache_key.clone(),
                label,
            }
        } else {
            OverlayKind::Approval {
                cache_key: self.cache_key.clone(),
                label,
                detail: self.reason.clone(),
            }
        }
    }
}

/// Channel-based [`ApprovalGate`] for `tui_rata`. Holds the sender half of the
/// request channel plus the shared [`ApprovalsStore`] for the D-04 session-tier
/// short-circuit.
pub struct TuiApprovalGate {
    tx: UnboundedSender<ApprovalRequest>,
    approvals: Arc<ApprovalsStore>,
}

impl TuiApprovalGate {
    /// `tx`: the sender half wired to `App.approval_rx` (drained by
    /// `recv_approval_request` in `run_app_inner`'s `select!`). `approvals`: the
    /// SAME process-lifetime store every `spawn_turn` gating closure shares (WR-01)
    /// so a `[s]ession` grant persists exactly as it does headless.
    pub fn new(tx: UnboundedSender<ApprovalRequest>, approvals: Arc<ApprovalsStore>) -> Self {
        Self { tx, approvals }
    }
}

#[async_trait]
impl ApprovalGate for TuiApprovalGate {
    async fn request_approval(
        &self,
        session_id: &str,
        tool_name: &str,
        reason: &str,
        args: &serde_json::Value,
    ) -> ApprovalOutcome {
        // Same command extraction + cache-key normalization CliApprovalGate uses,
        // so the [s]ession short-circuit shares the headless cache-key scope (D-04).
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or(reason)
            .to_string();
        let cache_key = ApprovalsStore::normalize_command(&command);

        // D-04: session-tier short-circuit BEFORE surfacing any overlay — mirrors
        // CliApprovalGate's check-cache-first discipline.
        if self.approvals.is_session_approved(&cache_key).await {
            return ApprovalOutcome::Approved;
        }

        let (resolve, rx) = oneshot::channel();
        if self
            .tx
            .send(ApprovalRequest {
                session_id: session_id.to_string(),
                tool_name: tool_name.to_string(),
                reason: reason.to_string(),
                command,
                cache_key,
                resolve,
            })
            .is_err()
        {
            // Receiver (main loop) is gone — fail closed.
            return ApprovalOutcome::Denied;
        }

        // Fail-closed on channel drop: a dropped oneshot::Sender (overlay
        // force-closed, main loop panic) resolves to Denied, never Approved.
        rx.await.unwrap_or(ApprovalOutcome::Denied)
    }

    /// Unused — resolution happens via the stashed `oneshot::Sender`, not this
    /// out-of-band method (mirrors `CliApprovalGate::resolve`). Idempotent no-op.
    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// tui_rata intercept builders (RESEARCH Pitfall 2 / Pitfall 4)
// ─────────────────────────────────────────────────────────────────────────────
//
// These mirror `crate::approval_gate::build_gated_terminal_intercept` /
// `build_gated_execute_code_intercept` EXACTLY, except they route
// `execute_gated_command`'s `approval_gate: Option<&dyn ApprovalGate>` parameter
// through the channel-based `TuiApprovalGate` instead of constructing a blocking
// `CliApprovalGate`. This is the fix for RESEARCH Pitfall 2's warning sign: leaving
// the CliApprovalGate stdin prompt wired underneath the overlay would hang the first
// time a real terminal/execute_code approval fires under crossterm raw mode. The
// original builders stay untouched (main.rs's other local surfaces still use them).

/// Build a `terminal` intercept for the `tui_rata` surface that gates through the
/// channel-based `gate` (a `TuiApprovalGate`) instead of the blocking CLI prompt.
#[allow(clippy::too_many_arguments)]
pub fn build_tui_gated_terminal_intercept(
    tool: Option<Arc<dyn ironhermes_tools::Tool>>,
    config: Arc<ironhermes_core::Config>,
    session_id: String,
    surface: &'static str,
    chat_id: String,
    yolo: bool,
    gate: Arc<dyn ApprovalGate>,
) -> ironhermes_tools::registry::InterceptHandler {
    Arc::new(move |args: serde_json::Value| {
        let tool = tool.clone();
        let config = config.clone();
        let session_id = session_id.clone();
        let chat_id = chat_id.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(
                &config.dangerous_commands,
            );
            let audit_log = ironhermes_core::AuditLog::load(config.audit.clone());
            let is_remote_backend = config.terminal.backend == "ssh";
            let forward_env_nonempty = !config.terminal.forward_env.is_empty();

            let outcome = ironhermes_hooks::execute_gated_command(
                "terminal",
                &command,
                &guard,
                Some(gate.as_ref()),
                &audit_log,
                &session_id,
                surface,
                &chat_id,
                yolo,
                is_remote_backend,
                forward_env_nonempty,
                || async move {
                    match tool {
                        Some(t) => t.execute(args).await,
                        None => Err(anyhow::anyhow!("terminal tool not registered on this runtime")),
                    }
                },
            )
            .await;

            Ok(outcome.to_string())
        })
    })
}

/// Build an `execute_code` intercept for the `tui_rata` surface that gates through
/// the channel-based `gate`. Mirrors `build_tui_gated_terminal_intercept` but passes
/// an empty opaque `classify_arg` and `is_remote_backend=false` /
/// `forward_env_nonempty=false` (D-11).
#[allow(clippy::too_many_arguments)]
pub fn build_tui_gated_execute_code_intercept(
    tool: Option<Arc<dyn ironhermes_tools::Tool>>,
    config: Arc<ironhermes_core::Config>,
    session_id: String,
    surface: &'static str,
    chat_id: String,
    yolo: bool,
    gate: Arc<dyn ApprovalGate>,
) -> ironhermes_tools::registry::InterceptHandler {
    Arc::new(move |args: serde_json::Value| {
        let tool = tool.clone();
        let config = config.clone();
        let session_id = session_id.clone();
        let chat_id = chat_id.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let guard = ironhermes_hooks::DangerousCommandGuardrail::from_config(
                &config.dangerous_commands,
            );
            let audit_log = ironhermes_core::AuditLog::load(config.audit.clone());

            let outcome = ironhermes_hooks::execute_gated_command(
                "execute_code",
                "", // D-11: opaque — Python source is not shell syntax
                &guard,
                Some(gate.as_ref()),
                &audit_log,
                &session_id,
                surface,
                &chat_id,
                yolo,
                false, // D-11: execute_code never routes to a remote backend
                false, // D-11: execute_code never forwards credentials cross-boundary
                || async move {
                    match tool {
                        Some(t) => t.execute(args).await,
                        None => Err(anyhow::anyhow!(
                            "execute_code tool not registered on this runtime"
                        )),
                    }
                },
            )
            .await;

            Ok(outcome.to_string())
        })
    })
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;

    /// A real gate + channel round-trip: a receiver task drains the request and
    /// sends `Approved`; `request_approval` returns `Approved`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_channel_roundtrip_approve() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_roundtrip.json"),
        ));
        let gate = TuiApprovalGate::new(tx, approvals);

        let req_task = tokio::spawn(async move {
            gate.request_approval(
                "sess",
                "terminal",
                "reason",
                &serde_json::json!({ "command": "echo hi" }),
            )
            .await
        });

        let req = rx.recv().await.expect("request surfaced on the channel");
        assert_eq!(req.cache_key, "echo hi");
        let _ = req.resolve.send(ApprovalOutcome::Approved);

        let outcome = req_task.await.unwrap();
        assert_eq!(outcome, ApprovalOutcome::Approved);
    }

    /// Dropping the `oneshot::Sender` without answering must fail closed → Denied.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_fails_closed_on_channel_drop() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_drop.json"),
        ));
        let gate = TuiApprovalGate::new(tx, approvals);

        let req_task = tokio::spawn(async move {
            gate.request_approval(
                "sess",
                "terminal",
                "reason",
                &serde_json::json!({ "command": "rm -rf /" }),
            )
            .await
        });

        let req = rx.recv().await.expect("request surfaced on the channel");
        // Drop the resolve sender WITHOUT sending — the fail-closed path.
        drop(req.resolve);

        let outcome = req_task.await.unwrap();
        assert_eq!(
            outcome,
            ApprovalOutcome::Denied,
            "a dropped oneshot::Sender must resolve to Denied (fail-closed), never Approved"
        );
    }

    /// A pre-approved cache_key short-circuits to Approved WITHOUT emitting an
    /// ApprovalRequest (D-04 session-tier reuse of ApprovalsStore).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_session_shortcircuit() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_session.json"),
        ));
        // Pre-approve the SAME normalized cache key on the SAME store.
        approvals.approve_session("echo hi").await;
        let gate = TuiApprovalGate::new(tx, approvals);

        let outcome = gate
            .request_approval(
                "sess",
                "terminal",
                "reason",
                &serde_json::json!({ "command": "echo   hi" }), // normalizes to "echo hi"
            )
            .await;

        assert_eq!(outcome, ApprovalOutcome::Approved);
        assert!(
            rx.try_recv().is_err(),
            "session short-circuit must NOT emit an ApprovalRequest"
        );
    }
}
