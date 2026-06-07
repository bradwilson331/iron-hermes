//! LLM tool surface for the kanban kernel (Plan 04, D-20).
//!
//! Six tools covering the worker lifecycle:
//! - [`KanbanShowTool`]    — orient: read task + parent handoffs + prior attempts
//! - [`KanbanListTool`]   — survey the board
//! - [`KanbanCommentTool`] — mid-work annotation (D-41 claim_lock gated in worker mode)
//! - [`KanbanCompleteTool`] — protocol terminator (D-22 expected_run_id + created_cards gates)
//! - [`KanbanBlockTool`]  — protocol terminator (D-23 expected_run_id gate)
//! - [`KanbanCreateTool`] — orchestrator fanout (D-24 idempotency, parents gating)
//!
//! Tools are registered under `toolset = "kanban"` so the ToolRegistry can
//! scope them in / out via `scope_to(&["kanban"])`.
//!
//! Tools gate visibility on:
//! - Worker mode: `HERMES_KANBAN_TASK` env var is set (D-20).
//! - Orchestrator mode: `explicit_enable = true` passed at registration (D-20).

pub mod block;
pub mod comment;
pub mod common;
pub mod complete;
pub mod create;
pub mod heartbeat;
pub mod link;
pub mod list;
pub mod mention;
pub mod show;
pub mod swarm;
pub mod unblock;
// Phase 36.3.7.10 — kanban_decompose + kanban_specify (LLM-driven triage tools).
pub mod decompose;
pub mod specify;

pub use block::KanbanBlockTool;
pub use comment::KanbanCommentTool;
pub use complete::KanbanCompleteTool;
pub use create::KanbanCreateTool;
pub use decompose::KanbanDecomposeTool;
pub use heartbeat::KanbanHeartbeatTool;
pub use link::KanbanLinkTool;
pub use list::KanbanListTool;
pub use mention::KanbanMentionTool;
pub use show::KanbanShowTool;
pub use specify::KanbanSpecifyTool;
pub use swarm::KanbanSwarmTool;
pub use unblock::KanbanUnblockTool;

use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

use crate::store::KanbanStore;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::Mutex as TokioMutex;

    /// Tool-count regression: `register_kanban_tools` must register exactly 13 tools
    /// so that schema counts and multi-board envelope assertions remain stable.
    #[test]
    fn register_kanban_tools_registers_exactly_thirteen() {
        let dir = tempdir().unwrap();
        let store = KanbanStore::new(dir.path().join("test.db")).unwrap();
        std::mem::forget(dir);
        let store = Arc::new(TokioMutex::new(store));
        let mut registry = ironhermes_tools::ToolRegistry::new();
        let before = registry.list_tools().len();
        register_kanban_tools(&mut registry, store, false);
        let after = registry.list_tools().len();
        assert_eq!(
            after - before,
            13,
            "register_kanban_tools must register exactly 13 tools, got {}",
            after - before
        );
    }
}

/// Register all 13 kanban tools onto `registry`.
///
/// Each tool shares the same `store` Arc for direct in-process DB access (D-20
/// backend portability — no CLI shelling).
///
/// `explicit_enable` mirrors the orchestrator-mode gate: when `true`, tools are
/// available even without `HERMES_KANBAN_TASK` set.  Plan 05 passes `true` when
/// the session is constructed with an explicit kanban toolset enable.
pub fn register_kanban_tools(
    registry: &mut ironhermes_tools::ToolRegistry,
    store: Arc<TokioMutex<KanbanStore>>,
    explicit_enable: bool,
) {
    registry.register(Box::new(KanbanShowTool::new(store.clone(), explicit_enable)));
    registry.register(Box::new(KanbanListTool::new(store.clone(), explicit_enable)));
    registry.register(Box::new(KanbanCommentTool::new(
        store.clone(),
        explicit_enable,
    )));
    registry.register(Box::new(KanbanCompleteTool::new(
        store.clone(),
        explicit_enable,
    )));
    registry.register(Box::new(KanbanBlockTool::new(store.clone(), explicit_enable)));
    registry.register(Box::new(KanbanCreateTool::new(store.clone(), explicit_enable)));
    // Phase 36.3.7.6 BUG-36.3.7.6-01 — kanban_heartbeat (D-heartbeat-impl: event row).
    registry.register(Box::new(KanbanHeartbeatTool::new(
        store.clone(),
        explicit_enable,
    )));
    // Phase 36.3.7.6 BUG-36.3.7.6-02 — kanban_link (D-link-cycle-detection: WITH RECURSIVE).
    registry.register(Box::new(KanbanLinkTool::new(
        store.clone(),
        explicit_enable,
    )));
    // Phase 36.3.7.6 BUG-36.3.7.6-03 — kanban_unblock (D-unblock-status-precondition: handler-side gate).
    registry.register(Box::new(KanbanUnblockTool::new(
        store.clone(),
        explicit_enable,
    )));
    // Phase 36.3.7.7 BUG-36.3.7.7-01 — kanban_swarm (D-topology-shapes: atomic fan-out graph).
    registry.register(Box::new(KanbanSwarmTool::new(
        store.clone(),
        explicit_enable,
    )));
    // Phase 36.3.7.8 — kanban_mention (@mention delegation parser inline routing).
    registry.register(Box::new(KanbanMentionTool::new(store.clone(), explicit_enable)));
    // Phase 36.3.7.10 — kanban_decompose (LLM-driven triage decomposer).
    registry.register(Box::new(KanbanDecomposeTool::new(store.clone(), explicit_enable)));
    // Phase 36.3.7.10 — kanban_specify (LLM-driven triage specifier, no fan-out).
    registry.register(Box::new(KanbanSpecifyTool::new(store.clone(), explicit_enable)));
}
