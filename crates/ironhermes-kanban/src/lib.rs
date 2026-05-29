//! IronHermes Kanban — durable, profile-aware work-queue kernel.
//!
//! This crate owns the `~/.ironhermes/kanban.db` SQLite board and the
//! atomic-claim CAS helpers, dispatcher primitives, and worker-side DB API
//! used by the kanban subsystem (Phase 36.3.7).
//!
//! Plan 01 scaffolds the public type/path/event/config surface that
//! downstream plans (02 store, 03 dispatcher, 04 tools, etc.) consume.
//! Plan 01 Task 1 creates the crate + error type; Plan 01 Task 2 adds
//! the types/paths/events/config/pid modules.

pub mod error;
pub use error::{KanbanError, Result};
