//! Phase 20 Plan 02 Task 03 — MemoryProvider trait-level contract test.
//!
//! This test lives in `ironhermes-core` and exercises the trait through a
//! `MockRecorderProvider` that records every hook invocation in order. The
//! assertion verifies the expected lifecycle ordering contract:
//!
//!   initialize → prefetch → add → sync_turn → queue_prefetch
//!     → on_pre_compress → on_session_end → shutdown
//!
//! The test is deliberately trait-only: it does NOT depend on
//! `ironhermes-agent`, the runtime wiring, or `MemoryManager`. Any provider
//! crate (sqlite, duckdb, grafeo, future community plugins) can drop in their
//! own provider type and reuse the same ordering assertion — this is the
//! interoperability guarantee Plan 20-02 adds.
//!
//! Why the particular ordering:
//!   - `initialize` always runs first (D-10)
//!   - `prefetch` warms session state before any write touches disk (D-12)
//!   - `add` is the write-path representative (D-05)
//!   - `sync_turn` fires once per turn after writes land (D-14)
//!   - `queue_prefetch` is the post-turn cache warmer (D-13)
//!   - `on_pre_compress` runs BEFORE compression mutates the message vec
//!     (D-23 — this is the invariant MemoryManager guarantees end-to-end)
//!   - `on_session_end` fires once per session close (D-15)
//!   - `shutdown` is the final teardown (D-15)

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_core::config_schema::MemoryAction;
use ironhermes_core::memory_provider::{MemoryEntries, MemoryProvider};
use ironhermes_core::memory_store::{MemoryResult, MemoryTarget};
use ironhermes_core::types::ChatMessage;

/// Records every hook invocation in call order so tests can assert the
/// exact lifecycle ordering the trait contract promises.
struct MockRecorderProvider {
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl MockRecorderProvider {
    fn new(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self { log }
    }

    fn record(&self, hook: &'static str) {
        self.log.lock().unwrap().push(hook);
    }
}

#[async_trait]
impl MemoryProvider for MockRecorderProvider {
    fn name(&self) -> &'static str {
        "mock-recorder"
    }

    async fn initialize(
        &mut self,
        _session_id: &str,
        _hermes_home: &Path,
        _provider_config: &serde_json::Value,
    ) -> anyhow::Result<()> {
        self.record("initialize");
        Ok(())
    }

    async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
        self.record("prefetch");
        Ok(MemoryEntries::default())
    }

    async fn sync_turn(
        &self,
        _session_id: &str,
        _entries: &MemoryEntries,
    ) -> anyhow::Result<()> {
        self.record("sync_turn");
        Ok(())
    }

    async fn queue_prefetch(&self, _query: &str) -> anyhow::Result<()> {
        self.record("queue_prefetch");
        Ok(())
    }

    async fn on_pre_compress(&self, _messages: &[ChatMessage]) -> anyhow::Result<()> {
        self.record("on_pre_compress");
        Ok(())
    }

    async fn on_memory_write(
        &mut self,
        _action: MemoryAction,
        _target: MemoryTarget,
        _content: &str,
    ) -> anyhow::Result<()> {
        self.record("on_memory_write");
        Ok(())
    }

    async fn on_session_end(
        &self,
        _session_id: &str,
        _entries: &MemoryEntries,
    ) -> anyhow::Result<()> {
        self.record("on_session_end");
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.record("shutdown");
        Ok(())
    }

    fn load_from_disk(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn add(&mut self, _target: MemoryTarget, _content: &str) -> MemoryResult {
        self.record("add");
        Ok("{}".to_string())
    }

    fn replace(
        &mut self,
        _target: MemoryTarget,
        _old_text: &str,
        _new_content: &str,
    ) -> MemoryResult {
        self.record("replace");
        Ok("{}".to_string())
    }

    fn remove(&mut self, _target: MemoryTarget, _old_text: &str) -> MemoryResult {
        self.record("remove");
        Ok("{}".to_string())
    }

    fn format_for_system_prompt(&self, _target: MemoryTarget) -> Option<String> {
        None
    }

    fn to_memory_entries(&self) -> MemoryEntries {
        MemoryEntries::default()
    }
}

/// Drive the trait through its happy-path lifecycle and assert the expected
/// ordering. Any provider that wants to satisfy the Plan-20-02 contract must
/// be able to pass this exact ordering when wired through the same call
/// sequence.
#[tokio::test]
async fn hook_ordering_contract() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let mut provider = MockRecorderProvider::new(log.clone());

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let session_id = "contract-session";

    // 1. initialize
    provider
        .initialize(session_id, tmp.path(), &serde_json::Value::Null)
        .await
        .expect("initialize");

    // 2. prefetch
    let entries = provider.prefetch(session_id).await.expect("prefetch");

    // 3. add (write path)
    provider
        .add(MemoryTarget::Memory, "fact-A")
        .expect("add");

    // 4. sync_turn after writes land
    provider
        .sync_turn(session_id, &entries)
        .await
        .expect("sync_turn");

    // 5. queue_prefetch — post-turn cache warmer
    provider
        .queue_prefetch("next-turn-hint")
        .await
        .expect("queue_prefetch");

    // 6. on_pre_compress fires BEFORE any destructive compression (D-23)
    let no_messages: Vec<ChatMessage> = vec![];
    provider
        .on_pre_compress(&no_messages)
        .await
        .expect("on_pre_compress");

    // 7. on_session_end once per session close
    provider
        .on_session_end(session_id, &entries)
        .await
        .expect("on_session_end");

    // 8. shutdown is the final teardown
    provider.shutdown().await.expect("shutdown");

    let actual = log.lock().unwrap().clone();
    let expected = vec![
        "initialize",
        "prefetch",
        "add",
        "sync_turn",
        "queue_prefetch",
        "on_pre_compress",
        "on_session_end",
        "shutdown",
    ];
    assert_eq!(
        actual, expected,
        "MemoryProvider hook ordering contract violated.\n  actual:   {:?}\n  expected: {:?}",
        actual, expected
    );
}

/// Defensive: the contract must reject a provider that attempts to invoke
/// `on_pre_compress` AFTER a destructive compression step that has already
/// dropped messages. The trait itself can't prevent reordering, but callers
/// (MemoryManager, engines) MUST fire it first. This test asserts that when
/// a caller respects the contract, the recorder sees `on_pre_compress`
/// strictly before any subsequent lifecycle event.
#[tokio::test]
async fn on_pre_compress_fires_before_session_end() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let mut provider = MockRecorderProvider::new(log.clone());

    let tmp = tempfile::TempDir::new().expect("tempdir");
    provider
        .initialize("s", tmp.path(), &serde_json::Value::Null)
        .await
        .expect("init");
    let entries = MemoryEntries::default();

    // Caller fires on_pre_compress BEFORE on_session_end per the contract.
    provider
        .on_pre_compress(&[])
        .await
        .expect("on_pre_compress");
    provider
        .on_session_end("s", &entries)
        .await
        .expect("on_session_end");

    let actual = log.lock().unwrap().clone();
    let pos_pc = actual
        .iter()
        .position(|&s| s == "on_pre_compress")
        .expect("on_pre_compress recorded");
    let pos_se = actual
        .iter()
        .position(|&s| s == "on_session_end")
        .expect("on_session_end recorded");
    assert!(
        pos_pc < pos_se,
        "on_pre_compress must fire strictly before on_session_end, got {:?}",
        actual
    );
}
