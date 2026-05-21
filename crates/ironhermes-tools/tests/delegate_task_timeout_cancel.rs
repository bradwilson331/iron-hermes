//! Phase 21.7 Plan 09 Task 9-01 — D-08 / AI-SPEC Pitfall 5.
//!
//! Proves the `tokio::time::timeout` arm inside `DelegateTaskTool::execute`
//! hard-cancels the child subagent's `CancellationToken` before returning
//! the timeout error. Regression-locks the Pitfall 5 fix: `tokio::time::timeout`
//! alone does NOT cancel the inner future in a way the child's detached
//! sub-tasks can observe — the child agent would keep running unless the
//! timeout arm explicitly calls `.cancel()` on the shared token.
//!
//! The runner spawns a DETACHED `tokio::spawn` task that outlives the outer
//! `run_child` future and flips the atomic when the cancel token fires. This
//! simulates real AgentLoop behaviour where inner tasks (provider calls,
//! tool execution) run on spawned tasks whose lifetime is controlled by the
//! shared cancel token, NOT by `Drop` of the outer future.

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

/// Mock runner that spawns a DETACHED task watching the cancel token, then
/// sleeps on the foreground path. The detached task flips the atomic when
/// the token is cancelled — proving the token was explicitly `.cancel()`ed
/// rather than merely dropped by `tokio::time::timeout`.
struct DetachedSleepRunner {
    observed_cancelled: Arc<AtomicBool>,
}

#[async_trait]
impl SubagentRunner for DetachedSleepRunner {
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
        let cancel = cancel_token.expect(
            "D-08 regression test: delegate_task must pass a CancellationToken \
             through to run_child so the timeout arm has something to cancel",
        );

        // Detached watcher — a clone of the token lives past `run_child`
        // being dropped by `tokio::time::timeout`. The only way the
        // watcher's `cancelled().await` returns is via an explicit
        // `.cancel()` call on the shared token.
        let watcher_cancel = cancel.clone();
        tokio::spawn(async move {
            watcher_cancel.cancelled().await;
            observed.store(true, Ordering::SeqCst);
        });

        // Foreground: long sleep that `tokio::time::timeout` will drop when
        // the deadline fires. The detached watcher above is what actually
        // observes the cancel.
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(Some("never".to_string()))
    }
}

#[tokio::test]
async fn timeout_arm_hard_cancels_child_token_within_grace() {
    let observed = Arc::new(AtomicBool::new(false));
    let runner: Arc<dyn SubagentRunner> = Arc::new(DetachedSleepRunner {
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
            child_timeout_seconds: 60, // fallback if per-call override is absent
            max_iterations: 5,
            max_concurrent_children: 2,
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

    // Give the detached watcher task a scheduler tick to observe the
    // cancel signal. Cancellation is synchronous on the sender side
    // (watch-channel semantic) but the spawned task needs to poll.
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        observed.load(Ordering::SeqCst),
        "D-08 / Pitfall 5: child CancellationToken MUST have been cancelled \
         by the timeout arm BEFORE returning. The observed-cancelled atomic \
         was false, which means tokio::time::timeout fired but the shared \
         token was never `.cancel()`ed — a detached child sub-task would \
         keep running with provider access despite the parent timeout."
    );
}

#[tokio::test]
async fn timeout_seconds_per_call_overrides_config_default() {
    // Sibling contract test: prove the schema-exposed per-call override
    // wins over config.timeout_secs.
    let observed = Arc::new(AtomicBool::new(false));
    let runner: Arc<dyn SubagentRunner> = Arc::new(DetachedSleepRunner {
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
            child_timeout_seconds: 3600,
            max_iterations: 5,
            max_concurrent_children: 2,
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

#[tokio::test]
async fn schema_exposes_timeout_seconds_field() {
    // Schema contract: consumers must see `timeout_seconds` as an optional
    // integer property on the delegate_task schema.
    struct DummyRunner;
    #[async_trait]
    impl SubagentRunner for DummyRunner {
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
            Ok(None)
        }
    }
    let tool = DelegateTaskTool::new(
        Arc::new(DummyRunner),
        Arc::new(Semaphore::new(1)),
        None,
        SubagentConfig::default(),
        None,
    );
    let schema = tool.schema();
    let props = &schema.function.parameters["properties"];
    assert!(
        props.get("timeout_seconds").is_some(),
        "D-08: schema must expose optional `timeout_seconds` property"
    );
    assert_eq!(
        props["timeout_seconds"]["type"].as_str(),
        Some("integer"),
        "D-08: timeout_seconds must be typed as integer"
    );
    // Must NOT be in `required` — per-call override is optional.
    let required = schema.function.parameters["required"].as_array().unwrap();
    for v in required {
        assert_ne!(
            v.as_str(),
            Some("timeout_seconds"),
            "D-08: timeout_seconds is optional, must not be in required"
        );
    }
}
