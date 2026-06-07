//! Phase 36.3.7.11 Plan 02 — client-side kanban utilities.
//!
//! Currently contains the drag-and-drop transition validator. The
//! server-side canonical of the same table lives in
//! `crate::server::kanban_api::patch_task_status` (which imports
//! `is_drag_allowed` from this module — D-14 reuse).
//!
//! KEEP IN SYNC with the canonical transition rules — both client and
//! server call the same function so the only drift risk is the wire-copy
//! `KanbanStatus` enum in `crate::protocol` versus the server's
//! `ironhermes_kanban::KanbanStatus`. The two are locked by inline tests
//! against `KanbanStatus::as_str` casing in `protocol.rs` (D-09 wire).

pub mod transitions;
