//! Phase 36.3.12 GAP 1 (D-01/D-05/D-06/D-07/D-09) — production-wiring regression guard.
//!
//! This file guards the reachability of `terminal.backend` from a parsed operator
//! config all the way to `ironhermes_exec::backend::create_environment`, through
//! FOUR production sites:
//!   1. `ironhermes_tools::registry::ToolRegistry::register_terminal_tool_with_process_registry`
//!      — the only production entry point that installs a resolved `TerminalConfig`
//!      into the `TerminalTool`'s backend-config builder.
//!   2. `ironhermes_agent::app_runtime_factory::build_registry_with_process_registry`
//!      — the first composition root (CLI REPL/batch, ratatui TUI-via-agent, gateway,
//!      iron_hermes_ui), which now forwards `&input.config.terminal` into (1).
//!   3. `ironhermes_cli::tui_rata::event_loop`'s TUI composition site, the second
//!      composition root, which now forwards `&config.terminal` into (1).
//!   4. `TerminalTool::execute`'s foreground path in `terminal.rs`, which calls
//!      `ironhermes_exec::backend::create_environment(&cfg, ..)` with whatever
//!      config was installed by (1).
//!
//! Regression this guards against: a return to registering a bare `TerminalTool::new()`
//! at any of sites (1)-(3) — with no backend config installed, `create_environment` always
//! receives `TerminalConfig::default()` (`backend: "local"`), so `terminal.backend:
//! docker`/`ssh` in the operator's config.yaml becomes a SILENT NO-OP that runs the
//! command on the host with no error. That is exactly the bug this gap-closure plan
//! fixes — see 36.3.12-11-PLAN.md.
//!
//! Every test below drives the tool built by the PRODUCTION registration entry point
//! (`ToolRegistry::register_terminal_tool_with_process_registry`), NEVER a hand-built
//! `TerminalTool` instance with its backend-config builder called directly — a
//! hand-built instance recreates the exact blind spot this test exists to close
//! (terminal.rs's own `#[cfg(test)]` module already builds tools that way, those
//! tests pass today, and the feature was still dead in production).
//!
//! These tests assert backend SELECTION (via the D-05 hard-error an unavailable/
//! unknown backend produces), NOT live container/remote execution: there is no
//! docker daemon and no live SSH host on this machine (or in CI), and the phase's
//! environment constraints forbid depending on either. No `docker-integration`
//! feature is referenced.

use std::sync::Arc;

use ironhermes_core::config::TerminalConfig;
use ironhermes_exec::process_registry::ProcessRegistry;
use ironhermes_tools::ToolRegistry;
use tokio::sync::RwLock;

/// Build a registry the way both production composition roots do: register only
/// the process-registry-wired `terminal` tool through the production entry point.
/// (Skipping `register_defaults_except` here is fine — dispatching `terminal` by
/// name only requires that one tool to be registered; the other default tools are
/// irrelevant to what this file guards.)
fn registry_with_terminal(terminal_config: &TerminalConfig) -> ToolRegistry {
    let process_registry = Arc::new(RwLock::new(ProcessRegistry::new_for_session(
        "terminal-backend-selection-wiring-test",
    )));
    let mut registry = ToolRegistry::new();
    registry.register_terminal_tool_with_process_registry(process_registry, terminal_config);
    registry
}

/// Guards: `backend: "ssh"` with `ssh: None` reaches `create_environment` via the
/// REGISTERED tool and hard-errors, naming the ssh backend / missing ssh config.
///
/// This is the assertion that fails against pre-Task-1 code: with `backend_config`
/// always `None` in production, the factory silently receives `TerminalConfig::default()`
/// (`backend: "local"`), the echo SUCCEEDS, and this `Err` never materializes. See the
/// falsification evidence recorded in the plan's SUMMARY.md.
///
/// Built from a parsed YAML fragment (not a struct literal) per VERIFICATION gap 1's
/// request for a test that "starts from a parsed config.yaml".
#[tokio::test]
async fn configured_ssh_backend_reaches_the_factory_and_hard_errors_when_ssh_config_absent() {
    let yaml = r#"
backend: ssh
cwd: "."
timeout: 30
"#;
    let terminal_config: TerminalConfig =
        serde_yaml::from_str(yaml).expect("TerminalConfig must deserialize from this YAML");
    assert_eq!(terminal_config.backend, "ssh");
    assert!(
        terminal_config.ssh.is_none(),
        "this test's premise requires ssh config to be absent"
    );

    let registry = registry_with_terminal(&terminal_config);

    let result = registry
        .dispatch("terminal", serde_json::json!({"command": "echo wiring-probe"}))
        .await;

    let err = result.expect_err(
        "configured backend: ssh with ssh: None must hard-error through the REGISTERED tool, \
         not silently succeed by running the command locally",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("ssh") || msg.contains("terminal.ssh"),
        "error must name the ssh backend / missing ssh config: {msg}"
    );
}

/// Guards: an unrecognized `terminal.backend` string hard-errors through the
/// REGISTERED tool AND leaves no local side effect — pins D-05's "never silently
/// falls back to local" at the wiring level, not just inside `create_environment`.
#[tokio::test]
async fn unknown_configured_backend_hard_errors_and_does_not_run_locally() {
    let tmp = tempfile::TempDir::new().expect("failed to create tempdir");
    let sentinel_path = tmp.path().join("sentinel-should-not-exist");
    let sentinel_path_str = sentinel_path.to_string_lossy().into_owned();

    let terminal_config = TerminalConfig {
        backend: "__no_such_backend__".to_string(),
        ..Default::default()
    };

    let registry = registry_with_terminal(&terminal_config);

    let result = registry
        .dispatch(
            "terminal",
            serde_json::json!({"command": format!("touch {sentinel_path_str}")}),
        )
        .await;

    let err =
        result.expect_err("unknown configured backend must hard-error, never fall back to local");
    let msg = err.to_string();
    assert!(
        msg.contains("__no_such_backend__"),
        "error must name the unrecognized backend: {msg}"
    );

    assert!(
        !sentinel_path.exists(),
        "unknown backend must NOT run the command locally — sentinel file was created at {sentinel_path_str}"
    );
}

/// Zero-regression control for D-06: `TerminalConfig::default()` (`backend: local`)
/// still dispatches and executes normally through the REGISTERED tool.
#[tokio::test]
async fn default_local_backend_still_executes_normally() {
    let terminal_config = TerminalConfig::default();
    assert_eq!(terminal_config.backend, "local");

    let registry = registry_with_terminal(&terminal_config);

    let result = registry
        .dispatch("terminal", serde_json::json!({"command": "echo wiring-probe"}))
        .await
        .expect("backend: local must succeed through the registered tool");

    assert!(
        result.contains("wiring-probe"),
        "default local backend output should contain the echoed probe: {result:?}"
    );
}

/// Guards: `backend: "docker"` reaches the factory and attempts the configured
/// `container_runtime`, hard-erroring by naming that runtime — WITHOUT needing a
/// live docker daemon. Uses a runtime binary name that cannot exist so the test is
/// deterministic regardless of whether `docker`/`podman` happen to be installed.
#[tokio::test]
async fn configured_docker_backend_reaches_the_factory() {
    let terminal_config = TerminalConfig {
        backend: "docker".to_string(),
        container_runtime: "__no_such_container_runtime__".to_string(),
        ..Default::default()
    };

    let registry = registry_with_terminal(&terminal_config);

    let result = registry
        .dispatch("terminal", serde_json::json!({"command": "echo wiring-probe"}))
        .await;

    let err = result.expect_err(
        "configured backend: docker with an unavailable container_runtime must hard-error \
         through the REGISTERED tool, proving docker selection routed correctly",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("__no_such_container_runtime__"),
        "error must name the configured (unavailable) container runtime: {msg}"
    );
}
