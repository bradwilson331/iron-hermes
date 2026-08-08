//! Phase 46.4 Plan 01 Task 2 — bilateral schema-migration test.
//!
//! Validates the v3 → v4 schema bump for the `task_attachments` table
//! (D-03/D-05) and `tasks.output_path TEXT NULL` column (D-10) per
//! CONTEXT.md.
//!
//! Four scenarios (mirrors `goal_toolset_migration.rs`):
//! 1. **fresh_db_has_v4_shape** — a never-existed path opened via
//!    `KanbanStore::new` lands at `schema_version = 4` with `output_path`
//!    column AND `task_attachments` table present (DDL path).
//! 2. **v3_db_upgrades_to_v4** — a hand-crafted v3 DB (lacking `output_path`
//!    and `task_attachments`, `schema_version = 3`) re-opened via
//!    `KanbanStore::new` migrates to v4 with both added (migration-ladder
//!    path). Existing rows are preserved with `output_path = NULL`.
//! 3. **migration_is_idempotent** — opening a v4 DB a second time is a
//!    no-op (no duplicate-column / duplicate-table error, version stays 4).
//! 4. **fresh_and_migrated_shapes_match** — the column/table shape produced
//!    by the fresh-DB DDL path and the migration-ladder path are identical
//!    (fresh-DB invariant). output_path round-trip through `complete_task`
//!    is covered in Plan 03 — this test only asserts existence.

use std::path::PathBuf;
use tempfile::TempDir;

use ironhermes_kanban::store::KanbanStore;
use rusqlite::Connection;

/// PRAGMA table_info(tasks) row shape: (cid, name, type, notnull, dflt_value, pk).
#[derive(Debug, Clone)]
struct ColumnInfo {
    name: String,
}

fn read_tasks_columns(conn: &Connection) -> Vec<ColumnInfo> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(tasks)")
        .expect("prepare table_info");
    let rows: Vec<ColumnInfo> = stmt
        .query_map([], |row| Ok(ColumnInfo { name: row.get(1)? }))
        .expect("query table_info")
        .collect::<Result<_, _>>()
        .expect("collect table_info");
    rows
}

fn task_attachments_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='task_attachments'",
        [],
        |r| r.get::<_, String>(0),
    )
    .is_ok()
}

fn open_store(dir: &TempDir) -> KanbanStore {
    let path: PathBuf = dir.path().join("kanban.db");
    KanbanStore::new(&path).expect("open store")
}

/// Fresh DB path: DDL block at schema.rs must include output_path column
/// and task_attachments table, schema_version row lands at 4.
#[test]
fn fresh_db_has_v4_shape() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(&dir);
    let cols = read_tasks_columns(&store.conn);

    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"output_path"),
        "fresh DB must have tasks.output_path column; actual columns: {:?}",
        names
    );
    assert!(
        task_attachments_table_exists(&store.conn),
        "fresh DB must have task_attachments table"
    );

    // schema_version row is at 4.
    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version");
    assert_eq!(version, 4, "fresh DB must land at schema_version 4");
}

/// Migration-ladder path: a hand-crafted v3 DB (has goal_toolset but NO
/// output_path column and NO task_attachments table, schema_version = 3)
/// re-opened via `KanbanStore::new` must emerge with both present and
/// schema_version = 4. Existing rows must be preserved with output_path = NULL.
#[test]
fn v3_db_upgrades_to_v4() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");

    // Seed a hand-crafted v3 schema: tasks table through goal_toolset but
    // without output_path, schema_version = 3, no task_attachments table.
    // Insert one task row to verify data preservation after migration.
    {
        let conn = Connection::open(&path).expect("seed open");
        conn.execute_batch(
            "
            CREATE TABLE schema_version (version INTEGER NOT NULL);
            INSERT INTO schema_version (version) VALUES (3);
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
                goal_toolset            TEXT
            );
            INSERT INTO tasks (id, title, assignee, created_at) VALUES ('t_seed1234567890', 'Seed Task', 'alice', 0.0);
            ",
        )
        .expect("seed v3 schema");
        // Connection drops here, releasing the file handle.
    }

    // Reopen via KanbanStore::new → triggers migration ladder.
    let store = KanbanStore::new(&path).expect("upgrade open");
    let cols = read_tasks_columns(&store.conn);
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();

    assert!(
        names.contains(&"output_path"),
        "v3 → v4 migration must add output_path column; actual columns: {:?}",
        names
    );
    assert!(
        task_attachments_table_exists(&store.conn),
        "v3 → v4 migration must create task_attachments table"
    );

    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version after migration");
    assert_eq!(
        version, 4,
        "schema_version row must be bumped to 4 after migration"
    );

    // Existing rows must be preserved. output_path is NULL on pre-v4 rows.
    let output_path_val: Option<String> = store
        .conn
        .query_row(
            "SELECT output_path FROM tasks WHERE id = 't_seed1234567890'",
            [],
            |r| r.get(0),
        )
        .expect("read seed task output_path");
    assert!(
        output_path_val.is_none(),
        "pre-v4 rows must have output_path = NULL after migration"
    );

    // Row itself (title) must be preserved untouched.
    let title: String = store
        .conn
        .query_row(
            "SELECT title FROM tasks WHERE id = 't_seed1234567890'",
            [],
            |r| r.get(0),
        )
        .expect("read seed task title");
    assert_eq!(title, "Seed Task", "existing row data must be preserved");
}

/// Re-opening a v4 DB must be a no-op. No duplicate-column / duplicate-table
/// error, version stays 4.
#[test]
fn migration_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");

    // First open creates the v4 schema.
    {
        let _store = KanbanStore::new(&path).expect("first open");
    }

    // Second open must succeed without errors.
    let store = KanbanStore::new(&path).expect("second open — idempotent");

    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version on second open");
    assert_eq!(version, 4, "schema_version must remain 4 on re-open");

    // output_path column and task_attachments table must still be present.
    let cols = read_tasks_columns(&store.conn);
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"output_path"),
        "output_path column must persist after idempotent re-open; actual: {:?}",
        names
    );
    assert!(
        task_attachments_table_exists(&store.conn),
        "task_attachments table must persist after idempotent re-open"
    );
}

/// Fresh-DB invariant: the shape produced by SCHEMA_SQL (fresh DB) must
/// match the shape produced by run_migrations (v3 → v4 upgrade) — same
/// column set on tasks, same task_attachments presence.
#[test]
fn fresh_and_migrated_shapes_match() {
    // Fresh DB.
    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh_store = open_store(&fresh_dir);
    let mut fresh_cols: Vec<String> = read_tasks_columns(&fresh_store.conn)
        .into_iter()
        .map(|c| c.name)
        .collect();
    fresh_cols.sort();

    // Migrated DB (v3 seed → upgrade).
    let mig_dir = tempfile::tempdir().unwrap();
    let mig_path: PathBuf = mig_dir.path().join("kanban.db");
    {
        let conn = Connection::open(&mig_path).expect("seed open");
        conn.execute_batch(
            "
            CREATE TABLE schema_version (version INTEGER NOT NULL);
            INSERT INTO schema_version (version) VALUES (3);
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
                goal_toolset            TEXT
            );
            ",
        )
        .expect("seed v3 schema");
    }
    let mig_store = KanbanStore::new(&mig_path).expect("upgrade open");
    let mut mig_cols: Vec<String> = read_tasks_columns(&mig_store.conn)
        .into_iter()
        .map(|c| c.name)
        .collect();
    mig_cols.sort();

    assert_eq!(
        fresh_cols, mig_cols,
        "fresh-DB and migration-ladder tasks column sets must be identical"
    );
    assert!(task_attachments_table_exists(&fresh_store.conn));
    assert!(task_attachments_table_exists(&mig_store.conn));
}
