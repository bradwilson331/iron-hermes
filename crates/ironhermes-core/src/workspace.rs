//! Per-cwd project workspace abstraction (Phase 25.3 D-W-1).
//!
//! Resolved ONCE at session start (frozen-snapshot pattern, mirrors Phase 17/18).
//! Mid-session switching is explicitly deferred (D-W-1; CONTEXT.md "Deferred Ideas").
//!
//! Resolution: walk up from `cwd` looking for `.ironhermes/` or `.hermes/` directory.
//! Returns Some(Workspace) when found; None means the caller uses the global
//! ~/.ironhermes/ fallback via `get_hermes_home()`.
//!
//! Cache-stability: callers MUST resolve once and store as `Arc<Workspace>` —
//! Plan 8 wireup. Re-resolving mid-session breaks the Anthropic prompt cache
//! (Plan 7 places `[Workspace: <root>]` in the durable Identity slot;
//! mutations there invalidate the cache — see RESEARCH.md Pitfall 2).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Per-cwd project workspace (Phase 25.3 D-W-1).
///
/// Six fields per the locked decision:
/// - `root`: resolved directory containing `.hermes/` or `.ironhermes/`
/// - `soul_path`: optional path to SOUL.md
/// - `agents_chain`: AGENTS.md/CLAUDE.md/.hermes.md chain (empty in 25.3 — placeholder)
/// - `memory_dir`: `<root>/<marker>/memory/`
/// - `skills_dir`: `<root>/skills/` — Phase 25.4 Curator skill-emission destination
/// - `tools_config`: optional `<root>/<marker>/tools.yaml` path if it exists
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub root: PathBuf,
    pub soul_path: Option<PathBuf>,
    pub agents_chain: Vec<PathBuf>,
    pub memory_dir: PathBuf,
    /// Phase 25.4 Curator emits `<workspace.skills_dir>/<slug>/SKILL.md` here when
    /// a workspace is active. 25.3 only exposes the field — no writer ships in 25.3.
    pub skills_dir: PathBuf,
    pub tools_config: Option<PathBuf>,
}

impl Workspace {
    /// Construct a Workspace from a known root + marker dir name (e.g., ".ironhermes").
    /// Used by `resolve_from_cwd` and tests; not typically called directly.
    ///
    /// Marker dir argument is the actual on-disk directory NAME found
    /// (".ironhermes" or ".hermes") — the leading dot is included.
    fn from_root_and_marker(root: PathBuf, marker_name: &str) -> Self {
        let marker_dir = root.join(marker_name);
        let soul_path = {
            let top = root.join("SOUL.md");
            let nested = marker_dir.join("SOUL.md");
            if top.exists() {
                Some(top)
            } else if nested.exists() {
                Some(nested)
            } else {
                None
            }
        };
        let memory_dir = marker_dir.join("memory");
        let skills_dir = root.join("skills");
        let tools_config = {
            let p = marker_dir.join("tools.yaml");
            if p.exists() { Some(p) } else { None }
        };
        Self {
            root,
            soul_path,
            agents_chain: Vec::new(), // 25.3: placeholder; chain scanning deferred
            memory_dir,
            skills_dir,
            tools_config,
        }
    }

    /// Phase 25.3-16 CR-03 close-out: single source of truth for the encoding
    /// of `self.root` used at every persistence and filter site (sessions table
    /// `workspace_root` column, /sessions --workspace filter, and the
    /// [Workspace: <root>] system prompt line).
    ///
    /// Uses `to_string_lossy().into_owned()` — lossy U+FFFD substitution for
    /// non-UTF-8 bytes. This matches what `Path::display()` produces, so the
    /// PromptBuilder's existing `format!("{}", root.display())` line agrees by
    /// construction.
    ///
    /// Centralizing the encoding here is the contract INV-25.3-12 enforces — every
    /// site touching workspace_root for SQLite persistence or filter MUST go
    /// through this helper.
    pub fn canonical_root_string(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
}

/// Walk up from `cwd` looking for `.ironhermes/` or `.hermes/` directory.
///
/// Resolution order (D-W-1):
/// 1. Check each ancestor for `.ironhermes/` (preferred) or `.hermes/` (compat) directory
/// 2. If found, return Some(Workspace) built from that ancestor + the marker found
/// 3. If filesystem root is reached without a hit, return None (caller uses
///    `get_hermes_home()` for the global fallback)
///
/// Preference: when both markers exist in the same directory, `.ironhermes/` wins
/// (per PATTERNS.md "NEW: workspace.rs" — IronHermes is the canonical brand).
pub fn resolve_from_cwd(cwd: &Path) -> Option<Workspace> {
    let mut current = cwd.to_path_buf();
    loop {
        let ironhermes = current.join(".ironhermes");
        let hermes = current.join(".hermes");
        if ironhermes.is_dir() {
            return Some(Workspace::from_root_and_marker(current, ".ironhermes"));
        }
        if hermes.is_dir() {
            return Some(Workspace::from_root_and_marker(current, ".hermes"));
        }
        // Walk to parent; pop returns false at filesystem root
        if !current.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolve_returns_none_when_no_marker_anywhere() {
        let dir = tempdir().unwrap();
        // Use a deeply nested cwd inside an empty tempdir tree
        let nested = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(resolve_from_cwd(&nested).is_none());
    }

    #[test]
    fn resolve_finds_ironhermes_at_cwd() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        let ws = resolve_from_cwd(&root).expect("must find marker at cwd");
        assert_eq!(ws.root, root);
        assert_eq!(ws.memory_dir, root.join(".ironhermes").join("memory"));
        assert_eq!(ws.skills_dir, root.join("skills"));
    }

    #[test]
    fn resolve_finds_hermes_at_cwd() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".hermes")).unwrap();
        let ws = resolve_from_cwd(&root).expect("must find .hermes marker at cwd");
        assert_eq!(ws.root, root);
        assert_eq!(ws.memory_dir, root.join(".hermes").join("memory"));
    }

    #[test]
    fn resolve_walks_up_one_parent() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        let nested = root.join("subdir");
        std::fs::create_dir_all(&nested).unwrap();
        let ws = resolve_from_cwd(&nested).expect("must find marker at parent");
        assert_eq!(
            ws.root, root,
            "Workspace.root must be the ancestor with the marker, not the cwd"
        );
    }

    #[test]
    fn resolve_walks_up_multiple_parents() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        let deep = root.join("a").join("b").join("c").join("d");
        std::fs::create_dir_all(&deep).unwrap();
        let ws = resolve_from_cwd(&deep).expect("must find marker by walking up");
        assert_eq!(ws.root, root);
    }

    #[test]
    fn ironhermes_wins_when_both_markers_present() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        std::fs::create_dir_all(root.join(".hermes")).unwrap();
        let ws = resolve_from_cwd(&root).expect("must resolve when both present");
        // .ironhermes is preferred — memory_dir should be under .ironhermes/
        assert_eq!(ws.memory_dir, root.join(".ironhermes").join("memory"));
    }

    #[test]
    fn skills_dir_is_top_level_not_under_marker() {
        // Per D-W-1: <workspace>/skills/ — not <workspace>/.ironhermes/skills/
        // This is what 25.4 Curator will write into; operator-facing path.
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        let ws = resolve_from_cwd(&root).unwrap();
        assert_eq!(ws.skills_dir, root.join("skills"));
        assert_ne!(ws.skills_dir, root.join(".ironhermes").join("skills"));
    }

    #[test]
    fn soul_path_prefers_top_level_over_marker_nested() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        std::fs::write(root.join("SOUL.md"), b"top-level soul").unwrap();
        std::fs::write(root.join(".ironhermes").join("SOUL.md"), b"nested soul").unwrap();
        let ws = resolve_from_cwd(&root).unwrap();
        assert_eq!(ws.soul_path, Some(root.join("SOUL.md")));
    }

    #[test]
    fn soul_path_falls_back_to_marker_nested() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        std::fs::write(root.join(".ironhermes").join("SOUL.md"), b"nested soul").unwrap();
        let ws = resolve_from_cwd(&root).unwrap();
        assert_eq!(ws.soul_path, Some(root.join(".ironhermes").join("SOUL.md")));
    }

    #[test]
    fn soul_path_is_none_when_no_soul_file() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        let ws = resolve_from_cwd(&root).unwrap();
        assert_eq!(ws.soul_path, None);
    }

    #[test]
    fn tools_config_some_only_when_file_exists() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        let ws = resolve_from_cwd(&root).unwrap();
        assert_eq!(ws.tools_config, None);
        std::fs::write(
            root.join(".ironhermes").join("tools.yaml"),
            b"toolsets: {}",
        )
        .unwrap();
        let ws2 = resolve_from_cwd(&root).unwrap();
        assert_eq!(
            ws2.tools_config,
            Some(root.join(".ironhermes").join("tools.yaml"))
        );
    }

    #[test]
    fn agents_chain_is_empty_placeholder_in_25_3() {
        // 25.3 ships agents_chain as an empty Vec — chain scanning deferred to a future plan.
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ironhermes")).unwrap();
        let ws = resolve_from_cwd(&root).unwrap();
        assert!(
            ws.agents_chain.is_empty(),
            "agents_chain placeholder must be empty in Phase 25.3"
        );
    }

    #[test]
    fn walk_up_terminates_at_filesystem_root() {
        // Don't infinite-loop. Use a path guaranteed to have no marker anywhere on the way to /.
        // Pick a real path under a temp-like prefix that we know does not have a workspace marker.
        // (CI environments don't have .hermes/ at /tmp/...; if they do, the test still terminates.)
        let dir = tempdir().unwrap();
        let p = dir.path().to_path_buf();
        // Resolve from the tempdir; on systems where every ancestor lacks a marker, this returns None.
        // The critical property is termination, not the value — None or Some are both acceptable here.
        let _ = resolve_from_cwd(&p);
        // If we reached this line, we did not infinite-loop. PASS.
    }

    #[test]
    fn canonical_root_string_returns_path_for_ascii() {
        let ws = Workspace {
            root: std::path::PathBuf::from("/proj/myapp"),
            soul_path: None,
            agents_chain: vec![],
            memory_dir: std::path::PathBuf::from("/proj/myapp/.ironhermes/memory"),
            skills_dir: std::path::PathBuf::from("/proj/myapp/skills"),
            tools_config: None,
        };
        assert_eq!(ws.canonical_root_string(), "/proj/myapp");
    }

    #[cfg(unix)]
    #[test]
    fn canonical_root_string_lossy_subs_non_utf8_unix() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        // Non-UTF-8 bytes — the regression target for CR-03.
        let bytes = vec![b'/', b't', b'm', b'p', b'/', 0xFFu8, 0xFEu8, 0xFDu8];
        let os = OsString::from_vec(bytes);
        let root = std::path::PathBuf::from(os);
        let ws = Workspace {
            root: root.clone(),
            soul_path: None,
            agents_chain: vec![],
            memory_dir: root.join(".ironhermes/memory"),
            skills_dir: root.join("skills"),
            tools_config: None,
        };
        let canonical = ws.canonical_root_string();
        // canonical_root_string MUST agree with Path::display() so the SQL filter
        // string and the SQLite column never diverge for non-UTF-8 paths.
        assert_eq!(canonical, format!("{}", ws.root.display()));
        assert!(!canonical.is_empty(), "must NOT return empty for non-UTF-8 paths");
        assert!(!canonical.contains('\0'));
    }
}
