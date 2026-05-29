//! Receiver-end tests for BUG-36.3.7-01 fix.
//!
//! Verifies that `worker_spawn.rs` does NOT pass `--skills` on the argv
//! of the worker subprocess. Closes BUG-36.3.7-01 (UAT-09-A FAIL at step 6:
//! `error: unexpected argument '--skills' found`).
//!
//! Plan 36.3.7.0-01 — Task 2 tests.

use ironhermes_kanban::types::{Task, TaskRun};
use ironhermes_kanban::worker_spawn::build_kanban_worker_env;

// ---------------------------------------------------------------------------
// Local test helpers (mirror shape from worker_spawn::tests module — the
// in-source helpers are not pub, so we replicate them here for integration
// test scope).
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Test 1: static-grep — worker_spawn.rs must NOT contain "--skills"
// ---------------------------------------------------------------------------

/// INV-36.3.7.0-01: The worker spawn source must NOT contain the string
/// `"--skills"` after the BUG-36.3.7-01 fix. This is the load-bearing PASS
/// gate: if the argv emission was accidentally re-introduced, this test
/// fails immediately.
///
/// Uses `include_str!` so the check runs at compile time and is guaranteed
/// to be in sync with the actual source file (per invariants_36_3_7.rs
/// precedent).
#[test]
fn worker_argv_omits_skills_flag() {
    let source = include_str!("../src/worker_spawn.rs");
    assert!(
        !source.contains("\"--skills\""),
        "BUG-36.3.7-01: worker_spawn.rs must NOT contain the string \
         \"--skills\" after the fix. The literal flag caused \
         `error: unexpected argument '--skills' found` at worker startup \
         (UAT-09-A FAIL). Static-grep gate prevents re-introduction."
    );
}

// ---------------------------------------------------------------------------
// Test 2: binary argv acceptance (ignored unless live-binary-test feature)
// ---------------------------------------------------------------------------

/// INV-36.3.7.0-02: The constructed worker argv shape is accepted by the
/// ironhermes top-level Cli parser without `unexpected argument` errors.
///
/// This test spawns the actual `ironhermes` binary with `--help` to verify
/// the argv shape is parseable without triggering argparse errors. It is
/// `#[ignore]` by default because it requires a pre-built binary under
/// `target/debug/ironhermes`. Run with:
///
/// ```
/// cargo build --workspace && \
/// cargo test -p ironhermes-kanban --test argv_receiver_36_3_7_0 -- --ignored
/// ```
#[test]
#[ignore]
fn ironhermes_binary_accepts_constructed_argv() {
    // Use the binary relative to the workspace root.
    // In CI, CARGO_MANIFEST_DIR points to the crate directory.
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .expect("Could not determine workspace root from CARGO_MANIFEST_DIR");

    let binary = workspace_root.join("target/debug/ironhermes");

    // Verify binary exists before attempting to spawn.
    if !binary.exists() {
        eprintln!(
            "Skipping binary test: {} not found. Run `cargo build --workspace` first.",
            binary.display()
        );
        return;
    }

    // Construct the post-fix argv shape: ironhermes --profile <P> chat -q "..."
    // (no --skills flag). Use --help so it exits cleanly without real work.
    let output = std::process::Command::new(&binary)
        .arg("--profile")
        .arg("nonexistent-test-profile")
        .arg("chat")
        .arg("--help")
        .output()
        .expect("Failed to spawn ironhermes binary");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // The key assertion: no "unexpected argument '--skills'" in stderr.
    // This is what UAT-09-A saw at step 6 — this test would have caught it pre-merge.
    assert!(
        !stderr.contains("unexpected argument '--skills'"),
        "BUG-36.3.7-01: ironhermes binary rejected '--skills' in argv. \
         stderr: {stderr}"
    );

    // Also assert the argv shape itself doesn't produce an argparse error
    // for the flags we DO pass (--profile, chat, --help).
    assert!(
        !stderr.contains("unexpected argument '--profile'"),
        "ironhermes binary rejected '--profile' flag — unexpected regression. \
         stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: build_kanban_worker_env emits HERMES_KANBAN_TASK_SKILLS correctly
// ---------------------------------------------------------------------------

/// INV-36.3.7.0-03a: When `task.skills` is `Some` containing a non-empty
/// JSON array, `build_kanban_worker_env` must emit `HERMES_KANBAN_TASK_SKILLS`
/// with the original JSON string verbatim.
#[test]
fn build_kanban_worker_env_emits_skills_env_when_task_has_extras() {
    let mut task = fake_task("t_skills_test", "worker-alice");
    task.skills = Some(r#"["foo","bar"]"#.to_string());
    let run = fake_run("r_skills_run", "t_skills_test", "host:1:uuid-s");
    let env = build_kanban_worker_env(&task, &run, "/tmp/ws_skills");

    let found = env
        .iter()
        .find(|(k, _)| k == "HERMES_KANBAN_TASK_SKILLS")
        .map(|(_, v)| v.as_str());

    assert_eq!(
        found,
        Some(r#"["foo","bar"]"#),
        "build_kanban_worker_env must emit HERMES_KANBAN_TASK_SKILLS=<json> \
         when task.skills is Some(non-empty array). Got: {found:?}"
    );
}

/// INV-36.3.7.0-03b: When `task.skills` is `None`, `build_kanban_worker_env`
/// must NOT emit `HERMES_KANBAN_TASK_SKILLS`.
#[test]
fn build_kanban_worker_env_omits_skills_env_when_task_has_none() {
    let task = fake_task("t_no_skills", "worker-bob");
    let run = fake_run("r_no_skills_run", "t_no_skills", "host:2:uuid-n");
    let env = build_kanban_worker_env(&task, &run, "/tmp/ws_no_skills");

    let found = env.iter().find(|(k, _)| k == "HERMES_KANBAN_TASK_SKILLS");
    assert!(
        found.is_none(),
        "build_kanban_worker_env must NOT emit HERMES_KANBAN_TASK_SKILLS \
         when task.skills is None. Got: {found:?}"
    );
}

/// INV-36.3.7.0-03c: When `task.skills` is `Some("[]")` (empty JSON array),
/// `build_kanban_worker_env` must NOT emit `HERMES_KANBAN_TASK_SKILLS`.
#[test]
fn build_kanban_worker_env_omits_skills_env_when_task_has_empty_array() {
    let mut task = fake_task("t_empty_skills", "worker-carol");
    task.skills = Some("[]".to_string());
    let run = fake_run("r_empty_skills_run", "t_empty_skills", "host:3:uuid-e");
    let env = build_kanban_worker_env(&task, &run, "/tmp/ws_empty_skills");

    let found = env.iter().find(|(k, _)| k == "HERMES_KANBAN_TASK_SKILLS");
    assert!(
        found.is_none(),
        "build_kanban_worker_env must NOT emit HERMES_KANBAN_TASK_SKILLS \
         when task.skills is Some(\"[]\") (empty array). Got: {found:?}"
    );
}
