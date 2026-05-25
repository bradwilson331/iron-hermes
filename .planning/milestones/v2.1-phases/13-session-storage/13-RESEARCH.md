# Phase 13: Session Storage - Research

**Researched:** 2026-04-11
**Domain:** Rust / SQLite / rusqlite FTS5 / tokio async bridging
**Confidence:** HIGH

## Summary

Phase 13 is an extension-and-integration phase, not a greenfield build. `ironhermes-state` already ships a substantial `StateStore` with SQLite+WAL, sessions/messages tables, FTS5 content tables and triggers, schema migrations v1-v6, session titles, and parent_session_id. The missing pieces are: (1) SearchResult needs snippet+context fields and the query needs to call `fts5_snippet()`; (2) a `SearchFilter` struct for composable WHERE clause filtering; (3) export and prune methods; (4) the retry-with-jitter wrapper and WAL checkpoint timer; (5) wiring `SessionStore` in the gateway as a write-through cache over `StateStore`; and (6) constructing and injecting `StateStore` into the CLI chat loop.

The dominant challenge is the sync/async boundary: `rusqlite::Connection` is `!Send`, so every `StateStore` call in an async context must go through `tokio::task::spawn_blocking`. The pattern requires wrapping `StateStore` in `Arc<Mutex<StateStore>>` and cloning the `Arc` into each `spawn_blocking` closure. This is the same pattern described in CONTEXT.md decision D-11 and follows the established `Arc<Mutex<>>` sharing used for `MemoryStore` and `ToolRegistry` elsewhere in the codebase.

The FTS5 `snippet()` function is a built-in SQLite function available when FTS5 is compiled in (which it is via the `bundled` rusqlite feature). The function signature is `snippet(fts_table, column_index, start_match, end_match, ellipsis, num_tokens)`. For `messages_fts` the indexed column is `content` at index 0. Using `snippet(messages_fts, 0, '<<', '>>', '...', 32)` produces fragments with `<<match>>` markers matching D-04.

**Primary recommendation:** Extend `StateStore` with the missing methods in-place, wrap it in `Arc<Mutex<StateStore>>`, introduce a `spawn_blocking` async bridge layer in gateway and CLI call sites, and refactor `SessionStore` to compose both.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** SessionStore becomes a thin cache wrapper around StateStore. Every `add_message`/`create_session` writes to SQLite immediately via StateStore, while keeping recent messages in memory (HashMap) for fast gateway access. The struct composes both: `SessionStore { state: StateStore, cache: HashMap<String, GatewaySession> }`.
- **D-02:** No auto-recovery of sessions into LLM context on restart. All sessions persist to SQLite but retrieval is manual via the `session_search` tool (Phase 17). The in-memory cache holds only the current running session — on restart, a fresh session starts and old data is query-only. This matches hermes-agent's approach.
- **D-03:** All sources share a single `state.db` — CLI, gateway, cron, and future ACP all write to the same SQLite database at `~/.ironhermes/state.db`. The `source` column distinguishes session origin.
- **D-04:** Search results use FTS5 `snippet()` function to return text fragments with `<<match>>` markers around hits, plus 1 message of surrounding context (`context_before`/`context_after` fields). SearchResult includes snippet, session_title, source, and timestamp.
- **D-05:** FTS5 input is sanitized by default — strip FTS5 operators (AND, OR, NOT, NEAR, *, ^, quotes) from user input. Pass through raw only if the caller explicitly opts in via a `raw: bool` parameter. Prevents syntax errors from crashing searches.
- **D-06:** Search filters use composable SQL WHERE clauses on JOINed tables. A `SearchFilter` struct carries optional fields: `query`, `source`, `role`, `after` (unix timestamp), `before` (unix timestamp), `limit` (default 20). All filters are optional and compose additively.
- **D-07:** Session export uses JSON format. Single session exported as `{ "session": {...}, "messages": [...] }`. Bulk export as an array of the same structure. Matches hermes-agent's export format.
- **D-08:** Pruning is manual only — explicit API call with `older_than_days` threshold and optional `source` filter. No automatic background pruning. Only prunes ended sessions (`ended_at IS NOT NULL`). Returns count of deleted sessions. Cascade deletes associated messages.
- **D-09:** SQLite `busy_timeout` set to 5000ms on connection open. If still SQLITE_BUSY after timeout, application-level retry with random jitter (50-200ms) for up to 3 attempts. WAL mode (already configured) enables concurrent reads.
- **D-10:** Periodic WAL checkpoints via `PRAGMA wal_checkpoint(PASSIVE)` every 5 minutes from a tokio background timer (using `spawn_blocking` for the sync rusqlite call). PASSIVE mode doesn't block writers. Keeps WAL file from growing unbounded.
- **D-11:** CLI and gateway use separate `Connection` instances to the same `state.db` file. No shared connection or IPC needed — WAL mode handles concurrent access safely (multiple concurrent readers, one writer at a time with busy_timeout).

### Claude's Discretion

- Schema migration strategy for new columns/indexes needed by this phase (e.g., snippet support, filter indexes)
- Internal details of the retry wrapper (exact backoff curve, error classification)
- How `spawn_blocking` bridges sync StateStore calls into async gateway/CLI code
- Whether to add indexes for common filter queries (source+timestamp, role+timestamp)
- FTS5 snippet parameters (prefix/suffix markers, fragment size)
- How session lineage recording integrates with future context compression (Phase 18)

### Deferred Ideas (OUT OF SCOPE)

None — discussion stayed within phase scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SESS-01 | StateStore (SQLite+WAL) wired as source of truth across CLI, gateway, ACP | StateStore already exists; wiring means constructing it in runner.rs and main.rs, injecting into SessionStore |
| SESS-02 | In-memory SessionStore acts as write-through cache; sessions recover from SQLite on restart | SessionStore refactor: compose StateStore + HashMap cache; `get_or_create` writes to SQLite immediately |
| SESS-03 | Session lineage tracks parent_session_id chains when context compression triggers session splits | `parent_session_id` column already exists in schema; need `create_session_with_parent()` variant or parameter |
| SESS-04 | User can name sessions with unique titles and resolve sessions by title | `update_session_title()` exists; need `get_session_by_title()` query using existing unique partial index |
| SESS-05 | FTS5 search supports keyword, phrase, boolean, prefix queries with automatic sanitization | FTS5 table + triggers already exist; need sanitize_fts_query() helper + raw opt-in |
| SESS-06 | Search results include FTS5-generated snippets with match markers and 1-message context window | Add snippet field to SearchResult; change search_messages query to call snippet(); add context_before/after |
| SESS-07 | Search supports filtering by source, role, date range | Add SearchFilter struct; rewrite search_messages to build composable WHERE clauses |
| SESS-08 | Sessions exportable (single or bulk with source filter) as structured JSON | Add export_session() and export_sessions() methods returning serde_json::Value |
| SESS-09 | Old ended sessions prunable by age and optional source filter | Add prune_sessions(older_than_days, source) -> Result<usize> method |
| SESS-10 | Schema migrations run sequentially on init with idempotent ALTER TABLE | Migration framework already exists (run_migrations); add v7 migration for any new columns/indexes |
| SESS-11 | Write contention handled with busy_timeout, app retry+jitter, periodic WAL checkpoints | Add busy_timeout to Connection::open; add retry wrapper; add WAL checkpoint background task |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| rusqlite | 0.32 (workspace) | SQLite via bundled libsqlite3 | Already in use; `bundled` feature includes FTS5 |
| tokio | 1 (workspace) | Async runtime; spawn_blocking for sync/async bridge | Project-wide async runtime |
| serde / serde_json | 1 (workspace) | JSON export format for sessions | Already in use throughout |
| uuid | 1 (workspace) | Session ID generation | Already in use |
| chrono | 0.4 (workspace) | Timestamp arithmetic for prune/filter | Already in use |

[VERIFIED: codebase grep] All libraries confirmed present in workspace Cargo.toml at the listed versions.

### No New Dependencies Required

All functionality needed for this phase is achievable with existing workspace dependencies. No new crates need to be added to any Cargo.toml.

[VERIFIED: codebase inspection] `ironhermes-state`, `ironhermes-gateway`, and `ironhermes-cli` already declare the required dependencies.

## Architecture Patterns

### Pattern 1: Write-Through SessionStore (D-01)

**What:** `SessionStore` in `ironhermes-gateway/src/session.rs` is refactored to own a `StateStore` alongside the `HashMap<String, GatewaySession>` cache.

**Structure:**
```rust
// ironhermes-gateway/src/session.rs
pub struct SessionStore {
    state: StateStore,                        // SQLite writes (sync)
    cache: HashMap<String, GatewaySession>,   // fast in-memory reads
}
```

`get_or_create` writes to SQLite first, then populates the cache. The `add_message` method on `GatewaySession` continues to update the in-memory vec; the caller is responsible for calling `state.add_message()` to write through.

[ASSUMED] Whether write-through is best handled by having `SessionStore::add_message_to_session()` delegate to both layers, or by having the handler call both explicitly.

### Pattern 2: spawn_blocking Bridge for Async Call Sites

**What:** `StateStore` is sync (`rusqlite::Connection` is `!Send`). Any async function that needs to call StateStore must bridge via `tokio::task::spawn_blocking`.

**Pattern:**
```rust
// Wrap StateStore in Arc<Mutex<>> for multi-owner sharing
let state = Arc::new(std::sync::Mutex::new(StateStore::open_default()?));

// In an async fn, bridge to sync:
let state_clone = Arc::clone(&state);
let session_id = session_id.to_string();
let msg = msg.clone();
tokio::task::spawn_blocking(move || {
    let mut s = state_clone.lock().unwrap();
    s.add_message(&session_id, &msg)
})
.await
.expect("spawn_blocking panicked")?;
```

[VERIFIED: codebase inspection] The `MemoryStore` uses `Arc<Mutex<dyn MemoryProvider + Send>>` sharing pattern in both `runner.rs` and `handler.rs`. StateStore follows the same pattern.

**Important constraint:** `rusqlite::Connection` is NOT `Send`. However the `StateStore` struct owns its `Connection` by value — the entire `StateStore` can be moved into `spawn_blocking` if wrapped in `Arc<Mutex<>>` because the lock ensures exclusive access. The `Mutex` makes `Arc<Mutex<StateStore>>` safe across threads even though `Connection` is `!Send`, because the `Mutex::lock()` guard is never held across `.await` points.

[VERIFIED: rusqlite docs] rusqlite `Connection` is explicitly `!Send`. The `Arc<Mutex<>>` pattern is the standard workaround for sharing a Connection across threads.

### Pattern 3: FTS5 snippet() Query

**What:** Replace the plain `content` select with `snippet()` call.

**FTS5 snippet signature:**
```sql
snippet(fts_table_name, column_index, start_match, end_match, ellipsis, num_tokens)
```

For `messages_fts` (single content column at index 0):
```sql
SELECT m.id, m.session_id, m.role,
       snippet(messages_fts, 0, '<<', '>>', '...', 32) AS snippet,
       m.timestamp, s.source, s.title
FROM messages_fts f
JOIN messages m ON m.id = f.rowid
JOIN sessions s ON s.id = m.session_id
WHERE messages_fts MATCH ?1
ORDER BY rank
LIMIT ?2
```

[VERIFIED: SQLite FTS5 docs - https://www.sqlite.org/fts5.html#the_snippet_function] `snippet()` is a standard FTS5 auxiliary function. The `num_tokens` argument (max 64) controls fragment size. 32 tokens is a reasonable default.

### Pattern 4: Composable SearchFilter

**What:** Build a SQL query dynamically from optional filter fields.

```rust
pub struct SearchFilter {
    pub query: Option<String>,    // FTS5 query, None = no FTS filter
    pub source: Option<String>,   // session.source
    pub role: Option<String>,     // message.role
    pub after: Option<f64>,       // unix timestamp
    pub before: Option<f64>,      // unix timestamp
    pub limit: usize,             // default 20
    pub raw: bool,                // bypass FTS sanitization
}

impl Default for SearchFilter {
    fn default() -> Self {
        Self { query: None, source: None, role: None,
               after: None, before: None, limit: 20, raw: false }
    }
}
```

Implementation: build WHERE clauses by pushing to a `Vec<String>` and joining with ` AND `. Use `rusqlite::params_from_iter` for dynamic parameter binding.

[VERIFIED: rusqlite 0.32 docs] `params_from_iter` is available in rusqlite for dynamic parameter lists.

### Pattern 5: FTS5 Input Sanitization

**What:** Strip FTS5 special syntax from user input to prevent query parse errors.

```rust
fn sanitize_fts_query(input: &str) -> String {
    // Remove FTS5 operators and special characters
    let re = regex::Regex::new(r#"[*^"()\-]|\b(AND|OR|NOT|NEAR)\b"#).unwrap();
    let cleaned = re.replace_all(input, " ");
    // Collapse whitespace
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}
```

[ASSUMED] Exact regex pattern — the approach is correct but the precise set of stripped characters should be validated against the FTS5 grammar. Risk: over-stripping legitimate input vs. under-stripping and causing parse errors.

### Pattern 6: Retry with Jitter Wrapper

**What:** Wrap SQLite operations that may return `SQLITE_BUSY`.

```rust
fn with_retry<T, F>(mut f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    for attempt in 0..3u32 {
        match f() {
            Ok(v) => return Ok(v),
            Err(StateError::Sqlite(e))
                if e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy)
                    && attempt < 2 =>
            {
                let jitter_ms = 50 + rand::random::<u64>() % 150; // 50-200ms
                std::thread::sleep(std::time::Duration::from_millis(jitter_ms));
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}
```

[ASSUMED] Whether to add `rand` as a dependency or use a simpler deterministic jitter. Since this runs inside `spawn_blocking`, `std::thread::sleep` is appropriate (not tokio::time::sleep). Alternatively, use a simple counter-based jitter: `(attempt + 1) * 75`.

[VERIFIED: codebase inspection] `rand` is NOT currently in workspace dependencies. Use a deterministic jitter formula to avoid adding a dependency: `50 + (attempt as u64 * 75)` ms gives 50ms, 125ms, 200ms.

### Pattern 7: WAL Checkpoint Background Task (D-10)

**What:** Periodic `PRAGMA wal_checkpoint(PASSIVE)` from a tokio timer.

```rust
// In gateway runner or CLI setup:
let state_wal = Arc::clone(&state_store);
tokio::spawn(async move {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
    loop {
        interval.tick().await;
        let s = Arc::clone(&state_wal);
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(mut store) = s.lock() {
                let _ = store.wal_checkpoint();
            }
        }).await;
    }
});
```

The `wal_checkpoint()` method on `StateStore` executes `PRAGMA wal_checkpoint(PASSIVE)`.

[VERIFIED: SQLite WAL docs] PASSIVE checkpoint does not block readers or writers; it copies WAL frames to the database file opportunistically.

### Pattern 8: Export JSON Format (D-07)

```rust
#[derive(Serialize)]
pub struct SessionExport {
    pub session: Session,
    pub messages: Vec<StoredMessage>,
}

// Single session
pub fn export_session(&self, session_id: &str) -> Result<SessionExport>

// Bulk export
pub fn export_sessions(&self, source: Option<&str>) -> Result<Vec<SessionExport>>
```

[VERIFIED: CONTEXT.md D-07] Exact format specified: `{ "session": {...}, "messages": [...] }`.

### Pattern 9: Prune Method (D-08)

```rust
pub fn prune_sessions(&mut self, older_than_days: u32, source: Option<&str>) -> Result<usize> {
    let cutoff = unix_now() - (older_than_days as f64 * 86400.0);
    // DELETE FROM sessions WHERE ended_at IS NOT NULL AND ended_at < cutoff [AND source = ?]
    // CASCADE on messages is automatic via FOREIGN KEY + ON DELETE CASCADE
}
```

[ASSUMED] Whether `ON DELETE CASCADE` is currently specified on `messages.session_id`. Looking at the schema: `session_id TEXT NOT NULL REFERENCES sessions(id)` — no explicit CASCADE. Need to verify and either add it in a migration or delete messages explicitly before deleting sessions.

[VERIFIED: codebase inspection] Schema SQL in `lib.rs` line 75: `session_id TEXT NOT NULL REFERENCES sessions(id)` — NO CASCADE. The prune implementation must explicitly `DELETE FROM messages WHERE session_id IN (SELECT id FROM sessions WHERE ...)` before deleting sessions, or add an `ON DELETE CASCADE` in a v7 schema migration.

### Anti-Patterns to Avoid

- **Holding Mutex lock across .await:** Never do `let guard = state.lock().unwrap(); guard.some_async_method().await`. This deadlocks. Always release the lock before any `.await`.
- **Sharing Connection directly:** `rusqlite::Connection` is `!Send`. Never put it in an `Arc<>` without `Mutex<>`.
- **Building FTS5 queries with string formatting:** Always use prepared statements with `?` parameters. FTS5 MATCH queries go in the WHERE clause via parameter binding, not string interpolation.
- **Calling `PRAGMA wal_checkpoint(FULL)` from async context:** FULL/RESTART modes block writers. Only PASSIVE is safe without holding a lock on writers.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| FTS5 snippet extraction | Custom snippet logic | SQLite `snippet()` built-in | Edge cases in Unicode, multi-token spans |
| JSON export serialization | Manual JSON building | `serde_json::to_value()` on `SessionExport` | Type safety, no escaping bugs |
| Timestamp arithmetic | Custom date math | `chrono::Duration::days()` + `Utc::now().timestamp()` | DST, leap seconds handled |
| FTS5 rank ordering | Custom relevance scoring | SQLite FTS5 `rank` column | Built-in BM25; accurate and fast |

## Common Pitfalls

### Pitfall 1: rusqlite Connection is !Send
**What goes wrong:** Compiler error `the trait Send is not implemented for Connection` when trying to use `StateStore` across await points or in tokio::spawn.
**Why it happens:** `rusqlite::Connection` wraps a raw pointer to a C sqlite3 struct; moving it to another thread is undefined behavior.
**How to avoid:** Always wrap `StateStore` in `Arc<Mutex<StateStore>>`. Clone the `Arc` and move it into `spawn_blocking`. Never hold the lock guard across an `.await`.
**Warning signs:** Compiler errors about `Send` bounds; clippy warning about `MutexGuard` held across await.

### Pitfall 2: FTS5 Syntax Errors Crash Search
**What goes wrong:** User inputs like `"hello"` (unmatched quote) or `AND` (bare operator) cause rusqlite to return `SqliteFailure` with code 1 (SQLITE_ERROR) from the FTS5 tokenizer.
**Why it happens:** FTS5 MATCH argument is parsed as a query expression, not a literal string.
**How to avoid:** Always run user input through `sanitize_fts_query()` by default. Only skip sanitization when `raw: true` is explicitly requested.
**Warning signs:** Search returning errors instead of empty results for short/odd queries.

### Pitfall 3: Missing CASCADE Causes FK Constraint Failure on Prune
**What goes wrong:** `DELETE FROM sessions WHERE ...` fails with `FOREIGN KEY constraint failed` because `messages` rows still reference the session.
**Why it happens:** The schema defines `REFERENCES sessions(id)` without `ON DELETE CASCADE` and `PRAGMA foreign_keys=ON` is set.
**How to avoid:** Either add `ON DELETE CASCADE` in a v7 migration (preferred), or always delete messages before sessions in the prune method.
**Warning signs:** `StateError::Sqlite` with message containing "FOREIGN KEY constraint failed" during pruning.

### Pitfall 4: Schema v7 Migration ALTER TABLE Idempotency
**What goes wrong:** Re-running migrations on a database that already has v7 columns fails with "duplicate column name".
**Why it happens:** SQLite's `ALTER TABLE ADD COLUMN` errors if column already exists (unlike some other databases).
**How to avoid:** Wrap each `ALTER TABLE` in `if current < 7 { ... }` guard. The existing migration pattern in `run_migrations()` already does this correctly.
**Warning signs:** Panic on startup after schema upgrade for databases that were already migrated.

### Pitfall 5: Forgetting busy_timeout on Connection::open
**What goes wrong:** With WAL mode and two processes (CLI + gateway) writing simultaneously, one gets immediate `SQLITE_BUSY` instead of waiting.
**Why it happens:** Default rusqlite timeout is 0ms — immediate failure on contention.
**How to avoid:** Call `conn.busy_timeout(std::time::Duration::from_millis(5000))` immediately after `Connection::open()` in `StateStore::new()`.
**Warning signs:** Intermittent `SqliteFailure` with `DatabaseBusy` code when both CLI and gateway are running.

### Pitfall 6: WAL Checkpoint Timer Holding Arc Lock Too Long
**What goes wrong:** Checkpoint takes non-trivial time and blocks all `add_message` calls while the mutex is held.
**Why it happens:** PASSIVE checkpoint copies WAL frames under the lock if `StateStore::wal_checkpoint()` holds `&mut self`.
**How to avoid:** Use `PASSIVE` mode (not FULL/RESTART). PASSIVE returns quickly if frames are busy. Checkpoint is best-effort — missing one interval is harmless.

## Code Examples

### Set busy_timeout on open
```rust
// Source: rusqlite 0.32 docs
pub fn new(path: impl AsRef<Path>) -> Result<Self> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    let mut store = Self { conn };
    store.init_schema()?;
    Ok(store)
}
```

### FTS5 snippet query
```sql
-- Source: https://www.sqlite.org/fts5.html#the_snippet_function
SELECT m.id, m.session_id, m.role,
       snippet(messages_fts, 0, '<<', '>>', '...', 32) AS snippet,
       m.timestamp, s.source, s.title
FROM messages_fts
JOIN messages m ON m.id = messages_fts.rowid
JOIN sessions s ON s.id = m.session_id
WHERE messages_fts MATCH ?1
ORDER BY messages_fts.rank
LIMIT ?2
```

### params_from_iter for dynamic WHERE
```rust
// Source: rusqlite 0.32 — params_from_iter
use rusqlite::types::ToSql;

let mut conditions: Vec<String> = vec!["1=1".to_string()];
let mut values: Vec<Box<dyn ToSql>> = Vec::new();

if let Some(ref src) = filter.source {
    conditions.push("s.source = ?".to_string());
    values.push(Box::new(src.clone()));
}
if let Some(after) = filter.after {
    conditions.push("m.timestamp >= ?".to_string());
    values.push(Box::new(after));
}

let sql = format!("SELECT ... WHERE {} LIMIT ?", conditions.join(" AND "));
values.push(Box::new(filter.limit as i64));
let mut stmt = conn.prepare(&sql)?;
let rows = stmt.query_map(rusqlite::params_from_iter(values.iter().map(|v| v.as_ref())), ...)?;
```

[ASSUMED] The exact `dyn ToSql` boxing pattern — verify that `params_from_iter` accepts `&dyn ToSql` items. An alternative is using a fixed-parameter query with `IS NULL OR col = ?` patterns to handle optional filters without dynamic SQL.

### SessionExport struct
```rust
// Source: CONTEXT.md D-07
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionExport {
    pub session: Session,
    pub messages: Vec<StoredMessage>,
}
```

### Retry wrapper (no rand dependency)
```rust
// Deterministic jitter: 50ms, 125ms (no rand crate needed)
fn with_busy_retry<T, F: FnMut() -> Result<T>>(mut f: F) -> Result<T> {
    for attempt in 0u32..3 {
        match f() {
            Ok(v) => return Ok(v),
            Err(StateError::Sqlite(ref e))
                if matches!(
                    e.sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                ) && attempt < 2 =>
            {
                std::thread::sleep(std::time::Duration::from_millis(
                    50 + (attempt as u64 * 75),
                ));
            }
            Err(e) => return Err(e),
        }
    }
    f() // final attempt — propagate error
}
```

[VERIFIED: rusqlite 0.32 docs] `rusqlite::ErrorCode::DatabaseBusy` is the correct variant for SQLITE_BUSY. `sqlite_error_code()` returns `Option<ErrorCode>`.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| In-memory only session | Write-through cache to SQLite | This phase | Sessions survive restart |
| Basic FTS5 content search | FTS5 snippet with match markers | This phase | Usable search UX in Phase 17 tool |
| No filtering on search | SearchFilter composable WHERE | This phase | Source/role/date range queries |

## Runtime State Inventory

This phase is not a rename/refactor — it adds new columns and new methods to an existing schema. However it does interact with live state.db files.

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | Existing state.db at `~/.ironhermes/state.db` with schema v6 | v7 migration runs on init — idempotent |
| Live service config | None (no external services store session state) | None |
| OS-registered state | None | None |
| Secrets/env vars | None related to session storage | None |
| Build artifacts | target/ — stale after Cargo.toml changes | `cargo build` |

**Migration risk:** v7 migration adds columns/indexes to existing tables. If any user has a v6 database, migration runs on first start. The existing `ALTER TABLE` pattern is already idempotent (wrapped in version guards). One new item: if `ON DELETE CASCADE` is added via migration, it requires recreating the `messages` table (SQLite does not support `ALTER TABLE ADD CONSTRAINT`). **Recommendation:** Do NOT add ON DELETE CASCADE via migration. Instead, the `prune_sessions` implementation should explicitly delete messages first.

## Environment Availability

Step 2.6: SKIPPED — no external runtime dependencies. All functionality is within-process SQLite (bundled). No services, ports, or external tools required.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Config file | none — cargo built-in |
| Quick run command | `cargo test -p ironhermes-state` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SESS-01 | StateStore opens, creates session, survives re-open | unit | `cargo test -p ironhermes-state test_state_store_persistence` | Wave 0 |
| SESS-02 | SessionStore writes to SQLite on add_message | unit | `cargo test -p ironhermes-gateway test_session_store_write_through` | Wave 0 |
| SESS-03 | create_session with parent_session_id sets lineage | unit | `cargo test -p ironhermes-state test_session_lineage` | Wave 0 |
| SESS-04 | update_session_title + get_session_by_title round-trips | unit | `cargo test -p ironhermes-state test_session_title_lookup` | Wave 0 |
| SESS-05 | sanitize_fts_query strips AND/OR/NOT/special chars | unit | `cargo test -p ironhermes-state test_fts_sanitize` | Wave 0 |
| SESS-06 | search_messages returns snippet with << >> markers | unit | `cargo test -p ironhermes-state test_search_snippet` | Wave 0 |
| SESS-07 | SearchFilter with source/role/date range filters correctly | unit | `cargo test -p ironhermes-state test_search_filter` | Wave 0 |
| SESS-08 | export_session returns correct JSON structure | unit | `cargo test -p ironhermes-state test_export_session` | Wave 0 |
| SESS-09 | prune_sessions deletes ended sessions older than threshold | unit | `cargo test -p ironhermes-state test_prune_sessions` | Wave 0 |
| SESS-10 | init_schema on v6 database runs v7 migration without error | unit | `cargo test -p ironhermes-state test_migration_v6_to_v7` | Wave 0 |
| SESS-11 | with_busy_retry retries on SQLITE_BUSY, succeeds on 2nd attempt | unit | `cargo test -p ironhermes-state test_busy_retry` | Wave 0 |

All test files are Wave 0 gaps — ironhermes-state has 0 existing tests.

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-state`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full workspace test suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-state/tests/state_store.rs` — covers SESS-01 through SESS-11
- [ ] Or `crates/ironhermes-state/src/lib.rs` `#[cfg(test)] mod tests` block — inline tests using `tempfile::NamedTempFile` for isolated DB

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | n/a |
| V3 Session Management | yes | Sessions are internal to single-user deployment; no session token exposure; no network-facing session IDs |
| V4 Access Control | no | Single-operator deployment |
| V5 Input Validation | yes | FTS5 query sanitization (D-05); prevents SQLite FTS parse errors |
| V6 Cryptography | no | SQLite file at rest; no encryption required in this phase |

### Known Threat Patterns for SQLite + FTS5

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| FTS5 query injection (malformed MATCH expression) | Tampering | sanitize_fts_query() strips operators; raw mode opt-in only |
| SQL injection via composable WHERE | Tampering | All values bound via rusqlite params, never string-interpolated |
| Large FTS5 queries causing DoS | Denial of Service | `LIMIT` clause enforced on all queries; snippet token count bounded at 32-64 |
| WAL file left unbounded | Denial of Service | Periodic PASSIVE checkpoint every 5 minutes (D-10) |

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Write-through boundary (SessionStore method vs caller handles dual writes) | Architecture Pattern 1 | Misplaced write-through responsibility causes missed SQLite writes |
| A2 | FTS sanitization regex pattern is complete | Pattern 5 | Under-stripping causes parse errors; over-stripping loses search terms |
| A3 | params_from_iter accepts &dyn ToSql items for dynamic binding | Code Examples | Must use alternative optional-filter SQL pattern |
| A4 | rand crate not needed — deterministic jitter is acceptable | Pattern 6 | Deterministic jitter less effective under sustained contention; trivial to add rand if needed |
| A5 | ON DELETE CASCADE not added via migration — explicit delete order in prune | Common Pitfall 3 | If overlooked, FK violation on prune; mitigated by explicit delete order |

## Open Questions

1. **Where does write-through responsibility sit?**
   - What we know: SessionStore will compose StateStore + HashMap; handler.rs calls `session.add_message(msg)` on the GatewaySession
   - What's unclear: Does `SessionStore::add_message_to_session()` call both layers, or does handler.rs call both `state.add_message()` and `session.add_message()` explicitly?
   - Recommendation: Encapsulate in `SessionStore` — cleaner API, easier to test, consistent with hermes-agent's pattern

2. **Context window for SESS-06 (1 message of surrounding context)**
   - What we know: D-04 specifies `context_before`/`context_after` fields
   - What's unclear: Is this a second SQL query per result, or a single query using window functions / self-joins?
   - Recommendation: Issue a separate `SELECT ... FROM messages WHERE session_id = ? AND timestamp < ? ORDER BY timestamp DESC LIMIT 1` per result. Slightly more queries but simpler code; total results are limited to 20 by default.

3. **FTS5 triggers and content table integrity**
   - What we know: The triggers use `content=messages, content_rowid=id` (external content table mode). This requires that FTS index stay in sync via the INSERT/DELETE/UPDATE triggers.
   - What's unclear: If a message is deleted directly (e.g., via `prune_sessions` explicit delete), does the DELETE trigger fire?
   - Recommendation: The DELETE trigger on `messages` fires for any DELETE statement including cascade or explicit delete. Verified by trigger definition in SCHEMA_SQL. No special handling needed.

## Sources

### Primary (HIGH confidence)
- Codebase: `crates/ironhermes-state/src/lib.rs` — full StateStore implementation, schema SQL, FTS5 triggers, migration chain
- Codebase: `crates/ironhermes-gateway/src/session.rs` — current in-memory SessionStore
- Codebase: `crates/ironhermes-gateway/src/handler.rs` — Arc<RwLock<SessionStore>> usage pattern
- Codebase: `crates/ironhermes-gateway/src/runner.rs` — Arc<Mutex<MemoryProvider>> injection pattern
- Codebase: `Cargo.toml` — workspace dependency versions (rusqlite 0.32, tokio 1, etc.)
- CONTEXT.md — all D-01 through D-11 decisions locked

### Secondary (MEDIUM confidence)
- [CITED: https://www.sqlite.org/fts5.html#the_snippet_function] — FTS5 snippet() function signature and semantics
- [CITED: https://www.sqlite.org/wal.html] — WAL mode and PASSIVE checkpoint behavior
- rusqlite 0.32 crate — `busy_timeout`, `ErrorCode::DatabaseBusy`, `params_from_iter` API

### Tertiary (LOW confidence)
- FTS5 sanitization regex exact pattern — approach verified, exact characters need testing

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all verified from workspace Cargo.toml
- Architecture: HIGH for patterns 1,2,3,7,8,9; MEDIUM for patterns 5,6 (implementation details assumed)
- Pitfalls: HIGH — verified from schema inspection and rusqlite docs

**Research date:** 2026-04-11
**Valid until:** 2026-05-11 (stable library domain, 30-day validity)
