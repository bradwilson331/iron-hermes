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
pub mod complete;
pub mod create;
pub mod heartbeat;
pub mod list;
pub mod show;

pub use block::KanbanBlockTool;
pub use comment::KanbanCommentTool;
pub use complete::KanbanCompleteTool;
pub use create::KanbanCreateTool;
pub use heartbeat::KanbanHeartbeatTool;
pub use list::KanbanListTool;
pub use show::KanbanShowTool;

use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

use crate::store::KanbanStore;

/// Register all 6 kanban tools onto `registry`.
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
}
