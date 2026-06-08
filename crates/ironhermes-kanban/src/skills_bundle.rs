//! Bundled kanban skill assets (D-29 / D-30).
//!
//! The two SKILL.md files are embedded at compile time via `include_str!` so
//! that the binary is self-contained and the sync helper can write them into
//! `~/.ironhermes/skills/` on first run without any network call.
//!
//! Translation rules (D-29): upstream `hermes -p` → `ironhermes --profile` and
//! `~/.hermes/` → `~/.ironhermes/`.  NO other semantic edits.

use std::path::Path;

use crate::error::{KanbanError, Result};

// ---------------------------------------------------------------------------
// Bundled content constants
// ---------------------------------------------------------------------------

/// Bundled content of `skills/kanban-worker/SKILL.md` (v2.0.0), translated
/// per D-29 substitution rules.
pub const KANBAN_WORKER_SKILL_MD: &str = include_str!("../../../skills/kanban-worker/SKILL.md");

/// Bundled content of `skills/kanban-orchestrator/SKILL.md` (v3.0.0),
/// translated per D-29 substitution rules.
pub const KANBAN_ORCHESTRATOR_SKILL_MD: &str =
    include_str!("../../../skills/kanban-orchestrator/SKILL.md");

/// Canonical directory name for the kanban-worker skill.
pub const KANBAN_WORKER_SKILL_NAME: &str = "kanban-worker";

/// Canonical directory name for the kanban-orchestrator skill.
pub const KANBAN_ORCHESTRATOR_SKILL_NAME: &str = "kanban-orchestrator";

// ---------------------------------------------------------------------------
// Sync report types
// ---------------------------------------------------------------------------

/// Result of syncing a single bundled skill to disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    /// File was written (either newly created or force-restored).
    Wrote,
    /// File already existed and `force=false` — skipped to preserve user edits
    /// (D-30).
    Skipped,
    /// File was force-overwritten (force=true).  Alias of `Wrote` but
    /// distinguishes "first-write" from "force-restore" at the call site for
    /// human-readable messaging.
    Restored,
}

/// Aggregated result of a [`sync_bundled_kanban_skills`] call.
#[derive(Debug)]
pub struct SyncReport {
    /// Action taken for the kanban-worker skill.
    pub worker_action: SyncAction,
    /// Action taken for the kanban-orchestrator skill.
    pub orchestrator_action: SyncAction,
}

// ---------------------------------------------------------------------------
// Sync helper
// ---------------------------------------------------------------------------

/// Sync both bundled kanban skills into `skills_root`.
///
/// For each skill:
/// - Destination: `<skills_root>/<name>/SKILL.md`
/// - If `force=false` and the destination already exists: returns
///   [`SyncAction::Skipped`] (preserves user edits — D-30).
/// - If `force=true` or the destination does not exist: creates parent dirs,
///   writes the bundled content, returns [`SyncAction::Wrote`] /
///   [`SyncAction::Restored`].
///
/// # Errors
///
/// Returns [`KanbanError`] if directory creation or file write fails.
pub fn sync_bundled_kanban_skills(skills_root: &Path, force: bool) -> Result<SyncReport> {
    let worker_action = sync_one(
        skills_root,
        KANBAN_WORKER_SKILL_NAME,
        KANBAN_WORKER_SKILL_MD,
        force,
    )?;
    let orchestrator_action = sync_one(
        skills_root,
        KANBAN_ORCHESTRATOR_SKILL_NAME,
        KANBAN_ORCHESTRATOR_SKILL_MD,
        force,
    )?;
    Ok(SyncReport {
        worker_action,
        orchestrator_action,
    })
}

/// Force-write a single bundled kanban skill into `skills_root`.
///
/// Looks up the bundled content by `skill_name` (either `"kanban-worker"` or
/// `"kanban-orchestrator"`).  Any other name returns
/// [`KanbanError::UnknownSkill`].
///
/// Always overwrites the destination (equivalent to `force=true`).
pub fn restore_bundled_kanban_skill(skills_root: &Path, skill_name: &str) -> Result<()> {
    let content = match skill_name {
        KANBAN_WORKER_SKILL_NAME => KANBAN_WORKER_SKILL_MD,
        KANBAN_ORCHESTRATOR_SKILL_NAME => KANBAN_ORCHESTRATOR_SKILL_MD,
        other => return Err(KanbanError::UnknownSkill(other.to_string())),
    };
    let dest = skills_root.join(skill_name).join("SKILL.md");
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!("create_dir_all {:?}: {}", parent, e))
        })?;
    }
    std::fs::write(&dest, content)
        .map_err(|e| KanbanError::Other(anyhow::anyhow!("write {:?}: {}", dest, e)))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Private helper
// ---------------------------------------------------------------------------

fn sync_one(skills_root: &Path, name: &str, content: &str, force: bool) -> Result<SyncAction> {
    let dest = skills_root.join(name).join("SKILL.md");
    if dest.exists() && !force {
        return Ok(SyncAction::Skipped);
    }
    let action = if dest.exists() {
        SyncAction::Restored
    } else {
        SyncAction::Wrote
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            KanbanError::Other(anyhow::anyhow!("create_dir_all {:?}: {}", parent, e))
        })?;
    }
    std::fs::write(&dest, content)
        .map_err(|e| KanbanError::Other(anyhow::anyhow!("write {:?}: {}", dest, e)))?;
    Ok(action)
}
