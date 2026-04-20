//! MemoryManager: owns write-path dispatch + optional mirror fanout (D-25..D-29).
//!
//! The manager wraps a primary `MemoryProvider` plus an optional write-only
//! mirror provider. All writes flow:
//!   primary.handle_tool_call(name, args)  ->  on success  ->  mirror.on_memory_write(action, target, content)
//!
//! Invariants:
//! - Read paths (prefetch, format_for_system_prompt, system_prompt_block)
//!   go to PRIMARY ONLY (D-26, D-28 — mirror is write-only observational).
//! - Mirror failures are logged via `tracing::warn!` and NEVER propagated.
//! - Mirror calls are bounded by `tokio::time::timeout(Duration::from_secs(5), ...)`
//!   to protect the primary write path from slow mirrors (T-20-02b).
//! - The primary Mutex guard is DROPPED before the mirror call to avoid
//!   serializing primary reads behind a slow mirror (Pattern 3 caveat).
//!
//! Security: providers MUST NOT declare a tool named `session_search`
//! (owned by StateStore). `MemoryManager::new` enforces this (T-20-05).

use std::sync::Arc;
use std::time::Duration;

use ironhermes_core::config_schema::MemoryAction;
use ironhermes_core::memory_provider::{MemoryEntries, MemoryProvider};
use ironhermes_core::memory_store::{MemoryResult, MemoryTarget};
use ironhermes_core::types::{ChatMessage, ToolSchema};
use tokio::sync::Mutex;

const MIRROR_TIMEOUT: Duration = Duration::from_secs(5);
const RESERVED_TOOL_NAMES: &[&str] = &["session_search"];

pub type SharedProvider = Arc<Mutex<dyn MemoryProvider + Send>>;

pub struct MemoryManager {
    primary: SharedProvider,
    mirror: Option<SharedProvider>,
    /// GAP-4 / T-21.4-03: when false, writes to MemoryTarget::User are
    /// rejected at the manager level so all code paths (tool, direct calls)
    /// respect the config toggle. Default: true.
    user_profile_enabled: bool,
}

impl MemoryManager {
    pub async fn new(
        primary: SharedProvider,
        mirror: Option<SharedProvider>,
    ) -> anyhow::Result<Self> {
        // T-20-05: reject providers that shadow reserved tool names.
        {
            let p = primary.lock().await;
            Self::validate_schemas(p.get_tool_schemas())?;
        }
        if let Some(ref m) = mirror {
            let mg = m.lock().await;
            Self::validate_schemas(mg.get_tool_schemas())?;
        }
        Ok(Self { primary, mirror, user_profile_enabled: true })
    }

    /// Set whether the User profile target (USER.md) is enabled.
    /// When false, `handle_tool_call` rejects writes to `MemoryTarget::User`.
    /// Called by the factory after construction when `config.user_profile_enabled=false`.
    pub fn set_user_profile_enabled(&mut self, enabled: bool) {
        self.user_profile_enabled = enabled;
    }

    fn validate_schemas(schemas: Vec<ToolSchema>) -> anyhow::Result<()> {
        for s in schemas {
            if RESERVED_TOOL_NAMES.contains(&s.function.name.as_str()) {
                anyhow::bail!(
                    "MemoryProvider declared a reserved tool name `{}`; providers must not shadow it",
                    s.function.name
                );
            }
        }
        Ok(())
    }

    pub fn primary_handle(&self) -> SharedProvider {
        Arc::clone(&self.primary)
    }
    pub fn mirror_handle(&self) -> Option<SharedProvider> {
        self.mirror.as_ref().map(Arc::clone)
    }

    /// Merge primary + mirror tool schemas for registry-level enumeration.
    /// Plan 20-02 does NOT register mirror-provided tools — mirror is observational.
    pub async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        let p = self.primary.lock().await;
        p.get_tool_schemas()
    }

    pub async fn handle_tool_call(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> MemoryResult {
        // GAP-4 / T-21.4-03: reject User-target writes when user_profile_enabled=false.
        if !self.user_profile_enabled {
            if let Some(target_str) = args.get("target").and_then(|v| v.as_str()) {
                if target_str == "user" {
                    return Err(
                        "User profile memory is disabled via configuration. \
                         Enable it with memory.user_profile_enabled=true in config.yaml."
                            .to_string(),
                    );
                }
            }
        }

        // 1. Run the write on the primary. Drop guard before mirror call.
        let (outcome, action_target_content) = {
            let mut p = self.primary.lock().await;
            let outcome = p.handle_tool_call(name, args.clone())?;
            let inferred = infer_action_target_content(name, &args);
            (outcome, inferred)
        };

        // 2. Fire the mirror if one is configured. Errors logged, not propagated.
        if let (Some(mirror), Some((action, target, content))) =
            (&self.mirror, action_target_content)
        {
            let mirror = Arc::clone(mirror);
            let fut = async move {
                let mut m = mirror.lock().await;
                m.on_memory_write(action, target, &content).await
            };
            match tokio::time::timeout(MIRROR_TIMEOUT, fut).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        action = ?action,
                        target = ?target,
                        error = %e,
                        "mirror on_memory_write failed; primary write succeeded"
                    );
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        action = ?action,
                        target = ?target,
                        timeout_secs = MIRROR_TIMEOUT.as_secs(),
                        "mirror on_memory_write timed out; primary write succeeded"
                    );
                }
            }
        }

        Ok(outcome)
    }

    pub async fn add(&self, target: MemoryTarget, content: &str) -> MemoryResult {
        let args = serde_json::json!({
            "target": target_as_str(target),
            "content": content
        });
        self.handle_tool_call("memory_add", args).await
    }

    pub async fn replace(
        &self,
        target: MemoryTarget,
        old_text: &str,
        new_content: &str,
    ) -> MemoryResult {
        let args = serde_json::json!({
            "target": target_as_str(target),
            "old_text": old_text,
            "new_content": new_content
        });
        self.handle_tool_call("memory_replace", args).await
    }

    pub async fn remove(&self, target: MemoryTarget, old_text: &str) -> MemoryResult {
        let args = serde_json::json!({
            "target": target_as_str(target),
            "old_text": old_text
        });
        self.handle_tool_call("memory_remove", args).await
    }

    // ---- Read paths (primary only per D-26, D-28) ----
    pub async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries> {
        let p = self.primary.lock().await;
        p.prefetch(session_id).await
    }

    pub async fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String> {
        let p = self.primary.lock().await;
        p.format_for_system_prompt(target)
    }

    pub async fn system_prompt_block(&self) -> Option<String> {
        let p = self.primary.lock().await;
        p.system_prompt_block()
    }

    pub async fn to_memory_entries(&self) -> MemoryEntries {
        let p = self.primary.lock().await;
        p.to_memory_entries()
    }

    // ---- Post-turn + pre-compress hooks (fire on primary only) ----
    pub async fn queue_prefetch(&self, query: &str) -> anyhow::Result<()> {
        let p = self.primary.lock().await;
        p.queue_prefetch(query).await
    }

    pub async fn on_pre_compress(&self, messages: &[ChatMessage]) -> anyhow::Result<()> {
        let p = self.primary.lock().await;
        p.on_pre_compress(messages).await
    }

    pub async fn on_session_end(
        &self,
        session_id: &str,
        entries: &MemoryEntries,
    ) -> anyhow::Result<()> {
        let p = self.primary.lock().await;
        p.on_session_end(session_id, entries).await
    }

    pub async fn sync_turn(
        &self,
        session_id: &str,
        entries: &MemoryEntries,
    ) -> anyhow::Result<()> {
        let p = self.primary.lock().await;
        p.sync_turn(session_id, entries).await
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        // Shutdown the primary; mirror shutdown is best-effort.
        let primary_result = {
            let mut p = self.primary.lock().await;
            p.shutdown().await
        };
        if let Some(m) = &self.mirror {
            let mut mg = m.lock().await;
            if let Err(e) = mg.shutdown().await {
                tracing::warn!(error = %e, "mirror shutdown failed");
            }
        }
        primary_result
    }
}

fn target_as_str(t: MemoryTarget) -> &'static str {
    match t {
        MemoryTarget::Memory => "memory",
        MemoryTarget::User => "user",
    }
}

// ----------------------------------------------------------------------------
// Implement the `ironhermes_tools::MemoryManagerHandle` trait so `MemoryTool`
// can delegate writes through a manager without `ironhermes-tools` having to
// reverse-depend on `ironhermes-agent`. The handle forwards straight to the
// inherent `MemoryManager::handle_tool_call` method above.
// ----------------------------------------------------------------------------
#[async_trait::async_trait]
impl ironhermes_tools::MemoryManagerHandle for MemoryManager {
    async fn handle_tool_call(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> MemoryResult {
        MemoryManager::handle_tool_call(self, name, args).await
    }
}

/// Infer `(action, target, content)` from the tool call so mirror observers
/// receive a typed event regardless of the wire name used. Returns None
/// when the tool name is not an add/replace/remove — mirror is not called.
fn infer_action_target_content(
    name: &str,
    args: &serde_json::Value,
) -> Option<(MemoryAction, MemoryTarget, String)> {
    let action = match name {
        "memory_add" | "add" => MemoryAction::Add,
        "memory_replace" | "replace" => MemoryAction::Replace,
        "memory_remove" | "remove" => MemoryAction::Remove,
        _ => return None,
    };
    let target = match args.get("target")?.as_str()? {
        "memory" => MemoryTarget::Memory,
        "user" => MemoryTarget::User,
        _ => return None,
    };
    let content = match action {
        MemoryAction::Add => args.get("content")?.as_str()?.to_string(),
        MemoryAction::Replace => args.get("new_content")?.as_str()?.to_string(),
        MemoryAction::Remove => args.get("old_text")?.as_str()?.to_string(),
    };
    Some((action, target, content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::memory_store::MemoryStore;
    use std::path::Path;
    use std::sync::Mutex as StdMutex;

    // =========================================================================
    // MockRecorderProvider — shared-state recorder for mirror observation tests.
    //
    // The inner state is an `Arc<StdMutex<RecorderInner>>` so the test can hold
    // one handle for introspection while the manager holds the provider as
    // `Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>`.
    // =========================================================================
    #[derive(Default)]
    struct RecorderInner {
        writes: Vec<(MemoryAction, MemoryTarget, String)>,
        read_calls: Vec<&'static str>,
    }

    struct MockRecorderProvider {
        inner: Arc<StdMutex<RecorderInner>>,
    }

    impl MockRecorderProvider {
        fn new() -> (Self, Arc<StdMutex<RecorderInner>>) {
            let inner = Arc::new(StdMutex::new(RecorderInner::default()));
            (
                Self {
                    inner: Arc::clone(&inner),
                },
                inner,
            )
        }
    }

    #[async_trait]
    impl MemoryProvider for MockRecorderProvider {
        fn name(&self) -> &'static str {
            "mock-recorder"
        }
        async fn initialize(
            &mut self,
            _s: &str,
            _h: &Path,
            _v: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn prefetch(&self, _sid: &str) -> anyhow::Result<MemoryEntries> {
            self.inner.lock().unwrap().read_calls.push("prefetch");
            Ok(MemoryEntries::default())
        }
        async fn sync_turn(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
            Ok(())
        }
        async fn on_session_end(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
            Ok(())
        }
        async fn shutdown(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn load_from_disk(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn add(&mut self, _t: MemoryTarget, _c: &str) -> MemoryResult {
            // Mirror recorder is reached via on_memory_write, never add.
            // Return Ok so accidental direct calls don't crash the test.
            Ok("{}".to_string())
        }
        fn replace(&mut self, _t: MemoryTarget, _o: &str, _n: &str) -> MemoryResult {
            Ok("{}".to_string())
        }
        fn remove(&mut self, _t: MemoryTarget, _o: &str) -> MemoryResult {
            Ok("{}".to_string())
        }
        fn format_for_system_prompt(&self, _t: MemoryTarget) -> Option<String> {
            self.inner
                .lock()
                .unwrap()
                .read_calls
                .push("format_for_system_prompt");
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
            Ok(())
        }
    }

    // =========================================================================
    // FailingMirror — every on_memory_write returns Err.
    // =========================================================================
    struct FailingMirror;
    #[async_trait]
    impl MemoryProvider for FailingMirror {
        fn name(&self) -> &'static str {
            "failing-mirror"
        }
        async fn initialize(
            &mut self,
            _s: &str,
            _h: &Path,
            _v: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn prefetch(&self, _sid: &str) -> anyhow::Result<MemoryEntries> {
            Ok(MemoryEntries::default())
        }
        async fn sync_turn(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
            Ok(())
        }
        async fn on_session_end(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
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
            None
        }
        fn to_memory_entries(&self) -> MemoryEntries {
            MemoryEntries::default()
        }
        async fn on_memory_write(
            &mut self,
            _a: MemoryAction,
            _t: MemoryTarget,
            _c: &str,
        ) -> anyhow::Result<()> {
            anyhow::bail!("mirror deliberately failing")
        }
    }

    // =========================================================================
    // ReservedNameProvider — declares a tool named `session_search`.
    // =========================================================================
    struct ReservedNameProvider;
    #[async_trait]
    impl MemoryProvider for ReservedNameProvider {
        fn name(&self) -> &'static str {
            "reserved-name"
        }
        fn get_tool_schemas(&self) -> Vec<ToolSchema> {
            vec![ToolSchema::new(
                "session_search",
                "pretends to be the reserved tool",
                serde_json::json!({ "type": "object", "properties": {} }),
            )]
        }
        async fn initialize(
            &mut self,
            _s: &str,
            _h: &Path,
            _v: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn prefetch(&self, _sid: &str) -> anyhow::Result<MemoryEntries> {
            Ok(MemoryEntries::default())
        }
        async fn sync_turn(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
            Ok(())
        }
        async fn on_session_end(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
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
            None
        }
        fn to_memory_entries(&self) -> MemoryEntries {
            MemoryEntries::default()
        }
    }

    /// Construct a temp-dir-backed MemoryStore as the primary. Returns the
    /// `TempDir` alongside so the caller keeps it alive for the test duration
    /// without leaking (the directory is cleaned up when the guard drops).
    fn primary_file() -> (SharedProvider, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memories");
        let mut store = MemoryStore::new(mem_dir);
        store.load_from_disk().ok();
        (Arc::new(Mutex::new(store)), tmp)
    }

    // =========================================================================
    // Tests
    // =========================================================================

    #[tokio::test]
    async fn construction() {
        let (primary, _tmp) = primary_file();
        assert!(MemoryManager::new(Arc::clone(&primary), None)
            .await
            .is_ok());

        let (mirror_provider, _inner) = MockRecorderProvider::new();
        let mirror: SharedProvider = Arc::new(Mutex::new(mirror_provider));
        assert!(MemoryManager::new(primary, Some(mirror)).await.is_ok());
    }

    #[tokio::test]
    async fn reserved_tool_name_is_rejected() {
        // A provider whose get_tool_schemas returns a schema named "session_search"
        // must cause MemoryManager::new to fail with a message naming the reserved tool.
        let primary_reserved: SharedProvider =
            Arc::new(Mutex::new(ReservedNameProvider));
        let result = MemoryManager::new(primary_reserved, None).await;
        assert!(result.is_err(), "reserved-name primary must be rejected");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("session_search"),
            "error must name the reserved tool: {msg}"
        );
        assert!(
            msg.contains("reserved"),
            "error must explain the rejection: {msg}"
        );

        // Also test: reserved-name MIRROR is rejected.
        let (primary_ok, _tmp) = primary_file();
        let mirror_reserved: SharedProvider =
            Arc::new(Mutex::new(ReservedNameProvider));
        let result2 = MemoryManager::new(primary_ok, Some(mirror_reserved)).await;
        assert!(result2.is_err(), "reserved-name mirror must be rejected");
    }

    #[tokio::test]
    async fn mirror_observes_writes() {
        let (primary, _tmp) = primary_file();
        let (mirror_provider, recorder_inner) = MockRecorderProvider::new();
        let mirror: SharedProvider = Arc::new(Mutex::new(mirror_provider));

        let mgr = MemoryManager::new(primary, Some(mirror)).await.unwrap();
        mgr.add(MemoryTarget::Memory, "fact-A").await.unwrap();
        mgr.replace(MemoryTarget::Memory, "fact-A", "fact-B")
            .await
            .unwrap();
        mgr.remove(MemoryTarget::Memory, "fact-B").await.unwrap();

        let writes = recorder_inner.lock().unwrap().writes.clone();
        assert_eq!(writes.len(), 3, "expected 3 mirror writes, got: {writes:?}");
        assert_eq!(writes[0].0, MemoryAction::Add);
        assert_eq!(writes[0].1, MemoryTarget::Memory);
        assert_eq!(writes[0].2, "fact-A");
        assert_eq!(writes[1].0, MemoryAction::Replace);
        assert_eq!(writes[1].2, "fact-B");
        assert_eq!(writes[2].0, MemoryAction::Remove);
        assert_eq!(writes[2].2, "fact-B");
    }

    #[tokio::test]
    async fn mirror_failure_does_not_block_primary() {
        let (primary, _tmp) = primary_file();
        let mirror: SharedProvider = Arc::new(Mutex::new(FailingMirror));
        let mgr = MemoryManager::new(primary, Some(mirror)).await.unwrap();
        let r = mgr.add(MemoryTarget::Memory, "still-writes").await;
        assert!(
            r.is_ok(),
            "primary write must succeed despite mirror failure; got {:?}",
            r.err()
        );

        // Use to_memory_entries() to inspect LIVE entries (not the frozen
        // prompt-snapshot, which is only refreshed by load_from_disk() per D-12).
        let entries = mgr.to_memory_entries().await;
        let mem_entries = entries
            .entries
            .get(&MemoryTarget::Memory)
            .expect("Memory target should have entries");
        assert!(
            mem_entries.iter().any(|e| e.contains("still-writes")),
            "primary must have the new entry; entries were: {mem_entries:?}"
        );
    }

    #[tokio::test]
    async fn read_paths_hit_primary_only() {
        let (primary, _tmp) = primary_file();
        let (mirror_provider, recorder_inner) = MockRecorderProvider::new();
        let mirror: SharedProvider = Arc::new(Mutex::new(mirror_provider));
        let mgr = MemoryManager::new(primary, Some(mirror)).await.unwrap();

        // No writes — only reads.
        let _ = mgr.prefetch("sid").await.unwrap();
        let _ = mgr.system_prompt_block().await;
        let _ = mgr.format_for_system_prompt(MemoryTarget::Memory).await;

        let reads = recorder_inner.lock().unwrap().read_calls.clone();
        assert!(
            reads.is_empty(),
            "mirror must receive ZERO reads; got: {reads:?}"
        );
    }

    // =========================================================================
    // GAP-4: user_profile_enabled toggle tests (T-21.4-03)
    // =========================================================================

    #[tokio::test]
    async fn user_target_rejected_when_user_profile_disabled() {
        let (primary, _tmp) = primary_file();
        let mut mgr = MemoryManager::new(primary, None).await.unwrap();
        mgr.set_user_profile_enabled(false);

        let args = serde_json::json!({ "target": "user", "content": "some fact" });
        let result = mgr.handle_tool_call("memory_add", args).await;
        assert!(
            result.is_err(),
            "User-target write must fail when user_profile_enabled=false"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("user_profile_enabled"),
            "error must mention the config key, got: {msg}"
        );
    }

    #[tokio::test]
    async fn memory_target_allowed_when_user_profile_disabled() {
        let (primary, _tmp) = primary_file();
        let mut mgr = MemoryManager::new(primary, None).await.unwrap();
        mgr.set_user_profile_enabled(false);

        // MEMORY target must still work when only user_profile is disabled.
        let args = serde_json::json!({ "target": "memory", "content": "general fact" });
        let result = mgr.handle_tool_call("memory_add", args).await;
        assert!(
            result.is_ok(),
            "Memory-target write must succeed when user_profile_enabled=false, got: {:?}",
            result.err()
        );
    }
}
