//! `execute_gated_command()` — the surface-agnostic guardrail→approval→audit chokepoint.
//!
//! Phase 36.3.12 (D-08/D-09/D-10/D-11): generalizes
//! `ironhermes-gateway::shell_exec::shell_exec()` so CLI, TUI, and gateway can share ONE
//! decision path for BOTH the `terminal` tool and `execute_code`. Two deltas over the
//! gateway-only helper it generalizes (RESEARCH.md §Common Pitfalls):
//!
//! 1. **Audits every resolution** — including `Allow` (Pitfall 3: today's Allow path is
//!    never audited on any surface).
//! 2. **Forces approval on remote / credential-forwarding runs** — even when the command
//!    classifies as `Allow` (D-08).
//!
//! The executor is a caller-supplied `run` closure so this module never depends on the
//! tool-execution or backend-exec crates — no crate-dependency cycle is introduced.
//! Callers (CLI/TUI/gateway `terminal`/`execute_code` call sites, wired in Plan 07) supply
//! the actual subprocess/sandbox invocation.
//!
//! # Decision routing (mirrors `shell_exec.rs`, extended)
//!
//! ```text
//! guard.check(tool_name, {"command": classify_arg})
//!          │
//!          ├─ Block (Tier-2, D-03) ──────────────────► Blocked — run NEVER invoked,
//!          │                                            audited (fires before yolo, D-13).
//!          │                                            No yolo check exists below this
//!          │                                            arm — yolo cannot reach it.
//!          │
//!          ├─ Warn (Tier-1) / NeedsApproval (defensive)
//!          │       ├─ yolo=true  ─────────────────────► Ran (audited "allowed"/"bypass")
//!          │       │                                     — NeedsApproval never bypasses
//!          │       │                                       yolo (fail-closed default)
//!          │       ├─ gate present ─────────────────────► request_approval() → Ran/Denied
//!          │       │                                       (audited per outcome)
//!          │       └─ no gate ─────────────────────────► Denied (audited "denied"/"no_gate")
//!          │
//!          └─ Allow ─────────────────────────────────────► audited "allowed"; UNLESS
//!                    is_remote_backend || forward_env_nonempty (D-08 forced approval),
//!                    in which case:
//!                        ├─ yolo=true  ────────────────► Ran (audited "allowed"/"bypass")
//!                        │                                — the yolo carve-out below
//!                        └─ yolo=false ────────────────► routed through the SAME
//!                                                          approval branch as Warn
//! ```
//!
//! # The yolo / D-08 forced-approval carve-out (decided posture, Phase 36.3.12 Plan 12)
//!
//! `needs_forced_approval && !yolo` (the `Allow` arm's guard condition, below) is a
//! **decided, documented** carve-out, not an oversight:
//!
//! 1. **`yolo=true` bypasses D-08's forced-approval-on-remote path**, per Plan 03's literal
//!    spec (`needs_forced_approval && !yolo`) — INCLUDING on an `Allow`-classified command
//!    that is running against a remote backend (`is_remote_backend`) or forwarding
//!    credentials (`forward_env_nonempty`). Under `yolo=true` such a command runs exactly
//!    like any other `Allow` command: no approval prompt, but still audited
//!    (`resolution = "bypass"`).
//! 2. **The Tier-2 `Block` remains the unconditional floor.** `Block` is matched and
//!    returned before any `yolo` check exists in the call graph (see the routing diagram's
//!    top arm) — there is no code path from there that invokes `run`. Yolo can relax the
//!    *approval* tier; it can never relax the Tier-2 *block*.
//! 3. **Practical consequence:** an operator running with `yolo=true` and
//!    `terminal.backend: ssh` (or a non-empty `terminal.forward_env`) will NOT be prompted
//!    before a command runs remotely with forwarded credentials. Every resolution is still
//!    AUDITED — the audit floor (this module's first architectural guarantee, above) is
//!    unconditional — so the action is attributable after the fact even when it was not
//!    gated before the fact.
//! 4. **Whether this carve-out should survive is deferred to the tiered DEFCON strictness
//!    phase**, where "what may yolo relax" is the actual subject matter — D-08 explicitly
//!    fences DEFCON tiering out of this phase (`36.3.12-CONTEXT.md`: "Tiered DEFCON
//!    strictness stays in its own phase"). That future phase is the named owner of any
//!    reconsideration; this module keeps the carve-out exactly as Plan 03 specified it.

use std::future::Future;

use ironhermes_core::{ApprovalGate, ApprovalOutcome, AuditLog};

use crate::guardrail::{DangerousCommandGuardrail, GuardrailDecision, GuardrailHook};

/// The outcome of a gated command execution.
///
/// Mirrors `ironhermes-gateway::shell_exec::ShellExecOutcome`'s shape so callers
/// migrating off that gateway-only type have a familiar surface.
#[derive(Debug, Clone, PartialEq)]
pub enum GatedOutcome {
    /// The `run` closure executed successfully; inner string is its output.
    Ran(String),
    /// The command was denied — no approval gate wired, operator denied, timed out, or
    /// another approval already pending for this session.
    Denied(String),
    /// Tier-2 hard-block (D-03) — `run` was NEVER invoked. This is the one outcome that
    /// yolo can never turn into `Ran` (D-13 invariant).
    Blocked(String),
    /// `run` was invoked but returned an error (WR-03 semantics — distinguishable from a
    /// command that never executed at all).
    Failed(String),
}

impl std::fmt::Display for GatedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ran(s) => write!(f, "Ran({s})"),
            Self::Denied(s) => write!(f, "Denied({s})"),
            Self::Blocked(s) => write!(f, "Blocked({s})"),
            Self::Failed(s) => write!(f, "Failed({s})"),
        }
    }
}

/// Surface-agnostic guardrail→approval→audit chokepoint (D-08/D-09/D-10/D-11).
///
/// # Parameters
///
/// - `tool_name`: `"terminal"` or `"execute_code"`.
/// - `classify_arg`: the shell command string for `"terminal"`; an EMPTY opaque string
///   for `"execute_code"` (D-11 — `DangerousCommandGuardrail`'s `DANGEROUS_PATTERNS`
///   target shell syntax, not Python source, so passing raw Python here would be both
///   meaningless and a false sense of coverage).
/// - `guard`: the compiled two-tier dangerous-command guardrail.
/// - `approval_gate`: `None` fails closed on any approval-required resolution (D-12).
/// - `audit_log`: appended to on EVERY resolution branch, `Allow` included (Pitfall 3).
/// - `session_id` / `surface` / `chat_id`: forwarded into the audit entry and the
///   approval-gate prompt.
/// - `yolo`: bypasses Tier-1 (`Warn`) and forced-approval-on-remote/`Allow` approval
///   (still audited) — but NEVER bypasses a Tier-2 `Block`, because the `Block` branch is
///   matched and returned before the `yolo` check is ever reached (D-13 invariant).
/// - `is_remote_backend`: true for a non-local execution backend (e.g. ssh) — forces
///   approval even on an `Allow` classification (D-08).
/// - `forward_env_nonempty`: true when credentials/env vars are being forwarded into the
///   remote/child environment — forces approval even on an `Allow` classification (D-08).
/// - `run`: the caller-supplied executor closure. NEVER invoked on a `Blocked` outcome.
#[allow(clippy::too_many_arguments)]
pub async fn execute_gated_command<F, Fut>(
    tool_name: &str,
    classify_arg: &str,
    guard: &DangerousCommandGuardrail,
    approval_gate: Option<&dyn ApprovalGate>,
    audit_log: &AuditLog,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    yolo: bool,
    is_remote_backend: bool,
    forward_env_nonempty: bool,
    run: F,
) -> GatedOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    let raw_args = serde_json::json!({ "command": classify_arg });
    let decision = guard.check(tool_name, &raw_args);

    match decision {
        // ── Tier-2: hard-block (D-03/D-13) ────────────────────────────────
        // This branch is matched and returned BEFORE any yolo check exists in the call
        // stack below. There is no code path from here that invokes `run`.
        GuardrailDecision::Block { reason } => {
            audit(
                audit_log,
                tool_name,
                session_id,
                surface,
                chat_id,
                &reason,
                "blocked",
                "dropped",
                &raw_args,
            )
            .await;
            GatedOutcome::Blocked(reason)
        }

        // ── Tier-1 (network/exfil/eval/metachar) — approval-required ─────
        GuardrailDecision::Warn { reason } => {
            handle_approval_required(
                tool_name,
                &reason,
                &raw_args,
                approval_gate,
                audit_log,
                session_id,
                surface,
                chat_id,
                yolo,
                run,
            )
            .await
        }

        // ── Defensive fail-closed mapping (D-08) ──────────────────────────
        // `DangerousCommandGuardrail::check()` never returns this variant today (it only
        // reads a "command" arg; `NeedsApproval` is emitted by the MCP-mutation/wrangler
        // guardrails instead), but the match must stay exhaustive and safe if the
        // guardrail chain passed to this helper ever changes. Yolo does NOT bypass this
        // branch — an unrecognized "needs approval" signal always fails closed.
        GuardrailDecision::NeedsApproval { reason } => {
            handle_approval_required(
                tool_name,
                &reason,
                &raw_args,
                approval_gate,
                audit_log,
                session_id,
                surface,
                chat_id,
                false,
                run,
            )
            .await
        }

        // ── Allow — audited unconditionally (Pitfall 3); forced through the
        // approval branch when remote/cred-forwarding (D-08), unless yolo. ──
        GuardrailDecision::Allow => {
            let needs_forced_approval = is_remote_backend || forward_env_nonempty;
            if needs_forced_approval && !yolo {
                let reason = forced_approval_reason(is_remote_backend, forward_env_nonempty);
                handle_approval_required(
                    tool_name,
                    &reason,
                    &raw_args,
                    approval_gate,
                    audit_log,
                    session_id,
                    surface,
                    chat_id,
                    false,
                    run,
                )
                .await
            } else {
                let resolution = if needs_forced_approval {
                    "bypass"
                } else {
                    "allowed"
                };
                audit(
                    audit_log,
                    tool_name,
                    session_id,
                    surface,
                    chat_id,
                    "command classified Allow by the dangerous-command guardrail",
                    "allowed",
                    resolution,
                    &raw_args,
                )
                .await;
                run_and_wrap(run).await
            }
        }
    }
}

/// D-08 forced-approval reason text — used both in the audit entry and the
/// operator-facing approval prompt.
fn forced_approval_reason(is_remote_backend: bool, forward_env_nonempty: bool) -> String {
    if is_remote_backend && forward_env_nonempty {
        "remote backend AND non-empty forward_env — approval required regardless of \
         guardrail classification (D-08)"
            .to_string()
    } else if is_remote_backend {
        "remote backend (e.g. ssh) — approval required regardless of guardrail \
         classification (D-08)"
            .to_string()
    } else {
        "non-empty forward_env (credential forwarding) — approval required regardless of \
         guardrail classification (D-08)"
            .to_string()
    }
}

/// Shared approval-required handling for `Warn`, defensive `NeedsApproval`, and the
/// forced-approval-on-remote/`Allow` path. Every outcome is audited before returning.
#[allow(clippy::too_many_arguments)]
async fn handle_approval_required<F, Fut>(
    tool_name: &str,
    reason: &str,
    raw_args: &serde_json::Value,
    approval_gate: Option<&dyn ApprovalGate>,
    audit_log: &AuditLog,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    yolo: bool,
    run: F,
) -> GatedOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    if yolo {
        // D-13: yolo bypasses Tier-1/forced-approval — Tier-2 Block is matched and
        // returned in execute_gated_command before this function is ever called for a
        // Block decision, so no code path here can bypass a Tier-2 command.
        audit(
            audit_log, tool_name, session_id, surface, chat_id, reason, "allowed", "bypass",
            raw_args,
        )
        .await;
        return run_and_wrap(run).await;
    }

    let Some(gate) = approval_gate else {
        // D-12: no gate wired — fail-closed.
        audit(
            audit_log, tool_name, session_id, surface, chat_id, reason, "denied", "no_gate",
            raw_args,
        )
        .await;
        return GatedOutcome::Denied(format!(
            "Command requires approval; no approval gate is wired for this surface: {reason}"
        ));
    };

    let outcome = gate
        .request_approval(session_id, tool_name, reason, raw_args)
        .await;

    let (decision, resolution) = match outcome {
        ApprovalOutcome::Approved => ("approved", "operator"),
        ApprovalOutcome::Denied => ("denied", "operator"),
        ApprovalOutcome::TimedOut => ("denied", "timeout"),
        ApprovalOutcome::AlreadyPending => ("denied", "already_pending"),
        ApprovalOutcome::GateUnavailable => ("denied", "gate_unavailable"),
    };
    audit(
        audit_log, tool_name, session_id, surface, chat_id, reason, decision, resolution,
        raw_args,
    )
    .await;

    match outcome {
        ApprovalOutcome::Approved => run_and_wrap(run).await,
        other => GatedOutcome::Denied(format!(
            "Command denied (outcome: {other:?}); did not execute."
        )),
    }
}

/// Append one audit entry, unconditionally (Pitfall 3). A write failure is logged but
/// does not change the caller's decision outcome — this module owns only the write;
/// a fail-closed *downgrade* on audit-append failure (mirroring
/// `ApprovalCoordinator::request()` in the gateway) is a caller-level concern for
/// surfaces that need that stronger guarantee.
#[allow(clippy::too_many_arguments)]
async fn audit(
    audit_log: &AuditLog,
    tool_name: &str,
    session_id: &str,
    surface: &str,
    chat_id: &str,
    reason: &str,
    decision: &str,
    resolution: &str,
    raw_args: &serde_json::Value,
) {
    let entry = audit_log.make_entry(
        "shell_command",
        tool_name,
        None,
        session_id,
        surface,
        chat_id,
        reason,
        decision,
        resolution,
        raw_args,
    );
    if let Err(e) = audit_log.append(&entry).await {
        tracing::warn!(
            target: "ironhermes::security::gated_exec",
            error = %e,
            tool = %tool_name,
            "failed to append audit entry for gated command resolution"
        );
    }
}

async fn run_and_wrap<F, Fut>(run: F) -> GatedOutcome
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    match run().await {
        Ok(output) => GatedOutcome::Ran(output),
        // WR-03 semantics: an execution error is distinguishable from a command that
        // never ran (Denied/Blocked).
        Err(e) => GatedOutcome::Failed(format!("execution error: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Tests — exhaustive branch coverage + audit-floor assertions (Task 2, D-08/D-10/D-11)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::AuditConfig;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Fake `ApprovalGate` returning a preset outcome; records whether
    /// `request_approval` was invoked so tests can assert the gate was (or was not)
    /// consulted.
    struct FakeGate {
        outcome: ApprovalOutcome,
        called: AtomicBool,
    }

    impl FakeGate {
        fn new(outcome: ApprovalOutcome) -> Self {
            Self {
                outcome,
                called: AtomicBool::new(false),
            }
        }
    }

    #[async_trait::async_trait]
    impl ApprovalGate for FakeGate {
        async fn request_approval(
            &self,
            _session_id: &str,
            _tool_name: &str,
            _reason: &str,
            _args: &serde_json::Value,
        ) -> ApprovalOutcome {
            self.called.store(true, Ordering::SeqCst);
            self.outcome
        }

        async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
            false
        }
    }

    fn test_guard() -> DangerousCommandGuardrail {
        DangerousCommandGuardrail::new()
    }

    /// Tempfile-backed `AuditLog` (Task 2 requirement) — returns the `TempDir` guard
    /// alongside the log so the directory isn't dropped before assertions read it back.
    fn test_log() -> (tempfile::TempDir, AuditLog) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::with_path(path, AuditConfig::default());
        (dir, log)
    }

    fn read_entries(dir: &tempfile::TempDir) -> Vec<ironhermes_core::AuditEntry> {
        let path = dir.path().join("audit.jsonl");
        let contents = std::fs::read_to_string(&path).unwrap_or_default();
        contents
            .lines()
            .map(|l| serde_json::from_str(l).expect("valid AuditEntry JSON"))
            .collect()
    }

    /// D-03/D-13: a Tier-2 `Block` command NEVER invokes `run` (AtomicBool stays false),
    /// EVEN UNDER yolo=true, and is audited with decision "blocked".
    #[tokio::test]
    async fn block_never_invokes_run_even_under_yolo() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();

        let outcome = execute_gated_command(
            "terminal",
            "rm -rf /",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            true, // yolo=true — must NOT bypass Tier-2 (D-13)
            false,
            false,
            || async move {
                ran_clone.store(true, Ordering::SeqCst);
                Ok("should never run".to_string())
            },
        )
        .await;

        assert!(
            matches!(outcome, GatedOutcome::Blocked(_)),
            "expected Blocked, got {outcome:?}"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "run must NEVER be invoked on a Tier-2 Block, even under yolo (D-13)"
        );

        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1, "Block must be audited exactly once");
        assert_eq!(entries[0].decision, "blocked");
        assert_eq!(entries[0].resolution, "dropped");
    }

    /// Warn + no approval gate wired → Denied + audited "denied"/"no_gate" (D-12).
    #[tokio::test]
    async fn warn_no_gate_denies_and_audits() {
        let guard = test_guard();
        let (dir, log) = test_log();

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("unused".to_string()) },
        )
        .await;

        assert!(
            matches!(outcome, GatedOutcome::Denied(_)),
            "expected Denied, got {outcome:?}"
        );
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "denied");
        assert_eq!(entries[0].resolution, "no_gate");
    }

    /// Warn + gate returns Approved → run invoked + audited decision "approved".
    #[tokio::test]
    async fn warn_approved_runs_and_audits() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("ran ok".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("ran ok".to_string()));
        assert!(gate.called.load(Ordering::SeqCst), "gate must be consulted");
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "approved");
        assert_eq!(entries[0].resolution, "operator");
    }

    /// Warn + gate returns Denied/TimedOut/AlreadyPending/GateUnavailable → Denied +
    /// audited decision "denied" for each outcome.
    #[tokio::test]
    async fn warn_gate_denied_variants_deny_and_audit() {
        for outcome_in in [
            ApprovalOutcome::Denied,
            ApprovalOutcome::TimedOut,
            ApprovalOutcome::AlreadyPending,
            ApprovalOutcome::GateUnavailable,
        ] {
            let guard = test_guard();
            let (dir, log) = test_log();
            let gate = FakeGate::new(outcome_in);

            let outcome = execute_gated_command(
                "terminal",
                "curl https://x",
                &guard,
                Some(&gate),
                &log,
                "sess1",
                "cli",
                "chat1",
                false,
                false,
                false,
                || async { Ok("should not run".to_string()) },
            )
            .await;

            assert!(
                matches!(outcome, GatedOutcome::Denied(_)),
                "outcome_in={outcome_in:?} expected Denied, got {outcome:?}"
            );
            let entries = read_entries(&dir);
            assert_eq!(entries.len(), 1, "outcome_in={outcome_in:?}");
            assert_eq!(entries[0].decision, "denied", "outcome_in={outcome_in:?}");
        }
    }

    /// Allow (not remote, forward_env empty) → run invoked AND audited exactly once
    /// with decision "allowed" — closes the routine-command audit gap (Pitfall 3).
    #[tokio::test]
    async fn allow_runs_and_audits_routine_command() {
        let guard = test_guard();
        let (dir, log) = test_log();

        let outcome = execute_gated_command(
            "terminal",
            "ls -la",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("file listing".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("file listing".to_string()));
        let entries = read_entries(&dir);
        assert_eq!(
            entries.len(),
            1,
            "an Allow command must produce exactly one audit entry (Pitfall 3 closed)"
        );
        assert_eq!(entries[0].decision, "allowed");
        assert_eq!(entries[0].resolution, "allowed");
    }

    /// D-08 load-bearing test: `is_remote_backend=true` forces even an `Allow` command
    /// through the approval gate (asserts the fake gate's `request_approval` was
    /// invoked) AND is audited.
    #[tokio::test]
    async fn allow_forced_through_gate_when_remote_backend() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "ls -la",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            true, // is_remote_backend
            false,
            || async { Ok("remote ls".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("remote ls".to_string()));
        assert!(
            gate.called.load(Ordering::SeqCst),
            "is_remote_backend=true must route an Allow command through the approval gate (D-08)"
        );
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "approved");
    }

    /// D-08: `forward_env_nonempty=true` forces even an `Allow` command through the
    /// approval gate, and is audited.
    #[tokio::test]
    async fn allow_forced_through_gate_when_forward_env_nonempty() {
        let guard = test_guard();
        let (dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Approved);

        let outcome = execute_gated_command(
            "terminal",
            "ls -la",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            true, // forward_env_nonempty
            || async { Ok("cred-forwarded ls".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("cred-forwarded ls".to_string()));
        assert!(
            gate.called.load(Ordering::SeqCst),
            "forward_env_nonempty=true must route an Allow command through the approval gate (D-08)"
        );
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, "approved");
    }

    /// yolo=true bypasses Tier-1 approval (still audited) but a Tier-2 Block under yolo
    /// still returns Blocked.
    #[tokio::test]
    async fn yolo_bypasses_tier1_but_not_block() {
        let guard = test_guard();
        let (dir, log) = test_log();

        // Tier-1 under yolo: runs without consulting a gate, audited "allowed"/"bypass".
        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            true,
            false,
            false,
            || async { Ok("ran under yolo".to_string()) },
        )
        .await;
        assert_eq!(outcome, GatedOutcome::Ran("ran under yolo".to_string()));

        // Tier-2 under yolo: still Blocked, run never invoked.
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();
        let outcome2 = execute_gated_command(
            "terminal",
            "rm -rf /",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            true,
            false,
            false,
            || async move {
                ran_clone.store(true, Ordering::SeqCst);
                Ok("never".to_string())
            },
        )
        .await;
        assert!(matches!(outcome2, GatedOutcome::Blocked(_)));
        assert!(!ran.load(Ordering::SeqCst));

        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 2, "both calls must each produce one audit entry");
        assert_eq!(entries[0].decision, "allowed");
        assert_eq!(entries[0].resolution, "bypass");
        assert_eq!(entries[1].decision, "blocked");
    }

    /// A `run` error maps to `Failed`, distinguishable from Denied/Blocked (WR-03
    /// semantics) — proves a command that ran-but-errored is not confused with one that
    /// never ran at all.
    #[tokio::test]
    async fn run_error_maps_to_failed() {
        let guard = test_guard();
        let (_dir, log) = test_log();

        let outcome = execute_gated_command(
            "terminal",
            "ls -la",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Err(anyhow::anyhow!("spawn failed")) },
        )
        .await;

        assert!(
            matches!(outcome, GatedOutcome::Failed(_)),
            "expected Failed, got {outcome:?}"
        );
    }

    /// D-11: `execute_code` passes an EMPTY opaque `classify_arg` — raw Python source
    /// never reaches `DangerousCommandGuardrail`'s shell-syntax `DANGEROUS_PATTERNS`
    /// matching, so a benign script classifies Allow (and is still audited).
    #[tokio::test]
    async fn execute_code_empty_classify_arg_allows_and_audits() {
        let guard = test_guard();
        let (dir, log) = test_log();

        let outcome = execute_gated_command(
            "execute_code",
            "",
            &guard,
            None,
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("python output".to_string()) },
        )
        .await;

        assert_eq!(outcome, GatedOutcome::Ran("python output".to_string()));
        let entries = read_entries(&dir);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool, "execute_code");
        assert_eq!(entries[0].decision, "allowed");
    }

    /// A Warn command routes to the approval branch, not silent pass-through — the
    /// generic `Allow | Warn` fast-path anti-pattern (RESEARCH.md Pitfall 2 / this
    /// plan's `prohibitions` entry) must never be reachable from this helper. Proven by
    /// `warn_no_gate_denies_and_audits` (Denied, not Ran, with no gate) and
    /// `warn_approved_runs_and_audits` (only runs after an explicit Approved outcome).
    #[tokio::test]
    async fn warn_never_silently_passes_through_without_gate_consultation() {
        let guard = test_guard();
        let (_dir, log) = test_log();
        let gate = FakeGate::new(ApprovalOutcome::Denied);

        let outcome = execute_gated_command(
            "terminal",
            "curl https://x",
            &guard,
            Some(&gate),
            &log,
            "sess1",
            "cli",
            "chat1",
            false,
            false,
            false,
            || async { Ok("must not run".to_string()) },
        )
        .await;

        assert!(gate.called.load(Ordering::SeqCst), "Warn must always consult the gate");
        assert!(
            matches!(outcome, GatedOutcome::Denied(_)),
            "Warn + explicit Denied must Deny, not silently pass through, got {outcome:?}"
        );
    }
}
