//! Schema definition, version constant, and migration ladder for the kanban
//! SQLite database (D-05 / D-07, CONTEXT.md).
//!
//! Mirrors the `ironhermes-state` `SCHEMA_VERSION` / `run_migrations()` pattern
//! (Research Q1). All DDL uses `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF
//! NOT EXISTS` so `execute_batch(SCHEMA_SQL)` is idempotent on re-open.

use rusqlite::Connection;

use crate::error::Result;

/// Current schema version. Increment when `run_migrations()` gains a new step.
///
/// v2 — Phase 36.3.7.12: adds `goal_mode`, `goal_max_turns`, `goal_turns_used`
/// columns to `tasks` for the in-session worker goal loop (D-01).
///
/// v3 — Phase 36.3.7.13: adds `goal_toolset TEXT NULL` to `tasks`
/// for the restricted/extended/full toolset preset (D-B1).
///
/// v4 — Phase 46.4: adds `task_attachments` table (D-03/D-05) +
/// `tasks.output_path` column (D-10).
///
/// v5 — Phase 47 Plan 03: adds `generation_ledger` (running count per
/// `root_task_id`) + `generation_ledger_log` (per-generation `model`+`mode`
/// audit rows) for the cross-process kanban spend guardrail (GEN-05, D-04,
/// D-08).
pub const SCHEMA_VERSION: i64 = 5;

/// Full DDL executed on every open via `execute_batch`.
/// Order: pragmas → schema_version → 5 tables → indexes.
///
/// **PRAGMA journal_mode=WAL** is the first statement (D-03, Research Q1).
pub(crate) const SCHEMA_SQL: &str = "
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

-- -----------------------------------------------------------------------
-- tasks
-- -----------------------------------------------------------------------
-- Phase 36.3.7.12 — v2 adds goal_mode, goal_max_turns, goal_turns_used (D-01).
-- 25 columns matching types.rs Task struct (D-07).
CREATE TABLE IF NOT EXISTS tasks (
    id                      TEXT PRIMARY KEY,
    title                   TEXT NOT NULL,
    body                    TEXT,
    assignee                TEXT NOT NULL,
    status                  TEXT NOT NULL DEFAULT 'ready',
    priority                INTEGER NOT NULL DEFAULT 0,
    tenant                  TEXT,
    workspace               TEXT,
    skills                  TEXT,                   -- JSON array (D-28)
    idempotency_key         TEXT UNIQUE,            -- nullable-unique (D-24)
    claim_lock              TEXT,                   -- <host>:<pid>:<uuid> (D-08)
    claim_expires           REAL,
    current_run_id          TEXT,                   -- FK filled after INSERT (circular ref)
    consecutive_failures    INTEGER NOT NULL DEFAULT 0,
    max_retries             INTEGER,
    max_runtime_seconds     INTEGER,
    scheduled_at            REAL,                   -- dispatcher skips future tasks (D-07)
    workflow_template_id    TEXT,                   -- v2-reserved (D-07)
    current_step_key        TEXT,                   -- v2-reserved (D-07)
    created_by              TEXT,                   -- profile slug (D-22 created_cards gate)
    created_at              REAL NOT NULL,
    started_at              REAL,
    ended_at                REAL,
    -- Phase 36.3.7.12 Goal Mode (D-01). Fresh-DB path; existing DBs pick these
    -- up via run_migrations() v1→v2 branch. goal_max_turns DEFAULT 20 mirrors
    -- the D-03 budget default at the column level.
    goal_mode               INTEGER NOT NULL DEFAULT 0,
    goal_max_turns          INTEGER NOT NULL DEFAULT 20,
    goal_turns_used         INTEGER NOT NULL DEFAULT 0,
    -- Phase 36.3.7.13 — v3 adds goal_toolset (D-B1).
    goal_toolset            TEXT,
    -- Phase 46.4 — v4 adds output_path (D-10).
    output_path             TEXT
);

-- -----------------------------------------------------------------------
-- task_links  (D-05, D-39 cross-tenant gate enforced in store.rs)
-- -----------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS task_links (
    parent_id   TEXT NOT NULL,
    child_id    TEXT NOT NULL,
    created_at  REAL NOT NULL,
    PRIMARY KEY (parent_id, child_id),
    FOREIGN KEY (parent_id) REFERENCES tasks(id) ON DELETE CASCADE,
    FOREIGN KEY (child_id)  REFERENCES tasks(id) ON DELETE CASCADE
);

-- -----------------------------------------------------------------------
-- task_comments  (D-05)
-- -----------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS task_comments (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author      TEXT NOT NULL,
    body        TEXT NOT NULL,
    created_at  REAL NOT NULL
);

-- -----------------------------------------------------------------------
-- task_events  (D-05 — append-only audit log)
-- -----------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS task_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    run_id      TEXT,
    kind        TEXT NOT NULL,
    payload     TEXT,
    created_at  REAL NOT NULL
);

-- -----------------------------------------------------------------------
-- task_runs  (D-05)
-- -----------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS task_runs (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    claim_lock  TEXT NOT NULL,
    claim_pid   INTEGER,
    started_at  REAL NOT NULL,
    ended_at    REAL,
    outcome     TEXT,
    summary     TEXT,
    metadata    TEXT,
    error       TEXT,
    log_path    TEXT
);

-- -----------------------------------------------------------------------
-- Orphan removal (post-Phase-36.3.7.6 housekeeping 2026-05-30)
-- -----------------------------------------------------------------------
-- During Phase 36.3.7.5 planning the gateway-notifier subscriptions table
-- went through an intermediate name `kanban_notify_subs` before the locked
-- D-table-name decision renamed it to `kanban_subscriptions`. Any DB
-- created against a pre-rename build retains the orphan with 0 rows.
-- `DROP TABLE IF EXISTS` is idempotent and ~microseconds per open; safe for
-- installs that never had the orphan (no-op) and corrective for installs
-- that did.
DROP TABLE IF EXISTS kanban_notify_subs;

-- -----------------------------------------------------------------------
-- kanban_subscriptions  (Phase 36.3.7.5 BUG-36.3.7.5-01 — gateway notifier subscriptions)
-- -----------------------------------------------------------------------
-- One row per (task, chat-origin) pair. Auto-subscriptions from /kanban create
-- have source='auto'; explicit notify-subscribe verbs have source='explicit'.
-- thread_id defaults to '' (NOT NULL) so the UNIQUE constraint covers thread-less
-- chats (SQLite UNIQUE treats NULL as distinct, which would break our key).
CREATE TABLE IF NOT EXISTS kanban_subscriptions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    platform    TEXT NOT NULL,
    chat_id     TEXT NOT NULL,
    thread_id   TEXT NOT NULL DEFAULT '',
    source      TEXT NOT NULL CHECK (source IN ('auto', 'explicit')),
    created_at  REAL NOT NULL,
    UNIQUE (task_id, platform, chat_id, thread_id)
);

-- -----------------------------------------------------------------------
-- task_attachments  (Phase 46.4 — D-03/D-05, files uploaded to a task,
-- copied into the resolved workspace at worker-spawn time)
-- -----------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS task_attachments (
    id             TEXT PRIMARY KEY,
    task_id        TEXT NOT NULL,
    filename       TEXT NOT NULL,
    size_bytes     INTEGER NOT NULL,
    content_type   TEXT,
    stored_path    TEXT NOT NULL,
    uploaded_by    TEXT,
    created_at     REAL NOT NULL
);

-- -----------------------------------------------------------------------
-- Indexes
-- -----------------------------------------------------------------------
CREATE INDEX IF NOT EXISTS idx_tasks_status_assignee
    ON tasks(status, assignee);

CREATE INDEX IF NOT EXISTS idx_tasks_idempotency
    ON tasks(idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_task_events_task
    ON task_events(task_id, id);

CREATE INDEX IF NOT EXISTS idx_task_runs_task
    ON task_runs(task_id, started_at);

CREATE INDEX IF NOT EXISTS idx_subs_task_id
    ON kanban_subscriptions(task_id);

CREATE INDEX IF NOT EXISTS idx_subs_chat
    ON kanban_subscriptions(platform, chat_id, thread_id);

CREATE INDEX IF NOT EXISTS idx_task_attachments_task
    ON task_attachments(task_id);

-- -----------------------------------------------------------------------
-- generation_ledger / generation_ledger_log
-- (Phase 47 Plan 03 — GEN-05 / D-04 / D-08 cross-process spend guardrail)
-- -----------------------------------------------------------------------
-- `generation_ledger` carries the running reservation count keyed by a
-- caller-supplied `root_task_id` (the swarm root task id for a swarm, or a
-- solitary task's OWN id when it is not part of a swarm — the store method
-- is agnostic to which id it receives; see `try_reserve_generation_slot`).
-- `generation_ledger_log` is a companion append-only row-log recording
-- `model` + `mode` per successfully reserved generation (D-04 $-ready: a
-- future phase can attach per-model cost weights with no schema change).
CREATE TABLE IF NOT EXISTS generation_ledger (
    root_task_id  TEXT PRIMARY KEY,
    count         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS generation_ledger_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    root_task_id  TEXT NOT NULL,
    model         TEXT NOT NULL,
    mode          TEXT NOT NULL,
    created_at    REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_generation_ledger_log_root
    ON generation_ledger_log(root_task_id);
";

/// Migration ladder. Called on re-open when schema_version row already exists.
///
/// Each `if current < N` block is idempotent: `let _ = conn.execute(ALTER …)`
/// suppresses "duplicate column" on partial re-runs (mirror of
/// `ironhermes-state` precedent, Research Q1 point 4).
///
/// v1 is the initial schema — no migration steps yet. The function shape is
/// here so v2 has a landing place.
///
/// # Atomicity (T-4)
///
/// Callers **must** wrap `run_migrations(conn, current)` **and** the
/// subsequent `UPDATE schema_version SET version = ?` inside a single
/// `BEGIN` / `COMMIT` transaction. A crash between the two statements would
/// leave the DB migrated but with the old `schema_version`, causing the next
/// open to re-run the migration and potentially fail on already-applied ALTERs.
/// `KanbanStore::init_schema` satisfies this invariant via explicit
/// `BEGIN`/`COMMIT`/`ROLLBACK` guards around the `v < SCHEMA_VERSION` arm.
pub fn run_migrations(conn: &mut Connection, current: i64) -> Result<()> {
    if current < 2 {
        // Phase 36.3.7.12: add goal_mode, goal_max_turns, goal_turns_used to tasks (D-01).
        //
        // `let _ = conn.execute(ALTER ...)` deliberately swallows the
        // rusqlite "duplicate column" error on a partial re-run (T-36.3.7.12-01-T02).
        // The subsequent `UPDATE schema_version` propagates via `?` so a true
        // ALTER failure (disk-full, permission) still surfaces — and the
        // bilateral `goal_mode_migration::v1_db_upgrades_to_v2` test asserts
        // PRAGMA table_info shows the new columns, catching silent failures.
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN goal_mode INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN goal_max_turns INTEGER NOT NULL DEFAULT 20",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN goal_turns_used INTEGER NOT NULL DEFAULT 0",
            [],
        );
        conn.execute("UPDATE schema_version SET version = 2", [])?;
    }
    if current < 3 {
        // Phase 36.3.7.13: add goal_toolset TEXT NULL to tasks (D-B1).
        //
        // NULL on existing goal_mode cards resolves to "restricted" at
        // worker spawn (D-E1). Same `let _ =` idempotency pattern as v1→v2.
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN goal_toolset TEXT", []);
        conn.execute("UPDATE schema_version SET version = 3", [])?;
    }
    if current < 4 {
        // Phase 46.4: add tasks.output_path (D-10) + task_attachments table
        // (D-03/D-05). Same `let _ =` idempotency pattern as goal_toolset for
        // the ALTER; CREATE TABLE IF NOT EXISTS is idempotent by construction
        // so the table/index statements propagate errors via `?`.
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN output_path TEXT", []);
        conn.execute(
            "CREATE TABLE IF NOT EXISTS task_attachments (
                id             TEXT PRIMARY KEY,
                task_id        TEXT NOT NULL,
                filename       TEXT NOT NULL,
                size_bytes     INTEGER NOT NULL,
                content_type   TEXT,
                stored_path    TEXT NOT NULL,
                uploaded_by    TEXT,
                created_at     REAL NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_task_attachments_task ON task_attachments(task_id)",
            [],
        )?;
        conn.execute("UPDATE schema_version SET version = 4", [])?;
    }
    if current < 5 {
        // Phase 47 Plan 03: add generation_ledger + generation_ledger_log
        // (GEN-05 / D-04 / D-08). CREATE TABLE IF NOT EXISTS is idempotent by
        // construction so these propagate errors via `?` — no `let _ =`
        // swallowing needed (no ALTER statements in this step).
        conn.execute(
            "CREATE TABLE IF NOT EXISTS generation_ledger (
                root_task_id  TEXT PRIMARY KEY,
                count         INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS generation_ledger_log (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                root_task_id  TEXT NOT NULL,
                model         TEXT NOT NULL,
                mode          TEXT NOT NULL,
                created_at    REAL NOT NULL
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_generation_ledger_log_root ON generation_ledger_log(root_task_id)",
            [],
        )?;
        conn.execute("UPDATE schema_version SET version = 5", [])?;
    }
    Ok(())
}
