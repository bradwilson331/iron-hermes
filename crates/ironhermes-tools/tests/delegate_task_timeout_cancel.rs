//! Phase 21.7 Plan 09 Task 9-01 — D-08 / AI-SPEC Pitfall 5.
//!
//! Proves the `tokio::time::timeout` arm inside `DelegateTaskTool::execute`
//! hard-cancels the child subagent's `CancellationToken` before returning
//! the timeout error. Regression-locks the Pitfall 5 fix: `tokio::time::timeout`
//! alone does NOT cancel the inner future — the child agent would keep running
//! unless the timeout arm explicitly calls `.cancel()` on the child token.
//!
//! The test runs in real wall-clock (no `start_paused`) with a 1-second
//! timeout and a child that awaits `tokio::select!` on either a 10s sleep
//! or the cancel signal. If the fix is correct, the test finishes in
//! ~1.1s and the atomic flag is `true`. Before the fix, the test would
//! hang the full 10s (and miss the cancel signal entirely).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::SubagentConfig;
use ironhermes_tools::delegate_task::{
    ChildToolProgressCallback, DelegateTaskTool, SubagentRunner,
};
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

/// Mock runner that reports (via an atomic) whether its CancellationToken
/// was cancelled before the runner's own 10s sleep fired.
struct SleepRunner {
    observed_cancelled: Arc<AtomicBool>,
}

#[async_trait]
impl SubagentRunner for SleepRunner {
    async fn run_child(
        &self,
        _registry: Arc<RwLock<ToolRegistry>>,
        _system_prompt: String,
        _max_iterations: usize,
        _model_override: Option<&str>,
        cancel_token: Option<CancellationToken>,
        _tool_progress: Option<ChildToolProgressCallback>,
    ) -> anyhow::Result<Option<String>> {
        let observed = self.observed_cancelled.clone();
        let cancel = cancel_token.expect(
            "D-08 regression test: delegate_task must pass a CancellationToken \
             through to run_child so the timeout arm has something to cancel",
        );
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                Ok(Some("never".to_string()))
            }
            _ = cancel.cancelled() => {
                observed.store(true, Ordering::SeqCst);
                anyhow::bail!("cancelled via parent token")
            }
        }
    }
}

#[tokio::test]
async fn timeout_arm_hard_cancels_child_token_within_grace() {
    let observed = Arc::new(AtomicBool::new(false));
    let runner: Arc<dyn SubagentRunner> = Arc::new(SleepRunner {
        observed_cancelled: observed.clone(),
    });

    // Parent cancel token MUST exist for `child_cancel_token` to be
    // derived (DelegateTaskTool sets child = parent.child_token() when a
    // parent is present). Without the parent, child_cancel_token is None
    // and the timeout arm has nothing to cancel. The production CLI wires
    // a parent cancel token at run_chat / run_single; mirror that here.
    let parent_cancel = CancellationToken::new();

    let tool = DelegateTaskTool::new(
        runner,
        Arc::new(Semaphore::new(1)),
        None, // memory_manager
        SubagentConfig {
            timeout_secs: 60, // fallback if per-call override is absent
            max_iterations: 5,
            max_subagents: 2,
            ..SubagentConfig::default()
        },
        Some(parent_cancel),
    );

    // D-08: per-call `timeout_seconds` override. 1s means the tool's
    // timeout arm fires at ~1s while the child's sleep is 10s — only a
    // hard-cancel on the child token can return control within the
    // grace window asserted below.
    let args = serde_json::json!({
        "task": "sleep forever",
        "timeout_seconds": 1,
    });

    let start = std::time::Instant::now();
    let result = tool.execute(args).await;
    let elapsed = start.elapsed();

    assert!(
        result.is_err(),
        "D-08: per-call timeout_seconds=1 must produce Err, got Ok"
    );
    let err = result.err().unwrap().to_string();
    assert!(
        err.contains("timed out"),
        "D-08: error message must mention timeout, got: {err}"
    );
    assert!(
        elapsed < Duration::from_millis(2_500),
        "D-08 / Pitfall 5: timeout path must return in ~1s+grace (< 2.5s), \
         not wait for the 10s sleep. Elapsed: {elapsed:?}"
    );
    assert!(
        observed.load(Ordering::SeqCst),
        "D-08 / Pitfall 5: child CancellationToken MUST have been cancelled \
         by the timeout arm BEFORE returning. The observed-cancelled atomic \
         was false, which means tokio::time::timeout fired but the inner \
         future was never cancelled."
    );
}

#[tokio::test]
async fn timeout_seconds_per_call_overrides_config_default() {
    // Sibling contract test: prove the schema-exposed per-call override
    // wins over config.timeout_secs.
    let observed = Arc::new(AtomicBool::new(false));
    let runner: Arc<dyn SubagentRunner> = Arc::new(SleepRunner {
        observed_cancelled: observed.clone(),
    });

    let parent_cancel = CancellationToken::new();
    let tool = DelegateTaskTool::new(
        runner,
        Arc::new(Semaphore::new(1)),
        None,
        SubagentConfig {
            // Config default is huge; only the per-call override can
            // cut it down to 1s.
            timeout_secs: 3600,
            max_iterations: 5,
            max_subagents: 2,
            ..SubagentConfig::default()
        },
        Some(parent_cancel),
    );

    let start = std::time::Instant::now();
    let result = tool
        .execute(serde_json::json!({
            "task": "sleep forever",
            "timeout_seconds": 1,
        }))
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_err());
    assert!(
        elapsed < Duration::from_millis(2_500),
        "per-call timeout_seconds=1 must override config.timeout_secs=3600. \
         Elapsed: {elapsed:?}"
    );
}
