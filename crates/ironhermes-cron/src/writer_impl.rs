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
use std::fmt::Write as _;

use ironhermes_core::commands::context::{CronJobSpec, CronJobWriter, RawJobSpec};

use crate::blueprint::{fill_blueprint, find_blueprint};
use crate::job::{JobOrigin, JobState, ScheduleParsed};
use crate::parser::parse_schedule;
use crate::scanner::scan_cron_prompt;
use crate::store::{JobStore, NewJobSpec};

/// Shared refusal message for `stop_job_for_chat` (Phase 49.7 Plan 04,
/// T-49.7-04-02): returned identically whether the id is unknown, the job
/// has no origin, or the job belongs to a different chat, so a probe
/// cannot learn which case it hit — enumeration must not be cheaper than
/// guessing.
const STOP_REFUSED: &str = "No loop found with that id for this chat.";

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

/// Derive a job name from the first 48 characters of `prompt`, trimmed at a
/// char boundary so a multibyte character is never split mid-codepoint.
/// Phase 49.7 Plan 01.
fn job_name_from_prompt(prompt: &str) -> String {
    match prompt.char_indices().nth(48) {
        Some((byte_idx, _)) => prompt[..byte_idx].to_string(),
        None => prompt.to_string(),
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

    /// Phase 49.7 Plan 01 (D-01/D-06/D-07): `/loop <cadence> <prompt>` raw
    /// creation, bypassing the blueprint catalog entirely. Copies
    /// `create_job_from_blueprint`'s ordering exactly — scan, then parse,
    /// then persist (T-49.7-01-01) — and additionally refuses a cadence
    /// that resolves to a one-shot schedule, because `/loop` must always
    /// create a RECURRING job (D-06).
    fn create_raw_job(&self, spec: RawJobSpec) -> Result<String, String> {
        // Injection scan before persist — see create_job_from_blueprint's
        // parity note above. `scan_cron_prompt` is not called inside
        // `add_job_spec`; every caller is individually responsible for it,
        // and this call MUST stay ahead of `parse_schedule` (T-49.7-01-01).
        scan_cron_prompt(&spec.prompt)?;

        let schedule = parse_schedule(&spec.cadence).map_err(|e| e.to_string())?;
        if matches!(schedule, ScheduleParsed::Once { .. }) {
            return Err(format!(
                "/loop creates a RECURRING job, but {:?} resolved to a one-shot schedule. \
                 Use a recurring cadence instead — \"every 30m\", a bare duration like \
                 \"30m\", or a cron expression like \"0 9 * * 1\" — or use /cron for \
                 one-shot scheduling.",
                spec.cadence
            ));
        }
        let schedule_display = schedule_display_of(&schedule);

        let name = job_name_from_prompt(&spec.prompt);
        let deliver = if spec.origin_chat_id.is_some() {
            "origin"
        } else {
            "local"
        };

        let mut new_spec =
            NewJobSpec::new(name, spec.prompt, schedule, schedule_display, deliver);
        new_spec.repeat_times = spec.budget;
        new_spec.enabled_toolsets = spec.tools;
        // Leave script/workdir/base_url/no_agent/context_from/model/provider/
        // continuity at NewJobSpec::new's zero values — RawJobSpec carries no
        // fields for them, so there is nothing to assign (T-49.7-01-02).
        if let (Some(platform), Some(chat_id)) = (spec.origin_platform, spec.origin_chat_id) {
            new_spec.origin = Some(JobOrigin {
                platform,
                chat_id,
                chat_name: None,
                thread_id: spec.origin_thread_id,
            });
        }

        let mut store = JobStore::new().map_err(|e| format!("open cron store: {e}"))?;
        let job = store
            .add_job_spec(new_spec)
            .map_err(|e| format!("create job: {e}"))?;

        Ok(job.id)
    }

    /// Phase 49.7 Plan 04 (D-10): render jobs originating from `chat_id` as
    /// text for `/loop list`. Filters on `job.origin` being `Some` with a
    /// matching `chat_id`; an origin-less job (CLI-created) belongs to no
    /// chat and is skipped for every chat id. Matches
    /// `CronJobReaderImpl::list_jobs_text`'s (`display::format_job_list`)
    /// column idiom so `/loop list` and `/cron list` read alike, extended
    /// with the id and per-job budget state a chat-scoped listing needs.
    fn list_jobs_for_chat(&self, chat_id: &str) -> Result<String, String> {
        let store = JobStore::new().map_err(|e| format!("open cron store: {e}"))?;
        let mine: Vec<&crate::job::CronJob> = store
            .list_jobs()
            .iter()
            .filter(|j| j.origin.as_ref().is_some_and(|o| o.chat_id == chat_id))
            .collect();

        if mine.is_empty() {
            // Explicit empty-state sentence — a blank chat reply reads as a
            // failure, not as "you have no loops".
            return Ok("No loops running for this chat.".to_string());
        }

        let mut out = String::new();
        let _ = writeln!(out, "Your Loops");
        let _ = writeln!(out, "{}", "-".repeat(70));
        let _ = writeln!(
            out,
            "  {:<24} {:<16} {:<12} {:<10} PROMPT",
            "ID", "SCHEDULE", "STATUS", "BUDGET"
        );
        for job in &mine {
            let status_str = match job.state {
                JobState::Scheduled => {
                    if job.enabled {
                        "scheduled"
                    } else {
                        "disabled"
                    }
                }
                JobState::Paused => "paused",
                JobState::Completed => "completed",
            };
            let budget_str = match job.repeat.times {
                Some(times) => format!("{}/{}", job.repeat.completed, times),
                None => "unbounded".to_string(),
            };
            let _ = writeln!(
                out,
                "  {:<24} {:<16} {:<12} {:<10} {}",
                job.id,
                job.schedule_display,
                status_str,
                budget_str,
                job_name_from_prompt(&job.prompt)
            );
        }
        let _ = writeln!(out, "{}", "-".repeat(70));
        let _ = writeln!(out, "  {} loop(s)", mine.len());

        Ok(out.trim_end().to_string())
    }

    /// Phase 49.7 Plan 04 (D-09/D-10): stop (pause) the job `id_or_name` on
    /// behalf of `chat_id` for `/loop stop <id>`. The access check runs
    /// entirely BEFORE any mutation, and lives here in the impl rather than
    /// only in the handler, so a second caller of this seam cannot bypass
    /// it (T-49.7-04-01). Resolution reuses `JobStore::find_job`, the same
    /// id-first-then-name resolver `/cron pause` already uses, so `/loop
    /// stop` inherits its disambiguation rather than inventing a second
    /// one. The only mutating call is `JobStore::toggle_job(&id, false)` —
    /// the same method `CronJobReaderImpl::pause_job` wraps — which sets
    /// `enabled = false`, `state = JobState::Paused` and `paused_at =
    /// Some(now)` and leaves `next_run_at` untouched; this method does not
    /// assign any `CronJob` field directly, so the stop path and `/cron
    /// resume` semantics stay in agreement.
    fn stop_job_for_chat(&self, chat_id: &str, id_or_name: &str) -> Result<String, String> {
        let mut store = JobStore::new().map_err(|e| format!("open cron store: {e}"))?;

        let (id, name) = {
            let job = store
                .find_job(id_or_name)
                .ok_or_else(|| STOP_REFUSED.to_string())?;
            match &job.origin {
                Some(origin) if origin.chat_id == chat_id => (job.id.clone(), job.name.clone()),
                _ => return Err(STOP_REFUSED.to_string()),
            }
        };

        store.toggle_job(&id, false).map_err(|e| e.to_string())?;
        Ok(format!("Stopped: {}", name))
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
