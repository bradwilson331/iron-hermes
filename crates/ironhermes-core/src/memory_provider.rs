//! MemoryProvider trait and supporting types for pluggable memory backends.
//!
//! MEM-07: Trait with Send + Sync + 'static bounds and async lifecycle hooks.
//! MEM-08: MemoryStore implements the trait as the default file-based backend.
//!
//! Phase 20 (D-01..D-15): Enriched hook surface — `name`, `is_available`,
//! `unavailable_reason`, `get_tool_schemas`, `handle_tool_call`,
//! `get_config_schema`, `save_config`, `system_prompt_block`, `queue_prefetch`,
//! `on_pre_compress`, `on_memory_write`. `initialize` is a breaking signature
//! change (D-10); `MemoryProviderConfig` is deleted.
//!
//! Security contract (T-20-01): providers that consume path-like strings
//! from `provider_config: &Value` MUST canonicalize and assert
//! `starts_with(hermes_home)` before use. The default file-provider
//! `initialize` is a no-op and inherits no surface.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use crate::config_schema::{ConfigField, MemoryAction};
use crate::memory_store::{MemoryResult, MemoryStore, MemoryTarget};
use crate::types::{ChatMessage, ToolSchema};

// =============================================================================
// MemoryEntries wrapper
// =============================================================================

#[derive(Debug, Clone, Default)]
pub struct MemoryEntries {
    pub entries: HashMap<MemoryTarget, Vec<String>>,
}

// =============================================================================
// Default tool schemas (used by `get_tool_schemas` default impl)
// =============================================================================

fn default_memory_tool_schemas() -> Vec<ToolSchema> {
    // TODO(20-04): decompose the existing action-based `memory` tool schema
    // into three discrete schemas (`memory_add`, `memory_replace`,
    // `memory_remove`) so providers that opt in to `get_tool_schemas` can
    // advertise them directly. For Plan 20-01 the conservative default is
    // empty — the existing tool registry already owns a `"memory"` tool, so
    // returning `vec![]` keeps the default wire-compatible while the trait
    // surface is in place for Plan 20-04 overrides.
    vec![]
}

// =============================================================================
// MemoryProvider trait (MEM-07) — enriched surface
// =============================================================================

#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    // ---- Identity (D-02, D-03) ----
    /// Stable provider identifier used for config file naming and logs.
    /// Must be a filename-safe literal — no slashes, dots, or `..`.
    fn name(&self) -> &'static str;

    fn is_available(&self) -> bool { true }
    fn unavailable_reason(&self) -> Option<String> { None }

    // ---- Tool surface (D-04, D-05) ----
    fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        default_memory_tool_schemas()
    }

    fn handle_tool_call(&mut self, name: &str, args: Value) -> MemoryResult {
        // Default dispatch: today's memory tool is a single tool with
        // action arg, but for trait-level parity we also accept the
        // hermes-agent naming `memory_add / memory_replace / memory_remove`.
        let target = match parse_target(&args) {
            Ok(t) => t,
            Err(e) => return Err(e.to_string()),
        };
        match name {
            "memory_add" | "add" => {
                let content = match args.get("content").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => return Err("missing `content`".to_string()),
                };
                self.add(target, content)
            }
            "memory_replace" | "replace" => {
                let old_text = match args.get("old_text").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => return Err("missing `old_text`".to_string()),
                };
                let new_content = match args.get("new_content").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => return Err("missing `new_content`".to_string()),
                };
                self.replace(target, old_text, new_content)
            }
            "memory_remove" | "remove" => {
                let old_text = match args.get("old_text").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => return Err("missing `old_text`".to_string()),
                };
                self.remove(target, old_text)
            }
            other => Err(format!("unknown memory tool: {other}")),
        }
    }

    // ---- Config schema (D-06, D-07) ----
    fn get_config_schema(&self) -> Vec<ConfigField> { vec![] }

    fn save_config(
        &self,
        _values: &HashMap<String, Value>,
        _hermes_home: &Path,
    ) -> anyhow::Result<()> {
        // T-20-04: default guard against accidental traversal in future
        // overrides. `name()` is `&'static str` so this is a programmer
        // safeguard rather than a runtime validation.
        debug_assert!(
            !self.name().contains(['/', '\\']) && !self.name().contains(".."),
            "MemoryProvider::name() must be a filename-safe literal, got: {}",
            self.name()
        );
        Ok(())
    }

    // ---- Prompt integration (D-11) ----
    fn system_prompt_block(&self) -> Option<String> { None }

    // ---- Async lifecycle (D-10, D-12..D-15) ----
    async fn initialize(
        &mut self,
        session_id: &str,
        hermes_home: &Path,
        provider_config: &Value,
    ) -> anyhow::Result<()>;

    async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries>;
    async fn sync_turn(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()>;

    async fn queue_prefetch(&self, _query: &str) -> anyhow::Result<()> { Ok(()) }
    async fn on_pre_compress(&self, _messages: &[ChatMessage]) -> anyhow::Result<()> { Ok(()) }
    async fn on_memory_write(
        &mut self,
        _action: MemoryAction,
        _target: MemoryTarget,
        _content: &str,
    ) -> anyhow::Result<()> { Ok(()) }

    async fn on_session_end(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()>;
    async fn shutdown(&mut self) -> anyhow::Result<()>;

    // ---- Sync operations (unchanged) ----
    fn load_from_disk(&mut self) -> anyhow::Result<()>;
    fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult;
    fn replace(&mut self, target: MemoryTarget, old_text: &str, new_content: &str) -> MemoryResult;
    fn remove(&mut self, target: MemoryTarget, old_text: &str) -> MemoryResult;
    fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String>;
    fn to_memory_entries(&self) -> MemoryEntries;
}

fn parse_target(args: &Value) -> anyhow::Result<MemoryTarget> {
    let raw = args
        .get("target")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing `target`"))?;
    match raw {
        "memory" => Ok(MemoryTarget::Memory),
        "user" => Ok(MemoryTarget::User),
        other => Err(anyhow::anyhow!("invalid target: {other}")),
    }
}

// =============================================================================
// MemoryProvider impl for MemoryStore (MEM-08) — file-based default
// =============================================================================

#[async_trait]
impl MemoryProvider for MemoryStore {
    fn name(&self) -> &'static str { "file" }

    async fn initialize(
        &mut self,
        _session_id: &str,
        _hermes_home: &Path,
        _provider_config: &Value,
    ) -> anyhow::Result<()> {
        // Pitfall 5: file provider is constructed by `MemoryStore::new(memory_dir)`.
        // Keep `initialize` a no-op to avoid double-construction.
        Ok(())
    }

    async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
        Ok(self.to_memory_entries())
    }

    async fn sync_turn(&self, _session_id: &str, _entries: &MemoryEntries) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_session_end(
        &self,
        _session_id: &str,
        _entries: &MemoryEntries,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> { Ok(()) }

    fn load_from_disk(&mut self) -> anyhow::Result<()> { MemoryStore::load_from_disk(self) }
    fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult {
        MemoryStore::add(self, target, content)
    }
    fn replace(&mut self, target: MemoryTarget, old_text: &str, new_content: &str) -> MemoryResult {
        MemoryStore::replace(self, target, old_text, new_content)
    }
    fn remove(&mut self, target: MemoryTarget, old_text: &str) -> MemoryResult {
        MemoryStore::remove(self, target, old_text)
    }
    fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String> {
        MemoryStore::format_for_system_prompt(self, target)
    }
    fn to_memory_entries(&self) -> MemoryEntries {
        MemoryEntries { entries: self.entries().clone() }
    }
}

// Note: The `MemoryProviderConfig` struct has been REMOVED in Phase 20 (D-10).
// Provider-specific config is passed via `initialize(_, _, &Value)`.

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_store::{MemoryResult, MemoryStore, MemoryTarget};

    #[tokio::test]
    async fn memory_store_implements_new_trait() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memories");
        let mut store = MemoryStore::new(mem_dir);

        // name()
        assert_eq!(store.name(), "file");
        // is_available()
        assert!(store.is_available());
        // unavailable_reason()
        assert!(store.unavailable_reason().is_none());
        // initialize() is a no-op with the new signature
        let hermes_home = tmp.path().to_path_buf();
        store
            .initialize("test-session", &hermes_home, &Value::Null)
            .await
            .expect("file provider initialize is no-op");
    }

    #[tokio::test]
    async fn default_hook_methods_return_defaults() {
        // Minimal test-local provider that only provides the required method
        // bodies — exercises the default impls of is_available,
        // unavailable_reason, get_tool_schemas, get_config_schema,
        // save_config, system_prompt_block, queue_prefetch, on_pre_compress,
        // on_memory_write.
        struct MinimalProvider;

        #[async_trait]
        impl MemoryProvider for MinimalProvider {
            fn name(&self) -> &'static str { "minimal" }

            async fn initialize(
                &mut self,
                _session_id: &str,
                _hermes_home: &Path,
                _provider_config: &Value,
            ) -> anyhow::Result<()> { Ok(()) }

            async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
                Ok(MemoryEntries::default())
            }
            async fn sync_turn(&self, _s: &str, _e: &MemoryEntries) -> anyhow::Result<()> { Ok(()) }
            async fn on_session_end(&self, _s: &str, _e: &MemoryEntries) -> anyhow::Result<()> {
                Ok(())
            }
            async fn shutdown(&mut self) -> anyhow::Result<()> { Ok(()) }

            fn load_from_disk(&mut self) -> anyhow::Result<()> { Ok(()) }
            fn add(&mut self, _t: MemoryTarget, _c: &str) -> MemoryResult { Ok("{}".to_string()) }
            fn replace(
                &mut self,
                _t: MemoryTarget,
                _o: &str,
                _n: &str,
            ) -> MemoryResult { Ok("{}".to_string()) }
            fn remove(&mut self, _t: MemoryTarget, _o: &str) -> MemoryResult {
                Ok("{}".to_string())
            }
            fn format_for_system_prompt(&self, _t: MemoryTarget) -> Option<String> { None }
            fn to_memory_entries(&self) -> MemoryEntries { MemoryEntries::default() }
        }

        let mut p = MinimalProvider;
        assert_eq!(p.name(), "minimal");
        assert!(p.is_available(), "default is_available should be true");
        assert!(p.unavailable_reason().is_none(), "default reason is None");
        assert!(p.get_tool_schemas().is_empty(), "default schemas is empty Vec");
        assert!(p.get_config_schema().is_empty(), "default config schema is empty");
        assert!(p.system_prompt_block().is_none(), "default prompt block is None");

        // Default async hooks succeed
        p.queue_prefetch("q").await.unwrap();
        p.on_pre_compress(&[]).await.unwrap();
        p.on_memory_write(MemoryAction::Add, MemoryTarget::Memory, "x")
            .await
            .unwrap();

        // save_config default is Ok(())
        let tmp = tempfile::TempDir::new().unwrap();
        p.save_config(&HashMap::new(), tmp.path()).unwrap();

        // handle_tool_call default with unknown name errors
        let err = p
            .handle_tool_call(
                "totally_unknown",
                serde_json::json!({ "target": "memory" }),
            )
            .expect_err("unknown tool should error");
        assert!(err.contains("unknown memory tool"), "got: {err}");

        // handle_tool_call dispatches to add when target+content present
        let ok = p
            .handle_tool_call(
                "memory_add",
                serde_json::json!({ "target": "memory", "content": "hi" }),
            )
            .expect("add should dispatch via default");
        assert_eq!(ok, "{}");
    }
}
