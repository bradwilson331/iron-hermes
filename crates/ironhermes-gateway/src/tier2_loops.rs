//! Phase 58.2 D-15/D-24: gateway-hosted Tier 2 memory schedulers.
//!
//! An hourly consolidation task and a nightly 02:00-local dreams task, spawned from
//! `GatewayRunner::start` next to the WAL/cleanup/heartbeat loops. The CLI never hosts these
//! (CLI-only wiring is inert). D-27: the irreversible forget purge is deferred out of this phase,
//! so the nightly task runs dreams only and `forgetting_enabled` alone spawns nothing.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, LocalResult, TimeZone, Utc};
use ironhermes_agent::MemoryManager;
use ironhermes_core::config::MemoryConfig;
use tokio::sync::Mutex as TokioMutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Local hour of the nightly run.
const NIGHTLY_HOUR: u32 = 2;

/// Parse an IANA zone name; `None` for absent or invalid names (callers fall back to host-local).
pub fn parse_tz(name: Option<&str>) -> Option<chrono_tz::Tz> {
    name?.parse().ok()
}

/// The next instant strictly after `now` at which the wall clock in `tz` (host-local when `None`)
/// reads `hour:00`. A DST gap resolves to the first valid instant after it; an overlap to the
/// earlier of the two.
pub fn next_run_local(now: DateTime<Utc>, tz: Option<chrono_tz::Tz>, hour: u32) -> DateTime<Utc> {
    match tz {
        Some(tz) => next_run_in(now, &tz, hour),
        None => next_run_in(now, &chrono::Local, hour),
    }
}

fn next_run_in<Z: TimeZone>(now: DateTime<Utc>, tz: &Z, hour: u32) -> DateTime<Utc> {
    let today = now.with_timezone(tz).date_naive();
    for date in [today, today + chrono::Days::new(1)] {
        let mut local = date
            .and_hms_opt(hour.min(23), 0, 0)
            .expect("hour clamped to 0..=23");
        let at = loop {
            match tz.from_local_datetime(&local) {
                LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => break t,
                // DST gap: step to the first local minute that exists.
                LocalResult::None => local += chrono::Duration::minutes(1),
            }
        };
        let at = at.with_timezone(&Utc);
        if at > now {
            return at;
        }
    }
    // Unreachable: tomorrow's 02:00 is always after now. Keep a sane answer rather than panic.
    now + chrono::Duration::hours(24)
}

/// The two Tier 2 jobs a home exposes to the schedulers. A trait so the loops are testable
/// without a real memory service.
#[async_trait::async_trait]
pub trait Tier2Jobs: Send + Sync {
    async fn consolidate(&self) -> anyhow::Result<()>;
    async fn dreams(&self) -> anyhow::Result<()>;
}

/// Production target: one profile home and its memory manager (D-24).
pub struct Tier2LoopTarget {
    pub home: PathBuf,
    pub manager: Arc<TokioMutex<MemoryManager>>,
}

#[async_trait::async_trait]
impl Tier2Jobs for Tier2LoopTarget {
    async fn consolidate(&self) -> anyhow::Result<()> {
        // Clone the handle out so the manager lock is not held across the (long) job.
        let Some(t2) = self.manager.lock().await.tier2() else {
            return Ok(());
        };
        let consolidated = t2.consolidate().await;
        // D-30: promotion runs after every attempt because candidates come from earlier runs too,
        // and a promotion failure never fails the consolidation job.
        match ironhermes_agent::memory::tier2_maintenance::promote_tier2(&self.manager).await {
            Ok(report) => info!(home = %self.home.display(), ?report, "tier2 promotion finished"),
            Err(e) => warn!(home = %self.home.display(), "tier2 promotion failed: {e:#}"),
        }
        let report = consolidated?;
        info!(home = %self.home.display(), ?report, "tier2 consolidation finished");
        Ok(())
    }
    async fn dreams(&self) -> anyhow::Result<()> {
        let Some(t2) = self.manager.lock().await.tier2() else {
            return Ok(());
        };
        let report = t2.run_dreams().await?;
        info!(home = %self.home.display(), ?report, "tier2 dreams finished");
        Ok(())
    }
}

/// D-24: one home this phase (the gateway process's own); a later phase pushes one target per
/// profile home. Empty when there is no manager or it has no Tier 2 handle.
pub async fn tier2_targets_for(
    home: PathBuf,
    manager: Option<Arc<TokioMutex<MemoryManager>>>,
) -> Vec<Arc<dyn Tier2Jobs>> {
    let mut targets: Vec<Arc<dyn Tier2Jobs>> = Vec::new();
    if let Some(manager) = manager
        && manager.lock().await.tier2().is_some()
    {
        targets.push(Arc::new(Tier2LoopTarget { home, manager }));
    }
    targets
}

#[derive(Clone, Copy)]
enum Job {
    Consolidate,
    Dreams,
}

/// Run one job for every target, each in its own task so a panic is logged (JoinError) and does
/// not end the loop; cancellation aborts the in-flight job.
async fn run_job(cancel: &CancellationToken, targets: &[Arc<dyn Tier2Jobs>], job: Job) {
    for target in targets {
        let target = Arc::clone(target);
        let mut handle = tokio::spawn(async move {
            match job {
                Job::Consolidate => target.consolidate().await,
                Job::Dreams => target.dreams().await,
            }
        });
        tokio::select! {
            _ = cancel.cancelled() => {
                handle.abort();
                return;
            }
            res = &mut handle => match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("tier2 job failed: {e:#}"),
                Err(e) => error!("tier2 job panicked or was aborted: {e}"),
            },
        }
    }
}

/// Spawn the Tier 2 tasks into `join_set`; returns how many were spawned.
pub fn spawn_tier2_loops(
    join_set: &mut JoinSet<()>,
    cancel: CancellationToken,
    targets: Vec<Arc<dyn Tier2Jobs>>,
    mem: &MemoryConfig,
    tz: Option<chrono_tz::Tz>,
) -> usize {
    if !mem.tier2_enabled || targets.is_empty() {
        return 0;
    }
    let targets: Arc<[Arc<dyn Tier2Jobs>]> = targets.into();
    let mut spawned = 0;

    if mem.consolidation_enabled {
        let (cancel, targets) = (cancel.clone(), Arc::clone(&targets));
        join_set.spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await; // no burst at boot: first run is one hour after start
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = interval.tick() => run_job(&cancel, &targets, Job::Consolidate).await,
                }
            }
        });
        spawned += 1;
    }

    // D-27: dreams only; `forgetting_enabled` alone spawns nothing (no purge job exists).
    if mem.dreams_enabled {
        join_set.spawn(async move {
            let mut after = Utc::now();
            loop {
                let target = next_run_local(after, tz, NIGHTLY_HOUR);
                let wait = (target - Utc::now()).to_std().unwrap_or_default();
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(wait) => {}
                }
                run_job(&cancel, &targets, Job::Dreams).await;
                // Never re-arm for the instant just served, even if the timer woke early.
                after = Utc::now().max(target);
            }
        });
        spawned += 1;
    }
    spawned
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    const NY: &str = "America/New_York";

    fn ny() -> Option<chrono_tz::Tz> {
        parse_tz(Some(NY))
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn next_run_local_same_day_and_next_day() {
        // 2026-11-08 is EST (UTC-5): 01:30 local = 06:30Z, 02:00 local = 07:00Z.
        assert_eq!(
            next_run_local(utc(2026, 11, 8, 6, 30), ny(), 2),
            utc(2026, 11, 8, 7, 0)
        );
        // 03:00 local = 08:00Z -> 02:00 the next day.
        assert_eq!(
            next_run_local(utc(2026, 11, 8, 8, 0), ny(), 2),
            utc(2026, 11, 9, 7, 0)
        );
    }

    #[test]
    fn next_run_local_dst_gap() {
        // 2027-03-14: 02:00 EST does not exist; the first valid instant is 03:00 EDT = 07:00Z.
        assert_eq!(
            next_run_local(utc(2027, 3, 14, 5, 0), ny(), 2),
            utc(2027, 3, 14, 7, 0)
        );
    }

    #[test]
    fn next_run_local_dst_overlap() {
        // 2026-11-01: 01:00-02:00 repeats, 02:00 EST (07:00Z) occurs once.
        let first = next_run_local(utc(2026, 11, 1, 4, 0), ny(), 2);
        assert_eq!(first, utc(2026, 11, 1, 7, 0));
        // Right after it, the next run is the following day.
        assert_eq!(
            next_run_local(first + chrono::Duration::seconds(1), ny(), 2),
            utc(2026, 11, 2, 7, 0)
        );
    }

    #[test]
    fn next_run_local_invalid_tz_falls_back() {
        assert!(parse_tz(Some("Not/AZone")).is_none());
        assert!(parse_tz(None).is_none());
        let now = utc(2026, 11, 8, 6, 30);
        let next = next_run_local(now, parse_tz(Some("Not/AZone")), 2);
        assert!(next > now && next <= now + chrono::Duration::hours(25));
    }

    #[derive(Default)]
    struct Counting {
        consolidations: AtomicUsize,
        dreams: AtomicUsize,
        panic_on_consolidate: bool,
    }

    #[async_trait::async_trait]
    impl Tier2Jobs for Counting {
        async fn consolidate(&self) -> anyhow::Result<()> {
            self.consolidations.fetch_add(1, Ordering::SeqCst);
            if self.panic_on_consolidate {
                panic!("boom");
            }
            Ok(())
        }
        async fn dreams(&self) -> anyhow::Result<()> {
            self.dreams.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn cfg(tier2: bool, consolidation: bool, dreams: bool, forgetting: bool) -> MemoryConfig {
        MemoryConfig {
            tier2_enabled: tier2,
            consolidation_enabled: consolidation,
            dreams_enabled: dreams,
            forgetting_enabled: forgetting,
            ..Default::default()
        }
    }

    async fn settle() {
        for _ in 0..30 {
            tokio::task::yield_now().await;
        }
    }

    async fn advance(d: Duration) {
        tokio::time::advance(d).await;
        settle().await;
    }

    fn spawn(fake: &Arc<Counting>, mem: &MemoryConfig) -> (JoinSet<()>, CancellationToken, usize) {
        let mut js = JoinSet::new();
        let cancel = CancellationToken::new();
        let n = spawn_tier2_loops(
            &mut js,
            cancel.clone(),
            vec![fake.clone() as Arc<dyn Tier2Jobs>],
            mem,
            ny(),
        );
        (js, cancel, n)
    }

    #[tokio::test(start_paused = true)]
    async fn tier2_loop_hourly_fires_under_paused_time() {
        let fake = Arc::new(Counting::default());
        let (_js, _cancel, n) = spawn(&fake, &cfg(true, true, false, false));
        assert_eq!(n, 1);
        settle().await;
        advance(Duration::from_secs(59 * 60)).await;
        assert_eq!(fake.consolidations.load(Ordering::SeqCst), 0);
        advance(Duration::from_secs(2 * 60)).await;
        assert_eq!(fake.consolidations.load(Ordering::SeqCst), 1);
        advance(Duration::from_secs(60 * 60)).await;
        assert_eq!(fake.consolidations.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn tier2_loop_nightly_runs_dreams() {
        let fake = Arc::new(Counting::default());
        let (_js, _cancel, n) = spawn(&fake, &cfg(true, false, true, false));
        assert_eq!(n, 1);
        settle().await;
        let wait = (next_run_local(Utc::now(), ny(), NIGHTLY_HOUR) - Utc::now())
            .to_std()
            .unwrap();
        advance(wait - Duration::from_secs(60)).await;
        assert_eq!(fake.dreams.load(Ordering::SeqCst), 0);
        advance(Duration::from_secs(120)).await;
        assert_eq!(fake.dreams.load(Ordering::SeqCst), 1);
        assert_eq!(fake.consolidations.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tier2_loop_flags_gate_spawning() {
        let fake = Arc::new(Counting::default());
        // tier2 off -> nothing, whatever the job flags.
        assert_eq!(spawn(&fake, &cfg(false, true, true, true)).2, 0);
        // all job flags off -> nothing.
        assert_eq!(spawn(&fake, &cfg(true, false, false, false)).2, 0);
        // D-27: forgetting alone spawns no nightly task (no purge job exists).
        assert_eq!(spawn(&fake, &cfg(true, false, false, true)).2, 0);
        assert_eq!(spawn(&fake, &cfg(true, true, false, false)).2, 1);
        assert_eq!(spawn(&fake, &cfg(true, false, true, false)).2, 1);
        assert_eq!(spawn(&fake, &cfg(true, true, true, true)).2, 2);
        // no targets -> nothing.
        let mut js = JoinSet::new();
        let n = spawn_tier2_loops(
            &mut js,
            CancellationToken::new(),
            vec![],
            &cfg(true, true, true, false),
            None,
        );
        assert_eq!(n, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tier2_loop_job_panic_is_contained() {
        let fake = Arc::new(Counting {
            panic_on_consolidate: true,
            ..Default::default()
        });
        let (_js, _cancel, _n) = spawn(&fake, &cfg(true, true, false, false));
        settle().await;
        advance(Duration::from_secs(61 * 60)).await;
        assert_eq!(fake.consolidations.load(Ordering::SeqCst), 1);
        advance(Duration::from_secs(60 * 60)).await;
        assert_eq!(fake.consolidations.load(Ordering::SeqCst), 2);
    }

    /// D-15: the gateway runner hosts the loops (CLI-only wiring is inert); D-24: a single target
    /// from the runner's own manager and home.
    #[test]
    fn tier2_loop_runner_source_lock() {
        let runner = include_str!("runner.rs");
        let wal = runner
            .find("9a. WAL checkpoint timer")
            .expect("9a marker present");
        let call = runner
            .find("tier2_loops::spawn_tier2_loops(")
            .expect("runner.rs must call spawn_tier2_loops");
        assert!(call > wal, "spawn_tier2_loops must follow the 9a WAL timer");
        let block = &runner[call.saturating_sub(900)..call];
        assert!(
            block.contains("tier2_targets_for(")
                && block.contains("get_hermes_home()")
                && block.contains("self.memory_manager.clone()"),
            "targets must be built from the runner's own home and memory manager"
        );
        assert_eq!(
            runner.matches("spawn_tier2_loops(").count(),
            1,
            "exactly one spawn site"
        );
        // D-15: no ironhermes-cli source hosts a Tier 2 scheduler.
        let cli = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ironhermes-cli/src");
        let mut stack = vec![cli];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&path).unwrap();
                    assert!(
                        !src.contains("spawn_tier2_loops"),
                        "{} hosts a Tier 2 scheduler",
                        path.display()
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn tier2_loop_runner_targets_helper() {
        use ironhermes_agent::memory::factory::build_memory_manager_at;
        let home = tempfile::tempdir().unwrap();
        let on = MemoryConfig {
            tier2_enabled: true,
            embeddings: ironhermes_core::config::EmbeddingsConfig {
                // Refused loopback port: CI never starts a built-in model download.
                base_url: Some("http://127.0.0.1:9".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let with = build_memory_manager_at(&on, home.path()).await.unwrap();
        assert_eq!(tier2_targets_for(home.path().into(), with).await.len(), 1);
        let home2 = tempfile::tempdir().unwrap();
        let without = build_memory_manager_at(&MemoryConfig::default(), home2.path())
            .await
            .unwrap();
        assert!(
            tier2_targets_for(home2.path().into(), without)
                .await
                .is_empty()
        );
        assert!(tier2_targets_for(home.path().into(), None).await.is_empty());
    }

    /// D-30: the hourly job promotes durable user facts after consolidating; the consolidation
    /// result itself (Ok or Err, there may be nothing to consolidate) does not matter here.
    #[tokio::test]
    async fn tier2_loop_target_consolidate_promotes() {
        use ironhermes_agent::memory::factory::build_memory_manager_at;
        use ironhermes_tier2::store::{NewClaim, Store};
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("tier2")).unwrap();
        Store::open(&home.path().join("tier2/tier2.db"))
            .unwrap()
            .insert_claim(&NewClaim {
                text: "User lives in Lisbon".into(),
                kind: "world".into(),
                subject: "user".into(),
                origin: "consolidation".into(),
                confidence: 0.95,
                source_url: None,
                tier1_target: None,
                tier1_ordinal: None,
                session_id: None,
            })
            .unwrap();
        let on = MemoryConfig {
            tier2_enabled: true,
            consolidation_enabled: true,
            promotion_enabled: true,
            embeddings: ironhermes_core::config::EmbeddingsConfig {
                base_url: Some("http://127.0.0.1:9".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let manager = build_memory_manager_at(&on, home.path())
            .await
            .unwrap()
            .expect("memory is enabled");
        let target = Tier2LoopTarget {
            home: home.path().into(),
            manager,
        };
        let _ = target.consolidate().await;
        let user = std::fs::read_to_string(home.path().join("memories/USER.md")).unwrap();
        assert!(user.contains("User lives in Lisbon"));
    }

    #[tokio::test(start_paused = true)]
    async fn tier2_loop_cancel_stops_tasks() {
        let fake = Arc::new(Counting::default());
        let (mut js, cancel, n) = spawn(&fake, &cfg(true, true, true, false));
        assert_eq!(n, 2);
        settle().await;
        cancel.cancel();
        while js.join_next().await.is_some() {}
        assert!(js.is_empty());
    }
}
