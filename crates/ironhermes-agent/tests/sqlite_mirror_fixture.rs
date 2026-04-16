//! Phase 20-04 Task 20-04-03: End-to-end fixture proving MemoryManager fires
//! on_memory_write through to a mirror provider when sqlite is the primary.
//!
//! Covers D-14, D-25..D-29 (mirror semantics) and T-20-07 (observability).
//!
//! Four scenarios:
//! 1. Primary write propagates to mirror via on_memory_write (Add).
//! 2. Multi-op sequence (Add -> Replace -> Remove) propagates in order.
//! 3. Failing mirror does NOT block primary writes (Err swallowed + logged).
//! 4. Read paths (prefetch) NEVER fan out to the mirror.

#![cfg(feature = "memory-sqlite")]

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use tokio::sync::Mutex;

use ironhermes_core::config_schema::MemoryAction;
use ironhermes_core::memory_provider::{MemoryEntries, MemoryProvider};
use ironhermes_core::memory_store::{MemoryResult, MemoryTarget};

use ironhermes_agent::memory::manager::{MemoryManager, SharedProvider};
use memory_sqlite::SqliteMemoryProvider;

// =============================================================================
// MockMirrorProvider — records every on_memory_write + counts read attempts.
// Optionally returns Err from on_memory_write to exercise the failure-swallow
// path (D-14 — mirror errors are logged, not propagated).
// =============================================================================

#[derive(Default)]
struct MirrorInner {
    writes: Vec<(MemoryAction, MemoryTarget, String)>,
    read_calls: usize,
}

struct MockMirrorProvider {
    inner: Arc<StdMutex<MirrorInner>>,
    fail_on_write: bool,
}

impl MockMirrorProvider {
    fn new() -> (Self, Arc<StdMutex<MirrorInner>>) {
        let inner = Arc::new(StdMutex::new(MirrorInner::default()));
        (
            Self {
                inner: Arc::clone(&inner),
                fail_on_write: false,
            },
            inner,
        )
    }

    fn failing() -> (Self, Arc<StdMutex<MirrorInner>>) {
        let inner = Arc::new(StdMutex::new(MirrorInner::default()));
        (
            Self {
                inner: Arc::clone(&inner),
                fail_on_write: true,
            },
            inner,
        )
    }
}

#[async_trait]
impl MemoryProvider for MockMirrorProvider {
    fn name(&self) -> &'static str {
        "mock-mirror"
    }

    async fn initialize(
        &mut self,
        _session_id: &str,
        _hermes_home: &Path,
        _provider_config: &serde_json::Value,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
        // Mirror MUST NOT receive reads (D-26, D-28). If this ever fires
        // the test assertion on read_calls == 0 will catch it.
        self.inner.lock().unwrap().read_calls += 1;
        Ok(MemoryEntries::default())
    }

    async fn sync_turn(&self, _s: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
        Ok(())
    }
    async fn on_session_end(&self, _s: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
        Ok(())
    }
    async fn shutdown(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn load_from_disk(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn add(&mut self, _t: MemoryTarget, _c: &str) -> MemoryResult {
        Ok("{}".to_string())
    }
    fn replace(&mut self, _t: MemoryTarget, _o: &str, _n: &str) -> MemoryResult {
        Ok("{}".to_string())
    }
    fn remove(&mut self, _t: MemoryTarget, _o: &str) -> MemoryResult {
        Ok("{}".to_string())
    }

    fn format_for_system_prompt(&self, _t: MemoryTarget) -> Option<String> {
        // Not a read path exercised in this fixture, but count it anyway so
        // any future accidental wiring is caught by the read-isolation test.
        self.inner.lock().unwrap().read_calls += 1;
        None
    }
    fn to_memory_entries(&self) -> MemoryEntries {
        MemoryEntries::default()
    }

    async fn on_memory_write(
        &mut self,
        action: MemoryAction,
        target: MemoryTarget,
        content: &str,
    ) -> anyhow::Result<()> {
        self.inner
            .lock()
            .unwrap()
            .writes
            .push((action, target, content.to_string()));
        if self.fail_on_write {
            anyhow::bail!("mirror kaput")
        } else {
            Ok(())
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn sqlite_primary() -> (SharedProvider, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("mem.db");
    let provider = SqliteMemoryProvider::new(&db_path).unwrap();
    let arc: SharedProvider = Arc::new(Mutex::new(provider));
    (arc, dir)
}

// =============================================================================
// Tests
// =============================================================================

#[tokio::test]
async fn sqlite_primary_fires_on_memory_write_to_mirror() {
    let (primary, _db_dir) = sqlite_primary();

    let (mirror_provider, mirror_state) = MockMirrorProvider::new();
    let mirror: SharedProvider = Arc::new(Mutex::new(mirror_provider));

    let manager = MemoryManager::new(primary.clone(), Some(mirror))
        .await
        .unwrap();

    let args = serde_json::json!({ "target": "memory", "content": "fact-1" });
    let res = manager.handle_tool_call("memory_add", args).await;
    assert!(res.is_ok(), "primary write must succeed: {:?}", res);

    // Mirror recorded exactly one on_memory_write invocation with Add.
    let writes = mirror_state.lock().unwrap().writes.clone();
    assert_eq!(writes.len(), 1, "expected exactly 1 mirror write");
    assert_eq!(writes[0].0, MemoryAction::Add);
    assert_eq!(writes[0].1, MemoryTarget::Memory);
    assert_eq!(writes[0].2, "fact-1");

    // Primary actually persisted the entry (reads via primary.prefetch).
    let entries = manager.prefetch("test-session").await.unwrap();
    let mem = entries
        .entries
        .get(&MemoryTarget::Memory)
        .expect("MEMORY entries must be present");
    assert!(
        mem.iter().any(|e| e == "fact-1"),
        "sqlite primary did not persist fact-1; got: {:?}",
        mem
    );

    // Mirror MUST NOT have received any reads.
    assert_eq!(
        mirror_state.lock().unwrap().read_calls,
        0,
        "mirror received read calls — D-26/D-28 violated",
    );
}

#[tokio::test]
async fn mirror_observes_replace_and_remove() {
    let (primary, _db_dir) = sqlite_primary();

    let (mirror_provider, mirror_state) = MockMirrorProvider::new();
    let mirror: SharedProvider = Arc::new(Mutex::new(mirror_provider));

    let manager = MemoryManager::new(primary, Some(mirror)).await.unwrap();

    // Add
    manager
        .handle_tool_call(
            "memory_add",
            serde_json::json!({ "target": "memory", "content": "fact-1" }),
        )
        .await
        .expect("add ok");

    // Replace
    manager
        .handle_tool_call(
            "memory_replace",
            serde_json::json!({
                "target": "memory",
                "old_text": "fact-1",
                "new_content": "fact-1-revised",
            }),
        )
        .await
        .expect("replace ok");

    // Remove
    manager
        .handle_tool_call(
            "memory_remove",
            serde_json::json!({
                "target": "memory",
                "old_text": "fact-1-revised",
            }),
        )
        .await
        .expect("remove ok");

    let writes = mirror_state.lock().unwrap().writes.clone();
    assert_eq!(
        writes.len(),
        3,
        "expected 3 mirror invocations (Add/Replace/Remove); got {:?}",
        writes
    );
    assert_eq!(writes[0].0, MemoryAction::Add);
    assert_eq!(writes[0].2, "fact-1");
    assert_eq!(writes[1].0, MemoryAction::Replace);
    assert_eq!(writes[1].2, "fact-1-revised");
    assert_eq!(writes[2].0, MemoryAction::Remove);
    assert_eq!(writes[2].2, "fact-1-revised");
    // Target consistent across the sequence.
    for (_, target, _) in &writes {
        assert_eq!(*target, MemoryTarget::Memory);
    }
}

#[tokio::test]
async fn failing_mirror_does_not_block_sqlite_writes() {
    let (primary, _db_dir) = sqlite_primary();

    let (mirror_provider, mirror_state) = MockMirrorProvider::failing();
    let mirror: SharedProvider = Arc::new(Mutex::new(mirror_provider));

    let manager = MemoryManager::new(primary.clone(), Some(mirror))
        .await
        .unwrap();

    // (a) Outer handle_tool_call must be Ok(_) — mirror failure is swallowed.
    let res = manager
        .handle_tool_call(
            "memory_add",
            serde_json::json!({ "target": "memory", "content": "fact-1" }),
        )
        .await;
    assert!(
        res.is_ok(),
        "failing mirror must not block primary write: {:?}",
        res
    );

    // (b) Primary sqlite actually persisted the entry.
    let entries = manager.prefetch("test-session").await.unwrap();
    let mem = entries
        .entries
        .get(&MemoryTarget::Memory)
        .expect("MEMORY entries present");
    assert!(
        mem.iter().any(|e| e == "fact-1"),
        "sqlite primary did not persist fact-1 despite successful outer Ok; got: {:?}",
        mem
    );

    // (c) Mirror's on_memory_write counter is still 1 — the call was made,
    //     the Err was swallowed by the manager (D-14).
    let writes = mirror_state.lock().unwrap().writes.clone();
    assert_eq!(
        writes.len(),
        1,
        "mirror.on_memory_write must have been invoked exactly once (err swallowed): {:?}",
        writes
    );
    assert_eq!(writes[0].0, MemoryAction::Add);
    assert_eq!(writes[0].2, "fact-1");
}

#[tokio::test]
async fn mirror_never_receives_reads() {
    let (primary, _db_dir) = sqlite_primary();

    let (mirror_provider, mirror_state) = MockMirrorProvider::new();
    let mirror: SharedProvider = Arc::new(Mutex::new(mirror_provider));

    let manager = MemoryManager::new(primary, Some(mirror)).await.unwrap();

    // Perform a read-path op on the manager. prefetch goes to primary only
    // per D-26/D-28 — mirror MUST NOT see it.
    let _ = manager.prefetch("test-session").await.unwrap();
    let _ = manager.format_for_system_prompt(MemoryTarget::Memory).await;
    let _ = manager.system_prompt_block().await;

    let state = mirror_state.lock().unwrap();
    assert_eq!(
        state.read_calls, 0,
        "mirror received read calls; reads must NOT fan out to mirror"
    );
    assert_eq!(
        state.writes.len(),
        0,
        "no writes were issued, mirror must be untouched"
    );
}
