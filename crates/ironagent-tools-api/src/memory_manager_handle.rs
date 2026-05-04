//! Thin trait that lets `MemoryTool` delegate writes without
//! taking a direct dependency on `ironhermes-agent::MemoryManager`.
//!
//! `ironhermes-agent` already depends on `ironhermes-tools`, so a direct
//! `use ironhermes_agent::MemoryManager` in the tools crate would create
//! a circular dependency. The trait + `dyn MemoryManagerHandle` shape
//! keeps the wiring one-directional: tools defines the trait, the agent
//! crate implements it on `MemoryManager`.
//!
//! The trait is intentionally minimal — just `handle_tool_call`. Read-path
//! plumbing (system_prompt_block etc.) stays on the concrete manager, because
//! PromptBuilder is in the agent crate and can import `MemoryManager` directly.

use async_trait::async_trait;
use ironhermes_core::memory_store::MemoryResult;

/// Shared handle to a MemoryManager. Implemented by
/// `ironhermes_agent::MemoryManager` (in the agent crate).
///
/// Wrapped as `std::sync::Arc<tokio::sync::Mutex<dyn MemoryManagerHandle + Send>>`
/// at the callsite so `MemoryTool` can lock + call `.handle_tool_call(...)`
/// from its async `execute` body.
#[async_trait]
pub trait MemoryManagerHandle: Send {
    /// Route a memory write (add/replace/remove) through the manager's
    /// primary provider, with optional mirror fanout.
    ///
    /// Returns the JSON envelope from the primary provider unchanged —
    /// MemoryTool formats success/error shapes for the LLM.
    async fn handle_tool_call(&self, name: &str, args: serde_json::Value) -> MemoryResult;
}
