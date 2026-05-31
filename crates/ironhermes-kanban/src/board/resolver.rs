//! 4-tier board context resolver (D-02).
//!
//! Resolves which kanban board is active based on the following priority chain
//! (highest first):
//!
//! 1. Explicit `--board <slug>` CLI flag
//! 2. `HERMES_KANBAN_BOARD` environment variable
//! 3. `~/.ironhermes/kanban/current` file (single-line slug)
//! 4. Literal `"default"` (legacy single-board path, D-01)
//!
//! This is a pure function — it reads env vars and one optional file but
//! performs NO database I/O.

use super::{BoardContext, BoardSource};
use super::slug::{BoardSlugError, validate_board_slug};

/// Resolve the active board context from the 4-tier precedence chain (D-02).
///
/// Returns `Ok(BoardContext)` on success. Returns `Err(BoardSlugError)` if
/// the resolved raw slug (from flag, env, or file) fails slug validation.
///
/// The `"default"` slug is special-cased: it skips validation (Pitfall 3) and
/// maps directly to the legacy `kanban_db_path()` via `board_db_path_for_slug`
/// (Pitfall 1).
pub fn resolve_board_context(cli_flag: Option<&str>) -> Result<BoardContext, BoardSlugError> {
    // 4-tier if-let chain — explicit source tracking alongside slug resolution.
    let (raw_slug, source) = if let Some(f) = cli_flag {
        (f.to_string(), BoardSource::Flag)
    } else if let Ok(v) = std::env::var("HERMES_KANBAN_BOARD") {
        if v.is_empty() {
            // Empty env var falls through to file/default tiers.
            if let Some(v) = read_current_board_file_opt() {
                (v, BoardSource::File)
            } else {
                ("default".to_string(), BoardSource::Default)
            }
        } else {
            (v, BoardSource::Env)
        }
    } else if let Some(v) = read_current_board_file_opt() {
        (v, BoardSource::File)
    } else {
        ("default".to_string(), BoardSource::Default)
    };

    // Pitfall 3: "default" skips validation to avoid rejecting the built-in board.
    let slug = if raw_slug == "default" {
        raw_slug
    } else {
        validate_board_slug(&raw_slug)?
    };

    // Pitfall 1: "default" maps to legacy path; named boards use board_db_path.
    let db_path = crate::paths::board_db_path_for_slug(&slug);

    Ok(BoardContext { slug, db_path, source })
}

/// Read the `~/.ironhermes/kanban/current` file and return `Some(trimmed_slug)`
/// if it contains a non-empty slug, or `None` otherwise (file absent or empty).
fn read_current_board_file_opt() -> Option<String> {
    match std::fs::read_to_string(crate::paths::current_board_file()) {
        Ok(s) => {
            let t = s.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }
        Err(_) => None, // NotFound silenced; other IO errors also fall through
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialise all env-mutation tests to prevent races when running with
    /// multiple test threads (follows the ENV_LOCK pattern from tools_smoke.rs).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Set IRONHERMES_HOME to a tempdir and return the tempdir + scoped guard.
    fn setup_home() -> (tempfile::TempDir, ScopedEnv) {
        let dir = tempfile::tempdir().unwrap();
        let guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());
        (dir, guard)
    }

    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded (serialised via ENV_LOCK); no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self { key: key.to_string(), prev }
        }

        fn remove(key: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: serialised via ENV_LOCK.
            unsafe { std::env::remove_var(key) };
            Self { key: key.to_string(), prev }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: serialised via ENV_LOCK.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    // -----------------------------------------------------------------------
    // Tier 1: CLI flag overrides everything
    // -----------------------------------------------------------------------

    #[test]
    fn flag_overrides_env() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, _home) = setup_home();
        let _env = ScopedEnv::set("HERMES_KANBAN_BOARD", "env-board");

        let ctx = resolve_board_context(Some("flag-board")).unwrap();
        assert_eq!(ctx.slug, "flag-board");
        assert_eq!(ctx.source, BoardSource::Flag);
    }

    // -----------------------------------------------------------------------
    // Tier 2: Env var overrides file
    // -----------------------------------------------------------------------

    #[test]
    fn env_overrides_file() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (dir, _home) = setup_home();

        // Write a current file
        let kanban_dir = dir.path().join("kanban");
        std::fs::create_dir_all(&kanban_dir).unwrap();
        std::fs::write(kanban_dir.join("current"), "file-board").unwrap();

        let _env = ScopedEnv::set("HERMES_KANBAN_BOARD", "env-board");

        let ctx = resolve_board_context(None).unwrap();
        assert_eq!(ctx.slug, "env-board");
        assert_eq!(ctx.source, BoardSource::Env);
    }

    // -----------------------------------------------------------------------
    // Tier 3: File overrides default
    // -----------------------------------------------------------------------

    #[test]
    fn file_overrides_default() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (dir, _home) = setup_home();
        let _no_env = ScopedEnv::remove("HERMES_KANBAN_BOARD");

        let kanban_dir = dir.path().join("kanban");
        std::fs::create_dir_all(&kanban_dir).unwrap();
        std::fs::write(kanban_dir.join("current"), "gamma\n").unwrap();

        let ctx = resolve_board_context(None).unwrap();
        assert_eq!(ctx.slug, "gamma");
        assert_eq!(ctx.source, BoardSource::File);
    }

    // -----------------------------------------------------------------------
    // Tier 4: Default fallback
    // -----------------------------------------------------------------------

    #[test]
    fn fourth_tier_is_default() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, _home) = setup_home();
        let _no_env = ScopedEnv::remove("HERMES_KANBAN_BOARD");
        // No current file exists in tempdir

        let ctx = resolve_board_context(None).unwrap();
        assert_eq!(ctx.slug, "default");
        assert_eq!(ctx.source, BoardSource::Default);
    }

    // -----------------------------------------------------------------------
    // D-01 + D-02: "default" slug must use legacy db_path (Pitfall 1 + 3)
    // -----------------------------------------------------------------------

    #[test]
    fn default_slug_skips_validation_and_uses_legacy_db_path() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, _home) = setup_home();
        let _no_env = ScopedEnv::remove("HERMES_KANBAN_BOARD");

        let ctx = resolve_board_context(None).unwrap();
        assert_eq!(ctx.slug, "default");
        assert_eq!(ctx.db_path, crate::paths::kanban_db_path());

        // Confirm it does NOT point into boards/
        let s = ctx.db_path.to_string_lossy();
        assert!(
            !s.contains("/boards/"),
            "default db_path must not contain '/boards/', got {:?}",
            ctx.db_path
        );
    }

    // -----------------------------------------------------------------------
    // T-1: path-traversal flag slug returns error
    // -----------------------------------------------------------------------

    #[test]
    fn invalid_flag_slug_returns_path_traversal_error() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, _home) = setup_home();

        let result = resolve_board_context(Some("BAD..PATH"));
        assert!(
            matches!(result, Err(BoardSlugError::PathTraversal)),
            "expected PathTraversal, got {:?}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // Whitespace trimming in current file
    // -----------------------------------------------------------------------

    #[test]
    fn current_file_trims_trailing_newline() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (dir, _home) = setup_home();
        let _no_env = ScopedEnv::remove("HERMES_KANBAN_BOARD");

        let kanban_dir = dir.path().join("kanban");
        std::fs::create_dir_all(&kanban_dir).unwrap();
        std::fs::write(kanban_dir.join("current"), "gamma\n\n").unwrap();

        let ctx = resolve_board_context(None).unwrap();
        assert_eq!(ctx.slug, "gamma");
        assert_eq!(ctx.source, BoardSource::File);
    }

    // -----------------------------------------------------------------------
    // Empty current file falls through to default
    // -----------------------------------------------------------------------

    #[test]
    fn empty_current_file_falls_through_to_default() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (dir, _home) = setup_home();
        let _no_env = ScopedEnv::remove("HERMES_KANBAN_BOARD");

        let kanban_dir = dir.path().join("kanban");
        std::fs::create_dir_all(&kanban_dir).unwrap();
        std::fs::write(kanban_dir.join("current"), "   \n").unwrap();

        let ctx = resolve_board_context(None).unwrap();
        assert_eq!(ctx.slug, "default");
        assert_eq!(ctx.source, BoardSource::Default);
    }
}
