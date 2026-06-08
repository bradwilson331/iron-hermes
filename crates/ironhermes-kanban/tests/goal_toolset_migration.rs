//! Phase 36.3.7.13 Plan 03 Task 1 — bilateral schema-migration test.
//!
//! Validates the v2 → v3 schema bump for the `goal_toolset TEXT NULL` column
//! per CONTEXT.md D-B1 / D-E1.
//!
//! Four scenarios:
//! 1. **fresh_db_has_v3_columns** — a never-existed path opened via
//!    `KanbanStore::new` lands at `schema_version = 3` with `goal_toolset`
//!    column present (DDL path).
//! 2. **v2_db_upgrades_to_v3** — a hand-crafted v2 DB (lacking `goal_toolset`,
//!    `schema_version = 2`) re-opened via `KanbanStore::new` migrates to v3
//!    with the column added (migration-ladder path). Existing rows are preserved
//!    and have `goal_toolset = NULL`.
//! 3. **migration_is_idempotent** — opening a v3 DB a second time is a no-op
//!    (no duplicate-column error, version stays 3).
//! 4. **round_trip_goal_toolset** — insert a task with
//!    `goal_toolset = Some("extended")`, retrieve via `get_task`, assert the
//!    field round-trips correctly.

use std::path::PathBuf;
use tempfile::TempDir;

use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
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

fn open_store(dir: &TempDir) -> KanbanStore {
    let path: PathBuf = dir.path().join("kanban.db");
    KanbanStore::new(&path).expect("open store")
}

/// Fresh DB path: DDL block at schema.rs must include the goal_toolset column,
/// schema_version row lands at 3.
#[test]
fn fresh_db_has_v3_columns() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(&dir);
    let cols = read_tasks_columns(&store.conn);

    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"goal_toolset"),
        "fresh DB must have goal_toolset column; actual columns: {:?}",
        names
    );

    // schema_version row is at 3.
    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version");
    assert_eq!(version, 3, "fresh DB must land at schema_version 3");
}

/// Migration-ladder path: a hand-crafted v2 DB (has goal_mode / goal_max_turns /
/// goal_turns_used columns, schema_version = 2, but NO goal_toolset column)
/// re-opened via `KanbanStore::new` must emerge with goal_toolset present and
/// schema_version = 3. Existing rows must be preserved with goal_toolset = NULL.
#[test]
fn v2_db_upgrades_to_v3() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");

    // Seed a hand-crafted v2 schema: tasks table with goal_mode / goal_max_turns /
    // goal_turns_used but without goal_toolset, schema_version = 2.
    // Insert one task row to verify data preservation after migration.
    {
        let conn = Connection::open(&path).expect("seed open");
        conn.execute_batch(
            "
            CREATE TABLE schema_version (version INTEGER NOT NULL);
            INSERT INTO schema_version (version) VALUES (2);
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
                goal_turns_used         INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO tasks (id, title, assignee, created_at) VALUES ('t_seed1234567890', 'Seed Task', 'alice', 0.0);
            ",
        )
        .expect("seed v2 schema");
        // Connection drops here, releasing the file handle.
    }

    // Reopen via KanbanStore::new → triggers migration ladder.
    let store = KanbanStore::new(&path).expect("upgrade open");
    let cols = read_tasks_columns(&store.conn);
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();

    assert!(
        names.contains(&"goal_toolset"),
        "v2 → v3 migration must add goal_toolset column; actual columns: {:?}",
        names
    );

    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version after migration");
    assert_eq!(
        version, 3,
        "schema_version row must be bumped to 3 after migration"
    );

    // Existing rows must be preserved. goal_toolset is NULL on pre-v3 rows.
    let goal_toolset_val: Option<String> = store
        .conn
        .query_row(
            "SELECT goal_toolset FROM tasks WHERE id = 't_seed1234567890'",
            [],
            |r| r.get(0),
        )
        .expect("read seed task goal_toolset");
    assert!(
        goal_toolset_val.is_none(),
        "pre-v3 rows must have goal_toolset = NULL after migration"
    );
}

/// Re-opening a v3 DB must be a no-op. No duplicate column error, version stays 3.
#[test]
fn migration_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");

    // First open creates the v3 schema.
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
    assert_eq!(version, 3, "schema_version must remain 3 on re-open");

    // goal_toolset column must still be present.
    let cols = read_tasks_columns(&store.conn);
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"goal_toolset"),
        "goal_toolset column must persist after idempotent re-open; actual: {:?}",
        names
    );
}

/// Round-trip test: insert a task with goal_toolset = Some("extended"),
/// retrieve via get_task, assert the field round-trips correctly.
#[test]
fn round_trip_goal_toolset() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open_store(&dir);

    let opts = CreateTaskOptions {
        goal_mode: true,
        goal_toolset: Some("extended".to_string()),
        ..CreateTaskOptions::default()
    };

    let task = store
        .create_task("Round-trip test", "alice", opts)
        .expect("create_task");

    assert_eq!(
        task.goal_toolset,
        Some("extended".to_string()),
        "goal_toolset must round-trip as Some(\"extended\") via create_task + row_to_task"
    );

    // Also verify via get_task read path.
    let fetched = store.get_task(&task.id).expect("get_task");
    assert_eq!(
        fetched.goal_toolset,
        Some("extended".to_string()),
        "goal_toolset must round-trip via get_task"
    );
}
