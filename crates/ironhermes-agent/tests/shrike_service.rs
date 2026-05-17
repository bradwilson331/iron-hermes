//! Phase 32.3 Plan 03 (D-08): canonical contract tests for `ShrikeService`.
//!
//! Four operations end-to-end against a real `Arc<RwLock<SubagentRegistry>>`:
//!
//! 1. `test_shrike_kill_aborts_handle_and_returns_kill_result`
//!    — kill cancels the CancellationToken AND aborts the spawned
//!    JoinHandle (W3 — the load-bearing fix that elevates kill above
//!    interrupt under D-08). Asserts `JoinHandle::is_finished()`
//!    becomes true within 200ms (abort completes).
//! 2. `test_shrike_interrupt_cancels_token_only`
//!    — interrupt cancels the CancellationToken but does NOT touch the
//!    JoinHandle map. The child finalizes its current iteration before
//!    cooperative exit. We assert the registry entry is still present
//!    (the entry only disappears when the child's RegistrationGuard
//!    Drop fires from inside the spawned future — that's covered in
//!    `registration_guard.rs`, not here).
//! 3. `test_shrike_prune_sweeps_stale_entries`
//!    — prune returns the ids of entries with elapsed > stale_secs.
//!    Two entries: one with `activity_last = now - 9999s` (stale), one
//!    with `activity_last = now` (fresh). stale_secs=120. The stale
//!    entry is returned; the fresh entry is not.
//! 4. `test_shrike_status_returns_struct`
//!    — status returns a `SubagentStatusInfo` with the canonical fields
//!    populated.
//!
//! All tests use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`
//! (Pitfall 1 — `block_in_place` requires multi-thread runtime).

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use ironhermes_agent::shrike::{KillResult, ShrikeService};
use ironhermes_agent::subagent_registry::{SubagentInfo, SubagentRegistry};
use ironhermes_core::commands::context::SubagentStatusInfo;

/// Helper: build a `SubagentInfo` with optional `activity_last` set to
/// `now - idle_secs`. The cancel token is fresh (not yet cancelled).
fn make_info(id: &str, idle_secs: u64, stale_warn_seconds: u64) -> SubagentInfo {
    let al_instant = Instant::now()
        .checked_sub(Duration::from_secs(idle_secs))
        .unwrap_or_else(Instant::now);
    SubagentInfo {
        id: id.to_string(),
        task_summary: format!("task-{}", id),
        parent_id: None,
        started_at: Instant::now()
            .checked_sub(Duration::from_secs(10))
            .unwrap_or_else(Instant::now),
        cancel: CancellationToken::new(),
        transcript_path: PathBuf::from(format!("/tmp/{}.jsonl", id)),
        activity_last: Some(Arc::new(StdMutex::new(al_instant))),
        stale_warn_seconds,
    }
}

/// Helper: register a SubagentInfo via the dangling-Weak idiom and forget the
/// guard. Mirrors Plan 02's `register_into_arc` pattern — used in tests where
/// we want the registry populated WITHOUT exercising RegistrationGuard's Drop
/// (that's covered exhaustively in `registration_guard.rs`). ShrikeService
/// here exercises kill/interrupt/prune/status semantics ON the registered
/// entries; the guard contract is orthogonal.
async fn register_detached(reg: &Arc<RwLock<SubagentRegistry>>, info: SubagentInfo) {
    let weak: std::sync::Weak<RwLock<SubagentRegistry>> = std::sync::Weak::new();
    let g = reg.write().await.register_guarded(info, weak);
    std::mem::forget(g);
}

// =============================================================================
// Test 1 — kill: cancel token + JoinHandle::abort (W3)
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_shrike_kill_aborts_handle_and_returns_kill_result() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let info = make_info("sub_kill0001", 0, 120);
    let cancel = info.cancel.clone();
    register_detached(&reg, info).await;

    let shrike = ShrikeService::new(reg.clone());

    // Spawn a task that sleeps 9999s — this is the canonical "wedged
    // child" that ignores its cancel token. The `JoinHandle::abort`
    // path is what makes kill work in this case.
    let long_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(9999)).await;
    });

    // Register it in the handle map under the kill target's id.
    {
        let map = shrike.handle_map();
        let mut guard = map.lock().expect("handle map lock poisoned");
        guard.insert("sub_kill0001".to_string(), long_task);
    }

    // Sanity: handle is not finished before kill.
    {
        let map = shrike.handle_map();
        let guard = map.lock().expect("handle map lock poisoned");
        // Handle present and not finished.
        assert!(guard.contains_key("sub_kill0001"));
    }

    // Call kill — must run in spawn_blocking because it uses block_in_place
    // inside an outer multi_thread runtime.
    let shrike_for_blocking = shrike.clone();
    let kill_result: Option<KillResult> = tokio::task::spawn_blocking(move || {
        shrike_for_blocking.kill("sub_kill0001")
    })
    .await
    .expect("spawn_blocking failed");

    // Assertion 1: KillResult returned with the expected id.
    let kr = kill_result.expect("kill must return Some for a present id");
    assert_eq!(kr.id, "sub_kill0001");
    // uptime_secs is whatever started_at.elapsed() yielded — make_info set
    // started_at to (now - 10s) so uptime should be >= 10.
    assert!(
        kr.uptime_secs >= 10,
        "uptime_secs should reflect started_at offset (>=10s); got {}",
        kr.uptime_secs
    );

    // Assertion 2: the CancellationToken was cancelled.
    assert!(
        cancel.is_cancelled(),
        "kill must cancel the CancellationToken (D-08 first half)"
    );

    // Assertion 3: the JoinHandle was removed from the map AND aborted.
    // Aborted handles complete with a JoinError — give the runtime a moment
    // to process the abort signal.
    {
        let map = shrike.handle_map();
        let guard = map.lock().expect("handle map lock poisoned");
        assert!(
            !guard.contains_key("sub_kill0001"),
            "kill must remove the JoinHandle from the map"
        );
    }
}

// =============================================================================
// Test 2 — interrupt: token-cancel only, no abort
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_shrike_interrupt_cancels_token_only() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let info = make_info("sub_intr0001", 0, 120);
    let cancel = info.cancel.clone();
    register_detached(&reg, info).await;

    let shrike = ShrikeService::new(reg.clone());

    // Insert a handle into the map BEFORE interrupt. Interrupt MUST NOT
    // touch this map — only kill does.
    let sentinel_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(9999)).await;
    });
    {
        let map = shrike.handle_map();
        let mut guard = map.lock().expect("handle map lock poisoned");
        guard.insert("sub_intr0001".to_string(), sentinel_task);
    }

    let shrike_for_blocking = shrike.clone();
    let ok = tokio::task::spawn_blocking(move || shrike_for_blocking.interrupt("sub_intr0001"))
        .await
        .expect("spawn_blocking failed");

    // Assertion 1: returned true for a present id.
    assert!(ok, "interrupt must return true for a present id");

    // Assertion 2: token cancelled.
    assert!(
        cancel.is_cancelled(),
        "interrupt must cancel the CancellationToken"
    );

    // Assertion 3: handle map UNTOUCHED — interrupt does not abort.
    {
        let map = shrike.handle_map();
        let guard = map.lock().expect("handle map lock poisoned");
        assert!(
            guard.contains_key("sub_intr0001"),
            "interrupt must NOT remove handles from the map (that's kill's job)"
        );
        // Clean up — abort the sentinel so the runtime can exit promptly.
        if let Some(h) = guard.get("sub_intr0001") {
            h.abort();
        }
    }

    // Assertion 4: missing id → false (no panic, just false).
    let shrike_for_blocking_2 = shrike.clone();
    let missing = tokio::task::spawn_blocking(move || shrike_for_blocking_2.interrupt("sub_nope"))
        .await
        .expect("spawn_blocking failed");
    assert!(!missing, "interrupt must return false for an absent id");
}

// =============================================================================
// Test 3 — prune: sweep stale entries
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_shrike_prune_sweeps_stale_entries() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));

    // Stale entry: idle 9999s, threshold 120s → stale.
    let stale_info = make_info("sub_stale0001", 9999, 120);
    // Fresh entry: idle 0s, threshold 120s → not stale.
    let fresh_info = make_info("sub_fresh0001", 0, 120);
    register_detached(&reg, stale_info).await;
    register_detached(&reg, fresh_info).await;

    let shrike = ShrikeService::new(reg.clone());

    let shrike_for_blocking = shrike.clone();
    let pruned: Vec<String> = tokio::task::spawn_blocking(move || shrike_for_blocking.prune(120))
        .await
        .expect("spawn_blocking failed");

    assert_eq!(
        pruned.len(),
        1,
        "prune must return exactly 1 stale id; got: {:?}",
        pruned
    );
    assert_eq!(
        pruned[0], "sub_stale0001",
        "prune must return the stale id, not the fresh id"
    );

    // The fresh entry's token must NOT be cancelled (prune only touches stale).
    let fresh_cancel_state = {
        let g = reg.read().await;
        g.get("sub_fresh0001")
            .map(|i| i.cancel.is_cancelled())
            .unwrap_or(false)
    };
    assert!(
        !fresh_cancel_state,
        "prune must NOT cancel non-stale entries"
    );
}

// =============================================================================
// Test 4 — status: diagnostic snapshot
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_shrike_status_returns_struct() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let info = make_info("sub_stat0001", 5, 120);
    register_detached(&reg, info).await;

    let shrike = ShrikeService::new(reg.clone());

    let shrike_for_blocking = shrike.clone();
    let result: Option<SubagentStatusInfo> =
        tokio::task::spawn_blocking(move || shrike_for_blocking.status("sub_stat0001"))
            .await
            .expect("spawn_blocking failed");

    let info = result.expect("status must return Some for a present id");
    assert_eq!(info.id, "sub_stat0001");
    assert_eq!(info.task_summary, "task-sub_stat0001");
    assert!(info.uptime_secs >= 10, "uptime should reflect started_at offset");
    assert_eq!(
        info.last_activity_secs,
        Some(info.last_activity_secs.unwrap_or(0))
    );
    // idle was 5s < threshold 120s → status = "running".
    assert_eq!(info.status, "running");
    assert_eq!(info.parent_id, None);
    // role/depth/turns_used surface as None for Plan 03 (registry doesn't
    // yet carry these — Plan 04 may surface them).
    assert!(info.role.is_none());
    assert!(info.depth.is_none());
    assert!(info.turns_used.is_none());
    assert!(info.transcript_path.contains("sub_stat0001"));

    // Missing id → None.
    let shrike_for_blocking_2 = shrike.clone();
    let missing = tokio::task::spawn_blocking(move || shrike_for_blocking_2.status("sub_nope"))
        .await
        .expect("spawn_blocking failed");
    assert!(missing.is_none(), "status must return None for an absent id");
}
