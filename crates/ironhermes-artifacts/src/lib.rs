//! Per-profile SQLite artifact store for IronHermes agent-authored webpages.
//!
//! Mirrors `ironhermes-state`'s `StateStore` idiom (schema/migration const
//! strings, thiserror-based error type, `new`/`open_default`/`init_schema`)
//! but owns its own sibling `artifacts.db` file — never shares `StateStore`'s
//! connection or mutex (RESEARCH Pitfall 5).

use std::path::{Path, PathBuf};

use ironhermes_core::get_hermes_home;
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use tracing::debug;

pub mod render;
pub use render::SourceFormat;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("rendered artifact exceeds 16 MiB limit")]
    TooLarge,

    #[error("artifact not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T, E = ArtifactError> = std::result::Result<T, E>;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

const SCHEMA_VERSION: i64 = 2;

/// Post-render byte-size cap enforced by `publish` before any row is
/// written (RESEARCH Security Domain V5 / D-05).
const MAX_RENDERED_BYTES: usize = 16 * 1024 * 1024;

const SCHEMA_SQL: &str = "
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts (
    id                TEXT PRIMARY KEY,
    profile           TEXT NOT NULL,
    title             TEXT NOT NULL,
    icon              TEXT,
    source_kind       TEXT,
    source_ref        TEXT,
    source_format     TEXT NOT NULL,
    created_at        REAL NOT NULL,
    updated_at        REAL NOT NULL,
    archived          INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS artifact_versions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    artifact_id       TEXT NOT NULL REFERENCES artifacts(id),
    version_no        INTEGER NOT NULL,
    body              TEXT NOT NULL,
    rendered_bytes    INTEGER NOT NULL,
    created_at        REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_artifacts_profile ON artifacts(profile, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_artifact_versions_artifact ON artifact_versions(artifact_id, version_no DESC);
";

// ---------------------------------------------------------------------------
// Data structs
// ---------------------------------------------------------------------------

/// Input for [`ArtifactStore::publish`].
///
/// When `update_id` is `Some` and refers to a known artifact, `publish`
/// appends a new `artifact_versions` row in place (D-05 append-only). When
/// `update_id` is `None` (or refers to an unknown id), a fresh artifact is
/// created with a new uuid.
#[derive(Debug, Clone)]
pub struct PublishInput {
    pub profile: String,
    pub update_id: Option<String>,
    pub title: Option<String>,
    pub icon: Option<String>,
    pub source_kind: Option<String>,
    pub source_ref: Option<String>,
    pub source_format: SourceFormat,
    pub body: String,
}

/// One row of a per-profile listing query (`ArtifactStore::list_for_profile`).
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactSummary {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub source_kind: Option<String>,
    pub source_ref: Option<String>,
    pub updated_at: f64,
    pub archived: bool,
}

// ---------------------------------------------------------------------------
// ArtifactStore
// ---------------------------------------------------------------------------

pub struct ArtifactStore {
    conn: Connection,
}

impl ArtifactStore {
    /// Open (or create) a database at the given path.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ArtifactError::Other(anyhow::anyhow!(
                    "create artifacts directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let conn = Connection::open(path).map_err(|e| {
            ArtifactError::Other(anyhow::anyhow!(
                "open SQLite database at {}: {e}",
                path.display()
            ))
        })?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        let mut store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Open the default database — the [`ARTIFACTS_DB_ENV`] override when set,
    /// else `get_hermes_home().join("artifacts.db")`. See [`default_db_path`].
    pub fn open_default() -> Result<Self> {
        let db_path = default_db_path();
        debug!("opening artifact store at {}", db_path.display());
        Self::new(db_path)
    }

    // -----------------------------------------------------------------------
    // Schema management
    // -----------------------------------------------------------------------

    fn init_schema(&mut self) -> Result<()> {
        // Idempotent: every DDL statement uses CREATE IF NOT EXISTS.
        self.conn.execute_batch(SCHEMA_SQL)?;

        let current: Option<i64> = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?;

        match current {
            None => {
                self.conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
            }
            Some(v) => {
                self.run_migrations(v)?;
            }
        }

        Ok(())
    }

    /// Idempotent forward migrations. v1→v2 (Phase 46.6): add the `archived`
    /// column that backs gallery archive/unarchive. The `pragma_table_info`
    /// guard keeps it safe to re-run even if a prior partial run added it.
    fn run_migrations(&mut self, current: i64) -> Result<()> {
        if current < 2 {
            let has_archived: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('artifacts') WHERE name = 'archived'",
                [],
                |r| r.get(0),
            )?;
            if has_archived == 0 {
                self.conn.execute(
                    "ALTER TABLE artifacts ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            self.conn
                .execute("UPDATE schema_version SET version = 2", [])?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Publish (create-or-update-by-id, append-only version history)
    // -----------------------------------------------------------------------

    /// Publish an artifact: create-or-update-by-id with append-only version
    /// history (D-05). Returns the artifact's id (new uuid on create, the
    /// same id on update).
    ///
    /// The rendered-size cap is enforced BEFORE any row is written (RESEARCH
    /// Security Domain V5): an oversized publish leaves both tables
    /// untouched.
    pub fn publish(&mut self, input: PublishInput) -> Result<String> {
        let rendered = render::render(input.source_format, &input.body);
        let rendered_bytes = rendered.len();
        if rendered_bytes > MAX_RENDERED_BYTES {
            return Err(ArtifactError::TooLarge);
        }
        let now = unix_now();

        // Resolve whether update_id refers to a known artifact — params!-bound,
        // never format!-interpolated (T-46.6-01 mitigation).
        let existing_id: Option<String> = if let Some(ref uid) = input.update_id {
            self.conn
                .query_row(
                    "SELECT id FROM artifacts WHERE id = ?1",
                    params![uid],
                    |r| r.get(0),
                )
                .optional()?
        } else {
            None
        };

        if let Some(id) = existing_id {
            let next_version: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(version_no), 0) + 1 FROM artifact_versions WHERE artifact_id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            // Append-only: INSERT a new version row, never UPDATE/DELETE a
            // prior one (D-05 invariant).
            self.conn.execute(
                "INSERT INTO artifact_versions (artifact_id, version_no, body, rendered_bytes, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, next_version, input.body, rendered_bytes as i64, now],
            )?;

            // Always refresh source_format + updated_at so load_latest_html
            // renders the latest version's body with the format it was published
            // in. Otherwise an artifact created as Markdown but later versioned
            // with HTML — e.g. the deterministic kanban capture versioning a
            // tool-published md artifact for the same task — keeps the stale
            // `md` format and renders the raw HTML through the Markdown renderer.
            // Only overwrite title/icon when explicitly supplied so a republish
            // that omits them doesn't clobber the prior value with NULL.
            self.conn.execute(
                "UPDATE artifacts SET source_format = ?1, updated_at = ?2 WHERE id = ?3",
                params![input.source_format.as_str(), now, id],
            )?;
            if let Some(ref title) = input.title {
                self.conn.execute(
                    "UPDATE artifacts SET title = ?1 WHERE id = ?2",
                    params![title, id],
                )?;
            }
            if let Some(ref icon) = input.icon {
                self.conn.execute(
                    "UPDATE artifacts SET icon = ?1 WHERE id = ?2",
                    params![icon, id],
                )?;
            }

            Ok(id)
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            let title = input
                .title
                .clone()
                .unwrap_or_else(|| "Untitled artifact".to_string());
            self.conn.execute(
                "INSERT INTO artifacts \
                 (id, profile, title, icon, source_kind, source_ref, source_format, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    input.profile,
                    title,
                    input.icon,
                    input.source_kind,
                    input.source_ref,
                    input.source_format.as_str(),
                    now,
                    now,
                ],
            )?;
            self.conn.execute(
                "INSERT INTO artifact_versions (artifact_id, version_no, body, rendered_bytes, created_at) \
                 VALUES (?1, 1, ?2, ?3, ?4)",
                params![id, input.body, rendered_bytes as i64, now],
            )?;
            Ok(id)
        }
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// List artifacts for `profile` only, newest `updated_at` first (D-04),
    /// excluding archived artifacts (the gallery default).
    pub fn list_for_profile(&self, profile: &str) -> Result<Vec<ArtifactSummary>> {
        self.list_for_profile_filtered(profile, false)
    }

    /// List artifacts for `profile`, newest first. When `include_archived` is
    /// false, archived artifacts are omitted; when true they are included (the
    /// gallery "show archived" toggle).
    pub fn list_for_profile_filtered(
        &self,
        profile: &str,
        include_archived: bool,
    ) -> Result<Vec<ArtifactSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, icon, source_kind, source_ref, updated_at, archived \
             FROM artifacts WHERE profile = ?1 AND (?2 OR archived = 0) \
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![profile, include_archived], |r| {
            Ok(ArtifactSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                icon: r.get(2)?,
                source_kind: r.get(3)?,
                source_ref: r.get(4)?,
                updated_at: r.get(5)?,
                archived: r.get::<_, i64>(6)? != 0,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Return the rendered HTML for the highest `version_no` row of `id`.
    pub fn load_latest_html(&self, id: &str) -> Result<String> {
        let source_format_str: Option<String> = self
            .conn
            .query_row(
                "SELECT source_format FROM artifacts WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        let source_format_str =
            source_format_str.ok_or_else(|| ArtifactError::NotFound(id.to_string()))?;
        let source_format = SourceFormat::parse(&source_format_str).ok_or_else(|| {
            ArtifactError::Other(anyhow::anyhow!(
                "unknown source_format: {source_format_str}"
            ))
        })?;

        let body: Option<String> = self
            .conn
            .query_row(
                "SELECT body FROM artifact_versions WHERE artifact_id = ?1 \
                 ORDER BY version_no DESC LIMIT 1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        let body = body.ok_or_else(|| ArtifactError::NotFound(id.to_string()))?;

        Ok(render::render(source_format, &body))
    }

    /// Return the most-recent artifact whose `(source_kind, source_ref)` match
    /// — used to link a producer's originating work back to the artifact it
    /// produced (e.g. a completed kanban task → its artifact, keyed by
    /// `source_kind="kanban"`, `source_ref=<task id>`; Phase 46.6 gap-closure).
    /// Returns `Ok(None)` when no such artifact exists.
    pub fn latest_for_source(
        &self,
        source_kind: &str,
        source_ref: &str,
    ) -> Result<Option<ArtifactSummary>> {
        self.conn
            .query_row(
                "SELECT id, title, icon, source_kind, source_ref, updated_at, archived \
                 FROM artifacts WHERE source_kind = ?1 AND source_ref = ?2 \
                 ORDER BY updated_at DESC LIMIT 1",
                params![source_kind, source_ref],
                |r| {
                    Ok(ArtifactSummary {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        icon: r.get(2)?,
                        source_kind: r.get(3)?,
                        source_ref: r.get(4)?,
                        updated_at: r.get(5)?,
                        archived: r.get::<_, i64>(6)? != 0,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Permanently delete an artifact (scoped to `profile`) and all of its
    /// version rows. Profile-scoped at the SQL layer so a caller can only ever
    /// delete artifacts in its own profile bucket (IDOR mitigation — mirrors the
    /// profile-scoped read path). Versions are removed first (they FK-reference
    /// `artifacts.id`); the version delete is itself profile-scoped via a
    /// subselect. Returns `true` iff a matching artifact row was deleted.
    pub fn delete(&mut self, profile: &str, id: &str) -> Result<bool> {
        self.conn.execute(
            "DELETE FROM artifact_versions WHERE artifact_id IN \
             (SELECT id FROM artifacts WHERE id = ?1 AND profile = ?2)",
            params![id, profile],
        )?;
        let n = self.conn.execute(
            "DELETE FROM artifacts WHERE id = ?1 AND profile = ?2",
            params![id, profile],
        )?;
        Ok(n > 0)
    }

    /// Archive or unarchive an artifact (scoped to `profile`; IDOR mitigation).
    /// Leaves `updated_at` untouched so listing order is stable across the
    /// toggle. Returns `true` iff a matching artifact row was updated.
    pub fn set_archived(&mut self, profile: &str, id: &str, archived: bool) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE artifacts SET archived = ?1 WHERE id = ?2 AND profile = ?3",
            params![archived, id, profile],
        )?;
        Ok(n > 0)
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Env var (Phase 46.6 gap-closure): when set to a non-empty path,
/// [`default_db_path`] / [`ArtifactStore::open_default`] resolve here instead
/// of `get_hermes_home()/artifacts.db`.
///
/// Mirrors kanban's `IRONHERMES_KANBAN_DB` board-redirect. A kanban WORKER runs
/// under the assignee PROFILE home (`IRONHERMES_HOME=profiles/<assignee>`), so
/// without this override its `open_default()` lands in
/// `profiles/<assignee>/artifacts.db` — invisible to the operator gallery,
/// which reads the ROOT store. `build_kanban_worker_env` (`ironhermes-kanban`)
/// sets this to the root `artifacts.db` path so kanban-produced artifacts are
/// visible in the gallery (D-01/D-04).
pub const ARTIFACTS_DB_ENV: &str = "IRONHERMES_ARTIFACTS_DB";

/// Resolve the default artifacts DB path: the [`ARTIFACTS_DB_ENV`] override when
/// set to a non-empty value, else `get_hermes_home()/artifacts.db`.
pub fn default_db_path() -> PathBuf {
    match std::env::var(ARTIFACTS_DB_ENV) {
        Ok(p) if !p.is_empty() => {
            debug!(
                target: "artifacts.db",
                path = %p,
                "using IRONHERMES_ARTIFACTS_DB override"
            );
            PathBuf::from(p)
        }
        _ => get_hermes_home().join("artifacts.db"),
    }
}

fn unix_now() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (ArtifactStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifacts.db");
        let store = ArtifactStore::new(&path).unwrap();
        (store, dir)
    }

    #[test]
    fn open_and_init_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifacts.db");
        let _store1 = ArtifactStore::new(&path).unwrap();
        let store2 = ArtifactStore::new(&path).unwrap();
        let count: i64 = store2
            .conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn open_default_resolves_under_hermes_home() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: test-only; nextest runs each test in its own process so
        // this env mutation cannot race with other tests.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", dir.path());
        }
        let path = default_db_path();
        assert!(path.ends_with("artifacts.db"));
        assert_eq!(path.parent().unwrap(), dir.path());
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    #[test]
    fn default_db_path_honors_env_override() {
        // Phase 46.6 gap-closure: IRONHERMES_ARTIFACTS_DB must win over the
        // hermes-home default so a kanban worker (profile home) can be
        // redirected at the ROOT store. SAFETY: nextest process-isolates tests.
        let dir = tempfile::tempdir().unwrap();
        let override_path = dir.path().join("root").join("artifacts.db");
        unsafe {
            std::env::set_var("IRONHERMES_HOME", dir.path().join("profile"));
            std::env::set_var(ARTIFACTS_DB_ENV, &override_path);
        }
        assert_eq!(
            default_db_path(),
            override_path,
            "IRONHERMES_ARTIFACTS_DB must override the hermes-home default"
        );
        // An empty override falls back to the hermes-home default.
        unsafe {
            std::env::set_var(ARTIFACTS_DB_ENV, "");
        }
        assert!(
            default_db_path().ends_with("artifacts.db")
                && default_db_path().starts_with(dir.path().join("profile")),
            "empty IRONHERMES_ARTIFACTS_DB must fall back to get_hermes_home()"
        );
        unsafe {
            std::env::remove_var(ARTIFACTS_DB_ENV);
            std::env::remove_var("IRONHERMES_HOME");
        }
    }

    // -- Task 3: publish / list / load -----------------------------------

    #[test]
    fn publish_creates_artifact_and_version() {
        let (mut store, _dir) = temp_store();
        let id = store
            .publish(PublishInput {
                profile: "alice".into(),
                update_id: None,
                title: Some("Hello".into()),
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Html,
                body: "<b>hi</b>".into(),
            })
            .unwrap();

        let artifact_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM artifacts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(artifact_count, 1);

        let version_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM artifact_versions WHERE artifact_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version_count, 1);

        let version_no: i64 = store
            .conn
            .query_row(
                "SELECT version_no FROM artifact_versions WHERE artifact_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version_no, 1);
    }

    #[test]
    fn version_history_preserved() {
        let (mut store, _dir) = temp_store();
        let id = store
            .publish(PublishInput {
                profile: "alice".into(),
                update_id: None,
                title: Some("Hello".into()),
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Html,
                body: "<b>v1</b>".into(),
            })
            .unwrap();

        store
            .publish(PublishInput {
                profile: "alice".into(),
                update_id: Some(id.clone()),
                title: Some("Hello v2".into()),
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Html,
                body: "<b>v2</b>".into(),
            })
            .unwrap();

        let version_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM artifact_versions WHERE artifact_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version_count, 2);

        let v1_body: String = store
            .conn
            .query_row(
                "SELECT body FROM artifact_versions WHERE artifact_id = ?1 AND version_no = 1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v1_body, "<b>v1</b>");
    }

    #[test]
    fn oversize_publish_rejected() {
        let (mut store, _dir) = temp_store();
        let big_body = "a".repeat(17 * 1024 * 1024);
        let result = store.publish(PublishInput {
            profile: "alice".into(),
            update_id: None,
            title: Some("Big".into()),
            icon: None,
            source_kind: None,
            source_ref: None,
            source_format: SourceFormat::Html,
            body: big_body,
        });
        assert!(matches!(result, Err(ArtifactError::TooLarge)));

        let artifact_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM artifacts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(artifact_count, 0);
        let version_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM artifact_versions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version_count, 0);
    }

    #[test]
    fn list_for_profile_is_scoped() {
        let (mut store, _dir) = temp_store();
        store
            .publish(PublishInput {
                profile: "alice".into(),
                update_id: None,
                title: Some("A1".into()),
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Html,
                body: "<p>a1</p>".into(),
            })
            .unwrap();
        store
            .publish(PublishInput {
                profile: "bob".into(),
                update_id: None,
                title: Some("B1".into()),
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Html,
                body: "<p>b1</p>".into(),
            })
            .unwrap();

        let alice_artifacts = store.list_for_profile("alice").unwrap();
        assert_eq!(alice_artifacts.len(), 1);
        assert_eq!(alice_artifacts[0].title, "A1");
    }

    #[test]
    fn load_latest_html_returns_newest_version() {
        let (mut store, _dir) = temp_store();
        let id = store
            .publish(PublishInput {
                profile: "alice".into(),
                update_id: None,
                title: Some("MD".into()),
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Markdown,
                body: "# v1".into(),
            })
            .unwrap();
        store
            .publish(PublishInput {
                profile: "alice".into(),
                update_id: Some(id.clone()),
                title: None,
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Markdown,
                body: "# v2".into(),
            })
            .unwrap();

        let html = store.load_latest_html(&id).unwrap();
        assert!(html.contains("v2"));
        assert!(html.contains("<h1>"));
    }

    #[test]
    fn load_latest_html_not_found() {
        let (store, _dir) = temp_store();
        let result = store.load_latest_html("does-not-exist");
        assert!(matches!(result, Err(ArtifactError::NotFound(_))));
    }

    // -- Artifact management: archive / delete / migration ----------------

    fn publish_one(store: &mut ArtifactStore, profile: &str, title: &str) -> String {
        store
            .publish(PublishInput {
                profile: profile.into(),
                update_id: None,
                title: Some(title.into()),
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Html,
                body: "<b>x</b>".into(),
            })
            .unwrap()
    }

    #[test]
    fn archive_hides_from_default_list_and_unarchive_restores() {
        let (mut store, _dir) = temp_store();
        let id = publish_one(&mut store, "alice", "A");

        let listed = store.list_for_profile("alice").unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].archived);

        assert!(store.set_archived("alice", &id, true).unwrap());
        assert!(
            store.list_for_profile("alice").unwrap().is_empty(),
            "archived artifact must be hidden from the default list"
        );
        let with_arch = store.list_for_profile_filtered("alice", true).unwrap();
        assert_eq!(with_arch.len(), 1);
        assert!(
            with_arch[0].archived,
            "included row must report archived=true"
        );

        assert!(store.set_archived("alice", &id, false).unwrap());
        assert_eq!(store.list_for_profile("alice").unwrap().len(), 1);
    }

    #[test]
    fn mutations_are_profile_scoped() {
        // IDOR mitigation: a caller can only delete/archive artifacts in its own
        // profile bucket. A different profile's id must never match.
        let (mut store, _dir) = temp_store();
        let id = publish_one(&mut store, "alice", "A");

        assert!(
            !store.delete("bob", &id).unwrap(),
            "cross-profile delete must not match"
        );
        assert!(
            !store.set_archived("bob", &id, true).unwrap(),
            "cross-profile archive must not match"
        );
        assert_eq!(
            store.list_for_profile("alice").unwrap().len(),
            1,
            "artifact must survive cross-profile mutation attempts"
        );

        // The owning profile succeeds.
        assert!(store.delete("alice", &id).unwrap());
        assert!(store.list_for_profile("alice").unwrap().is_empty());
    }

    #[test]
    fn delete_removes_artifact_and_versions() {
        let (mut store, _dir) = temp_store();
        let id = publish_one(&mut store, "alice", "A");
        // Append a second version.
        store
            .publish(PublishInput {
                profile: "alice".into(),
                update_id: Some(id.clone()),
                title: None,
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Html,
                body: "<b>v2</b>".into(),
            })
            .unwrap();

        assert!(store.delete("alice", &id).unwrap());
        assert!(store.list_for_profile("alice").unwrap().is_empty());
        let vcount: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM artifact_versions WHERE artifact_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(vcount, 0, "all version rows must be deleted");
        assert!(matches!(
            store.load_latest_html(&id),
            Err(ArtifactError::NotFound(_))
        ));
    }

    #[test]
    fn migrates_v1_db_adds_archived_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifacts.db");
        // Hand-build a v1 database (no `archived` column, schema_version = 1).
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL);
                 CREATE TABLE artifacts (
                    id TEXT PRIMARY KEY, profile TEXT NOT NULL, title TEXT NOT NULL,
                    icon TEXT, source_kind TEXT, source_ref TEXT, source_format TEXT NOT NULL,
                    created_at REAL NOT NULL, updated_at REAL NOT NULL);
                 CREATE TABLE artifact_versions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, artifact_id TEXT NOT NULL,
                    version_no INTEGER NOT NULL, body TEXT NOT NULL,
                    rendered_bytes INTEGER NOT NULL, created_at REAL NOT NULL);
                 INSERT INTO schema_version (version) VALUES (1);
                 INSERT INTO artifacts (id, profile, title, source_format, created_at, updated_at)
                    VALUES ('x', 'alice', 'Old', 'html', 1.0, 1.0);",
            )
            .unwrap();
        }
        // Opening runs the v1→v2 migration.
        let store = ArtifactStore::new(&path).unwrap();
        let ver: i64 = store
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(ver, 2, "schema must migrate to v2");
        let listed = store.list_for_profile("alice").unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].archived, "migrated rows default to not-archived");
    }

    #[test]
    fn update_refreshes_source_format_so_html_renders_as_html() {
        let (mut store, _dir) = temp_store();

        // Create as Markdown (the artifact tool's default source_format).
        let id = store
            .publish(PublishInput {
                profile: "default".to_string(),
                update_id: None,
                title: Some("t".to_string()),
                icon: None,
                source_kind: Some("kanban".to_string()),
                source_ref: Some("t_1".to_string()),
                source_format: SourceFormat::Markdown,
                body: "# original".to_string(),
            })
            .unwrap();

        // Version it with HTML — mirrors the deterministic kanban capture
        // versioning a tool-published md artifact for the same task.
        store
            .publish(PublishInput {
                profile: "default".to_string(),
                update_id: Some(id.clone()),
                title: None,
                icon: None,
                source_kind: None,
                source_ref: None,
                source_format: SourceFormat::Html,
                body: "# Updated Heading".to_string(),
            })
            .unwrap();

        // WR-02: the latest body must render with the format it was published in.
        // render(Html, ..) is a passthrough, so the literal "# " survives. If the
        // stored format were left as md (the bug), it would render to
        // "<h1>Updated Heading</h1>".
        let rendered = store.load_latest_html(&id).unwrap();
        assert_eq!(
            rendered, "# Updated Heading",
            "HTML version must render as HTML (passthrough), not through the md renderer"
        );
    }
}
