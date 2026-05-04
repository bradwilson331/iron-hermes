use std::path::{Path, PathBuf};

/// Case-sensitive priority chain for project context files.
/// Order: .hermes.md > HERMES.md > AGENTS.md > CLAUDE.md > .cursorrules
/// HERMES.md goes immediately after .hermes.md (both hermes-specific). Per D-18.
pub const CONTEXT_CANDIDATES: &[&str] = &[
    ".hermes.md",
    "HERMES.md",
    "AGENTS.md",
    "CLAUDE.md",
    ".cursorrules",
];

/// Walk upward from `start` looking for a `.git` directory or file (supports worktrees).
/// Stops at $HOME — does not traverse above it.
/// Returns the first directory containing `.git`, or `None` if not found.
/// Per D-01 and D-03.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().map(PathBuf::from);

    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current.clone());
        }

        // Stop at $HOME (do not check $HOME itself for .git, just stop)
        if let Some(ref h) = home {
            if &current == h {
                return None;
            }
        }

        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return None,
        }
    }
}

/// Strip YAML frontmatter from content.
/// If content starts with "---", finds the next line that is exactly "---" (trimmed)
/// and returns everything after it, trimmed of a single leading newline.
/// If no closing marker found, returns input unchanged.
/// Per D-02 and CTX-07.
pub fn strip_yaml_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }

    // Find the end of the opening "---" line
    let after_open = match trimmed.find('\n') {
        Some(pos) => &trimmed[pos + 1..],
        None => return content, // no newline after "---", malformed
    };

    // Walk lines looking for the closing "---" marker.
    // Track byte offset into `after_open` so we can slice without panics.
    let mut offset = 0usize;
    for line in after_open.lines() {
        let line_end = offset + line.len();
        if line.trim() == "---" {
            // Everything after this line (skip the trailing newline if present)
            let rest_start = line_end + 1; // +1 for '\n'
            let rest = if rest_start <= after_open.len() {
                &after_open[rest_start..]
            } else {
                ""
            };
            return rest.trim_start_matches('\n');
        }
        // Advance past line + newline; but after_open may not have a trailing newline
        offset = line_end + 1;
        if offset > after_open.len() {
            break;
        }
    }

    // No closing marker found — return unchanged
    content
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    /// Tests that manipulate HOME must hold this lock to prevent env var races.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // ── find_git_root tests ───────────────────────────────────────────────────

    #[test]
    fn test_find_git_root_found() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        let result = find_git_root(dir.path());
        assert_eq!(result, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn test_find_git_root_walks_up() {
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("sub").join("dir");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(parent.path().join(".git")).unwrap();

        let result = find_git_root(&child);
        assert_eq!(result, Some(parent.path().to_path_buf()));
    }

    #[test]
    fn test_find_git_root_stops_at_home() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let subdir = home.path().join("projects").join("myproject");
        fs::create_dir_all(&subdir).unwrap();
        // No .git anywhere

        unsafe {
            std::env::set_var("HOME", home.path());
        }

        let result = find_git_root(&subdir);

        unsafe {
            std::env::remove_var("HOME");
        }

        assert_eq!(result, None);
    }

    #[test]
    fn test_find_git_root_no_git() {
        let _lock = ENV_MUTEX.lock().unwrap();
        // Use a path that definitely has no .git and set HOME to something unreachable
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("a").join("b");
        fs::create_dir_all(&subdir).unwrap();

        // Set HOME to dir itself — walk from subdir stops at dir, no .git found
        unsafe {
            std::env::set_var("HOME", dir.path());
        }

        let result = find_git_root(&subdir);

        unsafe {
            std::env::remove_var("HOME");
        }

        assert_eq!(result, None);
    }

    // ── strip_yaml_frontmatter tests ─────────────────────────────────────────

    #[test]
    fn test_strip_frontmatter_basic() {
        let input = "---\nkey: val\n---\nbody";
        let result = strip_yaml_frontmatter(input);
        assert_eq!(result, "body");
    }

    #[test]
    fn test_strip_frontmatter_none() {
        let input = "no frontmatter here";
        let result = strip_yaml_frontmatter(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_strip_frontmatter_malformed() {
        let input = "---\nno closing";
        let result = strip_yaml_frontmatter(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_strip_frontmatter_empty_body() {
        let input = "---\nkey: val\n---\n";
        let result = strip_yaml_frontmatter(input);
        assert_eq!(result, "");
    }

    // ── CONTEXT_CANDIDATES tests ──────────────────────────────────────────────

    #[test]
    fn test_context_candidates_case_sensitive() {
        assert!(CONTEXT_CANDIDATES.contains(&".hermes.md"));
        assert!(CONTEXT_CANDIDATES.contains(&"HERMES.md"));
        assert!(CONTEXT_CANDIDATES.contains(&"AGENTS.md"));
        assert!(CONTEXT_CANDIDATES.contains(&"CLAUDE.md"));
        assert!(CONTEXT_CANDIDATES.contains(&".cursorrules"));
        assert_eq!(CONTEXT_CANDIDATES.len(), 5);

        // Must NOT contain lowercase variants
        assert!(!CONTEXT_CANDIDATES.contains(&"agents.md"));
        assert!(!CONTEXT_CANDIDATES.contains(&"claude.md"));
        assert!(!CONTEXT_CANDIDATES.contains(&".HERMES.md"));
    }

    #[test]
    fn test_hermes_md_in_candidates() {
        // HERMES.md must be at index 1 (immediately after .hermes.md). Per D-18.
        assert_eq!(CONTEXT_CANDIDATES[0], ".hermes.md");
        assert_eq!(CONTEXT_CANDIDATES[1], "HERMES.md");
    }
}
