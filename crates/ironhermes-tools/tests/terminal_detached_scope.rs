//! Phase 58.1 Plan 01 (D-11/D-12) — the production-registered `terminal` tool
//! reaches the systemd scope launcher for detached commands, on every surface,
//! with no per-surface code.
//!
//! Drives the tool built by `register_terminal_tool_with_process_registry` (the
//! entry point the gateway, web server and CLI/TUI all use), never a
//! hand-built `TerminalTool`. A fake launcher stands in for `systemd-run`, so
//! this runs on macOS too.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ironhermes_core::config::TerminalConfig;
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_tools::terminal::TerminalTool;
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::RwLock;

/// Serialises tests: they mutate process-wide env vars.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Fake `systemd-run`: logs each argv element on its own line to a path baked
/// into the script (the scoped child gets a scrubbed env), drops args through
/// the first `--`, then execs the rest.
fn write_fake_launcher(dir: &Path) -> (PathBuf, PathBuf) {
    let script = dir.join("fake-systemd-run");
    let log = dir.join("fake-systemd-run.log");
    let body = format!(
        "#!/usr/bin/env bash\n\
         for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\n\
         while [ \"$#\" -gt 0 ]; do\n\
           if [ \"$1\" = \"--\" ]; then shift; break; fi\n\
           shift\n\
         done\n\
         exec \"$@\"\n",
        log.display()
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

#[tokio::test]
async fn registered_terminal_tool_reaches_scope_launcher() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (script, log) = write_fake_launcher(tmp.path());
    // SAFETY: serialised by ENV_LOCK; set at test start.
    unsafe {
        std::env::set_var("IRONHERMES_SYSTEMD_RUN", &script);
        std::env::remove_var("IRONHERMES_JOB_SCOPE");
    }

    let process_registry = Arc::new(RwLock::new(ProcessRegistry::new_for_session("58.1-scope")));
    let mut registry = ToolRegistry::new();
    registry
        .register_terminal_tool_with_process_registry(process_registry, &TerminalConfig::default());

    let out = registry
        .dispatch(
            "terminal",
            serde_json::json!({"command": "nohup sleep 1 >/dev/null 2>&1 & disown; echo launched"}),
        )
        .await
        .expect("terminal dispatch should succeed");

    assert!(out.contains("launched"), "{out:?}");
    assert!(
        out.contains("ironhermes-job-"),
        "scope hint missing: {out:?}"
    );
    let log = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        log.lines().any(|l| l.starts_with("--unit=ironhermes-job-")),
        "launcher argv lacks --unit=ironhermes-job-*: {log:?}"
    );
}

#[test]
fn description_documents_job_scopes() {
    let d = TerminalTool::new().description().to_string();
    for want in [
        "ironhermes-job-",
        "systemctl --user stop",
        "timeout",
        "background=true process is NOT scoped",
    ] {
        assert!(d.contains(want), "description lacks {want:?}: {d}");
    }
}
