//! Worker subprocess spawn + env scrub (D-15 / D-16 / D-17 / D-18 / D-19 / D-28).
//!
//! This module owns two concerns:
//!
//! 1. **Env scrub** (`build_kanban_worker_env`) — builds the exact 7 safe
//!    system vars + up to 9 kanban env vars that the worker subprocess should
//!    receive. All other env vars from the dispatcher process are DROPPED.
//!    This is the security guarantee that prevents shell secrets
//!    (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.) from reaching the child
//!    process (T-36.3.7-03-01 / D-18 / INV-36.3.7-04).
//!
//! 2. **Subprocess spawn** (`spawn_worker`) — assembles the `ironhermes`
//!    command per D-15, calls `.env_clear()` before `.envs(...)`, redirects
//!    stdout/stderr to per-task log files (D-19), and returns the child PID.
//!
//! ## Env contract (D-17 / D-18)
//!
//! ```text
//! SAFE_SYSTEM_VARS (pass-through when present in caller env):
//!   PATH, HOME, USER, LANG, TERM, RUST_LOG, IRONHERMES_HOME
//!
//! 9 kanban env vars (always set):
//!   HERMES_KANBAN_TASK     = task.id
//!   HERMES_KANBAN_DB       = kanban_db_path()
//!   HERMES_KANBAN_BOARD    = "default"
//!   HERMES_KANBAN_WORKSPACES_ROOT = kanban_workspaces_root()
//!   HERMES_KANBAN_WORKSPACE = workspace
//!   HERMES_KANBAN_RUN_ID   = run.id
//!   HERMES_KANBAN_CLAIM_LOCK = run.claim_lock
//!   HERMES_PROFILE         = task.assignee
//!   HERMES_TENANT          = task.tenant (only when Some)
//! ```
//!
//! ## Spawn shape (D-15 / D-28)
//!
//! ```text
//! ironhermes --profile <assignee>
//!            chat -q "work kanban task <id>"
//! ```
//!
//! Static invariants checked by `tests/invariants_36_3_7.rs`:
//! - `build_kanban_worker_env` present (INV-36.3.7-02)
//! - `env_clear` present (INV-36.3.7-05)

use std::process::Stdio;

use tokio::process::Command;

use crate::error::{KanbanError, Result};
use crate::paths::{kanban_db_path, kanban_log_stderr, kanban_log_stdout, kanban_workspaces_root};
use crate::types::{Task, TaskRun};

// ---------------------------------------------------------------------------
// Safe system vars allowlist (D-18)
// ---------------------------------------------------------------------------

/// The 7 system env vars that are allowed to pass through to the worker
/// subprocess (D-18). Everything else is dropped by `env_clear()`.
///
/// Reuses the `build_safe_env()` allowlist concept from
/// `crates/ironhermes-exec/src/sandbox.rs` but adapted for the kanban worker
/// spawn (not the Python RPC sandbox).
pub const SAFE_SYSTEM_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TERM",
    "RUST_LOG",
    "IRONHERMES_HOME",
];

// ---------------------------------------------------------------------------
// build_kanban_worker_env
// ---------------------------------------------------------------------------

/// Build the env map for a kanban worker subprocess (D-17 / D-18).
///
/// Returns a `Vec<(String, String)>` suitable for
/// `tokio::process::Command::envs(...)`. The caller MUST call `.env_clear()`
/// on the `Command` before calling `.envs(...)` to ensure no other env vars
/// slip through (D-18 / INV-36.3.7-05).
///
/// # What is included
///
/// - Up to 7 safe system vars from [`SAFE_SYSTEM_VARS`] (only when present
///   in the current process env).
/// - 8 always-present kanban vars.
/// - `HERMES_TENANT` only when `task.tenant` is `Some`.
/// - `HERMES_KANBAN_TASK_SKILLS` only when `task.skills` is `Some` and
///   decodes to a non-empty `Vec<String>` (forward-compatible carrier for
///   skill extras; replaces the dropped `--skills` argv path, BUG-36.3.7-01).
///
/// # What is excluded
///
/// Every other env var, including `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
/// `GITHUB_TOKEN`, `*_SECRET`, `*_PASSWORD`, etc. (T-36.3.7-03-01).
pub fn build_kanban_worker_env(task: &Task, run: &TaskRun, workspace: &str) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();

    // Safe system pass-through (D-18): only include if present in caller env.
    for var in SAFE_SYSTEM_VARS {
        if let Ok(v) = std::env::var(var) {
            env.push((var.to_string(), v));
        }
    }

    // 9 kanban env vars (D-17).
    // HERMES_KANBAN_TASK — gates the 6 LLM tools in plan 04.
    env.push(("HERMES_KANBAN_TASK".into(), task.id.clone()));
    // HERMES_KANBAN_DB — path to kanban.db.
    env.push((
        "HERMES_KANBAN_DB".into(),
        kanban_db_path().to_string_lossy().into_owned(),
    ));
    // HERMES_KANBAN_BOARD — always "default" in v1 (D-03).
    env.push(("HERMES_KANBAN_BOARD".into(), "default".into()));
    // HERMES_KANBAN_WORKSPACES_ROOT — root for scratch workspaces (D-31).
    env.push((
        "HERMES_KANBAN_WORKSPACES_ROOT".into(),
        kanban_workspaces_root().to_string_lossy().into_owned(),
    ));
    // HERMES_KANBAN_WORKSPACE — the specific workspace for this task.
    env.push(("HERMES_KANBAN_WORKSPACE".into(), workspace.to_string()));
    // HERMES_KANBAN_RUN_ID — expected_run_id for protocol-terminator gate (D-22).
    env.push(("HERMES_KANBAN_RUN_ID".into(), run.id.clone()));
    // HERMES_KANBAN_CLAIM_LOCK — D-41 worker write gating.
    env.push(("HERMES_KANBAN_CLAIM_LOCK".into(), run.claim_lock.clone()));
    // HERMES_PROFILE — the assignee profile slug.
    env.push(("HERMES_PROFILE".into(), task.assignee.clone()));
    // HERMES_TENANT — only when task has a tenant (D-38).
    if let Some(ref t) = task.tenant {
        env.push(("HERMES_TENANT".into(), t.clone()));
    }
    // HERMES_KANBAN_TASK_SKILLS — forward-compatible carrier for task-level
    // skill extras (D-28 / BUG-36.3.7-01). Emitted only when task.skills is
    // Some AND decodes to a non-empty Vec<String>. Receiver-side consumption
    // is out of scope for 36.3.7.0 (see 36.3.7.0-01-SKILLS-EXTRAS-AUDIT.md).
    if let Some(ref skills_json) = task.skills {
        if let Ok(extras) = serde_json::from_str::<Vec<String>>(skills_json) {
            if !extras.is_empty() {
                env.push(("HERMES_KANBAN_TASK_SKILLS".into(), skills_json.clone()));
            }
        }
    }

    env
}

// ---------------------------------------------------------------------------
// spawn_worker
// ---------------------------------------------------------------------------

/// Spawn a kanban worker subprocess for a task.
///
/// Builds the `ironhermes --profile <P> chat -q "work kanban task <id>"`
/// command (D-15; D-28 superseded by 36.3.7.0 BUG-01 — extras now carried
/// via HERMES_KANBAN_TASK_SKILLS env), applies the env scrub via
/// `.env_clear()` + `.envs(build_kanban_worker_env(...))` (D-18 /
/// INV-36.3.7-05), and redirects stdout/stderr to per-task log files (D-19).
///
/// Returns the spawned child PID on success.
///
/// # Workspace creation (D-31)
///
/// If `workspace` starts with the `kanban_workspaces_root()` prefix (i.e.
/// it's a scratch workspace), this function creates the directory before
/// spawning. For `dir:<abs>` workspaces the directory must already exist.
/// Worktree workspaces are managed by the worker itself.
pub async fn spawn_worker(task: &Task, run: &TaskRun, workspace: &str) -> Result<u32> {
    let stdout_log = kanban_log_stdout(&task.id);
    let stderr_log = kanban_log_stderr(&task.id);

    // Create log parent directory (D-19).
    if let Some(parent) = stdout_log.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "create kanban log dir {}: {e}",
                parent.display()
            ))
        })?;
    }

    // Create scratch workspace dir if it is under workspaces root (D-31).
    let ws_root = kanban_workspaces_root();
    let ws_path = std::path::Path::new(workspace);
    if ws_path.starts_with(&ws_root) {
        std::fs::create_dir_all(ws_path).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "create kanban workspace {}: {e}",
                workspace
            ))
        })?;
    }

    // Open log files.
    let stdout_file = std::fs::File::create(&stdout_log).map_err(|e| {
        KanbanError::Other(anyhow::anyhow!(
            "create stdout log {}: {e}",
            stdout_log.display()
        ))
    })?;
    let stderr_file = std::fs::File::create(&stderr_log).map_err(|e| {
        KanbanError::Other(anyhow::anyhow!(
            "create stderr log {}: {e}",
            stderr_log.display()
        ))
    })?;

    // Assemble command (D-15):
    //   ironhermes --profile <assignee> chat -q "work kanban task <id>"
    //
    // No --skills flag: kanban tools are registered env-gated via
    // HERMES_KANBAN_TASK (Plan 05 / register_kanban_tools_if_applicable).
    // Skill extras ride HERMES_KANBAN_TASK_SKILLS env var (BUG-36.3.7-01).
    //
    // CRITICAL: .env_clear() BEFORE .envs(...) ensures no inherited shell
    // secrets reach the worker process (D-18 / INV-36.3.7-05).
    let child = Command::new("ironhermes")
        .arg("--profile")
        .arg(&task.assignee)
        .arg("chat")
        .arg("-q")
        .arg(format!("work kanban task {}", task.id))
        .env_clear()
        .envs(build_kanban_worker_env(task, run, workspace))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|e| {
            KanbanError::Other(anyhow::anyhow!(
                "spawn ironhermes worker for task {}: {e}",
                task.id
            ))
        })?;

    let pid = child.id().unwrap_or(0);
    tracing::info!(
        event = "spawned",
        task_id = %task.id,
        profile = %task.assignee,
        pid = pid,
        workspace = workspace,
    );
    Ok(pid)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Task, TaskRun};

    fn fake_task(id: &str, assignee: &str) -> Task {
        Task {
            id: id.to_string(),
            title: "test task".to_string(),
            body: None,
            assignee: assignee.to_string(),
            status: "ready".to_string(),
            priority: 0,
            tenant: None,
            workspace: None,
            skills: None,
            idempotency_key: None,
            claim_lock: None,
            claim_expires: None,
            current_run_id: None,
            consecutive_failures: 0,
            max_retries: None,
            max_runtime_seconds: None,
            scheduled_at: None,
            workflow_template_id: None,
            current_step_key: None,
            created_by: None,
            created_at: 1_700_000_000.0,
            started_at: None,
            ended_at: None,
        }
    }

    fn fake_run(id: &str, task_id: &str, claim_lock: &str) -> TaskRun {
        TaskRun {
            id: id.to_string(),
            task_id: task_id.to_string(),
            claim_lock: claim_lock.to_string(),
            claim_pid: None,
            started_at: 1_700_000_000.0,
            ended_at: None,
            outcome: None,
            summary: None,
            metadata: None,
            error: None,
            log_path: None,
        }
    }

    /// ENV scrub: a secret set in the test process must NOT appear in the
    /// env returned by build_kanban_worker_env (T-36.3.7-03-01 / D-18).
    #[test]
    fn build_kanban_worker_env_scrubs_secrets() {
        // Plant a sentinel secret in the test process env.
        // Safety: test-only; the test suite does not run in parallel
        // within a single process so this is safe.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-leak-test");
            std::env::set_var("ANTHROPIC_API_KEY", "ant-leak-test");
            std::env::set_var("GITHUB_TOKEN", "ghp_leak-test");
            std::env::set_var("MY_SECRET_KEY", "super-secret");
            std::env::set_var("MY_PASSWORD", "hunter2");
        }

        let task = fake_task("t_abc123", "alice");
        let run = fake_run("r_run001", "t_abc123", "host:123:uuid");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws");

        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

        assert!(
            !keys.contains(&"OPENAI_API_KEY"),
            "OPENAI_API_KEY must NOT appear in worker env"
        );
        assert!(
            !keys.contains(&"ANTHROPIC_API_KEY"),
            "ANTHROPIC_API_KEY must NOT appear in worker env"
        );
        assert!(
            !keys.contains(&"GITHUB_TOKEN"),
            "GITHUB_TOKEN must NOT appear in worker env"
        );
        assert!(
            !keys.contains(&"MY_SECRET_KEY"),
            "MY_SECRET_KEY must NOT appear in worker env"
        );
        assert!(
            !keys.contains(&"MY_PASSWORD"),
            "MY_PASSWORD must NOT appear in worker env"
        );
    }

    /// 8 always-present kanban vars must be in the output (HERMES_TENANT is
    /// optional, so only 8 are guaranteed when task.tenant is None).
    #[test]
    fn build_kanban_worker_env_includes_eight_kanban_vars() {
        let task = fake_task("t_def456", "bob");
        let run = fake_run("r_run002", "t_def456", "host:456:uuid2");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws2");

        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();

        for required in &[
            "HERMES_KANBAN_TASK",
            "HERMES_KANBAN_DB",
            "HERMES_KANBAN_BOARD",
            "HERMES_KANBAN_WORKSPACES_ROOT",
            "HERMES_KANBAN_WORKSPACE",
            "HERMES_KANBAN_RUN_ID",
            "HERMES_KANBAN_CLAIM_LOCK",
            "HERMES_PROFILE",
        ] {
            assert!(
                keys.contains(required),
                "required kanban var {required} missing from worker env"
            );
        }
    }

    /// HERMES_TENANT must only appear when task.tenant is Some.
    #[test]
    fn build_kanban_worker_env_includes_tenant_when_set() {
        let mut task = fake_task("t_ghi789", "carol");
        task.tenant = Some("acme".to_string());

        let run = fake_run("r_run003", "t_ghi789", "host:789:uuid3");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws3");

        let found = env.iter().find(|(k, _)| k == "HERMES_TENANT");
        assert!(found.is_some(), "HERMES_TENANT must be present when task.tenant is Some");
        assert_eq!(found.unwrap().1, "acme");
    }

    /// HERMES_KANBAN_TASK must equal task.id exactly.
    #[test]
    fn build_kanban_worker_env_task_id_matches() {
        let task = fake_task("t_specific_id", "diana");
        let run = fake_run("r_run004", "t_specific_id", "host:1:uuid4");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws4");

        let task_val = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_TASK")
            .map(|(_, v)| v.as_str());
        assert_eq!(task_val, Some("t_specific_id"));
    }

    /// HERMES_KANBAN_BOARD must always be "default" in v1.
    #[test]
    fn build_kanban_worker_env_board_is_default() {
        let task = fake_task("t_board_test", "evan");
        let run = fake_run("r_run005", "t_board_test", "host:2:uuid5");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws5");

        let board = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_BOARD")
            .map(|(_, v)| v.as_str());
        assert_eq!(board, Some("default"));
    }
}
