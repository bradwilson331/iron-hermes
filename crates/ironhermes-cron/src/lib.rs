use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use cron::Schedule;
use ironhermes_core::get_hermes_home;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{debug, info};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CronJob
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    /// The prompt/input to send to the agent.
    pub agent_input: String,
    /// A cron expression, e.g. "0 9 * * *".
    pub schedule: String,
    /// Delivery target: "local" | "origin" | "platform:<chat_id>".
    pub deliver: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub next_run: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub last_output: Option<String>,
}

// ---------------------------------------------------------------------------
// JobStore
// ---------------------------------------------------------------------------

/// Persists cron jobs as JSON at `{hermes_home}/cron/jobs.json`.
pub struct JobStore {
    path: PathBuf,
    jobs: Vec<CronJob>,
}

impl JobStore {
    /// Load (or initialise) the job store from disk.
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
            serde_json::from_str::<Vec<CronJob>>(&raw)
                .with_context(|| format!("failed to parse {}", path.display()))?
        } else {
            Vec::new()
        };

        debug!("JobStore loaded {} job(s) from {}", jobs.len(), path.display());
        Ok(Self { path, jobs })
    }

    /// Create a new job, persist it, and return a clone of the created record.
    pub fn add_job(
        &mut self,
        name: impl Into<String>,
        agent_input: impl Into<String>,
        schedule: impl Into<String>,
        deliver: impl Into<String>,
    ) -> Result<CronJob> {
        let schedule_str = schedule.into();
        let now = Utc::now();
        let next_run = compute_next_run(&schedule_str, now)?;

        let job = CronJob {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            agent_input: agent_input.into(),
            schedule: schedule_str,
            deliver: deliver.into(),
            enabled: true,
            created_at: now,
            next_run,
            last_run: None,
            last_output: None,
        };

        info!("Adding cron job '{}' (id={})", job.name, job.id);
        self.jobs.push(job.clone());
        self.save()?;
        Ok(job)
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

    /// Return all jobs.
    pub fn list_jobs(&self) -> &[CronJob] {
        &self.jobs
    }

    /// Return jobs that are enabled and whose `next_run` is at or before now.
    pub fn get_due_jobs(&self) -> Vec<&CronJob> {
        let now = Utc::now();
        self.jobs
            .iter()
            .filter(|j| j.enabled && now >= j.next_run)
            .collect()
    }

    /// Record a completed run: update `last_run`, `last_output`, and advance `next_run`.
    pub fn mark_job_run(&mut self, id: &str, output: impl Into<String>) -> Result<()> {
        let now = Utc::now();
        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| anyhow::anyhow!("job not found: {id}"))?;

        job.last_run = Some(now);
        job.last_output = Some(output.into());
        job.next_run = compute_next_run(&job.schedule, now)?;

        debug!(
            "Job id={} ran at {}, next_run={}",
            id, now, job.next_run
        );
        self.save()
    }

    /// Enable or disable a job.
    pub fn toggle_job(&mut self, id: &str, enabled: bool) -> Result<()> {
        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| anyhow::anyhow!("job not found: {id}"))?;
        job.enabled = enabled;
        info!("Job id={id} enabled={enabled}");
        self.save()
    }

    /// Atomically write the current state to disk.
    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.jobs)
            .context("failed to serialise jobs")?;

        // Atomic write: write to a temp file then rename.
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

        debug!("JobStore saved {} job(s) to {}", self.jobs.len(), self.path.display());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// compute_next_run
// ---------------------------------------------------------------------------

/// Parse `schedule` as a cron expression and return the first occurrence
/// strictly after `after`.
pub fn compute_next_run(schedule: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    // The `cron` crate expects a 6-field or 7-field expression; standard
    // 5-field POSIX cron lacks the seconds field.  We normalise by prepending
    // "0 " (seconds = 0) when only 5 fields are supplied.
    let normalised = normalise_cron_expr(schedule);
    let parsed = Schedule::from_str(&normalised)
        .with_context(|| format!("invalid cron expression: {schedule:?}"))?;

    parsed
        .after(&after)
        .next()
        .ok_or_else(|| anyhow::anyhow!("cron schedule {schedule:?} yields no future occurrences"))
}

/// Prepend a "0" seconds field when the expression has only 5 fields.
fn normalise_cron_expr(expr: &str) -> String {
    let fields = expr.split_whitespace().count();
    if fields == 5 {
        format!("0 {expr}")
    } else {
        expr.to_owned()
    }
}

// ---------------------------------------------------------------------------
// File-based tick lock
// ---------------------------------------------------------------------------

/// An RAII guard that removes the lock file on drop.
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        debug!("Released tick lock: {}", self.path.display());
    }
}

/// Try to acquire an exclusive tick lock using a `.tick.lock` file.
///
/// Returns `Ok(Some(guard))` if the lock was acquired, or `Ok(None)` if
/// another process already holds it (caller should skip this tick).
pub fn acquire_tick_lock() -> Result<Option<LockGuard>> {
    acquire_tick_lock_at(get_hermes_home().join("cron"))
}

/// Like [`acquire_tick_lock`] but at a caller-specified directory.
pub fn acquire_tick_lock_at(dir: PathBuf) -> Result<Option<LockGuard>> {
    let lock_path = dir.join(".tick.lock");

    // Ensure the directory exists.
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("failed to create cron dir: {}", dir.display()))?;
    }

    // Use O_CREAT | O_EXCL for an atomic create-or-fail.
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut f) => {
            // Write our PID so operators can inspect the lock.
            let _ = write!(f, "{}", std::process::id());
            debug!("Acquired tick lock: {}", lock_path.display());
            Ok(Some(LockGuard { path: lock_path }))
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            debug!("Tick lock already held: {}", lock_path.display());
            Ok(None)
        }
        Err(e) => Err(e).with_context(|| {
            format!("failed to acquire tick lock: {}", lock_path.display())
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_cron_dir() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        (dir, cron_dir)
    }

    #[test]
    fn test_compute_next_run_5field() {
        let now = Utc::now();
        let next = compute_next_run("0 9 * * *", now).expect("next run");
        assert!(next > now);
    }

    #[test]
    fn test_compute_next_run_6field() {
        let now = Utc::now();
        let next = compute_next_run("0 0 9 * * *", now).expect("next run");
        assert!(next > now);
    }

    #[test]
    fn test_invalid_schedule() {
        assert!(compute_next_run("not a cron expr", Utc::now()).is_err());
    }

    #[test]
    fn test_job_store_roundtrip() {
        let (_dir, cron_dir) = tmp_cron_dir();
        let mut store = JobStore::open(cron_dir.clone()).expect("store");
        assert!(store.list_jobs().is_empty());

        let job = store
            .add_job("daily-report", "summarise today", "0 9 * * *", "local")
            .expect("add");
        assert_eq!(store.list_jobs().len(), 1);

        // Reload from disk.
        let store2 = JobStore::open(cron_dir).expect("reload");
        assert_eq!(store2.list_jobs().len(), 1);
        assert_eq!(store2.list_jobs()[0].id, job.id);
        assert_eq!(store2.list_jobs()[0].name, "daily-report");
    }

    #[test]
    fn test_remove_job() {
        let (_dir, cron_dir) = tmp_cron_dir();
        let mut store = JobStore::open(cron_dir).expect("store");
        let job = store.add_job("x", "y", "0 9 * * *", "local").expect("add");
        store.remove_job(&job.id).expect("remove");
        assert!(store.list_jobs().is_empty());
        assert!(store.remove_job(&job.id).is_err());
    }

    #[test]
    fn test_toggle_job() {
        let (_dir, cron_dir) = tmp_cron_dir();
        let mut store = JobStore::open(cron_dir).expect("store");
        let job = store.add_job("x", "y", "0 9 * * *", "local").expect("add");
        assert!(store.get_job(&job.id).unwrap().enabled);
        store.toggle_job(&job.id, false).expect("toggle");
        assert!(!store.get_job(&job.id).unwrap().enabled);
    }

    #[test]
    fn test_get_due_jobs() {
        let (_dir, cron_dir) = tmp_cron_dir();
        let mut store = JobStore::open(cron_dir).expect("store");
        store.add_job("x", "y", "0 9 * * *", "local").expect("add");

        // Manually backdate next_run to make it due.
        store.jobs[0].next_run = Utc::now() - chrono::Duration::seconds(1);

        let due = store.get_due_jobs();
        assert_eq!(due.len(), 1);
    }

    #[test]
    fn test_mark_job_run() {
        let (_dir, cron_dir) = tmp_cron_dir();
        let mut store = JobStore::open(cron_dir).expect("store");
        let job = store.add_job("x", "y", "0 9 * * *", "local").expect("add");
        let job_id = job.id.clone();

        // Backdate next_run so the next computed occurrence is guaranteed later.
        store.jobs[0].next_run = Utc::now() - chrono::Duration::hours(48);
        let before_next = store.jobs[0].next_run;

        store.mark_job_run(&job_id, "done").expect("mark run");

        let updated = store.get_job(&job_id).unwrap();
        assert!(updated.last_run.is_some());
        assert_eq!(updated.last_output.as_deref(), Some("done"));
        assert!(updated.next_run > before_next);
    }

    #[test]
    fn test_tick_lock() {
        let (_dir, cron_dir) = tmp_cron_dir();
        let g1 = acquire_tick_lock_at(cron_dir.clone()).expect("lock1");
        assert!(g1.is_some());

        // A second attempt should fail to acquire.
        let g2 = acquire_tick_lock_at(cron_dir.clone()).expect("lock2");
        assert!(g2.is_none());

        // After dropping the first guard the lock file is gone.
        drop(g1);
        let g3 = acquire_tick_lock_at(cron_dir).expect("lock3");
        assert!(g3.is_some());
    }
}
