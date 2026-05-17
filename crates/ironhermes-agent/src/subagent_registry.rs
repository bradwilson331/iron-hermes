//! In-memory session-scoped subagent registry (D-03, D-04, D-09).
//!
//! Populated by the existing `SubagentProgressCallback` in
//! crates/ironhermes-cli/src/main.rs — wired in Wave 2 Plan 07.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub type SubagentId = String;

/// Phase 32.2 Plan 04 (D-11): Node in the subagent spawn tree.
///
/// Produced by `SubagentRegistry::build_tree()`. Each node holds the full
/// `SubagentInfo` for that subagent plus a (possibly empty) list of children
/// that were spawned with this node's `id` as their `parent_id`.
#[derive(Debug, Clone)]
pub struct SubagentTreeNode {
    pub info: SubagentInfo,
    pub children: Vec<SubagentTreeNode>,
}

#[derive(Debug, Clone)]
pub struct SubagentInfo {
    pub id: SubagentId,
    pub task_summary: String,
    pub parent_id: Option<SubagentId>,
    pub started_at: Instant,
    pub cancel: CancellationToken,
    pub transcript_path: PathBuf,
    /// Phase 32.3 Plan 01 (D-04 reservation), Plan 02 (live wiring): shared
    /// live clock from `AgentLoop::activity_last_arc()`. Plan 01 reserved the
    /// field as `None`; Plan 02 populates it with the real
    /// `Arc<Mutex<Instant>>` from the child AgentLoop so the registry can
    /// compute live `activity_last.lock().elapsed()` without push updates.
    pub activity_last: Option<std::sync::Arc<std::sync::Mutex<std::time::Instant>>>,
    /// Phase 32.3 Plan 02 (D-05): effective stale warn threshold for THIS
    /// subagent, resolved at delegation time from
    /// `task_obj.stale_warn_seconds` (per-call) → `config.stale_warn_seconds`
    /// (fallback) → 120. Stored on the entry so the registry render code can
    /// compute `elapsed > stale_warn_seconds` without re-reading config.
    pub stale_warn_seconds: u64,
}

#[derive(Default)]
pub struct SubagentRegistry {
    active: HashMap<SubagentId, SubagentInfo>,
    /// Phase 32.3 Plan 01 (D-06 prerequisite), Plan 02 (interior mutability):
    /// IDs that have already emitted `tracing::warn!` for the stale-threshold
    /// crossing. Wrapped in `std::sync::Mutex` so the once-per-child gate can
    /// fire from `&self` read paths (`flatten_tree` via `tree_summary` takes
    /// `&self`). `unregister_internal` clears the id so a re-registration
    /// starts fresh (matches the canonical Plan 01 D-06 prerequisite contract;
    /// `SubagentId` is a unique nonce so cross-id leak is structurally
    /// impossible — T-32.3-04 accepted).
    stale_warned: std::sync::Mutex<HashSet<SubagentId>>,
}

/// Phase 32.3 Plan 01 (D-01 / D-02 / D-03): RAII guard returned by
/// `SubagentRegistry::register_guarded`. On `Drop` (any exit path — natural
/// return, error, `tokio::time::timeout` future-drop, panic, cancel,
/// `JoinHandle::abort`) the guard calls `unregister_internal` synchronously
/// via the existing `block_in_place` + `block_on` bridge pattern.
///
/// The guard holds a `Weak<RwLock<SubagentRegistry>>` so it does not extend
/// registry lifetime: if the Arc is gone (session shutdown), Drop is a silent
/// no-op which is the correct behavior.
///
/// **Constraint:** Drop calls `block_in_place` — only safe on the tokio
/// multi-thread runtime. Tests that exercise the Drop path must use
/// `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` or higher
/// (RESEARCH.md Pitfall 1).
pub struct RegistrationGuard {
    registry: std::sync::Weak<tokio::sync::RwLock<SubagentRegistry>>,
    id: SubagentId,
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        if let Some(arc) = self.registry.upgrade() {
            let id = self.id.clone();
            // Same block_in_place + block_on bridge as SubagentRegistryHandle::kill
            // (lines 187-190). Safe on multi-thread runtime; required because Rust
            // has no async Drop.
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async { arc.write().await.unregister_internal(&id) });
            });
        }
        // If upgrade() is None the registry's Arc has already been dropped
        // (session shutdown). Silent no-op — there is nothing to unregister.
    }
}

impl SubagentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Phase 32.3 Plan 01 (D-01 / D-03): Register a subagent and return a
    /// `RegistrationGuard`. The guard MUST be bound (e.g. `let _guard = ...`)
    /// for the lifetime that registration should cover; dropping the guard
    /// triggers `unregister_internal` synchronously.
    ///
    /// `weak` is `Arc::downgrade(&registry_arc)`, taken from the
    /// `Arc<RwLock<SubagentRegistry>>` that wraps this registry.
    pub fn register_guarded(
        &mut self,
        info: SubagentInfo,
        weak: std::sync::Weak<tokio::sync::RwLock<SubagentRegistry>>,
    ) -> RegistrationGuard {
        let id = info.id.clone();
        self.active.insert(id.clone(), info);
        RegistrationGuard { registry: weak, id }
    }

    /// Phase 32.3 Plan 01 (D-01): Called exclusively from
    /// `RegistrationGuard::drop`. External callers must not call this directly
    /// — that's why visibility is `pub(crate)`. Removing the id from
    /// `stale_warned` here preserves once-per-child warn semantics across
    /// re-registration (D-06 prerequisite consumed by Plan 02).
    pub(crate) fn unregister_internal(&mut self, id: &str) -> Option<SubagentInfo> {
        if let Ok(mut warned) = self.stale_warned.lock() {
            warned.remove(id);
        }
        self.active.remove(id)
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn list(&self) -> Vec<SubagentInfo> {
        self.active.values().cloned().collect()
    }

    /// D-03 `/agents kill <id>`. Returns true if the id was present.
    /// Cancels the stored `CancellationToken` and removes the entry.
    pub fn kill(&mut self, id: &str) -> bool {
        match self.active.remove(id) {
            Some(info) => {
                info.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn transcript_path(&self, id: &str) -> Option<PathBuf> {
        self.active.get(id).map(|i| i.transcript_path.clone())
    }

    pub fn get(&self, id: &str) -> Option<&SubagentInfo> {
        self.active.get(id)
    }

    /// Phase 32.2 Plan 04 (D-11): Build a nested spawn tree from the flat active map.
    ///
    /// Root nodes are those whose `parent_id` is `None`. Each root recursively
    /// collects its children via `children_of`. All levels are sorted by `started_at`
    /// for stable ordering — earlier-started nodes appear first. Ties (same instant,
    /// practically impossible with `Instant::now()`) preserve arbitrary insertion order.
    pub fn build_tree(&self) -> Vec<SubagentTreeNode> {
        let mut roots: Vec<SubagentTreeNode> = self
            .active
            .values()
            .filter(|info| info.parent_id.is_none())
            .map(|info| SubagentTreeNode {
                info: info.clone(),
                children: self.children_of(&info.id),
            })
            .collect();
        roots.sort_by_key(|n| n.info.started_at);
        roots
    }

    /// Recursively collect all direct children of `parent_id`, sorted by `started_at`.
    fn children_of(&self, parent_id: &str) -> Vec<SubagentTreeNode> {
        let mut children: Vec<SubagentTreeNode> = self
            .active
            .values()
            .filter(|info| info.parent_id.as_deref() == Some(parent_id))
            .map(|info| SubagentTreeNode {
                info: info.clone(),
                children: self.children_of(&info.id),
            })
            .collect();
        children.sort_by_key(|n| n.info.started_at);
        children
    }
}

/// Plan 21.7-07 (D-03 / D-09): newtype wrapper around
/// `Arc<RwLock<SubagentRegistry>>` implementing `SubagentListSnapshot`.
/// Newtype required by Rust's orphan rule (foreign trait on foreign type).
///
/// All methods are SYNC by the trait definition, but the underlying lock is
/// `tokio::sync::RwLock`. We use `tokio::task::block_in_place` +
/// `Handle::current().block_on` to bridge — the same pattern used by
/// `ironhermes-core/src/commands/handlers.rs` for `/models refresh`. Safe on
/// the tokio multi-thread runtime; locks uncontended in practice
/// (single-session registry).
#[derive(Clone)]
pub struct SubagentRegistryHandle(pub std::sync::Arc<tokio::sync::RwLock<SubagentRegistry>>);

impl SubagentRegistryHandle {
    pub fn new(reg: std::sync::Arc<tokio::sync::RwLock<SubagentRegistry>>) -> Self {
        Self(reg)
    }
}

/// Phase 32.2 Plan 04 (D-11): Recursive depth-first walker that flattens a
/// `SubagentTreeNode` slice into a `Vec<SubagentTreeEntry>`.
///
/// Each node is emitted before its children (pre-order). `depth` tracks the
/// nesting level (0 = root). `status` is derived from `info.cancel.is_cancelled()`:
/// cancelled → `"killed"`, otherwise → `"running"`.
fn flatten_tree(
    nodes: &[SubagentTreeNode],
    depth: usize,
    out: &mut Vec<ironhermes_core::commands::context::SubagentTreeEntry>,
) {
    for node in nodes {
        let status = if node.info.cancel.is_cancelled() {
            "killed".to_string()
        } else {
            "running".to_string()
        };
        out.push(ironhermes_core::commands::context::SubagentTreeEntry {
            id: node.info.id.clone(),
            task_summary: node.info.task_summary.clone(),
            uptime: node.info.started_at.elapsed(),
            status,
            parent_id: node.info.parent_id.clone(),
            depth,
        });
        flatten_tree(&node.children, depth + 1, out);
    }
}

impl ironhermes_core::commands::context::SubagentListSnapshot for SubagentRegistryHandle {
    fn active_count(&self) -> usize {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.0.read().await.active_count() })
        })
    }

    fn list_summary(&self) -> Vec<(String, String, std::time::Duration)> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let guard = self.0.read().await;
                guard
                    .list()
                    .into_iter()
                    .map(|info| {
                        let uptime = info.started_at.elapsed();
                        (info.id, info.task_summary, uptime)
                    })
                    .collect()
            })
        })
    }

    fn kill(&self, id: &str) -> bool {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.0.write().await.kill(id) })
        })
    }

    fn transcript_path(&self, id: &str) -> Option<PathBuf> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { self.0.read().await.transcript_path(id) })
        })
    }

    /// Phase 32.2 Plan 04 (D-11): override of the trait default — returns a
    /// depth-tagged flat list derived from the real parent–child tree.
    ///
    /// Uses `build_tree()` + `flatten_tree()` so depths and `parent_id` values
    /// reflect actual spawn relationships. Uses the same `block_in_place` +
    /// `block_on` async-to-sync bridge as `list_summary()`.
    fn tree_summary(&self) -> Vec<ironhermes_core::commands::context::SubagentTreeEntry> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let guard = self.0.read().await;
                let tree = guard.build_tree();
                let mut out = Vec::new();
                flatten_tree(&tree, 0, &mut out);
                out
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tokio_util::sync::CancellationToken;

    /// Build a minimal `SubagentInfo` for tests.
    fn make_info(id: &str, parent_id: Option<&str>) -> SubagentInfo {
        SubagentInfo {
            id: id.to_string(),
            task_summary: format!("task for {}", id),
            parent_id: parent_id.map(|s| s.to_string()),
            started_at: std::time::Instant::now(),
            cancel: CancellationToken::new(),
            transcript_path: PathBuf::from("/dev/null"),
            activity_last: None,
            // Phase 32.3 Plan 02 (D-05): default stale threshold; tests that
            // exercise stale derivation override this explicitly.
            stale_warn_seconds: 120,
        }
    }

    /// Helper for tests that hold a `&mut SubagentRegistry` directly and only
    /// assert tree shape / status derivation, NOT the guard lifecycle.
    ///
    /// Uses a dangling `Weak` so the guard's Drop is a silent no-op
    /// (`upgrade()` returns None). The guard itself is then forgotten so it
    /// does not fire at end-of-scope. Plan 01's new `tests/registration_guard.rs`
    /// integration suite is the canonical exercise of Drop behaviour against
    /// a live Arc — those tests use real `Arc::downgrade(...)` weaks.
    fn register_into_sync(reg: &mut SubagentRegistry, info: SubagentInfo) {
        let weak: std::sync::Weak<tokio::sync::RwLock<SubagentRegistry>> = std::sync::Weak::new();
        let guard = reg.register_guarded(info, weak);
        std::mem::forget(guard);
    }

    /// Async variant: tests that hold `Arc<RwLock<SubagentRegistry>>` and want
    /// to register without exercising the guard lifecycle. Uses a dangling
    /// Weak (same rationale as `register_into_sync`).
    async fn register_into_arc(
        reg_arc: &std::sync::Arc<tokio::sync::RwLock<SubagentRegistry>>,
        info: SubagentInfo,
    ) {
        let weak: std::sync::Weak<tokio::sync::RwLock<SubagentRegistry>> = std::sync::Weak::new();
        let guard = reg_arc.write().await.register_guarded(info, weak);
        std::mem::forget(guard);
    }

    // -----------------------------------------------------------------------
    // build_tree tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_tree_flat_when_no_parents() {
        // Three root agents (parent_id=None) → tree has 3 roots, each childless.
        let mut reg = SubagentRegistry::new();
        register_into_sync(&mut reg, make_info("a", None));
        register_into_sync(&mut reg, make_info("b", None));
        register_into_sync(&mut reg, make_info("c", None));

        let tree = reg.build_tree();
        assert_eq!(tree.len(), 3, "expected 3 root nodes");
        for node in &tree {
            assert!(
                node.children.is_empty(),
                "root node '{}' should have no children",
                node.info.id
            );
        }
    }

    #[test]
    fn test_build_tree_groups_by_parent() {
        // root → [child_a, child_b]
        let mut reg = SubagentRegistry::new();
        register_into_sync(&mut reg, make_info("root", None));
        register_into_sync(&mut reg, make_info("child_a", Some("root")));
        register_into_sync(&mut reg, make_info("child_b", Some("root")));

        let tree = reg.build_tree();
        assert_eq!(tree.len(), 1, "expected 1 root node");
        let root = &tree[0];
        assert_eq!(root.info.id, "root");
        assert_eq!(root.children.len(), 2, "root should have 2 children");

        let child_ids: Vec<&str> = root.children.iter().map(|n| n.info.id.as_str()).collect();
        assert!(child_ids.contains(&"child_a"));
        assert!(child_ids.contains(&"child_b"));
        for child in &root.children {
            assert_eq!(child.info.parent_id.as_deref(), Some("root"));
            assert!(child.children.is_empty());
        }
    }

    #[test]
    fn test_build_tree_three_levels() {
        // root → mid → leaf  (3-level chain)
        let mut reg = SubagentRegistry::new();
        register_into_sync(&mut reg, make_info("root", None));
        register_into_sync(&mut reg, make_info("mid", Some("root")));
        register_into_sync(&mut reg, make_info("leaf", Some("mid")));

        let tree = reg.build_tree();
        assert_eq!(tree.len(), 1, "expected 1 root");
        let root = &tree[0];
        assert_eq!(root.info.id, "root");
        assert_eq!(root.children.len(), 1, "root should have 1 child");
        let mid = &root.children[0];
        assert_eq!(mid.info.id, "mid");
        assert_eq!(mid.children.len(), 1, "mid should have 1 child (leaf)");
        let leaf = &mid.children[0];
        assert_eq!(leaf.info.id, "leaf");
        assert!(leaf.children.is_empty(), "leaf should have no children");
    }

    // -----------------------------------------------------------------------
    // tree_summary via SubagentRegistryHandle
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_tree_summary_handle_flattens_with_depth() {
        // Two-level tree: root → [child_running, child_killed]
        // Verify depths, parent_id references, and status derivation.
        let root_info = make_info("root", None);
        let child_running_info = make_info("child_run", Some("root"));
        let child_killed_token = CancellationToken::new();
        child_killed_token.cancel(); // pre-cancel → status should be "killed"
        let child_killed_info = SubagentInfo {
            id: "child_kill".to_string(),
            task_summary: "task for child_kill".to_string(),
            parent_id: Some("root".to_string()),
            started_at: std::time::Instant::now(),
            cancel: child_killed_token,
            transcript_path: PathBuf::from("/dev/null"),
            activity_last: None,
            stale_warn_seconds: 120,
        };

        let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
        register_into_arc(&reg, root_info).await;
        register_into_arc(&reg, child_running_info).await;
        register_into_arc(&reg, child_killed_info).await;

        let handle = SubagentRegistryHandle::new(reg);
        // tree_summary uses block_in_place; must be called inside a blocking-capable runtime context.
        // tokio::task::spawn_blocking bridges us into that context.
        let entries = tokio::task::spawn_blocking(move || {
            use ironhermes_core::commands::context::SubagentListSnapshot;
            handle.tree_summary()
        })
        .await
        .expect("spawn_blocking failed");

        // Should have 3 entries: 1 root (depth 0) + 2 children (depth 1)
        assert_eq!(entries.len(), 3, "expected 3 entries in flat output");

        let root_entry = entries.iter().find(|e| e.id == "root").expect("root missing");
        assert_eq!(root_entry.depth, 0);
        assert!(root_entry.parent_id.is_none());
        assert_eq!(root_entry.status, "running");

        let running_entry = entries
            .iter()
            .find(|e| e.id == "child_run")
            .expect("child_run missing");
        assert_eq!(running_entry.depth, 1);
        assert_eq!(running_entry.parent_id.as_deref(), Some("root"));
        assert_eq!(running_entry.status, "running");

        let killed_entry = entries
            .iter()
            .find(|e| e.id == "child_kill")
            .expect("child_kill missing");
        assert_eq!(killed_entry.depth, 1);
        assert_eq!(killed_entry.parent_id.as_deref(), Some("root"));
        assert_eq!(killed_entry.status, "killed");
    }
}
