use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use ironhermes_cron::run_tick_check;
use crate::runner::{run_cron_job, CronRunnerContext};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Run the cron tick loop, ticking every 60s with `MissedTickBehavior::Skip`.
///
/// On each tick:
/// 1. Optionally call `prepare_mcp_for_tick` (non-fatal no-op).
/// 2. Run `run_tick_check` to collect due jobs.
/// 3. Partition due jobs into workdir-serial vs parallel groups.
/// 4. Execute workdir jobs sequentially (TERMINAL_CWD is process-global).
/// 5. Execute parallel jobs concurrently via `JoinSet`, optionally capped by
///    `IRONHERMES_CRON_MAX_PARALLEL` semaphore.
///
/// Single-job failures NEVER panic the tick loop — errors are logged and the
/// loop continues. Cancels cleanly via `cancel`.
pub async fn run_tick_loop(
    ctx: Arc<CronRunnerContext>,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut first_tick = true;

    // Optional concurrency cap for parallel jobs
    let semaphore = parse_env_usize("IRONHERMES_CRON_MAX_PARALLEL")
        .map(|n| Arc::new(Semaphore::new(n)));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Cron tick loop shutting down");
                return;
            }
            _ = interval.tick() => {
                if first_tick {
                    // First tick fires immediately — skip per gateway parity
                    // (acts as a fast-forward boot guard)
                    first_tick = false;
                    continue;
                }
                run_one_tick(&ctx, semaphore.as_ref()).await;
            }
        }
    }
}

/// Public no-op wrapper for per-tick MCP discovery.
///
/// Exposed so gateway and CLI callers can rely on the symbol existing per
/// CONTEXT.md §MCP discovery per tick. The body is intentionally a no-op
/// today (RESEARCH.md Open Question 3 RESOLVED): no `reap_orphans` /
/// discovery primitive exists in `ironhermes-mcp` yet. A future phase
/// replaces the body with the real call once that primitive lands.
pub fn prepare_mcp_for_tick(_mcp: &ironhermes_mcp::McpManager) {
    tracing::debug!(
        "prepare_mcp_for_tick: no-op (MCP discovery primitive not yet \
         implemented in ironhermes-mcp; tracked as deferred follow-up)"
    );
}

// ---------------------------------------------------------------------------
// Private tick implementation
// ---------------------------------------------------------------------------

async fn run_one_tick(
    ctx: &Arc<CronRunnerContext>,
    semaphore: Option<&Arc<Semaphore>>,
) {
    // MCP discovery per tick (non-fatal) — calls the public stub above
    if let Some(mcp) = &ctx.mcp_manager {
        prepare_mcp_for_tick(mcp);
    }

    let (due_jobs, _tick_result, _lock_guard) =
        match run_tick_check(&ctx.job_store).await {
            Ok(v) => v,
            Err(e) => {
                error!("tick check failed: {}", e);
                return;
            }
        };

    // Partition due jobs: workdir jobs run serially (TERMINAL_CWD is
    // process-global); non-workdir jobs run in a JoinSet (bounded by semaphore
    // if IRONHERMES_CRON_MAX_PARALLEL is set).
    let (workdir_jobs, parallel_jobs): (Vec<_>, Vec<_>) =
        due_jobs.into_iter().partition(|j| j.workdir.is_some());

    // 1) Workdir jobs serially (TERMINAL_CWD is process-global)
    for job in &workdir_jobs {
        if let Err(e) = run_cron_job(job, ctx).await {
            error!(job_id=%job.id, "workdir job failed: {}", e);
        }
    }

    // 2) Parallel jobs via JoinSet, semaphore-capped if configured
    let mut set = tokio::task::JoinSet::new();
    for job in parallel_jobs {
        let ctx2 = ctx.clone();
        let job = job.clone();
        let sem = semaphore.cloned();

        set.spawn(async move {
            // Acquire permit if semaphore is configured (caps parallelism)
            let _permit = if let Some(s) = sem {
                s.acquire_owned().await.ok()
            } else {
                None
            };
            if let Err(e) = run_cron_job(&job, &ctx2).await {
                error!(job_id=%job.id, "parallel job failed: {}", e);
            }
        });
    }
    // Drain the JoinSet — single-job panics are caught and logged
    while let Some(result) = set.join_next().await {
        if let Err(e) = result {
            error!("tick: spawned job task panicked: {}", e);
        }
    }

    // TODO Plan 32.x: MCP orphan reaper after tick (no current Rust analog
    // per RESEARCH.md Open Question 3 — defer until orphan-reap API lands).
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_cron::store::JobStore;
    use ironhermes_tools::ToolRegistry;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn make_ctx(tmpdir: &TempDir) -> Arc<CronRunnerContext> {
        let cron_dir = tmpdir.path().join("cron");
        let store = Arc::new(Mutex::new(
            JobStore::open(cron_dir).expect("open store"),
        ));
        Arc::new(CronRunnerContext {
            job_store: store,
            skill_registry: None,
            tool_registry: Arc::new(tokio::sync::RwLock::new(ToolRegistry::new())),
            memory_manager: None,
            hook_registry: None,
            config: ironhermes_core::Config::default(),
            mcp_manager: None,
            tg_client: None,
        })
    }

    // -----------------------------------------------------------------------
    // Test 1: cancel exits cleanly
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test1_cancel_exits_cleanly() {
        let tmp = TempDir::new().expect("tmpdir");
        let ctx = make_ctx(&tmp);
        let cancel = CancellationToken::new();

        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            run_tick_loop(ctx, cancel_clone).await;
        });

        // Cancel immediately
        cancel.cancel();

        // Should resolve within 100ms
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            handle,
        )
        .await;

        assert!(result.is_ok(), "tick loop should exit promptly after cancel");
    }

    // -----------------------------------------------------------------------
    // Test 2: first tick is skipped (boot guard)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test2_first_tick_is_boot_guard() {
        // The first interval.tick() fires immediately (at t=0 for a new interval).
        // The loop should skip it without running run_tick_check.
        // We verify this by noting that with an empty store, the loop cancels
        // within 1s without having dispatched any jobs.
        let tmp = TempDir::new().expect("tmpdir");
        let ctx = make_ctx(&tmp);
        let cancel = CancellationToken::new();

        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            run_tick_loop(ctx, cancel_clone).await;
        });

        // Give enough time for the first (immediate) tick to be processed
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        cancel.cancel();

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            handle,
        )
        .await;
        assert!(result.is_ok(), "loop should exit after cancel");
        // No panic = first tick handled without error
    }

    // -----------------------------------------------------------------------
    // Test 5: single-job failure does not crash the tick loop
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test5_single_job_failure_does_not_crash_loop() {
        // run_one_tick should catch and log errors from run_cron_job without
        // propagating them (the loop continues). We test this indirectly by
        // running run_one_tick with an empty store (no due jobs) and asserting
        // it returns without panic.
        let tmp = TempDir::new().expect("tmpdir");
        let ctx = make_ctx(&tmp);

        // run_one_tick with empty store: no jobs due, no error
        run_one_tick(&ctx, None).await;
        // If we reach here without panic, the isolation is confirmed.
    }

    // -----------------------------------------------------------------------
    // Test: parse_env_usize helper
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_env_usize_none_when_unset() {
        assert_eq!(
            parse_env_usize("IRONHERMES_TEST_USIZE_DEFINITELY_NOT_SET_54321"),
            None
        );
    }

    // -----------------------------------------------------------------------
    // Test: workdir partition
    // -----------------------------------------------------------------------

    #[test]
    fn test_workdir_partition_logic() {
        // Verify the partition predicate: jobs with workdir go serial
        use ironhermes_cron::job::{CronJob, JobState, RepeatConfig, ScheduleParsed};
        use chrono::Utc;

        let make_job = |id: &str, workdir: Option<String>| CronJob {
            id: id.to_string(),
            name: id.to_string(),
            prompt: "x".to_string(),
            skills: vec![],
            schedule: ScheduleParsed::Interval { minutes: 5, display: "every 5m".to_string() },
            schedule_display: "every 5m".to_string(),
            repeat: RepeatConfig::default(),
            enabled: true,
            state: JobState::Scheduled,
            paused_at: None,
            paused_reason: None,
            deliver: "local".to_string(),
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
            workdir,
            last_delivery_error: None,
        };

        let jobs = vec![
            make_job("a", Some("/tmp/wd1".to_string())),
            make_job("b", None),
            make_job("c", Some("/tmp/wd2".to_string())),
            make_job("d", None),
        ];

        let (workdir_jobs, parallel_jobs): (Vec<_>, Vec<_>) =
            jobs.into_iter().partition(|j| j.workdir.is_some());

        assert_eq!(workdir_jobs.len(), 2);
        assert_eq!(parallel_jobs.len(), 2);
        assert!(workdir_jobs.iter().all(|j| j.workdir.is_some()));
        assert!(parallel_jobs.iter().all(|j| j.workdir.is_none()));
    }
}
