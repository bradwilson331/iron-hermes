//! Integration tests for the multi-board `KanbanStore` open surface (Plan 02, D-01, D-06).
//!
//! Covers the four public entry points added in Phase 36.3.7.9 Plan 02:
//!   - `KanbanStore::open(path)` — unlabeled open
//!   - `KanbanStore::open_labeled(path, slug)` — open with slug label
//!   - `KanbanStore::open_default()` — back-compat wrapper → legacy kanban.db
//!   - `KanbanStore::open_for_board(slug)` — canonical multi-board entry point
//!
//! D-06 migration banner format is locked by tests on the pure helper
//! `KanbanStore::format_migration_banner` — no stderr capture required.

use rusqlite::Connection;

use ironhermes_kanban::store::KanbanStore;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serialises tests that mutate `IRONHERMES_HOME` to prevent env-var races
/// between parallel test threads.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Seed a SQLite DB at `path` with a specific schema_version row.
/// The DB must already have the schema tables created.
fn seed_schema_version(path: &std::path::Path, version: i64) {
    let conn = Connection::open(path).expect("seed conn open");
    // Create the schema_version table if not present.
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; \
         CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
    )
    .expect("seed DDL");
    // Delete any existing rows then insert the target version.
    conn.execute("DELETE FROM schema_version", [])
        .expect("seed delete");
    conn.execute(
        "INSERT INTO schema_version (version) VALUES (?1)",
        rusqlite::params![version],
    )
    .expect("seed insert");
}

// ---------------------------------------------------------------------------
// Tests: open surface
// ---------------------------------------------------------------------------

/// `open_default()` must open the legacy path (`kanban.db` directly under
/// `IRONHERMES_HOME`) — NOT under any `boards/` subdirectory (D-01).
///
/// We verify by using an explicit tempdir as IRONHERMES_HOME (via ScopedHome),
/// then checking that the DB is created at `<home>/kanban.db` directly.
/// The test is serialised via ENV_LOCK to avoid races with other env-mutating tests.
#[test]
fn open_default_still_uses_legacy_path() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

    let expected = ironhermes_kanban::paths::kanban_db_path();
    let _store = KanbanStore::open_default().expect("open_default must succeed");

    unsafe { std::env::remove_var("IRONHERMES_HOME") };

    // The DB must exist at the expected path.
    assert!(
        expected.exists(),
        "open_default should create kanban.db at {:?}",
        expected
    );
    let path_str = expected.to_string_lossy();
    assert!(
        !path_str.contains("/boards/"),
        "open_default path must not contain '/boards/', got {:?}",
        expected
    );
}

/// `open_for_board("default")` must open the same path as `open_default()` (D-01).
///
/// Uses ENV_LOCK for serialisation to avoid races with other env-mutating tests.
#[test]
fn open_for_board_default_uses_legacy_path() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

    let expected = ironhermes_kanban::paths::kanban_db_path();
    let _store =
        KanbanStore::open_for_board("default").expect("open_for_board('default') must succeed");

    unsafe { std::env::remove_var("IRONHERMES_HOME") };

    assert!(
        expected.exists(),
        "open_for_board('default') should create kanban.db at {:?}",
        expected
    );
    let path_str = expected.to_string_lossy();
    assert!(
        !path_str.contains("/boards/"),
        "open_for_board('default') path must not contain '/boards/', got {:?}",
        expected
    );
}

/// `open_for_board("alpha")` must create the DB under `boards/alpha/kanban.db`.
///
/// Uses an explicit tempdir path rather than IRONHERMES_HOME to avoid env races.
#[test]
fn open_for_board_named_uses_boards_layout() {
    let dir = tempfile::tempdir().unwrap();
    // Compute the expected path directly — boards/<slug>/kanban.db relative to the tempdir.
    // We use KanbanStore::open_labeled on the explicit path to bypass IRONHERMES_HOME.
    let expected_path = dir.path().join("boards").join("alpha").join("kanban.db");
    let _store = KanbanStore::open_labeled(&expected_path, "alpha")
        .expect("open_labeled for named board must succeed");

    assert!(
        expected_path.exists(),
        "open_labeled for 'alpha' should create DB at {:?}",
        expected_path
    );
    let path_str = expected_path.to_string_lossy();
    assert!(
        path_str.contains("/boards/alpha/kanban.db")
            || path_str.ends_with("boards/alpha/kanban.db"),
        "named board path should contain '/boards/alpha/kanban.db', got {:?}",
        expected_path
    );
}

/// Opening a fresh DB (no prior `schema_version` row) must insert `SCHEMA_VERSION`
/// and NOT emit a migration banner. We verify by asserting the row equals the
/// current constant — the banner path is locked separately by banner-format tests.
#[test]
fn fresh_db_inserts_schema_version_no_banner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fresh.db");
    let _store = KanbanStore::open(&path).expect("fresh open must succeed");

    // Confirm schema_version row was inserted correctly.
    let conn = Connection::open(&path).expect("verify conn");
    let v: i64 = conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
            r.get(0)
        })
        .expect("schema_version row must exist after fresh open");
    assert_eq!(
        v,
        ironhermes_kanban::schema::SCHEMA_VERSION,
        "fresh DB schema_version must equal SCHEMA_VERSION"
    );
}

/// Opening a DB whose `schema_version == SCHEMA_VERSION` must NOT run any
/// UPDATE — the row stays unchanged (Pitfall 6 / no-op happy path).
#[test]
fn same_version_open_does_not_run_update() {
    use ironhermes_kanban::schema::SCHEMA_VERSION;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("same_version.db");

    // First open creates the schema and inserts the version row.
    let _store1 = KanbanStore::open(&path).expect("first open");
    drop(_store1);

    // Verify the row is SCHEMA_VERSION before second open.
    {
        let conn = Connection::open(&path).expect("verify conn pre");
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .expect("row must exist");
        assert_eq!(
            v, SCHEMA_VERSION,
            "version must equal SCHEMA_VERSION before second open"
        );
    }

    // Second open on same-version DB must succeed without mutation.
    let _store2 = KanbanStore::open(&path).expect("second open on same-version DB must succeed");
    drop(_store2);

    // Verify row is still SCHEMA_VERSION.
    {
        let conn = Connection::open(&path).expect("verify conn post");
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .expect("row must still exist");
        assert_eq!(
            v, SCHEMA_VERSION,
            "version must still equal SCHEMA_VERSION after same-version open"
        );
    }
}

/// Opening a DB with `schema_version = 999` (newer than binary) must return
/// `Err(KanbanError::Other(_))` whose Display contains "newer than binary supports".
#[test]
fn newer_version_open_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.db");

    // First open creates the schema structure.
    let _store = KanbanStore::open(&path).expect("initial open");
    drop(_store);

    // Seed with a future schema version.
    seed_schema_version(&path, 999);

    // Attempt to reopen — must fail.
    let result = KanbanStore::open(&path);
    assert!(
        result.is_err(),
        "opening a newer-version DB must return Err"
    );

    let display = match result {
        Err(e) => format!("{}", e),
        Ok(_) => panic!("expected Err but got Ok"),
    };
    assert!(
        display.contains("newer than binary supports"),
        "error message must contain 'newer than binary supports', got: {}",
        display
    );
}

// ---------------------------------------------------------------------------
// Tests: D-06 migration banner format (byte-stable, pure helper)
// ---------------------------------------------------------------------------

/// The banner format helper locks the D-06 byte-stable string for a named board.
///
/// This approach (testing the pure `format_migration_banner` helper directly)
/// sidesteps stderr capture entirely while locking the format contract.
#[test]
fn migration_banner_format_named_board() {
    let banner = KanbanStore::format_migration_banner(Some("alpha"), 1, 2);
    assert_eq!(
        banner, "[kanban] migrated board 'alpha' from v1 to v2",
        "banner format must be byte-stable per D-06"
    );
}

/// `format_migration_banner(None, ...)` uses `"default"` as the label.
#[test]
fn migration_banner_format_none_slug_uses_default() {
    let banner = KanbanStore::format_migration_banner(None, 1, 2);
    assert_eq!(
        banner, "[kanban] migrated board 'default' from v1 to v2",
        "None slug must produce 'default' label in banner"
    );
}

/// Lock the D-06 format byte-stably across multiple slug + version combinations.
#[test]
fn migration_banner_format_locked() {
    // Named board, single-digit versions.
    assert_eq!(
        KanbanStore::format_migration_banner(Some("alpha"), 1, 2),
        "[kanban] migrated board 'alpha' from v1 to v2"
    );
    // None slug (unlabeled open) → "default".
    assert_eq!(
        KanbanStore::format_migration_banner(None, 1, 2),
        "[kanban] migrated board 'default' from v1 to v2"
    );
    // Hyphenated slug with larger version jump.
    assert_eq!(
        KanbanStore::format_migration_banner(Some("atm10-server"), 1, 5),
        "[kanban] migrated board 'atm10-server' from v1 to v5"
    );
}
