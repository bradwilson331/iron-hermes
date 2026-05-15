//! Inactivity + wall-clock timeout primitives.
//! Implemented in Task 2 of plan 32.1-05b.

use anyhow::{anyhow, Result};
use std::future::Future;
use std::time::Duration;
use tokio::time::{sleep, timeout};

// ---------------------------------------------------------------------------
// Wall-clock timeout
// ---------------------------------------------------------------------------

/// Wrap a future in a wall-clock deadline.
///
/// If `wall_secs == 0`, the gate is disabled and the future runs to completion
/// without any timeout enforced.
///
/// Returns `Err` with the substring `wall-clock timeout` when the deadline
/// fires before the future resolves.
pub async fn run_with_wall_clock<F, T>(fut: F, wall_secs: u64) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    if wall_secs == 0 {
        return fut.await;
    }
    match timeout(Duration::from_secs(wall_secs), fut).await {
        Ok(inner) => inner,
        Err(_) => Err(anyhow!("wall-clock timeout after {}s", wall_secs)),
    }
}

// ---------------------------------------------------------------------------
// Inactivity timeout
// ---------------------------------------------------------------------------

/// Wrap a future in an inactivity-polling gate.
///
/// Every 5 seconds, `activity_summary_fn()` is called.  It returns the number
/// of seconds since the last observed activity on the agent.  If that value
/// exceeds `inactivity_secs` for a single poll, the gate fires.
///
/// If `inactivity_secs == 0`, the gate is disabled and the future runs to
/// completion without any timeout enforced.
///
/// Returns `Err` with the substring `inactivity timeout` when the gate fires.
pub async fn run_with_inactivity_timeout<F, T, S>(
    fut: F,
    activity_summary_fn: S,
    inactivity_secs: u64,
) -> Result<T>
where
    F: Future<Output = Result<T>> + Send,
    S: Fn() -> f64 + Send + 'static,
{
    if inactivity_secs == 0 {
        return fut.await;
    }

    tokio::select! {
        result = fut => result,
        _ = async {
            loop {
                sleep(Duration::from_secs(5)).await;
                let idle_secs = activity_summary_fn();
                if idle_secs > inactivity_secs as f64 {
                    break;
                }
            }
        } => Err(anyhow!(
            "inactivity timeout after {}s of no activity",
            inactivity_secs
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    // Test 1: wall-clock fires before slow future
    #[tokio::test]
    async fn test1_wall_clock_fires() {
        let start = Instant::now();
        let result = run_with_wall_clock(
            async {
                sleep(Duration::from_secs(5)).await;
                Ok::<(), anyhow::Error>(())
            },
            1,
        )
        .await;

        assert!(result.is_err(), "Expected Err from wall-clock timeout");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("wall-clock timeout"),
            "Expected 'wall-clock timeout' in error message, got: {msg}"
        );
        // Should fire well under 2 seconds
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "Elapsed time should be under 2s, was {:?}",
            start.elapsed()
        );
    }

    // Test 2: wall-clock disabled when wall_secs == 0
    #[tokio::test]
    async fn test2_wall_clock_disabled_when_zero() {
        let result = run_with_wall_clock(async { Ok::<_, anyhow::Error>(42i32) }, 0).await;
        assert!(result.is_ok(), "Expected Ok when wall_secs == 0");
        assert_eq!(result.unwrap(), 42);
    }

    // Test 3: wall-clock succeeds when future completes within deadline
    #[tokio::test]
    async fn test3_wall_clock_succeeds_fast_future() {
        let result = run_with_wall_clock(async { Ok::<_, anyhow::Error>("done") }, 60).await;
        assert!(result.is_ok(), "Expected Ok for fast future with 60s deadline");
        assert_eq!(result.unwrap(), "done");
    }

    // Test 4: inactivity fires when activity_summary_fn always reports high idle time
    #[tokio::test]
    async fn test4_inactivity_fires() {
        let start = Instant::now();
        let result = run_with_inactivity_timeout(
            async {
                sleep(Duration::from_secs(60)).await;
                Ok::<(), anyhow::Error>(())
            },
            || 1000.0_f64, // always 1000 seconds since last activity
            5,
        )
        .await;

        assert!(result.is_err(), "Expected Err from inactivity timeout");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("inactivity timeout"),
            "Expected 'inactivity timeout' in error message, got: {msg}"
        );
        // Should fire within 7s (5s poll interval + 5s inactivity_secs would fire
        // on first poll since idle_secs=1000 > inactivity_secs=5; worst case 5s poll + slack)
        assert!(
            start.elapsed() < Duration::from_secs(7),
            "Elapsed should be under 7s, was {:?}",
            start.elapsed()
        );
    }

    // Test 5: inactivity disabled when inactivity_secs == 0
    #[tokio::test]
    async fn test5_inactivity_disabled_when_zero() {
        let result = run_with_inactivity_timeout(
            async { Ok::<_, anyhow::Error>(99i32) },
            || 9999.0_f64, // would always fire if enabled
            0,
        )
        .await;
        assert!(result.is_ok(), "Expected Ok when inactivity_secs == 0");
        assert_eq!(result.unwrap(), 99);
    }

    // Test 6: inactivity succeeds when activity is recent
    #[tokio::test]
    async fn test6_inactivity_succeeds_when_active() {
        let result = run_with_inactivity_timeout(
            async {
                sleep(Duration::from_secs(2)).await;
                Ok::<_, anyhow::Error>("active")
            },
            || 0.1_f64, // always recent activity
            600,
        )
        .await;
        assert!(
            result.is_ok(),
            "Expected Ok when activity is recent, got: {:?}",
            result
        );
        assert_eq!(result.unwrap(), "active");
    }

    // Test 7: brief idleness does not trigger — idle < inactivity_secs
    #[tokio::test]
    async fn test7_brief_idleness_does_not_trigger() {
        let result = run_with_inactivity_timeout(
            async {
                sleep(Duration::from_secs(3)).await;
                Ok::<_, anyhow::Error>("ok")
            },
            || 2.0_f64, // 2 seconds idle, but threshold is 10
            10,
        )
        .await;
        assert!(
            result.is_ok(),
            "Expected Ok when idle ({}) < inactivity_secs ({}), got: {:?}",
            2.0_f64,
            10,
            result
        );
        assert_eq!(result.unwrap(), "ok");
    }
}
