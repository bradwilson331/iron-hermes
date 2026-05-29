//! Error types for the kanban kernel.
//!
//! Mirrors the `ironhermes-state::StateError` shape: a `thiserror`-derived enum
//! with `#[from]` conversions for the foreign error types we transparently
//! propagate (`rusqlite::Error`, `serde_json::Error`, `anyhow::Error`).

use thiserror::Error;

/// All errors emitted by the kanban kernel.
#[derive(Debug, Error)]
pub enum KanbanError {
    /// SQLite operation failed (open, prepare, execute, etc.).
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// JSON serialization or deserialization failed.
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// Task lookup by id failed.
    #[error("task not found: {0}")]
    TaskNotFound(String),

    /// Cross-tenant `task_links` rejected at link time (D-39).
    #[error("tenant mismatch: parent={parent}, child={child}")]
    TenantMismatch {
        /// Tenant string from the parent task ("" when tenant-less).
        parent: String,
        /// Tenant string from the child task ("" when tenant-less).
        child: String,
    },

    /// `dir:<path>` workspace with a non-absolute path (D-31 confused-deputy
    /// gate; Pitfall 6 in RESEARCH.md).
    #[error("`dir:` workspace must use an absolute path: {0}")]
    RelativeDirWorkspace(String),

    /// Worker protocol violation (e.g. exit-0 with task still running, D-27).
    #[error("kanban protocol violation: {0}")]
    ProtocolViolation(String),

    /// Generic fallback for ad-hoc errors carried from `anyhow`.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T, E = KanbanError> = std::result::Result<T, E>;
