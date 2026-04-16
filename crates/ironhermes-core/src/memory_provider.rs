//! MemoryProvider trait and supporting types for pluggable memory backends.
//!
//! MEM-07: Trait with Send + Sync + 'static bounds and async lifecycle hooks.
//! MEM-08: MemoryStore implements the trait as the default file-based backend.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::memory_store::{MemoryResult, MemoryStore, MemoryTarget};

// =============================================================================
// MemoryEntries wrapper
// =============================================================================

#[derive(Debug, Clone, Default)]
pub struct MemoryEntries {
    pub entries: HashMap<MemoryTarget, Vec<String>>,
}

// =============================================================================
// MemoryProviderConfig
// =============================================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryProviderConfig {
    pub provider: String,
    pub memory_dir: PathBuf,
    pub memory_char_limit: usize,
    pub user_char_limit: usize,
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

// =============================================================================
// MemoryProvider trait (MEM-07)
// =============================================================================

#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    // Lifecycle hooks (async)
    async fn initialize(&mut self, config: &MemoryProviderConfig) -> anyhow::Result<()>;
    async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries>;
    async fn sync_turn(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()>;
    async fn on_session_end(
        &self,
        session_id: &str,
        entries: &MemoryEntries,
    ) -> anyhow::Result<()>;
    async fn shutdown(&mut self) -> anyhow::Result<()>;

    // Operational methods (sync)
    fn load_from_disk(&mut self) -> anyhow::Result<()>;
    fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult;
    fn replace(&mut self, target: MemoryTarget, old_text: &str, new_content: &str)
        -> MemoryResult;
    fn remove(&mut self, target: MemoryTarget, old_text: &str) -> MemoryResult;
    fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String>;
    fn to_memory_entries(&self) -> MemoryEntries;
}

// =============================================================================
// MemoryProvider impl for MemoryStore (MEM-08)
// =============================================================================

#[async_trait]
impl MemoryProvider for MemoryStore {
    async fn initialize(&mut self, _config: &MemoryProviderConfig) -> anyhow::Result<()> {
        // File-based provider is already initialized at construction; no-op.
        Ok(())
    }

    async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
        Ok(self.to_memory_entries())
    }

    async fn sync_turn(&self, _session_id: &str, _entries: &MemoryEntries) -> anyhow::Result<()> {
        // File-based provider writes on every mutation; no-op for sync_turn.
        Ok(())
    }

    async fn on_session_end(
        &self,
        _session_id: &str,
        _entries: &MemoryEntries,
    ) -> anyhow::Result<()> {
        // File-based provider persists on every mutation; no-op.
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        // No resources to release for file-based provider.
        Ok(())
    }

    fn load_from_disk(&mut self) -> anyhow::Result<()> {
        MemoryStore::load_from_disk(self)
    }

    fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult {
        MemoryStore::add(self, target, content)
    }

    fn replace(
        &mut self,
        target: MemoryTarget,
        old_text: &str,
        new_content: &str,
    ) -> MemoryResult {
        MemoryStore::replace(self, target, old_text, new_content)
    }

    fn remove(&mut self, target: MemoryTarget, old_text: &str) -> MemoryResult {
        MemoryStore::remove(self, target, old_text)
    }

    fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String> {
        MemoryStore::format_for_system_prompt(self, target)
    }

    fn to_memory_entries(&self) -> MemoryEntries {
        MemoryEntries {
            entries: self.entries().clone(),
        }
    }
}

