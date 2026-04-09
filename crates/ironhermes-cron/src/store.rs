use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ironhermes_core::get_hermes_home;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::job::{CronJob, JobOrigin, JobState, RepeatConfig, ScheduleParsed};
use crate::parser::{compute_next_run, parse_schedule};

// ---------------------------------------------------------------------------
// LegacyCronJob — matches the OLD CronJob shape for migration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyCronJob {
    pub id: String,
    pub name: String,
    pub agent_input: String,
    pub schedule: String,
    pub deliver: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub next_run: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub last_output: Option<String>,
}

impl From<LegacyCronJob> for CronJob {
    fn from(legacy: LegacyCronJob) -> Self {
        let schedule_str = legacy.schedule.clone();
        let schedule = parse_schedule(&schedule_str).unwrap_or_else(|_| ScheduleParsed::Cron {
            expr: schedule_str.clone(),
            display: schedule_str.clone(),
        });
        let schedule_display = match &schedule {
            ScheduleParsed::Once { display, .. } => display.clone(),
            ScheduleParsed::Interval { display, .. } => display.clone(),
            ScheduleParsed::Cron { display, .. } => display.clone(),
        };

        CronJob {
            id: legacy.id,
            name: legacy.name,
            prompt: legacy.agent_input,
            skills: vec![],
            schedule,
            schedule_display,
            repeat: RepeatConfig::default(),
            enabled: legacy.enabled,
            state: JobState::Scheduled,
            paused_at: None,
            paused_reason: None,
            deliver: legacy.deliver,
            origin: None,
            created_at: legacy.created_at,
            next_run_at: Some(legacy.next_run),
            last_run_at: legacy.last_run,
            last_status: legacy.last_output.as_ref().map(|_| "ok".to_string()),
            last_error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// JobUpdate — partial update struct
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct JobUpdate {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub deliver: Option<String>,
    pub schedule: Option<ScheduleParsed>,
    pub schedule_display: Option<String>,
    pub skills: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// JobStore
// ---------------------------------------------------------------------------

/// Persists cron jobs as JSON at `{dir}/jobs.json`.
pub struct JobStore {
    path: PathBuf,
    pub jobs: Vec<CronJob>,
    pub grace_seconds: i64,
}

impl JobStore {
    /// Load (or initialise) the job store from the default hermes home directory.
    pub fn new() -> Result<Self> {
        Self::open(get_hermes_home().join("cron"))
    }

    /// Load (or initialise) the job store at a specific directory.
    pub fn open(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create cron directory: {}", dir.display()))?;

        let path = dir.join("jobs.json");
        let jobs = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;

            // Try new format first, then legacy
            if let Ok(jobs) = serde_json::from_str::<Vec<CronJob>>(&raw) {
                debug!(
                    "JobStore loaded {} job(s) from {} (new format)",
                    jobs.len(),
                    path.display()
                );
                jobs
            } else if let Ok(legacy_jobs) = serde_json::from_str::<Vec<LegacyCronJob>>(&raw) {
                info!(
                    "Migrating {} legacy job(s) from {}",
                    legacy_jobs.len(),
                    path.display()
                );
                let jobs: Vec<CronJob> = legacy_jobs.into_iter().map(CronJob::from).collect();
                // Save migrated jobs immediately
                let tmp_path = path.with_extension("json.tmp");
                let json = serde_json::to_string_pretty(&jobs).context("serialize migrated")?;
                {
                    let mut f = fs::File::create(&tmp_path)
                        .with_context(|| format!("create tmp: {}", tmp_path.display()))?;
                    f.write_all(json.as_bytes())?;
                    f.flush()?;
                }
                fs::rename(&tmp_path, &path)?;
                jobs
            } else {
                warn!(
                    "Could not parse {} as new or legacy format, starting empty",
                    path.display()
                );
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Ok(Self {
            path,
            jobs,
            grace_seconds: 3600,
        })
    }

    /// Create a new job, persist it, and return a clone of the created record.
    #[allow(clippy::too_many_arguments)]
    pub fn add_job(
        &mut self,
        name: impl Into<String>,
        prompt: impl Into<String>,
        schedule: ScheduleParsed,
        schedule_display: impl Into<String>,
        deliver: impl Into<String>,
        skills: Vec<String>,
        origin: Option<JobOrigin>,
    ) -> Result<CronJob> {
        let now = Utc::now();
        let next_run_at = compute_next_run(&schedule, now)?;

        // Auto-set repeat.times=Some(1) for Once kind
        let repeat = match &schedule {
            ScheduleParsed::Once { .. } => RepeatConfig {
                times: Some(1),
                completed: 0,
            },
            _ => RepeatConfig::default(),
        };

        let display = schedule_display.into();
        let job = CronJob {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            prompt: prompt.into(),
            skills,
            schedule,
            schedule_display: display,
            repeat,
            enabled: true,
            state: JobState::Scheduled,
            paused_at: None,
            paused_reason: None,
            deliver: deliver.into(),
            origin,
            created_at: now,
            next_run_at,
            last_run_at: None,
            last_status: None,
            last_error: None,
        };

        info!("Adding cron job '{}' (id={})", job.name, job.id);
        self.jobs.push(job.clone());
        self.save()?;
        Ok(job)
    }

    /// Partially update a job by id.
    pub fn update_job(&mut self, id: &str, updates: JobUpdate) -> Result<CronJob> {
        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| anyhow::anyhow!("job not found: {id}"))?;

        if let Some(name) = updates.name {
            job.name = name;
        }
        if let Some(prompt) = updates.prompt {
            job.prompt = prompt;
        }
        if let Some(deliver) = updates.deliver {
            job.deliver = deliver;
        }
        if let Some(skills) = updates.skills {
            job.skills = skills;
        }
        if let Some(schedule) = updates.schedule {
            // Recompute next_run_at when schedule changes
            let now = Utc::now();
            job.next_run_at = compute_next_run(&schedule, now)?;
            if let Some(display) = updates.schedule_display {
                job.schedule_display = display;
            } else {
                job.schedule_display = match &schedule {
                    ScheduleParsed::Once { display, .. } => display.clone(),
                    ScheduleParsed::Interval { display, .. } => display.clone(),
                    ScheduleParsed::Cron { display, .. } => display.clone(),
                };
            }
            job.schedule = schedule;
        }

        let updated = job.clone();
        self.save()?;
        Ok(updated)
    }

    /// Remove a job by id.
    pub fn remove_job(&mut self, id: &str) -> Result<()> {
        let before = self.jobs.len();
        self.jobs.retain(|j| j.id != id);
        if self.jobs.len() == before {
            anyhow::bail!("job not found: {id}");
        }
        info!("Removed cron job id={id}");
        self.save()
    }

    /// Look up a job by id.
    pub fn get_job(&self, id: &str) -> Option<&CronJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Find a job by id first, then by name (case-insensitive).
    pub fn find_job(&self, id_or_name: &str) -> Option<&CronJob> {
        self.jobs
            .iter()
            .find(|j| j.id == id_or_name)
            .or_else(|| {
                let lower = id_or_name.to_lowercase();
                self.jobs.iter().find(|j| j.name.to_lowercase() == lower)
            })
    }

    /// Return all jobs.
    pub fn list_jobs(&self) -> &[CronJob] {
        &self.jobs
    }

    /// Return jobs that are enabled, scheduled, and whose `next_run_at` is at or before now.
    /// Skips stale jobs (next_run_at is more than grace_seconds old) and fast-forwards them.
    pub fn get_due_jobs(&mut self) -> Vec<&CronJob> {
        let now = Utc::now();
        let grace = self.grace_seconds;

        // Fast-forward stale jobs
        for job in self.jobs.iter_mut() {
            if job.state != JobState::Scheduled || !job.enabled {
                continue;
            }
            if let Some(next_run_at) = job.next_run_at {
                let age_secs = (now - next_run_at).num_seconds();
                if age_secs > grace {
                    // Fast-forward: compute next from now
                    if let Ok(Some(new_next)) = compute_next_run(&job.schedule, now) {
                        warn!(
                            "Fast-forwarding stale job '{}' from {} to {}",
                            job.name, next_run_at, new_next
                        );
                        job.next_run_at = Some(new_next);
                    }
                }
            }
        }

        // Collect due jobs (borrow again after mutation)
        self.jobs
            .iter()
            .filter(|j| {
                j.state == JobState::Scheduled
                    && j.enabled
                    && j.next_run_at.is_some_and(|t| now >= t)
            })
            .collect()
    }

    /// Enable or disable a job.
    pub fn toggle_job(&mut self, id: &str, enabled: bool) -> Result<()> {
        let now = Utc::now();
        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| anyhow::anyhow!("job not found: {id}"))?;

        job.enabled = enabled;
        if enabled {
            job.state = JobState::Scheduled;
            job.paused_at = None;
            // Recompute next_run_at from now
            job.next_run_at = compute_next_run(&job.schedule, now)?;
        } else {
            job.state = JobState::Paused;
            job.paused_at = Some(now);
        }

        info!("Job id={id} enabled={enabled} state={:?}", job.state);
        self.save()
    }

    /// Record a completed run. Advances next_run_at BEFORE marking (at-most-once semantics).
    pub fn mark_job_run(&mut self, id: &str, output: impl Into<String>, status: &str) -> Result<()> {
        let now = Utc::now();
        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| anyhow::anyhow!("job not found: {id}"))?;

        // Advance next_run_at FIRST (at-most-once)
        job.next_run_at = compute_next_run(&job.schedule, now)?;

        // Record run
        job.last_run_at = Some(now);
        job.repeat.completed += 1;
        let output_str = output.into();

        if status == "error" {
            job.last_error = Some(output_str.clone());
            job.last_status = Some("error".to_string());
        } else {
            job.last_status = Some(output_str.clone());
            job.last_error = None;
        }

        // Check if repeat limit reached
        if job.repeat.times.is_some_and(|times| job.repeat.completed >= times) {
            job.state = JobState::Completed;
            job.next_run_at = None;
        }

        debug!("Job id={} ran at {}, next_run_at={:?}", id, now, job.next_run_at);
        self.save()
    }

    /// Atomically write the current state to disk.
    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.jobs).context("failed to serialise jobs")?;

        let tmp_path = self.path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp_path)
                .with_context(|| format!("failed to create temp file: {}", tmp_path.display()))?;
            f.write_all(json.as_bytes())
                .with_context(|| format!("failed to write temp file: {}", tmp_path.display()))?;
            f.flush()?;
        }
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to rename {} -> {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;

        debug!(
            "JobStore saved {} job(s) to {}",
            self.jobs.len(),
            self.path.display()
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::ScheduleParsed;
    use chrono::Duration;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn tmp_store() -> (TempDir, JobStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        let store = JobStore::open(cron_dir).expect("store");
        (dir, store)
    }

    fn tmp_store_dir() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        (dir, cron_dir)
    }

    fn interval_sched(minutes: u32) -> ScheduleParsed {
        ScheduleParsed::Interval {
            minutes,
            display: format!("every {}m", minutes),
        }
    }

    fn cron_sched(expr: &str) -> ScheduleParsed {
        ScheduleParsed::Cron {
            expr: expr.to_string(),
            display: expr.to_string(),
        }
    }

    fn once_sched_future() -> ScheduleParsed {
        let run_at = Utc::now() + Duration::hours(1);
        ScheduleParsed::Once {
            run_at,
            display: "once in 60m".to_string(),
        }
    }

    // Helper to add a simple interval job
    fn add_interval_job(store: &mut JobStore, name: &str, minutes: u32) -> CronJob {
        store
            .add_job(
                name,
                "do something",
                interval_sched(minutes),
                format!("every {}m", minutes),
                "local",
                vec![],
                None,
            )
            .expect("add_job")
    }

    // --- open() ---

    #[test]
    fn store_open_empty_dir_creates_empty_store() {
        let (_dir, store) = tmp_store();
        assert!(store.list_jobs().is_empty());
    }

    #[test]
    fn store_open_legacy_jobs_json_migrates() {
        let (_dir, cron_dir) = tmp_store_dir();
        fs::create_dir_all(&cron_dir).unwrap();

        // Write a legacy jobs.json
        let legacy_json = serde_json::json!([{
            "id": "legacy-id-1",
            "name": "legacy-job",
            "agent_input": "do the thing",
            "schedule": "0 9 * * *",
            "deliver": "local",
            "enabled": true,
            "created_at": "2026-01-01T00:00:00Z",
            "next_run": "2026-01-02T09:00:00Z",
            "last_run": null,
            "last_output": null
        }]);
        fs::write(cron_dir.join("jobs.json"), legacy_json.to_string()).unwrap();

        let store = JobStore::open(cron_dir).expect("open with legacy");
        assert_eq!(store.list_jobs().len(), 1);
        let job = &store.list_jobs()[0];
        assert_eq!(job.id, "legacy-id-1");
        assert_eq!(job.name, "legacy-job");
        assert_eq!(job.prompt, "do the thing");
        assert!(job.skills.is_empty());
        assert_eq!(job.state, JobState::Scheduled);
    }

    // --- add_job() ---

    #[test]
    fn add_job_once_sets_repeat_times_1() {
        let (_dir, mut store) = tmp_store();
        let run_at = Utc::now() + Duration::hours(2);
        let sched = ScheduleParsed::Once {
            run_at,
            display: "once in 2h".to_string(),
        };
        let job = store
            .add_job("once-job", "prompt", sched, "once in 2h", "local", vec![], None)
            .expect("add");
        assert_eq!(job.repeat.times, Some(1));
        assert_eq!(job.repeat.completed, 0);
    }

    #[test]
    fn add_job_interval_sets_repeat_times_none() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "interval-job", 60);
        assert_eq!(job.repeat.times, None);
    }

    // --- update_job() ---

    #[test]
    fn update_job_name_preserves_other_fields() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "original", 30);
        let original_prompt = job.prompt.clone();
        let original_deliver = job.deliver.clone();

        let updated = store
            .update_job(
                &job.id,
                JobUpdate {
                    name: Some("new-name".to_string()),
                    ..Default::default()
                },
            )
            .expect("update");

        assert_eq!(updated.name, "new-name");
        assert_eq!(updated.prompt, original_prompt);
        assert_eq!(updated.deliver, original_deliver);
    }

    #[test]
    fn update_job_schedule_recomputes_next_run() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "test", 30);
        let old_next = job.next_run_at;

        let new_sched = interval_sched(120);
        let updated = store
            .update_job(
                &job.id,
                JobUpdate {
                    schedule: Some(new_sched),
                    ..Default::default()
                },
            )
            .expect("update");

        // next_run_at should have changed (now + 120m vs now + 30m)
        assert_ne!(updated.next_run_at, old_next);
    }

    #[test]
    fn update_job_skills_set_correctly() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "test", 30);

        let updated = store
            .update_job(
                &job.id,
                JobUpdate {
                    skills: Some(vec!["focus".to_string(), "writing".to_string()]),
                    ..Default::default()
                },
            )
            .expect("update");

        assert_eq!(updated.skills, vec!["focus", "writing"]);
    }

    // --- toggle_job() ---

    #[test]
    fn toggle_job_disable_sets_paused_state() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "test", 60);

        store.toggle_job(&job.id, false).expect("toggle");
        let updated = store.get_job(&job.id).unwrap();
        assert_eq!(updated.state, JobState::Paused);
        assert!(updated.paused_at.is_some());
        assert!(!updated.enabled);
    }

    #[test]
    fn toggle_job_enable_sets_scheduled_state() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "test", 60);

        // First disable
        store.toggle_job(&job.id, false).expect("disable");
        // Then enable
        store.toggle_job(&job.id, true).expect("enable");

        let updated = store.get_job(&job.id).unwrap();
        assert_eq!(updated.state, JobState::Scheduled);
        assert!(updated.paused_at.is_none());
        assert!(updated.enabled);
        assert!(updated.next_run_at.is_some());
    }

    // --- mark_job_run() ---

    #[test]
    fn mark_job_run_advances_next_run_at_before_marking() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "test", 60);
        let job_id = job.id.clone();

        // Backdate next_run_at
        store.jobs[0].next_run_at = Some(Utc::now() - Duration::hours(48));

        store.mark_job_run(&job_id, "done", "ok").expect("mark run");
        let updated = store.get_job(&job_id).unwrap();
        assert!(updated.last_run_at.is_some());
        // next_run_at should be approximately now + 60 min (from mark_job_run's internal now)
        let next = updated.next_run_at.expect("next_run_at set");
        assert!(next > Utc::now() - Duration::minutes(5)); // at least recently computed
    }

    #[test]
    fn mark_job_run_once_completes_after_single_run() {
        let (_dir, mut store) = tmp_store();
        let run_at = Utc::now() + Duration::hours(1);
        let sched = ScheduleParsed::Once {
            run_at,
            display: "once".to_string(),
        };
        let job = store
            .add_job("once", "p", sched, "once", "local", vec![], None)
            .expect("add");
        let job_id = job.id.clone();
        assert_eq!(job.repeat.times, Some(1));

        store.mark_job_run(&job_id, "output", "ok").expect("mark");
        let updated = store.get_job(&job_id).unwrap();
        assert_eq!(updated.state, JobState::Completed);
        assert_eq!(updated.next_run_at, None);
        assert_eq!(updated.repeat.completed, 1);
    }

    // --- get_due_jobs() ---

    #[test]
    fn get_due_jobs_skips_paused_jobs() {
        let (_dir, mut store) = tmp_store();
        let _job = add_interval_job(&mut store, "test", 60);

        // Backdate next_run_at to be due
        store.jobs[0].next_run_at = Some(Utc::now() - Duration::seconds(1));
        // But pause the job
        store.jobs[0].state = JobState::Paused;

        let due = store.get_due_jobs();
        assert!(due.is_empty());
    }

    #[test]
    fn get_due_jobs_returns_scheduled_due_jobs() {
        let (_dir, mut store) = tmp_store();
        add_interval_job(&mut store, "test", 60);

        // Backdate next_run_at to make it due
        store.jobs[0].next_run_at = Some(Utc::now() - Duration::seconds(1));

        let due = store.get_due_jobs();
        assert_eq!(due.len(), 1);
    }

    #[test]
    fn get_due_jobs_fast_forwards_stale_jobs() {
        let (_dir, mut store) = tmp_store();
        add_interval_job(&mut store, "stale", 60);

        // Backdate way beyond grace period (default 3600s)
        store.jobs[0].next_run_at = Some(Utc::now() - Duration::seconds(7200));

        let due = store.get_due_jobs();
        // Should be empty because stale job was fast-forwarded
        assert!(due.is_empty());
        // And next_run_at should now be in the future
        let next = store.jobs[0].next_run_at.unwrap();
        assert!(next > Utc::now() - Duration::minutes(1));
    }

    // --- find_job() ---

    #[test]
    fn find_job_by_id() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "my-job", 60);
        let found = store.find_job(&job.id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, job.id);
    }

    #[test]
    fn find_job_by_name_case_insensitive() {
        let (_dir, mut store) = tmp_store();
        add_interval_job(&mut store, "My-Job", 60);
        let found = store.find_job("my-job");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "My-Job");
    }

    // --- persistence ---

    #[test]
    fn store_roundtrip_persists_and_reloads() {
        let (_dir, cron_dir) = tmp_store_dir();
        let mut store = JobStore::open(cron_dir.clone()).expect("store");
        let job = add_interval_job(&mut store, "daily-report", 60);

        let store2 = JobStore::open(cron_dir).expect("reload");
        assert_eq!(store2.list_jobs().len(), 1);
        assert_eq!(store2.list_jobs()[0].id, job.id);
        assert_eq!(store2.list_jobs()[0].name, "daily-report");
    }

    #[test]
    fn remove_job_works() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "x", 60);
        store.remove_job(&job.id).expect("remove");
        assert!(store.list_jobs().is_empty());
        assert!(store.remove_job(&job.id).is_err());
    }
}
