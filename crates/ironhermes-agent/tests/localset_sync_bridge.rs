//! Regression test for the Phase 41.3 UAT failure on the Web surface.
//!
//! `/agents` on the Dioxus web UI panicked with
//! `"can call blocking only when running on the multi-threaded runtime"` at
//! `subagent_registry.rs:371` (`tree_summary`). The Dioxus fullstack websocket
//! server-fn handler polls its future inside a per-connection `LocalSet`, where
//! `tokio::task::block_in_place` is illegal even though the underlying runtime
//! is multi-threaded.
//!
//! This is the same failure class that Phase 26.7-06 UAT hit and 26.7-07 fixed
//! for `RegistrationGuard::drop` (see the constraint comment on that type). The
//! `SubagentListSnapshot` / `ShrikeService` sync bridges were never given the
//! same treatment because until Phase 41.3 Plan 04 (`bc5f1c0c3`) the Web
//! `CommandContext` wired `subagent_registry: None` and the code was
//! unreachable from a `LocalSet`.
//!
//! Each test drives the sync trait method from inside a `LocalSet` — the exact
//! context Dioxus uses — and asserts it returns rather than panics.

use std::sync::Arc;
use tokio::sync::RwLock;

use ironhermes_agent::shrike::ShrikeService;
use ironhermes_agent::subagent_registry::{SubagentInfo, SubagentRegistry, SubagentRegistryHandle};
use ironhermes_core::commands::context::SubagentListSnapshot;

fn make_info(id: &str) -> SubagentInfo {
    SubagentInfo {
        id: id.to_string(),
        task_summary: format!("task for {id}"),
        parent_id: None,
        started_at: std::time::Instant::now(),
        cancel: tokio_util::sync::CancellationToken::new(),
        transcript_path: std::path::PathBuf::from("/dev/null"),
        activity_last: Some(Arc::new(std::sync::Mutex::new(std::time::Instant::now()))),
        stale_warn_seconds: 120,
    }
}

/// Build a registry holding one entry, plus the multi-thread runtime the Dioxus
/// server actually runs on.
fn fixture() -> (tokio::runtime::Runtime, Arc<RwLock<SubagentRegistry>>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build multi-thread runtime");
    let registry = Arc::new(RwLock::new(SubagentRegistry::new()));
    rt.block_on(async {
        let weak: std::sync::Weak<RwLock<SubagentRegistry>> = std::sync::Weak::new();
        let mut w = registry.write().await;
        // Dangling Weak so the guard's Drop is a silent no-op — these tests care
        // about the read bridge, not lifecycle.
        let guard = w.register_guarded(make_info("child-1"), weak);
        std::mem::forget(guard);
    });
    (rt, registry)
}

/// The exact UAT reproduction: `/agents` renders through `tree_summary()`.
#[test]
fn tree_summary_survives_a_localset() {
    let (rt, registry) = fixture();
    let handle = SubagentRegistryHandle::new(registry);
    let local = tokio::task::LocalSet::new();

    let entries = local.block_on(&rt, async move { handle.tree_summary() });

    assert_eq!(
        entries.len(),
        1,
        "tree_summary must return the registered child from inside a LocalSet"
    );
    assert_eq!(entries[0].id, "child-1");
}

/// `active_count` / `list_summary` / `transcript_path` share the same bridge and
/// are all reachable from the `/agents` render path.
#[test]
fn the_other_snapshot_reads_survive_a_localset() {
    let (rt, registry) = fixture();
    let handle = SubagentRegistryHandle::new(registry);
    let local = tokio::task::LocalSet::new();

    let (count, summary, transcript) = local.block_on(&rt, async move {
        (
            handle.active_count(),
            handle.list_summary(),
            handle.transcript_path("child-1"),
        )
    });

    assert_eq!(count, 1);
    assert_eq!(summary.len(), 1);
    assert!(transcript.is_some());
}

/// `/agents kill|interrupt|prune|status` route through `ShrikeService`, which
/// uses the same bridge.
#[test]
fn shrike_operations_survive_a_localset() {
    let (rt, registry) = fixture();
    let shrike = ShrikeService::new(registry);
    let local = tokio::task::LocalSet::new();

    let (status, interrupted, pruned) = local.block_on(&rt, async move {
        (
            shrike.status("child-1"),
            shrike.interrupt("child-1"),
            // stale_secs far in the future -> nothing is stale, but the bridge
            // still has to execute.
            shrike.prune(86_400),
        )
    });

    assert!(
        status.is_some(),
        "status must resolve from inside a LocalSet"
    );
    assert!(interrupted, "interrupt must find the registered child");
    assert!(pruned.is_empty(), "nothing should be stale at 24h");
}
