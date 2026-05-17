//! D-03 / D-04 / D-09 SubagentRegistry contract tests.
//!
//! Phase 32.3 Plan 01 migration: `register` / `unregister` no longer exist on
//! the public API — `register_guarded` returns a `RegistrationGuard` whose
//! Drop runs `unregister_internal` (pub(crate)). These tests now exercise the
//! guard-driven lifecycle exclusively. End-to-end exit-path coverage
//! (natural / timeout / panic / cancel) lives in
//! `tests/registration_guard.rs`.

use ironhermes_agent::subagent_registry::{SubagentInfo, SubagentRegistry};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

fn mk(id: &str, cancel: CancellationToken) -> SubagentInfo {
    SubagentInfo {
        id: id.to_string(),
        task_summary: format!("task {}", id),
        parent_id: None,
        started_at: Instant::now(),
        cancel,
        transcript_path: PathBuf::from(format!("/tmp/{}.jsonl", id)),
        activity_last: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_then_drop_guard_leaves_count_zero() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let tok = CancellationToken::new();
    {
        let weak = Arc::downgrade(&reg);
        let _guard = reg.write().await.register_guarded(mk("sub1", tok), weak);
        // While the guard is alive, the entry is present.
        assert_eq!(reg.read().await.active_count(), 1);
        assert_eq!(reg.read().await.list().len(), 1);
        // Guard drops at end of block — block_in_place bridge runs unregister.
        // Detach the read lock first so the Drop's write().await won't deadlock.
    }
    // Give Drop's block_in_place a moment to settle.
    tokio::task::yield_now().await;
    assert_eq!(
        reg.read().await.active_count(),
        0,
        "RegistrationGuard::drop must deregister the entry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_cancels_token_and_removes_entry() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let tok = CancellationToken::new();
    let weak = Arc::downgrade(&reg);
    let guard = reg
        .write()
        .await
        .register_guarded(mk("sub1", tok.clone()), weak);

    assert!(
        reg.write().await.kill("sub1"),
        "kill must return true for present id"
    );
    assert!(tok.is_cancelled(), "D-03 kill must cancel the token");
    assert_eq!(reg.read().await.active_count(), 0);

    // Drop the guard explicitly to exercise the no-op upgrade-after-removal path.
    drop(guard);
    tokio::task::yield_now().await;
    // active_count still 0 — guard's unregister_internal on an absent id is a no-op.
    assert_eq!(reg.read().await.active_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_missing_returns_false() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    assert!(!reg.write().await.kill("nope"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcript_path_returns_stored_value() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let tok = CancellationToken::new();
    let weak = Arc::downgrade(&reg);
    let _guard = reg.write().await.register_guarded(mk("sub1", tok), weak);

    assert_eq!(
        reg.read().await.transcript_path("sub1"),
        Some(PathBuf::from("/tmp/sub1.jsonl"))
    );
    assert_eq!(reg.read().await.transcript_path("missing"), None);
    // Forget so this test's assertion isn't perturbed by the guard's Drop
    // racing the test-thread shutdown. Lifecycle coverage lives in
    // tests/registration_guard.rs.
    std::mem::forget(_guard);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_preserves_all_entries() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let mut guards = Vec::new();
    for id in ["a", "b", "c"] {
        let weak = Arc::downgrade(&reg);
        let g = reg
            .write()
            .await
            .register_guarded(mk(id, CancellationToken::new()), weak);
        guards.push(g);
    }
    assert_eq!(reg.read().await.active_count(), 3);
    let list = reg.read().await.list();
    assert_eq!(list.len(), 3);
    let ids: std::collections::HashSet<String> = list.into_iter().map(|i| i.id).collect();
    assert!(ids.contains("a") && ids.contains("b") && ids.contains("c"));
    // Drop guards inside the multi-thread runtime so block_in_place is legal.
    drop(guards);
    tokio::task::yield_now().await;
    assert_eq!(reg.read().await.active_count(), 0);
}
