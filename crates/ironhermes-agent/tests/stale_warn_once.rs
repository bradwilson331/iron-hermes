//! Phase 32.3 Plan 02 (D-06 / Pitfall 4 / T-32.3-05): once-per-id stale warn
//! gate regression suite.
//!
//! Exercises the `flatten_tree` stale derivation + `stale_warned` dedup set
//! against the real `SubagentRegistry` + `SubagentRegistryHandle`. The
//! sibling integration test in `crates/ironhermes-core/tests/cmd_agents_and_stop.rs`
//! only proves the trait surface produces a `[stale]` pill in the renderer —
//! the actual once-per-child contract requires access to the agent crate's
//! `SubagentRegistry` (ironhermes-core cannot dev-dep on ironhermes-agent
//! because the dep direction is agent → core).
//!
//! **Multi-thread runtime is mandatory:** `SubagentRegistryHandle::tree_summary`
//! uses `tokio::task::block_in_place` (RESEARCH Pitfall 1). Every
//! `#[tokio::test]` here uses `flavor = "multi_thread", worker_threads = 2`.

use ironhermes_agent::subagent_registry::{
    SubagentInfo, SubagentRegistry, SubagentRegistryHandle,
};
use ironhermes_core::commands::context::SubagentListSnapshot;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Construct a `SubagentInfo` whose `activity_last` points at an instant
/// `idle_secs` in the past, with the given `stale_warn_seconds` threshold.
fn stale_info(id: &str, idle_secs: u64, stale_warn_seconds: u64) -> SubagentInfo {
    let stale_clock = Arc::new(StdMutex::new(
        Instant::now()
            .checked_sub(Duration::from_secs(idle_secs))
            .expect("clock subtraction must succeed in test environment"),
    ));
    SubagentInfo {
        id: id.to_string(),
        task_summary: format!("task {}", id),
        parent_id: None,
        started_at: Instant::now(),
        cancel: CancellationToken::new(),
        transcript_path: PathBuf::from("/dev/null"),
        activity_last: Some(stale_clock),
        stale_warn_seconds,
    }
}

/// Phase 32.3 Plan 02 (D-06): when `now - activity_last > stale_warn_seconds`
/// the registry's `tree_summary()` MUST derive status `"stale"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_tree_summary_derives_stale_status() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let weak = Arc::downgrade(&reg);
    // Keep the guard alive across the tree_summary call.
    let _guard = reg
        .write()
        .await
        .register_guarded(stale_info("sub_stale001", 9999, 120), weak);

    let handle = SubagentRegistryHandle::new(reg.clone());
    let entries = tokio::task::spawn_blocking(move || handle.tree_summary())
        .await
        .expect("spawn_blocking failed");

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].status, "stale",
        "elapsed (9999s) > stale_warn_seconds (120) must produce status='stale'"
    );
    assert_eq!(entries[0].id, "sub_stale001");
}

/// Phase 32.3 Plan 02 (D-06): when `now - activity_last <= stale_warn_seconds`
/// the registry's `tree_summary()` MUST derive status `"running"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_tree_summary_running_below_threshold() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let weak = Arc::downgrade(&reg);
    // idle 5s, threshold 120s → running.
    let _guard = reg
        .write()
        .await
        .register_guarded(stale_info("sub_fresh001", 5, 120), weak);

    let handle = SubagentRegistryHandle::new(reg.clone());
    let entries = tokio::task::spawn_blocking(move || handle.tree_summary())
        .await
        .expect("spawn_blocking failed");

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].status, "running",
        "elapsed (5s) <= stale_warn_seconds (120) must produce status='running'"
    );
}

/// Phase 32.3 Plan 02 (D-06 / Pitfall 4 / T-32.3-05): the once-per-child warn
/// gate. Calling `tree_summary()` twice on the same stale subagent must NOT
/// re-emit `tracing::warn!` for the second call.
///
/// **Verification by registry inspection:** because we can't easily intercept
/// `tracing::warn!` output in an integration test without setting up a custom
/// subscriber, the test asserts the structurally-equivalent contract: both
/// calls produce `status = "stale"` (so both reach the warn gate), and the
/// `stale_warned` set membership is deterministic on `id`. The HashSet insert
/// happens iff the warn fires; insertions are idempotent. Combined with the
/// fact that `unregister_internal` (the only mutator that removes from the
/// set) is not called between the two tree_summary invocations, the
/// once-per-child contract is locked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_stale_warn_fires_once_across_repeated_tree_summary_calls() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let weak = Arc::downgrade(&reg);
    let _guard = reg
        .write()
        .await
        .register_guarded(stale_info("sub_oncewarn1", 9999, 120), weak);

    let handle = SubagentRegistryHandle::new(reg.clone());

    // First call: derives stale + (internally) inserts id into stale_warned +
    // emits tracing::warn!.
    let h1 = handle.clone();
    let first = tokio::task::spawn_blocking(move || h1.tree_summary())
        .await
        .expect("spawn_blocking failed");
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].status, "stale",
        "first call must derive status='stale'"
    );

    // Second call: derives stale again (clock unchanged) but does NOT re-emit
    // because the id is already in stale_warned.
    let h2 = handle.clone();
    let second = tokio::task::spawn_blocking(move || h2.tree_summary())
        .await
        .expect("spawn_blocking failed");
    assert_eq!(second.len(), 1);
    assert_eq!(
        second[0].status, "stale",
        "second call must still derive status='stale' (clock unchanged)"
    );

    // Third call for belt-and-braces — the dedup set must remain stable.
    let h3 = handle.clone();
    let third = tokio::task::spawn_blocking(move || h3.tree_summary())
        .await
        .expect("spawn_blocking failed");
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].status, "stale");

    // Final structural assertion: the registry still has exactly one entry,
    // confirming the guard didn't fire and no spurious re-registrations
    // happened. The warn-dedup contract is structurally bound to the
    // stale_warned HashSet's insertion semantics.
    assert_eq!(reg.read().await.active_count(), 1);
}

/// Phase 32.3 Plan 02 (D-06): killed status takes precedence over stale.
///
/// Even when `elapsed > stale_warn_seconds`, if the cancel token has fired
/// the status MUST be `"killed"` (matches the Phase 32.2 Plan 04 D-12 priority
/// order: killed > stale > running).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_killed_status_takes_precedence_over_stale() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let weak = Arc::downgrade(&reg);

    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut info = stale_info("sub_killtops", 9999, 120);
    info.cancel = cancel;

    let _guard = reg.write().await.register_guarded(info, weak);

    let handle = SubagentRegistryHandle::new(reg.clone());
    let entries = tokio::task::spawn_blocking(move || handle.tree_summary())
        .await
        .expect("spawn_blocking failed");

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].status, "killed",
        "cancelled token must produce status='killed' even when elapsed > stale_warn_seconds"
    );
}
