//! Atomic CAS claim primitive and worker-write gates (D-40 / D-41).
//!
//! The canonical implementation of `BEGIN IMMEDIATE` + CAS UPDATE that is the
//! load-bearing concurrency guarantee for the whole phase.
//!
//! **Concurrency contract (D-40):**
//! - `atomic_claim` uses `TransactionBehavior::Immediate` — NOT the default
//!   `DEFERRED`. `DEFERRED` would allow two dispatcher instances to both read
//!   `status='ready'` before either locks, then both succeed the UPDATE
//!   (Pitfall 1 / RESEARCH.md).
//! - The `task_runs` INSERT and the `claimed` event are in the **same**
//!   transaction so the pointer and run row are coherent by construction.
//!
//! **Static invariants (INV-36.3.7-01, INV-36.3.7-04):**
//! The strings `"Immediate"` and `"INSERT INTO task_runs"` are present in
//! this source file so that `tests/invariants_36_3_7.rs` can assert them via
//! `include_str!`.

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{KanbanError, Result};

/// Default claim TTL (15 minutes, D-13).
pub const DEFAULT_CLAIM_TTL_SECONDS: u64 = 900;

// ---------------------------------------------------------------------------
// build_claim_lock
// ---------------------------------------------------------------------------

/// Build a `<host>:<pid>:<uuid>` claim-lock string (D-08).
///
/// The UUID component disambiguates rapid reclaim-then-reclaim races where
/// the same PID might be reused by the OS within the TTL window.
pub fn build_claim_lock(host: &str, pid: u32) -> String {
    format!("{}:{}:{}", host, pid, uuid::Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------------
// atomic_claim
// ---------------------------------------------------------------------------

/// Atomically claim a `ready` task (D-40 CAS pattern).
///
/// Runs `BEGIN IMMEDIATE` so that the write lock is acquired at transaction
/// start, preventing two concurrent callers from both succeeding the CAS
/// UPDATE on the same task row.
///
/// Returns `Ok(true)` if this caller won the claim (changes == 1), `Ok(false)`
/// if the task was already taken or doesn't match the WHERE predicate.
///
/// On win, inserts a `task_runs` row AND a `claimed` event in the **same**
/// transaction (D-40 invariant: "pointer + run row are coherent by
/// construction").
pub fn atomic_claim(
    conn: &mut Connection,
    task_id: &str,
    claim_lock: &str,
    claim_pid: u32,
    ttl_secs: u64,
    now: f64,
    run_id: &str,
) -> Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let claim_expires = now + ttl_secs as f64;

    tx.execute(
        "UPDATE tasks \
         SET status='running', claim_lock=?1, claim_expires=?2, \
             started_at=COALESCE(started_at, ?3), current_run_id=?4 \
         WHERE id=?5 AND status='ready' AND claim_lock IS NULL \
           AND (scheduled_at IS NULL OR scheduled_at <= ?3)",
        params![claim_lock, claim_expires, now, run_id, task_id],
    )?;

    // tx.changes() works via Deref<Target=Connection> — rusqlite 0.32.
    let won = tx.changes() == 1;

    if won {
        // INSERT INTO task_runs in the same transaction (D-40 / INV-36.3.7-04).
        tx.execute(
            "INSERT INTO task_runs (id, task_id, claim_lock, claim_pid, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, task_id, claim_lock, claim_pid, now],
        )?;

        // Append claimed event in same tx.
        let payload = serde_json::json!({
            "lock": claim_lock,
            "expires": claim_expires,
            "run_id": run_id,
        });
        tx.execute(
            "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) \
             VALUES (?1, ?2, 'claimed', ?3, ?4)",
            params![task_id, run_id, payload.to_string(), now],
        )?;
    }

    tx.commit()?;
    Ok(won)
}

// ---------------------------------------------------------------------------
// release_claim
// ---------------------------------------------------------------------------

/// Release a claim, resetting the task to `ready`.
///
/// Only succeeds (rows-affected == 1) if `claim_lock` still matches the
/// current lock on the task — prevents a displaced worker from releasing a
/// newer claim.
///
/// Returns the number of rows affected (0 = stale lock, 1 = success).
pub fn release_claim(
    conn: &mut Connection,
    task_id: &str,
    claim_lock: &str,
    reason: &str,
) -> Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_secs();

    tx.execute(
        "UPDATE tasks \
         SET status='ready', claim_lock=NULL, claim_expires=NULL, current_run_id=NULL \
         WHERE id=?1 AND claim_lock=?2",
        params![task_id, claim_lock],
    )?;
    let affected = tx.changes() as usize;

    if affected == 1 {
        let payload = serde_json::json!({
            "stale_lock": claim_lock,
            "reason": reason,
        });
        tx.execute(
            "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) \
             VALUES (?1, NULL, 'reclaimed', ?2, ?3)",
            params![task_id, payload.to_string(), now],
        )?;
    }

    tx.commit()?;
    Ok(affected)
}

// ---------------------------------------------------------------------------
// assert_run_id / assert_claim_lock
// ---------------------------------------------------------------------------

/// Assert that `task.current_run_id == expected_run_id`.
///
/// Returns `Err(KanbanError::StaleRunId)` when they differ (D-22 / D-41).
///
/// Used by `complete_task` and `block_task` to reject writes from a worker
/// whose run was superseded by a reclaim + respawn.
pub fn assert_run_id(conn: &Connection, task_id: &str, expected_run_id: &str) -> Result<()> {
    let actual: Option<String> = conn
        .query_row(
            "SELECT current_run_id FROM tasks WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    let actual_str = actual.unwrap_or_default();
    if actual_str != expected_run_id {
        return Err(KanbanError::StaleRunId {
            expected: expected_run_id.to_string(),
            actual: actual_str,
        });
    }
    Ok(())
}

/// Same as `assert_run_id` but operates on an open `Transaction`.
///
/// Used by `store.rs` `complete_task` / `block_task` where the caller already
/// holds a `BEGIN IMMEDIATE` transaction.
pub fn assert_run_id_tx(tx: &Transaction<'_>, task_id: &str, expected_run_id: &str) -> Result<()> {
    let actual: Option<String> = tx
        .query_row(
            "SELECT current_run_id FROM tasks WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    let actual_str = actual.unwrap_or_default();
    if actual_str != expected_run_id {
        return Err(KanbanError::StaleRunId {
            expected: expected_run_id.to_string(),
            actual: actual_str,
        });
    }
    Ok(())
}

/// Assert that `task.claim_lock == claim_lock`.
///
/// Returns `Err(KanbanError::ClaimExpired)` when they differ (D-41).
pub fn assert_claim_lock(
    conn: &Connection,
    task_id: &str,
    claim_lock: &str,
) -> Result<()> {
    let actual: Option<String> = conn
        .query_row(
            "SELECT claim_lock FROM tasks WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    let actual_str = actual.unwrap_or_default();
    if actual_str != claim_lock {
        return Err(KanbanError::ClaimExpired {
            task_id: task_id.to_string(),
            presented_lock: claim_lock.to_string(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// worker_write_gated
// ---------------------------------------------------------------------------

/// Guard a worker write behind a `WHERE claim_lock=?` check (D-41).
///
/// Opens a `BEGIN IMMEDIATE` transaction, checks that `claim_lock` still
/// matches the task's current lock. If it does, calls `write_fn` with the
/// open transaction and returns `Ok(true)`. If not, appends a `claim_expired`
/// advisory event and returns `Ok(false)` (the write is a no-op).
///
/// `write_fn` must not commit or roll back the transaction — the caller
/// commits on success.
pub fn worker_write_gated<F>(
    conn: &mut Connection,
    task_id: &str,
    claim_lock: &str,
    write_fn: F,
) -> Result<bool>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_secs();

    let actual_lock: Option<String> = tx
        .query_row(
            "SELECT claim_lock FROM tasks WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    if actual_lock.as_deref() != Some(claim_lock) {
        // claim_expired advisory — non-blocking (D-41).
        let payload = serde_json::json!({ "presented_lock": claim_lock });
        tx.execute(
            "INSERT INTO task_events (task_id, run_id, kind, payload, created_at) \
             VALUES (?1, NULL, 'claim_expired', ?2, ?3)",
            params![task_id, payload.to_string(), now],
        )?;
        tx.commit()?;
        return Ok(false);
    }

    write_fn(&tx)?;
    tx.commit()?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
