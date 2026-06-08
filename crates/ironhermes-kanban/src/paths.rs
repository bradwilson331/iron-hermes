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
    if let Some(tail) = workspace.strip_prefix("dir:")
        && !Path::new(tail).is_absolute()
    {
        return Err(KanbanError::RelativeDirWorkspace(workspace.to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-board path helpers (Phase 36.3.7.9, D-01)
// ---------------------------------------------------------------------------

/// `~/.ironhermes/kanban/boards/` — root directory for all named boards.
pub fn boards_root() -> PathBuf {
    ironhermes_core::get_hermes_home()
        .join("kanban")
        .join("boards")
}

/// `~/.ironhermes/kanban/boards/<slug>/` — directory for a single named board.
pub fn board_dir(slug: &str) -> PathBuf {
    boards_root().join(slug)
}

/// `~/.ironhermes/kanban/boards/<slug>/kanban.db` — SQLite file for a named board.
pub fn board_db_path(slug: &str) -> PathBuf {
    board_dir(slug).join("kanban.db")
}

/// Returns the correct `db_path` for a slug, honouring the D-01 "default" special case.
///
/// - `"default"` → `kanban_db_path()` (legacy `~/.ironhermes/kanban.db`, D-01 back-compat)
/// - anything else → `board_db_path(slug)`
///
/// The explicit branch (not a helper call) is the documented invariant — do not refactor.
pub fn board_db_path_for_slug(slug: &str) -> PathBuf {
    if slug == "default" {
        kanban_db_path()
    } else {
        board_db_path(slug)
    }
}

/// `~/.ironhermes/kanban/current` — slug-only single-line file written by `boards switch`.
pub fn current_board_file() -> PathBuf {
    ironhermes_core::get_hermes_home()
        .join("kanban")
        .join("current")
}

/// `~/.ironhermes/kanban/notifier.toml` — `subscribe_boards` config (D-03).
pub fn notifier_config_path() -> PathBuf {
    ironhermes_core::get_hermes_home()
        .join("kanban")
        .join("notifier.toml")
}

/// `~/.ironhermes/kanban/boards/_archived/` — archive staging directory (D-07).
pub fn archive_dir() -> PathBuf {
    boards_root().join("_archived")
}

/// List all named board slugs on disk. Does NOT include `"default"` (callers prepend
/// it explicitly when needed). Skips entries whose name starts with `'_'` (e.g.
/// `_archived`) and skips non-directory entries. Returns slugs sorted alphabetically.
///
/// Returns `Ok(vec![])` when `boards_root()` does not yet exist.
pub fn list_boards() -> std::io::Result<Vec<String>> {
    let root = boards_root();
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut slugs = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('_') {
            continue; // skip _archived and any other _-prefixed entries
        }
        if entry.file_type()?.is_dir() {
            slugs.push(name);
        }
    }
    slugs.sort();
    Ok(slugs)
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

    // -----------------------------------------------------------------------
    // Multi-board path helper tests (Phase 36.3.7.9)
    // -----------------------------------------------------------------------

    #[test]
    fn boards_root_ends_with_boards() {
        let p = boards_root();
        assert!(
            p.ends_with("boards"),
            "boards_root should end with 'boards', got {:?}",
            p
        );
    }

    #[test]
    fn board_dir_appends_slug() {
        let p = board_dir("alpha");
        assert!(
            p.ends_with("alpha"),
            "board_dir('alpha') should end with 'alpha', got {:?}",
            p
        );
        // Must be under boards/
        let parent = p.parent().unwrap();
        assert!(
            parent.ends_with("boards"),
            "board_dir parent should end with 'boards', got {:?}",
            parent
        );
    }

    #[test]
    fn board_db_path_appends_kanban_db() {
        let p = board_db_path("alpha");
        assert!(
            p.ends_with("kanban.db"),
            "board_db_path should end with 'kanban.db', got {:?}",
            p
        );
        let parent = p.parent().unwrap();
        assert!(
            parent.ends_with("alpha"),
            "board_db_path parent should be 'alpha', got {:?}",
            parent
        );
    }

    #[test]
    fn board_db_path_for_slug_default_is_legacy() {
        let p = board_db_path_for_slug("default");
        // Must end with kanban.db
        assert!(
            p.ends_with("kanban.db"),
            "default slug should end with kanban.db, got {:?}",
            p
        );
        // Must NOT contain /boards/ anywhere (D-01 back-compat)
        let s = p.to_string_lossy();
        assert!(
            !s.contains("/boards/"),
            "default slug path must NOT contain '/boards/', got {:?}",
            p
        );
    }

    #[test]
    fn board_db_path_for_slug_named_uses_boards_layout() {
        let p = board_db_path_for_slug("alpha");
        let s = p.to_string_lossy();
        assert!(
            s.contains("/boards/alpha/kanban.db") || s.ends_with("boards/alpha/kanban.db"),
            "named slug path should contain '/boards/alpha/kanban.db', got {:?}",
            p
        );
    }

    #[test]
    fn current_board_file_ends_with_current() {
        let p = current_board_file();
        assert!(
            p.ends_with("current"),
            "current_board_file should end with 'current', got {:?}",
            p
        );
    }

    #[test]
    fn notifier_config_path_ends_with_notifier_toml() {
        let p = notifier_config_path();
        assert!(
            p.ends_with("notifier.toml"),
            "notifier_config_path should end with 'notifier.toml', got {:?}",
            p
        );
    }

    #[test]
    fn archive_dir_ends_with_archived() {
        let p = archive_dir();
        assert!(
            p.ends_with("_archived"),
            "archive_dir should end with '_archived', got {:?}",
            p
        );
        // Must be under boards/
        let parent = p.parent().unwrap();
        assert!(
            parent.ends_with("boards"),
            "archive_dir parent should end with 'boards', got {:?}",
            parent
        );
    }

    #[test]
    fn list_boards_returns_empty_when_root_absent() {
        // Use a tempdir as IRONHERMES_HOME so boards_root() doesn't exist.
        let dir = tempfile::tempdir().unwrap();
        // Temporarily override IRONHERMES_HOME
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let result = list_boards().expect("list_boards should return Ok when root absent");
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn list_boards_skips_underscore_prefix_and_files() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let boards = dir.path().join("kanban").join("boards");

        // Create: _archived/ (dir, should be skipped), alpha/ (dir, included),
        // beta/ (dir, included), .hidden (file, skipped), readme.txt (file, skipped)
        fs::create_dir_all(boards.join("_archived")).unwrap();
        fs::create_dir_all(boards.join("alpha")).unwrap();
        fs::create_dir_all(boards.join("beta")).unwrap();
        fs::write(boards.join("readme.txt"), b"").unwrap();

        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        let mut result = list_boards().expect("list_boards should succeed");
        result.sort();
        assert_eq!(result, vec!["alpha".to_string(), "beta".to_string()]);
    }

    /// RAII guard that sets an env var and restores the previous value on drop.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }
}
