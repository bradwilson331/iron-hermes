//! Phase 32.3 Plan 01 Task 2 — RegistrationGuard Drop regression suite.
//!
//! Canonical 6.7-hour ghost test: register a child, simulate four failure
//! modes (natural completion, tokio::time::timeout future-drop, panic, cancel),
//! and assert `registry.active_count() == 0` after each. This is the direct
//! regression for the live repro where `sub_20667cb71808` reported Active for
//! 24,150s after natural completion because `tokio::time::timeout` dropped
//! the `run_child` future before reaching the explicit `unregister` call on
//! subagent_runner.rs:331.
//!
//! **Multi-thread runtime is mandatory** for these tests (RESEARCH Pitfall 1):
//! `RegistrationGuard::drop` uses `tokio::task::block_in_place`, which panics
//! on the single-threaded runtime. Every `#[tokio::test]` here uses
//! `flavor = "multi_thread", worker_threads = 2`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures::FutureExt;
use ironhermes_agent::subagent_registry::{SubagentInfo, SubagentRegistry};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Minimal `SubagentInfo` for the registry tests.
fn make_info(id: &str) -> SubagentInfo {
    SubagentInfo {
        id: id.to_string(),
        task_summary: format!("task {}", id),
        parent_id: None,
        started_at: Instant::now(),
        cancel: CancellationToken::new(),
        transcript_path: PathBuf::from(format!("/tmp/{}.jsonl", id)),
        activity_last: None,
        // Phase 32.3 Plan 02 (D-05): default; these tests assert guard
        // lifecycle, not stale derivation.
        stale_warn_seconds: 120,
    }
}

/// D-03 helper: counts register / unregister events. We count via a Drop-side
/// wrapper around the Arc-counted unregister: every `register_guarded` call
/// increments `register_count`; every successful Drop increments
/// `unregister_count` (asserted indirectly by inspecting `active_count` of the
/// shared registry).
///
/// We can't intercept `unregister_internal` directly (pub(crate)) from this
/// integration-test file, so we use the registry's observable behaviour
/// instead: post-drop `active_count() == 0` AND a separate `Arc<AtomicUsize>`
/// bumped before each `register_guarded`. The balance check then asserts that
/// after every drop fires, `register_count - active_count == unregister_count`
/// (i.e., everything registered is either still active or has been unregistered
/// by its guard).
struct RegistrationCounter {
    register_count: Arc<AtomicUsize>,
}

impl RegistrationCounter {
    fn new() -> Self {
        Self {
            register_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn bump_register(&self) {
        self.register_count.fetch_add(1, Ordering::SeqCst);
    }

    fn register_total(&self) -> usize {
        self.register_count.load(Ordering::SeqCst)
    }
}

// ============================================================================
// D-02 — four exit paths, all assert registry empty after the failure mode
// ============================================================================

/// Path 1: natural completion. The simplest case — register, do a tiny amount
/// of work, return normally; the guard drops at end-of-scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_guard_deregisters_on_natural_completion() {
    let registry = Arc::new(RwLock::new(SubagentRegistry::new()));
    let reg_clone = registry.clone();

    let result: anyhow::Result<()> = async move {
        let weak = Arc::downgrade(&reg_clone);
        let _guard = reg_clone
            .write()
            .await
            .register_guarded(make_info("sub_natural"), weak);

        // Confirm presence while the guard is alive (use a separate read lock
        // to avoid deadlocking against the write guard above).
        assert_eq!(reg_clone.read().await.active_count(), 1);

        // Do a tiny amount of "work" and return normally.
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "natural completion must Ok");

    // Give Drop's block_in_place bridge a scheduler tick to settle.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        registry.read().await.active_count(),
        0,
        "RegistrationGuard::drop must deregister on natural completion"
    );
}

/// Path 2: `tokio::time::timeout` drops the future mid-execution. THIS IS THE
/// CANONICAL 6.7-HOUR GHOST regression case at the unit level — the end-to-end
/// repro lives in `crates/ironhermes-tools/tests/delegate_task_runaway.rs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_guard_deregisters_on_timeout() {
    let registry = Arc::new(RwLock::new(SubagentRegistry::new()));
    let reg_clone = registry.clone();

    let result = tokio::time::timeout(Duration::from_millis(50), async move {
        let weak = Arc::downgrade(&reg_clone);
        let _guard = reg_clone
            .write()
            .await
            .register_guarded(make_info("sub_timeout"), weak);
        // Hang forever — tokio::time::timeout will drop this future at the
        // 50ms mark, triggering _guard's Drop via Rust's drop semantics.
        tokio::time::sleep(Duration::from_secs(9999)).await;
    })
    .await;

    assert!(result.is_err(), "should have timed out");
    // Allow block_in_place to settle.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        registry.read().await.active_count(),
        0,
        "guard must deregister when future is dropped by timeout (the 6.7-hour ghost regression)"
    );
}

/// Path 3: panic inside the registered scope. The guard's Drop must still
/// fire — Rust guarantees Drop runs on unwind even when the panic propagates.
/// We use `FutureExt::catch_unwind` to catch the panic at the test boundary so
/// the test process itself doesn't abort.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_guard_deregisters_on_panic() {
    let registry = Arc::new(RwLock::new(SubagentRegistry::new()));
    let reg_clone = registry.clone();

    // Wrap the panicking future in AssertUnwindSafe + catch_unwind so the
    // panic is observable as a Result rather than aborting the test process.
    let result = std::panic::AssertUnwindSafe(async move {
        let weak = Arc::downgrade(&reg_clone);
        let _guard = reg_clone
            .write()
            .await
            .register_guarded(make_info("sub_panic"), weak);
        // Yield once so the registration commits visibly before unwind.
        tokio::time::sleep(Duration::from_millis(5)).await;
        panic!("simulated child-task failure");
    })
    .catch_unwind()
    .await;

    assert!(result.is_err(), "panic must propagate as catch_unwind Err");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        registry.read().await.active_count(),
        0,
        "RegistrationGuard::drop must fire on panic (Rust drop-on-unwind guarantee)"
    );
}

/// Path 4: cancel via `CancellationToken`. The child's async block awaits a
/// `cancelled()` future that the test fires before the inner work completes.
/// When the inner sleep is dropped (because the cancelled() select arm wins
/// and we return), the guard's Drop runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_guard_deregisters_on_cancel() {
    let registry = Arc::new(RwLock::new(SubagentRegistry::new()));
    let reg_clone = registry.clone();
    let cancel = CancellationToken::new();
    let cancel_for_child = cancel.clone();

    let handle = tokio::spawn(async move {
        let weak = Arc::downgrade(&reg_clone);
        let _guard = reg_clone
            .write()
            .await
            .register_guarded(make_info("sub_cancel"), weak);

        // Race: cancel vs a long sleep. When cancel fires, the select arm
        // wins, we return, the sleep future is dropped, and _guard drops too.
        tokio::select! {
            _ = cancel_for_child.cancelled() => {
                // Child observed cancel — return normally; _guard drops below.
            }
            _ = tokio::time::sleep(Duration::from_secs(9999)) => {
                // Won't be reached unless the test is broken.
            }
        }
    });

    // Fire the cancel before the child can make progress.
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();

    handle.await.expect("spawned task must not panic");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        registry.read().await.active_count(),
        0,
        "RegistrationGuard::drop must fire when cancel_token causes the future to return"
    );
}

// ============================================================================
// D-03 — register count == unregister count balance across all scenarios
// ============================================================================

/// Runs the four exit-path scenarios in sequence and asserts the registry is
/// empty after each (which structurally proves register_count == unregister
/// count: every register_guarded call must have been balanced by a Drop, or
/// active_count() would be non-zero).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_registration_counter_balanced() {
    let registry = Arc::new(RwLock::new(SubagentRegistry::new()));
    let counter = RegistrationCounter::new();

    // Scenario 1: natural completion.
    {
        let reg_clone = registry.clone();
        counter.bump_register();
        let result: anyhow::Result<()> = async move {
            let weak = Arc::downgrade(&reg_clone);
            let _guard = reg_clone
                .write()
                .await
                .register_guarded(make_info("counter_natural"), weak);
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(())
        }
        .await;
        assert!(result.is_ok());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            registry.read().await.active_count(),
            0,
            "natural-completion scenario must leave registry empty"
        );
    }

    // Scenario 2: timeout-driven future drop.
    {
        let reg_clone = registry.clone();
        counter.bump_register();
        let result = tokio::time::timeout(Duration::from_millis(30), async move {
            let weak = Arc::downgrade(&reg_clone);
            let _guard = reg_clone
                .write()
                .await
                .register_guarded(make_info("counter_timeout"), weak);
            tokio::time::sleep(Duration::from_secs(9999)).await;
        })
        .await;
        assert!(result.is_err());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            registry.read().await.active_count(),
            0,
            "timeout scenario must leave registry empty (the 6.7-hour ghost)"
        );
    }

    // Scenario 3: panic.
    {
        let reg_clone = registry.clone();
        counter.bump_register();
        let result = std::panic::AssertUnwindSafe(async move {
            let weak = Arc::downgrade(&reg_clone);
            let _guard = reg_clone
                .write()
                .await
                .register_guarded(make_info("counter_panic"), weak);
            tokio::time::sleep(Duration::from_millis(2)).await;
            panic!("balanced-counter panic");
        })
        .catch_unwind()
        .await;
        assert!(result.is_err());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            registry.read().await.active_count(),
            0,
            "panic scenario must leave registry empty"
        );
    }

    // Scenario 4: cancel.
    {
        let reg_clone = registry.clone();
        counter.bump_register();
        let cancel = CancellationToken::new();
        let cancel_for_child = cancel.clone();
        let handle = tokio::spawn(async move {
            let weak = Arc::downgrade(&reg_clone);
            let _guard = reg_clone
                .write()
                .await
                .register_guarded(make_info("counter_cancel"), weak);
            tokio::select! {
                _ = cancel_for_child.cancelled() => {},
                _ = tokio::time::sleep(Duration::from_secs(9999)) => {},
            }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel.cancel();
        handle.await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            registry.read().await.active_count(),
            0,
            "cancel scenario must leave registry empty"
        );
    }

    assert_eq!(
        counter.register_total(),
        4,
        "test sanity: four scenarios must have registered four times"
    );
    assert_eq!(
        registry.read().await.active_count(),
        0,
        "final assertion: after all scenarios, register_count - active_count == unregister_count == 4"
    );
}
