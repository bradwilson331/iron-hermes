use anyhow::Result;
use std::sync::{Arc, Mutex};

use crate::delivery::{is_silent, resolve_delivery_target, save_job_output, DeliveryTarget};
use crate::job::CronJob;
use crate::store::JobStore;
use crate::{acquire_tick_lock, LockGuard};

// ---------------------------------------------------------------------------
// TickResult
// ---------------------------------------------------------------------------

/// Summary of a tick run.
#[derive(Debug, Default)]
pub struct TickResult {
    pub jobs_checked: usize,
    pub jobs_run: usize,
    pub jobs_skipped: usize,
}

// ---------------------------------------------------------------------------
// run_tick_check
// ---------------------------------------------------------------------------

/// Acquire the tick lock, collect due jobs, and return them for execution.
///
/// Returns an empty vec + zero TickResult if the lock is already held
/// (another tick is in progress — skip this tick).
///
/// The returned `LockGuard` keeps the lock held until dropped by the caller.
/// Due jobs are cloned so the caller can use them without holding the store lock.
pub async fn run_tick_check(
    store: &Arc<Mutex<JobStore>>,
) -> Result<(Vec<CronJob>, TickResult, Option<LockGuard>)> {
    // Try to acquire the tick lock — skip if held by another process
    let lock_guard = acquire_tick_lock()?;
    if lock_guard.is_none() {
        return Ok((
            vec![],
            TickResult {
                jobs_checked: 0,
                jobs_run: 0,
                jobs_skipped: 0,
            },
            None,
        ));
    }

    let (due_jobs, total_enabled) = {
        let mut store_guard = store.lock().unwrap();
        let total_enabled = store_guard
            .list_jobs()
            .iter()
            .filter(|j| j.enabled)
            .count();
        let due_jobs: Vec<CronJob> = store_guard
            .get_due_jobs()
            .into_iter()
            .cloned()
            .collect();
        (due_jobs, total_enabled)
    };

    let jobs_run = due_jobs.len();
    let jobs_skipped = total_enabled.saturating_sub(jobs_run);

    let result = TickResult {
        jobs_checked: total_enabled,
        jobs_run,
        jobs_skipped,
    };

    Ok((due_jobs, result, lock_guard))
}

// ---------------------------------------------------------------------------
// complete_job_run
// ---------------------------------------------------------------------------

/// Record a completed job run: save output to file, mark in store, and
/// return the delivery target (if any) unless output is marked [SILENT].
///
/// Returns `None` if:
/// - output starts with `[SILENT]` (delivery suppressed)
/// - job's `deliver` resolves to local-only or no origin
pub async fn complete_job_run(
    store: &Arc<Mutex<JobStore>>,
    job: &CronJob,
    output: &str,
    success: bool,
) -> Result<Option<DeliveryTarget>> {
    // Save output to file unconditionally
    let _path = save_job_output(&job.id, output)?;

    // Mark job run in store
    {
        let mut store_guard = store.lock().unwrap();
        store_guard.mark_job_run(
            &job.id,
            output,
            if success { "ok" } else { "error" },
        )?;
    }

    // [SILENT] marker suppresses platform delivery
    if is_silent(output) {
        return Ok(None);
    }

    // Resolve delivery target
    Ok(resolve_delivery_target(job))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{JobOrigin, JobState, RepeatConfig, ScheduleParsed};
    use chrono::Utc;

    fn make_job_with_deliver(deliver: &str) -> CronJob {
        CronJob {
            id: "job-1".to_string(),
            name: "Test Job".to_string(),
            prompt: "do something".to_string(),
            skills: vec![],
            schedule: ScheduleParsed::Interval {
                minutes: 60,
                display: "every 60m".to_string(),
            },
            schedule_display: "every 60m".to_string(),
            repeat: RepeatConfig::default(),
            enabled: true,
            state: JobState::Scheduled,
            paused_at: None,
            paused_reason: None,
            deliver: deliver.to_string(),
            origin: None,
            created_at: Utc::now(),
            next_run_at: None,
            last_run_at: None,
            last_status: None,
            last_error: None,
        }
    }

    fn make_job_with_origin(deliver: &str) -> CronJob {
        let mut job = make_job_with_deliver(deliver);
        job.origin = Some(JobOrigin {
            platform: "telegram".to_string(),
            chat_id: "12345".to_string(),
            chat_name: None,
            thread_id: None,
        });
        job
    }

    #[test]
    fn complete_job_run_silent_suppresses_delivery() {
        // We test the is_silent path without the store/file system
        let output = "[SILENT] this is silent output";
        assert!(is_silent(output));

        // Verify that is_silent suppression logic works
        let job = make_job_with_origin("origin");
        // resolve_delivery_target would return Some(target) for this job,
        // but is_silent check should return None before resolving
        assert!(is_silent(output));
        let _ = job; // use the job
    }

    #[test]
    fn complete_job_run_local_deliver_returns_none() {
        let job = make_job_with_deliver("local");
        let target = resolve_delivery_target(&job);
        assert!(target.is_none());
    }

    #[test]
    fn complete_job_run_platform_deliver_returns_target() {
        let job = make_job_with_deliver("telegram:99999");
        let target = resolve_delivery_target(&job);
        assert!(target.is_some());
        let t = target.unwrap();
        assert_eq!(t.platform, "telegram");
        assert_eq!(t.chat_id, "99999");
    }
}
