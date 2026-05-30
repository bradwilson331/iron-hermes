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

impl KanbanStore {
    /// Open (or create) a database at `path`.
    ///
    /// - Creates parent directories if missing.
    /// - Applies `PRAGMA journal_mode=WAL` and `foreign_keys=ON`.
    /// - Runs the migration ladder if the DB was opened before.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create kanban dir {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("open kanban DB at {}", path.display()))?;

        conn.busy_timeout(std::time::Duration::from_millis(5000))?;

        let mut store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Open the default board DB at `~/.ironhermes/kanban.db`.
    pub fn open_default() -> Result<Self> {
        Self::new(crate::paths::kanban_db_path())
    }

    // -----------------------------------------------------------------------
    // Schema management
    // -----------------------------------------------------------------------

    fn init_schema(&mut self) -> Result<()> {
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
                self.conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
            }
            Some(v) => {
                run_migrations(&mut self.conn, v)?;
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

        // ---------------- Idempotency replay (before opening tx) ----------------
        //
        // If the root already exists under `k:root`, the whole graph already
        // exists (atomic-transaction guarantee — fully-present-or-fully-absent).
        // Walk `task_links` from root to gather worker IDs in their original
        // insertion order, then look up verifier/synthesizer keys explicitly.
        if let Some(ref kr) = key_root {
            if let Some(existing_root) = self.find_by_idempotency_key(kr)? {
                let root_id = existing_root.id.clone();

                // Worker IDs in insertion order — order by `created_at` to match
                // the linear loop order at first-write time.
                let worker_ids: Vec<String> = {
                    let mut stmt = self.conn.prepare(
                        "SELECT child_id FROM task_links \
                         WHERE parent_id = ?1 \
                         ORDER BY created_at ASC, child_id ASC",
                    )?;
                    let rows = stmt.query_map(params![&root_id], |r| r.get::<_, String>(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };

                let verifier_id = match key_verifier.as_deref() {
                    Some(k) => self.find_by_idempotency_key(k)?.map(|t| t.id),
                    None => None,
                };
                let synthesizer_id = match key_synth.as_deref() {
                    Some(k) => self.find_by_idempotency_key(k)?.map(|t| t.id),
                    None => None,
                };

                // The blackboard event id is NOT recoverable from replay (it is
                // a `task_comments` rowid emitted only at first creation). Return
                // `None` — callers re-using `idempotency_key` are expected to
                // ignore this field on replay.
                return Ok(SwarmGraphIds {
                    root_id,
                    worker_ids,
                    verifier_id,
                    synthesizer_id,
                    blackboard_event_id: None,
                });
            }
        }

        // ---------------- Open transaction --------------------------------------

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

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
}
