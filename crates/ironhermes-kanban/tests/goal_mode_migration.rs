//! Phase 36.3.7.12 Plan 01 Task 1 — bilateral schema-migration test.
//!
//! Validates the v1 → v2 schema bump for goal-mode columns
//! (`goal_mode`, `goal_max_turns`, `goal_turns_used`) per CONTEXT.md D-01 and
//! RESEARCH Finding 5 + Pitfall 1.
//!
//! Three scenarios:
//! 1. **fresh_db_has_v2_columns** — a never-existed path opened via
//!    `KanbanStore::new` lands at `schema_version = 2` with all three
//!    goal-mode columns present at the expected defaults (DDL path).
//! 2. **v1_db_upgrades_to_v2** — a hand-crafted v1 DB (lacking the columns,
//!    `schema_version = 1`) re-opened via `KanbanStore::new` migrates to v2
//!    with the three columns added (migration-ladder path).
//! 3. **migration_is_idempotent** — opening a v2 DB a second time is a no-op
//!    (no `PartialReExec`, no duplicate-column error).

use std::path::PathBuf;
use tempfile::TempDir;

use ironhermes_kanban::store::KanbanStore;
use rusqlite::Connection;

/// PRAGMA table_info(tasks) row shape: (cid, name, type, notnull, dflt_value, pk).
#[derive(Debug, Clone)]
struct ColumnInfo {
    name: String,
    /// SQLite stores DEFAULT as the literal expression text (e.g. `"0"`, `"20"`).
    dflt_value: Option<String>,
    notnull: i64,
}

fn read_tasks_columns(conn: &Connection) -> Vec<ColumnInfo> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(tasks)")
        .expect("prepare table_info");
    let rows: Vec<ColumnInfo> = stmt
        .query_map([], |row| {
            Ok(ColumnInfo {
                name: row.get(1)?,
                notnull: row.get(3)?,
                dflt_value: row.get(4)?,
            })
        })
        .expect("query table_info")
        .collect::<Result<_, _>>()
        .expect("collect table_info");
    rows
}

fn open_store(dir: &TempDir) -> KanbanStore {
    let path: PathBuf = dir.path().join("kanban.db");
    KanbanStore::new(&path).expect("open store")
}

/// Fresh DB path: DDL block at schema.rs must include the three goal-mode columns,
/// schema_version row lands at 2.
#[test]
fn fresh_db_has_v2_columns() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(&dir);
    let cols = read_tasks_columns(&store.conn);

    let by_name: std::collections::HashMap<String, ColumnInfo> =
        cols.iter().map(|c| (c.name.clone(), c.clone())).collect();

    let goal_mode = by_name
        .get("goal_mode")
        .expect("fresh DB must have goal_mode column");
    assert_eq!(
        goal_mode.notnull, 1,
        "goal_mode must be NOT NULL"
    );
    assert_eq!(
        goal_mode.dflt_value.as_deref(),
        Some("0"),
        "goal_mode DEFAULT must be 0"
    );

    let goal_max_turns = by_name
        .get("goal_max_turns")
        .expect("fresh DB must have goal_max_turns column");
    assert_eq!(goal_max_turns.notnull, 1, "goal_max_turns must be NOT NULL");
    assert_eq!(
        goal_max_turns.dflt_value.as_deref(),
        Some("20"),
        "goal_max_turns DEFAULT must be 20 (D-03 budget default)"
    );

    let goal_turns_used = by_name
        .get("goal_turns_used")
        .expect("fresh DB must have goal_turns_used column");
    assert_eq!(goal_turns_used.notnull, 1, "goal_turns_used must be NOT NULL");
    assert_eq!(
        goal_turns_used.dflt_value.as_deref(),
        Some("0"),
        "goal_turns_used DEFAULT must be 0"
    );

    // schema_version row is at 2.
    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version");
    assert_eq!(version, 2, "fresh DB must land at schema_version 2");
}

/// Migration-ladder path: a hand-crafted v1 DB (no goal-mode columns,
/// schema_version row = 1) re-opened via `KanbanStore::new` must emerge with
/// all three columns present and schema_version = 2.
#[test]
fn v1_db_upgrades_to_v2() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");

    // Seed a hand-crafted v1 schema:
    // - tasks table without goal-mode columns
    // - schema_version row = 1
    // The minimum subset required for the migration ladder to detect v1.
    {
        let conn = Connection::open(&path).expect("seed open");
        conn.execute_batch(
            "
            CREATE TABLE schema_version (version INTEGER NOT NULL);
            INSERT INTO schema_version (version) VALUES (1);
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
                ended_at                REAL
            );
            ",
        )
        .expect("seed v1 schema");
        // Connection drops here, releasing the file handle.
    }

    // Reopen via KanbanStore::new → triggers migration ladder.
    let store = KanbanStore::new(&path).expect("upgrade open");
    let cols = read_tasks_columns(&store.conn);
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();

    assert!(
        names.contains(&"goal_mode"),
        "v1 → v2 migration must add goal_mode column; actual columns: {:?}",
        names
    );
    assert!(
        names.contains(&"goal_max_turns"),
        "v1 → v2 migration must add goal_max_turns column; actual columns: {:?}",
        names
    );
    assert!(
        names.contains(&"goal_turns_used"),
        "v1 → v2 migration must add goal_turns_used column; actual columns: {:?}",
        names
    );

    let version: i64 = store
        .conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("read schema_version after migration");
    assert_eq!(
        version, 2,
        "schema_version row must be bumped to 2 after migration"
    );
}

/// Re-opening a v2 DB must be a no-op. No `PartialReExec`, no duplicate
/// column error, no version regression. RESEARCH Finding 5 idempotency
/// contract — `let _ = conn.execute("ALTER TABLE ... ADD COLUMN ...")` must
/// swallow the duplicate-column error.
#[test]
fn migration_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path: PathBuf = dir.path().join("kanban.db");

    // First open creates the v2 schema.
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
    assert_eq!(version, 2, "schema_version must remain 2 on re-open");

    // All three goal-mode columns are still present after the second open.
    let cols = read_tasks_columns(&store.conn);
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    for col in &["goal_mode", "goal_max_turns", "goal_turns_used"] {
        assert!(
            names.contains(col),
            "column '{}' must persist after idempotent re-open; actual: {:?}",
            col,
            names
        );
    }
}
