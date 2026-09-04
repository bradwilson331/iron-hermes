//! Phase 49.5 Plan 05: production `CronJobWriter` impl.
//!
//! Lives in ironhermes-cron (NOT ironhermes-cli) because the gateway needs
//! to construct it at `CommandContext` build-time and ironhermes-cli already
//! depends on ironhermes-gateway (the reverse direction would be circular).
//! ironhermes-cron already depends on ironhermes-core, and ironhermes-gateway
//! already depends on ironhermes-cron, so this direction introduces no cycle
//! — the same topology `KanbanStoreWriterImpl` uses in ironhermes-kanban.
//!
//! `create_job_from_blueprint` opens a fresh `JobStore` per call and drops
//! it after, exactly as `KanbanStoreWriterImpl` opens a fresh store per
//! method — no shared mutable state at the impl layer, so the trait object
//! is safe to clone into multiple contexts.
//!
//! Every failure is mapped to a `String` the chat surface can print. This
//! code path runs inside a live gateway session, so it must never `unwrap`,
//! `expect`, or `panic!` — a panic here kills the session (T-49.5-05-07).

use std::collections::BTreeMap;

use ironhermes_core::commands::context::{CronJobSpec, CronJobWriter};

use crate::blueprint::{fill_blueprint, find_blueprint};
use crate::job::ScheduleParsed;
use crate::parser::parse_schedule;
use crate::scanner::scan_cron_prompt;
use crate::store::{JobStore, NewJobSpec};

/// Production impl that opens the default cron job store per call.
/// Phase 49.5 Plan 05.
pub struct CronJobWriterImpl;

impl CronJobWriterImpl {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CronJobWriterImpl {
    fn default() -> Self {
        Self::new()
    }
}

/// Render a `ScheduleParsed`'s human-readable display string. Mirrors the
/// `schedule_display_of` precedent in `iron_hermes_ui::server::schedules_api`
/// (a per-crate helper, not shared, since that one is `pub(crate)` in a
/// different crate).
fn schedule_display_of(schedule: &ScheduleParsed) -> String {
    match schedule {
        ScheduleParsed::Once { display, .. } => display.clone(),
        ScheduleParsed::Interval { display, .. } => display.clone(),
        ScheduleParsed::Cron { display, .. } => display.clone(),
    }
}

impl CronJobWriter for CronJobWriterImpl {
    fn create_job_from_blueprint(&self, spec: CronJobSpec) -> Result<String, String> {
        let blueprint = find_blueprint(&spec.blueprint_key)
            .ok_or_else(|| format!("unknown blueprint key: {:?}", spec.blueprint_key))?;

        let values: BTreeMap<String, String> = spec.values.into_iter().collect();
        let filled = fill_blueprint(blueprint, &values).map_err(|e| e.to_string())?;

        // Injection scan before persist — parity with every other job-creation
        // path (ironhermes-cli/src/cron.rs, ironhermes-tools/src/cronjob_tool.rs,
        // restgw routes/jobs.rs, schedules_api.rs). Slot values substitute
        // verbatim into prompt_template, so a crafted value would otherwise be
        // written durably to jobs.json and only rejected at tick time.
        scan_cron_prompt(&filled.prompt)?;

        let schedule = parse_schedule(&filled.schedule_expr).map_err(|e| e.to_string())?;
        let schedule_display = schedule_display_of(&schedule);

        let mut new_spec =
            NewJobSpec::new(filled.name, filled.prompt, schedule, schedule_display, filled.deliver);
        new_spec.skills = filled.skills;
        // No advanced fields set — FilledBlueprint carries none of them, and
        // NewJobSpec::new already zeroes script/no_agent/workdir/base_url.

        let mut store = JobStore::new().map_err(|e| format!("open cron store: {e}"))?;
        let job = store
            .add_job_spec(new_spec)
            .map_err(|e| format!("create job: {e}"))?;

        Ok(job.id)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod cron_job_writer_tests {
    use super::*;
    use tempfile::TempDir;

    fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
        crate::test_env_lock().blocking_lock()
    }

    /// Isolates `IRONHERMES_HOME` at `tmp` for the duration of `body`, then
    /// restores the environment. Holds `env_lock()` across the whole call so
    /// concurrent tests in other modules never interleave their env writes.
    fn with_isolated_home<T>(body: impl FnOnce(&std::path::Path) -> T) -> T {
        let _guard = env_lock();
        let tmp = TempDir::new().expect("tempdir");
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }
        let result = body(tmp.path());
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
        result
    }

    #[test]
    fn create_from_blueprint_writes_a_readable_job() {
        with_isolated_home(|home| {
            let writer = CronJobWriterImpl::new();
            let spec = CronJobSpec {
                blueprint_key: "morning-brief".to_string(),
                values: vec![("time".to_string(), "08:00".to_string())],
            };

            let job_id = writer
                .create_job_from_blueprint(spec)
                .expect("create_job_from_blueprint should succeed");
            assert!(!job_id.is_empty());

            let store = JobStore::open(home.join("cron")).expect("reopen store");
            let found = store.jobs.iter().find(|j| j.id == job_id);
            assert!(found.is_some(), "created job must be readable back from disk");
        });
    }

    #[test]
    fn create_from_blueprint_returns_err_for_unknown_key() {
        with_isolated_home(|home| {
            let writer = CronJobWriterImpl::new();
            let spec = CronJobSpec {
                blueprint_key: "not-a-real-blueprint".to_string(),
                values: vec![],
            };

            let err = writer
                .create_job_from_blueprint(spec)
                .expect_err("unknown key must error");
            assert!(
                err.contains("not-a-real-blueprint"),
                "error must name the unknown key: {err}"
            );

            let store = JobStore::open(home.join("cron")).expect("reopen store");
            assert!(store.jobs.is_empty(), "no job should be written on error");
        });
    }

    #[test]
    fn create_from_blueprint_returns_err_for_invalid_slot_value() {
        with_isolated_home(|home| {
            let writer = CronJobWriterImpl::new();
            let spec = CronJobSpec {
                blueprint_key: "morning-brief".to_string(),
                values: vec![("time".to_string(), "not-a-time".to_string())],
            };

            writer
                .create_job_from_blueprint(spec)
                .expect_err("malformed time must error");

            let store = JobStore::open(home.join("cron")).expect("reopen store");
            assert!(store.jobs.is_empty(), "no job should be written on error");
        });
    }

    #[test]
    fn created_job_carries_no_script_no_agent_workdir_or_base_url() {
        with_isolated_home(|home| {
            let writer = CronJobWriterImpl::new();
            let spec = CronJobSpec {
                blueprint_key: "morning-brief".to_string(),
                values: vec![("time".to_string(), "08:00".to_string())],
            };

            let job_id = writer
                .create_job_from_blueprint(spec)
                .expect("create_job_from_blueprint should succeed");

            let store = JobStore::open(home.join("cron")).expect("reopen store");
            let job = store
                .jobs
                .iter()
                .find(|j| j.id == job_id)
                .expect("created job must be present");
            assert!(job.script.is_none());
            assert!(job.workdir.is_none());
            assert!(job.base_url.is_none());
            assert!(!job.no_agent);
        });
    }

    #[test]
    fn two_creations_yield_distinct_ids() {
        with_isolated_home(|home| {
            let writer = CronJobWriterImpl::new();
            let make_spec = || CronJobSpec {
                blueprint_key: "morning-brief".to_string(),
                values: vec![("time".to_string(), "08:00".to_string())],
            };

            let id1 = writer
                .create_job_from_blueprint(make_spec())
                .expect("first create should succeed");
            let id2 = writer
                .create_job_from_blueprint(make_spec())
                .expect("second create should succeed");

            assert_ne!(id1, id2);

            let store = JobStore::open(home.join("cron")).expect("reopen store");
            assert_eq!(store.jobs.len(), 2);
        });
    }
}
