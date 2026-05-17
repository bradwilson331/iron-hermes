//! Phase 32.3 Plan 01 Task 2 — End-to-end runaway repro: the canonical
//! 6.7-hour ghost regression at the delegate_task.rs integration boundary.
//!
//! **The 6.7-hour ghost (live repro 2026-05-17):** A research subagent
//! `sub_20667cb71808` finished writing its output files at 02:52 AM but the
//! `SubagentRegistry::list()` snapshot kept reporting it as Active for
//! 24,150s (~6.7 hours) because `tokio::time::timeout` dropped the
//! `run_child` future before reaching the explicit `unregister` call on
//! `subagent_runner.rs:331`. Plan 01 closes the bug structurally via
//! `RegistrationGuard::drop`.
//!
//! **What this test exercises:** the exact same integration boundary that
//! produced the bug — `DelegateTaskTool::execute()` wrapping a runner future
//! with `tokio::time::timeout` (delegate_task.rs:803). The test uses a
//! synthetic `SubagentRunner` impl that holds a `RegistrationGuard`-style
//! RAII drop (an `Arc<AtomicUsize>` decremented on Drop) so the test is
//! self-contained within `ironhermes-tools` — `ironhermes-tools` cannot depend
//! on `ironhermes-agent` (circular). The structural drop-on-future-drop
//! contract is what `RegistrationGuard` relies on; this test locks the
//! contract at the same call site that produced the bug.
//!
//! Unit-level guard Drop coverage (natural / timeout / panic / cancel) for
//! the actual `RegistrationGuard` type lives in
//! `crates/ironhermes-agent/tests/registration_guard.rs`. This test is the
//! Wave 0 end-to-end repro that asserts the contract at the boundary.
//!
//! **Pitfall 1 (RESEARCH.md):** multi-thread runtime is required because
//! the actual `RegistrationGuard::drop` uses `tokio::task::block_in_place`.
//! We use `flavor = "multi_thread", worker_threads = 4"` here even though
//! the synthetic guard in this test doesn't need it — the assertion is
//! about the integration boundary, and that boundary uses block_in_place
//! in production.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ironhermes_core::SubagentConfig;
use ironhermes_tools::delegate_task::{
    ChildToolProgressCallback, DelegateTaskTool, SubagentRunner,
};
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

/// Synthetic RAII guard that mirrors `RegistrationGuard`'s Drop contract:
/// decrements the active-count atomic when dropped. Self-contained in this
/// test so it can live in `ironhermes-tools` (which cannot depend on
/// `ironhermes-agent` — circular).
struct ActiveCounterGuard {
    active: Arc<AtomicUsize>,
}

impl Drop for ActiveCounterGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runner that "registers" by bumping `active` on entry and binding an
/// `ActiveCounterGuard` for the lifetime of `run_child`. The guard mirrors
/// `RegistrationGuard`'s drop-on-future-drop contract.
struct RegisteringRunner {
    active: Arc<AtomicUsize>,
}

#[async_trait]
impl SubagentRunner for RegisteringRunner {
    async fn run_child(
        &self,
        _registry: Arc<RwLock<ToolRegistry>>,
        _system_prompt: String,
        _max_iterations: usize,
        _model_override: Option<&str>,
        _cancel_token: Option<CancellationToken>,
        _tool_progress: Option<ChildToolProgressCallback>,
    ) -> anyhow::Result<Option<String>> {
        // "Register": bump active count.
        self.active.fetch_add(1, Ordering::SeqCst);
        // Bind RAII guard — mirrors RegistrationGuard. On Drop (natural
        // return, error, tokio::time::timeout future-drop, panic, cancel)
        // the active count decrements.
        let _guard = ActiveCounterGuard {
            active: self.active.clone(),
        };

        // Hang forever — DelegateTaskTool's tokio::time::timeout will drop
        // this future at the configured deadline. When the future is dropped,
        // Rust unwinds the function locals: `_guard` drops, which decrements
        // `active`. THAT is the 6.7-hour ghost regression contract.
        tokio::time::sleep(Duration::from_secs(9999)).await;
        Ok(Some("never".to_string()))
    }
}

/// **THE CANONICAL 6.7-HOUR GHOST REGRESSION TEST.**
///
/// Spawn a child via the real `DelegateTaskTool::execute()` path with a
/// 200ms timeout. After timeout fires, assert the synthetic registry's
/// active count is back to 0 within 1500ms (loose Nyquist bound around the
/// plan's D-02 ~1s target). This locks the integration-boundary contract
/// that closes the bug.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_runaway_subagent_deregisters_after_timeout() {
    let active = Arc::new(AtomicUsize::new(0));
    let runner: Arc<dyn SubagentRunner> = Arc::new(RegisteringRunner {
        active: active.clone(),
    });

    // Build DelegateTaskTool with default SubagentConfig — child_timeout_seconds
    // stays at 300 per D-07. The per-call `timeout_seconds` arg below overrides
    // it for this test (1s is enough to fire while the runner sleeps for 9999s).
    let tool = DelegateTaskTool::new(
        runner,
        Arc::new(Semaphore::new(1)),
        None, // memory_manager
        SubagentConfig::default(),
        None, // parent_cancel_token
    );

    let start = Instant::now();
    let args = serde_json::json!({
        "task": "this will hang forever",
        "timeout_seconds": 1,
    });
    let result = tool.execute(args).await;
    let elapsed_after_timeout = start.elapsed();

    // The timeout MUST have fired.
    assert!(
        result.is_err(),
        "DelegateTaskTool::execute must return Err on timeout (got: {result:?})"
    );
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("timed out"),
        "error message must mention timeout; got: {err_msg}"
    );

    // The 6.7-hour ghost regression: active count must drop to 0 within ~1s
    // of the timeout firing (plan's D-02 target). Loose Nyquist bound is
    // 1500ms to tolerate scheduler jitter on busy CI runners.
    let deadline = Instant::now() + Duration::from_millis(1500);
    while active.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let final_active = active.load(Ordering::SeqCst);
    let total_elapsed = start.elapsed();

    assert_eq!(
        final_active, 0,
        "6.7-HOUR GHOST REGRESSION: synthetic registry's active count must \
         drop to 0 within 1.5s of tokio::time::timeout firing. \
         Current count: {final_active}. Elapsed since execute() started: \
         {total_elapsed:?}. Elapsed up to timeout-err return: {elapsed_after_timeout:?}. \
         This is the bug that left sub_20667cb71808 reporting Active for 24,150s \
         (~6.7 hours) on 2026-05-17. RegistrationGuard's Drop-on-future-drop \
         contract must hold at this integration boundary."
    );
    assert!(
        total_elapsed < Duration::from_millis(2500),
        "End-to-end runaway-cleanup wall-clock must be < 2.5s; got {total_elapsed:?}"
    );
}

/// Sibling assertion: prove the contract also holds on natural success.
/// (Belt-and-braces — the canonical bug is the timeout path, but a working
/// RAII contract MUST drop the guard on every exit path.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_runaway_repro_natural_completion_also_deregisters() {
    struct InstantRunner {
        active: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SubagentRunner for InstantRunner {
        async fn run_child(
            &self,
            _registry: Arc<RwLock<ToolRegistry>>,
            _system_prompt: String,
            _max_iterations: usize,
            _model_override: Option<&str>,
            _cancel_token: Option<CancellationToken>,
            _tool_progress: Option<ChildToolProgressCallback>,
        ) -> anyhow::Result<Option<String>> {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _guard = ActiveCounterGuard {
                active: self.active.clone(),
            };
            // Return almost immediately — natural completion path.
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(Some("done".to_string()))
        }
    }

    let active = Arc::new(AtomicUsize::new(0));
    let runner: Arc<dyn SubagentRunner> = Arc::new(InstantRunner {
        active: active.clone(),
    });
    let tool = DelegateTaskTool::new(
        runner,
        Arc::new(Semaphore::new(1)),
        None,
        SubagentConfig::default(),
        None,
    );
    let result = tool
        .execute(serde_json::json!({ "task": "quick task" }))
        .await
        .expect("natural completion must succeed");
    assert!(result.contains("done"));
    assert_eq!(
        active.load(Ordering::SeqCst),
        0,
        "natural-completion path must also leave active count at 0"
    );
}
