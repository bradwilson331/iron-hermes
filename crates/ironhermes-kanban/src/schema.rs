//! Schema definition, version constant, and migration ladder for the kanban
//! SQLite database (D-05 / D-07, CONTEXT.md).
//!
//! Mirrors the `ironhermes-state` `SCHEMA_VERSION` / `run_migrations()` pattern
//! (Research Q1). All DDL uses `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF
//! NOT EXISTS` so `execute_batch(SCHEMA_SQL)` is idempotent on re-open.

use rusqlite::Connection;

use crate::error::Result;

/// Current schema version. Increment when `run_migrations()` gains a new step.
pub const SCHEMA_VERSION: i64 = 1;

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
-- 22 columns matching types.rs Task struct (D-07).
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
    ended_at                REAL
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
";

/// Migration ladder. Called on re-open when schema_version row already exists.
///
/// Each `if current < N` block is idempotent: `let _ = conn.execute(ALTER …)`
/// suppresses "duplicate column" on partial re-runs (mirror of
/// `ironhermes-state` precedent, Research Q1 point 4).
///
/// v1 is the initial schema — no migration steps yet. The function shape is
/// here so v2 has a landing place.
pub fn run_migrations(conn: &mut Connection, current: i64) -> Result<()> {
    // v1 is the baseline — no ALTER statements needed.
    // Example for v2 (add a column):
    //   if current < 2 {
    //       let _ = conn.execute("ALTER TABLE tasks ADD COLUMN new_col TEXT", []);
    //       conn.execute("UPDATE schema_version SET version = 2", [])?;
    //   }
    let _ = current; // suppress unused-variable warning until first real migration
    let _ = conn;
    Ok(())
}
