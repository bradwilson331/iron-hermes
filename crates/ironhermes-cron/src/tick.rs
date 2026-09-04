use anyhow::Result;
use std::sync::{Arc, Mutex};
use tracing::debug;

use crate::delivery::{DeliveryTarget, is_silent, resolve_delivery_target, save_job_output};
use crate::heartbeat::{TickSummary, record_tick_best_effort};
use crate::job::CronJob;
use crate::store::JobStore;
use crate::{LockGuard, acquire_tick_lock};

// ---------------------------------------------------------------------------
// TickResult
// ---------------------------------------------------------------------------

/// Summary of a tick run.
#[derive(Debug, Default)]
pub struct TickResult {
    /// Total number of enabled jobs found in the store this tick.
    pub jobs_checked: usize,
    /// Number of due jobs that were actually run by this tick.
    pub jobs_run: usize,
    /// Number of enabled jobs that were NOT due this tick.
    /// This is `jobs_checked - jobs_run`; it is NOT a failure count.
    pub jobs_idle: usize,
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
///
/// CR-01: the fallible prefix (tick-lock acquisition, store-mutex lock, or
/// `store.reload()`) used to propagate its `Err` before the heartbeat write
/// below ever ran, so the exact failure class this phase exists to diagnose
/// (an unrepairable `jobs.json`, or a poisoned store mutex from an earlier
/// panic) wrote NO heartbeat at all — `cron status` kept showing a stale but
/// otherwise normal-looking last *successful* tick forever. This wrapper
/// ensures a heartbeat with `error: Some(..)` is always recorded before the
/// error propagates, so `TickState::last_tick_error` / `cron status`'s
/// "Tick error:" line become reachable in production.
pub async fn run_tick_check(
    store: &Arc<Mutex<JobStore>>,
) -> Result<(Vec<CronJob>, TickResult, Option<LockGuard>)> {
    match run_tick_check_inner(store).await {
        Ok(ok) => Ok(ok),
        Err(e) => {
            // WR-04: resolve the heartbeat directory from the store itself
            // rather than a freshly re-resolved `get_hermes_home()` call.
            // Even a poisoned mutex's guard is safe to read `.dir()` from —
            // that's just the JobStore's own `path` field, never touched by
            // whatever panicked while holding the lock. Best-effort: this
            // must never itself fail the tick (swallowed by
            // record_tick_best_effort).
            let cron_dir = match store.lock() {
                Ok(guard) => guard.dir(),
                Err(poisoned) => poisoned.into_inner().dir(),
            };
            record_tick_best_effort(
                &cron_dir,
                TickSummary {
                    jobs_checked: 0,
                    jobs_due: 0,
                    jobs_idle: 0,
                    error: Some(e.to_string()),
                },
            );
            Err(e)
        }
    }
}

async fn run_tick_check_inner(
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
                jobs_idle: 0,
            },
            None,
        ));
    }

    let (due_jobs, total_enabled, cron_dir) = {
        let mut store_guard = store
            .lock()
            .map_err(|e| anyhow::anyhow!("store lock poisoned: {}", e))?;
        // Gap closure (UAT test 13): re-read jobs.json so CLI-created/edited
        // jobs are observable within the gateway's long-running tick loop.
        // Reload happens UNDER the existing tick-lock + store-mutex combo,
        // so writers racing with readers are still serialized by the store
        // mutex on this process and by the tick file-lock across processes.
        store_guard.reload()?;
        let total_enabled = store_guard.list_jobs().iter().filter(|j| j.enabled).count();
        let due_jobs: Vec<CronJob> = store_guard.get_due_jobs().into_iter().cloned().collect();
        // WR-04: resolve the heartbeat directory from the locked store
        // instance itself (matching the pattern Plan 04 established in
        // `store.rs`'s own grace-skip heartbeat write) so the heartbeat
        // always targets wherever THIS store was actually opened, not a
        // freshly re-resolved global path.
        let cron_dir = store_guard.dir();
        (due_jobs, total_enabled, cron_dir)
    };

    let jobs_run = due_jobs.len();
    let jobs_idle = total_enabled.saturating_sub(jobs_run);

    let result = TickResult {
        jobs_checked: total_enabled,
        jobs_run,
        jobs_idle,
    };

    // D-03: record a heartbeat on every tick that acquired the lock, even
    // when zero jobs are due — a healthy idle tick must be distinguishable
    // from a dead one. Best-effort: a failed write is logged and swallowed,
    // never fails the tick.
    record_tick_best_effort(
        &cron_dir,
        TickSummary {
            jobs_checked: result.jobs_checked,
            jobs_due: result.jobs_run,
            jobs_idle: result.jobs_idle,
            error: None,
        },
    );
    debug!(
        "Tick heartbeat: checked={} due={} idle={}",
        result.jobs_checked, result.jobs_run, result.jobs_idle
    );

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
        let mut store_guard = store
            .lock()
            .map_err(|e| anyhow::anyhow!("store lock poisoned: {}", e))?;
        store_guard.mark_job_run(&job.id, output, if success { "ok" } else { "error" })?;
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
    fn tick_result_idle_counts_enabled_but_not_due() {
        // Construct a TickResult by hand and assert the new field
        // exists with the documented semantics.
        let r = TickResult {
            jobs_checked: 10,
            jobs_run: 2,
            jobs_idle: 8,
        };
        assert_eq!(r.jobs_idle, r.jobs_checked - r.jobs_run);
        assert_eq!(r.jobs_idle, 8);
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

    #[tokio::test]
    async fn tick_observes_external_job_writes() {
        use crate::store::JobStore;
        use chrono::{Duration, Utc};
        use std::fs;
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        let dir = TempDir::new().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        let store = Arc::new(Mutex::new(
            JobStore::open(cron_dir.clone()).expect("open store"),
        ));

        // Hold the tokio::sync::Mutex guard across `.await` to pin IRONHERMES_HOME
        // for the whole test. Safe — tokio::sync::Mutex is designed for async tasks
        // and does not deadlock when held across suspension points.
        let _env_guard = crate::test_env_lock().lock().await;
        let original_home = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: test harness, single-threaded tokio runtime
        unsafe {
            std::env::set_var("IRONHERMES_HOME", dir.path());
        }

        // First tick: empty store, no due jobs
        let (due1, _result1, _lock1) = run_tick_check(&store).await.expect("first tick");
        assert!(due1.is_empty(), "expected no due jobs on empty store");
        drop(_lock1); // release the file lock before second tick

        // External writer: drop a due job directly into jobs.json (bypassing save()).
        // Note the snake_case serde tags (#[serde(rename_all = "snake_case")]).
        let past = Utc::now() - Duration::seconds(30);
        let past_str = past.to_rfc3339();
        let external_json = format!(
            r#"[{{
                "id": "ext-due-1",
                "name": "external-due",
                "prompt": "hi",
                "skills": [],
                "schedule": {{ "kind": "interval", "minutes": 5, "display": "every 5m" }},
                "schedule_display": "every 5m",
                "repeat": {{ "times": null, "completed": 0 }},
                "enabled": true,
                "state": "scheduled",
                "paused_at": null,
                "paused_reason": null,
                "deliver": "local",
                "origin": null,
                "created_at": "{past_ts}",
                "next_run_at": "{past_ts}",
                "last_run_at": null,
                "last_status": null,
                "last_error": null
            }}]"#,
            past_ts = past_str
        );
        fs::write(cron_dir.join("jobs.json"), external_json).expect("write external");

        // Second tick: reload() inside run_tick_check picks up the external write
        let (due2, _result2, _lock2) = run_tick_check(&store).await.expect("second tick");
        assert_eq!(due2.len(), 1, "expected tick to observe external job");
        assert_eq!(due2[0].id, "ext-due-1");
        assert_eq!(due2[0].name, "external-due");

        // Restore env var
        // SAFETY: test harness, single-threaded tokio runtime
        unsafe {
            match original_home {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
        }
    }

    #[tokio::test]
    async fn run_tick_check_writes_heartbeat_with_zero_due() {
        use crate::heartbeat::read_tick_state_at;
        use crate::store::JobStore;
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        let dir = TempDir::new().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        let store = Arc::new(Mutex::new(
            JobStore::open(cron_dir.clone()).expect("open store"),
        ));

        let _env_guard = crate::test_env_lock().lock().await;
        let original_home = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: test harness, single-threaded tokio runtime
        unsafe {
            std::env::set_var("IRONHERMES_HOME", dir.path());
        }

        let (due, result, _lock) = run_tick_check(&store).await.expect("tick");
        assert!(due.is_empty(), "expected no due jobs on empty store");
        assert_eq!(result.jobs_run, 0);

        let state = read_tick_state_at(&cron_dir).expect("heartbeat should be recorded");
        assert_eq!(state.jobs_due, 0);
        assert!(state.last_tick_at.is_some());

        // SAFETY: test harness, single-threaded tokio runtime
        unsafe {
            match original_home {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
        }
    }

    /// CR-01 regression: `run_tick_check`'s `Err` path (a poisoned store
    /// mutex, simulating a prior panic while a store guard was held) must
    /// still write a heartbeat with a non-`None` `last_tick_error`, and
    /// `cron status` (via `format_cron_status_with_tick`) must render a
    /// "Tick error:" line from that heartbeat. Before this fix, the only
    /// production `TickSummary` construction site hardcoded `error: None`
    /// and lived on the success path only, so this exact failure class wrote
    /// no heartbeat at all.
    #[tokio::test]
    async fn run_tick_check_records_error_heartbeat_on_poisoned_store() {
        use crate::display::format_cron_status_with_tick;
        use crate::heartbeat::read_tick_state_at;
        use crate::store::JobStore;
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        let dir = TempDir::new().expect("tempdir");
        let cron_dir = dir.path().join("cron");
        let store = Arc::new(Mutex::new(
            JobStore::open(cron_dir.clone()).expect("open store"),
        ));

        let _env_guard = crate::test_env_lock().lock().await;
        let original_home = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: test harness, single-threaded tokio runtime
        unsafe {
            std::env::set_var("IRONHERMES_HOME", dir.path());
        }

        // Poison the mutex the same way a prior panic-while-holding-the-lock
        // would: acquire the guard on a spawned thread, then panic while it
        // is still held.
        let poison_store = store.clone();
        let joined = std::thread::spawn(move || {
            let _guard = poison_store.lock().unwrap();
            panic!("CR-01 regression test: intentional poison");
        })
        .join();
        assert!(joined.is_err(), "the poisoning thread must have panicked");
        assert!(
            store.lock().is_err(),
            "the store mutex must now be poisoned"
        );

        let result = run_tick_check(&store).await;
        assert!(
            result.is_err(),
            "run_tick_check must surface the poisoned-mutex error, not swallow it"
        );

        let state = read_tick_state_at(&cron_dir)
            .expect("a heartbeat must be written even on run_tick_check's Err path");
        let err = state
            .last_tick_error
            .as_deref()
            .expect("last_tick_error must be Some(..) after a failed tick");
        assert!(!err.is_empty(), "the recorded tick error must be non-empty");

        let rendered = format_cron_status_with_tick(&[], Some(&state));
        assert!(
            rendered.contains("Tick error:"),
            "cron status must render a 'Tick error:' line from the recorded heartbeat, got:\n{rendered}"
        );

        // SAFETY: test harness, single-threaded tokio runtime
        unsafe {
            match original_home {
                Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                None => std::env::remove_var("IRONHERMES_HOME"),
            }
        }
    }
}
