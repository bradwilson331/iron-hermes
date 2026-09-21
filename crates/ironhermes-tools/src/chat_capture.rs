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
/// broken by path for determinism. Thin wrapper over
/// [`primary_by_extension_in`] so the two implementations cannot drift.
fn primary_html_in(root: &Path, since: Option<SystemTime>) -> Option<PathBuf> {
    primary_by_extension_in(root, &["html"], since)
}

/// The largest fresh file directly under `root` (non-recursive) whose
/// extension case-insensitively matches any entry in `exts`, ties broken by
/// path for determinism. Generalization of the legacy HTML-only scan —
/// [`primary_html_in`] is a thin wrapper over this.
fn primary_by_extension_in(root: &Path, exts: &[&str], since: Option<SystemTime>) -> Option<PathBuf> {
    let mut candidates: Vec<(u64, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let matches_ext = path
                .extension()
                .is_some_and(|ext| exts.iter().any(|e| ext.eq_ignore_ascii_case(e)));
            if matches_ext && path.is_file() && is_fresh(&path, since) {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                Some((size, path))
            } else {
                None
            }
        })
        .collect();
    // Largest first; tie-break by path so the choice is deterministic.
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    candidates.into_iter().next().map(|(_, path)| path)
}

/// The project's workspace marker directory name — the same dot-prefixed
/// directory `profile_workspace_dir` creates
/// (`crates/iron_hermes_ui/src/server/profile_api.rs::profile_workspace_dir`)
/// and `ironhermes_core::workspace::resolve_from_cwd` walks up looking for.
/// Its presence on a scan root is what makes that root ELIGIBLE for the
/// widened producer-capture tiers (D-01/D-04, [`is_eligible_producer_root`]).
pub const WORKSPACE_MARKER_DIR: &str = ".ironhermes";

/// Extensions recognized as a "code" deliverable by the widened producer
/// capture engine (D-01, [`locate_producer_deliverable`] tier 5).
/// Configuration, lock and data-file extensions (`toml`, `json`, `yaml`,
/// `yml`, `lock`, `ini`, `cfg`, `txt`) are deliberately EXCLUDED — they
/// dominate any ordinary directory and are not work products; including them
/// would turn "capture the bot's script" into "publish whatever config file
/// happens to be newest."
pub const CODE_EXTENSIONS: &[&str] = &[
    "py", "rs", "ts", "tsx", "js", "jsx", "sh", "bash", "zsh", "c", "h", "cc", "cpp", "hpp", "go",
    "rb", "java", "kt", "swift", "php", "pl", "lua", "sql", "r", "scala", "cs",
];

/// Whether `scan_root` is eligible for the widened (tiers 2/4/5) producer
/// capture — a host-provisioned workspace (carries [`WORKSPACE_MARKER_DIR`]),
/// or a caller-supplied freshness bound (`since: Some`). An unbounded,
/// marker-less scan of a directory the host did not provision is an
/// uncontrolled directory (an operator's own project), and selecting an
/// arbitrary source file out of it and publishing it is an
/// information-disclosure path, not a capture (T-52.1-02).
fn is_eligible_producer_root(scan_root: &Path, since: Option<SystemTime>) -> bool {
    since.is_some() || scan_root.join(WORKSPACE_MARKER_DIR).is_dir()
}

/// Widened producer-scan deliverable locator shared by all three bot/kanban
/// producer paths (D-01/D-04). Tier order, first match wins, every tier
/// freshness-filtered through [`is_fresh`]:
///  1. always — exact name `index.html`, as Html.
///  2. eligible roots only — exact name `README.md`, as Markdown.
///  3. always — largest `*.html`, as Html.
///  4. eligible roots only — largest `*.md`/`*.markdown`, as Markdown.
///  5. eligible roots only — largest [`CODE_EXTENSIONS`] file, as Code.
///
/// Tiers 1 and 3 reproduce the legacy HTML-only engine's exact behavior on
/// an ineligible root, so widening cannot regress an existing capture.
pub fn locate_producer_deliverable(
    scan_root: &Path,
    since: Option<SystemTime>,
) -> Option<(PathBuf, SourceFormat)> {
    let eligible = is_eligible_producer_root(scan_root, since);

    // Tier 1: always, exact index.html.
    let index_html = scan_root.join("index.html");
    if index_html.is_file() && is_fresh(&index_html, since) {
        return Some((index_html, SourceFormat::Html));
    }

    // Tier 2: eligible roots only, exact README.md.
    if eligible {
        let readme = scan_root.join("README.md");
        if readme.is_file() && is_fresh(&readme, since) {
            return Some((readme, SourceFormat::Markdown));
        }
    }

    // Tier 3: always, largest *.html.
    if let Some(html) = primary_html_in(scan_root, since) {
        return Some((html, SourceFormat::Html));
    }

    if eligible {
        // Tier 4: eligible roots only, largest *.md / *.markdown.
        if let Some(md) = primary_by_extension_in(scan_root, &["md", "markdown"], since) {
            return Some((md, SourceFormat::Markdown));
        }
        // Tier 5: eligible roots only, largest CODE_EXTENSIONS file.
        if let Some(code) = primary_by_extension_in(scan_root, CODE_EXTENSIONS, since) {
            return Some((code, SourceFormat::Code));
        }
    }

    None
}

/// Input to [`publish_producer_deliverable`] — the shared publish path all
/// three bot/kanban producers call (D-01/D-04/D-06).
pub struct ProducerPublish<'a> {
    pub scan_root: &'a Path,
    pub since: Option<SystemTime>,
    pub source_kind: &'a str,
    pub source_ref: &'a str,
    pub title: &'a str,
    /// D-05 semantics: a declared-prose fallback published as Markdown ONLY
    /// when the turn wrote no file. `None`, or blank after trimming,
    /// publishes nothing.
    pub fallback_body: Option<&'a str>,
}

/// Locate and publish a producer's deliverable (D-01/D-04/D-06): the file a
/// completed turn wrote wins (file-wins, D-04, via
/// [`locate_producer_deliverable`]); when no file exists and
/// `fallback_body` is `Some` and non-blank after trimming, that text
/// publishes as Markdown (D-05); when neither, returns `None`. Every
/// failure path (read, store open, publish) is a `tracing::warn!` plus a
/// `None` return — capture must never fail the caller's completion.
pub fn publish_producer_deliverable(input: ProducerPublish<'_>) -> Option<String> {
    let (body, source_format) = match locate_producer_deliverable(input.scan_root, input.since) {
        Some((path, fmt)) => match std::fs::read_to_string(&path) {
            Ok(b) => (b, fmt),
            Err(e) => {
                tracing::warn!(
                    source_kind = %input.source_kind, source_ref = %input.source_ref,
                    path = %path.display(), error = %e,
                    "producer capture: failed to read deliverable"
                );
                return None;
            }
        },
        None => {
            let fallback = input
                .fallback_body
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let text = fallback?;
            (text.to_string(), SourceFormat::Markdown)
        }
    };

    // Operator override (dispatcher-set) wins, else the canonical profile —
    // mirrors `complete.rs::capture_completion_artifact`'s resolution, so
    // new producers' rows stay visible to the gallery's profile filter.
    let profile = std::env::var(ironhermes_core::ARTIFACTS_PROFILE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(ironhermes_core::current_profile);

    let mut store = match ArtifactStore::open_default() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                source_kind = %input.source_kind, source_ref = %input.source_ref, error = %e,
                "producer capture: failed to open artifact store"
            );
            return None;
        }
    };

    // Idempotent per source: a repeat publish versions the existing artifact
    // in place rather than duplicating.
    let update_id = store
        .latest_for_source(input.source_kind, input.source_ref)
        .ok()
        .flatten()
        .map(|summary| summary.id);

    match store.publish(PublishInput {
        profile,
        update_id,
        title: Some(input.title.to_string()),
        icon: None,
        source_kind: Some(input.source_kind.to_string()),
        source_ref: Some(input.source_ref.to_string()),
        source_format,
        body,
    }) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(
                source_kind = %input.source_kind, source_ref = %input.source_ref, error = %e,
                "producer capture: publish failed"
            );
            None
        }
    }
}

#[cfg(test)]
mod producer_capture_tests {
    use super::*;
    use std::fs;

    /// An unmarked root with no `since` selects nothing the legacy
    /// HTML-only engine would not have selected: a README.md, a *.md, or a
    /// *.py in that root is NOT selected (T-52.1-02 regression guard).
    #[test]
    fn unmarked_unbounded_root_selects_only_what_legacy_engine_would() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("README.md"), "# notes").unwrap();
        fs::write(root.path().join("report.py"), "print(1)").unwrap();
        assert!(
            locate_producer_deliverable(root.path(), None).is_none(),
            "an unmarked root with no since bound must not select README.md or report.py"
        );

        fs::write(root.path().join("index.html"), "<h1>x</h1>").unwrap();
        let (path, fmt) = locate_producer_deliverable(root.path(), None)
            .expect("index.html is always selected, marked or not");
        assert_eq!(path.file_name().unwrap(), "index.html");
        assert_eq!(fmt, SourceFormat::Html);
    }

    /// A marked root: a lone report.py is selected as Code; a lone notes.md
    /// is selected as Markdown.
    #[test]
    fn marked_root_selects_code_and_markdown() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(WORKSPACE_MARKER_DIR)).unwrap();
        fs::write(root.path().join("report.py"), "print(1)").unwrap();
        let (path, fmt) =
            locate_producer_deliverable(root.path(), None).expect("marked root selects report.py");
        assert_eq!(path.file_name().unwrap(), "report.py");
        assert_eq!(fmt, SourceFormat::Code);

        let root2 = tempfile::tempdir().unwrap();
        fs::create_dir_all(root2.path().join(WORKSPACE_MARKER_DIR)).unwrap();
        fs::write(root2.path().join("notes.md"), "# notes").unwrap();
        let (path2, fmt2) = locate_producer_deliverable(root2.path(), None)
            .expect("marked root selects notes.md");
        assert_eq!(path2.file_name().unwrap(), "notes.md");
        assert_eq!(fmt2, SourceFormat::Markdown);
    }

    /// An unmarked root with `since: Some(t)`: a file written after `t` is
    /// selected, a file written before `t` is not.
    #[test]
    fn unmarked_root_with_since_selects_only_fresh_files() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("old.py"), "print('old')").unwrap();
        // Sleep to guarantee a distinguishable mtime boundary.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let cutoff = SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(root.path().join("new.py"), "print('new')").unwrap();

        let (path, fmt) = locate_producer_deliverable(root.path(), Some(cutoff))
            .expect("the file written after the cutoff must be selected");
        assert_eq!(path.file_name().unwrap(), "new.py");
        assert_eq!(fmt, SourceFormat::Code);
    }

    /// A Cargo.toml, a config.json and a settings.yaml in a marked root are
    /// never selected — CODE_EXTENSIONS deliberately excludes config/lock/
    /// data extensions.
    #[test]
    fn marked_root_never_selects_config_lock_or_data_files() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(WORKSPACE_MARKER_DIR)).unwrap();
        fs::write(root.path().join("Cargo.toml"), "[package]").unwrap();
        fs::write(root.path().join("config.json"), "{}").unwrap();
        fs::write(root.path().join("settings.yaml"), "a: b").unwrap();
        assert!(
            locate_producer_deliverable(root.path(), None).is_none(),
            "config/lock/data extensions must never be selected as a code deliverable"
        );
    }

    #[test]
    fn code_extensions_excludes_config_lock_and_data_extensions() {
        for banned in ["toml", "json", "yaml", "yml", "lock", "ini", "cfg", "txt"] {
            assert!(
                !CODE_EXTENSIONS.contains(&banned),
                "CODE_EXTENSIONS must not contain {banned}"
            );
        }
    }

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
    fn publish_producer_deliverable_prefers_file_over_fallback() {
        let (_guard, _db_dir) = setup_artifacts_db();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(WORKSPACE_MARKER_DIR)).unwrap();
        fs::write(root.path().join("report.py"), "print('file wins')").unwrap();

        let id = publish_producer_deliverable(ProducerPublish {
            scan_root: root.path(),
            since: None,
            source_kind: "test_kind",
            source_ref: "ref1",
            title: "Title",
            fallback_body: Some("this text must be ignored"),
        })
        .expect("file must be published");

        let store = ArtifactStore::open_default().unwrap();
        let summary = store
            .latest_for_source("test_kind", "ref1")
            .unwrap()
            .expect("artifact recorded");
        assert_eq!(summary.id, id);
    }

    #[test]
    fn publish_producer_deliverable_publishes_fallback_when_no_file() {
        let (_guard, _db_dir) = setup_artifacts_db();
        let root = tempfile::tempdir().unwrap();

        let id = publish_producer_deliverable(ProducerPublish {
            scan_root: root.path(),
            since: None,
            source_kind: "test_kind",
            source_ref: "ref2",
            title: "Title",
            fallback_body: Some("declared result text"),
        })
        .expect("fallback text must be published as Markdown");

        let store = ArtifactStore::open_default().unwrap();
        let summary = store.latest_for_source("test_kind", "ref2").unwrap();
        assert!(summary.is_some());
        drop(id);
    }

    #[test]
    fn publish_producer_deliverable_returns_none_for_no_file_and_no_fallback() {
        let (_guard, _db_dir) = setup_artifacts_db();
        let root = tempfile::tempdir().unwrap();

        assert!(
            publish_producer_deliverable(ProducerPublish {
                scan_root: root.path(),
                since: None,
                source_kind: "test_kind",
                source_ref: "ref3",
                title: "Title",
                fallback_body: None,
            })
            .is_none()
        );

        assert!(
            publish_producer_deliverable(ProducerPublish {
                scan_root: root.path(),
                since: None,
                source_kind: "test_kind",
                source_ref: "ref3b",
                title: "Title",
                fallback_body: Some("   "),
            })
            .is_none(),
            "whitespace-only fallback must not publish"
        );
    }
}

/// Marker prefixed to every pointer-artifact body (D-12), so a pointer row is
/// visibly distinguishable from a full deliverable at a glance and by grep —
/// never confused with an ordinary Markdown deliverable.
pub const POINTER_ARTIFACT_MARKER: &str = "<!-- ironhermes:pointer-artifact -->";

/// D-12's six named pointer fields, minus the artifact title — the title is
/// supplied separately by [`publish_pointer_artifact`]'s caller so it flows
/// through `PublishInput::title` unchanged, exactly as the full-artifact path
/// already does.
pub struct PointerRecord<'a> {
    pub producer: &'a str,
    pub destination: &'a str,
    pub byte_len: usize,
    pub digest_hex: &'a str,
    pub recorded_at: &'a str,
}

/// Render `record` as a small Markdown document, prefixed by
/// [`POINTER_ARTIFACT_MARKER`], so it renders through the existing
/// Markdown-to-HTML path with no new render arm. Never includes the
/// deliverable's own body — a pointer carries metadata about output that
/// lives elsewhere.
///
/// **CR-01:** `producer` and `destination` are attacker-influenceable —
/// `destination` in particular is the filename component of a path the
/// PRODUCING AGENT chose, i.e. LLM-steerable via prompt injection. Because
/// this body is published as `SourceFormat::Markdown` and pulldown-cmark
/// passes inline literal HTML straight through unsanitized, both fields are
/// escaped with the renderer's own `escape_html_text` (ampersand-first
/// ordering) before interpolation, so neither can smuggle a live tag into
/// the rendered pointer artifact. `byte_len`/`digest_hex`/`recorded_at` are
/// computed by this module, never attacker-controlled, so they need no
/// escaping.
pub fn pointer_body(record: &PointerRecord<'_>) -> String {
    let producer = ironhermes_artifacts::render::escape_html_text(record.producer);
    let destination = ironhermes_artifacts::render::escape_html_text(record.destination);
    format!(
        "{marker}\n\n\
         The operator opted out of publishing this deliverable's body, or directed it \
         elsewhere, so only a pointer to it is recorded here — the deliverable itself \
         was not stored.\n\n\
         - **Producer:** {producer}\n\
         - **Destination:** {destination}\n\
         - **Size:** {byte_len} bytes\n\
         - **SHA-256:** {digest_hex}\n\
         - **Recorded at:** {recorded_at}\n",
        marker = POINTER_ARTIFACT_MARKER,
        byte_len = record.byte_len,
        digest_hex = record.digest_hex,
        recorded_at = record.recorded_at,
    )
}

/// Hex-encode the SHA-256 digest of `bytes`, using the workspace-pinned `sha2`
/// crate rather than a hand-rolled checksum. Kept as a standalone helper so a
/// unit test can assert its output against an independently known digest.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Publish a demoted "pointer" artifact for a producer's deliverable (D-12):
/// when the operator opted out of a full-body publish, or directed the
/// deliverable elsewhere, this records title, producer, destination, byte
/// size, a real SHA-256 digest and a timestamp — never the deliverable's own
/// bytes. Keyed by the SAME `source_kind`/`source_ref` the full artifact
/// would have used, so a pointer versions in place exactly like
/// [`publish_producer_deliverable`] does (shared idempotency key).
///
/// **Deliberate divergence from Phase 46.7's D-15.** The web-chat opt-out
/// path (`capture_chat_deliverable`/`capture_chat_deliverable_since`,
/// gated by [`detect_turn_opt_out`]) captures nothing on opt-out and is left
/// completely unmodified and uncalled by this function — 52.1's D-12 is
/// deliberately stricter for the bot/kanban/team producer paths ONLY. See
/// `52.1-CONTEXT.md`'s D-12 entry for the reasoning; a downstream agent must
/// not "reconcile" the two by making chat publish pointers too.
///
/// Every failure path (store open, publish) is a `tracing::warn!` plus a
/// `None` return — pointer publishing is best-effort, matching every other
/// capture fn in this module.
pub fn publish_pointer_artifact(
    source_kind: &str,
    source_ref: &str,
    title: &str,
    producer: &str,
    destination: &str,
    bytes: &[u8],
) -> Option<String> {
    let digest_hex = sha256_hex(bytes);
    let recorded_at = chrono::Utc::now().to_rfc3339();

    let record = PointerRecord {
        producer,
        destination,
        byte_len: bytes.len(),
        digest_hex: &digest_hex,
        recorded_at: &recorded_at,
    };
    let body = pointer_body(&record);

    // Operator override (dispatcher-set) wins, else the canonical profile —
    // mirrors `publish_producer_deliverable`'s resolution, so a pointer row
    // stays visible to the gallery's profile filter exactly like the full
    // artifact it demotes would have been.
    let profile = std::env::var(ironhermes_core::ARTIFACTS_PROFILE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(ironhermes_core::current_profile);

    let mut store = match ArtifactStore::open_default() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                source_kind = %source_kind, source_ref = %source_ref, error = %e,
                "pointer capture: failed to open artifact store"
            );
            return None;
        }
    };

    // Same idempotency key the full artifact would have used — a pointer
    // occupies the same slot rather than a distinct one.
    let update_id = store
        .latest_for_source(source_kind, source_ref)
        .ok()
        .flatten()
        .map(|summary| summary.id);

    match store.publish(PublishInput {
        profile,
        update_id,
        title: Some(title.to_string()),
        icon: None,
        source_kind: Some(source_kind.to_string()),
        source_ref: Some(source_ref.to_string()),
        source_format: SourceFormat::Markdown,
        body,
    }) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(
                source_kind = %source_kind, source_ref = %source_ref, error = %e,
                "pointer capture: publish failed"
            );
            None
        }
    }
}

#[cfg(test)]
mod pointer_capture_tests {
    use super::*;

    fn setup_artifacts_db() -> (tokio::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = crate::ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("artifacts.db");
        unsafe {
            std::env::set_var(ironhermes_artifacts::ARTIFACTS_DB_ENV, &db_path);
        }
        (guard, dir)
    }

    /// NIST SHA-256 test vector for `"abc"` — an independently known digest,
    /// not derived from the implementation under test.
    #[test]
    fn sha256_hex_matches_independently_known_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn pointer_body_carries_marker_and_all_six_fields_but_not_deliverable_text() {
        let record = PointerRecord {
            producer: "alice-bot",
            destination: "/workspace/report.py",
            byte_len: 42,
            digest_hex: "deadbeef",
            recorded_at: "2026-09-19T00:00:00Z",
        };
        let body = pointer_body(&record);
        assert!(body.starts_with(POINTER_ARTIFACT_MARKER));
        assert!(body.contains("alice-bot"));
        assert!(body.contains("/workspace/report.py"));
        assert!(body.contains("42 bytes"));
        assert!(body.contains("deadbeef"));
        assert!(body.contains("2026-09-19T00:00:00Z"));
        assert!(
            !body.contains("THE DELIVERABLE'S SECRET BODY"),
            "a pointer body must never contain deliverable text"
        );
    }

    #[test]
    fn publish_pointer_artifact_writes_marked_body_without_deliverable_text() {
        let (_guard, _db_dir) = setup_artifacts_db();
        let deliverable_text = b"THIS IS THE DELIVERABLE'S OWN SECRET TEXT, NEVER STORED";

        let id = publish_pointer_artifact(
            "test_kind",
            "ptr-ref-1",
            "Title",
            "bot-producer",
            "/workspace/out.py",
            deliverable_text,
        )
        .expect("pointer publish must succeed");

        let store = ArtifactStore::open_default().unwrap();
        let (fmt, body) = store.load_latest_source(&id).unwrap();
        assert_eq!(fmt, SourceFormat::Markdown, "pointer body is markdown-wire");
        assert!(body.starts_with(POINTER_ARTIFACT_MARKER));
        assert!(body.contains("bot-producer"));
        assert!(body.contains("/workspace/out.py"));
        assert!(
            !body.contains("SECRET TEXT"),
            "the deliverable's own text must never appear in the stored pointer body"
        );

        let independently_computed = sha256_hex(deliverable_text);
        assert!(
            body.contains(&independently_computed),
            "the stored digest must match an independently computed digest of the same bytes"
        );
    }

    #[test]
    fn publish_pointer_artifact_twice_same_key_versions_not_duplicates() {
        let (_guard, _db_dir) = setup_artifacts_db();

        let first = publish_pointer_artifact(
            "test_kind",
            "ptr-ref-2",
            "Title",
            "bot-producer",
            "/workspace/out.py",
            b"first bytes",
        )
        .expect("first pointer publish must succeed");

        let second = publish_pointer_artifact(
            "test_kind",
            "ptr-ref-2",
            "Title",
            "bot-producer",
            "/workspace/out.py",
            b"second bytes, different content",
        )
        .expect("second pointer publish must succeed");

        assert_eq!(
            first, second,
            "a repeat pointer publish under the same source kind/ref must version in place"
        );

        let store = ArtifactStore::open_default().unwrap();
        let artifacts = store.list_for_profile(&ironhermes_core::current_profile()).unwrap();
        assert_eq!(
            artifacts.len(),
            1,
            "exactly one artifact row must exist after two pointer publishes under the same key"
        );
    }

    #[test]
    fn publish_pointer_artifact_for_declared_prose_records_prose_bytes_and_digest() {
        let (_guard, _db_dir) = setup_artifacts_db();
        let prose = b"the declared result text, published as a pointer instead of a file";

        let id = publish_pointer_artifact(
            "test_kind",
            "ptr-ref-prose",
            "Title",
            "bot-producer",
            "declared prose (no file)",
            prose,
        )
        .expect("prose pointer publish must succeed");

        let store = ArtifactStore::open_default().unwrap();
        let (_fmt, body) = store.load_latest_source(&id).unwrap();
        assert!(body.contains("declared prose (no file)"));
        assert!(body.contains(&prose.len().to_string()));
        assert!(body.contains(&sha256_hex(prose)));
        assert!(
            !body.contains("the declared result text"),
            "the prose's own text must not appear in the pointer body"
        );
    }

    /// CR-01 regression: `producer` and `destination` are attacker-influenceable
    /// (an agent chooses its own output filename), and the pointer body is
    /// published as `SourceFormat::Markdown` — rendered through the real
    /// `ironhermes_artifacts::render::render` path, an inline HTML tag in
    /// either field must never survive as a live element. Asserting only on
    /// `pointer_body`'s raw string would be the "verifies its own assumption"
    /// pattern this project keeps getting burned by, so this renders through
    /// the actual Markdown-to-HTML pipeline the pointer artifact is served
    /// through in production.
    #[test]
    fn pointer_body_with_metacharacters_in_producer_and_destination_renders_no_live_element() {
        let record = PointerRecord {
            producer: "<img src=x onerror=alert('producer-pwn')>",
            destination: "<script>alert('destination-pwn')</script>report.html",
            byte_len: 7,
            digest_hex: "deadbeef",
            recorded_at: "2026-09-19T00:00:00Z",
        };
        let body = pointer_body(&record);
        let rendered = ironhermes_artifacts::render::render(SourceFormat::Markdown, &body);

        assert!(
            !rendered.contains("<img"),
            "an <img> tag from the producer field must not survive rendering: {rendered}"
        );
        assert!(
            !rendered.contains("<script>"),
            "a <script> tag from the destination field must not survive rendering: {rendered}"
        );
        assert!(
            rendered.contains("&lt;img"),
            "the producer's angle bracket must be escaped, not merely absent: {rendered}"
        );
        assert!(
            rendered.contains("&lt;script&gt;"),
            "the destination's script tag must be escaped, not merely absent: {rendered}"
        );
    }

    #[test]
    fn publish_pointer_artifact_store_open_failure_returns_none_not_panic() {
        let _guard = crate::ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let bogus_db_path = dir.path().join("not-a-file");
        std::fs::create_dir_all(&bogus_db_path).unwrap();
        unsafe {
            std::env::set_var(ironhermes_artifacts::ARTIFACTS_DB_ENV, &bogus_db_path);
        }

        let result = publish_pointer_artifact(
            "test_kind",
            "ptr-ref-fail",
            "Title",
            "bot-producer",
            "/workspace/out.py",
            b"bytes",
        );
        assert!(
            result.is_none(),
            "a store that cannot open must yield None, not a panic"
        );

        unsafe {
            std::env::remove_var(ironhermes_artifacts::ARTIFACTS_DB_ENV);
        }
    }
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
