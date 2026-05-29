//! Canonical path helpers for the kanban subsystem (D-03 / D-19 / D-31).
//!
//! All paths are rooted at `ironhermes_core::get_hermes_home()` which
//! resolves to `$IRONHERMES_HOME` (when set) or `~/.ironhermes`.

use std::path::{Path, PathBuf};

use crate::error::{KanbanError, Result};

/// `~/.ironhermes/kanban.db` — the default board's SQLite file (D-03).
pub fn kanban_db_path() -> PathBuf {
    ironhermes_core::get_hermes_home().join("kanban.db")
}

/// `~/.ironhermes/kanban/workspaces/` — root for `scratch` workspaces
/// (D-31).
pub fn kanban_workspaces_root() -> PathBuf {
    ironhermes_core::get_hermes_home()
        .join("kanban")
        .join("workspaces")
}

/// `~/.ironhermes/kanban/workspaces/<task_id>/` — scratch workspace for a
/// single task. Dispatcher creates this dir before spawning the worker
/// (Pitfall 8); workers wipe it on `kanban_complete` (D-31).
pub fn kanban_workspace_for(task_id: &str) -> PathBuf {
    kanban_workspaces_root().join(task_id)
}

/// `~/.ironhermes/logs/kanban/` — directory holding per-task worker logs.
pub fn kanban_logs_dir() -> PathBuf {
    ironhermes_core::get_hermes_home()
        .join("logs")
        .join("kanban")
}

/// `~/.ironhermes/logs/kanban/<task_id>.stdout.log` — worker stdout (D-19).
pub fn kanban_log_stdout(task_id: &str) -> PathBuf {
    kanban_logs_dir().join(format!("{task_id}.stdout.log"))
}

/// `~/.ironhermes/logs/kanban/<task_id>.stderr.log` — worker stderr (D-19).
pub fn kanban_log_stderr(task_id: &str) -> PathBuf {
    kanban_logs_dir().join(format!("{task_id}.stderr.log"))
}

/// `~/.ironhermes/skills/` — bundled skills root (D-30).
pub fn kanban_skills_dir() -> PathBuf {
    ironhermes_core::get_hermes_home().join("skills")
}

/// Confused-deputy gate on `dir:<path>` workspace strings (D-31, Pitfall 6).
///
/// When `workspace` starts with the `dir:` prefix, the tail MUST be an
/// absolute filesystem path. Relative tails (e.g. `dir:../foo`, `dir:./bar`,
/// `dir:foo`) are rejected with [`KanbanError::RelativeDirWorkspace`].
///
/// Non-`dir:` workspaces (`scratch`, `worktree`, `worktree:<path>`) are
/// passed through unchanged — their validation lives elsewhere.
pub fn validate_dir_workspace(workspace: &str) -> Result<()> {
    if let Some(tail) = workspace.strip_prefix("dir:") {
        if !Path::new(tail).is_absolute() {
            return Err(KanbanError::RelativeDirWorkspace(workspace.to_string()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_relative_rejected() {
        assert!(matches!(
            validate_dir_workspace("dir:../foo"),
            Err(KanbanError::RelativeDirWorkspace(_))
        ));
        assert!(matches!(
            validate_dir_workspace("dir:./foo"),
            Err(KanbanError::RelativeDirWorkspace(_))
        ));
        assert!(matches!(
            validate_dir_workspace("dir:foo"),
            Err(KanbanError::RelativeDirWorkspace(_))
        ));
    }

    #[test]
    fn dir_absolute_accepted() {
        // Platform-portable absolute path: use the system-root style each
        // host understands. /abs/path is absolute on unix; on Windows the
        // test still runs but `Path::is_absolute()` rejects it — so guard.
        #[cfg(unix)]
        assert!(validate_dir_workspace("dir:/abs/path").is_ok());
    }

    #[test]
    fn non_dir_workspaces_pass_through() {
        assert!(validate_dir_workspace("scratch").is_ok());
        assert!(validate_dir_workspace("worktree").is_ok());
        // worktree:<tail> has its own validation rules (not our concern).
        assert!(validate_dir_workspace("worktree:.worktrees/t_abc").is_ok());
    }

    #[test]
    fn paths_under_hermes_home() {
        // Paths derive from get_hermes_home(); we don't assert the actual
        // home (may be a tmp dir in CI) — just that each helper produces a
        // distinct, non-empty path with the expected leaf.
        assert!(kanban_db_path().ends_with("kanban.db"));
        assert!(kanban_workspaces_root().ends_with("workspaces"));
        assert!(kanban_workspace_for("t_abc").ends_with("t_abc"));
        assert!(kanban_logs_dir().ends_with("kanban"));
        assert!(kanban_log_stdout("t_abc").ends_with("t_abc.stdout.log"));
        assert!(kanban_log_stderr("t_abc").ends_with("t_abc.stderr.log"));
        assert!(kanban_skills_dir().ends_with("skills"));
    }
}
