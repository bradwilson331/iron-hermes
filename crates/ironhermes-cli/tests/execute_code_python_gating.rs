//! Tracer test (Phase 54 Task 2, D-01/D-02/D-05): the real
//! `build_gated_execute_code_intercept` closure — not a bespoke re-assembly of
//! `execute_gated_command` plus a mock guard — is driven end to end: Python source
//! reaches `DangerousCommandGuardrail`, is classified, is routed through the approval
//! chain, and lands in `<IRONHERMES_HOME>/logs/audit.jsonl` as exactly one
//! `kind == "shell_command"` line.
//!
//! Per 54-RESEARCH.md Pitfall 1: a test that constructs `DangerousCommandGuardrail` +
//! `execute_gated_command` directly (mirroring `gated_exec.rs`'s own test style, or
//! `execute_code.rs`'s in-crate `execute_code_audit` test) proves the *function* works
//! but says nothing about whether the CLI's actual per-surface intercept closure
//! constructs its `classify_arg` correctly and passes the real Python source through.
//! This file drives `ironhermes_cli::approval_gate::build_gated_execute_code_intercept`
//! itself — the production entry point every local-surface `execute_code` call goes
//! through.
//!
//! RED phase (this commit): before Task 2's fix lands, `build_gated_execute_code_intercept`
//! still passes a literal `""` as `classify_arg` (D-11), so
//! `execute_code_intercept_classifies_and_audits_python` observes the stub tool
//! running under `yolo=true` (unconditionally true regardless of classification) but
//! the SECOND test — `execute_code_intercept_denies_dangerous_python_without_gate`,
//! `yolo=false` — currently RUNS the stub tool instead of denying it, because an empty
//! `classify_arg` classifies as `Allow`, not `Warn`, and an `Allow` command with no
//! forced-approval path never reaches the approval gate. That is the failing assertion
//! this RED commit records.
//!
//! Because this test mutates `IRONHERMES_HOME` (a process-global env var), the
//! `ironhermes-cli` crate must be run with `--test-threads=1` — the CLI/`iron_hermes_ui`
//! env-var-race property documented project-wide.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ironhermes_cli::approval_gate::build_gated_execute_code_intercept;
use ironhermes_core::config::DefconLevel;
use ironhermes_core::defcon::session_defcon;
use ironhermes_core::{ApprovalsStore, AuditEntry, Config, ToolSchema};
use ironhermes_tools::Tool;

/// Pins one session's live DEFCON level for a test and forgets the session on drop
/// (Phase 57 plan 13). The CLI builders take no yolo flag any more: they read the
/// session's level when a call is decided, so "yolo on" is level 5 for that session.
struct SessionLevel(String);

impl SessionLevel {
    fn set(session: &str, level: DefconLevel) -> Self {
        session_defcon().set_session_level(session, level);
        Self(session.to_string())
    }
}

impl Drop for SessionLevel {
    fn drop(&mut self) {
        session_defcon().forget(&self.0);
    }
}

/// RAII guard that sets `IRONHERMES_HOME` and restores the previous value on drop.
/// Mirrors `dispatch_profile_gate.rs`'s own `ScopedEnv` test helper in this crate.
struct ScopedHomeEnv {
    prev: Option<String>,
}

impl ScopedHomeEnv {
    fn set(dir: &std::path::Path) -> Self {
        let prev = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: single-threaded within this nextest-isolated process
        // (`--test-threads=1` is mandatory for this crate/file).
        unsafe { std::env::set_var("IRONHERMES_HOME", dir) };
        Self { prev }
    }
}

impl Drop for ScopedHomeEnv {
    fn drop(&mut self) {
        match &self.prev {
            // SAFETY: single-threaded within this nextest-isolated process.
            Some(v) => unsafe { std::env::set_var("IRONHERMES_HOME", v) },
            None => unsafe { std::env::remove_var("IRONHERMES_HOME") },
        }
    }
}

/// A stub `execute_code` tool that records whether it was invoked and returns a fixed
/// string — proves the guardrail's classification actually gates dispatch to the real
/// tool rather than being cosmetic.
struct StubExecuteCodeTool {
    invoked: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Tool for StubExecuteCodeTool {
    fn name(&self) -> &str {
        "execute_code"
    }

    fn toolset(&self) -> &str {
        "code"
    }

    fn description(&self) -> &str {
        "stub execute_code tool for gating tests"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "execute_code",
            self.description(),
            serde_json::json!({
                "type": "object",
                "properties": {"code": {"type": "string"}},
                "required": ["code"]
            }),
        )
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        self.invoked.store(true, Ordering::SeqCst);
        Ok("stub-output".to_string())
    }
}

fn read_entries(home: &std::path::Path) -> Vec<AuditEntry> {
    let path = home.join("logs").join("audit.jsonl");
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    contents
        .lines()
        .map(|l| serde_json::from_str(l).expect("valid AuditEntry JSON"))
        .collect()
}

/// A Python source matching a `PYTHON_DANGEROUS_PATTERNS` entry (`os.system`) that is
/// neither a D-23 mutation verb nor a D-13 reach marker: a pure floor Warn. It keeps the
/// two Phase 54 tests below exercising the bypass and the approval-denied paths.
///
/// Phase 57 plan 07: this replaced the ADR's own evidence line, which these tests used
/// until D-23 made it Critical (see `PHASE46_SOURCE` and the two Blocked tests at the end).
const DANGEROUS_PYTHON_SOURCE: &str = "import os\nos.system('ls')";

/// The ADR's own evidence line, list-argument form
/// (`subprocess.run(['wrangler', 'delete', ...])`). D-23: Critical at every level, so it
/// is Blocked even under yolo, where the Phase 54 version of the first test saw it bypassed.
const PHASE46_SOURCE: &str =
    "import subprocess\nsubprocess.run(['wrangler', 'delete', 'hello-world-worker'])";

/// Driving the real intercept builder with a Python source matching a dangerous
/// pattern, under `yolo=true`, produces exactly one `kind == "shell_command"` /
/// `tool == "execute_code"` line in `<IRONHERMES_HOME>/logs/audit.jsonl` — resolving
/// the D-05 audit contradiction with live evidence instead of a claim.
#[tokio::test]
async fn execute_code_intercept_classifies_and_audits_python() {
    let home = tempfile::tempdir().expect("tempdir");
    let _env = ScopedHomeEnv::set(home.path());
    // Level 5 for this session: a classified Warn still runs (bypass), but is audited.
    let _level = SessionLevel::set("sess-t2", DefconLevel::FIVE);

    let invoked = Arc::new(AtomicBool::new(false));
    let stub_tool: Arc<dyn Tool> = Arc::new(StubExecuteCodeTool {
        invoked: invoked.clone(),
    });

    let config = Arc::new(Config::default());
    let approvals_tmp = tempfile::tempdir().expect("approvals tempdir");
    let store = Arc::new(ApprovalsStore::with_path(
        approvals_tmp.path().join("approvals.json"),
    ));

    let intercept = build_gated_execute_code_intercept(
        Some(stub_tool),
        config,
        "sess-t2".to_string(),
        "cli",
        "chat-t2".to_string(),
        store,
    );

    let result = intercept(serde_json::json!({ "code": DANGEROUS_PYTHON_SOURCE }))
        .await
        .expect("intercept must not error");

    assert!(
        invoked.load(Ordering::SeqCst),
        "under yolo=true the classified command must still execute (bypass, audited): {result}"
    );

    let entries = read_entries(home.path());
    assert_eq!(
        entries.len(),
        1,
        "exactly one audit entry must be written for the execute_code call, got: {entries:?}"
    );
    assert_eq!(entries[0].kind, "shell_command");
    assert_eq!(entries[0].tool, "execute_code");
}

/// Driving the real intercept builder with the same dangerous Python source, under
/// `yolo=false` and no interactive operator (non-TTY under `cargo nextest`), produces a
/// denial rather than the stub tool's output — proving the classification actually
/// reaches the approval branch and is not merely cosmetic.
#[tokio::test]
async fn execute_code_intercept_denies_dangerous_python_without_gate() {
    let home = tempfile::tempdir().expect("tempdir");
    let _env = ScopedHomeEnv::set(home.path());

    let invoked = Arc::new(AtomicBool::new(false));
    let stub_tool: Arc<dyn Tool> = Arc::new(StubExecuteCodeTool {
        invoked: invoked.clone(),
    });

    let config = Arc::new(Config::default());
    let approvals_tmp = tempfile::tempdir().expect("approvals tempdir");
    let store = Arc::new(ApprovalsStore::with_path(
        approvals_tmp.path().join("approvals.json"),
    ));

    let intercept = build_gated_execute_code_intercept(
        Some(stub_tool),
        config,
        "sess-t2b".to_string(),
        "cli",
        "chat-t2b".to_string(),
        // Default level 3 — CliApprovalGate fails closed: no TTY under nextest
        store,
    );

    let result = intercept(serde_json::json!({ "code": DANGEROUS_PYTHON_SOURCE }))
        .await
        .expect("intercept must not error even when the command is denied");

    assert!(
        !invoked.load(Ordering::SeqCst),
        "a denied command must never reach the stub tool: {result}"
    );
    assert!(
        result.contains("Denied"),
        "expected a Denied GatedOutcome string, got: {result}"
    );
}

/// Builds the real intercept over a stub tool and runs `source` through it.
async fn run_intercept(source: &str, yolo: bool, tag: &str) -> (String, bool, Vec<AuditEntry>) {
    let home = tempfile::tempdir().expect("tempdir");
    let _env = ScopedHomeEnv::set(home.path());
    // `yolo` is level 5 for this session, level 3 (the process default) otherwise.
    let _level = yolo.then(|| SessionLevel::set(&format!("sess-{tag}"), DefconLevel::FIVE));

    let invoked = Arc::new(AtomicBool::new(false));
    let stub_tool: Arc<dyn Tool> = Arc::new(StubExecuteCodeTool {
        invoked: invoked.clone(),
    });
    let approvals_tmp = tempfile::tempdir().expect("approvals tempdir");
    let store = Arc::new(ApprovalsStore::with_path(
        approvals_tmp.path().join("approvals.json"),
    ));

    let intercept = build_gated_execute_code_intercept(
        Some(stub_tool),
        Arc::new(Config::default()),
        format!("sess-{tag}"),
        "cli",
        format!("chat-{tag}"),
        store,
    );
    let result = intercept(serde_json::json!({ "code": source }))
        .await
        .expect("intercept must not error");
    (
        result,
        invoked.load(Ordering::SeqCst),
        read_entries(home.path()),
    )
}

/// D-23 (Phase 57 plan 07): the Phase 46 source is Blocked through the real intercept
/// builder at the legacy `yolo=false` level (3), and the stub tool is never reached.
#[tokio::test]
async fn execute_code_intercept_blocks_phase46_source_at_default_level() {
    let (result, invoked, _entries) = run_intercept(PHASE46_SOURCE, false, "t2c").await;
    assert!(!invoked, "Blocked must not run: {result}");
    assert!(result.contains("Blocked"), "expected Blocked, got: {result}");
}

/// D-23 / D-07: the same source under `yolo=true` (level 5, bypass active) is STILL
/// Blocked. The Phase 54 version of the first test asserted the stub ran here; that
/// bypass is exactly the Phase 46 incident.
#[tokio::test]
async fn execute_code_intercept_blocks_phase46_source_even_under_yolo() {
    let (result, invoked, entries) = run_intercept(PHASE46_SOURCE, true, "t2d").await;
    assert!(!invoked, "Blocked must not run: {result}");
    assert!(result.contains("Blocked"), "expected Blocked, got: {result}");
    assert!(
        entries
            .iter()
            .any(|e| e.kind == "shell_command" && e.decision == "blocked"),
        "the Block must be audited: {entries:?}"
    );
}
