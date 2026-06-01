//! `KanbanStore` — durable SQLite store for the kanban kernel (Plan 02, D-04).
//!
//! Mirrors the `ironhermes-state` `StateStore` open/migration idiom:
//! - `create_dir_all` parent on open
//! - `busy_timeout(5000 ms)`
//! - `execute_batch(SCHEMA_SQL)` (idempotent DDL)
//! - schema_version row check → insert on first run / migrate on upgrade
//!
//! All write operations use `rusqlite::params!` bindings — no `format!`-
//! interpolated SQL (T-36.3.7-02-05 / Threat Register).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::error::{KanbanError, Result};
use crate::events::KanbanEventKind;
use crate::paths::validate_dir_workspace;
use crate::schema::{SCHEMA_SQL, SCHEMA_VERSION, run_migrations};
use crate::types::{
    KanbanStatus, KanbanWorkerSpec, Subscription, SwarmGraphIds, SwarmGraphSpec, Task,
    TaskComment, TaskRun,
};

// ---------------------------------------------------------------------------
// Public option / filter types
// ---------------------------------------------------------------------------

/// Options for [`KanbanStore::create_task`].
#[derive(Debug, Clone, Default)]
pub struct CreateTaskOptions {
    pub body: Option<String>,
    /// Parent task ids. Links are inserted (with tenant check) after the task row.
    pub parents: Vec<String>,
    pub tenant: Option<String>,
    pub workspace: Option<String>,
    pub skills: Option<Vec<String>>,
    pub priority: Option<i64>,
    /// When set, `create_task` short-circuits to the existing task if the key
    /// already exists (D-24).
    pub idempotency_key: Option<String>,
    pub scheduled_at: Option<f64>,
    pub max_runtime_seconds: Option<i64>,
    pub max_retries: Option<i64>,
    /// Put task in `triage` status instead of `ready` (D-06).
    pub triage: bool,
    /// Profile slug that created the task (D-22 `created_cards` gate).
    pub created_by: Option<String>,
}

/// Filters for [`KanbanStore::list_tasks`].
#[derive(Debug, Clone, Default)]
pub struct ListFilters {
    pub assignee: Option<String>,
    pub status: Option<String>,
    pub tenant: Option<String>,
    /// When false (default) archived tasks are excluded.
    pub archived: bool,
    pub limit: Option<i64>,
}

// ---------------------------------------------------------------------------
// Phase 36.3.7.8 — @mention delegation public types
// ---------------------------------------------------------------------------

/// Input plan for [`KanbanStore::create_mention_children`].
///
/// Carries the resolved, pre-validated mention children that the store will
/// materialize atomically into the DB. The caller (Plan 03 LLM tool or Plan 04
/// CLI verb) is responsible for running the mention parser + resolver before
/// constructing this struct; the store only sees post-resolution children.
#[derive(Debug, Clone)]
pub struct MentionPlan {
    /// ID of the parent task whose body was parsed.
    pub parent_id: String,
    /// Resolved, validated mention children to materialize (may be empty).
    pub children: Vec<MentionChildSpec>,
    /// When set, enables idempotency replay: re-invocation with the same key
    /// returns the existing children instead of inserting new rows (REQ-12).
    ///
    /// # Invariant
    /// The key must be stable across re-invocations even if the parent body
    /// changes — idempotency is keyed on the *intent*, not the content.
    pub idempotency_key: Option<String>,
    pub workspace: Option<String>,
    pub skills: Option<Vec<String>>,
    pub tenant: Option<String>,
    /// Profile slug that created the mention batch (`$HERMES_PROFILE`).
    pub created_by: Option<String>,
}

/// A single resolved mention child ready for DB insertion.
///
/// `handle` is the original-cased handle from prose; `assignee` is the
/// lowercased, `validate_profile_name`-validated form. Both are stored so the
/// caller can surface the original text in UI without a separate lookup.
#[derive(Debug, Clone)]
pub struct MentionChildSpec {
    /// Original-cased handle as it appeared in prose (e.g. `"Reviewer"`).
    pub handle: String,
    /// Normalized + validated profile name (post-resolver, e.g. `"reviewer"`).
    pub assignee: String,
    /// 0-indexed position within the ORIGINAL parsed mention list (REQ-12 key
    /// stability). Skipped mentions reserve their slot so `mention_index` is
    /// deterministic on re-parse regardless of which handles were resolved.
    pub mention_index: usize,
    /// Auto-derived title, e.g. `"@reviewer mention from t_abcd"`.
    pub title: String,
    /// Optional body for the child card.
    pub body: Option<String>,
}

/// Result returned by [`KanbanStore::create_mention_children`].
#[derive(Debug, Clone)]
pub struct MentionResult {
    pub children: Vec<MentionChildIds>,
}

/// IDs and metadata for a single materialized mention child card.
///
/// # Handle on replay
/// When [`MentionResult`] is produced via the idempotency replay path
/// ([`KanbanStore::find_mentions_by_idempotency_key`]), `handle` is `""` because
/// the original-cased handle is not persisted in a separate column. Callers that
/// need it can recover it from the `task_events` row whose payload contains
/// `"source":"mention"` and `"handle":<original>`.
#[derive(Debug, Clone)]
pub struct MentionChildIds {
    pub child_id: String,
    pub assignee: String,
    /// Original-cased handle. Empty string on idempotency-replay paths — see
    /// type-level doc for details.
    pub handle: String,
    /// Original parse-order index (matches [`MentionChildSpec::mention_index`]).
    pub mention_index: usize,
}

// ---------------------------------------------------------------------------
// KanbanStore
// ---------------------------------------------------------------------------

/// Durable SQLite store for the kanban kernel.
pub struct KanbanStore {
    /// The raw rusqlite connection.
    ///
    /// `pub` to allow integration tests (in `tests/`) to seed state that
    /// cannot be expressed via the public CRUD API (backdating timestamps,
    /// inserting bare `task_runs` rows, etc.). Production callers should use
    /// the typed methods instead.
    pub conn: Connection,
}

/// Escape SQLite LIKE wildcards (`%`, `_`) and the escape character itself
/// (`\`) so the result can be used as a literal-match LIKE pattern with
/// `ESCAPE '\'`.
///
/// WR-01 (Phase 36.3.7.8 code review): operator/LLM-supplied idempotency
/// keys may contain wildcard characters that, without escaping, would cause
/// cross-batch identity confusion when matching `{base}:mention:%`. Apply
/// this helper at every LIKE site that interpolates user-supplied data.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

impl KanbanStore {
    /// Phase 36.3.7.10 — KanbanConfig accessor (CR-WARNING-5 sourcing seam).
    ///
    /// Returns a default `KanbanConfig` because `KanbanStore` does not store
    /// its own per-board config (the per-board config.yaml is not loaded at
    /// `open` time). Callers that need operator-configured decomposer settings
    /// should load the main config via `Config::load()` and deserialize
    /// `config.kanban` into `KanbanConfig` directly (see `commands.rs`
    /// `load_kanban_config` helper).
    ///
    /// This accessor exists so that `cmd_decompose` / `cmd_specify` can call
    /// `store.config()` without an additional argument when a board-level
    /// override has not been loaded, matching the interface promised by the plan.
    pub fn config(&self) -> crate::config::KanbanConfig {
        crate::config::KanbanConfig::default()
    }

    /// Open (or create) a database at `path`. No slug label — migration banner
    /// uses `"default"` if it ever fires.
    ///
    /// - Creates parent directories if missing.
    /// - Applies `PRAGMA journal_mode=WAL` and `foreign_keys=ON`.
    /// - Runs the migration ladder if the DB schema is behind the binary.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::new_inner(path.as_ref(), None)
    }

    /// Open (or create) a database at `path` with `slug` used in the D-06
    /// migration banner so the user sees which board was migrated.
    pub fn open_labeled(path: impl AsRef<Path>, slug: &str) -> Result<Self> {
        Self::new_inner(path.as_ref(), Some(slug))
    }

    /// Open the default board DB at `~/.ironhermes/kanban.db` (D-01 back-compat).
    pub fn open_default() -> Result<Self> {
        Self::open(crate::paths::kanban_db_path())
    }

    /// Open a board's DB by slug. Resolves the path via
    /// `paths::board_db_path_for_slug`: `"default"` → legacy `kanban.db`;
    /// any other slug → `boards/<slug>/kanban.db` (D-01).
    pub fn open_for_board(slug: &str) -> Result<Self> {
        let path = crate::paths::board_db_path_for_slug(slug);
        Self::open_labeled(path, slug)
    }

    /// Back-compat alias for [`KanbanStore::open`].
    ///
    /// Existing callers (`KanbanStore::new(path)`) continue to work unchanged.
    /// New code should prefer the explicit `open` / `open_for_board` surface.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(path)
    }

    // -----------------------------------------------------------------------
    // Private open implementation
    // -----------------------------------------------------------------------

    fn new_inner(path: &Path, slug_hint: Option<&str>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create kanban dir {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("open kanban DB at {}", path.display()))?;

        conn.busy_timeout(std::time::Duration::from_millis(5000))?;

        let mut store = Self { conn };
        store.init_schema(slug_hint)?;
        Ok(store)
    }

    // -----------------------------------------------------------------------
    // Schema management
    // -----------------------------------------------------------------------

    /// Format the D-06 migration banner string (pure helper — no I/O).
    ///
    /// The banner is emitted to stderr by `init_schema` when a board's
    /// `schema_version` row is behind the binary's `SCHEMA_VERSION`. This
    /// pure helper exists so tests can lock the format byte-stably without
    /// needing to capture stderr.
    ///
    /// The format is the D-06 banner: `migrated board '<label>' from v<old> to v<new>`,
    /// prefixed with the `[kanban]` tag. No trailing punctuation; `eprintln!` adds the newline.
    pub fn format_migration_banner(
        slug: Option<&str>,
        old: i64,
        new: i64,
    ) -> String {
        let label = slug.unwrap_or("default");
        format!("[kanban] migrated board '{}' from v{} to v{}", label, old, new)
    }

    fn init_schema(&mut self, slug_hint: Option<&str>) -> Result<()> {
        // Idempotent DDL — CREATE TABLE IF NOT EXISTS / CREATE INDEX IF NOT EXISTS.
        self.conn.execute_batch(SCHEMA_SQL)?;

        let current: Option<i64> = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?;

        match current {
            None => {
                // Fresh DB: insert the current schema version. No banner.
                self.conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
            }
            Some(v) if v < SCHEMA_VERSION => {
                // Migration needed. Wrap run_migrations + UPDATE in a rusqlite
                // RAII transaction so a crash or early return automatically
                // rolls back rather than leaving a partial-migrated DB with an
                // un-bumped version row (T-4 mitigation, CR-04 fix).
                //
                // Note: run_migrations receives &mut self.conn (a &mut Connection)
                // so it can perform DDL statements. The rusqlite Transaction type
                // does not implement DerefMut, so we obtain the transaction after
                // migrations have been applied and wrap only the version UPDATE +
                // banner in the RAII scope. If any step fails the Transaction Drop
                // rolls back automatically.
                run_migrations(&mut self.conn, v)?;
                let banner = Self::format_migration_banner(slug_hint, v, SCHEMA_VERSION);
                let tx = self.conn.transaction()?;
                eprintln!("{}", banner);
                tx.execute(
                    "UPDATE schema_version SET version = ?1",
                    params![SCHEMA_VERSION],
                )?;
                tx.commit()?;
            }
            Some(v) if v > SCHEMA_VERSION => {
                // Board was opened with a newer binary — refuse to downgrade.
                return Err(KanbanError::Other(anyhow::anyhow!(
                    "board schema version {} is newer than binary supports ({}); upgrade binary",
                    v,
                    SCHEMA_VERSION
                )));
            }
            Some(_) => {
                // v == SCHEMA_VERSION: happy path, nothing to do (Pitfall 6).
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    fn new_id(prefix: &str) -> String {
        // `t_` + first 16 hex chars of a v4 UUID (no hyphens).
        let id = uuid::Uuid::new_v4().simple().to_string();
        format!("{prefix}_{}", &id[..16])
    }

    fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            body: row.get(2)?,
            assignee: row.get(3)?,
            status: row.get(4)?,
            priority: row.get(5)?,
            tenant: row.get(6)?,
            workspace: row.get(7)?,
            skills: row.get(8)?,
            idempotency_key: row.get(9)?,
            claim_lock: row.get(10)?,
            claim_expires: row.get(11)?,
            current_run_id: row.get(12)?,
            consecutive_failures: row.get(13)?,
            max_retries: row.get(14)?,
            max_runtime_seconds: row.get(15)?,
            scheduled_at: row.get(16)?,
            workflow_template_id: row.get(17)?,
            current_step_key: row.get(18)?,
            created_by: row.get(19)?,
            created_at: row.get(20)?,
            started_at: row.get(21)?,
            ended_at: row.get(22)?,
        })
    }

    /// Append an event row and return the new auto-increment id.
    fn append_event_internal(
        conn: &Connection,
        task_id: &str,
        run_id: Option<&str>,
        kind: KanbanEventKind,
        payload: Option<&Value>,
        now: f64,
    ) -> Result<i64> {
        let payload_str = payload.map(|v| v.to_string());
        conn.execute(
            "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, run_id, kind.as_str(), payload_str, now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    // -----------------------------------------------------------------------
    // Public CRUD
    // -----------------------------------------------------------------------

    /// Create a new task. Respects idempotency_key short-circuit (D-24).
    pub fn create_task(
        &mut self,
        title: &str,
        assignee: &str,
        opts: CreateTaskOptions,
    ) -> Result<Task> {
        // Assignee validation (D-17 / CONTEXT.md code_context).
        ironhermes_core::profile::validate_profile_name(assignee)
            .map_err(|e| KanbanError::Other(anyhow::anyhow!("invalid assignee: {e}")))?;

        // Workspace validation for dir: prefix (D-31 / Pitfall 6).
        if let Some(ref ws) = opts.workspace {
            validate_dir_workspace(ws)?;
        }

        // Idempotency short-circuit (D-24).
        if let Some(ref key) = opts.idempotency_key {
            if let Some(existing) = self.find_by_idempotency_key(key)? {
                return Ok(existing);
            }
        }

        let now = Self::now();
        let id = Self::new_id("t");

        // Determine initial status (D-06).
        let status = if opts.triage {
            KanbanStatus::Triage.as_str()
        } else if opts.parents.is_empty() {
            KanbanStatus::Ready.as_str()
        } else {
            KanbanStatus::Todo.as_str()
        };

        let skills_json = opts
            .skills
            .as_ref()
            .map(|v| serde_json::to_string(v))
            .transpose()?;

        self.conn.execute(
            "INSERT INTO tasks \
             (id, title, body, assignee, status, priority, tenant, workspace, skills, \
              idempotency_key, consecutive_failures, max_retries, max_runtime_seconds, \
              scheduled_at, created_by, created_at) \
             VALUES \
             (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12, ?13, ?14, ?15)",
            params![
                id,
                title,
                opts.body,
                assignee,
                status,
                opts.priority.unwrap_or(0),
                opts.tenant,
                opts.workspace,
                skills_json,
                opts.idempotency_key,
                opts.max_retries,
                opts.max_runtime_seconds,
                opts.scheduled_at,
                opts.created_by,
                now,
            ],
        )?;

        // Insert parent links (honors D-39 tenant check inside insert_link).
        for parent_id in &opts.parents {
            self.insert_link(parent_id, &id)?;
        }

        // Append `created` event.
        let payload = serde_json::json!({
            "assignee": assignee,
            "status": status,
            "parents": opts.parents,
            "tenant": opts.tenant,
        });
        Self::append_event_internal(&self.conn, &id, None, KanbanEventKind::Created, Some(&payload), now)?;

        self.get_task(&id)
    }

    /// Look up a task by id. Returns `KanbanError::TaskNotFound` on miss.
    pub fn get_task(&self, id: &str) -> Result<Task> {
        self.conn
            .query_row(
                "SELECT id, title, body, assignee, status, priority, tenant, workspace, skills, \
                 idempotency_key, claim_lock, claim_expires, current_run_id, consecutive_failures, \
                 max_retries, max_runtime_seconds, scheduled_at, workflow_template_id, \
                 current_step_key, created_by, created_at, started_at, ended_at \
                 FROM tasks WHERE id = ?1",
                params![id],
                Self::row_to_task,
            )
            .optional()?
            .ok_or_else(|| KanbanError::TaskNotFound(id.to_string()))
    }

    /// List tasks with optional filters.
    ///
    /// `archived` tasks are excluded by default unless `filters.archived = true`.
    pub fn list_tasks(&self, filters: ListFilters) -> Result<Vec<Task>> {
        let mut sql = String::from(
            "SELECT id, title, body, assignee, status, priority, tenant, workspace, skills, \
             idempotency_key, claim_lock, claim_expires, current_run_id, consecutive_failures, \
             max_retries, max_runtime_seconds, scheduled_at, workflow_template_id, \
             current_step_key, created_by, created_at, started_at, ended_at \
             FROM tasks WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut idx = 1usize;

        if !filters.archived {
            sql.push_str(" AND status != 'archived'");
        }
        if let Some(ref a) = filters.assignee {
            sql.push_str(&format!(" AND assignee = ?{idx}"));
            args.push(Box::new(a.clone()));
            idx += 1;
        }
        if let Some(ref s) = filters.status {
            sql.push_str(&format!(" AND status = ?{idx}"));
            args.push(Box::new(s.clone()));
            idx += 1;
        }
        if let Some(ref t) = filters.tenant {
            sql.push_str(&format!(" AND tenant = ?{idx}"));
            args.push(Box::new(t.clone()));
            idx += 1;
        }

        sql.push_str(" ORDER BY created_at ASC");

        if let Some(limit) = filters.limit {
            sql.push_str(&format!(" LIMIT ?{idx}"));
            args.push(Box::new(limit));
        }

        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let tasks = stmt
            .query_map(refs.as_slice(), Self::row_to_task)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(tasks)
    }

    /// Add a comment to a task. Returns the new `TaskComment`.
    pub fn add_comment(&mut self, task_id: &str, author: &str, body: &str) -> Result<TaskComment> {
        // Verify task exists.
        self.get_task(task_id)?;

        let id = Self::new_id("c");
        let now = Self::now();

        self.conn.execute(
            "INSERT INTO task_comments (id, task_id, author, body, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, task_id, author, body, now],
        )?;

        Ok(TaskComment {
            id,
            task_id: task_id.to_string(),
            author: author.to_string(),
            body: body.to_string(),
            created_at: now,
        })
    }

    /// Insert a parent→child dependency link (D-39 tenant check).
    ///
    /// Rejects with `TenantMismatch` when **both** tasks have a non-NULL,
    /// non-equal tenant (D-39: "use a tenant-less parent if cross-tenant fanout
    /// is needed").
    pub fn insert_link(&mut self, parent_id: &str, child_id: &str) -> Result<()> {
        let parent_tenant: Option<String> = self
            .conn
            .query_row(
                "SELECT tenant FROM tasks WHERE id = ?1",
                params![parent_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();

        let child_tenant: Option<String> = self
            .conn
            .query_row(
                "SELECT tenant FROM tasks WHERE id = ?1",
                params![child_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();

        // D-39: reject only when BOTH are Some and unequal.
        if let (Some(p), Some(c)) = (&parent_tenant, &child_tenant) {
            if p != c {
                return Err(KanbanError::TenantMismatch {
                    parent: p.clone(),
                    child: c.clone(),
                });
            }
        }

        let now = Self::now();
        self.conn.execute(
            "INSERT OR IGNORE INTO task_links (parent_id, child_id, created_at) \
             VALUES (?1, ?2, ?3)",
            params![parent_id, child_id, now],
        )?;
        Ok(())
    }

    /// Sibling of [`insert_link`](Self::insert_link) that runs `WITH RECURSIVE`
    /// descendant-walk cycle detection + the existing tenant gate (D-39) inside
    /// a single `BEGIN IMMEDIATE` transaction (Phase 36.3.7.6 BUG-36.3.7.6-02,
    /// D-link-cycle-detection).
    ///
    /// Rejects with:
    /// - [`KanbanError::TaskNotFound`] when either id is absent from `tasks`.
    /// - [`KanbanError::LinkCycle`] when `parent_id` is already a transitive
    ///   descendant of `child_id` (i.e., the new link would close a cycle).
    /// - [`KanbanError::TenantMismatch`] per D-39 (same as `insert_link`).
    ///
    /// The CTE walks descendants of the proposed `child_id` and rejects if the
    /// proposed `parent_id` appears in the descendant set. The underlying SQL
    /// uses SQLite's `WITH RECURSIVE` (supported since 3.8.3) against the
    /// existing PRIMARY KEY `(parent_id, child_id)` index on `task_links`, so
    /// no new index is required. The `BEGIN IMMEDIATE` transaction matches the
    /// pattern used by `block_task` (this file) — TOCTOU-safe under WAL-mode
    /// concurrent writers.
    ///
    /// `insert_link` is left unchanged so the existing `kanban_create::parents`
    /// path keeps the legacy (cycle-impossible-by-construction) behavior. Only
    /// new writes via the LLM-tool `kanban_link` surface are cycle-checked.
    pub fn insert_link_checked(&mut self, parent_id: &str, child_id: &str) -> Result<()> {
        use rusqlite::TransactionBehavior;

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Phantom-id pre-check: friendlier than letting the FK trigger.
        let parent_exists: Option<String> = tx
            .query_row(
                "SELECT id FROM tasks WHERE id = ?1",
                params![parent_id],
                |r| r.get(0),
            )
            .optional()?;
        if parent_exists.is_none() {
            return Err(KanbanError::TaskNotFound(parent_id.to_string()));
        }
        let child_exists: Option<String> = tx
            .query_row(
                "SELECT id FROM tasks WHERE id = ?1",
                params![child_id],
                |r| r.get(0),
            )
            .optional()?;
        if child_exists.is_none() {
            return Err(KanbanError::TaskNotFound(child_id.to_string()));
        }

        // Tenant gate (D-39 — same logic as insert_link).
        let parent_tenant: Option<String> = tx
            .query_row(
                "SELECT tenant FROM tasks WHERE id = ?1",
                params![parent_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let child_tenant: Option<String> = tx
            .query_row(
                "SELECT tenant FROM tasks WHERE id = ?1",
                params![child_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let (Some(p), Some(c)) = (&parent_tenant, &child_tenant) {
            if p != c {
                return Err(KanbanError::TenantMismatch {
                    parent: p.clone(),
                    child: c.clone(),
                });
            }
        }

        // Cycle gate: walk descendants of child; reject if parent appears.
        let cycle: Option<i64> = tx
            .query_row(
                "WITH RECURSIVE descendants(id) AS ( \
                    SELECT child_id FROM task_links WHERE parent_id = ?1 \
                    UNION \
                    SELECT tl.child_id FROM task_links tl JOIN descendants d ON tl.parent_id = d.id \
                 ) \
                 SELECT 1 FROM descendants WHERE id = ?2 LIMIT 1",
                params![child_id, parent_id],
                |r| r.get(0),
            )
            .optional()?;
        if cycle.is_some() {
            return Err(KanbanError::LinkCycle {
                parent_id: parent_id.to_string(),
                child_id: child_id.to_string(),
            });
        }

        // Write.
        let now = Self::now();
        tx.execute(
            "INSERT OR IGNORE INTO task_links (parent_id, child_id, created_at) \
             VALUES (?1, ?2, ?3)",
            params![parent_id, child_id, now],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Materialize a multi-card swarm graph atomically (Phase 36.3.7.7 D-topology-shapes,
    /// D-atomic-transaction, D-no-create-task-reuse, D-idempotency-suffix-scheme).
    ///
    /// Sibling of [`create_task`](Self::create_task) — never re-enters it; INSERT
    /// statements are inlined directly inside the transaction handle so the
    /// outer `BEGIN IMMEDIATE` boundary stays clean (an outer tx that called
    /// `create_task` would deadlock or break atomicity since `create_task`
    /// takes `&mut self` and runs its own autocommit per statement).
    ///
    /// Graph shape (per D-topology-shapes — 4 supported topologies):
    /// 1. Fan-out only:           `root → [w_1..w_N]`           (verifier=None, synth=None)
    /// 2. Fan-out + verifier:     `root → [w_1..w_N] → verifier` (verifier=Some, synth=None)
    /// 3. Full 4-tier:            `root → [w_1..w_N] → verifier → synthesizer`
    /// 4. P3 quorum:              `root → [w_1..w_N] → synthesizer` (no verifier)
    ///
    /// Row writes inside one `BEGIN IMMEDIATE`:
    /// - 1 root card (`status='done'`, `ended_at=now`, `assignee=spec.created_by`)
    ///   + Created event row.
    /// - N worker cards (`status='todo'`, `ended_at=NULL`) + N Created event rows
    ///   + N task_links rows `(root → w_i)`.
    /// - 0..1 verifier card (`status='todo'`) + 0..1 Created event row
    ///   + N task_links rows `(w_i → verifier)` when present.
    /// - 0..1 synthesizer card (`status='todo'`) + 0..1 Created event row
    ///   + 1 task_links row `(verifier → synth)` when verifier present
    ///     OR N task_links rows `(w_i → synth)` when no verifier.
    /// - 0..1 `task_comments` row on root (`author='swarm'`) when blackboard provided.
    ///
    /// On any insert error: explicit transaction drop triggers automatic rollback
    /// (`?` propagation). Receiver test
    /// `swarm_invalid_assignee_rolls_back_whole_graph` locks this guarantee.
    ///
    /// Per-card idempotency key suffixes (D-idempotency-suffix-scheme): when
    /// `spec.idempotency_key = Some("k")`, root gets `k:root`, worker i gets
    /// `k:worker:{i}` (0-indexed), verifier gets `k:verifier`, synthesizer gets
    /// `k:synthesizer`. Re-invocation with the same key short-circuits via
    /// [`find_by_idempotency_key`](Self::find_by_idempotency_key) on `k:root` —
    /// the whole graph is then reconstructed by walking `task_links` from root.
    ///
    /// Cycle detection: plain `INSERT OR IGNORE INTO task_links` is sufficient
    /// because all swarm node IDs are freshly minted within this transaction
    /// (no pre-existing edges can connect to them).
    pub fn create_swarm(&mut self, spec: SwarmGraphSpec) -> Result<SwarmGraphIds> {
        use rusqlite::TransactionBehavior;

        // ---------------- Pre-flight validation (before opening tx) -------------

        if spec.workers.is_empty() {
            return Err(KanbanError::Other(anyhow::anyhow!("empty workers")));
        }

        let root_assignee = spec
            .created_by
            .clone()
            .ok_or_else(|| KanbanError::Other(anyhow::anyhow!("created_by required")))?;

        // Validate every assignee (root, each worker, optional verifier, optional
        // synthesizer) BEFORE opening the tx — matches `create_task:210-211`.
        ironhermes_core::profile::validate_profile_name(&root_assignee)
            .map_err(|e| KanbanError::Other(anyhow::anyhow!("invalid assignee: {e}")))?;
        for w in &spec.workers {
            ironhermes_core::profile::validate_profile_name(&w.assignee)
                .map_err(|e| KanbanError::Other(anyhow::anyhow!("invalid assignee: {e}")))?;
        }
        if let Some(ref v) = spec.verifier {
            ironhermes_core::profile::validate_profile_name(v)
                .map_err(|e| KanbanError::Other(anyhow::anyhow!("invalid assignee: {e}")))?;
        }
        if let Some(ref s) = spec.synthesizer {
            ironhermes_core::profile::validate_profile_name(s)
                .map_err(|e| KanbanError::Other(anyhow::anyhow!("invalid assignee: {e}")))?;
        }

        // Workspace validation (D-31 / Pitfall 6).
        if let Some(ref ws) = spec.workspace {
            validate_dir_workspace(ws)?;
        }

        let now = Self::now();
        let n = spec.workers.len();

        let key_root = spec.idempotency_key.as_ref().map(|k| format!("{k}:root"));
        let key_verifier = spec
            .idempotency_key
            .as_ref()
            .map(|k| format!("{k}:verifier"));
        let key_synth = spec
            .idempotency_key
            .as_ref()
            .map(|k| format!("{k}:synthesizer"));

        // ---------------- Open transaction --------------------------------------

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        // ---------------- Idempotency replay (INSIDE transaction) ---------------
        //
        // WR-01 (Phase 36.3.7.7 code review): the lookup MUST happen inside
        // `BEGIN IMMEDIATE` so a concurrent first-create race resolves
        // deterministically. If T1 commits its root row first, T2 (which
        // blocked on BEGIN IMMEDIATE) then observes the row and replays
        // instead of trying its own INSERT and failing with a UNIQUE-
        // constraint Sqlite error.
        //
        // If the root already exists under `k:root`, the whole graph already
        // exists (atomic-transaction guarantee — fully-present-or-fully-absent).
        // Walk per-card idempotency keys to gather worker / verifier / synth
        // IDs in their original insertion order.
        if let Some(ref kr) = key_root {
            let existing: Option<Task> = tx
                .query_row(
                    "SELECT id, title, body, assignee, status, priority, tenant, workspace, skills, \
                     idempotency_key, claim_lock, claim_expires, current_run_id, consecutive_failures, \
                     max_retries, max_runtime_seconds, scheduled_at, workflow_template_id, \
                     current_step_key, created_by, created_at, started_at, ended_at \
                     FROM tasks WHERE idempotency_key = ?1",
                    params![kr],
                    Self::row_to_task,
                )
                .optional()?;
            if let Some(existing_root) = existing {
                let root_id = existing_root.id.clone();

                // Worker IDs in insertion order — look up each per-card key
                // (`k:worker:0`, `k:worker:1`, ...) in order. This matches the
                // first-write linear loop and is independent of SQL ORDER BY
                // tie-break behavior on equal `created_at` timestamps.
                let mut worker_ids: Vec<String> = Vec::new();
                if let Some(ref base) = spec.idempotency_key {
                    let mut i = 0usize;
                    loop {
                        let kw = format!("{base}:worker:{i}");
                        let row: Option<String> = tx
                            .query_row(
                                "SELECT id FROM tasks WHERE idempotency_key = ?1",
                                params![&kw],
                                |r| r.get(0),
                            )
                            .optional()?;
                        match row {
                            Some(id) => {
                                worker_ids.push(id);
                                i += 1;
                            }
                            None => break,
                        }
                    }
                }

                let verifier_id: Option<String> = match key_verifier.as_deref() {
                    Some(k) => tx
                        .query_row(
                            "SELECT id FROM tasks WHERE idempotency_key = ?1",
                            params![k],
                            |r| r.get(0),
                        )
                        .optional()?,
                    None => None,
                };
                let synthesizer_id: Option<String> = match key_synth.as_deref() {
                    Some(k) => tx
                        .query_row(
                            "SELECT id FROM tasks WHERE idempotency_key = ?1",
                            params![k],
                            |r| r.get(0),
                        )
                        .optional()?,
                    None => None,
                };

                // WR-02 (Phase 36.3.7.7 code review): recover the blackboard
                // `task_comments` rowid by looking up the first author='swarm'
                // comment on the root card. This makes the replay envelope
                // shape-identical to the first-create envelope, so an
                // orchestrator that re-uses `idempotency_key` to reach the
                // blackboard via `blackboard_event_id` keeps working.
                let blackboard_event_id: Option<i64> = tx
                    .query_row(
                        "SELECT MIN(id) FROM task_comments WHERE task_id = ?1 AND author = 'swarm'",
                        params![&root_id],
                        |r| r.get::<_, Option<i64>>(0),
                    )
                    .optional()?
                    .flatten();

                // Reads only — no rows mutated. Drop the tx via commit (no-op).
                tx.commit()?;

                return Ok(SwarmGraphIds {
                    root_id,
                    worker_ids,
                    verifier_id,
                    synthesizer_id,
                    blackboard_event_id,
                });
            }
        }

        let skills_json: Option<String> = spec
            .skills
            .as_ref()
            .map(|v| serde_json::to_string(v))
            .transpose()?;

        // ---------------- Root card (status='done', ended_at=now) ---------------

        let root_id = Self::new_id("t");
        let root_status = KanbanStatus::Done.as_str();

        // Root has the extra `ended_at` column (Pitfall 1 — `create_task` omits it,
        // we add it as ?16 here so root's done-state has a valid end-time).
        tx.execute(
            "INSERT INTO tasks \
             (id, title, body, assignee, status, priority, tenant, workspace, skills, \
              idempotency_key, consecutive_failures, max_retries, max_runtime_seconds, \
              scheduled_at, created_by, created_at, ended_at) \
             VALUES \
             (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                root_id,
                spec.goal,
                spec.body,
                root_assignee,
                root_status,
                spec.priority.unwrap_or(0),
                spec.tenant,
                spec.workspace,
                skills_json,
                key_root,
                spec.max_retries,
                spec.max_runtime_seconds,
                Option::<f64>::None, // scheduled_at
                spec.created_by,
                now,
                now, // ended_at — root ships pre-done (D-root-ended-at)
            ],
        )?;
        let root_payload = serde_json::json!({
            "assignee": root_assignee,
            "status": root_status,
            "parents": Vec::<String>::new(),
            "tenant": spec.tenant,
        });
        Self::append_event_internal(
            &tx,
            &root_id,
            None,
            KanbanEventKind::Created,
            Some(&root_payload),
            now,
        )?;

        // ---------------- Worker cards (status='todo', parents=[root]) ----------

        let worker_status = KanbanStatus::Todo.as_str();
        let mut worker_ids: Vec<String> = Vec::with_capacity(n);
        let worker_specs: &Vec<KanbanWorkerSpec> = &spec.workers;

        for (i, w) in worker_specs.iter().enumerate() {
            let wid = Self::new_id("t");
            let title = w
                .title
                .clone()
                .unwrap_or_else(|| format!("{} — worker {} of {}", spec.goal, i + 1, n));
            let body = w.body.clone().or_else(|| spec.body.clone());
            let key_worker = spec
                .idempotency_key
                .as_ref()
                .map(|k| format!("{k}:worker:{i}"));

            tx.execute(
                "INSERT INTO tasks \
                 (id, title, body, assignee, status, priority, tenant, workspace, skills, \
                  idempotency_key, consecutive_failures, max_retries, max_runtime_seconds, \
                  scheduled_at, created_by, created_at) \
                 VALUES \
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12, ?13, ?14, ?15)",
                params![
                    wid,
                    title,
                    body,
                    w.assignee,
                    worker_status,
                    spec.priority.unwrap_or(0),
                    spec.tenant,
                    spec.workspace,
                    skills_json,
                    key_worker,
                    spec.max_retries,
                    spec.max_runtime_seconds,
                    Option::<f64>::None, // scheduled_at
                    spec.created_by,
                    now,
                ],
            )?;
            let payload = serde_json::json!({
                "assignee": w.assignee,
                "status": worker_status,
                "parents": vec![root_id.clone()],
                "tenant": spec.tenant,
            });
            Self::append_event_internal(
                &tx,
                &wid,
                None,
                KanbanEventKind::Created,
                Some(&payload),
                now,
            )?;
            // Plain INSERT OR IGNORE — cycles impossible for freshly-minted IDs.
            tx.execute(
                "INSERT OR IGNORE INTO task_links (parent_id, child_id, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![&root_id, &wid, now],
            )?;
            worker_ids.push(wid);
        }

        // ---------------- Verifier card (optional) ------------------------------

        let verifier_id: Option<String> = if let Some(ref v_assignee) = spec.verifier {
            let vid = Self::new_id("t");
            tx.execute(
                "INSERT INTO tasks \
                 (id, title, body, assignee, status, priority, tenant, workspace, skills, \
                  idempotency_key, consecutive_failures, max_retries, max_runtime_seconds, \
                  scheduled_at, created_by, created_at) \
                 VALUES \
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12, ?13, ?14, ?15)",
                params![
                    vid,
                    format!("{} — verifier", spec.goal),
                    spec.body,
                    v_assignee,
                    worker_status,
                    spec.priority.unwrap_or(0),
                    spec.tenant,
                    spec.workspace,
                    skills_json,
                    key_verifier,
                    spec.max_retries,
                    spec.max_runtime_seconds,
                    Option::<f64>::None,
                    spec.created_by,
                    now,
                ],
            )?;
            let payload = serde_json::json!({
                "assignee": v_assignee,
                "status": worker_status,
                "parents": worker_ids.clone(),
                "tenant": spec.tenant,
            });
            Self::append_event_internal(
                &tx,
                &vid,
                None,
                KanbanEventKind::Created,
                Some(&payload),
                now,
            )?;
            for wid in &worker_ids {
                tx.execute(
                    "INSERT OR IGNORE INTO task_links (parent_id, child_id, created_at) \
                     VALUES (?1, ?2, ?3)",
                    params![wid, &vid, now],
                )?;
            }
            Some(vid)
        } else {
            None
        };

        // ---------------- Synthesizer card (optional) ---------------------------

        let synthesizer_id: Option<String> = if let Some(ref s_assignee) = spec.synthesizer {
            let sid = Self::new_id("t");
            let synth_parents: Vec<String> = match &verifier_id {
                Some(vid) => vec![vid.clone()],
                None => worker_ids.clone(),
            };
            tx.execute(
                "INSERT INTO tasks \
                 (id, title, body, assignee, status, priority, tenant, workspace, skills, \
                  idempotency_key, consecutive_failures, max_retries, max_runtime_seconds, \
                  scheduled_at, created_by, created_at) \
                 VALUES \
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12, ?13, ?14, ?15)",
                params![
                    sid,
                    format!("{} — synthesizer", spec.goal),
                    spec.body,
                    s_assignee,
                    worker_status,
                    spec.priority.unwrap_or(0),
                    spec.tenant,
                    spec.workspace,
                    skills_json,
                    key_synth,
                    spec.max_retries,
                    spec.max_runtime_seconds,
                    Option::<f64>::None,
                    spec.created_by,
                    now,
                ],
            )?;
            let payload = serde_json::json!({
                "assignee": s_assignee,
                "status": worker_status,
                "parents": synth_parents.clone(),
                "tenant": spec.tenant,
            });
            Self::append_event_internal(
                &tx,
                &sid,
                None,
                KanbanEventKind::Created,
                Some(&payload),
                now,
            )?;
            for pid in &synth_parents {
                tx.execute(
                    "INSERT OR IGNORE INTO task_links (parent_id, child_id, created_at) \
                     VALUES (?1, ?2, ?3)",
                    params![pid, &sid, now],
                )?;
            }
            Some(sid)
        } else {
            None
        };

        // ---------------- Blackboard task_comments row (optional) ---------------

        let blackboard_event_id: Option<i64> = if let Some(ref bb) = spec.blackboard {
            // "swarm" is a literal author sentinel — `task_comments.author` is a
            // free-text column with no validation (verified in RESEARCH §A1).
            let comment_id = Self::new_id("c");
            let bb_str = serde_json::to_string(bb)?;
            tx.execute(
                "INSERT INTO task_comments (id, task_id, author, body, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![comment_id, &root_id, "swarm", bb_str, now],
            )?;
            Some(tx.last_insert_rowid())
        } else {
            None
        };

        tx.commit()?;

        Ok(SwarmGraphIds {
            root_id,
            worker_ids,
            verifier_id,
            synthesizer_id,
            blackboard_event_id,
        })
    }

    // -----------------------------------------------------------------------
    // Phase 36.3.7.8 — @mention delegation store primitive
    // -----------------------------------------------------------------------

    /// Maximum ancestor hops the store-layer cycle check will walk (REQ-10 layer 2).
    ///
    /// Mirrors `mention::MAX_MENTION_CHAIN_DEPTH` — when the `mention` module
    /// (Plan 01) merges, this local constant will be unified via re-export.
    const MAX_MENTION_CHAIN_DEPTH: usize = 4;

    /// Find all mention children created under a given idempotency base key.
    ///
    /// Scans `tasks` where `idempotency_key LIKE '{base_key}:mention:%'`, parses
    /// the trailing `mention_index`, and returns children sorted by `mention_index`
    /// ascending (REQ-12 stability guarantee).
    ///
    /// Returns `Ok(None)` when no rows match — i.e., the batch was never created.
    ///
    /// # Handle field
    /// The `handle` field in [`MentionChildIds`] is not persisted as a separate
    /// column. On replay, `handle` is set to `""` (empty string). Callers that
    /// need the original-cased handle should store it in the task body or derive
    /// it from the `task_events` payload (where `"source":"mention"` rows carry
    /// `"handle"`). This trade-off avoids a schema column addition in this phase.
    pub fn find_mentions_by_idempotency_key(
        &self,
        base_key: &str,
    ) -> Result<Option<MentionResult>> {
        // WR-01 (Phase 36.3.7.8 code review): operator/LLM-supplied `base_key`
        // may contain `_` or `%`, which are LIKE wildcards by default in SQLite.
        // Escape them (and the escape char itself) and emit ESCAPE '\' so the
        // pattern matches the literal base_key only — preventing cross-batch
        // leakage where `my_batch_1` would otherwise match `myxbatchY1:mention:0`.
        let pattern = format!("{}:mention:%", like_escape(base_key));
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, idempotency_key, assignee \
                 FROM tasks \
                 WHERE idempotency_key LIKE ?1 ESCAPE '\\' \
                 ORDER BY idempotency_key",
            )
            .map_err(KanbanError::from)?;

        let rows: Vec<(String, String, String)> = stmt
            .query_map(params![pattern], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(KanbanError::from)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(KanbanError::from)?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut children: Vec<MentionChildIds> = Vec::with_capacity(rows.len());
        for (child_id, idem_key, assignee) in rows {
            // Parse mention_index from suffix: "{base_key}:mention:{i}"
            let mention_index: usize = idem_key
                .split(":mention:")
                .last()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            // handle is not persisted — see doc comment above.
            children.push(MentionChildIds {
                child_id,
                assignee,
                handle: String::new(),
                mention_index,
            });
        }

        // Sort by mention_index for REQ-12 stability (ORDER BY idem_key is
        // lexicographic, not numeric — e.g. ":mention:10" < ":mention:2").
        children.sort_by_key(|c| c.mention_index);

        Ok(Some(MentionResult { children }))
    }

    /// Materialize resolved @mention children as child cards atomically (Phase 36.3.7.8,
    /// REQ-10 layer 2, REQ-11, REQ-12).
    ///
    /// Sibling of [`create_swarm`](Self::create_swarm) — never re-enters [`create_task`](Self::create_task);
    /// INSERT statements are inlined directly inside the `BEGIN IMMEDIATE` transaction
    /// to avoid deadlock and keep atomicity.
    ///
    /// # Zero-children plan
    /// If `plan.children.is_empty()`, returns `Ok(MentionResult { children: vec![] })`
    /// without opening a transaction. This is a valid no-op (all mentions were skipped
    /// by the resolver), NOT an error — contrast with `create_swarm` which rejects empty
    /// workers.
    ///
    /// # Pre-flight
    /// Every `MentionChildSpec.assignee` is validated via
    /// `ironhermes_core::profile::validate_profile_name` BEFORE the transaction opens,
    /// minimising the lock window.
    ///
    /// # Idempotency (REQ-12, WR-01 carry-over from 36.3.7.7)
    /// The replay short-circuit lives INSIDE `BEGIN IMMEDIATE` so a concurrent
    /// first-create race resolves deterministically. If `plan.idempotency_key = Some("k")`,
    /// per-child keys are `"k:mention:{mention_index}"`.
    ///
    /// # Ancestor-chain cycle detection (REQ-10 layer 2)
    /// Before inserting each child link, a `WITH RECURSIVE ancestors` walk over
    /// `task_links` checks whether `spec.assignee` already appears in the ancestor
    /// chain of `plan.parent_id` up to `MAX_MENTION_CHAIN_DEPTH` hops. A match
    /// returns [`KanbanError::Other`] and the dropped `Transaction` auto-rolls back
    /// (RAII), producing zero rows (REQ-11).
    pub fn create_mention_children(
        &mut self,
        plan: MentionPlan,
    ) -> Result<MentionResult> {
        use rusqlite::TransactionBehavior;

        // ---------------- Empty-plan short-circuit (not an error) ---------------
        if plan.children.is_empty() {
            return Ok(MentionResult { children: vec![] });
        }

        // ---------------- Pre-flight: validate every assignee before tx ---------
        for spec in &plan.children {
            ironhermes_core::profile::validate_profile_name(&spec.assignee)
                .map_err(|e| KanbanError::Other(anyhow::anyhow!("invalid assignee: {e}")))?;
        }

        // Workspace validation (D-31 / Pitfall 6).
        if let Some(ref ws) = plan.workspace {
            validate_dir_workspace(ws)?;
        }

        // ---------------- Open transaction (BEGIN IMMEDIATE) --------------------
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        // ---------------- Idempotency replay INSIDE transaction (WR-01, CR-01, WR-03) ---
        //
        // WR-01 (Phase 36.3.7.7 code review carry-over): the lookup MUST happen
        // inside BEGIN IMMEDIATE so a concurrent first-create race resolves
        // deterministically. If T1 commits its first-child row first, T2 (which
        // blocked on BEGIN IMMEDIATE) then observes the row and replays instead of
        // trying its own INSERT and failing with a UNIQUE-constraint error.
        //
        // CR-01 (Phase 36.3.7.8 code review): scan for ANY persisted child under
        // this batch using the LIKE pattern (escaped per WR-01 of 36.3.7.8) instead
        // of probing only `:mention:0`. The first surviving MentionChildSpec may
        // have `mention_index >= 1` if the resolver skipped index 0 (malformed,
        // self-reference, etc.), so a `:mention:0` row may never exist.
        //
        // WR-03 (Phase 36.3.7.8 code review): the replay reconstruction is inlined
        // directly inside this `&tx` borrow — no commit-then-reborrow dance, no
        // second BEGIN IMMEDIATE, no defensive fall-through. This keeps the entire
        // existence-check-plus-reconstruction inside a single atomic boundary.
        if let Some(ref base) = plan.idempotency_key {
            let pattern = format!("{}:mention:%", like_escape(base));
            let rows: Vec<(String, String, String)> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT id, idempotency_key, assignee \
                         FROM tasks \
                         WHERE idempotency_key LIKE ?1 ESCAPE '\\' \
                         ORDER BY idempotency_key",
                    )
                    .map_err(KanbanError::from)?;
                let collected: rusqlite::Result<Vec<(String, String, String)>> = stmt
                    .query_map(params![pattern], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(KanbanError::from)?
                    .collect();
                collected.map_err(KanbanError::from)?
            };

            if !rows.is_empty() {
                // Reconstruct MentionResult identically to
                // find_mentions_by_idempotency_key.
                let mut children: Vec<MentionChildIds> = Vec::with_capacity(rows.len());
                for (child_id, idem_key, assignee) in rows {
                    let mention_index: usize = idem_key
                        .split(":mention:")
                        .last()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    children.push(MentionChildIds {
                        child_id,
                        assignee,
                        handle: String::new(),
                        mention_index,
                    });
                }
                children.sort_by_key(|c| c.mention_index);
                tx.commit()?;
                return Ok(MentionResult { children });
            }
        }

        Self::mention_insert_loop(tx, plan)
    }

    /// Inner INSERT loop for [`create_mention_children`](Self::create_mention_children).
    ///
    /// Free function (not `&mut self`) so the borrow checker allows passing a
    /// `Transaction<'_>` (which borrows `conn`) without conflicting with `&mut self`.
    fn mention_insert_loop(
        tx: rusqlite::Transaction<'_>,
        plan: MentionPlan,
    ) -> Result<MentionResult> {
        let now = Self::now();
        let skills_json: Option<String> = plan
            .skills
            .as_ref()
            .map(|v| serde_json::to_string(v))
            .transpose()?;

        let mut children: Vec<MentionChildIds> = Vec::with_capacity(plan.children.len());

        for spec in &plan.children {
            // ---- REQ-10 layer 2: ancestor-chain assignee-identity cycle check ----
            //
            // Walk PARENTS of plan.parent_id up to MAX_MENTION_CHAIN_DEPTH hops.
            // Reject if `spec.assignee` already appears as any ancestor task's
            // assignee. This catches the case where the resolver's layer-1
            // self-reference check missed a transitive cycle (legitimate layer-2
            // scenario). Dropping `tx` on error auto-rolls back (RAII → REQ-11).
            //
            // Bind order: ?1 = plan.parent_id, ?2 = depth limit, ?3 = assignee.
            let cycle: Option<i64> = tx
                .query_row(
                    "WITH RECURSIVE ancestors(id, depth) AS ( \
                        SELECT parent_id, 1 FROM task_links WHERE child_id = ?1 \
                        UNION \
                        SELECT tl.parent_id, a.depth + 1 \
                          FROM task_links tl \
                          JOIN ancestors a ON tl.child_id = a.id \
                         WHERE a.depth < ?2 \
                     ) \
                     SELECT 1 FROM tasks t \
                      WHERE t.id IN (SELECT id FROM ancestors) \
                        AND t.assignee = ?3 \
                      LIMIT 1",
                    params![
                        &plan.parent_id,
                        Self::MAX_MENTION_CHAIN_DEPTH as i64,
                        &spec.assignee
                    ],
                    |r| r.get(0),
                )
                .optional()
                .map_err(KanbanError::from)?;

            if cycle.is_some() {
                // Drop of `tx` triggers rusqlite RAII rollback → zero rows (REQ-11).
                return Err(KanbanError::Other(anyhow::anyhow!(
                    "mention_cycle: assignee {} appears in ancestor chain of {}",
                    spec.assignee,
                    plan.parent_id
                )));
            }

            // ---- Compute child ID and per-child idempotency key -----------------
            let child_id = Self::new_id("t");
            let child_key: Option<String> = plan
                .idempotency_key
                .as_ref()
                .map(|k| format!("{k}:mention:{}", spec.mention_index));

            // ---- INSERT INTO tasks (15-column shape from create_task:246-270) ---
            //
            // Column order mirrors create_task exactly (INV-36.3.7):
            //   id, title, body, assignee, status='todo', priority=0, tenant,
            //   workspace, skills, idempotency_key, consecutive_failures=0,
            //   max_retries=NULL, max_runtime_seconds=NULL, scheduled_at=NULL,
            //   created_by, created_at
            //
            // We do NOT call create_task re-entrantly (D-no-create-task-reuse):
            // that would break the BEGIN IMMEDIATE boundary.
            tx.execute(
                "INSERT INTO tasks \
                 (id, title, body, assignee, status, priority, tenant, workspace, skills, \
                  idempotency_key, consecutive_failures, max_retries, max_runtime_seconds, \
                  scheduled_at, created_by, created_at) \
                 VALUES \
                 (?1, ?2, ?3, ?4, 'todo', 0, ?5, ?6, ?7, ?8, 0, NULL, NULL, NULL, ?9, ?10)",
                params![
                    child_id,
                    spec.title,
                    spec.body,
                    spec.assignee,
                    plan.tenant,
                    plan.workspace,
                    skills_json,
                    child_key,
                    plan.created_by,
                    now,
                ],
            )
            .map_err(KanbanError::from)?;

            // ---- INSERT INTO task_links (inline — calling insert_link_checked ----
            //      would open its own tx → deadlock).
            tx.execute(
                "INSERT OR IGNORE INTO task_links (parent_id, child_id, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![&plan.parent_id, &child_id, now],
            )
            .map_err(KanbanError::from)?;

            // ---- Append Created event (mention-specific payload) ----------------
            let payload = serde_json::json!({
                "source": "mention",
                "handle": &spec.handle,
                "mention_index": spec.mention_index,
                "parent_id": &plan.parent_id,
            });
            Self::append_event_internal(
                &tx,
                &child_id,
                None,
                KanbanEventKind::Created,
                Some(&payload),
                now,
            )?;

            children.push(MentionChildIds {
                child_id,
                assignee: spec.assignee.clone(),
                handle: spec.handle.clone(),
                mention_index: spec.mention_index,
            });
        }

        tx.commit()?;
        Ok(MentionResult { children })
    }

    /// Append an event row for a task and return the new event id.
    pub fn append_event(
        &mut self,
        task_id: &str,
        run_id: Option<&str>,
        kind: KanbanEventKind,
        payload: Option<&Value>,
    ) -> Result<i64> {
        let now = Self::now();
        Self::append_event_internal(&self.conn, task_id, run_id, kind, payload, now)
    }

    // -----------------------------------------------------------------------
    // Subscriptions CRUD (Phase 36.3.7.5 BUG-36.3.7.5-02)
    // -----------------------------------------------------------------------
    //
    // Five methods on `KanbanStore` that the gateway notifier loop (Plan 02),
    // the gateway runner's spawn gate (Plan 03), and the auto-subscribe hook +
    // 3 CLI verbs (Plan 04) all consume. `thread_id: Option<&str>` is the
    // caller-facing form; `None` is substituted with `""` at the SQL boundary
    // because the schema's UNIQUE constraint would otherwise treat `NULL` as
    // distinct (locked CONTEXT decision). All methods propagate UNIQUE / CHECK
    // violations as `KanbanError::Db(rusqlite::Error)` via `?`.

    /// Append a `kanban_subscriptions` row and return its new `id`.
    ///
    /// `thread_id` `None` is stored as `''` per the locked empty-string
    /// substitution. Returns `Err(KanbanError::Db(_))` on a UNIQUE violation
    /// (duplicate `(task_id, platform, chat_id, thread_id)` tuple) or a CHECK
    /// violation (source not in `('auto', 'explicit')`). (Phase 36.3.7.5 BUG-36.3.7.5-02)
    pub fn append_subscription(
        &mut self,
        task_id: &str,
        platform: &str,
        chat_id: &str,
        thread_id: Option<&str>,
        source: &str,
    ) -> Result<i64> {
        let thread = thread_id.unwrap_or("");
        let now = Self::now();
        self.conn.execute(
            "INSERT INTO kanban_subscriptions \
             (task_id, platform, chat_id, thread_id, source, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![task_id, platform, chat_id, thread, source, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List all subscriptions for a task, ordered by `id ASC` (deterministic).
    ///
    /// Returns an empty `Vec` when no rows match. (Phase 36.3.7.5 BUG-36.3.7.5-02)
    pub fn list_subscriptions_for_task(&self, task_id: &str) -> Result<Vec<Subscription>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, platform, chat_id, thread_id, source, created_at \
             FROM kanban_subscriptions WHERE task_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![task_id], |row| {
            Ok(Subscription {
                id: row.get(0)?,
                task_id: row.get(1)?,
                platform: row.get(2)?,
                chat_id: row.get(3)?,
                thread_id: row.get(4)?,
                source: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// List subscriptions for a `(platform, chat_id, thread_id)` triple with
    /// strict thread-id filtering.
    ///
    /// `thread_id` semantics:
    /// - `None` → matches rows where `thread_id = ''` (the empty-string default
    ///   stored when callers pass `None` to `append_subscription`).
    /// - `Some(t)` → matches rows where `thread_id = t` exactly.
    ///
    /// Empty string and `"7"` are therefore distinct keys, matching the UNIQUE
    /// constraint's view of the world. Ordered by `id ASC`. (Phase 36.3.7.5 BUG-36.3.7.5-02)
    pub fn list_subscriptions_for_chat(
        &self,
        platform: &str,
        chat_id: &str,
        thread_id: Option<&str>,
    ) -> Result<Vec<Subscription>> {
        let row_to_sub = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Subscription> {
            Ok(Subscription {
                id: row.get(0)?,
                task_id: row.get(1)?,
                platform: row.get(2)?,
                chat_id: row.get(3)?,
                thread_id: row.get(4)?,
                source: row.get(5)?,
                created_at: row.get(6)?,
            })
        };
        let mut out = Vec::new();
        match thread_id {
            Some(t) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, task_id, platform, chat_id, thread_id, source, created_at \
                     FROM kanban_subscriptions \
                     WHERE platform = ?1 AND chat_id = ?2 AND thread_id = ?3 \
                     ORDER BY id ASC",
                )?;
                let rows = stmt.query_map(params![platform, chat_id, t], row_to_sub)?;
                for r in rows {
                    out.push(r?);
                }
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, task_id, platform, chat_id, thread_id, source, created_at \
                     FROM kanban_subscriptions \
                     WHERE platform = ?1 AND chat_id = ?2 AND thread_id = '' \
                     ORDER BY id ASC",
                )?;
                let rows = stmt.query_map(params![platform, chat_id], row_to_sub)?;
                for r in rows {
                    out.push(r?);
                }
            }
        }
        Ok(out)
    }

    /// Remove subscription rows for a task, optionally filtered by platform
    /// and / or chat_id. Returns the number of rows deleted.
    ///
    /// Filter cases:
    /// - `(task, None, None)` → delete ALL rows for the task.
    /// - `(task, Some(p), None)` → delete rows for `task AND platform=p`.
    /// - `(task, None, Some(c))` → delete rows for `task AND chat_id=c`.
    /// - `(task, Some(p), Some(c))` → delete rows for `task AND platform=p AND chat_id=c`.
    ///
    /// Returns `0` when nothing matched. (Phase 36.3.7.5 BUG-36.3.7.5-02)
    pub fn remove_subscriptions(
        &mut self,
        task_id: &str,
        platform: Option<&str>,
        chat_id: Option<&str>,
    ) -> Result<usize> {
        let n = match (platform, chat_id) {
            (None, None) => self.conn.execute(
                "DELETE FROM kanban_subscriptions WHERE task_id = ?1",
                params![task_id],
            )?,
            (Some(p), None) => self.conn.execute(
                "DELETE FROM kanban_subscriptions WHERE task_id = ?1 AND platform = ?2",
                params![task_id, p],
            )?,
            (None, Some(c)) => self.conn.execute(
                "DELETE FROM kanban_subscriptions WHERE task_id = ?1 AND chat_id = ?2",
                params![task_id, c],
            )?,
            (Some(p), Some(c)) => self.conn.execute(
                "DELETE FROM kanban_subscriptions \
                 WHERE task_id = ?1 AND platform = ?2 AND chat_id = ?3",
                params![task_id, p, c],
            )?,
        };
        Ok(n)
    }

    /// Convenience alias: delete ALL subscription rows for a task.
    ///
    /// Equivalent to `remove_subscriptions(task_id, None, None)`. The notifier
    /// loop (Plan 02) calls this after delivering the terminal-event message
    /// to every subscriber. (Phase 36.3.7.5 BUG-36.3.7.5-02)
    pub fn remove_all_subscriptions_for_task(&mut self, task_id: &str) -> Result<usize> {
        self.remove_subscriptions(task_id, None, None)
    }

    /// List ALL subscription rows across every task, ordered by `id ASC`.
    ///
    /// Supports `cmd_notify_list` without a task filter (the operator-side
    /// "show me everything" view). Phase 36.3.7.5 BUG-36.3.7.5-05 — supports
    /// cmd_notify_list without a task filter.
    pub fn list_all_subscriptions(&self) -> Result<Vec<Subscription>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, platform, chat_id, thread_id, source, created_at \
             FROM kanban_subscriptions ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Subscription {
                id: row.get(0)?,
                task_id: row.get(1)?,
                platform: row.get(2)?,
                chat_id: row.get(3)?,
                thread_id: row.get(4)?,
                source: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Return the existing task whose `idempotency_key` matches, or `None`.
    pub fn find_by_idempotency_key(&self, key: &str) -> Result<Option<Task>> {
        self.conn
            .query_row(
                "SELECT id, title, body, assignee, status, priority, tenant, workspace, skills, \
                 idempotency_key, claim_lock, claim_expires, current_run_id, consecutive_failures, \
                 max_retries, max_runtime_seconds, scheduled_at, workflow_template_id, \
                 current_step_key, created_by, created_at, started_at, ended_at \
                 FROM tasks WHERE idempotency_key = ?1",
                params![key],
                Self::row_to_task,
            )
            .optional()
            .map_err(KanbanError::from)
    }

    // -----------------------------------------------------------------------
    // Lifecycle mutations
    // -----------------------------------------------------------------------

    /// Complete a task (D-22 dual-gate + created_cards + hallucinated_ref scan).
    ///
    /// Runs inside a `BEGIN IMMEDIATE` transaction for the `expected_run_id`
    /// assertion + status update.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_task(
        &mut self,
        task_id: &str,
        summary: Option<&str>,
        metadata: Option<&Value>,
        result: Option<&str>,
        expected_run_id: Option<&str>,
        created_cards: Option<&[String]>,
        current_profile: &str,
    ) -> Result<()> {
        use rusqlite::TransactionBehavior;

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Self::now();

        // (a) Load task.
        let task: Task = tx
            .query_row(
                "SELECT id, title, body, assignee, status, priority, tenant, workspace, skills, \
                 idempotency_key, claim_lock, claim_expires, current_run_id, consecutive_failures, \
                 max_retries, max_runtime_seconds, scheduled_at, workflow_template_id, \
                 current_step_key, created_by, created_at, started_at, ended_at \
                 FROM tasks WHERE id = ?1",
                params![task_id],
                Self::row_to_task,
            )
            .optional()?
            .ok_or_else(|| KanbanError::TaskNotFound(task_id.to_string()))?;

        // (b) expected_run_id gate (D-22, D-41).
        if let Some(eid) = expected_run_id {
            crate::cas::assert_run_id_tx(&tx, task_id, eid)?;
        }

        // (c) created_cards validation (D-22).
        if let Some(cards) = created_cards {
            let mut phantom_ids: Vec<String> = Vec::new();
            let mut wrong_profile_ids: Vec<String> = Vec::new();

            for card_id in cards {
                let row: Option<Option<String>> = tx
                    .query_row(
                        "SELECT created_by FROM tasks WHERE id = ?1",
                        params![card_id],
                        |r| r.get(0),
                    )
                    .optional()?;

                match row {
                    None => phantom_ids.push(card_id.clone()),
                    Some(created_by) => {
                        if created_by.as_deref() != Some(current_profile) {
                            wrong_profile_ids.push(card_id.clone());
                        }
                    }
                }
            }

            if !phantom_ids.is_empty() || !wrong_profile_ids.is_empty() {
                // Permanent completion_rejected event (D-22).
                let payload = serde_json::json!({
                    "phantom_ids": phantom_ids,
                    "wrong_profile_ids": wrong_profile_ids,
                });
                tx.execute(
                    "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) \
                     VALUES (?1, ?2, 'completion_rejected', ?3, ?4)",
                    params![task_id, task.current_run_id, payload.to_string(), now],
                )?;
                tx.commit()?;
                return Err(KanbanError::CreatedCardsRejected {
                    phantom: phantom_ids,
                    wrong_profile: wrong_profile_ids,
                });
            }
        }

        // (d) Advisory free-form prose scan for unresolved t_<hex> references.
        if let Some(s) = summary {
            let re = regex::Regex::new(r"t_[0-9a-f]{8,}").unwrap();
            let unresolved: Vec<String> = re
                .find_iter(s)
                .map(|m| m.as_str().to_string())
                .filter(|id| {
                    tx.query_row(
                        "SELECT COUNT(*) FROM tasks WHERE id = ?1",
                        params![id],
                        |r| r.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                        == 0
                })
                .collect();

            if !unresolved.is_empty() {
                let payload = serde_json::json!({ "unresolved_ids": unresolved });
                tx.execute(
                    "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) \
                     VALUES (?1, ?2, 'hallucinated_ref', ?3, ?4)",
                    params![task_id, task.current_run_id, payload.to_string(), now],
                )?;
            }
        }

        // (e) Set status = 'done', ended_at = now.
        tx.execute(
            "UPDATE tasks SET status='done', ended_at=?1 WHERE id=?2",
            params![now, task_id],
        )?;

        // (f) Close current run row.
        let metadata_str = metadata.map(|v| v.to_string());
        if let Some(ref run_id) = task.current_run_id {
            tx.execute(
                "UPDATE task_runs SET outcome='completed', summary=?1, metadata=?2, ended_at=?3 \
                 WHERE id=?4",
                params![summary, metadata_str, now, run_id],
            )?;
        }

        // (g) Synthesize a zero-duration run if task was never claimed.
        if task.current_run_id.is_none()
            && (summary.is_some() || metadata.is_some() || result.is_some())
        {
            let synth_id = format!("r_{}", uuid::Uuid::new_v4().simple());
            tx.execute(
                "INSERT INTO task_runs (id, task_id, claim_lock, started_at, ended_at, \
                 outcome, summary, metadata) \
                 VALUES (?1, ?2, 'synthetic', ?3, ?3, 'completed', ?4, ?5)",
                params![synth_id, task_id, now, summary, metadata_str],
            )?;
        }

        // (h) Clear current_run_id.
        tx.execute(
            "UPDATE tasks SET current_run_id=NULL WHERE id=?1",
            params![task_id],
        )?;

        // (i) Append completed event.
        let result_len = result.map(|r| r.len()).unwrap_or(0);
        let summary_preview = summary
            .map(|s| s.lines().next().unwrap_or("").chars().take(400).collect::<String>())
            .unwrap_or_default();
        let payload = serde_json::json!({
            "result_len": result_len,
            "summary": summary_preview,
        });
        tx.execute(
            "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) \
             VALUES (?1, ?2, 'completed', ?3, ?4)",
            params![task_id, task.current_run_id, payload.to_string(), now],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Block a task with a reason (D-23 / D-22 expected_run_id gate).
    pub fn block_task(
        &mut self,
        task_id: &str,
        reason: &str,
        expected_run_id: Option<&str>,
    ) -> Result<()> {
        use rusqlite::TransactionBehavior;

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Self::now();

        let task: Task = tx
            .query_row(
                "SELECT id, title, body, assignee, status, priority, tenant, workspace, skills, \
                 idempotency_key, claim_lock, claim_expires, current_run_id, consecutive_failures, \
                 max_retries, max_runtime_seconds, scheduled_at, workflow_template_id, \
                 current_step_key, created_by, created_at, started_at, ended_at \
                 FROM tasks WHERE id = ?1",
                params![task_id],
                Self::row_to_task,
            )
            .optional()?
            .ok_or_else(|| KanbanError::TaskNotFound(task_id.to_string()))?;

        if let Some(eid) = expected_run_id {
            crate::cas::assert_run_id_tx(&tx, task_id, eid)?;
        }

        tx.execute(
            "UPDATE tasks SET status='blocked' WHERE id=?1",
            params![task_id],
        )?;

        if let Some(ref run_id) = task.current_run_id {
            tx.execute(
                "UPDATE task_runs SET outcome='blocked', error=?1, ended_at=?2 WHERE id=?3",
                params![reason, now, run_id],
            )?;
        }

        // Synthesize run if never claimed but summary/reason provided.
        if task.current_run_id.is_none() {
            let synth_id = format!("r_{}", uuid::Uuid::new_v4().simple());
            tx.execute(
                "INSERT INTO task_runs (id, task_id, claim_lock, started_at, ended_at, \
                 outcome, error) \
                 VALUES (?1, ?2, 'synthetic', ?3, ?3, 'blocked', ?4)",
                params![synth_id, task_id, now, reason],
            )?;
        }

        tx.execute(
            "UPDATE tasks SET current_run_id=NULL WHERE id=?1",
            params![task_id],
        )?;

        let payload = serde_json::json!({ "reason": reason });
        tx.execute(
            "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) \
             VALUES (?1, ?2, 'blocked', ?3, ?4)",
            params![task_id, task.current_run_id, payload.to_string(), now],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Unblock a task — resets to `ready`, clears claim fields.
    pub fn unblock_task(&mut self, task_id: &str) -> Result<()> {
        let now = Self::now();
        self.conn.execute(
            "UPDATE tasks SET status='ready', claim_lock=NULL, current_run_id=NULL, \
             claim_expires=NULL WHERE id=?1",
            params![task_id],
        )?;
        Self::append_event_internal(&self.conn, task_id, None, KanbanEventKind::Unblocked, None, now)?;
        Ok(())
    }

    /// Archive a task. If the task is currently `running`, closes the active
    /// run with outcome='reclaimed' and emits a `reclaimed` event first
    /// (reference.md "Reclaimed runs from status changes").
    pub fn archive_task(&mut self, task_id: &str) -> Result<()> {
        let now = Self::now();
        let task = self.get_task(task_id)?;

        if task.status == KanbanStatus::Running.as_str() {
            // Close current run as reclaimed.
            if let Some(ref run_id) = task.current_run_id {
                self.conn.execute(
                    "UPDATE task_runs SET outcome='reclaimed', ended_at=?1 WHERE id=?2",
                    params![now, run_id],
                )?;
            }
            let payload = serde_json::json!({ "stale_lock": task.claim_lock });
            Self::append_event_internal(
                &self.conn,
                task_id,
                task.current_run_id.as_deref(),
                KanbanEventKind::Reclaimed,
                Some(&payload),
                now,
            )?;
        }

        self.conn.execute(
            "UPDATE tasks SET status='archived', claim_lock=NULL, current_run_id=NULL, \
             claim_expires=NULL WHERE id=?1",
            params![task_id],
        )?;
        Self::append_event_internal(&self.conn, task_id, None, KanbanEventKind::Archived, None, now)?;
        Ok(())
    }

    /// Assign a task to a new profile (validates via `validate_profile_name`).
    pub fn assign_task(&mut self, task_id: &str, new_assignee: &str) -> Result<()> {
        ironhermes_core::profile::validate_profile_name(new_assignee)
            .map_err(|e| KanbanError::Other(anyhow::anyhow!("invalid assignee: {e}")))?;
        let now = Self::now();
        self.conn.execute(
            "UPDATE tasks SET assignee=?1 WHERE id=?2",
            params![new_assignee, task_id],
        )?;
        let payload = serde_json::json!({ "assignee": new_assignee });
        Self::append_event_internal(
            &self.conn,
            task_id,
            None,
            KanbanEventKind::Assigned,
            Some(&payload),
            now,
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Phase 36.3.7.5 BUG-36.3.7.5-03 — notifier helpers
    // -----------------------------------------------------------------------
    //
    // Two read-only helpers consumed by the gateway notifier loop
    // (`notifier::run_notifier_tick` + `notifier::init_watermark`). They are
    // shaped to keep the notifier module FREE of raw rusqlite access — the
    // store owns the connection; the notifier owns the polling logic.

    /// Return all `task_events` rows whose `id > watermark` AND whose `kind`
    /// is in the notifier's terminal set (`completed`, `blocked`, `gave_up`,
    /// `crashed`, `timed_out`). Ordered by `id ASC` so the notifier's watermark
    /// advance is monotonic. (Phase 36.3.7.5 BUG-36.3.7.5-03 — notifier helper)
    pub fn list_terminal_events_after(
        &self,
        watermark: i64,
    ) -> Result<Vec<crate::events::KanbanEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, run_id, kind, payload, created_at \
             FROM task_events \
             WHERE id > ?1 \
               AND kind IN ('completed', 'blocked', 'gave_up', 'crashed', 'timed_out') \
             ORDER BY id ASC",
        )?;
        let events = stmt
            .query_map(params![watermark], |r| {
                Ok(crate::events::KanbanEvent {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    run_id: r.get(2)?,
                    kind: r.get(3)?,
                    payload: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(events)
    }

    /// Return `MAX(id) FROM task_events`, or `0` when the table is empty.
    ///
    /// Used by the notifier loop at startup to initialize its in-memory
    /// watermark (locked CONTEXT decision: in-memory only; gateway-downtime
    /// loss accepted for v1). (Phase 36.3.7.5 BUG-36.3.7.5-03 — notifier helper)
    pub fn max_event_id(&self) -> Result<i64> {
        let max: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM task_events", [], |r| {
                r.get(0)
            })?;
        Ok(max)
    }

    /// Return all events for a task ordered by id (insertion order).
    pub fn get_events(&self, task_id: &str) -> Result<Vec<crate::events::KanbanEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, run_id, kind, payload, created_at \
             FROM task_events WHERE task_id = ?1 ORDER BY id ASC",
        )?;
        let events = stmt
            .query_map(params![task_id], |r| {
                Ok(crate::events::KanbanEvent {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    run_id: r.get(2)?,
                    kind: r.get(3)?,
                    payload: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(events)
    }

    /// Return all runs for a task ordered by started_at.
    pub fn get_runs(&self, task_id: &str) -> Result<Vec<TaskRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, claim_lock, claim_pid, started_at, ended_at, \
             outcome, summary, metadata, error, log_path \
             FROM task_runs WHERE task_id = ?1 ORDER BY started_at ASC",
        )?;
        let runs = stmt
            .query_map(params![task_id], |r| {
                Ok(TaskRun {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    claim_lock: r.get(2)?,
                    claim_pid: r.get(3)?,
                    started_at: r.get(4)?,
                    ended_at: r.get(5)?,
                    outcome: r.get(6)?,
                    summary: r.get(7)?,
                    metadata: r.get(8)?,
                    error: r.get(9)?,
                    log_path: r.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(runs)
    }

    // ---------------------------------------------------------------------------
    // Phase 36.3.7.9 Plan 03 — boards rm pre-flight (D-07)
    // ---------------------------------------------------------------------------

    /// Count tasks in open statuses (`todo`, `ready`, `in_progress`).
    ///
    /// Used by `cmd_boards_rm --delete` for the D-07 pre-flight check: refuses
    /// hard-delete if the count is > 0. Keeping the SQL inside the kanban crate
    /// preserves rusqlite isolation from ironhermes-cli.
    pub fn open_tasks_count(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE status IN ('todo','ready','in_progress')",
            [],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    // ---------------------------------------------------------------------------
    // Phase 36.3.7.10 — decomposer store helpers
    // ---------------------------------------------------------------------------

    /// Specify a triage task in-place: rewrite title + body, promote to `todo`.
    ///
    /// SQL executed inside a single `BEGIN IMMEDIATE` transaction:
    /// ```sql
    /// UPDATE tasks SET title=?, body=?, status='todo'
    ///     WHERE id=? AND status='triage';
    /// INSERT INTO task_events … kind='specified' …;
    /// ```
    ///
    /// Returns `true` when the status guard matched (task promoted), `false`
    /// when `rows_affected == 0` (already processed — idempotent double-call).
    pub fn apply_specify(
        &mut self,
        task_id: &str,
        new_title: &str,
        new_body: &str,
    ) -> Result<bool> {
        use rusqlite::TransactionBehavior;

        let now = Self::now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Status-guarded UPDATE — the WHERE clause is the single-writer gate.
        let rows = tx.execute(
            "UPDATE tasks SET title = ?1, body = ?2, status = 'todo' \
             WHERE id = ?3 AND status = 'triage'",
            params![new_title, new_body, task_id],
        )?;

        if rows == 0 {
            // Already processed — rollback (no-op) and signal caller.
            tx.rollback()?;
            return Ok(false);
        }

        // Append the `specified` event inside the same transaction.
        let payload = serde_json::json!({ "new_title": new_title });
        Self::append_event_internal(
            &tx,
            task_id,
            None,
            crate::events::KanbanEventKind::Specified,
            Some(&payload),
            now,
        )?;

        tx.commit()?;
        Ok(true)
    }

    /// Decompose a triage task: rewrite title + body, promote to `todo`,
    /// insert child tasks, and insert `task_links` rows — all atomically.
    ///
    /// SQL executed inside a single `BEGIN IMMEDIATE` transaction:
    /// ```sql
    /// UPDATE tasks SET title=?, body=?, status='todo'
    ///     WHERE id=? AND status='triage';
    /// INSERT INTO task_events … kind='decomposed' …;
    /// INSERT INTO tasks … status='todo' … (per child);
    /// INSERT INTO task_links (parent_id, child_id, …) (per child);
    /// ```
    ///
    /// # Assignee fallback cascade (RESEARCH §Q5)
    ///
    /// When `child.assignee` is empty:
    /// 1. Fall back to `config.default_assignee`.
    /// 2. If that is also empty, fall back to `parent_assignee`.
    ///
    /// A child with `assignee = ""` never reaches the DB.
    ///
    /// Returns [`crate::decomposer::DecomposedIds::already_processed`] when the
    /// status guard observes 0 rows affected (idempotent double-call).
    pub fn apply_decompose(
        &mut self,
        parent_id: &str,
        new_title: &str,
        new_body: &str,
        children: &[crate::decomposer::ChildSpec],
        parent_assignee: &str,
        config: &crate::config::KanbanConfig,
    ) -> Result<crate::decomposer::DecomposedIds> {
        use rusqlite::TransactionBehavior;

        let now = Self::now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Status-guarded parent UPDATE.
        let rows = tx.execute(
            "UPDATE tasks SET title = ?1, body = ?2, status = 'todo' \
             WHERE id = ?3 AND status = 'triage'",
            params![new_title, new_body, parent_id],
        )?;

        if rows == 0 {
            tx.rollback()?;
            return Ok(crate::decomposer::DecomposedIds::already_processed());
        }

        // WR-03 fix: propagate parent's tenant to children. The previous
        // INSERT hard-coded `tenant = NULL`, which left decomposed children
        // outside any tenant-scoped list/dispatch queries and bypassed
        // `KanbanError::TenantMismatch` for downstream link operations.
        let parent_tenant: Option<String> = tx
            .query_row(
                "SELECT tenant FROM tasks WHERE id = ?1",
                params![parent_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(None);

        // Insert child tasks and links.
        let mut child_ids: Vec<String> = Vec::with_capacity(children.len());

        for child in children {
            let child_id = Self::new_id("t");

            // Assignee fallback cascade: child → config.default_assignee → parent.
            let assignee = if !child.assignee.is_empty() {
                child.assignee.clone()
            } else if !config.default_assignee.is_empty() {
                config.default_assignee.clone()
            } else {
                parent_assignee.to_string()
            };

            // WR-04 fix: post-cascade emptiness guard. If child/config/parent
            // are all empty, the dispatcher will never claim the child (it
            // matches no worker profile) and the task stalls silently in
            // `todo`. The rustdoc on `ChildSpec::assignee` already promises
            // "empty string must never reach the DB" — enforce it here.
            if assignee.is_empty() {
                tx.rollback()?;
                return Err(crate::error::KanbanError::Other(anyhow::anyhow!(
                    "decompose: child '{}' has no resolvable assignee \
                     (child/config/parent all empty)",
                    child.title
                )));
            }

            tx.execute(
                "INSERT INTO tasks \
                 (id, title, body, assignee, status, priority, tenant, workspace, skills, \
                  consecutive_failures, created_by, created_at) \
                 VALUES \
                 (?1, ?2, ?3, ?4, 'todo', 0, ?5, NULL, NULL, 0, ?6, ?7)",
                params![child_id, child.title, child.body, assignee, parent_tenant, parent_id, now],
            )?;

            // Child creation event.
            let child_payload = serde_json::json!({
                "assignee": assignee,
                "status": "todo",
                "parents": [parent_id],
            });
            Self::append_event_internal(
                &tx,
                &child_id,
                None,
                crate::events::KanbanEventKind::Created,
                Some(&child_payload),
                now,
            )?;

            // Parent → child link.
            tx.execute(
                "INSERT OR IGNORE INTO task_links (parent_id, child_id, created_at) \
                 VALUES (?1, ?2, ?3)",
                params![parent_id, &child_id, now],
            )?;

            child_ids.push(child_id);
        }

        // Decomposed event on the parent.
        let decomposed_payload = serde_json::json!({
            "child_count": children.len(),
            "child_ids": child_ids,
        });
        Self::append_event_internal(
            &tx,
            parent_id,
            None,
            crate::events::KanbanEventKind::Decomposed,
            Some(&decomposed_payload),
            now,
        )?;

        tx.commit()?;

        Ok(crate::decomposer::DecomposedIds {
            child_ids,
            root_promoted: true,
        })
    }

    /// Read accessor for `task_links` rows by parent ID.
    ///
    /// Used by the receiver tests in `tests/decomposer_logic.rs` to verify
    /// that child links were inserted atomically. Returns all links where
    /// `parent_id = ?`. (Phase 36.3.7.10 — read accessor for receiver tests)
    pub fn list_links_for_parent(&self, parent_id: &str) -> Result<Vec<crate::types::TaskLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT parent_id, child_id, created_at FROM task_links WHERE parent_id = ?1 ORDER BY child_id ASC",
        )?;
        let links = stmt
            .query_map(params![parent_id], |r| {
                Ok(crate::types::TaskLink {
                    parent_id: r.get(0)?,
                    child_id: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(links)
    }
}

// ---------------------------------------------------------------------------
// Phase 36.3.7.9 Plan 03 — open_tasks_count unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod open_tasks_count_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_tasks_count_returns_zero_on_fresh_store() {
        let dir = TempDir::new().unwrap();
        let store = KanbanStore::open(dir.path().join("test.db")).unwrap();
        assert_eq!(store.open_tasks_count().unwrap(), 0);
    }

    #[test]
    fn open_tasks_count_returns_n_after_seeding_tasks() {
        let dir = TempDir::new().unwrap();
        let mut store = KanbanStore::open(dir.path().join("test.db")).unwrap();

        // Seed 3 tasks in open statuses
        for i in 0..3 {
            store
                .create_task(
                    &format!("Task {}", i),
                    "alice",
                    CreateTaskOptions::default(),
                )
                .unwrap();
        }

        assert_eq!(store.open_tasks_count().unwrap(), 3);
    }
}
