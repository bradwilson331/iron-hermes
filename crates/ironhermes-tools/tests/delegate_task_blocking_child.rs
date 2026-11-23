//! Phase 58.1 Plan 02 — D-05 (RESEARCH Exposure 1 and 2), D-13.
//!
//! A delegating turn must return by its deadline even when a child cannot be
//! polled (blocked in synchronous work) or is starved of a delegate slot.
//!
//! These tests use `flavor = "multi_thread"` on purpose: a child that blocks
//! its worker thread with `std::thread::sleep` must not also be the thread
//! that polls the parent's collect loop, or the parent could never observe its
//! own deadline (the very hang this plan fixes).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::SubagentConfig;
use ironhermes_tools::delegate_task::{
    ChildToolProgressCallback, DelegateTaskTool, SubagentRunner,
};
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

/// Runner that watches the cancel token on a detached task, then blocks its
/// own thread synchronously (the in-task `tokio::time::timeout` cannot fire).
struct BlockingRunner {
    observed_cancelled: Arc<AtomicBool>,
}

#[async_trait]
impl SubagentRunner for BlockingRunner {
    async fn run_child(
        &self,
        _registry: Arc<RwLock<ToolRegistry>>,
        _system_prompt: String,
        _max_iterations: usize,
        _model_override: Option<&str>,
        cancel_token: Option<CancellationToken>,
        _tool_progress: Option<ChildToolProgressCallback>,
        _stale_warn_seconds: u64,
    ) -> anyhow::Result<Option<String>> {
        let observed = self.observed_cancelled.clone();
        let cancel = cancel_token.expect("batch child must receive a cancel token");
        tokio::spawn(async move {
            cancel.cancelled().await;
            observed.store(true, Ordering::SeqCst);
        });
        // Let the watcher register on the token before this thread blocks: a task spawned
        // from a worker lands in its non-stealable LIFO slot and would otherwise never run.
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_secs(8));
        Ok(Some("late".to_string()))
    }
}

fn config(child_timeout_seconds: u64) -> SubagentConfig {
    SubagentConfig {
        child_timeout_seconds,
        max_iterations: 5,
        max_concurrent_children: 2,
        ..SubagentConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_child_blocked_synchronously_is_aborted_at_parent_deadline() {
    let observed = Arc::new(AtomicBool::new(false));
    let tool = DelegateTaskTool::new(
        Arc::new(BlockingRunner {
            observed_cancelled: observed.clone(),
        }),
        Arc::new(Semaphore::new(1)),
        None,
        config(60),
        Some(CancellationToken::new()),
    );

    let start = std::time::Instant::now();
    let out = tool
        .execute(serde_json::json!({"tasks": [{"goal": "block", "timeout_seconds": 1}]}))
        .await
        .expect("batch returns Ok with a timed-out slot");
    let elapsed = start.elapsed();

    assert!(
        out.contains("timed out"),
        "slot must report a timeout, got: {out}"
    );
    assert!(
        !out.contains("late"),
        "blocked child's late reply must not surface: {out}"
    );
    assert!(
        elapsed < Duration::from_secs(6),
        "parent must return at deadline (1s) + grace (2s), not wait for the 8s block. Elapsed: {elapsed:?}"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        observed.load(Ordering::SeqCst),
        "T-58.1-09: the child token must be cancelled before the abort"
    );
}

/// Runner that sleeps then replies (never reached when the delegate slot is starved).
struct SleepThenReplyRunner {
    sleep: Duration,
    reply: &'static str,
}

#[async_trait]
impl SubagentRunner for SleepThenReplyRunner {
    async fn run_child(
        &self,
        _registry: Arc<RwLock<ToolRegistry>>,
        _system_prompt: String,
        _max_iterations: usize,
        _model_override: Option<&str>,
        _cancel_token: Option<CancellationToken>,
        _tool_progress: Option<ChildToolProgressCallback>,
        _stale_warn_seconds: u64,
    ) -> anyhow::Result<Option<String>> {
        tokio::time::sleep(self.sleep).await;
        Ok(Some(self.reply.to_string()))
    }
}

fn starved_tool() -> DelegateTaskTool {
    DelegateTaskTool::new(
        Arc::new(SleepThenReplyRunner {
            sleep: Duration::ZERO,
            reply: "ran",
        }),
        Arc::new(Semaphore::new(0)),
        None,
        config(60),
        Some(CancellationToken::new()),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_semaphore_wait_is_bounded_by_child_deadline() {
    let tool = starved_tool();
    let start = std::time::Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        tool.execute(serde_json::json!({"tasks": [{"goal": "wait", "timeout_seconds": 1}]})),
    )
    .await
    .expect("outer guard must not elapse: the slot wait must be bounded")
    .expect("batch returns Ok with a timed-out slot");

    assert!(
        out.contains("timed out") && out.contains("delegate slot"),
        "got: {out}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "elapsed {:?}",
        start.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_semaphore_wait_is_bounded_by_timeout() {
    let tool = starved_tool();
    let start = std::time::Instant::now();
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        tool.execute(serde_json::json!({"task": "wait", "timeout_seconds": 1})),
    )
    .await
    .expect("outer guard must not elapse: the slot wait must be bounded")
    .expect_err("starved single task must time out");

    let msg = err.to_string();
    assert!(
        msg.contains("timed out") && msg.contains("delegate slot"),
        "got: {msg}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "elapsed {:?}",
        start.elapsed()
    );
}

/// D-13: the model's `timeout_seconds` wins over `child_timeout_seconds` (no clamp).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_timeout_above_config_is_not_clamped() {
    let tool = DelegateTaskTool::new(
        Arc::new(SleepThenReplyRunner {
            sleep: Duration::from_millis(1500),
            reply: "finished",
        }),
        Arc::new(Semaphore::new(2)),
        None,
        config(1),
        Some(CancellationToken::new()),
    );

    let single = tool
        .execute(serde_json::json!({"task": "x", "timeout_seconds": 3}))
        .await
        .expect("single path honors the model's 3s");
    assert!(single.contains("finished"), "single: {single}");

    let batch = tool
        .execute(serde_json::json!({"tasks": [{"goal": "x", "timeout_seconds": 3}]}))
        .await
        .expect("batch path honors the model's 3s");
    assert!(batch.contains("finished"), "batch: {batch}");
}

/// CR-01: a model-supplied `timeout_seconds` of `u64::MAX` (unclamped per D-13)
/// must not panic the deadline arithmetic in either path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn huge_timeout_seconds_does_not_panic() {
    let tool = DelegateTaskTool::new(
        Arc::new(SleepThenReplyRunner {
            sleep: Duration::ZERO,
            reply: "finished",
        }),
        Arc::new(Semaphore::new(2)),
        None,
        config(60),
        Some(CancellationToken::new()),
    );

    let batch = tool
        .execute(serde_json::json!({"tasks": [{"goal": "x", "timeout_seconds": u64::MAX}]}))
        .await
        .expect("batch must survive a u64::MAX timeout");
    assert!(batch.contains("finished"), "batch: {batch}");

    let single = tool
        .execute(serde_json::json!({"task": "x", "timeout_seconds": u64::MAX}))
        .await
        .expect("single must survive a u64::MAX timeout");
    assert!(single.contains("finished"), "single: {single}");
}

/// WR-03: a parent-side abort must still tell the UI the child is done, or its
/// progress row stays "running" forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_abort_emits_completed_progress() {
    use ironhermes_tools::delegate_task::SubagentProgress;
    use std::sync::Mutex;

    let events: Arc<Mutex<Vec<(usize, bool)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let tool = DelegateTaskTool::new(
        Arc::new(BlockingRunner {
            observed_cancelled: Arc::new(AtomicBool::new(false)),
        }),
        Arc::new(Semaphore::new(1)),
        None,
        config(60),
        Some(CancellationToken::new()),
    )
    .with_progress_callback(Arc::new(move |i, ev| {
        sink.lock()
            .unwrap()
            .push((i, matches!(ev, SubagentProgress::Completed)));
    }));

    tool.execute(serde_json::json!({"tasks": [{"goal": "block", "timeout_seconds": 1}]}))
        .await
        .expect("batch returns Ok with a timed-out slot");

    assert!(
        events.lock().unwrap().contains(&(0, true)),
        "aborted child must emit Completed, got {:?}",
        events.lock().unwrap()
    );
}

/// WR-04: a task queued behind a sibling keeps its full run budget. Each task
/// has 2s; with one slot the second waits 1.5s then runs 1.5s (3s total from
/// spawn), which a single shared spawn-time deadline would have cut off.
#[tokio::test(start_paused = true)]
async fn queued_batch_child_keeps_full_run_budget() {
    let tool = DelegateTaskTool::new(
        Arc::new(SleepThenReplyRunner {
            sleep: Duration::from_millis(1500),
            reply: "finished",
        }),
        Arc::new(Semaphore::new(1)),
        None,
        config(60),
        Some(CancellationToken::new()),
    );

    let out = tool
        .execute(serde_json::json!({"tasks": [
            {"goal": "a", "timeout_seconds": 2},
            {"goal": "b", "timeout_seconds": 2},
        ]}))
        .await
        .expect("batch returns Ok");

    assert_eq!(out.matches("finished").count(), 2, "got: {out}");
    assert!(!out.contains("timed out"), "got: {out}");
}
