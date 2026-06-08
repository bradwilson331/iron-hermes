//! Phase 36.2 Plan 02 — Migration v9 + UsageEvent integration tests.
//!
//! Behaviors verified (per 36.2-02-PLAN.md):
//!   1. Fresh DB has cache_read_tokens / cache_creation_tokens / cost_usd_micros INTEGER cols.
//!   2. usage_events table exists with 10 expected columns after migration.
//!   3. Indexes usage_events_session_idx and usage_events_ts_idx exist.
//!   4. Migration is idempotent — re-running on a partially-migrated DB does not error.
//!   5. insert_usage_event inserts one row.
//!   6. update_session_stats increments all three new columns.
//!   7. Atomicity — INSERT + UPDATE inside one transaction commits/rolls back together.
//!   8. error_kind round-trips through INSERT + SELECT preserving the value.
//!   9. i64 microdollars round-trip losslessly (no f64 conversion).

use ironhermes_state::{StateStore, UsageEvent};
use rusqlite::params;
use tempfile::NamedTempFile;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fresh_store() -> (StateStore, NamedTempFile) {
    let f = NamedTempFile::new().unwrap();
    let store = StateStore::new(f.path()).unwrap();
    (store, f)
}

/// Return true if a column exists on a given table (PRAGMA table_info).
fn column_exists(store: &StateStore, table: &str, column: &str) -> bool {
    let sql = format!("PRAGMA table_info({table})");
    let conn = store.conn_for_test();
    let mut stmt = conn.prepare(&sql).unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let name: String = row.get(1).unwrap();
        if name == column {
            return true;
        }
    }
    false
}

/// Return true if a named index exists in sqlite_master.
fn index_exists(store: &StateStore, name: &str) -> bool {
    let conn = store.conn_for_test();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
            params![name],
            |r| r.get(0),
        )
        .unwrap();
    count > 0
}

/// Return current schema_version (row 1).
fn schema_version(store: &StateStore) -> i64 {
    let conn = store.conn_for_test();
    conn.query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
        r.get(0)
    })
    .unwrap()
}

// ---------------------------------------------------------------------------
// Behavior 1: sessions has three new cache/cost columns after migration
// ---------------------------------------------------------------------------

#[test]
fn b1_sessions_has_cache_and_cost_columns() {
    let (store, _f) = fresh_store();
    assert!(column_exists(&store, "sessions", "cache_read_tokens"));
    assert!(column_exists(&store, "sessions", "cache_creation_tokens"));
    assert!(column_exists(&store, "sessions", "cost_usd_micros"));
}

// ---------------------------------------------------------------------------
// Behavior 2: usage_events table exists with 10 expected columns
// ---------------------------------------------------------------------------

#[test]
fn b2_usage_events_table_with_ten_columns() {
    let (store, _f) = fresh_store();
    let expected = [
        "id",
        "session_id",
        "ts",
        "provider",
        "model",
        "in_tok",
        "out_tok",
        "cache_read",
        "cache_create",
        "cost_usd_micros",
        "error_kind",
    ];
    for col in expected {
        assert!(
            column_exists(&store, "usage_events", col),
            "usage_events missing column: {col}"
        );
    }
}

// ---------------------------------------------------------------------------
// Behavior 3: Indexes usage_events_session_idx and usage_events_ts_idx exist
// ---------------------------------------------------------------------------

#[test]
fn b3_usage_events_indexes_exist() {
    let (store, _f) = fresh_store();
    assert!(index_exists(&store, "usage_events_session_idx"));
    assert!(index_exists(&store, "usage_events_ts_idx"));
}

// ---------------------------------------------------------------------------
// Behavior 4: Migration is idempotent (re-opening DB at v9 does not error)
// ---------------------------------------------------------------------------

#[test]
fn b4_migration_idempotent_on_reopen() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_path_buf();

    // First open: fresh DB.
    {
        let store = StateStore::new(&path).unwrap();
        assert_eq!(schema_version(&store), 9);
    }
    // Re-open: should not error.
    {
        let store = StateStore::new(&path).unwrap();
        assert_eq!(schema_version(&store), 9);
    }
    // Force schema_version backwards to simulate partial migration, then reopen.
    {
        let store = StateStore::new(&path).unwrap();
        store
            .conn_for_test()
            .execute("UPDATE schema_version SET version = 8", [])
            .unwrap();
    }
    {
        let store = StateStore::new(&path).unwrap();
        assert_eq!(
            schema_version(&store),
            9,
            "re-running migration v9 should bring version back to 9 without error"
        );
    }
}

// ---------------------------------------------------------------------------
// Behavior 5: insert_usage_event inserts one row
// ---------------------------------------------------------------------------

#[test]
fn b5_insert_usage_event_writes_one_row() {
    let (mut store, _f) = fresh_store();
    store
        .create_session("sess-b5", "cli", None, None, None, None)
        .unwrap();

    let ev = UsageEvent {
        session_id: "sess-b5".into(),
        ts: 1_700_000_000_000,
        provider: "anthropic".into(),
        model: "claude-opus-4-7".into(),
        in_tok: 100,
        out_tok: 50,
        cache_read: 8_200,
        cache_create: 4_200,
        cost_usd_micros: 234_000,
        error_kind: None,
    };
    store.insert_usage_event(&ev).unwrap();

    let conn = store.conn_for_test();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

// ---------------------------------------------------------------------------
// Behavior 6: update_session_stats increments all three new columns
// ---------------------------------------------------------------------------

#[test]
fn b6_update_session_stats_increments_new_columns() {
    let (mut store, _f) = fresh_store();
    store
        .create_session("sess-b6", "cli", None, None, None, None)
        .unwrap();

    // First call.
    store
        .update_session_stats("sess-b6", 100, 50, 1, 8_200, 4_200, 234_000)
        .unwrap();
    // Second call to verify aggregation.
    store
        .update_session_stats("sess-b6", 10, 5, 0, 800, 400, 1_000)
        .unwrap();

    let conn = store.conn_for_test();
    let (in_tok, out_tok, tool_calls, cache_r, cache_c, cost): (i64, i64, i64, i64, i64, i64) =
        conn.query_row(
            "SELECT input_tokens, output_tokens, tool_call_count, \
             cache_read_tokens, cache_creation_tokens, cost_usd_micros \
             FROM sessions WHERE id = ?1",
            params!["sess-b6"],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(in_tok, 110);
    assert_eq!(out_tok, 55);
    assert_eq!(tool_calls, 1);
    assert_eq!(cache_r, 9_000);
    assert_eq!(cache_c, 4_600);
    assert_eq!(cost, 235_000);
}

// ---------------------------------------------------------------------------
// Behavior 7 (atomicity): commit-vs-rollback inside an unchecked_transaction
// ---------------------------------------------------------------------------

#[test]
fn b7_atomic_commit_persists_both_rows() {
    let (mut store, _f) = fresh_store();
    store
        .create_session("sess-b7c", "cli", None, None, None, None)
        .unwrap();

    let ev = UsageEvent {
        session_id: "sess-b7c".into(),
        ts: 1_700_000_000_001,
        provider: "anthropic".into(),
        model: "claude-opus-4-7".into(),
        in_tok: 7,
        out_tok: 3,
        cache_read: 0,
        cache_create: 0,
        cost_usd_micros: 42,
        error_kind: None,
    };

    // Wrap both writes in one transaction and commit explicitly.
    {
        let tx = store.conn_for_test().unchecked_transaction().unwrap();
        tx.execute(
            "INSERT INTO usage_events \
             (session_id, ts, provider, model, in_tok, out_tok, \
              cache_read, cache_create, cost_usd_micros, error_kind) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                ev.session_id,
                ev.ts,
                ev.provider,
                ev.model,
                ev.in_tok,
                ev.out_tok,
                ev.cache_read,
                ev.cache_create,
                ev.cost_usd_micros,
                ev.error_kind
            ],
        )
        .unwrap();
        tx.execute(
            "UPDATE sessions SET input_tokens = input_tokens + ?1, \
             output_tokens = output_tokens + ?2, \
             cost_usd_micros = cost_usd_micros + ?3 \
             WHERE id = ?4",
            params![ev.in_tok, ev.out_tok, ev.cost_usd_micros, ev.session_id],
        )
        .unwrap();
        tx.commit().unwrap();
    }

    let conn = store.conn_for_test();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    let (in_tok, cost): (i64, i64) = conn
        .query_row(
            "SELECT input_tokens, cost_usd_micros FROM sessions WHERE id = ?1",
            params!["sess-b7c"],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(in_tok, 7);
    assert_eq!(cost, 42);
}

#[test]
fn b7_atomic_rollback_leaves_both_rows_unchanged() {
    let (mut store, _f) = fresh_store();
    store
        .create_session("sess-b7r", "cli", None, None, None, None)
        .unwrap();

    // Run both writes inside a tx, then drop without committing → rollback.
    {
        let tx = store.conn_for_test().unchecked_transaction().unwrap();
        tx.execute(
            "INSERT INTO usage_events \
             (session_id, ts, provider, model, in_tok, out_tok, \
              cache_read, cache_create, cost_usd_micros, error_kind) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                "sess-b7r",
                1_700_000_000_002_i64,
                "anthropic",
                "claude-opus-4-7",
                7_i64,
                3_i64,
                0_i64,
                0_i64,
                42_i64,
                Option::<String>::None
            ],
        )
        .unwrap();
        tx.execute(
            "UPDATE sessions SET input_tokens = input_tokens + ?1 WHERE id = ?2",
            params![7_i64, "sess-b7r"],
        )
        .unwrap();
        // Drop without commit → rollback.
        drop(tx);
    }

    let conn = store.conn_for_test();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
        .unwrap();
    let in_tok: i64 = conn
        .query_row(
            "SELECT input_tokens FROM sessions WHERE id = ?1",
            params!["sess-b7r"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "usage_events row must NOT survive rollback");
    assert_eq!(in_tok, 0, "sessions update must NOT survive rollback");
}

// ---------------------------------------------------------------------------
// Behavior 8: error_kind round-trip
// ---------------------------------------------------------------------------

#[test]
fn b8_error_kind_round_trips_through_select() {
    let (mut store, _f) = fresh_store();
    store
        .create_session("sess-b8", "cli", None, None, None, None)
        .unwrap();
    let ev = UsageEvent {
        session_id: "sess-b8".into(),
        ts: 1_700_000_000_003,
        provider: "anthropic".into(),
        model: "claude-opus-4-7".into(),
        in_tok: 0,
        out_tok: 0,
        cache_read: 0,
        cache_create: 0,
        cost_usd_micros: 0,
        error_kind: Some("RateLimited".into()),
    };
    store.insert_usage_event(&ev).unwrap();
    let conn = store.conn_for_test();
    let kind: Option<String> = conn
        .query_row(
            "SELECT error_kind FROM usage_events WHERE session_id = ?1",
            params!["sess-b8"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind.as_deref(), Some("RateLimited"));
}

// ---------------------------------------------------------------------------
// Behavior 9: i64 microdollar fidelity (no f64 conversion anywhere)
// ---------------------------------------------------------------------------

#[test]
fn b9_i64_microdollars_round_trip_losslessly() {
    let (mut store, _f) = fresh_store();
    store
        .create_session("sess-b9", "cli", None, None, None, None)
        .unwrap();
    let big = 9_999_999_999_i64; // just under $10,000 USD in micro-USD
    let ev = UsageEvent {
        session_id: "sess-b9".into(),
        ts: 1_700_000_000_004,
        provider: "anthropic".into(),
        model: "claude-opus-4-7".into(),
        in_tok: 0,
        out_tok: 0,
        cache_read: 0,
        cache_create: 0,
        cost_usd_micros: big,
        error_kind: None,
    };
    store.insert_usage_event(&ev).unwrap();
    store
        .update_session_stats("sess-b9", 0, 0, 0, 0, 0, big)
        .unwrap();

    let conn = store.conn_for_test();
    let cost_ev: i64 = conn
        .query_row(
            "SELECT cost_usd_micros FROM usage_events WHERE session_id = ?1",
            params!["sess-b9"],
            |r| r.get(0),
        )
        .unwrap();
    let cost_sess: i64 = conn
        .query_row(
            "SELECT cost_usd_micros FROM sessions WHERE id = ?1",
            params!["sess-b9"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cost_ev, big);
    assert_eq!(cost_sess, big);
}
