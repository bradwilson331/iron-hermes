//! SQLite-based session persistence for the IronHermes agent.
//!
//! Provides [`StateStore`] for creating and querying sessions, storing messages,
//! and performing full-text search via FTS5.  All operations are synchronous
//! (rusqlite is a sync library).

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use chrono::Utc;
use ironhermes_core::{get_hermes_home, ChatMessage, Role};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum StateError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T, E = StateError> = std::result::Result<T, E>;

// ---------------------------------------------------------------------------
// Schema version
// ---------------------------------------------------------------------------

const SCHEMA_VERSION: i64 = 7;

const SCHEMA_SQL: &str = "
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id                  TEXT PRIMARY KEY,
    source              TEXT NOT NULL,
    user_id             TEXT,
    model               TEXT,
    system_prompt       TEXT,
    parent_session_id   TEXT,
    started_at          REAL NOT NULL,
    ended_at            REAL,
    end_reason          TEXT,
    message_count       INTEGER DEFAULT 0,
    tool_call_count     INTEGER DEFAULT 0,
    input_tokens        INTEGER DEFAULT 0,
    output_tokens       INTEGER DEFAULT 0,
    title               TEXT,
    FOREIGN KEY (parent_session_id) REFERENCES sessions(id)
);

CREATE TABLE IF NOT EXISTS messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL REFERENCES sessions(id),
    role            TEXT NOT NULL,
    content         TEXT,
    tool_call_id    TEXT,
    tool_calls      TEXT,
    tool_name       TEXT,
    timestamp       REAL NOT NULL,
    token_count     INTEGER,
    finish_reason   TEXT
);

CREATE INDEX IF NOT EXISTS idx_sessions_source  ON sessions(source);
CREATE INDEX IF NOT EXISTS idx_sessions_parent  ON sessions(parent_session_id);
CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);
";

const FTS_SQL: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content,
    content=messages,
    content_rowid=id
);

CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
END;
";

// ---------------------------------------------------------------------------
// Data structs
// ---------------------------------------------------------------------------

/// A stored session record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub source: String,
    pub user_id: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub parent_session_id: Option<String>,
    /// Unix timestamp (seconds since epoch).
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub end_reason: Option<String>,
    pub message_count: i64,
    pub tool_call_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub title: Option<String>,
}

/// A single message row retrieved from storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: Option<String>,
    pub tool_call_id: Option<String>,
    /// JSON-encoded tool calls array, if any.
    pub tool_calls: Option<String>,
    pub tool_name: Option<String>,
    /// Unix timestamp.
    pub timestamp: f64,
    pub token_count: Option<i64>,
    pub finish_reason: Option<String>,
}

/// A result from FTS5 full-text search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub message_id: i64,
    pub session_id: String,
    pub role: String,
    pub content: Option<String>,
    /// FTS5-generated snippet with <<match>> markers. Only present when query is FTS.
    pub snippet: Option<String>,
    /// The message immediately before the match in the same session.
    pub context_before: Option<String>,
    /// The message immediately after the match in the same session.
    pub context_after: Option<String>,
    pub timestamp: f64,
    pub session_source: Option<String>,
    pub session_title: Option<String>,
}

/// Composable search filter for full-text and metadata queries.
#[derive(Debug, Clone)]
pub struct SearchFilter {
    /// FTS5 query string. None = no full-text filter (metadata-only query).
    pub query: Option<String>,
    /// Filter by session source (e.g., "cli", "telegram").
    pub source: Option<String>,
    /// Filter by message role (e.g., "user", "assistant").
    pub role: Option<String>,
    /// Only messages after this unix timestamp.
    pub after: Option<f64>,
    /// Only messages before this unix timestamp.
    pub before: Option<f64>,
    /// Maximum number of results (default 20).
    pub limit: usize,
    /// If true, pass query directly to FTS5 without sanitization.
    pub raw: bool,
}

impl Default for SearchFilter {
    fn default() -> Self {
        Self {
            query: None,
            source: None,
            role: None,
            after: None,
            before: None,
            limit: 20,
            raw: false,
        }
    }
}

impl SearchFilter {
    pub fn new() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// StateStore
// ---------------------------------------------------------------------------

/// SQLite-backed state store for IronHermes sessions.
pub struct StateStore {
    conn: Connection,
}

impl StateStore {
    /// Open (or create) a database at the given path.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create state directory {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("open SQLite database at {}", path.display()))?;

        conn.busy_timeout(std::time::Duration::from_millis(5000))?;

        let mut store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Open the default database at `$IRONHERMES_HOME/state.db`.
    pub fn open_default() -> Result<Self> {
        let db_path = default_db_path();
        debug!("opening state store at {}", db_path.display());
        Self::new(db_path)
    }

    // -----------------------------------------------------------------------
    // Schema management
    // -----------------------------------------------------------------------

    fn init_schema(&mut self) -> Result<()> {
        // Run the base DDL (idempotent: uses CREATE IF NOT EXISTS).
        self.conn.execute_batch(SCHEMA_SQL)?;

        // Determine current schema version.
        let current: Option<i64> = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?;

        match current {
            None => {
                self.conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
            }
            Some(v) => {
                self.run_migrations(v)?;
            }
        }

        // Ensure unique partial index on title (safe to re-run).
        self.conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_title_unique \
             ON sessions(title) WHERE title IS NOT NULL;",
        )?;

        // FTS5 setup — check existence first because CREATE VIRTUAL TABLE can
        // be unreliable inside execute_batch on some builds.
        let fts_exists: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages_fts'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);

        if !fts_exists {
            self.conn.execute_batch(FTS_SQL)?;
        }

        Ok(())
    }

    fn run_migrations(&mut self, current: i64) -> Result<()> {
        if current < 2 {
            // v2: add finish_reason to messages
            let _ = self
                .conn
                .execute("ALTER TABLE messages ADD COLUMN finish_reason TEXT", []);
            self.conn
                .execute("UPDATE schema_version SET version = 2", [])?;
        }
        if current < 3 {
            // v3: add title to sessions
            let _ = self
                .conn
                .execute("ALTER TABLE sessions ADD COLUMN title TEXT", []);
            self.conn
                .execute("UPDATE schema_version SET version = 3", [])?;
        }
        if current < 4 {
            // v4: unique partial index on title (applied unconditionally after this block)
            self.conn
                .execute("UPDATE schema_version SET version = 4", [])?;
        }
        if current < 5 {
            // v5: extended cost/billing columns (Python-only, not in Rust schema)
            self.conn
                .execute("UPDATE schema_version SET version = 5", [])?;
        }
        if current < 6 {
            // v6: reasoning columns in messages (Python-only, not in Rust schema)
            self.conn
                .execute("UPDATE schema_version SET version = 6", [])?;
        }
        if current < 7 {
            // v7: Add composite indexes for search filtering
            let _ = self.conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp);
                 CREATE INDEX IF NOT EXISTS idx_sessions_source_started ON sessions(source, started_at DESC);
                 CREATE INDEX IF NOT EXISTS idx_sessions_ended ON sessions(ended_at);",
            );
            self.conn
                .execute("UPDATE schema_version SET version = 7", [])?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Session lifecycle
    // -----------------------------------------------------------------------

    /// Create a new session record.
    pub fn create_session(
        &mut self,
        id: &str,
        source: &str,
        model: Option<&str>,
        system_prompt: Option<&str>,
        parent_session_id: Option<&str>,
    ) -> Result<()> {
        let now = unix_now();
        self.conn.execute(
            "INSERT OR IGNORE INTO sessions \
             (id, source, model, system_prompt, parent_session_id, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, source, model, system_prompt, parent_session_id, now],
        )?;
        debug!("created session {id} source={source} parent={parent_session_id:?}");
        Ok(())
    }

    /// Mark a session as ended.
    pub fn end_session(&mut self, id: &str, reason: &str) -> Result<()> {
        let now = unix_now();
        let rows = self.conn.execute(
            "UPDATE sessions SET ended_at = ?1, end_reason = ?2 WHERE id = ?3",
            params![now, reason, id],
        )?;
        if rows == 0 {
            warn!("end_session: no session found for id={id}");
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Messages
    // -----------------------------------------------------------------------

    /// Append a [`ChatMessage`] to a session. Returns the new row id.
    pub fn add_message(&mut self, session_id: &str, msg: &ChatMessage) -> Result<i64> {
        let role = role_str(&msg.role);
        let content = msg.content.as_ref().and_then(|c| c.as_text()).map(str::to_owned);
        let tool_calls_json = msg
            .tool_calls
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let tool_name = msg.name.as_deref();
        let timestamp = unix_now();

        self.conn.execute(
            "INSERT INTO messages \
             (session_id, role, content, tool_call_id, tool_calls, tool_name, timestamp) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session_id,
                role,
                content,
                msg.tool_call_id,
                tool_calls_json,
                tool_name,
                timestamp,
            ],
        )?;
        let row_id = self.conn.last_insert_rowid();

        // Increment message_count (and tool_call_count when appropriate).
        let is_tool_call = msg.tool_calls.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
        if is_tool_call {
            self.conn.execute(
                "UPDATE sessions SET message_count = message_count + 1, \
                 tool_call_count = tool_call_count + 1 WHERE id = ?1",
                params![session_id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE sessions SET message_count = message_count + 1 WHERE id = ?1",
                params![session_id],
            )?;
        }

        debug!("added message {row_id} to session {session_id} role={role}");
        Ok(row_id)
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Look up a single session by id.
    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.conn
            .query_row(
                "SELECT id, source, user_id, model, system_prompt, parent_session_id, \
                 started_at, ended_at, end_reason, message_count, tool_call_count, \
                 input_tokens, output_tokens, title \
                 FROM sessions WHERE id = ?1",
                params![id],
                session_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Retrieve all messages for a session ordered by timestamp ascending.
    pub fn get_messages(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, content, tool_call_id, tool_calls, tool_name, \
             timestamp, token_count, finish_reason \
             FROM messages WHERE session_id = ?1 ORDER BY timestamp ASC",
        )?;
        let rows = stmt.query_map(params![session_id], message_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// List sessions, optionally filtered by source, most recent first.
    pub fn list_sessions(
        &self,
        source: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Session>> {
        if let Some(src) = source {
            let mut stmt = self.conn.prepare(
                "SELECT id, source, user_id, model, system_prompt, parent_session_id, \
                 started_at, ended_at, end_reason, message_count, tool_call_count, \
                 input_tokens, output_tokens, title \
                 FROM sessions WHERE source = ?1 ORDER BY started_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![src, limit as i64], session_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, source, user_id, model, system_prompt, parent_session_id, \
                 started_at, ended_at, end_reason, message_count, tool_call_count, \
                 input_tokens, output_tokens, title \
                 FROM sessions ORDER BY started_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], session_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        }
    }

    /// Full-text and metadata search across messages.
    ///
    /// When `filter.query` is `Some`, uses FTS5 with `snippet()` for match
    /// highlighting.  When `None`, performs a metadata-only query.
    pub fn search_messages(&self, filter: &SearchFilter) -> Result<Vec<SearchResult>> {
        let use_fts = filter.query.is_some();
        let query_text = if let Some(ref q) = filter.query {
            let sanitized = if filter.raw {
                q.clone()
            } else {
                sanitize_fts_query(q)
            };
            if sanitized.is_empty() {
                return Ok(vec![]);
            }
            Some(sanitized)
        } else {
            None
        };

        // Build dynamic WHERE clauses
        let mut conditions: Vec<String> = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut param_idx = 1usize;

        // Base query depends on whether FTS is used
        let base_select = if use_fts {
            "SELECT m.id, m.session_id, m.role, m.content, \
             snippet(messages_fts, 0, '<<', '>>', '...', 32) AS snip, \
             m.timestamp, s.source, s.title \
             FROM messages_fts \
             JOIN messages m ON m.id = messages_fts.rowid \
             JOIN sessions s ON s.id = m.session_id"
                .to_string()
        } else {
            "SELECT m.id, m.session_id, m.role, m.content, \
             NULL AS snip, \
             m.timestamp, s.source, s.title \
             FROM messages m \
             JOIN sessions s ON s.id = m.session_id"
                .to_string()
        };

        if let Some(ref qt) = query_text {
            conditions.push(format!("messages_fts MATCH ?{param_idx}"));
            param_values.push(Box::new(qt.clone()));
            param_idx += 1;
        }
        if let Some(ref src) = filter.source {
            conditions.push(format!("s.source = ?{param_idx}"));
            param_values.push(Box::new(src.clone()));
            param_idx += 1;
        }
        if let Some(ref role) = filter.role {
            conditions.push(format!("m.role = ?{param_idx}"));
            param_values.push(Box::new(role.clone()));
            param_idx += 1;
        }
        if let Some(after) = filter.after {
            conditions.push(format!("m.timestamp >= ?{param_idx}"));
            param_values.push(Box::new(after));
            param_idx += 1;
        }
        if let Some(before) = filter.before {
            conditions.push(format!("m.timestamp <= ?{param_idx}"));
            param_values.push(Box::new(before));
            param_idx += 1;
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let order = if use_fts {
            "ORDER BY messages_fts.rank"
        } else {
            "ORDER BY m.timestamp DESC"
        };
        let sql = format!("{base_select}{where_clause} {order} LIMIT ?{param_idx}");
        param_values.push(Box::new(filter.limit as i64));

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_ref.as_slice(), |r| {
            Ok(SearchResult {
                message_id: r.get(0)?,
                session_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                snippet: r.get(4)?,
                context_before: None,
                context_after: None,
                timestamp: r.get(5)?,
                session_source: r.get(6)?,
                session_title: r.get(7)?,
            })
        })?;

        let mut results: Vec<SearchResult> = rows.collect::<rusqlite::Result<Vec<_>>>()?;

        // Populate context_before and context_after (1 message each).
        for result in &mut results {
            result.context_before = self
                .conn
                .query_row(
                    "SELECT content FROM messages WHERE session_id = ?1 AND timestamp < ?2 \
                     ORDER BY timestamp DESC LIMIT 1",
                    params![result.session_id, result.timestamp],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();

            result.context_after = self
                .conn
                .query_row(
                    "SELECT content FROM messages WHERE session_id = ?1 AND timestamp > ?2 \
                     ORDER BY timestamp ASC LIMIT 1",
                    params![result.session_id, result.timestamp],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
        }

        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Updates
    // -----------------------------------------------------------------------

    /// Update aggregate token and tool-call statistics for a session.
    pub fn update_session_stats(
        &mut self,
        id: &str,
        input_tokens: i64,
        output_tokens: i64,
        tool_call_count: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET \
             input_tokens = input_tokens + ?1, \
             output_tokens = output_tokens + ?2, \
             tool_call_count = tool_call_count + ?3 \
             WHERE id = ?4",
            params![input_tokens, output_tokens, tool_call_count, id],
        )?;
        Ok(())
    }

    /// Set or replace the human-readable title for a session.
    pub fn update_session_title(&mut self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    /// Look up a single session by its unique title.
    pub fn get_session_by_title(&self, title: &str) -> Result<Option<Session>> {
        self.conn
            .query_row(
                "SELECT id, source, user_id, model, system_prompt, parent_session_id, \
                 started_at, ended_at, end_reason, message_count, tool_call_count, \
                 input_tokens, output_tokens, title \
                 FROM sessions WHERE title = ?1",
                params![title],
                session_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Run a passive WAL checkpoint to keep the WAL file from growing unbounded.
    pub fn wal_checkpoint(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FTS5 sanitization
// ---------------------------------------------------------------------------

/// Strip FTS5 special operators from user input to prevent query parse errors.
/// Pass `raw: true` in [`SearchFilter`] to bypass this.
pub fn sanitize_fts_query(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '*' | '^' | '"' | '(' | ')' | '-' | '{' | '}' | ':' => result.push(' '),
            _ => result.push(ch),
        }
    }
    // Remove FTS5 boolean keywords
    let result = result
        .split_whitespace()
        .filter(|w| !matches!(w.to_uppercase().as_str(), "AND" | "OR" | "NOT" | "NEAR"))
        .collect::<Vec<_>>()
        .join(" ");
    if result.trim().is_empty() {
        String::new()
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// Retry wrapper
// ---------------------------------------------------------------------------

/// Retry a closure up to 3 times on `SQLITE_BUSY`, with deterministic jitter
/// (50 ms, then 125 ms). No `rand` dependency required.
fn with_busy_retry<T, F: FnMut() -> Result<T>>(mut f: F) -> Result<T> {
    for attempt in 0u32..3 {
        match f() {
            Ok(v) => return Ok(v),
            Err(ref e) if is_busy(e) && attempt < 2 => {
                let jitter_ms = 50 + (attempt as u64 * 75); // 50ms, 125ms
                std::thread::sleep(std::time::Duration::from_millis(jitter_ms));
            }
            Err(e) => return Err(e),
        }
    }
    f() // final attempt — propagate error
}

/// Check whether a [`StateError`] is a `SQLITE_BUSY` error.
fn is_busy(e: &StateError) -> bool {
    if let StateError::Sqlite(sq) = e {
        matches!(sq.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy))
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn default_db_path() -> PathBuf {
    get_hermes_home().join("state.db")
}

fn unix_now() -> f64 {
    Utc::now().timestamp_millis() as f64 / 1000.0
}

fn role_str(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn session_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        source: r.get(1)?,
        user_id: r.get(2)?,
        model: r.get(3)?,
        system_prompt: r.get(4)?,
        parent_session_id: r.get(5)?,
        started_at: r.get(6)?,
        ended_at: r.get(7)?,
        end_reason: r.get(8)?,
        message_count: r.get(9)?,
        tool_call_count: r.get(10)?,
        input_tokens: r.get(11)?,
        output_tokens: r.get(12)?,
        title: r.get(13)?,
    })
}

fn message_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessage> {
    Ok(StoredMessage {
        id: r.get(0)?,
        session_id: r.get(1)?,
        role: r.get(2)?,
        content: r.get(3)?,
        tool_call_id: r.get(4)?,
        tool_calls: r.get(5)?,
        tool_name: r.get(6)?,
        timestamp: r.get(7)?,
        token_count: r.get(8)?,
        finish_reason: r.get(9)?,
    })
}
