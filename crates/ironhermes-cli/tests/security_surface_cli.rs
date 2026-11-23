//! SC-1 for the CLI surface (Phase 57 plan 13, D-09 / D-21): the REAL
//! `build_gated_terminal_intercept` closure — the one every CLI REPL, single-shot and
//! kanban-worker turn installs — screens through the booted sidecar service and decides
//! bypass from the session's LIVE DEFCON level, never from a flag captured when the
//! closure was built.
//!
//! The sidecar is the in-process `FakeBackend` (`install_fake_for_tests`); no binary is
//! spawned. The tests mutate two process-globals (`IRONHERMES_HOME` and the installed
//! service), so this crate's `--test-threads=1` rule applies to this file.
//!
//! Requires `test-support` for `io_gate::set_force_tty_for_test`, which pins the "no
//! operator at a terminal" path so the level-3 leg is deterministic whether or not a
//! human runs the suite from an interactive shell.

#![cfg(feature = "test-support")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ironhermes_cli::approval_gate::build_gated_terminal_intercept;
use ironhermes_cli::io_gate::set_force_tty_for_test;
use ironhermes_core::config::DefconLevel;
use ironhermes_core::defcon::session_defcon;
use ironhermes_core::{ApprovalsStore, AuditEntry, Config, ToolSchema};
use ironhermes_security::policy::Severity;
use ironhermes_security::testing::{
    FakeBackend, clear_installed_for_tests, install_fake_for_tests,
};
use ironhermes_tools::Tool;

/// Restores every process-global this file touches, including on a failed assertion.
struct Globals {
    prev_home: Option<String>,
    session: String,
}

impl Globals {
    fn new(home: &std::path::Path, session: &str) -> Self {
        let prev_home = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: this file runs with `--test-threads=1`, so nothing else touches the env.
        unsafe { std::env::set_var("IRONHERMES_HOME", home) };
        Self {
            prev_home,
            session: session.to_string(),
        }
    }
}

impl Drop for Globals {
    fn drop(&mut self) {
        match &self.prev_home {
            // SAFETY: as in `new`.
            Some(v) => unsafe { std::env::set_var("IRONHERMES_HOME", v) },
            None => unsafe { std::env::remove_var("IRONHERMES_HOME") },
        }
        session_defcon().forget(&self.session);
        set_force_tty_for_test(None);
        clear_installed_for_tests();
    }
}

/// A `terminal` tool that counts how many times it really ran.
struct CountingTerminal {
    runs: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for CountingTerminal {
    fn name(&self) -> &str {
        "terminal"
    }

    fn toolset(&self) -> &str {
        "terminal"
    }

    fn description(&self) -> &str {
        "counting stub terminal tool for the CLI surface tests"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "terminal",
            self.description(),
            serde_json::json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }),
        )
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok("stub-ran".to_string())
    }
}

fn shell_rows(home: &std::path::Path) -> Vec<AuditEntry> {
    let path = home.join("logs").join("audit.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<AuditEntry>(l).expect("audit row parses"))
        .filter(|r| r.kind == "shell_command")
        .collect()
}

/// The CLI builder over a counting stub tool, the way `run_single` / `run_chat` build it.
fn cli_intercept(
    session: &str,
    approvals_dir: &std::path::Path,
    runs: &Arc<AtomicUsize>,
) -> ironhermes_tools::registry::InterceptHandler {
    let tool: Arc<dyn Tool> = Arc::new(CountingTerminal { runs: runs.clone() });
    build_gated_terminal_intercept(
        Some(tool),
        Arc::new(Config::default()),
        session.to_string(),
        "cli-repl",
        session.to_string(),
        Arc::new(ApprovalsStore::with_path(
            approvals_dir.join("approvals.json"),
        )),
    )
}

/// A canary the sidecar blocks is Blocked through the real CLI intercept at every level
/// that is reachable from the CLI, including level 5: a Block is never bypassed (D-21),
/// and the run closure — the stub tool — never executes.
#[tokio::test]
async fn cli_gated_call_canary_blocked() {
    let home = tempfile::tempdir().expect("tempdir");
    let session = "s-cli-canary";
    let _globals = Globals::new(home.path(), session);
    install_fake_for_tests(
        FakeBackend::new().block_if_contains("CANARY-BLOCK", Severity::Critical),
    );
    set_force_tty_for_test(Some(false));

    let runs = Arc::new(AtomicUsize::new(0));
    let intercept = cli_intercept(session, home.path(), &runs);

    for level in [DefconLevel::THREE, DefconLevel::FIVE] {
        session_defcon().set_session_level(session, level);
        let result = intercept(serde_json::json!({ "command": "echo CANARY-BLOCK" }))
            .await
            .expect("the closure itself must not error");
        assert!(
            result.contains("Blocked"),
            "level {}: the canary must be Blocked, got: {result}",
            level.level()
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "level {}: a blocked canary must never execute",
            level.level()
        );
    }

    let rows = shell_rows(home.path());
    assert_eq!(rows.len(), 2, "one audited row per call: {rows:?}");
    assert!(
        rows.iter().all(|r| r.decision == "blocked"),
        "both calls are audited as blocked: {rows:?}"
    );
}

/// The same Tier-1 `curl` through one intercept follows the session's live level: at 5 it is
/// an audited bypass (`shell_command` row, resolution `bypass`); set to 3 afterwards it
/// reaches the CLI gate and, with no operator at a terminal, is denied (row decision
/// `denied`). One closure, built once: only the level changes.
#[tokio::test]
async fn cli_builder_follows_live_level() {
    let home = tempfile::tempdir().expect("tempdir");
    let session = "s-cli";
    let _globals = Globals::new(home.path(), session);
    install_fake_for_tests(FakeBackend::new());
    set_force_tty_for_test(Some(false));

    let runs = Arc::new(AtomicUsize::new(0));
    let intercept = cli_intercept(session, home.path(), &runs);
    let args = serde_json::json!({ "command": "curl https://example.com" });

    session_defcon().set_session_level(session, DefconLevel::FIVE);
    let bypassed = intercept(args.clone()).await.expect("intercept ok");
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "level 5 bypasses the Warn and runs it: {bypassed}"
    );
    let rows = shell_rows(home.path());
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].resolution, "bypass", "{rows:?}");

    session_defcon().set_session_level(session, DefconLevel::THREE);
    let denied = intercept(args).await.expect("intercept ok");
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "level 3 reaches the gate, which denies with no operator: {denied}"
    );
    assert!(denied.contains("Denied"), "expected Denied, got: {denied}");
    let rows = shell_rows(home.path());
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1].decision, "denied", "{rows:?}");
    assert_ne!(rows[1].resolution, "bypass", "{rows:?}");
}
