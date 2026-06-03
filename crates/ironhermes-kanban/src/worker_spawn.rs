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
//!   HERMES_KANBAN_DB       = board_db_path_for_slug(board_slug)
//!   HERMES_KANBAN_BOARD    = board_slug (the resolved board slug, Phase 36.3.7.9)
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
use crate::paths::{kanban_log_stderr, kanban_log_stdout, kanban_workspaces_root};
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
    "IRONHERMES_WORKER_BIN",  // Phase 36.3.7.13 D-02: forward-compat for recursive worker spawn
];

/// Phase 36.3.7.13 D-02: resolve the ironhermes worker binary.
///
/// Reads `IRONHERMES_WORKER_BIN` first (lets `cargo run` in a worktree pin
/// workers to the same binary without the `~/.local/bin/ironhermes` symlink
/// dance from Phase 36.3.7.12 UAT). Falls back to `"ironhermes"` (PATH
/// lookup — pre-36.3.7.13 behavior preserved bit-for-bit).
///
/// Callers: [`spawn_worker`] at the `Command::new` site (line 254 area).
/// The env var also rides the `SAFE_SYSTEM_VARS` allowlist above so that
/// recursive worker spawn (dispatcher → worker → sub-worker) carries the
/// override forward without additional wiring.
pub fn resolve_worker_bin() -> String {
    std::env::var("IRONHERMES_WORKER_BIN").unwrap_or_else(|_| "ironhermes".to_string())
}

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
pub fn build_kanban_worker_env(task: &Task, run: &TaskRun, workspace: &str, board_slug: &str) -> Vec<(String, String)> {
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
    // HERMES_KANBAN_DB — path to the board's kanban.db (routes to legacy path for "default").
    env.push((
        "HERMES_KANBAN_DB".into(),
        crate::paths::board_db_path_for_slug(board_slug).to_string_lossy().into_owned(),
    ));
    // HERMES_KANBAN_BOARD — the resolved board slug for this task's board (Phase 36.3.7.9).
    env.push(("HERMES_KANBAN_BOARD".into(), board_slug.to_string()));
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

    // Phase 36.3.7.12 (D-03 / D-06): goal-mode env pair.
    //
    // Emitted ONLY when the task opts into goal mode. When goal_mode=false
    // (every existing card) this block is a no-op and the 8-var env contract
    // (INV-36.3.7-07) is preserved bit-for-bit.
    //
    // The two new vars ride the per-task env list path downstream of
    // env_clear() + SAFE_SYSTEM_VARS allowlist (T-36.3.7.12-02-I01: they
    // carry only a feature flag and an integer budget — zero secret
    // material — so no new exfil vector exists).
    //
    // Defensive 0 → 20 coercion: a struct-literal caller that forgets to
    // set goal_max_turns lands `0`. We re-apply D-03's "20" default here so
    // the worker harness never sees a 0-budget signal that could mask as
    // "no budget enforcement." This mirrors the producer-level coercion
    // already in KanbanStore::create_task (Plan 01).
    if task.goal_mode {
        env.push(("HERMES_KANBAN_GOAL_MODE".into(), "1".into()));
        let budget = if task.goal_max_turns == 0 {
            20
        } else {
            task.goal_max_turns
        };
        env.push(("HERMES_KANBAN_GOAL_MAX_TURNS".into(), budget.to_string()));
    }

    env
}

// ---------------------------------------------------------------------------
// spawn_worker
// ---------------------------------------------------------------------------

/// Spawn a kanban worker subprocess for a task (single-board / back-compat version).
///
/// Always passes `"default"` as the board slug. Retained for callers that do
/// not yet carry a resolved board slug. Prefer [`spawn_worker_for_board`] in
/// new multi-board code (Phase 36.3.7.9).
pub async fn spawn_worker(task: &Task, run: &TaskRun, workspace: &str) -> Result<u32> {
    spawn_worker_for_board(task, run, workspace, "default").await
}

/// Spawn a kanban worker subprocess for a task, with an explicit board slug.
///
/// Builds the `ironhermes --profile <P> chat -q "work kanban task <id>"`
/// command (D-15; D-28 superseded by 36.3.7.0 BUG-01 — extras now carried
/// via HERMES_KANBAN_TASK_SKILLS env), applies the env scrub via
/// `.env_clear()` + `.envs(build_kanban_worker_env(..., board_slug))` (D-18 /
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
pub async fn spawn_worker_for_board(task: &Task, run: &TaskRun, workspace: &str, board_slug: &str) -> Result<u32> {
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
    // Phase 36.3.7.13 D-02: use resolve_worker_bin() so IRONHERMES_WORKER_BIN
    // can pin the subprocess to a specific binary (worktree cargo-run scenario).
    let child = Command::new(resolve_worker_bin())
        .arg("--profile")
        .arg(&task.assignee)
        .arg("chat")
        .arg("-q")
        .arg(format!("work kanban task {}", task.id))
        .env_clear()
        .envs(build_kanban_worker_env(task, run, workspace, board_slug))
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
            // Phase 36.3.7.12 — fake_task defaults: goal mode off, budget 20.
            goal_mode: false,
            goal_max_turns: 20,
            goal_turns_used: 0,
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
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws", "default");

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
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws2", "default");

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
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws3", "default");

        let found = env.iter().find(|(k, _)| k == "HERMES_TENANT");
        assert!(found.is_some(), "HERMES_TENANT must be present when task.tenant is Some");
        assert_eq!(found.unwrap().1, "acme");
    }

    /// HERMES_KANBAN_TASK must equal task.id exactly.
    #[test]
    fn build_kanban_worker_env_task_id_matches() {
        let task = fake_task("t_specific_id", "diana");
        let run = fake_run("r_run004", "t_specific_id", "host:1:uuid4");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws4", "default");

        let task_val = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_TASK")
            .map(|(_, v)| v.as_str());
        assert_eq!(task_val, Some("t_specific_id"));
    }

    /// HERMES_KANBAN_BOARD must equal "default" when board_slug is "default".
    #[test]
    fn build_kanban_worker_env_board_is_default() {
        let task = fake_task("t_board_test", "evan");
        let run = fake_run("r_run005", "t_board_test", "host:2:uuid5");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws5", "default");

        let board = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_BOARD")
            .map(|(_, v)| v.as_str());
        assert_eq!(board, Some("default"));
    }

    // -----------------------------------------------------------------------
    // Phase 36.3.7.12 Plan 02 Task 2 — goal-mode env append
    // -----------------------------------------------------------------------

    /// D-06 / INV-36.3.7-07: when `task.goal_mode == false` the existing
    /// 8-var env contract is preserved — NO `HERMES_KANBAN_GOAL_*` keys
    /// appear in the returned env list.
    #[test]
    fn goal_mode_off_env_unchanged() {
        let task = fake_task("t_goal_off", "alice");
        // fake_task defaults: goal_mode: false, goal_max_turns: 20.
        let run = fake_run("r_goal_off", "t_goal_off", "host:1:goal_off");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_goal_off", "default");

        let goal_keys: Vec<&str> = env
            .iter()
            .map(|(k, _)| k.as_str())
            .filter(|k| k.starts_with("HERMES_KANBAN_GOAL_"))
            .collect();

        assert!(
            goal_keys.is_empty(),
            "goal_mode=false must NOT emit any HERMES_KANBAN_GOAL_* keys; got {goal_keys:?}"
        );
    }

    /// D-03 / D-06: `task.goal_mode == true` with a non-default budget
    /// emits exactly two new env entries: `HERMES_KANBAN_GOAL_MODE=1` and
    /// `HERMES_KANBAN_GOAL_MAX_TURNS=<budget>`.
    #[test]
    fn goal_mode_on_env_has_budget() {
        let mut task = fake_task("t_goal_on", "bob");
        task.goal_mode = true;
        task.goal_max_turns = 7;
        let run = fake_run("r_goal_on", "t_goal_on", "host:2:goal_on");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_goal_on", "default");

        let mode = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_GOAL_MODE")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            mode,
            Some("1"),
            "HERMES_KANBAN_GOAL_MODE must equal \"1\" when goal_mode=true"
        );

        let budget = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_GOAL_MAX_TURNS")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            budget,
            Some("7"),
            "HERMES_KANBAN_GOAL_MAX_TURNS must equal goal_max_turns.to_string()"
        );

        // Exactly one of each — guards against accidental double-push.
        let mode_count = env
            .iter()
            .filter(|(k, _)| k == "HERMES_KANBAN_GOAL_MODE")
            .count();
        let budget_count = env
            .iter()
            .filter(|(k, _)| k == "HERMES_KANBAN_GOAL_MAX_TURNS")
            .count();
        assert_eq!(mode_count, 1, "HERMES_KANBAN_GOAL_MODE must appear exactly once");
        assert_eq!(
            budget_count, 1,
            "HERMES_KANBAN_GOAL_MAX_TURNS must appear exactly once"
        );
    }

    /// D-03 defensive coercion: `goal_mode == true` paired with a 0 budget
    /// (struct-literal default) must NOT emit "0" — the env value falls
    /// back to "20" (the D-03 documented default).
    #[test]
    fn goal_mode_on_with_zero_budget_falls_back_to_twenty() {
        let mut task = fake_task("t_goal_zero", "carol");
        task.goal_mode = true;
        task.goal_max_turns = 0; // defensive: caller forgot the budget
        let run = fake_run("r_goal_zero", "t_goal_zero", "host:3:goal_zero");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_goal_zero", "default");

        let budget = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_GOAL_MAX_TURNS")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            budget,
            Some("20"),
            "0 budget must defensively coerce to \"20\" in the emitted env"
        );
    }

    /// HERMES_KANBAN_BOARD and HERMES_KANBAN_DB must reflect the named slug
    /// when board_slug is a non-default value (Phase 36.3.7.9 D-04).
    #[test]
    fn build_kanban_worker_env_board_slug_propagates() {
        let task = fake_task("t_slug_test", "alice");
        let run = fake_run("r_run999", "t_slug_test", "host:2:uuid9");
        let env = build_kanban_worker_env(&task, &run, "/tmp/ws_slug", "atm10-server");

        let board = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_BOARD")
            .map(|(_, v)| v.as_str());
        assert_eq!(board, Some("atm10-server"));

        let db = env
            .iter()
            .find(|(k, _)| k == "HERMES_KANBAN_DB")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        assert!(
            db.contains("boards/atm10-server/kanban.db"),
            "HERMES_KANBAN_DB must contain 'boards/atm10-server/kanban.db', got: {db}"
        );
    }
}
