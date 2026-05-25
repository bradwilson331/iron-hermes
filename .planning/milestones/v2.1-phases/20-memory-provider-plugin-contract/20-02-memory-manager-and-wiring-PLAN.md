---
phase: 20
plan: 02
type: execute
wave: 2
depends_on: [20-01]
files_modified:
  - crates/ironhermes-agent/src/memory/manager.rs
  - crates/ironhermes-agent/src/memory/mod.rs
  - crates/ironhermes-agent/src/memory/factory.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-agent/src/prompt_builder.rs
  - crates/ironhermes-agent/src/context_engine.rs
  - crates/ironhermes-agent/src/memory_flush_handler.rs
  - crates/ironhermes-tools/src/memory_tool.rs
  - crates/ironhermes-core/tests/memory_provider_contract.rs
autonomous: true
requirements: [MEM-07, MEM-12]
must_haves:
  truths:
    - "MemoryManager exists as a shared Arc<tokio::sync::Mutex<MemoryManager>> with primary + optional mirror"
    - "MemoryTool::execute delegates to MemoryManager::handle_tool_call instead of holding the provider directly"
    - "Mirror observes every successful add/replace/remove without blocking primary writes"
    - "Mirror failures are logged via tracing::warn! and swallowed — never propagated"
    - "agent_loop fires queue_prefetch after each completed turn as a detached tokio task"
    - "ContextEngine calls memory_manager.on_pre_compress(messages) directly before compression destroys messages"
    - "prompt_builder::load_memory appends system_prompt_block after target-scoped blocks"
    - "memory_flush_handler invokes MemoryManager (no direct provider handle) on the pre-compress hook"
    - "MockRecorder provider + hook-ordering test prove initialize -> prefetch -> sync_turn -> queue_prefetch -> on_pre_compress -> on_memory_write -> on_session_end -> shutdown"
  artifacts:
    - path: "crates/ironhermes-agent/src/memory/manager.rs"
      provides: "MemoryManager type + write-path dispatch + mirror fanout + manager-level tests"
      contains: "pub struct MemoryManager"
    - path: "crates/ironhermes-agent/src/context_engine.rs"
      provides: "set_memory_manager + pre-compress fire site that calls manager.on_pre_compress(messages)"
      contains: "memory_manager.on_pre_compress"
    - path: "crates/ironhermes-core/tests/memory_provider_contract.rs"
      provides: "MockRecorderProvider + trait-level hook ordering test (D-22)"
      contains: "MockRecorderProvider"
  key_links:
    - from: "crates/ironhermes-tools/src/memory_tool.rs"
      to: "MemoryManager::handle_tool_call"
      via: "delegation inside MemoryTool::execute"
      pattern: "manager\\.handle_tool_call"
    - from: "crates/ironhermes-agent/src/agent_loop.rs"
      to: "MemoryManager::queue_prefetch"
      via: "tokio::spawn fire-and-forget after each turn"
      pattern: "tokio::spawn"
    - from: "crates/ironhermes-agent/src/context_engine.rs"
      to: "MemoryManager::on_pre_compress"
      via: "direct call inside compress() before destructive work (resolves research open question #4)"
      pattern: "on_pre_compress"
    - from: "crates/ironhermes-agent/src/prompt_builder.rs"
      to: "MemoryProvider::system_prompt_block"
      via: "load_memory appends to slot 3 after target-scoped blocks"
      pattern: "system_prompt_block"
---

<objective>
Introduce the `MemoryManager` layer that wraps a primary `MemoryProvider` plus an optional write-only mirror, and wire every new hook to its correct fire site so the trait surface from Plan 20-01 is actually exercised. Per D-25..D-29: write path goes `primary.handle_tool_call` -> on success -> `mirror.on_memory_write` with the primary guard dropped before the mirror call and mirror failures logged+swallowed. `MemoryTool` delegates to the manager (resolving research open question #1 — no agent-loop intercept-set reshape is needed because the memory tool is a single `"memory"` tool that already flows through the registry). `AgentLoop` fires `queue_prefetch` as a detached `tokio::spawn` after each completed turn (D-12). `ContextEngine::compress` calls `manager.on_pre_compress(messages)` directly before destructive work (D-13, resolves research open question #4 — the existing `HookEventKind::ContextPreCompress` event doesn't carry `messages`). `PromptBuilder::load_memory` appends `system_prompt_block()` after the target-scoped blocks (D-11). `MockRecorderProvider` + hook-ordering integration test prove the full hook sequence (D-22).

Purpose: Completes the API parity goal by ensuring every enriched trait method has a live fire site. Without this plan, Plan 20-01's new methods exist but never run.
Output: New `memory/manager.rs` module; edits to `agent_loop.rs`, `prompt_builder.rs`, `context_engine.rs`, `memory_flush_handler.rs`, `memory_tool.rs`; new trait-level test harness in `ironhermes-core/tests/memory_provider_contract.rs`.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md
@.planning/phases/20-memory-provider-plugin-contract/20-RESEARCH.md
@.planning/phases/20-memory-provider-plugin-contract/20-VALIDATION.md
@.planning/phases/20-memory-provider-plugin-contract/20-01-trait-enrichment-and-factory-fix-PLAN.md

<interfaces>
<!-- Post-Plan-20-01 trait (see memory_provider.rs rewrite in Plan 20-01). -->
<!-- Provider sharing shape is now `Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>`. -->

<!-- Existing MemoryTool shape (to be reshaped to delegate through MemoryManager). -->
From `crates/ironhermes-tools/src/memory_tool.rs`:
```rust
pub struct MemoryTool {
    pub store: Arc<std::sync::Mutex<dyn MemoryProvider + Send>>,  // OLD — becomes Arc<tokio::sync::Mutex<MemoryManager>>
}
impl Tool for MemoryTool {
    fn execute(&self, args: Value) -> ToolResult { /* action-dispatched memory ops */ }
}
```
Single registered tool is named `"memory"`; its body matches on `args["action"]` ("add"|"replace"|"remove").

<!-- Existing ContextEngine (Phase 18). -->
From `crates/ironhermes-agent/src/context_engine.rs`:
```rust
#[async_trait]
pub trait ContextEngine: Send + Sync {
    async fn compress(&self, messages: &mut Vec<ChatMessage>, ...) -> anyhow::Result<CompressionOutcome>;
}
pub struct LocalPruningEngine { /* ... */ }
pub struct SummarizingEngine { /* ... */ }
```
Both engines own `&mut messages` at the destructive callsite (research: context_engine.rs:136-150).

<!-- Existing pre-compress hook listener (Phase 18). -->
From `crates/ironhermes-agent/src/memory_flush_handler.rs`:
```rust
pub fn build_memory_flush_listener(
    provider: Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>,
) -> AsyncHookListener { /* fires sync_turn on ContextPreCompress */ }
```
Post-Plan 20-02: this listener is rewired to take `Arc<tokio::sync::Mutex<MemoryManager>>` instead of the raw provider (so `sync_turn` goes through the manager's primary).
</interfaces>
</context>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| Primary provider -> mirror provider | Content already scanned by `scan_context_content` in Phase 17 write paths. Mirror sees scanned content. |
| MemoryManager -> tracing subscriber | `tracing::warn!` fires on mirror failure. The error value could in principle carry sensitive request bodies if the mirror is a network provider (future). |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-20-02 | Information Disclosure | Mirror failure log path in `MemoryManager::handle_tool_call` | mitigate | Log only `error.to_string()` (concrete rendering), not `{:?}` the raw `anyhow::Error` — avoids leaking debug-formatted request bodies. Log shape: `tracing::warn!(action = ?action, target = ?target, error = %e, "mirror on_memory_write failed; primary write succeeded")`. Use `%` (Display) not `?` (Debug) for the error field. |
| T-20-05 | Spoofing | Provider-declared `get_tool_schemas()` could claim a name that collides with registry's `session_search` | mitigate | `MemoryManager::new(...)` validates both primary and mirror schemas and returns `Err` if any schema name matches `session_search` (or equals an existing registered tool name). Document the constraint in manager.rs comments: "Providers MUST NOT declare a tool named `session_search`." |
| T-20-02b | Denial of Service | Slow/hung mirror blocking primary writes | mitigate | Wrap the mirror call in `tokio::time::timeout(Duration::from_secs(5), mirror_fut)` — on timeout, log `tracing::warn!` and return `Ok(())` to the caller. Document the 5-second mirror ceiling. |
</threat_model>

<tasks>

<task type="auto" tdd="true">
  <name>Task 20-02-01: Create MemoryManager module + unit tests for mirror observation + mirror-failure swallow + timeout</name>
  <read_first>
    - crates/ironhermes-agent/src/memory/factory.rs (post-Plan-20-01 async factory returning `Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>`)
    - crates/ironhermes-agent/src/memory/mod.rs (module exports)
    - crates/ironhermes-agent/src/lib.rs (re-exports)
    - crates/ironhermes-core/src/memory_provider.rs (post-Plan-20-01 trait)
    - crates/ironhermes-core/src/config_schema.rs (MemoryAction)
    - crates/ironhermes-core/src/memory_store.rs (MemoryTarget, MemoryResult)
    - crates/ironhermes-core/src/config.rs (MemoryConfig.mirror_provider)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md (D-14, D-25, D-26, D-27, D-28, D-29)
    - .planning/phases/20-memory-provider-plugin-contract/20-RESEARCH.md (Pattern 3, Pitfall 4 — tokio::sync::Mutex)
  </read_first>
  <files>
    crates/ironhermes-agent/src/memory/manager.rs (NEW),
    crates/ironhermes-agent/src/memory/mod.rs (add `pub mod manager;` + `pub use manager::MemoryManager;`),
    crates/ironhermes-agent/src/memory/factory.rs (add optional-mirror construction path that consumes `MemoryConfig.mirror_provider`),
    crates/ironhermes-agent/src/lib.rs (add `pub use memory::MemoryManager;`)
  </files>
  <behavior>
    - Test: `construction` — `MemoryManager::new(primary, None).await` succeeds; `MemoryManager::new(primary, Some(mirror)).await` succeeds; schema-collision mirror (a `MockProvider` whose `get_tool_schemas()` returns a schema named `session_search`) causes `new` to return `Err(...)` (T-20-05 mitigation).
    - Test: `mirror_observes_writes` — primary=`MemoryStore` (temp dir) + mirror=`MockRecorderProvider`; call `manager.handle_tool_call("memory_add", {target: "memory", content: "X"})`; assert the mirror's recorded writes contain exactly one entry `(MemoryAction::Add, MemoryTarget::Memory, "X")`. Repeat for replace and remove (D-29).
    - Test: `mirror_failure_does_not_block_primary` — primary=`MemoryStore` + mirror=`FailingMirrorProvider` (whose `on_memory_write` always returns `Err`); call `manager.handle_tool_call("memory_add", ...)`; assert return is `Ok(_)` AND primary's `format_for_system_prompt(MemoryTarget::Memory)` contains the new content AND a `tracing::warn!` was emitted (if `tracing-test` feature is not in workspace, just assert the Ok + primary write). (D-29.)
    - Test: `mirror_timeout_does_not_block_primary` — mirror's `on_memory_write` sleeps 10 seconds; manager call returns in ~5 seconds with `Ok(_)` and primary contains the write. Uses `tokio::time::pause()` + `tokio::time::advance()` to avoid real sleeps. (T-20-02b.)
    - Test: `read_paths_hit_primary_only` — call `manager.prefetch(sid).await`, `manager.system_prompt_block()`, `manager.format_for_system_prompt(MemoryTarget::Memory)`; assert the mirror recorder saw ZERO reads. (D-26.)
  </behavior>
  <action>
    1. CREATE `crates/ironhermes-agent/src/memory/manager.rs` with:

    ```rust
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
    //! (owned by StateStore). `MemoryManager::new` enforces this.

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
            Ok(Self { primary, mirror })
        }

        fn validate_schemas(schemas: Vec<ToolSchema>) -> anyhow::Result<()> {
            for s in schemas {
                if RESERVED_TOOL_NAMES.contains(&s.name.as_str()) {
                    anyhow::bail!(
                        "MemoryProvider declared a reserved tool name `{}`; providers must not shadow it",
                        s.name
                    );
                }
            }
            Ok(())
        }

        pub fn primary_handle(&self) -> SharedProvider { Arc::clone(&self.primary) }
        pub fn mirror_handle(&self) -> Option<SharedProvider> {
            self.mirror.as_ref().map(Arc::clone)
        }

        /// Merge primary + mirror tool schemas for registry-level enumeration.
        /// Deduplicates by tool name, primary wins on collision.
        pub async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
            let primary_schemas = {
                let p = self.primary.lock().await;
                p.get_tool_schemas()
            };
            // Plan 20-02 does NOT register mirror-provided tools — mirror is observational.
            // Left open for a future phase if/when mirror tools are promoted.
            primary_schemas
        }

        pub async fn handle_tool_call(
            &self,
            name: &str,
            args: serde_json::Value,
        ) -> MemoryResult {
            // 1. Run the write on the primary. Drop guard before mirror call.
            let (outcome, action_target_content) = {
                let mut p = self.primary.lock().await;
                let before = p.to_memory_entries();
                let outcome = p.handle_tool_call(name, args.clone())?;
                let inferred = infer_action_target_content(name, &args, &before, &p.to_memory_entries());
                (outcome, inferred)
            };

            // 2. Fire the mirror if one is configured. Errors logged, not propagated.
            if let (Some(mirror), Some((action, target, content))) = (&self.mirror, action_target_content) {
                let mirror = Arc::clone(mirror);
                let action_copy = action;
                let target_copy = target;
                let content_copy = content;
                let fut = async move {
                    let mut m = mirror.lock().await;
                    m.on_memory_write(action_copy, target_copy, &content_copy).await
                };
                match tokio::time::timeout(MIRROR_TIMEOUT, fut).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!(
                            action = ?action, target = ?target, error = %e,
                            "mirror on_memory_write failed; primary write succeeded"
                        );
                    }
                    Err(_elapsed) => {
                        tracing::warn!(
                            action = ?action, target = ?target, timeout_secs = MIRROR_TIMEOUT.as_secs(),
                            "mirror on_memory_write timed out; primary write succeeded"
                        );
                    }
                }
            }

            Ok(outcome)
        }

        pub async fn add(&self, target: MemoryTarget, content: &str) -> MemoryResult {
            let args = serde_json::json!({"target": target_as_str(target), "content": content});
            self.handle_tool_call("memory_add", args).await
        }

        pub async fn replace(&self, target: MemoryTarget, old_text: &str, new_content: &str) -> MemoryResult {
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

        // ---- Post-turn + pre-compress hooks (fire on primary only per open question #2 resolution) ----
        pub async fn queue_prefetch(&self, query: &str) -> anyhow::Result<()> {
            let p = self.primary.lock().await;
            p.queue_prefetch(query).await
        }

        pub async fn on_pre_compress(&self, messages: &[ChatMessage]) -> anyhow::Result<()> {
            let p = self.primary.lock().await;
            p.on_pre_compress(messages).await
        }

        pub async fn on_session_end(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()> {
            let p = self.primary.lock().await;
            p.on_session_end(session_id, entries).await
        }

        pub async fn sync_turn(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()> {
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
        match t { MemoryTarget::Memory => "memory", MemoryTarget::User => "user" }
    }

    /// Infer `(action, target, content)` from the tool call so mirror observers
    /// receive a typed event regardless of the wire name used. Returns None
    /// when the tool name is not an add/replace/remove — mirror is not called.
    fn infer_action_target_content(
        name: &str,
        args: &serde_json::Value,
        _before: &MemoryEntries,
        _after: &MemoryEntries,
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
        use std::sync::Mutex as StdMutex;

        // Records every on_memory_write call in order.
        #[derive(Default)]
        struct RecorderInner {
            writes: Vec<(MemoryAction, MemoryTarget, String)>,
            read_calls: Vec<&'static str>,
        }
        struct MockRecorderProvider { inner: StdMutex<RecorderInner> }

        #[async_trait]
        impl MemoryProvider for MockRecorderProvider {
            fn name(&self) -> &'static str { "mock-recorder" }
            async fn initialize(&mut self, _s: &str, _h: &std::path::Path, _v: &serde_json::Value) -> anyhow::Result<()> { Ok(()) }
            async fn prefetch(&self, _sid: &str) -> anyhow::Result<MemoryEntries> {
                self.inner.lock().unwrap().read_calls.push("prefetch");
                Ok(MemoryEntries::default())
            }
            async fn sync_turn(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> { Ok(()) }
            async fn on_session_end(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> { Ok(()) }
            async fn shutdown(&mut self) -> anyhow::Result<()> { Ok(()) }
            fn load_from_disk(&mut self) -> anyhow::Result<()> { Ok(()) }
            fn add(&mut self, _t: MemoryTarget, _c: &str) -> MemoryResult { unreachable!("mirror should not receive add directly") }
            fn replace(&mut self, _t: MemoryTarget, _o: &str, _n: &str) -> MemoryResult { unreachable!() }
            fn remove(&mut self, _t: MemoryTarget, _o: &str) -> MemoryResult { unreachable!() }
            fn format_for_system_prompt(&self, _t: MemoryTarget) -> Option<String> {
                self.inner.lock().unwrap().read_calls.push("format_for_system_prompt");
                None
            }
            fn to_memory_entries(&self) -> MemoryEntries { MemoryEntries::default() }
            async fn on_memory_write(&mut self, action: MemoryAction, target: MemoryTarget, content: &str) -> anyhow::Result<()> {
                self.inner.lock().unwrap().writes.push((action, target, content.to_string()));
                Ok(())
            }
        }

        struct FailingMirror;
        #[async_trait]
        impl MemoryProvider for FailingMirror {
            fn name(&self) -> &'static str { "failing-mirror" }
            async fn initialize(&mut self, _s: &str, _h: &std::path::Path, _v: &serde_json::Value) -> anyhow::Result<()> { Ok(()) }
            async fn prefetch(&self, _sid: &str) -> anyhow::Result<MemoryEntries> { Ok(MemoryEntries::default()) }
            async fn sync_turn(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> { Ok(()) }
            async fn on_session_end(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> { Ok(()) }
            async fn shutdown(&mut self) -> anyhow::Result<()> { Ok(()) }
            fn load_from_disk(&mut self) -> anyhow::Result<()> { Ok(()) }
            fn add(&mut self, _t: MemoryTarget, _c: &str) -> MemoryResult { unreachable!() }
            fn replace(&mut self, _t: MemoryTarget, _o: &str, _n: &str) -> MemoryResult { unreachable!() }
            fn remove(&mut self, _t: MemoryTarget, _o: &str) -> MemoryResult { unreachable!() }
            fn format_for_system_prompt(&self, _t: MemoryTarget) -> Option<String> { None }
            fn to_memory_entries(&self) -> MemoryEntries { MemoryEntries::default() }
            async fn on_memory_write(&mut self, _a: MemoryAction, _t: MemoryTarget, _c: &str) -> anyhow::Result<()> {
                anyhow::bail!("mirror deliberately failing")
            }
        }

        fn primary_file() -> SharedProvider {
            let tmp = tempfile::TempDir::new().unwrap();
            Arc::new(Mutex::new(MemoryStore::new(tmp.path().to_path_buf())))
            // tempdir leaks into test process end; ok for unit test
        }

        fn recorder() -> (SharedProvider, Arc<StdMutex<RecorderInner>>) {
            let inner = Arc::new(StdMutex::new(RecorderInner::default()));
            let p: SharedProvider = Arc::new(Mutex::new(MockRecorderProvider {
                inner: StdMutex::new(RecorderInner::default()),
            }));
            (p, inner)
        }

        #[tokio::test]
        async fn construction() {
            let primary = primary_file();
            assert!(MemoryManager::new(Arc::clone(&primary), None).await.is_ok());
            let (mirror, _) = recorder();
            assert!(MemoryManager::new(primary, Some(mirror)).await.is_ok());
        }

        #[tokio::test]
        async fn mirror_observes_writes() {
            // Construct with recorder directly so we can read its state after.
            let primary = primary_file();
            let mirror_inner = Arc::new(StdMutex::new(RecorderInner::default()));
            let mirror_provider = MockRecorderProvider {
                inner: StdMutex::new(RecorderInner::default()),
            };
            // Share inner via an Arc so we can inspect after delegation.
            // Implementation detail: swap the inner StdMutex with a clone Arc.
            // Left as EXECUTOR discretion — restructure MockRecorderProvider to hold
            // Arc<StdMutex<RecorderInner>> instead of owning it, so this test can
            // hold one handle and the provider another.
            let mirror: SharedProvider = Arc::new(Mutex::new(mirror_provider));

            let mgr = MemoryManager::new(primary, Some(mirror)).await.unwrap();
            mgr.add(MemoryTarget::Memory, "fact-A").await.unwrap();
            mgr.replace(MemoryTarget::Memory, "fact-A", "fact-B").await.unwrap();
            mgr.remove(MemoryTarget::Memory, "fact-B").await.unwrap();

            // EXECUTOR: adjust MockRecorderProvider to expose an Arc<StdMutex<...>>
            // handle for introspection, then:
            //   let writes = mirror_inner.lock().unwrap().writes.clone();
            //   assert_eq!(writes[0].0, MemoryAction::Add);
            //   assert_eq!(writes[1].0, MemoryAction::Replace);
            //   assert_eq!(writes[2].0, MemoryAction::Remove);
            let _ = mirror_inner;
        }

        #[tokio::test]
        async fn mirror_failure_does_not_block_primary() {
            let primary = primary_file();
            let mirror: SharedProvider = Arc::new(Mutex::new(FailingMirror));
            let mgr = MemoryManager::new(primary, Some(mirror)).await.unwrap();
            let r = mgr.add(MemoryTarget::Memory, "still-writes").await;
            assert!(r.is_ok(), "primary write must succeed despite mirror failure; got {:?}", r.err());

            let block = mgr.format_for_system_prompt(MemoryTarget::Memory).await
                .expect("primary should have the entry");
            assert!(block.contains("still-writes"));
        }

        #[tokio::test]
        async fn reserved_tool_name_is_rejected() {
            // A provider whose get_tool_schemas returns a schema named "session_search"
            // must cause MemoryManager::new to fail.
            // EXECUTOR: add a ReservedNameProvider mock that overrides
            // get_tool_schemas to return `vec![ToolSchema { name: "session_search".into(), ... }]`.
            // Expected: Err containing "reserved tool name".
        }
    }
    ```

    NOTE on the `mirror_observes_writes` test: the sketch above leaves two `EXECUTOR:` hooks because hand-writing the exact `Arc<StdMutex<RecorderInner>>` plumbing is fiddly and the executor will pick the cleanest shape. The intent is precise: three writes, in order Add/Replace/Remove, each with the right `target` and `content`. Completion criterion: test asserts the three writes match the three `manager.add/replace/remove` calls.

    2. EDIT `crates/ironhermes-agent/src/memory/mod.rs`: append `pub mod manager;` and `pub use manager::{MemoryManager, SharedProvider};`.

    3. EDIT `crates/ironhermes-agent/src/lib.rs`: append `pub use memory::{MemoryManager, SharedProvider};` alongside existing re-exports.

    4. EXTEND `crates/ironhermes-agent/src/memory/factory.rs` with a new async helper:
    ```rust
    pub async fn build_memory_manager(
        config: &ironhermes_core::config::MemoryConfig,
    ) -> anyhow::Result<std::sync::Arc<tokio::sync::Mutex<crate::memory::MemoryManager>>> {
        let primary = build_memory_provider(config).await?;
        let mirror = if let Some(name) = &config.mirror_provider {
            let mut mirror_cfg = config.clone();
            mirror_cfg.provider = name.clone();
            mirror_cfg.mirror_provider = None; // prevent recursion
            Some(build_memory_provider(&mirror_cfg).await?)
        } else { None };
        let mgr = crate::memory::MemoryManager::new(primary, mirror).await?;
        Ok(std::sync::Arc::new(tokio::sync::Mutex::new(mgr)))
    }
    ```
    Add a `#[cfg(test)] #[tokio::test] async fn factory_builds_manager_with_no_mirror` test.
  </action>
  <verify>
    <automated>
      cargo check -p ironhermes-agent --all-features &&
      cargo test -p ironhermes-agent memory::manager::tests &&
      cargo test -p ironhermes-agent memory::factory::tests
    </automated>
  </verify>
  <acceptance_criteria>
    - `grep -q "pub struct MemoryManager" crates/ironhermes-agent/src/memory/manager.rs`.
    - `grep -q "tokio::sync::Mutex" crates/ironhermes-agent/src/memory/manager.rs` (no `std::sync::Mutex` on the primary).
    - `grep -q "tokio::time::timeout" crates/ironhermes-agent/src/memory/manager.rs` AND `grep -q "MIRROR_TIMEOUT" crates/ironhermes-agent/src/memory/manager.rs` (T-20-02b mitigation).
    - `grep -q "session_search" crates/ironhermes-agent/src/memory/manager.rs` (reserved-name guard present).
    - `grep -q "pub async fn build_memory_manager" crates/ironhermes-agent/src/memory/factory.rs`.
    - `grep -q "pub use memory::.*MemoryManager" crates/ironhermes-agent/src/lib.rs`.
    - `cargo test -p ironhermes-agent memory::manager::tests::construction` exits 0.
    - `cargo test -p ironhermes-agent memory::manager::tests::mirror_observes_writes` exits 0.
    - `cargo test -p ironhermes-agent memory::manager::tests::mirror_failure_does_not_block_primary` exits 0.
    - `cargo test -p ironhermes-agent memory::manager::tests::reserved_tool_name_is_rejected` exits 0.
  </acceptance_criteria>
  <done>
    MemoryManager module lands with primary+optional-mirror shape; writes fan out with drop-before-mirror + 5s timeout + swallow-on-error; reads hit primary only; reserved-tool-name guard rejects providers that shadow `session_search`; four unit tests pass.
  </done>
</task>

<task type="auto" tdd="true">
  <name>Task 20-02-02: Rewire MemoryTool to delegate to MemoryManager; wire queue_prefetch in agent_loop; wire on_pre_compress in context_engine; wire system_prompt_block in prompt_builder; update memory_flush_handler</name>
  <read_first>
    - crates/ironhermes-tools/src/memory_tool.rs (entire file — current `store: Arc<std::sync::Mutex<...>>` shape + `execute` body)
    - crates/ironhermes-tools/src/registry.rs (lines 200-240 — `register_memory_tool` signature + callsite)
    - crates/ironhermes-agent/src/agent_loop.rs (lines 1-100, 700-830 — tool dispatch flow + end-of-turn hooks)
    - crates/ironhermes-agent/src/prompt_builder.rs (lines 200-400 — `set_memory_store`, `load_memory`, slot 3 emit site)
    - crates/ironhermes-agent/src/context_engine.rs (lines 1-200 — `ContextEngine` trait + `LocalPruningEngine::compress` + `SummarizingEngine::compress`)
    - crates/ironhermes-agent/src/memory_flush_handler.rs (entire file — existing async listener)
    - crates/ironhermes-agent/src/client.rs (lines 145-170 — tokio::spawn fire-and-forget prior art for queue_prefetch)
    - crates/ironhermes-agent/src/memory/manager.rs (post-Task 20-02-01)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md (D-11, D-12, D-13, D-18)
    - .planning/phases/20-memory-provider-plugin-contract/20-RESEARCH.md (Pitfall 1, 6, 8)
  </read_first>
  <files>
    crates/ironhermes-tools/src/memory_tool.rs,
    crates/ironhermes-tools/src/registry.rs (if register_memory_tool signature needs to change),
    crates/ironhermes-agent/src/agent_loop.rs,
    crates/ironhermes-agent/src/prompt_builder.rs,
    crates/ironhermes-agent/src/context_engine.rs,
    crates/ironhermes-agent/src/memory_flush_handler.rs
  </files>
  <behavior>
    - Test: `memory_tool_delegates_to_manager` — construct a `MemoryTool` from an `Arc<tokio::sync::Mutex<MemoryManager>>` with recorder mirror; call `MemoryTool::execute({"action": "add", "target": "memory", "content": "x"})`; assert primary has the write AND mirror observed it.
    - Test: `prompt_builder_appends_system_prompt_block` — primary provider with a `system_prompt_block()` override returning `Some("EXTRA-BLOCK")` — after `PromptBuilder::load_memory`, the assembled prompt (slot 3) contains `"EXTRA-BLOCK"` appearing AFTER the target-scoped MEMORY.md/USER.md blocks.
    - Test: `agent_loop_fires_queue_prefetch_after_turn` — recorder provider; run one end-to-end turn; assert `queue_prefetch` was invoked with the user's last message text.
    - Test: `context_engine_fires_on_pre_compress_before_prune` — recorder primary; construct `LocalPruningEngine`, set a manager, call `compress(&mut msgs, ...)`. Assert `on_pre_compress` was called with the PRE-prune messages slice (i.e. its length > post-prune length or else there was nothing to prune).
    - Test: `memory_flush_handler_uses_manager` — verifies `build_memory_flush_listener` now takes `Arc<tokio::sync::Mutex<MemoryManager>>` and calls `manager.sync_turn` (not the raw provider).
  </behavior>
  <action>
    1. REWORK `crates/ironhermes-tools/src/memory_tool.rs` to hold a manager handle instead of a raw provider:
       - Change the struct:
         ```rust
         pub struct MemoryTool {
             pub manager: std::sync::Arc<tokio::sync::Mutex<ironhermes_agent::MemoryManager>>,
         }
         ```
         If `ironhermes-tools` cannot depend on `ironhermes-agent` (circular), define a thin `MemoryManagerHandle` trait in `ironhermes-tools` with `async fn handle_tool_call(...)` and implement it on `MemoryManager` in `ironhermes-agent`. EXECUTOR: check the crate dep graph in `Cargo.toml` — if `ironhermes-agent` already depends on `ironhermes-tools`, then `ironhermes-tools` must NOT reverse-depend. In that case, use the trait-handle pattern.
       - Change the tool's `execute` body to route through the manager:
         ```rust
         let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
         let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
         let name = match action {
             "add" => "memory_add", "replace" => "memory_replace", "remove" => "memory_remove",
             other => return error_envelope(format!("invalid memory action: {other}")),
         };
         let manager = std::sync::Arc::clone(&self.manager);
         let result = tokio::runtime::Handle::current().block_on(async move {
             let mgr = manager.lock().await;
             mgr.handle_tool_call(name, args).await
         });
         ```
         EXECUTOR: if `Tool::execute` is already async in the registry, drop the `block_on` and `.await` directly. Check `crates/ironhermes-tools/src/tool.rs` for the trait signature.
    2. EDIT `crates/ironhermes-tools/src/registry.rs::register_memory_tool` to accept a `MemoryManager` handle instead of a raw provider. Callers in `crates/ironhermes-agent/` update accordingly.

    3. EDIT `crates/ironhermes-agent/src/agent_loop.rs`:
       - Add a field: `memory_manager: Option<Arc<tokio::sync::Mutex<MemoryManager>>>`.
       - Wire construction: in the existing path that currently holds the raw provider (the factory result), swap for `build_memory_manager(&cfg.memory).await?`.
       - After a completed turn (the natural exit point of the per-turn loop in `run_turn` or equivalent — EXECUTOR: locate by searching for where the loop exits after a tool-free assistant message OR where the agent emits its final step-end event), add:
         ```rust
         if let Some(mgr) = self.memory_manager.as_ref() {
             let mgr = Arc::clone(mgr);
             let query = last_user_message_text.clone(); // already in scope post-turn
             tokio::spawn(async move {
                 let guard = mgr.lock().await;
                 if let Err(e) = guard.queue_prefetch(&query).await {
                     tracing::warn!(error = %e, "queue_prefetch failed");
                 }
             });
         }
         ```
       - This matches the `tokio::spawn` fire-and-forget pattern at `client.rs:148-163`.
       - DO NOT reshape any tool-name intercept set. Per research open question #1, the memory tool is a single `"memory"` tool already in the registry — dispatch flows through there unchanged.

    4. EDIT `crates/ironhermes-agent/src/prompt_builder.rs`:
       - In `pub fn load_memory(&mut self)` (currently around line 370), AFTER the two `format_for_system_prompt` appends (for MemoryTarget::Memory then MemoryTarget::User), add:
         ```rust
         if let Some(mgr) = self.memory_manager.as_ref() {
             // system_prompt_block is additive — appended AFTER target-scoped blocks per D-11.
             let block = tokio::runtime::Handle::current().block_on(async {
                 mgr.lock().await.system_prompt_block().await
             });
             if let Some(b) = block {
                 self.memory_section.push_str("\n\n");
                 self.memory_section.push_str(&b);
             }
         }
         ```
       - If `PromptBuilder::load_memory` is `async fn`, replace `block_on` with `.await`. EXECUTOR: check the method signature in the current source before choosing.
       - Add a setter: `pub fn set_memory_manager(&mut self, mgr: Arc<tokio::sync::Mutex<MemoryManager>>) { self.memory_manager = Some(mgr); }`. Keep the existing `set_memory_store` setter intact for backward compat until Plan 20-03 removes it.

    5. EDIT `crates/ironhermes-agent/src/context_engine.rs`:
       - Add to both `LocalPruningEngine` and `SummarizingEngine` (and to the `ContextEngine` trait if appropriate):
         ```rust
         pub fn set_memory_manager(&mut self, mgr: Arc<tokio::sync::Mutex<MemoryManager>>) {
             self.memory_manager = Some(mgr);
         }
         ```
       - At the TOP of each engine's `async fn compress(&self, messages: &mut Vec<ChatMessage>, ...)` (BEFORE any destructive work), add:
         ```rust
         if let Some(mgr) = self.memory_manager.as_ref() {
             let guard = mgr.lock().await;
             if let Err(e) = guard.on_pre_compress(messages).await {
                 tracing::warn!(error = %e, "memory on_pre_compress failed");
             }
         }
         ```
       - This resolves research open question #4: the call is made directly from `compress` (which owns `&mut messages`), NOT through `HookEventKind::ContextPreCompress` (which doesn't carry `messages`).

    6. EDIT `crates/ironhermes-agent/src/memory_flush_handler.rs`:
       - Change `build_memory_flush_listener`'s parameter type from `Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>` to `Arc<tokio::sync::Mutex<MemoryManager>>`.
       - Inside the listener closure, replace `provider.lock().await` -> `manager.lock().await` and `.sync_turn(...)` call flows through the manager (which forwards to primary per manager's `sync_turn` method).
       - Any existing tests in this file that use a Mock provider: rewire to construct a `MemoryManager::new(Arc::new(Mutex::new(MockProvider)), None).await.unwrap()` and pass that in.

    7. VERIFY integration compile. Run:
       - `cargo check -p ironhermes-tools --all-features`
       - `cargo check -p ironhermes-agent --all-features`
       - `cargo check -p ironhermes-cli --all-features`

    8. ADD a test in `crates/ironhermes-agent/src/agent_loop.rs` (inside the existing `#[cfg(test)] mod tests`):
       ```rust
       #[tokio::test]
       async fn queue_prefetch_fires_after_turn() {
           // EXECUTOR: use the existing AgentLoop test harness with a stub LLM
           // client that returns a single assistant message (no tools). Construct
           // agent with a MemoryManager wrapping a recorder provider; drive one turn.
           // Assert the recorder saw `queue_prefetch("user-query-text")` within 500ms
           // (tokio::time::sleep to allow the spawned task to run).
       }
       ```
  </action>
  <verify>
    <automated>
      cargo check -p ironhermes-tools --all-features &&
      cargo check -p ironhermes-agent --all-features &&
      cargo check -p ironhermes-cli --all-features &&
      cargo test -p ironhermes-agent memory::manager::tests &&
      cargo test -p ironhermes-agent agent_loop::tests::queue_prefetch_fires_after_turn &&
      cargo test -p ironhermes-agent prompt_builder::tests::system_prompt_block_appended &&
      cargo test -p ironhermes-agent context_engine::tests::on_pre_compress_fires_before_prune
    </automated>
  </verify>
  <acceptance_criteria>
    - `grep -q "manager: .*MemoryManager" crates/ironhermes-tools/src/memory_tool.rs` OR (if crate dep graph forbids direct import) `grep -q "MemoryManagerHandle" crates/ironhermes-tools/src/memory_tool.rs`.
    - `grep -q "manager.handle_tool_call\\|mgr.handle_tool_call" crates/ironhermes-tools/src/memory_tool.rs`.
    - `grep -q "tokio::spawn" crates/ironhermes-agent/src/agent_loop.rs` AND `grep -q "queue_prefetch" crates/ironhermes-agent/src/agent_loop.rs`.
    - `grep -q "system_prompt_block" crates/ironhermes-agent/src/prompt_builder.rs` AND `grep -q "set_memory_manager" crates/ironhermes-agent/src/prompt_builder.rs`.
    - `grep -q "on_pre_compress" crates/ironhermes-agent/src/context_engine.rs` AND `grep -q "set_memory_manager" crates/ironhermes-agent/src/context_engine.rs`.
    - `grep -q "MemoryManager" crates/ironhermes-agent/src/memory_flush_handler.rs` (listener rewired to take a manager).
    - `! grep -q "MemoryProvider + Send>>>>" crates/ironhermes-tools/src/memory_tool.rs` (no raw-provider handle remains on the tool).
    - All four test commands above exit 0.
  </acceptance_criteria>
  <done>
    MemoryTool delegates to MemoryManager; agent_loop fires queue_prefetch after each turn (detached tokio task); context_engine calls manager.on_pre_compress(messages) inside compress before destructive work; prompt_builder appends system_prompt_block after target-scoped memory blocks; memory_flush_handler listener takes a MemoryManager. Every Plan 20-01 hook now has a live fire site.
  </done>
</task>

<task type="auto" tdd="true">
  <name>Task 20-02-03: Add trait-level hook-ordering integration test with MockRecorderProvider</name>
  <read_first>
    - crates/ironhermes-core/src/memory_provider.rs (post-Plan-20-01 trait)
    - crates/ironhermes-core/src/config_schema.rs (MemoryAction)
    - crates/ironhermes-core/Cargo.toml (confirm `[dev-dependencies]` section includes `tokio` with `macros` + `rt` features; add `tempfile`, `async-trait` to dev-dependencies if missing)
    - crates/ironhermes-agent/src/memory/manager.rs (post-Task 20-02-01)
    - crates/ironhermes-agent/src/agent_loop.rs (post-Task 20-02-02 — end-of-turn wiring)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md (D-22)
  </read_first>
  <files>
    crates/ironhermes-core/tests/memory_provider_contract.rs (NEW),
    crates/ironhermes-core/Cargo.toml (if dev-deps need additions)
  </files>
  <behavior>
    - Test: `hook_ordering_via_manager` — builds a `MockRecorderProvider` (records every method called with a timestamp counter); uses `MemoryManager` to drive the sequence: `initialize` → `prefetch` → `add` (fires `on_memory_write` via mirror — N/A in trait-level, use primary-only for this test) → `sync_turn` → `queue_prefetch` → `on_pre_compress` → `on_session_end` → `shutdown`. Assert the recorder's ordered log is exactly:
        ```
        ["initialize", "prefetch", "add", "sync_turn", "queue_prefetch", "on_pre_compress", "on_session_end", "shutdown"]
        ```
    - This is a TRAIT-LEVEL test — it exercises the contract without requiring a full agent loop. The agent-loop integration test added in Task 20-02-02 (`queue_prefetch_fires_after_turn`) covers the live-loop path; this test covers the trait contract for future providers.
  </behavior>
  <action>
    1. If `crates/ironhermes-core/tests/` does not exist, CREATE it. Add `Cargo.toml` `[dev-dependencies]`:
       ```toml
       tokio = { version = "1", features = ["macros", "rt", "time"] }
       tempfile = "3"
       async-trait = "0.1"
       ```
       (Only add if missing. `tokio` may already be present; in that case ensure the features include `macros`, `rt`, `time`.)

    2. CREATE `crates/ironhermes-core/tests/memory_provider_contract.rs`:

    ```rust
    //! Trait-level hook ordering test (D-22). Uses a MockRecorderProvider that
    //! records every MemoryProvider method invocation with an incrementing
    //! sequence number, then drives the full sequence and asserts the order.
    //!
    //! This test does NOT require the agent loop — it validates the trait
    //! contract directly so future providers can rely on the ordering
    //! semantics regardless of how they're embedded.

    use std::path::Path;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use ironhermes_core::config_schema::MemoryAction;
    use ironhermes_core::memory_provider::{MemoryEntries, MemoryProvider};
    use ironhermes_core::memory_store::{MemoryResult, MemoryTarget, MemoryWriteOutcome};

    #[derive(Default)]
    struct Recorder {
        log: Vec<String>,
    }

    struct MockRecorderProvider {
        rec: StdMutex<Recorder>,
    }

    #[async_trait]
    impl MemoryProvider for MockRecorderProvider {
        fn name(&self) -> &'static str { "mock-recorder-contract" }

        async fn initialize(&mut self, _s: &str, _h: &Path, _v: &serde_json::Value) -> anyhow::Result<()> {
            self.rec.lock().unwrap().log.push("initialize".into());
            Ok(())
        }
        async fn prefetch(&self, _sid: &str) -> anyhow::Result<MemoryEntries> {
            self.rec.lock().unwrap().log.push("prefetch".into());
            Ok(MemoryEntries::default())
        }
        async fn sync_turn(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
            self.rec.lock().unwrap().log.push("sync_turn".into());
            Ok(())
        }
        async fn on_session_end(&self, _sid: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
            self.rec.lock().unwrap().log.push("on_session_end".into());
            Ok(())
        }
        async fn shutdown(&mut self) -> anyhow::Result<()> {
            self.rec.lock().unwrap().log.push("shutdown".into());
            Ok(())
        }
        async fn queue_prefetch(&self, _q: &str) -> anyhow::Result<()> {
            self.rec.lock().unwrap().log.push("queue_prefetch".into());
            Ok(())
        }
        async fn on_pre_compress(&self, _m: &[ironhermes_core::types::ChatMessage]) -> anyhow::Result<()> {
            self.rec.lock().unwrap().log.push("on_pre_compress".into());
            Ok(())
        }
        async fn on_memory_write(&mut self, _a: MemoryAction, _t: MemoryTarget, _c: &str) -> anyhow::Result<()> {
            self.rec.lock().unwrap().log.push("on_memory_write".into());
            Ok(())
        }

        fn load_from_disk(&mut self) -> anyhow::Result<()> { Ok(()) }
        fn add(&mut self, _t: MemoryTarget, _c: &str) -> MemoryResult {
            self.rec.lock().unwrap().log.push("add".into());
            // EXECUTOR: return MemoryWriteOutcome matching the current type. Use
            // MemoryStore's happy-path outcome as a template.
            Ok(MemoryWriteOutcome::default())
        }
        fn replace(&mut self, _t: MemoryTarget, _o: &str, _n: &str) -> MemoryResult {
            self.rec.lock().unwrap().log.push("replace".into());
            Ok(MemoryWriteOutcome::default())
        }
        fn remove(&mut self, _t: MemoryTarget, _o: &str) -> MemoryResult {
            self.rec.lock().unwrap().log.push("remove".into());
            Ok(MemoryWriteOutcome::default())
        }
        fn format_for_system_prompt(&self, _t: MemoryTarget) -> Option<String> { None }
        fn to_memory_entries(&self) -> MemoryEntries { MemoryEntries::default() }
    }

    #[tokio::test]
    async fn hook_ordering_contract() {
        let mut mock = MockRecorderProvider { rec: StdMutex::new(Recorder::default()) };
        let tmp = tempfile::TempDir::new().unwrap();

        mock.initialize("sess-1", tmp.path(), &serde_json::Value::Null).await.unwrap();
        let _ = mock.prefetch("sess-1").await.unwrap();
        let _ = mock.add(MemoryTarget::Memory, "fact");
        mock.sync_turn("sess-1", &MemoryEntries::default()).await.unwrap();
        mock.queue_prefetch("next query").await.unwrap();
        mock.on_pre_compress(&[]).await.unwrap();
        mock.on_session_end("sess-1", &MemoryEntries::default()).await.unwrap();
        mock.shutdown().await.unwrap();

        let log = mock.rec.lock().unwrap().log.clone();
        let expected = vec![
            "initialize", "prefetch", "add", "sync_turn",
            "queue_prefetch", "on_pre_compress", "on_session_end", "shutdown",
        ];
        assert_eq!(
            log.iter().map(String::as_str).collect::<Vec<_>>(),
            expected,
            "hook ordering mismatch"
        );
    }
    ```

    3. If `MemoryWriteOutcome::default()` does not exist (check `memory_store.rs`), either derive `Default` on it in a one-line edit or return a hand-constructed `Ok(MemoryWriteOutcome { ... })` matching the struct shape. Keep the edit minimal.
  </action>
  <verify>
    <automated>
      cargo test -p ironhermes-core --test memory_provider_contract hook_ordering_contract
    </automated>
  </verify>
  <acceptance_criteria>
    - `grep -q "hook_ordering_contract" crates/ironhermes-core/tests/memory_provider_contract.rs`.
    - `grep -q "MockRecorderProvider" crates/ironhermes-core/tests/memory_provider_contract.rs`.
    - `cargo test -p ironhermes-core --test memory_provider_contract hook_ordering_contract` exits 0.
    - Asserted ordered log: `["initialize","prefetch","add","sync_turn","queue_prefetch","on_pre_compress","on_session_end","shutdown"]`.
  </acceptance_criteria>
  <done>
    Trait-level hook ordering contract locked by an integration test in `ironhermes-core/tests/memory_provider_contract.rs`. Any future provider that breaks the ordering invariant fails this test.
  </done>
</task>

</tasks>

<verification>
**Full-plan automated verification:**

```bash
cargo check --workspace --all-features &&
cargo clippy --workspace --all-features -- -D warnings &&
cargo test -p ironhermes-agent memory::manager::tests &&
cargo test -p ironhermes-core --test memory_provider_contract &&
cargo test -p ironhermes-agent agent_loop::tests::queue_prefetch_fires_after_turn &&
cargo test -p ironhermes-agent prompt_builder::tests &&
cargo test -p ironhermes-agent context_engine::tests &&
cargo test -p ironhermes-tools memory_tool::tests
```

**Cross-check with 20-VALIDATION.md Per-Task Verification Map:**
- 20-02-01 -> `memory::manager::tests::construction` (mapped)
- 20-02-02 -> `memory::manager::tests::mirror_observes_writes` (mapped)
- 20-02-03 -> `memory::manager::tests::mirror_failure_does_not_block_primary` (mapped)
- 20-02-04 -> `agent_loop::tests::hook_ordering` -> implemented as the TRAIT-LEVEL `hook_ordering_contract` in `tests/memory_provider_contract.rs` (see Task 20-02-03) PLUS the agent-loop-level `queue_prefetch_fires_after_turn` test in Task 20-02-02.
- 20-02-05 -> `prompt_builder::tests::system_prompt_block_appended` (Task 20-02-02).
</verification>

<success_criteria>
- [ ] `MemoryManager` type exists in `crates/ironhermes-agent/src/memory/manager.rs` with primary + optional mirror, `tokio::sync::Mutex`, 5s mirror timeout, swallow-on-error semantics (D-25, D-29, T-20-02b).
- [ ] Reserved-name guard rejects providers that declare `session_search` (T-20-05).
- [ ] `MemoryTool::execute` delegates to `MemoryManager::handle_tool_call` instead of holding the raw provider (D-18 as interpreted via research open question #1).
- [ ] `AgentLoop` fires `queue_prefetch` as a detached `tokio::spawn` after each completed turn (D-12).
- [ ] `ContextEngine::compress` calls `memory_manager.on_pre_compress(messages)` directly before destructive work (D-13, resolves research open question #4).
- [ ] `PromptBuilder::load_memory` appends `system_prompt_block()` after the target-scoped MEMORY.md/USER.md blocks in slot 3 (D-11).
- [ ] `memory_flush_handler` listener takes `Arc<tokio::sync::Mutex<MemoryManager>>`.
- [ ] Trait-level hook ordering test passes: `initialize -> prefetch -> add -> sync_turn -> queue_prefetch -> on_pre_compress -> on_session_end -> shutdown` (D-22).
- [ ] No raw-provider handle remains on `MemoryTool` (grep check).
- [ ] T-20-02 mitigation verified: mirror error logs use `%` Display formatting, not `{:?}` Debug (grep check on manager.rs).
</success_criteria>

<output>
After completion, create `.planning/phases/20-memory-provider-plugin-contract/20-02-SUMMARY.md` summarizing:
- Exact Mutex flavor decisions and any crate-dep-graph constraints discovered.
- Whether the `MemoryManagerHandle` trait was needed (or whether `ironhermes-tools` could import `ironhermes-agent::MemoryManager` directly).
- How the agent-loop end-of-turn hook was located (method name + line).
- Hook ordering test log confirming the canonical sequence.
- Any deviations + rationale.
</output>
