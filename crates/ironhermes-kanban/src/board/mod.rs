//! Multi-board context types and resolution (Phase 36.3.7.9).
//!
//! This module provides:
//! - [`BoardContext`] — the resolved board (slug + db_path + source)
//! - [`BoardSource`] — which tier of the 4-tier chain produced the slug
//! - [`resolve_board_context`] — the pure 4-tier resolver (D-02)
//! - [`default_board_context`] — convenience helper for the fallback path
//! - [`validate_board_slug`] / [`BoardSlugError`] — T-1 path-traversal guard

pub mod resolver;
pub mod slug;

use std::path::PathBuf;

/// Resolved board context returned by [`resolve_board_context`].
#[derive(Debug, Clone)]
pub struct BoardContext {
    /// Validated, downcased board slug (e.g. `"default"`, `"atm10-server"`).
    pub slug: String,
    /// Absolute path to the board's SQLite database file.
    pub db_path: PathBuf,
    /// Which resolution tier produced this context.
    pub source: BoardSource,
}

/// Resolution tier that produced a [`BoardContext`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardSource {
    /// Explicit `--board <slug>` CLI flag (tier 1).
    Flag,
    /// `IRONHERMES_KANBAN_BOARD` environment variable (tier 2).
    Env,
    /// `~/.ironhermes/kanban/current` file (tier 3).
    File,
    /// Literal `"default"` fallback (tier 4).
    Default,
}

impl BoardSource {
    /// Returns the lowercase string representation consumed by Plan 07 envelope
    /// serialisation: `"flag"`, `"env"`, `"file"`, or `"default"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            BoardSource::Flag => "flag",
            BoardSource::Env => "env",
            BoardSource::File => "file",
            BoardSource::Default => "default",
        }
    }
}

/// Convenience constructor for the `Default` tier context (used by Plan 07
/// tool `Err` fallback paths that need a valid `BoardContext` to serialize
/// the `board_source` envelope field even when resolution fails).
pub fn default_board_context() -> BoardContext {
    BoardContext {
        slug: "default".to_string(),
        db_path: crate::paths::kanban_db_path(),
        source: BoardSource::Default,
    }
}

// Re-exports for consumers that `use crate::board::*` or
// `use ironhermes_kanban::board::*`.
pub use resolver::resolve_board_context;
pub use slug::{BoardSlugError, validate_board_slug};
