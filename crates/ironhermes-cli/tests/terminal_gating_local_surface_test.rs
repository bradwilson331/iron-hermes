//! D-08/D-10 regression: the local-surface `terminal` tool-call path is BLOCKED for
//! Tier-2 catastrophic commands and AUDITED on every resolution (Allow included).
//!
//! This is the bug-proving test named in the phase objective: before Plan 07's Task
//! 2/3 wiring lands, the CLI/TUI `terminal_intercept: None` sites route the LLM's
//! `terminal` tool call directly to `TerminalTool::execute` with NO guardrail check
//! and NO audit entry — a live Tier-2 command (e.g. `mkfs.ext4 /dev/sda1`) would run
//! unguarded.
//!
//! RED phase (this task): references `ironhermes_cli::approval_gate::CliApprovalGate`,
//! which does NOT exist until Task 2 — `cargo test -p ironhermes-cli --test
//! terminal_gating_local_surface_test` must fail to COMPILE until Task 2 lands
//! (mirrors the established RED pattern in `approval_gate_test.rs`). A test that only
//! re-exercised `ironhermes_hooks::execute_gated_command` in isolation would pass
//! today even though the CLI's local surfaces are still completely unwired — that
//! would be a test that verifies its own assumptions, not the actual bug (recorded
//! failure mode in this repo). Requiring `CliApprovalGate` — the real type the CLI's
//! terminal_intercept closure constructs — ties this test to Task 2's actual
//! deliverable.
//!
//! The benign-command case constructs a REAL `CliApprovalGate` and passes it as the
//! `approval_gate` argument to `execute_gated_command`, proving it satisfies the
//! `ironhermes_core::ApprovalGate` trait object the chokepoint requires. It is never
//! actually consulted in that case (an `Allow`-classified command with no forced
//! approval never calls `request_approval`), so this exercises real production wiring
//! without touching stdin (which would be flaky/hangy under a TTY-attached test run).
//!
//! Two cases:
//! 1. A Tier-2 command is `Blocked`, `run` is NEVER invoked (proves the command did
//!    not execute locally), and an audit entry is written.
//! 2. A benign `Allow` command runs (gated through a real `CliApprovalGate`) AND
//!    produces an audit entry (the Allow-path audit floor — RESEARCH.md Pitfall 3).
//!
//! Per the plan's instruction, the catastrophic command literal is not embedded in any
//! acceptance-criteria grep — it is referenced via a local variable, and blocking is
//! proven by the guardrail's own `Block` classification, not by string-matching the
//! test's own command text.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ironhermes_cli::approval_gate::CliApprovalGate;
use ironhermes_core::{ApprovalsStore, AuditConfig};
use ironhermes_hooks::{DangerousCommandGuardrail, GatedOutcome, execute_gated_command};

/// Tempfile-backed `AuditLog` (mirrors Plan 03's own test helper) — the `TempDir`
/// guard is returned alongside the log so the directory outlives the assertions.
fn test_log() -> (tempfile::TempDir, ironhermes_core::AuditLog) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("audit.jsonl");
    let log = ironhermes_core::AuditLog::with_path(path, AuditConfig::default());
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

/// D-10 / T-36.3.12-19: a Tier-2 catastrophic `terminal` command issued via the
/// local-surface chokepoint construction is `Blocked` before any spawn, `run` is
/// NEVER invoked, and the resolution is audited.
#[tokio::test]
async fn tier2_local_terminal_command_is_blocked_never_runs_and_is_audited() {
    let guard = DangerousCommandGuardrail::from_config(
        &ironhermes_core::config::DangerousCommandsConfig::default(),
    );
    let (dir, log) = test_log();

    // A Tier-2 pattern the built-in DANGEROUS_PATTERNS classifies as Block
    // (mkfs — filesystem-format, catastrophic). Referenced only as a local
    // variable, not embedded in any grep-checked acceptance text.
    let catastrophic_command = "mkfs.ext4 /dev/sda1";

    let ran = Arc::new(AtomicBool::new(false));
    let ran_clone = ran.clone();

    let outcome = execute_gated_command(
        "terminal",
        catastrophic_command,
        &guard,
        None, // no approval gate needed — Block fires before any gate consultation
        &log,
        "sess-local-cli",
        "cli",
        "chat-local",
        false, // yolo=false
        false, // is_remote_backend=false — local surface
        false, // forward_env_nonempty=false
        || async move {
            // If this ever runs, the D-10 gap this phase closes has regressed.
            ran_clone.store(true, Ordering::SeqCst);
            Ok("SHOULD NEVER EXECUTE LOCALLY".to_string())
        },
    )
    .await;

    assert!(
        matches!(outcome, GatedOutcome::Blocked(_)),
        "expected the local-surface Tier-2 terminal command to be Blocked, got {outcome:?}"
    );
    assert!(
        !ran.load(Ordering::SeqCst),
        "the run closure must NEVER be invoked for a Tier-2 local terminal command (D-10 gap closure)"
    );

    let entries = read_entries(&dir);
    assert_eq!(
        entries.len(),
        1,
        "the blocked local terminal command must be audited exactly once (D-08)"
    );
    assert_eq!(entries[0].decision, "blocked");
    assert_eq!(entries[0].tool, "terminal");
}

/// Allow-path audit floor (RESEARCH.md Pitfall 3): a benign local `terminal` command
/// runs AND produces exactly one audit entry — proving routine commands are no longer
/// silently unaudited on the local surface. Uses a REAL `CliApprovalGate` (Task 2's
/// production type) as the approval gate, proving it satisfies `ApprovalGate` — it is
/// never actually consulted here since Allow-without-forced-approval never calls
/// `request_approval` (no stdin touched, no flake risk).
#[tokio::test]
async fn benign_local_terminal_command_runs_and_is_audited() {
    let guard = DangerousCommandGuardrail::from_config(
        &ironhermes_core::config::DangerousCommandsConfig::default(),
    );
    let (dir, log) = test_log();
    let approvals_tmp = tempfile::tempdir().expect("approvals tempdir");
    let store = ApprovalsStore::with_path(approvals_tmp.path().join("approvals.json"));
    let cli_gate = CliApprovalGate::new(store, false);

    let outcome = execute_gated_command(
        "terminal",
        "ls -la /tmp",
        &guard,
        Some(&cli_gate),
        &log,
        "sess-local-cli",
        "cli",
        "chat-local",
        false,
        false,
        false,
        || async { Ok("total 0".to_string()) },
    )
    .await;

    assert_eq!(
        outcome,
        GatedOutcome::Ran("total 0".to_string()),
        "a benign command must run on the local surface"
    );

    let entries = read_entries(&dir);
    assert_eq!(
        entries.len(),
        1,
        "an Allow-classified local terminal command must still produce exactly one audit entry (Pitfall 3 closure)"
    );
    assert_eq!(entries[0].decision, "allowed");
    assert_eq!(entries[0].tool, "terminal");
}
