//! Phase 22.4.2.1 Plan 01 (D-03 / D-12) — behavioral tests for the /cron
//! slash command handler using a minimal fake CronJobReader trait-object impl.
//!
//! Driven through `dispatch(...)` so the slash-command routing, the
//! registry match arm, and the cmd_cron handler body are all exercised.
//! Tests follow the cmd_agents_and_stop.rs pattern exactly.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ironhermes_core::commands::context::{CommandContext, CronJobReader};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;

// =============================================================================
// Fakes
// =============================================================================

/// Minimal in-memory cron job representation for testing.
struct FakeCronJob {
    name: String,
    id: String,
}

/// Fake CronJobReader parameterized by a list of in-memory jobs.
/// Returns synthetic strings — does not depend on the real CronJob constructor.
struct FakeCronJobReader {
    jobs: Vec<FakeCronJob>,
}

impl CronJobReader for FakeCronJobReader {
    fn list_jobs_text(&self) -> String {
        if self.jobs.is_empty() {
            "Scheduled Jobs\n  No scheduled jobs.".to_string()
        } else {
            let names: Vec<&str> = self.jobs.iter().map(|j| j.name.as_str()).collect();
            format!("Scheduled Jobs\n  {}", names.join("\n  "))
        }
    }

    fn get_job_text(&self, id_or_name: &str) -> Option<String> {
        self.jobs
            .iter()
            .find(|j| j.id == id_or_name || j.name == id_or_name)
            .map(|j| format!("name={} id={}", j.name, j.id))
    }

    fn status_text(&self) -> String {
        format!("Total: {}", self.jobs.len())
    }

    fn pause_job(&self, id_or_name: &str) -> Result<String, String> {
        if self
            .jobs
            .iter()
            .any(|j| j.id == id_or_name || j.name == id_or_name)
        {
            Ok(format!("Paused: {}", id_or_name))
        } else {
            Err(format!("No cron job found: {}", id_or_name))
        }
    }

    fn resume_job(&self, id_or_name: &str) -> Result<String, String> {
        if self
            .jobs
            .iter()
            .any(|j| j.id == id_or_name || j.name == id_or_name)
        {
            Ok(format!("Resumed: {}", id_or_name))
        } else {
            Err(format!("No cron job found: {}", id_or_name))
        }
    }

    fn remove_job(&self, id_or_name: &str) -> Result<String, String> {
        if self
            .jobs
            .iter()
            .any(|j| j.id == id_or_name || j.name == id_or_name)
        {
            Ok(format!("Removed: {}", id_or_name))
        } else {
            Err(format!("No cron job found: {}", id_or_name))
        }
    }

    fn queue_run(&self, id_or_name: &str) -> Result<String, String> {
        if self
            .jobs
            .iter()
            .any(|j| j.id == id_or_name || j.name == id_or_name)
        {
            Ok(format!("Job queued for next tick: {}", id_or_name))
        } else {
            Err(format!("No cron job found: {}", id_or_name))
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn base_ctx() -> CommandContext {
    CommandContext::new(
        Platform::Local,
        "test-session".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
}

fn make_test_ctx_with_cron_store(jobs: Vec<FakeCronJob>) -> CommandContext {
    let store: Arc<dyn CronJobReader> = Arc::new(FakeCronJobReader { jobs });
    CommandContext::new(
        Platform::Local,
        "test-session".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
    .with_cron_store(store)
}

fn find_cmd(name: &str) -> CommandDef {
    build_registry()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("Command '{}' not found in registry", name))
}

fn router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

// =============================================================================
// Tests (GREEN after Task 2)
// =============================================================================

#[test]
fn cron_without_store_returns_not_configured() {
    let ctx = base_ctx();
    let cmd = find_cmd("cron");
    let r = router();
    let res = dispatch(&cmd, &[], &ctx, &r);
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("not configured"),
            "expected 'not configured' for None cron_store; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn cron_list_empty_store_says_no_scheduled_jobs() {
    let ctx = make_test_ctx_with_cron_store(vec![]);
    let cmd = find_cmd("cron");
    let r = router();
    let res = dispatch(&cmd, &[], &ctx, &r);
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("No scheduled jobs"),
            "empty store should say 'No scheduled jobs'; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn cron_list_one_job_contains_job_name() {
    let ctx = make_test_ctx_with_cron_store(vec![FakeCronJob {
        name: "foo".to_string(),
        id: "id-foo".to_string(),
    }]);
    let cmd = find_cmd("cron");
    let r = router();
    let res = dispatch(&cmd, &[], &ctx, &r);
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("foo"),
            "expected job name 'foo' in output; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn cron_status_returns_output() {
    let ctx = make_test_ctx_with_cron_store(vec![]);
    let cmd = find_cmd("cron");
    let r = router();
    let res = dispatch(&cmd, &["status"], &ctx, &r);
    match res {
        CommandResult::Output(_) => {}
        other => panic!("expected Output for /cron status, got {:?}", other),
    }
}

#[test]
fn cron_get_missing_id_returns_error() {
    let ctx = make_test_ctx_with_cron_store(vec![]);
    let cmd = find_cmd("cron");
    let r = router();
    let res = dispatch(&cmd, &["get", "no-such-id"], &ctx, &r);
    match res {
        CommandResult::Error(_) => {}
        other => panic!("expected Error for /cron get <missing>, got {:?}", other),
    }
}

#[test]
fn cron_unknown_subcommand_returns_error_with_typo_suggestion() {
    let ctx = make_test_ctx_with_cron_store(vec![]);
    let cmd = find_cmd("cron");
    let r = router();
    let res = dispatch(&cmd, &["lst"], &ctx, &r);
    match res {
        CommandResult::Error(s) => assert!(
            s.contains("list"),
            "expected typo suggestion 'list' for 'lst'; got: {}",
            s
        ),
        other => panic!("expected Error with typo suggestion, got {:?}", other),
    }
}
