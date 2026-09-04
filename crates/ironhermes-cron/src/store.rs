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
use crate::parser::{
    ONESHOT_GRACE_SECONDS, compute_grace_seconds, compute_next_run, compute_next_run_from,
    parse_schedule,
};

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
            model: None,
            provider: None,
            base_url: None,
            script: None,
            no_agent: false,
            context_from: None,
            enabled_toolsets: None,
            workdir: None,
            last_delivery_error: None,
            continuity: false,
        }
    }
}

// ---------------------------------------------------------------------------
// NewJobSpec — full-surface job creation input (D-15)
// ---------------------------------------------------------------------------

/// Every argument `JobStore::add_job` takes today, plus the nine advanced
/// fields (`model`, `provider`, `base_url`, `script`, `no_agent`,
/// `context_from`, `enabled_toolsets`, `workdir`) and `continuity` — the full
/// surface of `CronJob`'s creation-time fields. `add_job_spec` is the single
/// `CronJob { .. }` construction site for job creation; `add_job` builds one
/// of these from its narrow positional args and delegates.
///
/// `ScheduleParsed` has no meaningful default, so this does not derive
/// `Default` — use [`NewJobSpec::new`] and assign the fields you need.
#[derive(Debug, Clone)]
pub struct NewJobSpec {
    pub name: String,
    pub prompt: String,
    pub schedule: ScheduleParsed,
    pub schedule_display: String,
    pub deliver: String,
    pub skills: Vec<String>,
    pub origin: Option<JobOrigin>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub script: Option<String>,
    pub no_agent: bool,
    pub context_from: Option<Vec<String>>,
    pub enabled_toolsets: Option<Vec<String>>,
    pub workdir: Option<String>,
    pub continuity: bool,
}

impl NewJobSpec {
    /// A spec carrying only the required fields. `skills` starts empty,
    /// `origin` starts `None`, and every advanced field plus `continuity`
    /// starts at its zero value — callers assign the ones they care about.
    pub fn new(
        name: impl Into<String>,
        prompt: impl Into<String>,
        schedule: ScheduleParsed,
        schedule_display: impl Into<String>,
        deliver: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            prompt: prompt.into(),
            schedule,
            schedule_display: schedule_display.into(),
            deliver: deliver.into(),
            skills: Vec::new(),
            origin: None,
            model: None,
            provider: None,
            base_url: None,
            script: None,
            no_agent: false,
            context_from: None,
            enabled_toolsets: None,
            workdir: None,
            continuity: false,
        }
    }
}

/// An empty or whitespace-only string is indistinguishable from a real value
/// (e.g. a provider name) once it reaches the resolution layer below, so
/// both job creation ([`JobStore::add_job_spec`]) and partial updates
/// ([`JobStore::update_job`]) normalize it to `None` for the five
/// `Option<String>` advanced fields.
fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
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
    pub model: Option<String>,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub script: Option<String>,
    pub workdir: Option<String>,
    pub context_from: Option<Vec<String>>,
    pub enabled_toolsets: Option<Vec<String>>,
    pub no_agent: Option<bool>,
    pub continuity: Option<bool>,
}

// ---------------------------------------------------------------------------
// JobStore
// ---------------------------------------------------------------------------

/// Persists cron jobs as JSON at `{dir}/jobs.json`.
pub struct JobStore {
    path: PathBuf,
    pub jobs: Vec<CronJob>,
}

impl JobStore {
    /// Load (or initialise) the job store from the default hermes home directory.
    pub fn new() -> Result<Self> {
        Self::open(get_hermes_home().join("cron"))
    }

    /// The directory holding this store's `jobs.json` — the parent of
    /// `self.path`. Used so store-side heartbeat writes (the runtime
    /// grace-skip record in [`JobStore::get_due_jobs`]) target the directory
    /// this store was actually opened at, rather than re-deriving it from
    /// [`get_hermes_home`]: a store opened at a temp dir in a test must never
    /// write into the operator's real `~/.ironhermes`.
    pub fn dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Load (or initialise) the job store at a specific directory.
    pub fn open(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create cron directory: {}", dir.display()))?;

        // Unix: restrict cron directory to owner only (0700)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }

        let path = dir.join("jobs.json");
        let jobs = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;

            // Try new format first, then legacy, with control-char repair fallback
            let parse_result = parse_jobs_with_repair(&raw, &path);
            match parse_result {
                ParseResult::NewFormat(jobs) => {
                    debug!(
                        "JobStore loaded {} job(s) from {} (new format)",
                        jobs.len(),
                        path.display()
                    );
                    jobs
                }
                ParseResult::Legacy(jobs) => {
                    info!(
                        "Migrating {} legacy job(s) from {}",
                        jobs.len(),
                        path.display()
                    );
                    let migrated: Vec<CronJob> = jobs.into_iter().map(CronJob::from).collect();
                    // Save migrated jobs immediately
                    let tmp_path = path.with_extension("json.tmp");
                    let json =
                        serde_json::to_string_pretty(&migrated).context("serialize migrated")?;
                    {
                        let mut f = fs::File::create(&tmp_path)
                            .with_context(|| format!("create tmp: {}", tmp_path.display()))?;
                        f.write_all(json.as_bytes())?;
                        f.flush()?;
                        f.sync_all()?;
                    }
                    fs::rename(&tmp_path, &path)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
                    }
                    migrated
                }
                ParseResult::Empty => {
                    warn!(
                        "Could not parse {} as new or legacy format, starting empty",
                        path.display()
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        Ok(Self { path, jobs })
    }

    /// Re-read `jobs.json` into `self.jobs` without recreating the handle.
    ///
    /// Honors the same format-fallback ladder as `open()` (new format -> legacy -> empty+warn).
    /// Does not create the directory; the store must already be open.
    pub fn reload(&mut self) -> Result<()> {
        let jobs = if self.path.exists() {
            let raw = fs::read_to_string(&self.path)
                .with_context(|| format!("failed to read {}", self.path.display()))?;

            match parse_jobs_with_repair(&raw, &self.path) {
                ParseResult::NewFormat(jobs) => {
                    debug!(
                        "JobStore reloaded {} job(s) from {} (new format)",
                        jobs.len(),
                        self.path.display()
                    );
                    jobs
                }
                ParseResult::Legacy(legacy_jobs) => {
                    info!(
                        "Reload: migrating {} legacy job(s) from {}",
                        legacy_jobs.len(),
                        self.path.display()
                    );
                    let jobs: Vec<CronJob> = legacy_jobs.into_iter().map(CronJob::from).collect();
                    // Persist migrated format so subsequent reloads take the fast path.
                    let tmp_path = self.path.with_extension("json.tmp");
                    let json = serde_json::to_string_pretty(&jobs).context("serialize migrated")?;
                    {
                        let mut f = fs::File::create(&tmp_path)
                            .with_context(|| format!("create tmp: {}", tmp_path.display()))?;
                        f.write_all(json.as_bytes())?;
                        f.flush()?;
                        f.sync_all()?;
                    }
                    fs::rename(&tmp_path, &self.path)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
                    }
                    jobs
                }
                ParseResult::Empty => {
                    warn!(
                        "Reload: could not parse {} as new or legacy format, keeping empty",
                        self.path.display()
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        self.jobs = jobs;
        Ok(())
    }

    /// Create a new job from a full spec, persist it, and return a clone of
    /// the created record. The single `CronJob { .. }` construction site for
    /// job creation — [`JobStore::add_job`] is a thin wrapper that delegates
    /// here.
    pub fn add_job_spec(&mut self, spec: NewJobSpec) -> Result<CronJob> {
        let now = Utc::now();
        let next_run_at = compute_next_run(&spec.schedule, now)?;

        // Auto-set repeat.times=Some(1) for Once kind
        let repeat = match &spec.schedule {
            ScheduleParsed::Once { .. } => RepeatConfig {
                times: Some(1),
                completed: 0,
            },
            _ => RepeatConfig::default(),
        };

        let job = CronJob {
            id: Uuid::new_v4().to_string(),
            name: spec.name,
            prompt: spec.prompt,
            skills: spec.skills,
            schedule: spec.schedule,
            schedule_display: spec.schedule_display,
            repeat,
            enabled: true,
            state: JobState::Scheduled,
            paused_at: None,
            paused_reason: None,
            deliver: spec.deliver,
            origin: spec.origin,
            created_at: now,
            next_run_at,
            last_run_at: None,
            last_status: None,
            last_error: None,
            model: normalize_optional_string(spec.model),
            provider: normalize_optional_string(spec.provider),
            base_url: normalize_optional_string(spec.base_url),
            script: normalize_optional_string(spec.script),
            no_agent: spec.no_agent,
            context_from: spec.context_from,
            enabled_toolsets: spec.enabled_toolsets,
            workdir: normalize_optional_string(spec.workdir),
            last_delivery_error: None,
            continuity: spec.continuity,
        };

        info!("Adding cron job '{}' (id={})", job.name, job.id);
        self.jobs.push(job.clone());
        self.save()?;
        Ok(job)
    }

    /// Create a new job carrying only the fields this narrow entry point
    /// predates: name, prompt, schedule, delivery target, skills, and
    /// origin. Every one of the nine advanced fields plus `continuity` is
    /// left at its zero value — no per-job model/provider override, no
    /// output-dir scripting, no cross-job context, no continuity — silently,
    /// with no error. Callers needing any of those must use
    /// [`JobStore::add_job_spec`] instead.
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
        let mut spec = NewJobSpec::new(name, prompt, schedule, schedule_display, deliver);
        spec.skills = skills;
        spec.origin = origin;
        self.add_job_spec(spec)
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
        if let Some(model) = updates.model {
            job.model = normalize_optional_string(Some(model));
        }
        if let Some(provider) = updates.provider {
            job.provider = normalize_optional_string(Some(provider));
        }
        if let Some(base_url) = updates.base_url {
            job.base_url = normalize_optional_string(Some(base_url));
        }
        if let Some(script) = updates.script {
            job.script = normalize_optional_string(Some(script));
        }
        if let Some(workdir) = updates.workdir {
            job.workdir = normalize_optional_string(Some(workdir));
        }
        if let Some(context_from) = updates.context_from {
            job.context_from = Some(context_from);
        }
        if let Some(enabled_toolsets) = updates.enabled_toolsets {
            job.enabled_toolsets = Some(enabled_toolsets);
        }
        if let Some(no_agent) = updates.no_agent {
            job.no_agent = no_agent;
        }
        if let Some(continuity) = updates.continuity {
            job.continuity = continuity;
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
        self.jobs.iter().find(|j| j.id == id_or_name).or_else(|| {
            let lower = id_or_name.to_lowercase();
            self.jobs.iter().find(|j| j.name.to_lowercase() == lower)
        })
    }

    /// Return all jobs.
    pub fn list_jobs(&self) -> &[CronJob] {
        &self.jobs
    }

    /// Mutable accessor over the in-memory job list. Used by the
    /// `ironhermes-cron-runner` crate to set `last_delivery_error`
    /// in place after dispatching delivery (Plan 32.1-06 Task 2).
    ///
    /// Callers MUST follow up with `self.save()` to persist mutations.
    /// Prefer `update_job` + `JobUpdate` for canonical partial updates;
    /// this accessor exists specifically because `last_delivery_error`
    /// accumulation in the cron runner is a write-only side effect
    /// that doesn't model cleanly as a `JobUpdate` variant.
    pub fn jobs_mut(&mut self) -> &mut Vec<CronJob> {
        &mut self.jobs
    }

    /// Return jobs that are enabled, scheduled, and whose `next_run_at` is at or before now.
    ///
    /// - Jobs with missing `next_run_at` (None) are recovered:
    ///   - `Once`: treated as due if `run_at` is within `ONESHOT_GRACE_SECONDS` and never run
    ///   - `Interval`/`Cron`: recomputed from `last_run_at` anchor and persisted (best-effort)
    /// - Jobs whose `next_run_at` is stale (older than per-schedule grace) are fast-forwarded.
    pub fn get_due_jobs(&mut self) -> Vec<&CronJob> {
        let now = Utc::now();

        // Track which jobs had their next_run_at recomputed (for best-effort save)
        let mut needs_save = false;

        // D-03/D-05 (Plan 04): the stale fast-forward branch below is a
        // second, independent silent-skip site (distinct from the gateway's
        // boot-time `fast_forward_backlog`). It rolls a job forward during
        // normal ticking, not just at boot. Collect each occurrence here and
        // record it to the heartbeat after the pass so it is visible from a
        // separate `cron status` process — the skip DECISION itself
        // (`grace_secs`/`age_secs`/the recompute) is unchanged.
        let mut grace_skip_events: Vec<crate::heartbeat::BacklogEvent> = Vec::new();

        // Pass 1: recovery + stale fast-forward
        for job in self.jobs.iter_mut() {
            if job.state != JobState::Scheduled || !job.enabled {
                continue;
            }

            match job.next_run_at {
                None => {
                    // Recovery: recompute next_run_at for recurring schedules
                    match &job.schedule {
                        ScheduleParsed::Once { .. } => {
                            // Handled in the filter below — leave None, let Once grace logic run
                        }
                        _ => {
                            if let Ok(Some(new_next)) =
                                compute_next_run_from(&job.schedule, now, job.last_run_at)
                            {
                                debug!(
                                    "Recovering missing next_run_at for job '{}': set to {}",
                                    job.name, new_next
                                );
                                job.next_run_at = Some(new_next);
                                needs_save = true;
                            }
                        }
                    }
                }
                Some(next_run_at) => {
                    // Stale fast-forward: per-schedule dynamic grace
                    let grace_secs = compute_grace_seconds(&job.schedule);
                    let age_secs = (now - next_run_at).num_seconds();
                    if age_secs > grace_secs
                        && let Ok(Some(new_next)) = compute_next_run(&job.schedule, now)
                    {
                        warn!(
                            "Fast-forwarding stale job '{}' from {} to {}",
                            job.name, next_run_at, new_next
                        );
                        grace_skip_events.push(crate::heartbeat::BacklogEvent {
                            job_id: job.id.clone(),
                            job_name: job.name.clone(),
                            missed_at: next_run_at,
                            action: crate::heartbeat::BacklogAction::Skipped,
                            rescheduled_to: Some(new_next),
                        });
                        job.next_run_at = Some(new_next);
                        needs_save = true;
                    }
                }
            }
        }

        // Best-effort save for recovered next_run_at values
        if needs_save && let Err(e) = self.save() {
            warn!(
                "get_due_jobs: failed to persist recovered next_run_at: {}",
                e
            );
        }

        // Best-effort heartbeat write of the runtime grace-skip events. Must
        // never abort or fail get_due_jobs — a broken heartbeat must not stop
        // jobs from running.
        if let Err(e) = crate::heartbeat::record_grace_skips_at(&self.dir(), grace_skip_events) {
            warn!("get_due_jobs: failed to record grace-skip heartbeat: {}", e);
        }

        // Pass 2: collect due jobs
        // Clone the vec of references from the immutable borrow after mutation
        let now = Utc::now();
        self.jobs
            .iter()
            .filter(|j| {
                if j.state != JobState::Scheduled || !j.enabled {
                    return false;
                }
                match j.next_run_at {
                    Some(t) => now >= t,
                    None => {
                        // Only Once schedules reach here (recurring was recovered above)
                        if let ScheduleParsed::Once { run_at, .. } = &j.schedule {
                            let age = (now - *run_at).num_seconds();
                            j.last_run_at.is_none() && (0..=ONESHOT_GRACE_SECONDS).contains(&age)
                        } else {
                            false
                        }
                    }
                }
            })
            .collect()
    }

    /// Manually trigger a job by id or name: sets `next_run_at = Utc::now()` and persists.
    pub fn trigger_job(&mut self, id_or_name: &str) -> Result<()> {
        let id = self
            .find_job(id_or_name)
            .map(|j| j.id.clone())
            .ok_or_else(|| anyhow::anyhow!("job not found: {id_or_name}"))?;

        let job = self
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .expect("id found above");

        let now = Utc::now();
        job.next_run_at = Some(now);
        info!("Job id={id} manually triggered");
        self.save()
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
    pub fn mark_job_run(
        &mut self,
        id: &str,
        output: impl Into<String>,
        status: &str,
    ) -> Result<()> {
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
        if job
            .repeat
            .times
            .is_some_and(|times| job.repeat.completed >= times)
        {
            job.state = JobState::Completed;
            job.next_run_at = None;
        }

        debug!(
            "Job id={} ran at {}, next_run_at={:?}",
            id, now, job.next_run_at
        );
        self.save()
    }

    /// Atomically write the current state to disk.
    ///
    /// Write sequence: serialize → write tmp → flush → sync_all → rename.
    /// `sync_all` ensures the new inode's bytes are durable before the rename
    /// makes them visible, satisfying POSIX durability (PARITY §11.18).
    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.jobs).context("failed to serialise jobs")?;

        let tmp_path = self.path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp_path)
                .with_context(|| format!("failed to create temp file: {}", tmp_path.display()))?;
            f.write_all(json.as_bytes())
                .with_context(|| format!("failed to write temp file: {}", tmp_path.display()))?;
            f.flush()?;
            f.sync_all()?;
        }
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "failed to rename {} -> {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;

        // Unix: restrict jobs.json to owner only (0600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }

        debug!(
            "JobStore saved {} job(s) to {}",
            self.jobs.len(),
            self.path.display()
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Result of parsing a jobs.json file (with control-char repair fallback).
enum ParseResult {
    NewFormat(Vec<CronJob>),
    Legacy(Vec<LegacyCronJob>),
    Empty,
}

/// Returns true if the raw string contains bare control characters (0x00–0x1F
/// except `\n`, `\r`, `\t`) — indicating a corrupted jobs.json.
fn raw_has_bare_ctrl_chars(raw: &str) -> bool {
    raw.bytes()
        .any(|b| b < 0x20 && !matches!(b, b'\n' | b'\r' | b'\t'))
}

/// Strip bare control characters (0x00–0x1F, preserving `\n`/`\r`/`\t`) by
/// replacing them with a space. Used to repair corrupted jobs.json files.
fn strip_ctrl_chars(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if (c as u32) < 0x20 && !matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Parse a raw jobs.json string, attempting new format, then legacy, then
/// control-char repair (PARITY §11.17). Returns `ParseResult::Empty` only
/// when all attempts fail.
fn parse_jobs_with_repair(raw: &str, path: &std::path::Path) -> ParseResult {
    // Fast path: new format parses cleanly
    if let Ok(jobs) = serde_json::from_str::<Vec<CronJob>>(raw) {
        return ParseResult::NewFormat(jobs);
    }

    // Fast path: legacy format parses cleanly
    if let Ok(legacy) = serde_json::from_str::<Vec<LegacyCronJob>>(raw) {
        return ParseResult::Legacy(legacy);
    }

    // Control-char repair: strip bare control bytes and retry
    if raw_has_bare_ctrl_chars(raw) {
        let cleaned = strip_ctrl_chars(raw);
        if let Ok(jobs) = serde_json::from_str::<Vec<CronJob>>(&cleaned) {
            warn!(
                "jobs.json had bare control characters — repaired and rewriting: {}",
                path.display()
            );
            // Best-effort rewrite — write errors surface on next save()
            let _ = fs::write(path, &cleaned);
            return ParseResult::NewFormat(jobs);
        }
        if let Ok(legacy) = serde_json::from_str::<Vec<LegacyCronJob>>(&cleaned) {
            warn!(
                "jobs.json had bare control characters (legacy format) — repaired and rewriting: {}",
                path.display()
            );
            let _ = fs::write(path, &cleaned);
            return ParseResult::Legacy(legacy);
        }
    }

    ParseResult::Empty
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heartbeat::{read_tick_state_at, BacklogAction};
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

    #[allow(dead_code)]
    fn cron_sched(expr: &str) -> ScheduleParsed {
        ScheduleParsed::Cron {
            expr: expr.to_string(),
            display: expr.to_string(),
        }
    }

    #[allow(dead_code)]
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

    /// D-16: a job migrated through `LegacyCronJob::from` — which predates
    /// `continuity` entirely — must come out with `continuity == false`, not
    /// silently inheriting some other default. `LegacyCronJob` is private to
    /// this module, so (per plan deviation, see 49.5-03-SUMMARY.md) this test
    /// lives here rather than in job.rs's `cronjob_serde_tests`, exercising
    /// the same file-based migration path `store_open_legacy_jobs_json_migrates`
    /// already uses.
    #[test]
    fn legacy_migration_sets_continuity_false() {
        let (_dir, cron_dir) = tmp_store_dir();
        fs::create_dir_all(&cron_dir).unwrap();

        let legacy_json = serde_json::json!([{
            "id": "legacy-id-2",
            "name": "legacy-job-2",
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
        let job = &store.list_jobs()[0];
        assert!(!job.continuity, "legacy-migrated job must not silently opt into continuity");
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
            .add_job(
                "once-job",
                "prompt",
                sched,
                "once in 2h",
                "local",
                vec![],
                None,
            )
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

    // --- dir() (Phase 49.2 Plan 04) ---

    #[test]
    fn job_store_dir_returns_parent_of_jobs_json() {
        let (dir, cron_dir) = tmp_store_dir();
        let store = JobStore::open(cron_dir.clone()).expect("open");
        assert_eq!(store.dir(), cron_dir);
        drop(dir);
    }

    // --- get_due_jobs() grace-skip heartbeat (Phase 49.2 Plan 04, D-03) ---

    #[test]
    fn get_due_jobs_records_grace_skip_event() {
        let (_dir, mut store) = tmp_store();
        let job = store
            .add_job(
                "cron-job",
                "do something",
                cron_sched("* * * * *"),
                "every minute",
                "local",
                vec![],
                None,
            )
            .expect("add_job");

        // Backdate beyond the Cron grace window (3600s).
        let missed_at = Utc::now() - Duration::hours(2);
        store.jobs[0].next_run_at = Some(missed_at);

        let due = store.get_due_jobs();
        assert!(due.is_empty(), "stale job must be fast-forwarded, not returned as due");

        // The policy is unchanged: next_run_at still moves into the future.
        let next = store.jobs[0].next_run_at.expect("next_run_at set");
        assert!(next > Utc::now() - Duration::minutes(1));

        let state = read_tick_state_at(&store.dir()).expect("heartbeat written");
        assert_eq!(state.recent_skips.len(), 1);
        let event = &state.recent_skips[0];
        assert_eq!(event.job_id, job.id);
        assert_eq!(event.job_name, "cron-job");
        assert_eq!(event.action, BacklogAction::Skipped);
        assert_eq!(event.missed_at, missed_at);
        assert!(event.rescheduled_to.is_some());
    }

    #[test]
    fn get_due_jobs_inside_grace_window_records_no_skip_event() {
        let (_dir, mut store) = tmp_store();
        store
            .add_job(
                "cron-job-fresh",
                "do something",
                cron_sched("* * * * *"),
                "every minute",
                "local",
                vec![],
                None,
            )
            .expect("add_job");

        // Backdate by 10 minutes — well inside the 3600s Cron grace window.
        store.jobs[0].next_run_at = Some(Utc::now() - Duration::minutes(10));

        let due = store.get_due_jobs();
        assert_eq!(due.len(), 1, "a job inside the grace window is due, not skipped");

        let state = read_tick_state_at(&store.dir());
        assert!(
            state.is_none_or(|s| s.recent_skips.is_empty()),
            "no grace-skip event should be recorded for a job inside its grace window"
        );
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
    fn reload_picks_up_external_mutations() {
        let (_dir, cron_dir) = tmp_store_dir();
        let mut store = JobStore::open(cron_dir.clone()).expect("open");
        let _original = add_interval_job(&mut store, "in-memory-job", 30);
        assert_eq!(store.list_jobs().len(), 1);
        assert_eq!(store.list_jobs()[0].name, "in-memory-job");

        // Simulate an external writer (e.g. a CLI subcommand in a separate process)
        // replacing jobs.json on disk. Note the snake_case tag/state values —
        // CronJob serde uses #[serde(rename_all = "snake_case")].
        let external_jobs = serde_json::json!([
            {
                "id": "ext-id-1",
                "name": "external-job",
                "prompt": "external",
                "skills": [],
                "schedule": { "kind": "interval", "minutes": 15, "display": "every 15m" },
                "schedule_display": "every 15m",
                "repeat": { "times": null, "completed": 0 },
                "enabled": true,
                "state": "scheduled",
                "paused_at": null,
                "paused_reason": null,
                "deliver": "local",
                "origin": null,
                "created_at": "2026-01-01T00:00:00Z",
                "next_run_at": "2030-01-01T00:00:00Z",
                "last_run_at": null,
                "last_status": null,
                "last_error": null
            }
        ]);
        fs::write(cron_dir.join("jobs.json"), external_jobs.to_string()).unwrap();

        store.reload().expect("reload");
        assert_eq!(
            store.list_jobs().len(),
            1,
            "reload should replace in-memory jobs"
        );
        assert_eq!(store.list_jobs()[0].id, "ext-id-1");
        assert_eq!(store.list_jobs()[0].name, "external-job");
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

// ---------------------------------------------------------------------------
// Phase 32.1 Plan 03 hardening tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod store_phase_32_1_tests {
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

    fn once_sched(run_at: DateTime<Utc>) -> ScheduleParsed {
        ScheduleParsed::Once {
            run_at,
            display: "once".to_string(),
        }
    }

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

    // Test 1: save() produces a valid JSON file
    #[test]
    fn test1_save_produces_valid_json() {
        let (_dir, mut store) = tmp_store();
        add_interval_job(&mut store, "job1", 60);
        store.save().expect("save");
        let contents = fs::read_to_string(&store.path).expect("read back");
        let parsed: serde_json::Value = serde_json::from_str(&contents).expect("valid json");
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }

    // Test 2 (Unix only): permissions after open and save
    #[cfg(unix)]
    #[test]
    fn test2_unix_permissions_after_open_and_save() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, cron_dir) = tmp_store_dir();
        let mut store = JobStore::open(cron_dir.clone()).expect("open");

        // Directory should be 0700
        let dir_meta = fs::metadata(&cron_dir).expect("dir meta");
        let dir_mode = dir_meta.permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "cron dir must be 0700, got {:o}", dir_mode);

        // After save, jobs.json should be 0600
        add_interval_job(&mut store, "perm-job", 60);
        store.save().expect("save");
        let file_meta = fs::metadata(&store.path).expect("file meta");
        let file_mode = file_meta.permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "jobs.json must be 0600, got {:o}",
            file_mode
        );
    }

    // Test 3: control-char repair — bell byte in name is repaired
    #[test]
    fn test3_control_char_repair() {
        let (_dir, cron_dir) = tmp_store_dir();
        fs::create_dir_all(&cron_dir).unwrap();

        // Write a jobs.json with a bare ASCII 0x07 (BEL) in the name field
        let raw = "[{\"id\":\"x\",\"name\":\"\u{0007}bad\",\"prompt\":\"p\",\
            \"skills\":[],\
            \"schedule\":{\"kind\":\"interval\",\"minutes\":60,\"display\":\"every 60m\"},\
            \"schedule_display\":\"every 60m\",\
            \"repeat\":{\"times\":null,\"completed\":0},\
            \"enabled\":true,\"state\":\"scheduled\",\
            \"paused_at\":null,\"paused_reason\":null,\"deliver\":\"local\",\
            \"origin\":null,\"created_at\":\"2026-01-01T00:00:00Z\",\
            \"next_run_at\":null,\"last_run_at\":null,\
            \"last_status\":null,\"last_error\":null}]";
        let jobs_path = cron_dir.join("jobs.json");
        fs::write(&jobs_path, raw).unwrap();

        let store = JobStore::open(cron_dir.clone()).expect("open with ctrl chars");
        assert_eq!(store.list_jobs().len(), 1, "must load 1 job after repair");
        // BEL (0x07) must be replaced — name must not contain the byte 0x07
        let name = &store.list_jobs()[0].name;
        assert!(
            !name.contains('\u{0007}'),
            "name must not contain BEL after repair: {:?}",
            name
        );
        assert!(name.contains("bad"), "name must still contain 'bad'");

        // The repaired file must not contain bare 0x07 either
        let repaired = fs::read(jobs_path).expect("read repaired");
        assert!(
            !repaired.contains(&0x07u8),
            "repaired file must not contain bare 0x07"
        );
    }

    // Test 4: trigger_job by id sets next_run_at ≈ now
    #[test]
    fn test4_trigger_job_by_id() {
        let (_dir, cron_dir) = tmp_store_dir();
        let mut store = JobStore::open(cron_dir.clone()).expect("open");
        let job = add_interval_job(&mut store, "trig-id", 60);

        let before = Utc::now() - Duration::seconds(5);
        store.trigger_job(&job.id).expect("trigger by id");
        let after = Utc::now() + Duration::seconds(5);

        // Reload and verify
        let store2 = JobStore::open(cron_dir).expect("reload");
        let updated = store2.get_job(&job.id).expect("job must exist");
        let nra = updated.next_run_at.expect("next_run_at must be Some");
        assert!(
            nra >= before && nra <= after,
            "next_run_at={:?} not within 5s window",
            nra
        );
    }

    // Test 5: trigger_job by name (case-insensitive)
    #[test]
    fn test5_trigger_job_by_name() {
        let (_dir, cron_dir) = tmp_store_dir();
        let mut store = JobStore::open(cron_dir.clone()).expect("open");
        let job = add_interval_job(&mut store, "daily-sync", 60);

        let before = Utc::now() - Duration::seconds(5);
        store.trigger_job("daily-sync").expect("trigger by name");
        let after = Utc::now() + Duration::seconds(5);

        let store2 = JobStore::open(cron_dir).expect("reload");
        let updated = store2.get_job(&job.id).expect("job must exist");
        let nra = updated.next_run_at.expect("next_run_at must be Some");
        assert!(
            nra >= before && nra <= after,
            "next_run_at={:?} not within 5s window",
            nra
        );
    }

    // Test 6: trigger_job nonexistent returns Err with "job not found"
    #[test]
    fn test6_trigger_job_nonexistent_returns_err() {
        let (_dir, mut store) = tmp_store();
        let err = store.trigger_job("nope").unwrap_err();
        assert!(
            err.to_string().contains("job not found"),
            "error must mention 'job not found', got: {}",
            err
        );
    }

    // Test 7: dynamic grace — Interval(10min) job 200s past is still due (grace=300s)
    #[test]
    fn test7_dynamic_grace_interval_within_grace() {
        let (_dir, mut store) = tmp_store();
        let job = add_interval_job(&mut store, "grace-test", 10);
        // Set next_run_at to 200s ago (within 300s grace for 10min interval)
        store.jobs[0].next_run_at = Some(Utc::now() - Duration::seconds(200));

        let due = store.get_due_jobs();
        assert_eq!(
            due.len(),
            1,
            "job within dynamic grace (200s < 300s) should be due"
        );
        assert_eq!(due[0].id, job.id);
    }

    // Test 8: oneshot grace — Once job 60s past is still due (grace=120s)
    #[test]
    fn test8_oneshot_grace_within_window() {
        let (_dir, mut store) = tmp_store();
        let run_at = Utc::now() - Duration::seconds(60);
        let sched = once_sched(run_at);
        let job = store
            .add_job("once-grace", "p", sched, "once", "local", vec![], None)
            .expect("add");
        // Set next_run_at to None so we exercise the Once recovery path
        store.jobs[0].next_run_at = None;
        // last_run_at stays None (never ran)

        let due = store.get_due_jobs();
        assert_eq!(
            due.len(),
            1,
            "Once job 60s past should be due within 120s grace"
        );
        assert_eq!(due[0].id, job.id);
    }

    // Test 9: oneshot beyond grace — Once job 200s past is NOT due
    #[test]
    fn test9_oneshot_beyond_grace_not_due() {
        let (_dir, mut store) = tmp_store();
        let run_at = Utc::now() - Duration::seconds(200);
        let sched = once_sched(run_at);
        store
            .add_job("once-old", "p", sched, "once", "local", vec![], None)
            .expect("add");
        store.jobs[0].next_run_at = None;
        // last_run_at stays None

        let due = store.get_due_jobs();
        assert!(
            due.is_empty(),
            "Once job 200s past should NOT be due (beyond 120s grace)"
        );
    }

    // Test 10: due recovery for recurring — None next_run_at gets recomputed
    #[test]
    fn test10_due_recovery_recurring_recomputes_next_run() {
        let (_dir, mut store) = tmp_store();
        add_interval_job(&mut store, "recover-interval", 1); // 1-min interval
        // Wipe next_run_at to simulate corrupted/missing field
        store.jobs[0].next_run_at = None;
        store.jobs[0].last_run_at = None;

        let _due = store.get_due_jobs();
        // After get_due_jobs, next_run_at must be Some (recomputed)
        assert!(
            store.jobs[0].next_run_at.is_some(),
            "next_run_at must be recomputed after recovery"
        );
    }

    // Test 11: due recovery for Once — None next_run_at within grace returns job as due
    #[test]
    fn test11_due_recovery_once_within_grace() {
        let (_dir, mut store) = tmp_store();
        let run_at = Utc::now() - Duration::seconds(30); // 30s ago — within 120s grace
        let sched = once_sched(run_at);
        let job = store
            .add_job("once-recover", "p", sched, "once", "local", vec![], None)
            .expect("add");
        store.jobs[0].next_run_at = None;
        store.jobs[0].last_run_at = None; // never ran

        let due = store.get_due_jobs();
        assert_eq!(
            due.len(),
            1,
            "Once job 30s past with None next_run should be due"
        );
        assert_eq!(due[0].id, job.id);
    }

    // Test 12: jobs_mut accessor allows in-place mutation, persists via save
    #[test]
    fn test12_jobs_mut_accessor() {
        let (_dir, cron_dir) = tmp_store_dir();
        let mut store = JobStore::open(cron_dir.clone()).expect("open");
        add_interval_job(&mut store, "job-a", 30);
        add_interval_job(&mut store, "job-b", 60);
        let job_b_id = store.jobs[1].id.clone();

        // Mutate via jobs_mut
        {
            let jobs = store.jobs_mut();
            jobs[1].last_delivery_error = Some("test-err".to_string());
        }
        store.save().expect("save after jobs_mut");

        // Reload and verify
        let store2 = JobStore::open(cron_dir).expect("reload");
        assert_eq!(
            store2
                .get_job(&job_b_id)
                .unwrap()
                .last_delivery_error
                .as_deref(),
            Some("test-err"),
            "job-b last_delivery_error must persist"
        );
        // job-a must be unchanged
        let job_a_id = store.jobs[0].id.clone();
        assert_eq!(
            store2.get_job(&job_a_id).unwrap().last_delivery_error,
            None,
            "job-a last_delivery_error must remain None"
        );
    }
}

// ---------------------------------------------------------------------------
// job_spec_widening_tests (D-15) — NewJobSpec / add_job_spec / widened JobUpdate
// ---------------------------------------------------------------------------

#[cfg(test)]
mod job_spec_widening_tests {
    use super::*;
    use crate::job::ScheduleParsed;
    use tempfile::TempDir;

    fn tmp_store() -> (TempDir, JobStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        let store = JobStore::open(cron_dir).expect("store");
        (dir, store)
    }

    fn interval_sched(minutes: u32) -> ScheduleParsed {
        ScheduleParsed::Interval {
            minutes,
            display: format!("every {}m", minutes),
        }
    }

    #[test]
    fn add_job_wrapper_matches_spec_with_zero_advanced_fields() {
        let (_dir1, mut store1) = tmp_store();
        let via_wrapper = store1
            .add_job(
                "job-via-wrapper",
                "do something",
                interval_sched(60),
                "every 60m",
                "local",
                vec![],
                None,
            )
            .expect("add_job");

        let (_dir2, mut store2) = tmp_store();
        let spec = NewJobSpec::new(
            "job-via-wrapper",
            "do something",
            interval_sched(60),
            "every 60m",
            "local",
        );
        let via_spec = store2.add_job_spec(spec).expect("add_job_spec");

        // id and created_at are freshly generated per call and are expected
        // to differ; everything else must match byte-for-byte.
        assert_eq!(via_wrapper.name, via_spec.name);
        assert_eq!(via_wrapper.prompt, via_spec.prompt);
        assert_eq!(via_wrapper.skills, via_spec.skills);
        assert_eq!(via_wrapper.schedule, via_spec.schedule);
        assert_eq!(via_wrapper.schedule_display, via_spec.schedule_display);
        assert_eq!(via_wrapper.deliver, via_spec.deliver);
        assert_eq!(via_wrapper.origin, via_spec.origin);
        assert_eq!(via_wrapper.enabled, via_spec.enabled);
        assert_eq!(via_wrapper.state, via_spec.state);
        assert_eq!(via_wrapper.model, via_spec.model);
        assert_eq!(via_wrapper.provider, via_spec.provider);
        assert_eq!(via_wrapper.base_url, via_spec.base_url);
        assert_eq!(via_wrapper.script, via_spec.script);
        assert_eq!(via_wrapper.no_agent, via_spec.no_agent);
        assert_eq!(via_wrapper.context_from, via_spec.context_from);
        assert_eq!(via_wrapper.enabled_toolsets, via_spec.enabled_toolsets);
        assert_eq!(via_wrapper.workdir, via_spec.workdir);
        assert_eq!(via_wrapper.continuity, via_spec.continuity);
        assert_ne!(via_wrapper.id, via_spec.id);
    }

    #[test]
    fn add_job_spec_persists_every_advanced_field() {
        let (dir, mut store) = tmp_store();
        let cron_dir = dir.path().join("cron");

        let mut spec = NewJobSpec::new(
            "full-spec-job",
            "do something",
            interval_sched(30),
            "every 30m",
            "local",
        );
        spec.model = Some("claude-3-opus".to_string());
        spec.provider = Some("anthropic".to_string());
        spec.base_url = Some("https://api.anthropic.com".to_string());
        spec.script = Some("check.sh".to_string());
        spec.no_agent = true;
        spec.context_from = Some(vec!["job-a".to_string(), "job-b".to_string()]);
        spec.enabled_toolsets = Some(vec!["web".to_string(), "code".to_string()]);
        spec.workdir = Some("/home/user/projects".to_string());
        spec.continuity = true;

        let created = store.add_job_spec(spec).expect("add_job_spec");
        let created_id = created.id.clone();

        let reopened = JobStore::open(cron_dir).expect("reopen");
        let job = reopened.get_job(&created_id).expect("job persisted");

        assert_eq!(job.model.as_deref(), Some("claude-3-opus"));
        assert_eq!(job.provider.as_deref(), Some("anthropic"));
        assert_eq!(job.base_url.as_deref(), Some("https://api.anthropic.com"));
        assert_eq!(job.script.as_deref(), Some("check.sh"));
        assert!(job.no_agent);
        assert_eq!(
            job.context_from.as_deref(),
            Some(["job-a".to_string(), "job-b".to_string()].as_slice())
        );
        assert_eq!(
            job.enabled_toolsets.as_deref(),
            Some(["web".to_string(), "code".to_string()].as_slice())
        );
        assert_eq!(job.workdir.as_deref(), Some("/home/user/projects"));
        assert!(job.continuity);
    }

    #[test]
    fn add_job_spec_normalizes_empty_strings_to_none() {
        let (_dir, mut store) = tmp_store();
        let mut spec = NewJobSpec::new(
            "empty-string-job",
            "do something",
            interval_sched(60),
            "every 60m",
            "local",
        );
        spec.provider = Some(String::new());
        spec.model = Some(String::new());
        spec.base_url = Some(String::new());
        spec.script = Some(String::new());
        spec.workdir = Some(String::new());

        let job = store.add_job_spec(spec).expect("add_job_spec");
        assert_eq!(job.provider, None);
        assert_eq!(job.model, None);
        assert_eq!(job.base_url, None);
        assert_eq!(job.script, None);
        assert_eq!(job.workdir, None);
    }

    fn full_spec_job(store: &mut JobStore) -> CronJob {
        let mut spec = NewJobSpec::new(
            "advanced-job",
            "do something",
            interval_sched(60),
            "every 60m",
            "local",
        );
        spec.model = Some("claude-3-opus".to_string());
        spec.provider = Some("anthropic".to_string());
        spec.base_url = Some("https://api.anthropic.com".to_string());
        spec.script = Some("check.sh".to_string());
        spec.no_agent = true;
        spec.context_from = Some(vec!["job-a".to_string()]);
        spec.enabled_toolsets = Some(vec!["web".to_string()]);
        spec.workdir = Some("/home/user/projects".to_string());
        spec.continuity = true;
        store.add_job_spec(spec).expect("add_job_spec")
    }

    #[test]
    fn update_job_applies_each_advanced_field_independently() {
        let (_dir, mut store) = tmp_store();
        let job = full_spec_job(&mut store);

        let updated = store
            .update_job(
                &job.id,
                JobUpdate {
                    model: Some("claude-3-sonnet".to_string()),
                    ..Default::default()
                },
            )
            .expect("update_job");

        assert_eq!(updated.model.as_deref(), Some("claude-3-sonnet"));
        // Every other advanced field (and continuity) must be untouched.
        assert_eq!(updated.provider.as_deref(), Some("anthropic"));
        assert_eq!(
            updated.base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(updated.script.as_deref(), Some("check.sh"));
        assert!(updated.no_agent);
        assert_eq!(
            updated.context_from.as_deref(),
            Some(["job-a".to_string()].as_slice())
        );
        assert_eq!(
            updated.enabled_toolsets.as_deref(),
            Some(["web".to_string()].as_slice())
        );
        assert_eq!(updated.workdir.as_deref(), Some("/home/user/projects"));
        assert!(updated.continuity);
    }

    #[test]
    fn update_job_none_leaves_field_unchanged() {
        let (_dir, mut store) = tmp_store();
        let job = full_spec_job(&mut store);
        let before = job.clone();

        let updated = store
            .update_job(&job.id, JobUpdate::default())
            .expect("update_job");

        assert_eq!(updated.model, before.model);
        assert_eq!(updated.provider, before.provider);
        assert_eq!(updated.base_url, before.base_url);
        assert_eq!(updated.script, before.script);
        assert_eq!(updated.no_agent, before.no_agent);
        assert_eq!(updated.context_from, before.context_from);
        assert_eq!(updated.enabled_toolsets, before.enabled_toolsets);
        assert_eq!(updated.workdir, before.workdir);
        assert_eq!(updated.continuity, before.continuity);
    }

    #[test]
    fn update_job_can_clear_a_list_field() {
        let (_dir, mut store) = tmp_store();
        let job = full_spec_job(&mut store);

        let updated = store
            .update_job(
                &job.id,
                JobUpdate {
                    context_from: Some(vec![]),
                    enabled_toolsets: Some(vec![]),
                    ..Default::default()
                },
            )
            .expect("update_job");

        assert_eq!(updated.context_from, Some(vec![]));
        assert_eq!(updated.enabled_toolsets, Some(vec![]));
    }
}
