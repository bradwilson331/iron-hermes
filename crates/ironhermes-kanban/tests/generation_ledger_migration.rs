//! Phase 47 Plan 03 Task 1 — generation-ledger schema migration test.
//!
//! Validates the v4 → v5 schema bump adding `generation_ledger` +
//! `generation_ledger_log` (GEN-05 / D-04 / D-08 cross-process spend
//! guardrail, CONTEXT.md).
//!
//! Three scenarios, mirroring `goal_mode_migration.rs`:
//! 1. **fresh_db_has_generation_ledger_tables** — a never-existed path opened
//!    via `KanbanStore::new` lands at `schema_version = SCHEMA_VERSION` with
//!    both tables present (DDL path).
//! 2. **v4_db_upgrades_to_v5_generation_ledger** — a hand-crafted v4 DB
//!    (lacking the ledger tables, `schema_version = 4`) re-opened via
//!    `KanbanStore::new` migrates forward and adds both tables without
//!    touching prior `tasks` rows (migration-ladder path).
//! 3. **generation_ledger_migration_is_idempotent** — opening an
//!    already-migrated DB a second time is a no-op (no error, no data loss).

use std::path::PathBuf;

use ironhermes_kanban::schema::SCHEMA_VERSION;
use ironhermes_kanban::store::KanbanStore;
use rusqlite::{Connection, OptionalExtension};

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_| Ok(()),
    )
    .optional()
    .expect("query sqlite_master")
    .is_some()
}

#[test]
fn fresh_db_has_generation_ledger_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");
    let store = KanbanStore::new(&path).expect("open store");

    assert!(
        table_exists(&store.conn, "generation_ledger"),
        "fresh DB must have generation_ledger table"
    );
    assert!(
        table_exists(&store.conn, "generation_ledger_log"),
        "fresh DB must have generation_ledger_log table"
    );

    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version");
    assert_eq!(
        version, SCHEMA_VERSION,
        "fresh DB must land at current SCHEMA_VERSION"
    );
}

/// A hand-crafted v4 DB (pre-ledger, one seeded task row) must migrate
/// forward to v5 with both ledger tables added and the prior task row
/// untouched.
#[test]
fn v4_db_upgrades_to_v5_generation_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");

    {
        let conn = Connection::open(&path).expect("seed open");
        conn.execute_batch(
            "
            CREATE TABLE schema_version (version INTEGER NOT NULL);
            INSERT INTO schema_version (version) VALUES (4);
            CREATE TABLE tasks (
                id                      TEXT PRIMARY KEY,
                title                   TEXT NOT NULL,
                body                    TEXT,
                assignee                TEXT NOT NULL,
                status                  TEXT NOT NULL DEFAULT 'ready',
                priority                INTEGER NOT NULL DEFAULT 0,
                tenant                  TEXT,
                workspace               TEXT,
                skills                  TEXT,
                idempotency_key         TEXT UNIQUE,
                claim_lock              TEXT,
                claim_expires           REAL,
                current_run_id          TEXT,
                consecutive_failures    INTEGER NOT NULL DEFAULT 0,
                max_retries             INTEGER,
                max_runtime_seconds     INTEGER,
                scheduled_at            REAL,
                workflow_template_id    TEXT,
                current_step_key        TEXT,
                created_by              TEXT,
                created_at              REAL NOT NULL,
                started_at              REAL,
                ended_at                REAL,
                goal_mode               INTEGER NOT NULL DEFAULT 0,
                goal_max_turns          INTEGER NOT NULL DEFAULT 20,
                goal_turns_used         INTEGER NOT NULL DEFAULT 0,
                goal_toolset            TEXT,
                output_path             TEXT
            );
            INSERT INTO tasks (id, title, assignee, created_at)
                VALUES ('t_seed0000000001', 'seed task', 'alice', 0.0);
            ",
        )
        .expect("seed v4 schema");
    }

    let store = KanbanStore::new(&path).expect("upgrade open");

    assert!(
        table_exists(&store.conn, "generation_ledger"),
        "v4 -> v5 migration must add generation_ledger table"
    );
    assert!(
        table_exists(&store.conn, "generation_ledger_log"),
        "v4 -> v5 migration must add generation_ledger_log table"
    );

    // Prior row must survive the migration untouched.
    let seed_title: String = store
        .conn
        .query_row(
            "SELECT title FROM tasks WHERE id = 't_seed0000000001'",
            [],
            |r| r.get(0),
        )
        .expect("seed row must survive migration");
    assert_eq!(seed_title, "seed task");

    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version after migration");
    assert_eq!(
        version, SCHEMA_VERSION,
        "schema_version row must be bumped to current SCHEMA_VERSION after migration"
    );
}

/// Re-opening an already-migrated DB must be a no-op — no error, no
/// duplicate-table failure (CREATE TABLE IF NOT EXISTS idempotency).
#[test]
fn generation_ledger_migration_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");

    {
        let _store = KanbanStore::new(&path).expect("first open");
    }

    let store = KanbanStore::new(&path).expect("second open — idempotent");

    assert!(table_exists(&store.conn, "generation_ledger"));
    assert!(table_exists(&store.conn, "generation_ledger_log"));

    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version on second open");
    assert_eq!(
        version, SCHEMA_VERSION,
        "schema_version must remain at current SCHEMA_VERSION on re-open"
    );
}
