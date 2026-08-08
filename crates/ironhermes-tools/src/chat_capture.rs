//! Deterministic chat-turn deliverable capture (Phase 46.7 D-13/D-14/D-15/D-16/D-25)
//! and structural web-chat file-tool path containment (D-23).
//!
//! Ported from the kanban completion-capture engine
//! (`crates/ironhermes-kanban/src/tools/complete.rs::capture_completion_artifact`
//! / `locate_deliverable` / `find_deliverable_in` / `primary_html_in`) — the
//! exact-name-first, largest-html-fallback logic — extended to also recognize
//! Markdown deliverables (D-14) and composite-keyed on `(session_id, filename)`
//! instead of a bare task id (D-16/D-25).
//!
//! Hard-won lessons baked in from Phase 46.6 rounds 6-8
//! (`.planning/phases/46.6-agent-artifact-webpage/deferred-items.md`):
//!  1. Voluntary tool calls are a coin-flip — capture is a deterministic
//!     chokepoint, not a model-invoked tool.
//!  2. NEVER recompute the scan path from `get_hermes_home()` + an id; always
//!     read the path the caller already resolved (may be redirected).
//!  3. A deliverable's filename is not guaranteed to be the canonical name —
//!     fall back to the largest `*.html` file when no exact match exists.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ironhermes_artifacts::{ArtifactStore, PublishInput, SourceFormat};

/// Deliverable files the deterministic turn-end capture publishes as
/// artifacts, in priority order — the first existing exact-name match wins
/// (D-14). Extends the kanban `CAPTURE_CANDIDATES` (`index.html` only) with
/// `README.md` so a turn that produces documentation-style output is
/// captured too.
pub const CAPTURE_CANDIDATES: &[(&str, SourceFormat)] = &[
    ("index.html", SourceFormat::Html),
    ("README.md", SourceFormat::Markdown),
];

/// A successfully captured chat-turn deliverable.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedArtifact {
    pub artifact_id: String,
    pub title: String,
    pub filename: String,
}

/// Pure root search for a turn's deliverable in `scan_root` (non-recursive).
/// Mirrors `find_deliverable_in`/`primary_html_in` in
/// `ironhermes-kanban::tools::complete`, ported to also recognize the
/// Markdown `CAPTURE_CANDIDATES` entry (D-14) and operate on a single
/// caller-resolved root (a chat turn always has exactly one workspace, unlike
/// kanban's CWD-then-fallback-root search).
pub fn locate_deliverable(scan_root: &Path) -> Option<(PathBuf, SourceFormat)> {
    locate_deliverable_since(scan_root, None)
}

/// [`locate_deliverable`], restricted to files modified at or after `since`.
///
/// Freshness must participate in SELECTION, not be applied to an
/// already-selected candidate. The TUI scans the operator's real CWD (D-22),
/// which is an uncontrolled project directory that routinely already holds a
/// stale `README.md` — an exact-name `CAPTURE_CANDIDATES` entry. Choosing by
/// name first and *then* testing that one candidate's mtime meant the stale
/// README shadowed the `*.html` the turn had just written, and the fallback
/// branch was unreachable: capture returned `None` on every turn and the
/// artifact chip never appeared (Phase 46.7 UAT test 7).
///
/// `since: None` disables the filter entirely (the web-chat path, whose
/// `scan_root` is a per-session workspace rather than a shared CWD).
pub fn locate_deliverable_since(
    scan_root: &Path,
    since: Option<SystemTime>,
) -> Option<(PathBuf, SourceFormat)> {
    // 1. Exact-name candidates (index.html, README.md) — fresh ones only.
    for (name, fmt) in CAPTURE_CANDIDATES {
        let candidate = scan_root.join(name);
        if candidate.is_file() && is_fresh(&candidate, since) {
            return Some((candidate, *fmt));
        }
    }
    // 2. Fallback: the largest *.html file directly under scan_root (round-8
    //    lesson — deliverables are named arbitrarily, e.g. "poem-abc123.html").
    primary_html_in(scan_root, since).map(|path| (path, SourceFormat::Html))
}

/// Whether `path` was modified at or after `since` (`None` → always true).
/// An unreadable mtime is treated as NOT fresh — capture is best-effort and
/// must never publish a file it can't establish provenance for.
fn is_fresh(path: &Path, since: Option<SystemTime>) -> bool {
    let Some(since) = since else { return true };
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|mtime| mtime >= since)
        .unwrap_or(false)
}

/// The filename component of a composite `source_ref`, with the `:` delimiter
/// neutralized (SEC-03).
///
/// `source_ref` is `"<session_id>:<filename>"`, and session ids themselves
/// contain colons, so consumers recover the id by splitting on the LAST colon.
/// That is only unambiguous while the filename holds no colon — but `:` is a
/// legal filename byte on Unix and the deliverable's name is model-chosen. This
/// makes the invariant hold at the point the key is built rather than trusting
/// every future consumer to guess the right split.
pub fn source_ref_leaf(filename: &str) -> String {
    filename.replace(':', "_")
}

/// The largest `*.html` file directly under `root` (non-recursive), ties
/// broken by path for determinism. Ported from
/// `ironhermes-kanban::tools::complete::primary_html_in`, plus the `since`
/// freshness filter (see [`locate_deliverable_since`]).
fn primary_html_in(root: &Path, since: Option<SystemTime>) -> Option<PathBuf> {
    let mut htmls: Vec<(u64, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_html = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"));
            if is_html && path.is_file() && is_fresh(&path, since) {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                Some((size, path))
            } else {
                None
            }
        })
        .collect();
    // Largest first; tie-break by path so the choice is deterministic.
    htmls.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    htmls.into_iter().next().map(|(_, path)| path)
}

/// Deterministically capture and publish a chat turn's deliverable, if any
/// (D-13). Called at the turn chokepoint by the web-chat (Plan 04) and TUI
/// (Plan 06) call sites — NEVER relies on a voluntary `artifact` tool call.
///
/// `scan_root` MUST be the caller's already-resolved session workspace path
/// — this function never recomputes it from `get_hermes_home()` + `session_id`
/// (the round-7 recompute-path bug this plan exists to make structurally
/// impossible).
///
/// Idempotent per `(session_id, filename)` (D-25): a repeat capture of the
/// same filename in the same session versions the existing artifact in place
/// via `latest_for_source("chat", "<session_id>:<filename>")` rather than
/// creating a duplicate. A different filename creates a new artifact.
///
/// `turn_opt_out` suppresses capture for this turn only (D-15) — the caller
/// computes it (see [`detect_turn_opt_out`]) from the user's message text.
///
/// Best-effort character mirrors the kanban capture: a deliverable that fails
/// to read or publish (I/O error, oversized body, store-open failure) is
/// logged and returns `Ok(None)` rather than failing the turn.
pub fn capture_chat_deliverable(
    scan_root: &Path,
    session_id: &str,
    turn_opt_out: bool,
) -> anyhow::Result<Option<CapturedArtifact>> {
    capture_chat_deliverable_since(scan_root, session_id, turn_opt_out, None)
}

/// [`capture_chat_deliverable`], restricted to deliverables modified at or
/// after `since` (the TUI's turn-start gate — see [`locate_deliverable_since`]).
/// `None` preserves the unfiltered behaviour used by the web-chat path.
pub fn capture_chat_deliverable_since(
    scan_root: &Path,
    session_id: &str,
    turn_opt_out: bool,
    since: Option<SystemTime>,
) -> anyhow::Result<Option<CapturedArtifact>> {
    if turn_opt_out {
        return Ok(None); // D-15
    }

    let Some((path, source_format)) = locate_deliverable_since(scan_root, since) else {
        return Ok(None); // nothing to capture
    };

    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "artifact".to_string());

    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                session_id = %session_id, path = %path.display(), error = %e,
                "chat capture: failed to read deliverable"
            );
            return Ok(None);
        }
    };

    // D-16/D-25 composite key: idempotent per (session, filename), not a bare
    // session id — a different filename in the same session is a distinct
    // deliverable and must NOT collide with a prior capture.
    //
    // SEC-03: the key is `:`-delimited and session ids ALREADY contain colons
    // (`agent:main:web:dm:<uuid>`), so consumers must split on the LAST colon
    // to recover the session id (`producing_session_id_from_source_ref`). That
    // only works while the filename is colon-free — and `:` is a legal filename
    // byte on Unix, with the largest-`*.html` fallback accepting whatever the
    // agent named the file. A deliverable named `a:b.html` would otherwise make
    // the gallery backlink resolve to `<session>:a` and spawn a junk session.
    // Neutralize the delimiter in the key only; `filename` itself is unchanged
    // for display and for the artifact body.
    let source_ref = format!("{session_id}:{}", source_ref_leaf(&filename));

    let mut store = match ArtifactStore::open_default() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                session_id = %session_id, error = %e,
                "chat capture: failed to open artifact store"
            );
            return Ok(None);
        }
    };

    let update_id = store
        .latest_for_source("chat", &source_ref)
        .ok()
        .flatten()
        .map(|summary| summary.id);

    let title = filename.clone();
    let profile = ironhermes_core::current_profile();

    match store.publish(PublishInput {
        profile,
        update_id,
        title: Some(title.clone()),
        icon: None,
        // D-16: source_kind/source_ref stamped from the trusted server-side
        // session_id argument — never a model-supplied value (T-46.7-09).
        source_kind: Some("chat".to_string()),
        source_ref: Some(source_ref),
        source_format,
        body,
    }) {
        Ok(artifact_id) => Ok(Some(CapturedArtifact {
            artifact_id,
            title,
            filename,
        })),
        Err(e) => {
            tracing::warn!(
                session_id = %session_id, error = %e,
                "chat capture: failed to publish artifact"
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod capture_tests {
    /// SEC-03 regression: `:` is a legal filename byte on Unix and the
    /// deliverable name is model-chosen, so a file named `a:b.html` would make
    /// the composite key ambiguous — the gallery backlink splits on the LAST
    /// colon and would resolve to `<session>:a`, spawning a junk session.
    /// The delimiter is neutralized where the key is BUILT.
    #[test]
    fn source_ref_leaf_neutralizes_the_key_delimiter() {
        use super::source_ref_leaf;
        assert_eq!(source_ref_leaf("a:b.html"), "a_b.html");
        assert_eq!(source_ref_leaf("index.html"), "index.html");

        // The full key must still split cleanly back to the real session id,
        // even for a hostile filename — this is what the gallery backlink does.
        let session = "agent:main:web:dm:af60c4b6-97c3-49a0-9017-85324469236f";
        let key = format!("{session}:{}", source_ref_leaf("evil:name.html"));
        let (recovered, leaf) = key.rsplit_once(':').unwrap();
        assert_eq!(
            recovered, session,
            "session id must survive a colon-bearing filename"
        );
        assert_eq!(leaf, "evil_name.html");
    }

    use super::*;
    use std::fs;

    /// Redirect `ArtifactStore::open_default()` to a fresh tempdir DB for this
    /// test. SAFETY: `set_var` mutates process-global state and plain
    /// `cargo test` runs tests as threads in ONE process (only nextest gives
    /// process-per-test), so every caller must hold the crate-wide `ENV_LOCK`
    /// for its full duration — capture reads the env lazily via
    /// `open_default()`, not just at setup time.
    fn setup_artifacts_db() -> (tokio::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = crate::ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("artifacts.db");
        unsafe {
            std::env::set_var(ironhermes_artifacts::ARTIFACTS_DB_ENV, &db_path);
        }
        (guard, dir)
    }

    #[test]
    fn captures_index_html_and_stamps_chat_source() {
        let (_env_guard, _db_dir) = setup_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        fs::write(scan_dir.path().join("index.html"), "<html>hi</html>").unwrap();

        let captured = capture_chat_deliverable(scan_dir.path(), "s1", false)
            .unwrap()
            .expect("index.html must be captured");
        assert_eq!(captured.filename, "index.html");

        let store = ArtifactStore::open_default().unwrap();
        let summary = store
            .latest_for_source("chat", "s1:index.html")
            .unwrap()
            .expect("artifact must be findable by composite source_ref");
        assert_eq!(summary.id, captured.artifact_id);
        assert_eq!(summary.source_kind.as_deref(), Some("chat"));
        assert_eq!(summary.source_ref.as_deref(), Some("s1:index.html"));
    }

    #[test]
    fn repeat_capture_same_session_and_filename_versions_in_place() {
        let (_env_guard, _db_dir) = setup_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        fs::write(scan_dir.path().join("index.html"), "<html>v1</html>").unwrap();

        let first = capture_chat_deliverable(scan_dir.path(), "s1", false)
            .unwrap()
            .unwrap();

        fs::write(scan_dir.path().join("index.html"), "<html>v2</html>").unwrap();
        let second = capture_chat_deliverable(scan_dir.path(), "s1", false)
            .unwrap()
            .unwrap();

        assert_eq!(
            first.artifact_id, second.artifact_id,
            "D-25: repeat capture of the same (session, filename) must version in place, not duplicate"
        );

        let store = ArtifactStore::open_default().unwrap();
        let artifacts = store.list_for_profile("default").unwrap();
        assert_eq!(
            artifacts.len(),
            1,
            "exactly one artifact row must exist after two captures of the same (session, filename)"
        );
    }

    #[test]
    fn captures_arbitrary_named_html_via_largest_fallback() {
        let (_env_guard, _db_dir) = setup_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        fs::write(
            scan_dir.path().join("poem-abc123.html"),
            "<html>poem</html>",
        )
        .unwrap();

        let captured = capture_chat_deliverable(scan_dir.path(), "s2", false)
            .unwrap()
            .expect("arbitrary-named html must be captured via the largest-html fallback");
        assert_eq!(captured.filename, "poem-abc123.html");
    }

    #[test]
    fn captures_readme_markdown_when_no_html_present() {
        let (_env_guard, _db_dir) = setup_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        fs::write(scan_dir.path().join("README.md"), "# hello").unwrap();

        let captured = capture_chat_deliverable(scan_dir.path(), "s3", false)
            .unwrap()
            .expect("README.md must be captured as a Markdown artifact (D-14)");
        assert_eq!(captured.filename, "README.md");
    }

    #[test]
    fn turn_opt_out_suppresses_capture() {
        let (_env_guard, _db_dir) = setup_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        fs::write(scan_dir.path().join("index.html"), "<html>hi</html>").unwrap();

        let result = capture_chat_deliverable(scan_dir.path(), "s4", true).unwrap();
        assert!(
            result.is_none(),
            "D-15: turn_opt_out=true must suppress capture"
        );
    }

    #[test]
    fn empty_scan_root_returns_none() {
        let (_env_guard, _db_dir) = setup_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();

        let result = capture_chat_deliverable(scan_dir.path(), "s5", false).unwrap();
        assert!(
            result.is_none(),
            "an empty scan_root has nothing to capture"
        );
    }

    #[test]
    fn different_filename_same_session_creates_a_new_artifact() {
        let (_env_guard, _db_dir) = setup_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        fs::write(scan_dir.path().join("index.html"), "<html>v1</html>").unwrap();
        let first = capture_chat_deliverable(scan_dir.path(), "s6", false)
            .unwrap()
            .unwrap();

        fs::remove_file(scan_dir.path().join("index.html")).unwrap();
        fs::write(scan_dir.path().join("README.md"), "# v2").unwrap();
        let second = capture_chat_deliverable(scan_dir.path(), "s6", false)
            .unwrap()
            .unwrap();

        assert_ne!(
            first.artifact_id, second.artifact_id,
            "a different filename in the same session must create a new artifact, not version the prior one (D-25)"
        );
    }

    #[test]
    fn locate_deliverable_prefers_exact_name_over_fallback() {
        let scan_dir = tempfile::tempdir().unwrap();
        fs::write(scan_dir.path().join("index.html"), "small").unwrap();
        fs::write(
            scan_dir.path().join("bigger.html"),
            "a much bigger deliverable body than the exact-name candidate",
        )
        .unwrap();

        let (path, fmt) = locate_deliverable(scan_dir.path()).expect("must find a deliverable");
        assert_eq!(path.file_name().unwrap(), "index.html");
        assert_eq!(fmt, SourceFormat::Html);
    }

    #[test]
    fn locate_deliverable_none_for_empty_root() {
        let scan_dir = tempfile::tempdir().unwrap();
        assert!(locate_deliverable(scan_dir.path()).is_none());
    }
}

/// Structurally resolve a web-chat file tool's caller-supplied relative path
/// against `workspace_root`, rejecting any path that would escape it (D-23).
///
/// This is the STRUCTURAL guard — not prompt-only — that closes the 46.4
/// `$VAR`-CWD-leak class for the web-chat surface: an absolute `requested`
/// path, a `..`-escaping relative path, or a symlink that would canonicalize
/// outside `workspace_root` are all rejected. This function does not consult
/// any caller-suppliable session id — it operates purely on the
/// already-resolved `workspace_root`, so one session's tools structurally
/// cannot address another session's workspace (T-46.7-07).
pub fn resolve_web_chat_path(workspace_root: &Path, requested: &str) -> anyhow::Result<PathBuf> {
    let requested_path = Path::new(requested);

    if requested_path.is_absolute() {
        anyhow::bail!(
            "path '{requested}' is absolute; web-chat file tools may only address paths inside the session workspace"
        );
    }

    // Lexically normalize the joined path (no fs access — the target may not
    // exist yet for a write). Reject any component sequence that climbs above
    // workspace_root.
    let joined = workspace_root.join(requested_path);
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    anyhow::bail!("path '{requested}' escapes the session workspace");
                }
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other),
        }
    }

    if !normalized.starts_with(workspace_root) {
        anyhow::bail!("path '{requested}' escapes the session workspace");
    }

    // Symlink-escape defense (T-46.7-06 hardening, security-review finding):
    // a canonical check gated on the FULL target existing misses (a) a
    // symlinked intermediate dir inside the workspace pointing outside with
    // a not-yet-existing leaf, and (b) a pre-planted dangling leaf symlink
    // (Path::exists() follows links and reports false). Instead, walk the
    // relative components from the canonicalized workspace root: every
    // physically-existing component that is a symlink must canonicalize to a
    // path still inside the canonical root (a symlink LEAF is refused
    // outright — a write must never follow a planted link), and once a
    // component does not physically exist, the remaining components are
    // plain already-validated segments that cannot re-introduce a link.
    let canonical_root = workspace_root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("failed to canonicalize workspace root: {e}"))?;

    let rel = normalized
        .strip_prefix(workspace_root)
        .expect("normalized starts_with workspace_root checked above");
    let components: Vec<std::path::Component> = rel.components().collect();
    let mut check_pos = canonical_root.clone();
    let mut vanished = false; // deepest existing ancestor passed; rest cannot exist
    for (i, component) in components.iter().enumerate() {
        let std::path::Component::Normal(segment) = component else {
            anyhow::bail!("path '{requested}' contains a non-plain component");
        };
        if segment.to_string_lossy().contains('\\') {
            anyhow::bail!("path '{requested}' contains a backslash segment");
        }
        if vanished {
            continue; // segment validated above; it cannot physically exist yet
        }
        let candidate = check_pos.join(segment);
        match std::fs::symlink_metadata(&candidate) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if i == components.len() - 1 {
                    anyhow::bail!(
                        "path '{requested}' resolves to a symlink leaf; refusing to follow a pre-planted link"
                    );
                }
                let canon = candidate.canonicalize().map_err(|e| {
                    anyhow::anyhow!(
                        "failed to canonicalize symlinked component in '{requested}': {e}"
                    )
                })?;
                if !canon.starts_with(&canonical_root) {
                    anyhow::bail!(
                        "path '{requested}' resolves outside the session workspace (symlink escape)"
                    );
                }
                check_pos = canon;
            }
            Ok(_) => {
                check_pos = candidate;
            }
            Err(_) => {
                vanished = true;
            }
        }
    }

    // Belt-and-braces: if the full target exists, re-assert containment on
    // its canonical form (the walk above already guarantees this; kept as an
    // independent final check that runs AFTER canonicalization).
    if normalized.exists() {
        let canonical_target = normalized
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("failed to canonicalize '{requested}': {e}"))?;
        if !canonical_target.starts_with(&canonical_root) {
            anyhow::bail!(
                "path '{requested}' resolves outside the session workspace (symlink escape)"
            );
        }
    }

    Ok(normalized)
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use std::fs;

    #[test]
    fn plain_relative_path_resolves_inside_workspace_root() {
        let root_dir = tempfile::tempdir().unwrap();
        let resolved = resolve_web_chat_path(root_dir.path(), "out.html").unwrap();
        assert_eq!(resolved, root_dir.path().join("out.html"));
        assert!(resolved.starts_with(root_dir.path()));
    }

    #[test]
    fn nested_relative_path_resolves_inside_workspace_root() {
        let root_dir = tempfile::tempdir().unwrap();
        let resolved = resolve_web_chat_path(root_dir.path(), "sub/dir/out.html").unwrap();
        assert_eq!(resolved, root_dir.path().join("sub/dir/out.html"));
        assert!(resolved.starts_with(root_dir.path()));
    }

    #[test]
    fn dotdot_escape_is_rejected() {
        let root_dir = tempfile::tempdir().unwrap();
        let result = resolve_web_chat_path(root_dir.path(), "../escape.txt");
        assert!(result.is_err(), "a '..' escape must be rejected");
    }

    #[test]
    fn deeply_nested_dotdot_escape_is_rejected() {
        let root_dir = tempfile::tempdir().unwrap();
        let result = resolve_web_chat_path(root_dir.path(), "sub/../../escape.txt");
        assert!(
            result.is_err(),
            "a '..' escape past the workspace root must be rejected even when nested"
        );
    }

    #[test]
    fn absolute_path_is_rejected() {
        let root_dir = tempfile::tempdir().unwrap();
        let result = resolve_web_chat_path(root_dir.path(), "/etc/passwd");
        assert!(
            result.is_err(),
            "an absolute path must be rejected — it must stay inside the session workspace"
        );
    }

    #[test]
    #[cfg(unix)]
    fn symlink_escape_is_rejected() {
        let root_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        fs::write(outside_dir.path().join("secret.txt"), "top secret").unwrap();

        let link_path = root_dir.path().join("escape_link");
        std::os::unix::fs::symlink(outside_dir.path(), &link_path).unwrap();

        let result = resolve_web_chat_path(root_dir.path(), "escape_link/secret.txt");
        assert!(
            result.is_err(),
            "a symlink that canonicalizes outside workspace_root must be rejected"
        );
    }

    #[test]
    fn ok_path_always_has_workspace_root_as_prefix() {
        let root_dir = tempfile::tempdir().unwrap();
        for requested in ["out.html", "sub/out.html", "a/b/c/out.html"] {
            let resolved = resolve_web_chat_path(root_dir.path(), requested).unwrap();
            assert!(
                resolved.starts_with(root_dir.path()),
                "resolved path {resolved:?} for {requested:?} must be inside {:?}",
                root_dir.path()
            );
        }
    }

    /// Security-review regression (T-46.7-06 hardening): an attacker
    /// pre-plants a symlinked directory INSIDE the workspace pointing
    /// OUTSIDE, then requests a NOT-YET-EXISTING leaf under it. A
    /// canonicalize-only-when-the-full-target-exists check misses this —
    /// the leaf doesn't exist at the link target, so no canonicalization
    /// runs, and a later write follows the symlinked intermediate outside
    /// containment.
    #[test]
    #[cfg(unix)]
    fn symlinked_intermediate_dir_with_nonexistent_leaf_is_rejected() {
        let root_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();

        let link_path = root_dir.path().join("exit_dir");
        std::os::unix::fs::symlink(outside_dir.path(), &link_path).unwrap();

        let result = resolve_web_chat_path(root_dir.path(), "exit_dir/new_file.html");
        assert!(
            result.is_err(),
            "a non-existent leaf under a symlinked intermediate dir pointing outside \
             workspace_root must be rejected; got: {result:?}"
        );
    }

    /// Security-review regression (T-46.7-06 hardening): an attacker
    /// pre-plants a symlink AT THE LEAF pointing to a not-yet-existing
    /// OUTSIDE file (dangling). `Path::exists()` follows symlinks and
    /// reports false for a dangling link, so an exists()-gated
    /// canonicalization check misses it — and a later write to the
    /// "new file" follows the planted symlink outside containment.
    #[test]
    #[cfg(unix)]
    fn preplanted_dangling_leaf_symlink_to_outside_is_rejected() {
        let root_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();

        let link_path = root_dir.path().join("planted.html");
        std::os::unix::fs::symlink(outside_dir.path().join("target.html"), &link_path).unwrap();

        let result = resolve_web_chat_path(root_dir.path(), "planted.html");
        assert!(
            result.is_err(),
            "a pre-planted dangling leaf symlink pointing outside workspace_root \
             must be rejected; got: {result:?}"
        );
    }

    /// Companion to the dangling case: a pre-planted leaf symlink whose
    /// outside target ALREADY exists must also be rejected (a symlink leaf
    /// is refused outright — a write must never follow a planted link).
    #[test]
    #[cfg(unix)]
    fn preplanted_leaf_symlink_to_existing_outside_file_is_rejected() {
        let root_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        fs::write(outside_dir.path().join("target.html"), "existing outside").unwrap();

        let link_path = root_dir.path().join("planted.html");
        std::os::unix::fs::symlink(outside_dir.path().join("target.html"), &link_path).unwrap();

        let result = resolve_web_chat_path(root_dir.path(), "planted.html");
        assert!(
            result.is_err(),
            "a pre-planted leaf symlink to an existing outside file must be rejected; got: {result:?}"
        );
    }

    /// An intermediate symlink that canonicalizes INSIDE the workspace is
    /// permitted (containment holds), so legitimate in-workspace layouts
    /// keep working after the hardening.
    #[test]
    #[cfg(unix)]
    fn symlinked_intermediate_dir_inside_workspace_is_allowed() {
        let root_dir = tempfile::tempdir().unwrap();
        let real_dir = root_dir.path().join("real_dir");
        fs::create_dir(&real_dir).unwrap();

        let link_path = root_dir.path().join("alias_dir");
        std::os::unix::fs::symlink(&real_dir, &link_path).unwrap();

        let result = resolve_web_chat_path(root_dir.path(), "alias_dir/out.html");
        assert!(
            result.is_ok(),
            "an intermediate symlink that stays inside workspace_root must be allowed; got: {result:?}"
        );
    }
}

/// Recognize a this-turn natural-language opt-out from safety-net capture
/// (D-15) — e.g. "just show it inline". Heuristic and intentionally
/// conservative (CONTEXT D-15 delegates exact detection to discretion): a
/// false negative (capturing when the user wanted inline) is a minor
/// annoyance, while a false positive (never capturing) breaks the D-13
/// default, so the phrase set stays tight. The Plan 04/06 call sites compute
/// this from the user's message text and pass the result to
/// [`capture_chat_deliverable`]'s `turn_opt_out` parameter.
pub fn detect_turn_opt_out(user_text: &str) -> bool {
    let lower = user_text.to_lowercase();

    let inline_show =
        lower.contains("inline") && (lower.contains("show") || lower.contains("just"));
    let dont_publish = lower.contains("don't publish") || lower.contains("do not publish");
    let dont_save_artifact = (lower.contains("don't save") || lower.contains("do not save"))
        && lower.contains("artifact");
    let no_artifact = lower.contains("no artifact");

    inline_show || dont_publish || dont_save_artifact || no_artifact
}

#[cfg(test)]
mod opt_out_tests {
    use super::*;

    #[test]
    fn just_show_it_inline_opts_out() {
        assert!(detect_turn_opt_out("just show it inline"));
    }

    #[test]
    fn show_inline_dont_publish_opts_out() {
        assert!(detect_turn_opt_out(
            "show me the result inline, don't publish"
        ));
    }

    #[test]
    fn dont_save_as_artifact_opts_out() {
        assert!(detect_turn_opt_out("don't save this as an artifact"));
    }

    #[test]
    fn ordinary_build_request_does_not_opt_out() {
        assert!(!detect_turn_opt_out("build me a dashboard"));
    }

    #[test]
    fn ordinary_question_does_not_opt_out() {
        assert!(!detect_turn_opt_out("what does this function do?"));
    }

    #[test]
    fn detection_is_case_insensitive() {
        assert!(detect_turn_opt_out("JUST SHOW IT INLINE"));
        assert!(detect_turn_opt_out("Don't Save This As An Artifact"));
    }
}
