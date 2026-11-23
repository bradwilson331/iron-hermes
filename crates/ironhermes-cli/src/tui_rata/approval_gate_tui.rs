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
//!
//! # D-21: this gate never decides a bypass
//!
//! Phase 57 retired the old shared-atomic yolo field and its first-statement
//! short-circuit. `request_approval` now ALWAYS either answers from the
//! `[s]ession` cache or surfaces a prompt on its channel: it returns `Approved`
//! only for an operator answer or a session grant, never because a level or a
//! flag says so. A Warn at DEFCON 4-5 is bypassed UPSTREAM, by
//! `execute_gated_command` (and the registry screen), which read the session's
//! live level at the decision and write the `resolution="bypass"` audit row,
//! the per-turn ledger entry and the operator notice. A live `/yolo <N>` is the
//! audited core command (`cmd_yolo`) and changes that level; this gate holds no
//! copy of it, so there is nothing for it to go stale on (closes the folded todo
//! `2026-10-07-tui-live-yolo-toggle-silent-bypass`).
//!
//! The intercept builders below take no yolo flag either: each reads
//! `GatePolicy::for_session(&session_id)` when the call is decided, so a
//! `/yolo` typed mid-turn is observed by the very next gated call.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use ironhermes_core::{ApprovalGate, ApprovalOutcome, ApprovalsStore, YoloBypassSummary};

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
/// short-circuit. It carries no DEFCON or yolo state: bypass is decided upstream.
pub struct TuiApprovalGate {
    tx: UnboundedSender<ApprovalRequest>,
    approvals: Arc<ApprovalsStore>,
    /// D-04 (Phase 54 Finding 1): optional sender for yolo-bypass notices,
    /// drained by `recv_yolo_notice` in `run_app_inner`'s `select!` and pushed
    /// into the ratatui transcript as a `Role::System` line via
    /// `App::surface_yolo_notice`. `None` on every existing construction site
    /// (via [`new`](Self::new)) and every test in this module — they keep
    /// compiling unchanged; only the production `event_loop.rs` call site
    /// supplies `Some` via [`new_with_notice_tx`](Self::new_with_notice_tx).
    notice_tx: Option<UnboundedSender<String>>,
}

impl TuiApprovalGate {
    /// `tx`: the sender half wired to `App.approval_rx` (drained by
    /// `recv_approval_request` in `run_app_inner`'s `select!`). `approvals`: the
    /// SAME process-lifetime store every `spawn_turn` gating closure shares (WR-01)
    /// so a `[s]ession` grant persists exactly as it does headless.
    ///
    /// The yolo-bypass notice sender is `None` from this constructor — use
    /// [`new_with_notice_tx`](Self::new_with_notice_tx) to wire D-04 notification.
    pub fn new(tx: UnboundedSender<ApprovalRequest>, approvals: Arc<ApprovalsStore>) -> Self {
        Self {
            tx,
            approvals,
            notice_tx: None,
        }
    }

    /// Same as [`new`](Self::new), but also wires the yolo-bypass notice channel
    /// (D-04, Phase 54 Finding 1) — `notice_tx` is the sender half of
    /// `App.yolo_notice_rx`, cloned in `spawn_turn`. Pass `None` to get the
    /// exact behavior of [`new`](Self::new) (default no-op notifier).
    pub fn new_with_notice_tx(
        tx: UnboundedSender<ApprovalRequest>,
        approvals: Arc<ApprovalsStore>,
        notice_tx: Option<UnboundedSender<String>>,
    ) -> Self {
        Self {
            tx,
            approvals,
            notice_tx,
        }
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

    /// D-04 (Phase 54 Finding 1): push a yolo-bypass notice onto the SAME
    /// channel shape this gate already uses to reach the ratatui transcript
    /// (mirrors `App.approval_tx`/`approval_rx`) — never `println!`/`eprintln!`,
    /// which would corrupt the frame under crossterm raw mode. Best-effort: a
    /// missing sender (constructed via [`new`](Self::new), or the receiver
    /// already dropped) is a silent no-op, not a panic.
    ///
    /// Phase 54.3: this is the PER-CALL unscoped fallback, reached only when a
    /// bypass happens outside any `YOLO_LEDGER` turn scope. The per-turn path is
    /// [`notify_yolo_bypass_turn`](ApprovalGate::notify_yolo_bypass_turn).
    async fn notify_yolo_bypass(&self, _session_id: &str, tool_name: &str, reason: &str) {
        if let Some(tx) = &self.notice_tx {
            let _ = tx.send(format!(
                "[YOLO BYPASS] `{tool_name}` ran without approval — reason: {reason}"
            ));
        }
    }

    /// Phase 54.3 D-03/D-08: ONE short message per bypassing turn on the existing
    /// notice channel, which `recv_yolo_notice` drains into exactly one collapsed
    /// `Role::System` transcript row via `App::surface_yolo_notice`.
    ///
    /// The text is `YoloBypassSummary::short_line()` — the same shared formatter
    /// every other surface uses — so it is newline-free and length-capped.
    ///
    /// Never `println!`/`eprintln!`: writing to stdout or stderr here corrupts
    /// the ratatui frame under crossterm raw mode (G-41.1-1); the channel is the
    /// only legal delivery path. Best-effort: a missing sender is a silent no-op.
    ///
    /// D-08: the signal deliberately stays in the TRANSCRIPT as a compact row. It
    /// was offered a home in the TUI status line and rejected; that is a
    /// deliberate divergence from the web surface's header placement (D-06) and
    /// must not be "fixed" for consistency.
    async fn notify_yolo_bypass_turn(&self, summary: &YoloBypassSummary) {
        if let Some(tx) = &self.notice_tx {
            let _ = tx.send(summary.short_line());
        }
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
                // D-09/D-21: the session's LIVE level and the booted sidecar, read when the
                // call is decided (never a flag captured when this closure was built).
                ironhermes_hooks::GatePolicy::for_session(&session_id),
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
/// the channel-based `gate`. Mirrors `build_tui_gated_terminal_intercept` but per
/// D-01 (overturns D-11) passes the caller's REAL Python source as `classify_arg` —
/// `DangerousCommandGuardrail::check`'s `tool_name == "execute_code"` branch
/// classifies it — and always forwards `is_remote_backend=false` /
/// `forward_env_nonempty=false`. Missing or non-string `code` in the incoming args
/// coerces to an empty `classify_arg` rather than panicking.
#[allow(clippy::too_many_arguments)]
pub fn build_tui_gated_execute_code_intercept(
    tool: Option<Arc<dyn ironhermes_tools::Tool>>,
    config: Arc<ironhermes_core::Config>,
    session_id: String,
    surface: &'static str,
    chat_id: String,
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

            // D-01 (overturns D-11): extract the real Python source so
            // DangerousCommandGuardrail's execute_code branch can classify it.
            let classify_arg = args
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let outcome = ironhermes_hooks::execute_gated_command(
                "execute_code",
                &classify_arg,
                &guard,
                Some(gate.as_ref()),
                &audit_log,
                &session_id,
                surface,
                &chat_id,
                // D-09/D-21: the session's LIVE level and the booted sidecar, read when the
                // call is decided (never a flag captured when this closure was built).
                ironhermes_hooks::GatePolicy::for_session(&session_id),
                false, // D-01: execute_code never routes to a remote backend
                false, // D-01: execute_code never forwards credentials cross-boundary
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

    /// D-21 (Phase 57 plan 13): the old yolo short-circuit is gone. The gate sends a
    /// prompt on its channel and returns the OPERATOR's answer; with a dead receiver it
    /// fails closed. Nothing about the session's DEFCON level — even 5, the highest
    /// bypass level — makes `request_approval` return `Approved` on its own.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_never_auto_approves() {
        // The bypass level is set on the very session the request names, so a gate that
        // still consulted the level would have an excuse to approve.
        ironhermes_core::defcon::session_defcon().set_session_level(
            "sess-never-auto",
            ironhermes_core::config::DefconLevel::FIVE,
        );

        // (a) dead receiver: fail closed, never Approved.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        drop(rx);
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_never_auto_dead.json"),
        ));
        let gate = TuiApprovalGate::new(tx, approvals);
        let outcome = gate
            .request_approval(
                "sess-never-auto",
                "terminal",
                "reason",
                &serde_json::json!({ "command": "rm -rf /tmp/x" }),
            )
            .await;
        assert_eq!(
            outcome,
            ApprovalOutcome::Denied,
            "a dead channel must fail closed at DEFCON 5 too, never auto-approve"
        );

        // (b) live receiver: a prompt is sent and the operator's Denied answer wins.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_never_auto_live.json"),
        ));
        let gate = TuiApprovalGate::new(tx, approvals);
        let req_task = tokio::spawn(async move {
            gate.request_approval(
                "sess-never-auto",
                "terminal",
                "reason",
                &serde_json::json!({ "command": "echo hi" }),
            )
            .await
        });
        let req = rx
            .recv()
            .await
            .expect("the gate must surface a prompt at every level");
        let _ = req.resolve.send(ApprovalOutcome::Denied);
        assert_eq!(
            req_task.await.unwrap(),
            ApprovalOutcome::Denied,
            "the operator's answer must win"
        );

        ironhermes_core::defcon::session_defcon().forget("sess-never-auto");
    }

    /// The overlay path: a request is surfaced and an operator `Approved` answer is
    /// honoured (the only way this gate returns Approved besides a session grant).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_surfaces_overlay_and_honours_answer() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_surfaces.json"),
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

        let req = rx
            .recv()
            .await
            .expect("an ApprovalRequest must be emitted on the channel");
        let _ = req.resolve.send(ApprovalOutcome::Approved);

        assert_eq!(req_task.await.unwrap(), ApprovalOutcome::Approved);
    }

    // ---------------------------------------------------------------------------
    // Task 3 (D-04): notify_yolo_bypass override — fails if the override is
    // deleted (defaults back to the trait's no-op and the channel never fires).
    // ---------------------------------------------------------------------------

    /// A gate constructed via `new_with_notice_tx(Some(tx))` delivers a
    /// `notify_yolo_bypass` call's message onto that channel — proven by
    /// draining it, not merely asserting the method compiles. If the override
    /// were deleted (falling back to the trait's default no-op), `rx.recv()`
    /// would hang forever instead of returning `Some(..)`.
    ///
    /// Phase 54.3: covers the PER-CALL unscoped fallback, retained unchanged;
    /// the per-turn contract is `tui_gate_notify_turn_sends_one_short_row_for_n_bypasses`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_notify_yolo_bypass_reaches_notice_channel() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        drop(rx);
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_notify.json"),
        ));
        let (notice_tx, mut notice_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let gate = TuiApprovalGate::new_with_notice_tx(tx, approvals, Some(notice_tx));

        gate.notify_yolo_bypass("sess", "execute_code", "dangerous pattern matched")
            .await;

        let notice = tokio::time::timeout(std::time::Duration::from_secs(2), notice_rx.recv())
            .await
            .expect("notice channel must receive within timeout — the override must not hang")
            .expect("channel must yield Some(..), not None (sender dropped)");
        assert!(
            notice.contains("execute_code"),
            "notice must name the tool: {notice}"
        );
        assert!(
            notice.contains("dangerous pattern matched"),
            "notice must contain the guardrail reason: {notice}"
        );
    }

    /// `new()` (no notice sender wired) must be a silent no-op — never a panic
    /// — when `notify_yolo_bypass` is called. Exercises the default-constructed
    /// path every existing production/test call site still uses.
    ///
    /// Phase 54.3: covers the PER-CALL unscoped fallback; the per-turn twin is
    /// `tui_gate_notify_turn_without_tx_does_not_panic`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_notify_yolo_bypass_without_notice_tx_does_not_panic() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        drop(rx);
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_notify_none.json"),
        ));
        let gate = TuiApprovalGate::new(tx, approvals);

        // Must not panic.
        gate.notify_yolo_bypass("sess", "terminal", "reason").await;
    }

    /// Phase 54.3 D-03/D-08: N bypasses in one turn reach the transcript as
    /// exactly ONE short message on the existing notice channel — a second
    /// `try_recv` must find the channel empty, and the row must be a single
    /// line of at most 120 chars so it renders as one transcript row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_notify_turn_sends_one_short_row_for_n_bypasses() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        drop(rx);
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_notify_turn.json"),
        ));
        let (notice_tx, mut notice_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let gate = TuiApprovalGate::new_with_notice_tx(tx, approvals, Some(notice_tx));
        let reason = "command contains shell metacharacters — string-level matching cannot verify safety; approval required (D-09)";
        let mut summary =
            ironhermes_core::YoloBypassSummary::single("sess", "tui", "chat", "terminal", reason);
        summary.items.push(ironhermes_core::YoloBypass {
            tool: "terminal".to_string(),
            reason: reason.to_string(),
        });

        gate.notify_yolo_bypass_turn(&summary).await;

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), notice_rx.recv())
            .await
            .expect("notice channel must receive within timeout — the override must not hang")
            .expect("channel must yield Some(..), not None (sender dropped)");
        assert!(
            notice_rx.try_recv().is_err(),
            "exactly ONE message per turn — a second per-call row must not arrive (D-08)"
        );
        assert!(msg.contains('2'), "must carry the aggregate count: {msg}");
        assert!(msg.contains("terminal"), "must name the tool: {msg}");
        assert!(!msg.contains('\n'), "must be a single row: {msg:?}");
        assert!(
            msg.chars().count() <= 120,
            "must be a short row (<= 120 chars), got {}: {msg}",
            msg.chars().count()
        );
    }

    /// Per-turn twin of the no-tx test: with no notice sender wired,
    /// `notify_yolo_bypass_turn` must be a silent no-op — never a panic.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tui_gate_notify_turn_without_tx_does_not_panic() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ApprovalRequest>();
        drop(rx);
        let approvals = Arc::new(ApprovalsStore::with_path(
            std::env::temp_dir().join("gsd_tui_gate_notify_turn_none.json"),
        ));
        let gate = TuiApprovalGate::new(tx, approvals);
        let summary =
            ironhermes_core::YoloBypassSummary::single("sess", "tui", "chat", "terminal", "reason");

        // Must not panic.
        gate.notify_yolo_bypass_turn(&summary).await;
    }
}
