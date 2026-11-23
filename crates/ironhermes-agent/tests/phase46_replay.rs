//! Phase 57 Plan 07 (D-15 / D-21 / D-23, SC-2): the Phase 46 Worker-delete replay, as a
//! behavioural test.
//!
//! The live Phase 46 incident was `execute_code` running
//! `subprocess.run(['wrangler', 'delete', 'hello-world-worker'])`: the Python-table Warn was
//! bypassed by yolo and denied on the web, and the Worker was deleted. This file drives the
//! REAL `ExecuteCodeTool` through the REAL `execute_gated_command` chokepoint with a FAKE
//! `wrangler` executable first on `PATH`. If the gate ever lets the source through, the fake
//! writes a marker file and the test fails on the marker, not on a claim.
//!
//! Pitfall 3 (research): nothing here lets tirith take the credit. The installed service is an
//! EMPTY `FakeBackend`, so the sidecar contributes no finding at all; only the
//! ironhermes-side D-23 rule (reconstructed argv matching a mutation verb) can be doing the
//! blocking.
//!
//! The positive control proves the harness would notice an execution: with the bypass level
//! active and a non-mutating `wrangler` call, the source runs and the marker exists.
//!
//! Process-global state (`PATH`, `IRONHERMES_HOME`, the installed service) is mutated, so the
//! tests serialize on [`ENV_LOCK`]. Run with `--test-threads=1`.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use ironhermes_core::config::DefconLevel;
use ironhermes_core::{AuditConfig, AuditLog, ExecConfig};
use ironhermes_hooks::{DangerousCommandGuardrail, GatePolicy, GatedOutcome, execute_gated_command};
use ironhermes_security::testing::{FakeBackend, clear_installed_for_tests, install_fake_for_tests};
use ironhermes_tools::execute_code::ExecuteCodeTool;
use ironhermes_tools::{Tool, ToolRegistry};
use std::sync::Arc;

/// Serializes tests that mutate process-global state.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Sets an env var and restores the previous value on drop.
struct ScopedEnv {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl ScopedEnv {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: every test in this file holds ENV_LOCK, so no other thread reads or writes
        // the environment while it is mutated.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        match &self.prev {
            // SAFETY: as in `set`; Drop runs while the test still holds ENV_LOCK.
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn level(n: u8) -> DefconLevel {
    DefconLevel::try_from(n).expect("1..=5 is a valid DEFCON level")
}

/// Writes an executable `wrangler` that records that it ran, regardless of its arguments.
fn write_fake_wrangler(dir: &Path, marker: &Path) {
    let script = format!("#!/bin/sh\necho \"$@\" > '{}'\n", marker.display());
    let bin = dir.join("wrangler");
    std::fs::write(&bin, script).expect("write fake wrangler");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake wrangler");
}

fn read_rows(audit_path: &Path) -> Vec<serde_json::Value> {
    let contents = std::fs::read_to_string(audit_path).unwrap_or_default();
    serde_json::Deserializer::from_str(&contents)
        .into_iter::<serde_json::Value>()
        .map(|r| r.expect("audit stream must parse"))
        .collect()
}

struct Replay {
    outcome: GatedOutcome,
    marker_exists: bool,
    rows: Vec<serde_json::Value>,
}

/// Runs `source` through `execute_gated_command` at `lvl`, with the real `ExecuteCodeTool`
/// as the `run` closure, an empty `FakeBackend` as the sidecar, and a fake `wrangler` first
/// on `PATH`.
async fn replay(source: &str, lvl: DefconLevel) -> Replay {
    let work = tempfile::tempdir().expect("tempdir");
    let bin_dir = work.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let marker: PathBuf = work.path().join("wrangler-ran.marker");
    write_fake_wrangler(&bin_dir, &marker);

    let _home = ScopedEnv::set("IRONHERMES_HOME", work.path().join("home"));
    let old_path = std::env::var("PATH").unwrap_or_default();
    let _path = ScopedEnv::set("PATH", format!("{}:{old_path}", bin_dir.display()));

    let service = install_fake_for_tests(FakeBackend::new());
    let tool = ExecuteCodeTool::new(Arc::new(ToolRegistry::new()), ExecConfig::default(), None);
    let guard = DangerousCommandGuardrail::new();
    let audit_path = work.path().join("audit.jsonl");
    let log = AuditLog::with_path(audit_path.clone(), AuditConfig::default());

    let code = source.to_string();
    let outcome = execute_gated_command(
        "execute_code",
        source,
        &guard,
        None,
        &log,
        "sess-46-replay",
        "test",
        "chat-46-replay",
        GatePolicy::fixed(lvl, Some(service)),
        false,
        false,
        || async move { tool.execute(serde_json::json!({ "code": code })).await },
    )
    .await;

    clear_installed_for_tests();
    Replay {
        outcome,
        marker_exists: marker.exists(),
        rows: read_rows(&audit_path),
    }
}

fn row_with<'a>(rows: &'a [serde_json::Value], kind: &str) -> Option<&'a serde_json::Value> {
    rows.iter().find(|r| r["kind"] == kind)
}

const LIST_ARGV: &str =
    "import subprocess\nsubprocess.run(['wrangler','delete','hello-world-worker'])";
const IMPORTLIB_VARIANT: &str = "import importlib\nm = importlib.import_module('subpro'+'cess')\n\
     m.run(['wrangler','delete','hello-world-worker'])";
const OS_SYSTEM_VARIANT: &str = "import os\nos.system('wrangler delete hello-world-worker')";

/// D-15 / D-23 / SC-2: the exact Phase 46 source never reaches `wrangler` at DEFCON 5, 4
/// and 1, with an empty sidecar, and the audit log records the Block and the rule.
#[tokio::test]
async fn phase46_replay_blocked_at_5_4_1() {
    let _guard = ENV_LOCK.lock().await;
    for n in [5u8, 4, 1] {
        let r = replay(LIST_ARGV, level(n)).await;
        assert!(
            matches!(r.outcome, GatedOutcome::Blocked(_)),
            "level {n}: expected Blocked, got {:?}",
            r.outcome
        );
        assert!(
            !r.marker_exists,
            "level {n}: the fake wrangler ran: the Phase 46 incident is reproducible"
        );
        let shell = row_with(&r.rows, "shell_command")
            .unwrap_or_else(|| panic!("level {n}: no shell_command row in {:?}", r.rows));
        assert_eq!(shell["decision"], "blocked", "level {n}: {shell}");
        let verdict = row_with(&r.rows, "security_verdict")
            .unwrap_or_else(|| panic!("level {n}: no security_verdict row in {:?}", r.rows));
        assert!(
            verdict.to_string().contains("mutation_command"),
            "level {n}: the security_verdict row must name the rule: {verdict}"
        );
        assert!(
            !verdict.to_string().contains("hello-world-worker"),
            "level {n}: the security_verdict row must not carry subject text: {verdict}"
        );
    }
}

/// D-23: the `importlib.import_module('subpro'+'cess')` evasion, where the module name is
/// never a literal, is reconstructed from the list argument and blocked at 5, 4 and 1.
#[tokio::test]
async fn phase46_importlib_variant_blocked() {
    let _guard = ENV_LOCK.lock().await;
    for n in [5u8, 4, 1] {
        let r = replay(IMPORTLIB_VARIANT, level(n)).await;
        assert!(
            matches!(r.outcome, GatedOutcome::Blocked(_)),
            "level {n}: expected Blocked, got {:?}",
            r.outcome
        );
        assert!(!r.marker_exists, "level {n}: the fake wrangler ran");
    }
}

/// D-23: the single-string form (`os.system('wrangler delete ...')`) is blocked too.
#[tokio::test]
async fn phase46_os_system_variant_blocked() {
    let _guard = ENV_LOCK.lock().await;
    let r = replay(OS_SYSTEM_VARIANT, level(5)).await;
    assert!(
        matches!(r.outcome, GatedOutcome::Blocked(_)),
        "expected Blocked, got {:?}",
        r.outcome
    );
    assert!(!r.marker_exists, "the fake wrangler ran");
}

/// Positive control: a NON-mutating `wrangler` call at level 5 (bypass active, no gate)
/// runs, and the marker exists. This proves the harness would catch an execution, so the
/// absent markers above mean something.
#[tokio::test]
async fn positive_control_harness_detects_execution() {
    let _guard = ENV_LOCK.lock().await;
    let r = replay(
        "import subprocess\nsubprocess.run(['wrangler','whoami'])",
        level(5),
    )
    .await;
    assert!(
        matches!(r.outcome, GatedOutcome::Ran(_)),
        "expected Ran at level 5, got {:?}",
        r.outcome
    );
    assert!(
        r.marker_exists,
        "the control must reach the fake wrangler, or the harness proves nothing"
    );
}
