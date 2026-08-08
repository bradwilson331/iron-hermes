//! Phase 36.3.7.13 Plan 02 Task 1 — bilateral worker-bin-resolution tests (F-02).
//!
//! Validates `resolve_worker_bin()` + SAFE_SYSTEM_VARS membership +
//! `build_kanban_worker_env` forward-carry for `IRONHERMES_WORKER_BIN`.
//!
//! Four scenarios:
//! 1. **env_wins** — with `IRONHERMES_WORKER_BIN` set, `resolve_worker_bin()`
//!    returns the env value verbatim.
//! 2. **fallback** — with `IRONHERMES_WORKER_BIN` unset, `resolve_worker_bin()`
//!    returns `"ironhermes"` literal (pre-36.3.7.13 PATH-lookup behavior).
//! 3. **safe_system_vars_membership** — `SAFE_SYSTEM_VARS` contains
//!    `"IRONHERMES_WORKER_BIN"` (forward-carry enabled for recursive worker spawn).
//! 4. **forward_carry** — with `IRONHERMES_WORKER_BIN` set in the parent process
//!    env, `build_kanban_worker_env` produces a child env that includes that entry.

use ironhermes_kanban::types::{Task, TaskRun};
use ironhermes_kanban::worker_spawn::{
    SAFE_SYSTEM_VARS, build_kanban_worker_env, resolve_worker_bin,
};

// ---------------------------------------------------------------------------
// ENV_LOCK — prevents concurrent env mutations from racing (STATE.md side-quest
// 4208bc28). All four tests share this mutex; each holds it for their full
// setup+execute+teardown lifetime.
// ---------------------------------------------------------------------------
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Minimal `Task` fixture sufficient to drive `build_kanban_worker_env`.
/// Only the fields touched by the env-build path need non-empty values.
fn make_task() -> Task {
    Task {
        id: "test-task-1".to_string(),
        title: "Test task".to_string(),
        body: None,
        assignee: "dev".to_string(),
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
        created_at: 0.0,
        started_at: None,
        ended_at: None,
        goal_mode: false,
        goal_max_turns: 0,
        goal_turns_used: 0,
        // Phase 36.3.7.13 D-B1: no toolset preset by default.
        goal_toolset: None,
        // Phase 46.4 D-10: no output_path set by default.
        output_path: None,
    }
}

/// Minimal `TaskRun` fixture.
fn make_run() -> TaskRun {
    TaskRun {
        id: "run-1".to_string(),
        task_id: "test-task-1".to_string(),
        claim_lock: "lock-abc".to_string(),
        claim_pid: None,
        started_at: 0.0,
        ended_at: None,
        outcome: None,
        summary: None,
        metadata: None,
        error: None,
        log_path: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1: IRONHERMES_WORKER_BIN env value is returned verbatim (F-02 env wins)
// ---------------------------------------------------------------------------
#[test]
fn env_wins() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let custom_path = "/path/to/my/ironhermes";
    unsafe { std::env::set_var("IRONHERMES_WORKER_BIN", custom_path) };

    let result = resolve_worker_bin();

    unsafe { std::env::remove_var("IRONHERMES_WORKER_BIN") };

    assert_eq!(
        result, custom_path,
        "resolve_worker_bin() must return IRONHERMES_WORKER_BIN value verbatim when set"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Fallback to "ironhermes" when IRONHERMES_WORKER_BIN is unset (F-02 fallback)
// ---------------------------------------------------------------------------
#[test]
fn fallback_to_ironhermes() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    unsafe { std::env::remove_var("IRONHERMES_WORKER_BIN") };

    let result = resolve_worker_bin();

    assert_eq!(
        result, "ironhermes",
        "resolve_worker_bin() must return \"ironhermes\" literal when IRONHERMES_WORKER_BIN is unset"
    );
}

// ---------------------------------------------------------------------------
// Test 3: SAFE_SYSTEM_VARS contains "IRONHERMES_WORKER_BIN" (F-02 SAFE_SYSTEM_VARS membership)
// ---------------------------------------------------------------------------
#[test]
fn safe_system_vars_contains_worker_bin() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    assert!(
        SAFE_SYSTEM_VARS.contains(&"IRONHERMES_WORKER_BIN"),
        "SAFE_SYSTEM_VARS must contain \"IRONHERMES_WORKER_BIN\" for forward-carry in recursive worker spawn"
    );
}

// ---------------------------------------------------------------------------
// Test 4: build_kanban_worker_env forward-carries IRONHERMES_WORKER_BIN (F-02 forward-carry)
// ---------------------------------------------------------------------------
#[test]
fn forward_carry_in_build_kanban_worker_env() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let custom_path = "/opt/ironhermes/bin/ironhermes";
    unsafe { std::env::set_var("IRONHERMES_WORKER_BIN", custom_path) };

    let task = make_task();
    let run = make_run();
    let env: Vec<(String, String)> = build_kanban_worker_env(&task, &run, "workspace-1", "default");

    unsafe { std::env::remove_var("IRONHERMES_WORKER_BIN") };

    let found = env.iter().find(|(k, _)| k == "IRONHERMES_WORKER_BIN");
    assert!(
        found.is_some(),
        "build_kanban_worker_env must include IRONHERMES_WORKER_BIN when set in parent env; got env keys: {:?}",
        env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>()
    );
    let (_, value) = found.unwrap();
    assert_eq!(
        value, custom_path,
        "build_kanban_worker_env must forward-carry IRONHERMES_WORKER_BIN value verbatim"
    );
}
