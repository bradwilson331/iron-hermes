//! SQLite-based session persistence for the IronHermes agent.
//!
//! Provides [`StateStore`] for creating and querying sessions, storing messages,
//! and performing full-text search via FTS5.  All operations are synchronous
//! (rusqlite is a sync library).

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use chrono::Utc;
use ironhermes_core::{
    ChatMessage, Role, get_hermes_home, session_attachments_dir, session_workspace_dir,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

// Phase 25.3 D-F-1: 4-file directory export (messages/metadata/context/trajectories).
pub mod session_export;
pub use session_export::SessionDirectoryExport;

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

const SCHEMA_VERSION: i64 = 11;

const SCHEMA_SQL: &str = "
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id                      TEXT PRIMARY KEY,
    source                  TEXT NOT NULL,
    user_id                 TEXT,
    model                   TEXT,
    system_prompt           TEXT,
    parent_session_id       TEXT,
    started_at              REAL NOT NULL,
    ended_at                REAL,
    end_reason              TEXT,
    message_count           INTEGER DEFAULT 0,
    tool_call_count         INTEGER DEFAULT 0,
    input_tokens            INTEGER DEFAULT 0,
    output_tokens           INTEGER DEFAULT 0,
    title                   TEXT,
    workspace_root          TEXT,
    -- Phase 36.2 (D-USAGE-02): cache token + cost columns for usage ledger.
    cache_read_tokens       INTEGER DEFAULT 0,
    cache_creation_tokens   INTEGER DEFAULT 0,
    cost_usd_micros         INTEGER DEFAULT 0,
    FOREIGN KEY (parent_session_id) REFERENCES sessions(id)
);

-- Phase 36.2 (D-USAGE-02): per-turn usage ledger. One row per LLM call,
-- including failures (error_kind is NULL on success, ProviderError variant
-- name on failure). Costs stored as i64 micro-USD (1 USD = 1_000_000) — no
-- float drift (Pitfall 5). Indexes optimize /usage queries by session and time.
CREATE TABLE IF NOT EXISTS usage_events (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id       TEXT NOT NULL,
    ts               INTEGER NOT NULL,
    provider         TEXT NOT NULL,
    model            TEXT NOT NULL,
    in_tok           INTEGER NOT NULL,
    out_tok          INTEGER NOT NULL,
    cache_read       INTEGER NOT NULL DEFAULT 0,
    cache_create     INTEGER NOT NULL DEFAULT 0,
    cost_usd_micros  INTEGER NOT NULL DEFAULT 0,
    error_kind       TEXT
);
CREATE INDEX IF NOT EXISTS usage_events_session_idx ON usage_events(session_id);
CREATE INDEX IF NOT EXISTS usage_events_ts_idx ON usage_events(ts);

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

-- Phase 36.17.9 (gateway session persistence): durable SessionKey -> session_id
-- routing for messaging platforms (Telegram/Discord/Slack), plus per-session
-- voice mode. Lets the gateway RESUME an ongoing conversation after a restart
-- instead of minting a fresh session (reverses the old D-02 stateless default).
-- `session_key` is `SessionKey::to_string_key()` (e.g. `Telegram:12345:678`).
-- `voice_mode` is one of `off` | `on` | `tts`.
CREATE TABLE IF NOT EXISTS gateway_routes (
    session_key  TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    voice_mode   TEXT NOT NULL DEFAULT 'off',
    updated_at   REAL NOT NULL
);

-- Phase 46.7 (D-10/D-11/D-21): chat attachment metadata for web-chat uploads
-- (code/images/etc). `stored_rel_path` is RELATIVE to
-- `session_attachments_dir(session_id)` (opaque-id subdir + validated leaf) so
-- the row never hard-codes an absolute home path (D-21 redirect-safety). No
-- content-hash dedupe column (D-29) and no age/size reaper (D-28) — v1 scope
-- is intentionally minimal; attachment lifetime is tied to session lifetime.
CREATE TABLE IF NOT EXISTS chat_attachments (
    id               TEXT PRIMARY KEY,
    session_id       TEXT NOT NULL,
    message_id       TEXT,
    filename         TEXT NOT NULL,
    content_type     TEXT,
    size_bytes       INTEGER NOT NULL,
    stored_rel_path  TEXT NOT NULL,
    created_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chat_attachments_session ON chat_attachments(session_id);

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
    /// Phase 25.3 D-W-1: per-cwd workspace root resolved at session start.
    /// NULL for sessions created before Phase 25.3 or for sessions without a workspace marker.
    #[serde(default)]
    pub workspace_root: Option<String>,
}

/// A single per-turn LLM usage event row.
///
/// Phase 36.2 (D-USAGE-02): persisted per LLM call (success or failure) so
/// the `/usage` reader (downstream Plan 10) can slice by session / provider /
/// model / time / error.
///
/// **Cost units:** all monetary fields are i64 micro-USD (1 USD = 1_000_000).
/// This avoids the silent precision loss that would result from f64
/// representation (Pitfall 5 — float drift).
///
/// **`error_kind` discipline:** populated ONLY from
/// `ProviderError::variant_name()` (Plan 03) — a bounded set of ~10
/// compile-time constants (e.g., `"RateLimited"`, `"Auth"`, `"ContextLength"`).
/// Callers MUST NOT serialize the full `ProviderError` debug payload here:
/// it may contain raw HTTP error bodies that leak PII or secret prefixes
/// (T-36.2-02-PII mitigation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub session_id: String,
    /// Unix epoch milliseconds.
    pub ts: i64,
    pub provider: String,
    pub model: String,
    pub in_tok: i64,
    pub out_tok: i64,
    pub cache_read: i64,
    pub cache_create: i64,
    /// Micro-USD (1 USD = 1_000_000). 0 on failed calls.
    pub cost_usd_micros: i64,
    /// `None` on success; `Some(ProviderError::variant_name())` on failure.
    /// MUST NOT contain raw error bodies (PII risk; see struct-level doc).
    pub error_kind: Option<String>,
}

/// Filter for `StateStore::query_usage_events` (Phase 36.2 Plan 10).
///
/// **T-36.2-10-INJ:** every field value flows through `rusqlite::params!`
/// bindings inside `query_usage_events` — never `format!`-interpolated into
/// the SQL string. Constructed from user-supplied `--provider X` / `--model X`
/// / `--since 7d` / `--today` flags at the `/usage` handler entry point.
#[derive(Debug, Clone, Default)]
pub struct UsageFilter {
    /// When `Some`, restrict to a single session. Set to `None` for cross-
    /// session aggregations (`--today`, `--provider`, etc.).
    pub session_id: Option<String>,
    /// Restrict to rows whose `ts` is at or after local-midnight today
    /// (the cutoff is computed at query time).
    pub today_only: bool,
    /// Optional `--provider X` filter.
    pub provider: Option<String>,
    /// Optional `--model X` filter.
    pub model: Option<String>,
    /// Optional `--since Nd` / `--since Nh` / `--since Nm` rolling window,
    /// expressed in seconds. The cutoff (`now - since_seconds`) is computed
    /// at query time.
    pub since_seconds: Option<i64>,
}

/// One row of the `(provider, model)` aggregation returned by
/// `StateStore::query_usage_events` (Phase 36.2 Plan 10).
///
/// All numerics are `i64`; cost is in micro-USD per the D-USAGE-01
/// integer-microdollar discipline (no float drift). The display layer is
/// the ONLY place that converts to `f64` for human-readable rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRollup {
    pub provider: String,
    pub model: String,
    pub in_tok: i64,
    pub out_tok: i64,
    pub cache_read: i64,
    pub cache_create: i64,
    pub cost_usd_micros: i64,
    pub event_count: i64,
}

/// Phase 36.2 follow-up: per-call statistics from `backfill_usage_costs`.
/// `total_cost_delta_micros` is the signed sum (new − old); positive means
/// total recognized cost increased after the backfill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageBackfillStats {
    pub rows_examined: usize,
    pub rows_updated: usize,
    pub total_cost_delta_micros: i64,
    pub sessions_resynced: usize,
    /// Count of `usage_events` rows whose `session_id` does not match any
    /// `sessions.id`. These rows are still counted toward `rows_examined` /
    /// `rows_updated` / `total_cost_delta_micros`, but their cost does NOT
    /// roll up into a `sessions` row.
    pub orphan_rows: usize,
    /// Number of orphan rows actually DELETED from `usage_events` during the
    /// backfill. Zero unless `clean_orphans=true` was passed. Always ≤
    /// `orphan_rows`.
    pub orphans_deleted: usize,
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

/// A durable gateway routing record (Phase 36.17.9).
///
/// Maps a `SessionKey::to_string_key()` to the active `session_id` for a
/// messaging-platform conversation, plus that conversation's persisted voice
/// mode. Read on gateway startup so an inbound message resumes its prior
/// session instead of starting fresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRecord {
    /// Active session UUID this key currently routes to.
    pub session_id: String,
    /// Persisted voice mode for this conversation: `off` | `on` | `tts`.
    pub voice_mode: String,
}

/// A single `chat_attachments` row (Phase 46.7 D-10/D-11): metadata for one
/// file uploaded into a web-chat session (code/images/etc).
///
/// `stored_rel_path` is relative to `ironhermes_core::session_attachments_dir(session_id)`
/// — never an absolute path — so the row survives an `IRONHERMES_HOME` redirect
/// (D-21). `message_id` is `None` until the attachment is associated with a
/// specific turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatAttachmentRow {
    pub id: String,
    pub session_id: String,
    pub message_id: Option<String>,
    pub filename: String,
    pub content_type: Option<String>,
    pub size_bytes: i64,
    pub stored_rel_path: String,
    pub created_at: i64,
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

/// JSON export envelope for a session and its messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExport {
    pub session: Session,
    pub messages: Vec<StoredMessage>,
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
        if current < 8 {
            // v8 (Phase 25.3 D-W-1): add workspace_root TEXT NULL to sessions.
            // SQLite ALTER TABLE ADD COLUMN with no DEFAULT is valid — existing rows
            // get NULL automatically. The `let _ =` here tolerates "duplicate column"
            // ONLY if a previous partial migration ran the ALTER but didn't bump
            // schema_version; the `if current < 8` version gate is the primary guard
            // (RESEARCH.md Pitfall 5).
            let _ = self
                .conn
                .execute("ALTER TABLE sessions ADD COLUMN workspace_root TEXT", []);
            self.conn
                .execute("UPDATE schema_version SET version = 8", [])?;
        }
        if current < 9 {
            // v9 (Phase 36.2 D-USAGE-02): cache token columns on sessions +
            // usage_events table. `let _ =` tolerates "duplicate column" on partial
            // migration (Pitfall 4 / v8 precedent). SQLite ALTER TABLE adds one
            // column at a time — do NOT collapse into one statement. CREATE TABLE
            // / CREATE INDEX are guarded with IF NOT EXISTS so re-runs are safe.
            let _ = self.conn.execute(
                "ALTER TABLE sessions ADD COLUMN cache_read_tokens INTEGER DEFAULT 0",
                [],
            );
            let _ = self.conn.execute(
                "ALTER TABLE sessions ADD COLUMN cache_creation_tokens INTEGER DEFAULT 0",
                [],
            );
            let _ = self.conn.execute(
                "ALTER TABLE sessions ADD COLUMN cost_usd_micros INTEGER DEFAULT 0",
                [],
            );
            self.conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS usage_events (
                    id               INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id       TEXT NOT NULL,
                    ts               INTEGER NOT NULL,
                    provider         TEXT NOT NULL,
                    model            TEXT NOT NULL,
                    in_tok           INTEGER NOT NULL,
                    out_tok          INTEGER NOT NULL,
                    cache_read       INTEGER NOT NULL DEFAULT 0,
                    cache_create     INTEGER NOT NULL DEFAULT 0,
                    cost_usd_micros  INTEGER NOT NULL DEFAULT 0,
                    error_kind       TEXT
                );
                CREATE INDEX IF NOT EXISTS usage_events_session_idx ON usage_events(session_id);
                CREATE INDEX IF NOT EXISTS usage_events_ts_idx ON usage_events(ts);
                ",
            )?;
            self.conn
                .execute("UPDATE schema_version SET version = 9", [])?;
        }
        if current < 10 {
            // v10 (Phase 36.17.9): gateway_routes table for durable
            // SessionKey -> session_id routing + per-session voice mode.
            // CREATE TABLE IF NOT EXISTS is re-run-safe (v9 precedent).
            self.conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS gateway_routes (
                    session_key  TEXT PRIMARY KEY,
                    session_id   TEXT NOT NULL,
                    voice_mode   TEXT NOT NULL DEFAULT 'off',
                    updated_at   REAL NOT NULL
                );
                ",
            )?;
            self.conn
                .execute("UPDATE schema_version SET version = 10", [])?;
        }
        if current < 11 {
            // v11 (Phase 46.7 D-10/D-11): chat_attachments table for web-chat
            // upload metadata. CREATE TABLE / INDEX IF NOT EXISTS are re-run-safe
            // (v9/v10 precedent) and independently gated — never assumes the
            // immediately-prior version ran.
            self.conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS chat_attachments (
                    id               TEXT PRIMARY KEY,
                    session_id       TEXT NOT NULL,
                    message_id       TEXT,
                    filename         TEXT NOT NULL,
                    content_type     TEXT,
                    size_bytes       INTEGER NOT NULL,
                    stored_rel_path  TEXT NOT NULL,
                    created_at       INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_chat_attachments_session ON chat_attachments(session_id);
                ",
            )?;
            self.conn
                .execute("UPDATE schema_version SET version = 11", [])?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Session lifecycle
    // -----------------------------------------------------------------------

    /// Create a new session record.
    ///
    /// Phase 25.3 D-W-1: `workspace_root` is the resolved per-cwd workspace path or None
    /// for global/no-workspace sessions. Frozen at session start; never mutated mid-session.
    pub fn create_session(
        &mut self,
        id: &str,
        source: &str,
        model: Option<&str>,
        system_prompt: Option<&str>,
        parent_session_id: Option<&str>,
        workspace_root: Option<&str>,
    ) -> Result<()> {
        let now = unix_now();
        self.conn.execute(
            "INSERT OR IGNORE INTO sessions \
             (id, source, model, system_prompt, parent_session_id, started_at, workspace_root) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                source,
                model,
                system_prompt,
                parent_session_id,
                now,
                workspace_root
            ],
        )?;
        debug!(
            "created session {id} source={source} parent={parent_session_id:?} workspace_root={workspace_root:?}"
        );
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

    /// Delete a single session and its messages.
    ///
    /// Phase 36.7.1 Plan 07: the one gap in an otherwise complete store surface — the
    /// only pre-existing deletes are the bulk retention sweeps ([`Self::prune_sessions`]),
    /// filtered on an end timestamp, which cannot express "remove this one session".
    /// Deletes messages first (no CASCADE in the schema — matches `prune_sessions`'s own
    /// delete-messages-before-sessions ordering) so a delete never leaves orphaned message
    /// rows behind the sessions row it just dropped.
    ///
    /// Returns whether a session row was actually removed, so a caller (the REST delete
    /// route) can distinguish a real delete from an identifier that never existed rather
    /// than reporting a vacuous success either way.
    pub fn delete_session(&mut self, id: &str) -> Result<bool> {
        self.conn
            .execute("DELETE FROM messages WHERE session_id = ?1", params![id])?;
        let rows = self
            .conn
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    // -----------------------------------------------------------------------
    // Messages
    // -----------------------------------------------------------------------

    /// Append a [`ChatMessage`] to a session. Returns the new row id.
    pub fn add_message(&mut self, session_id: &str, msg: &ChatMessage) -> Result<i64> {
        let role = role_str(&msg.role);
        let content = msg
            .content
            .as_ref()
            .and_then(|c| c.as_text())
            .map(str::to_owned);
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
        let is_tool_call = msg
            .tool_calls
            .as_ref()
            .map(|v| !v.is_empty())
            .unwrap_or(false);
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
                 input_tokens, output_tokens, title, workspace_root \
                 FROM sessions WHERE id = ?1",
                params![id],
                session_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Retrieve all messages for a session ordered by per-session insert order.
    ///
    /// Phase 25.1 GAP-7: orders by `id ASC` (not `timestamp ASC`). Same-tick
    /// `add_message` calls share a millisecond-resolution `timestamp` and
    /// previously tied non-deterministically — scrambling assistant↔tool
    /// pairing on session restore and producing OpenAI 400s. `id` is the
    /// strictly-monotonic AUTOINCREMENT primary key (lib.rs:71); within a
    /// `WHERE session_id = ?1` filter, id-order equals per-session insert
    /// order.
    pub fn get_messages(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, role, content, tool_call_id, tool_calls, tool_name, \
             timestamp, token_count, finish_reason \
             FROM messages WHERE session_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![session_id], message_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Retrieve a session's messages already converted to [`ChatMessage`],
    /// ready to rehydrate an in-memory conversation (Phase 36.17.9 resume).
    ///
    /// Rows with an unrecognized role are skipped (defensive — keeps the
    /// assistant↔tool pairing intact rather than injecting a bogus turn).
    pub fn get_chat_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>> {
        Ok(self
            .get_messages(session_id)?
            .iter()
            .filter_map(chat_message_from_stored)
            .collect())
    }

    // -----------------------------------------------------------------------
    // Gateway routing (Phase 36.17.9 — durable SessionKey -> session_id)
    // -----------------------------------------------------------------------

    /// Insert or refresh the routing record for `session_key`, pointing it at
    /// `session_id`. Preserves any existing `voice_mode` (only the session id +
    /// timestamp change on conflict). Called write-through whenever the gateway
    /// creates or resumes a session.
    pub fn upsert_route(&mut self, session_key: &str, session_id: &str) -> Result<()> {
        let now = unix_now();
        self.conn.execute(
            "INSERT INTO gateway_routes (session_key, session_id, voice_mode, updated_at) \
             VALUES (?1, ?2, 'off', ?3) \
             ON CONFLICT(session_key) DO UPDATE SET \
                 session_id = excluded.session_id, \
                 updated_at = excluded.updated_at",
            params![session_key, session_id, now],
        )?;
        Ok(())
    }

    /// Look up the durable routing record for `session_key`, if any.
    pub fn get_route(&self, session_key: &str) -> Result<Option<RouteRecord>> {
        self.conn
            .query_row(
                "SELECT session_id, voice_mode FROM gateway_routes WHERE session_key = ?1",
                params![session_key],
                |r| {
                    Ok(RouteRecord {
                        session_id: r.get(0)?,
                        voice_mode: r.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Persist the voice mode (`off` | `on` | `tts`) for `session_key`.
    /// Preserves the existing `session_id` on conflict; if no route row exists
    /// yet (voice toggled before the first turn persisted one) a placeholder
    /// row is created with an empty `session_id`, which the next `upsert_route`
    /// fills in.
    pub fn set_route_voice_mode(&mut self, session_key: &str, voice_mode: &str) -> Result<()> {
        let now = unix_now();
        self.conn.execute(
            "INSERT INTO gateway_routes (session_key, session_id, voice_mode, updated_at) \
             VALUES (?1, '', ?2, ?3) \
             ON CONFLICT(session_key) DO UPDATE SET \
                 voice_mode = excluded.voice_mode, \
                 updated_at = excluded.updated_at",
            params![session_key, voice_mode, now],
        )?;
        Ok(())
    }

    /// Phase 47.5 (D-04): remove a chat's durable route so a reset session
    /// cannot be resumed; the next message mints a fresh session and
    /// re-points the route via upsert_route.
    pub fn delete_route(&mut self, session_key: &str) -> Result<()> {
        let rows = self.conn.execute(
            "DELETE FROM gateway_routes WHERE session_key = ?1",
            params![session_key],
        )?;
        if rows == 0 {
            warn!("delete_route: no route found for session_key={session_key}");
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Chat attachments (Phase 46.7 D-10/D-11/D-21 — web-chat upload metadata)
    // -----------------------------------------------------------------------

    /// Record metadata for one uploaded chat attachment (D-10/D-11). Does NOT
    /// write bytes to disk — callers (upload transport) write the file under
    /// `ironhermes_core::session_attachments_dir(session_id)` first, then pass
    /// the resulting `stored_rel_path` (relative to that dir) here.
    ///
    /// No content-hash dedupe (D-29): every call inserts a fresh row keyed by
    /// a newly generated opaque id, even if identical bytes were uploaded
    /// before.
    pub fn add_chat_attachment(
        &mut self,
        session_id: &str,
        message_id: Option<&str>,
        filename: &str,
        content_type: Option<&str>,
        size_bytes: i64,
        stored_rel_path: &str,
    ) -> Result<ChatAttachmentRow> {
        let id = new_attachment_id();
        let created_at = Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO chat_attachments \
             (id, session_id, message_id, filename, content_type, size_bytes, stored_rel_path, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                session_id,
                message_id,
                filename,
                content_type,
                size_bytes,
                stored_rel_path,
                created_at,
            ],
        )?;
        Ok(ChatAttachmentRow {
            id,
            session_id: session_id.to_string(),
            message_id: message_id.map(String::from),
            filename: filename.to_string(),
            content_type: content_type.map(String::from),
            size_bytes,
            stored_rel_path: stored_rel_path.to_string(),
            created_at,
        })
    }

    /// List all attachment metadata for a session, oldest first (D-10/D-11 retrieval).
    pub fn list_chat_attachments(&self, session_id: &str) -> Result<Vec<ChatAttachmentRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, message_id, filename, content_type, size_bytes, \
             stored_rel_path, created_at \
             FROM chat_attachments WHERE session_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![session_id], chat_attachment_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// List attachment metadata linked to a single message (D-10).
    pub fn list_chat_attachments_for_message(
        &self,
        message_id: &str,
    ) -> Result<Vec<ChatAttachmentRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, message_id, filename, content_type, size_bytes, \
             stored_rel_path, created_at \
             FROM chat_attachments WHERE message_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![message_id], chat_attachment_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Delete all `chat_attachments` rows for a session AND best-effort remove
    /// its on-disk attachments + workspace directories (D-28: attachment
    /// lifetime is tied to session lifetime — no separate age/size reaper).
    /// A session with no attachment rows is a DB-side no-op but still attempts
    /// the (already-absent) dir removal, which is harmless.
    ///
    /// `std::fs::remove_dir_all` errors are swallowed when the directory does
    /// not exist (`ErrorKind::NotFound`); any other IO error is surfaced.
    /// Returns the count of deleted `chat_attachments` rows.
    pub fn delete_chat_attachments_for_session(&mut self, session_id: &str) -> Result<usize> {
        // SEC-02: this is a `remove_dir_all` primitive keyed by a session id.
        // It has no non-test callers yet, which is exactly why the guard goes in
        // now — the first caller must not be able to reintroduce CR-01 with a
        // deletion blast radius. `session_*_dir` are pure joins, so an
        // unvalidated id (`../../..`) would recursively delete outside the
        // sessions root.
        if ironhermes_core::safe_session_id(session_id).is_none() {
            return Err(anyhow::anyhow!("invalid session id: {session_id:?}").into());
        }

        let deleted = self.conn.execute(
            "DELETE FROM chat_attachments WHERE session_id = ?1",
            params![session_id],
        )?;

        for dir in [
            session_attachments_dir(session_id),
            session_workspace_dir(session_id),
        ] {
            if let Err(e) = std::fs::remove_dir_all(&dir)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(
                    anyhow::anyhow!("remove_dir_all({}) failed: {e}", dir.display()).into(),
                );
            }
        }

        Ok(deleted)
    }

    /// List sessions, optionally filtered by source, most recent first.
    ///
    /// Backward-compat shim: pre-25.3 `list_sessions` signature, no workspace
    /// filter. Delegates to `list_sessions_filtered` with workspace_root_filter=None.
    pub fn list_sessions(&self, source: Option<&str>, limit: usize) -> Result<Vec<Session>> {
        self.list_sessions_filtered(source, limit, None)
    }

    /// Phase 25.3 D-W-2: list sessions, optionally filtered by source AND/OR workspace_root.
    ///
    /// `source`: filter by `Session.source` ("cli" / "telegram" / etc.) — pre-25.3 semantics.
    /// `limit`: caps the result count.
    /// `workspace_root_filter`:
    /// - `None`: return sessions ignoring workspace_root.
    /// - `Some("/path")`: return only sessions whose `workspace_root` column equals "/path".
    ///   Empty-string filter does NOT match NULL workspace_root rows (SQL NULL semantics).
    ///
    /// SQL injection is impossible by construction — both filter values are bound via rusqlite
    /// positional params (T-25.3-10-01 mitigation).
    pub fn list_sessions_filtered(
        &self,
        source: Option<&str>,
        limit: usize,
        workspace_root_filter: Option<&str>,
    ) -> Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source, user_id, model, system_prompt, parent_session_id, \
             started_at, ended_at, end_reason, message_count, tool_call_count, \
             input_tokens, output_tokens, title, workspace_root \
             FROM sessions \
             WHERE (?1 IS NULL OR source = ?1) \
               AND (?2 IS NULL OR workspace_root = ?2) \
             ORDER BY started_at DESC \
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![source, workspace_root_filter, limit as i64],
            session_from_row,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
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

    /// Update aggregate token, tool-call, cache, and cost statistics for a session.
    ///
    /// Phase 36.2 (D-USAGE-02): extended to aggregate cache_read_tokens,
    /// cache_creation_tokens, and cost_usd_micros (micro-USD i64; no float).
    /// All six numeric columns use additive (`+ ?N`) semantics — each call
    /// increments the row by the supplied deltas. Designed to be wrapped by
    /// the caller in a single rusqlite transaction together with
    /// [`Self::insert_usage_event`] for per-turn atomicity (Pitfall 6).
    //
    // 7 numeric columns + the row key is the data shape mandated by
    // 36.2-02-PLAN.md acceptance criteria; collapsing into a struct would
    // diverge from the call site in downstream Plan 07 and obscure the
    // INTEGER-micros invariant for each column. The allow is intentional.
    #[allow(clippy::too_many_arguments)]
    pub fn update_session_stats(
        &mut self,
        id: &str,
        input_tokens: i64,
        output_tokens: i64,
        tool_call_count: i64,
        cache_read_tokens: i64,
        cache_creation_tokens: i64,
        cost_usd_micros: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET \
             input_tokens = input_tokens + ?1, \
             output_tokens = output_tokens + ?2, \
             tool_call_count = tool_call_count + ?3, \
             cache_read_tokens = cache_read_tokens + ?4, \
             cache_creation_tokens = cache_creation_tokens + ?5, \
             cost_usd_micros = cost_usd_micros + ?6 \
             WHERE id = ?7",
            params![
                input_tokens,
                output_tokens,
                tool_call_count,
                cache_read_tokens,
                cache_creation_tokens,
                cost_usd_micros,
                id
            ],
        )?;
        Ok(())
    }

    /// Insert a single per-turn usage event row.
    ///
    /// Phase 36.2 (D-USAGE-02): designed to be called inside the same
    /// rusqlite transaction as [`Self::update_session_stats`] for atomic
    /// per-turn writes (Pitfall 6). All SQL uses `params!` bindings —
    /// never string interpolation (T-36.2-02-INJ mitigation).
    pub fn insert_usage_event(&mut self, ev: &UsageEvent) -> Result<()> {
        self.conn.execute(
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
        )?;
        Ok(())
    }

    /// Test-only accessor for the underlying rusqlite Connection.
    ///
    /// Phase 36.2 (D-USAGE-02): the per-turn write path is two SQL
    /// statements (INSERT usage_events + UPDATE sessions) that MUST execute
    /// inside a single transaction. The transaction is owned by the caller
    /// (the agent loop, downstream Plan 07), not by the StateStore methods,
    /// so integration tests need direct connection access to construct an
    /// `unchecked_transaction()` and verify commit/rollback semantics.
    #[doc(hidden)]
    pub fn conn_for_test(&self) -> &Connection {
        &self.conn
    }

    /// Recompute `cost_usd_micros` on every `usage_events` row using the
    /// caller-supplied closure, then resync each session's aggregate column.
    /// Atomic: either all changes commit, or none. `dry_run = true` rolls
    /// back so operators can preview the impact.
    ///
    /// The closure receives `(model, in_tok, out_tok, cache_read, cache_create)`
    /// and must return the new `cost_usd_micros` value (i64 micro-USD).
    ///
    /// Phase 36.2 follow-up: existing rows were written when the disk-resident
    /// pricing cache was not threaded onto AgentLoop (every OpenRouter slug
    /// like `google/gemini-3.5-flash` got cost=0). This method lets the
    /// `hermes pricing backfill` CLI command retroactively populate the
    /// cost column using current pricing.
    pub fn backfill_usage_costs<F>(
        &mut self,
        mut recompute: F,
        dry_run: bool,
        clean_orphans: bool,
    ) -> Result<UsageBackfillStats>
    where
        F: FnMut(&str, i64, i64, i64, i64) -> i64,
    {
        let tx = self.conn.unchecked_transaction()?;
        let mut stats = UsageBackfillStats::default();

        // Collect all rows first so we can release the SELECT statement
        // before issuing UPDATEs (rusqlite borrows the connection mutably
        // for the duration of a prepared statement iter).
        #[allow(clippy::type_complexity)]
        // DB row tuple: one-off local; type alias would only exist here, inline is clearer
        let rows: Vec<(i64, String, String, i64, i64, i64, i64, i64)> = {
            let mut stmt = tx.prepare(
                "SELECT rowid, session_id, model, in_tok, out_tok, cache_read, cache_create, cost_usd_micros \
                 FROM usage_events",
            )?;
            let iter = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            })?;
            iter.collect::<rusqlite::Result<Vec<_>>>()?
        };

        stats.rows_examined = rows.len();

        // Count orphans (session_id with no matching sessions.id) — these
        // sum into total cost but never roll up into a sessions row. Report
        // but don't fix here (cleanup is a separate decision).
        let mut session_ids_present: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        {
            let mut stmt = tx.prepare("SELECT id FROM sessions")?;
            let iter = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for row in iter {
                session_ids_present.insert(row?);
            }
        }

        for (rowid, session_id, model, in_tok, out_tok, cache_read, cache_create, old_cost) in &rows
        {
            if !session_ids_present.contains(session_id) {
                stats.orphan_rows += 1;
            }
            let new_cost = recompute(model, *in_tok, *out_tok, *cache_read, *cache_create);
            if new_cost != *old_cost {
                stats.rows_updated += 1;
                stats.total_cost_delta_micros = stats
                    .total_cost_delta_micros
                    .saturating_add(new_cost - old_cost);
                tx.execute(
                    "UPDATE usage_events SET cost_usd_micros = ?1 WHERE rowid = ?2",
                    rusqlite::params![new_cost, rowid],
                )?;
            }
        }

        // Optional orphan cleanup: delete usage_events rows whose session_id
        // has no matching sessions.id. Runs BEFORE the sessions aggregate
        // resync so the SUM reflects the post-cleanup state. Always reports
        // the count via stats.orphans_deleted; in dry-run mode the rollback
        // below undoes the actual delete.
        if clean_orphans {
            stats.orphans_deleted = tx.execute(
                "DELETE FROM usage_events WHERE session_id NOT IN (SELECT id FROM sessions)",
                [],
            )?;
        }

        // Resync sessions.cost_usd_micros = SUM of matching usage_events.
        // The COALESCE handles sessions with no usage rows (sum is NULL).
        let sessions_resynced = tx.execute(
            "UPDATE sessions \
             SET cost_usd_micros = COALESCE((\
                 SELECT SUM(cost_usd_micros) FROM usage_events \
                 WHERE usage_events.session_id = sessions.id\
             ), 0)",
            [],
        )?;
        stats.sessions_resynced = sessions_resynced;

        if dry_run {
            tx.rollback()?;
        } else {
            tx.commit()?;
        }
        Ok(stats)
    }

    /// Aggregate `usage_events` rows by `(provider, model)` under the given
    /// filter (Phase 36.2 Plan 10).
    ///
    /// **T-36.2-10-INJ mitigation:** the SQL string is composed with an
    /// append-only conditional pattern over a fixed `"... WHERE 1=1"` base.
    /// Every user-supplied filter value is bound via `rusqlite::params!` —
    /// never `format!`-interpolated into the SQL. The b1 integration test
    /// (`crates/ironhermes-cli/tests/usage_command.rs`) verifies this with a
    /// malicious provider string that contains `'; DROP TABLE ...--`; the
    /// table survives the call and 0 rows are returned (no match).
    pub fn query_usage_events(&self, filter: &UsageFilter) -> Result<Vec<UsageRollup>> {
        let mut sql = String::from(
            "SELECT provider, model, \
             SUM(in_tok), SUM(out_tok), SUM(cache_read), SUM(cache_create), \
             SUM(cost_usd_micros), COUNT(*) \
             FROM usage_events WHERE 1=1",
        );
        let mut bound: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(sid) = &filter.session_id {
            sql.push_str(" AND session_id = ?");
            bound.push(Box::new(sid.clone()));
        }
        if filter.today_only {
            // Local-midnight today as unix epoch ms.
            let start_ms = chrono::Local::now()
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .expect("00:00:00 is always a valid time of day")
                .and_local_timezone(chrono::Local)
                .single()
                .map(|dt| dt.timestamp_millis())
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() - 86_400_000);
            sql.push_str(" AND ts >= ?");
            bound.push(Box::new(start_ms));
        }
        if let Some(provider) = &filter.provider {
            sql.push_str(" AND provider = ?");
            bound.push(Box::new(provider.clone()));
        }
        if let Some(model) = &filter.model {
            sql.push_str(" AND model = ?");
            bound.push(Box::new(model.clone()));
        }
        if let Some(secs) = filter.since_seconds {
            let cutoff_ms = chrono::Utc::now().timestamp_millis() - secs * 1000;
            sql.push_str(" AND ts >= ?");
            bound.push(Box::new(cutoff_ms));
        }
        sql.push_str(" GROUP BY provider, model ORDER BY SUM(cost_usd_micros) DESC");

        let param_refs: Vec<&dyn rusqlite::ToSql> = bound.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows: rusqlite::Result<Vec<UsageRollup>> = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(UsageRollup {
                    provider: row.get(0)?,
                    model: row.get(1)?,
                    in_tok: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    out_tok: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    cache_read: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    cache_create: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    cost_usd_micros: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                    event_count: row.get(7)?,
                })
            })?
            .collect();
        Ok(rows?)
    }

    /// Set or replace the human-readable title for a session.
    pub fn update_session_title(&mut self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    /// Set or replace the model a session is locked to.
    ///
    /// Phase 36.7.1 Plan 07: the caller (the REST model-lock route) validates the model
    /// against the model registry BEFORE calling this — this method performs no
    /// validation of its own, matching `update_session_title`'s own "just write the
    /// column" shape.
    pub fn update_session_model(&mut self, id: &str, model: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET model = ?1 WHERE id = ?2",
            params![model, id],
        )?;
        Ok(())
    }

    /// Look up a single session by its unique title.
    pub fn get_session_by_title(&self, title: &str) -> Result<Option<Session>> {
        self.conn
            .query_row(
                "SELECT id, source, user_id, model, system_prompt, parent_session_id, \
                 started_at, ended_at, end_reason, message_count, tool_call_count, \
                 input_tokens, output_tokens, title, workspace_root \
                 FROM sessions WHERE title = ?1",
                params![title],
                session_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Run a passive WAL checkpoint to keep the WAL file from growing unbounded.
    pub fn wal_checkpoint(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Export & Prune
    // -----------------------------------------------------------------------

    /// Export a single session with all its messages as a structured object.
    pub fn export_session(&self, session_id: &str) -> Result<SessionExport> {
        let session = self
            .get_session(session_id)?
            .ok_or_else(|| StateError::SessionNotFound(session_id.to_string()))?;
        let messages = self.get_messages(session_id)?;
        Ok(SessionExport { session, messages })
    }

    /// Export multiple sessions, optionally filtered by source.
    /// Returns a Vec of SessionExport (each with session metadata + messages).
    pub fn export_sessions(&self, source: Option<&str>) -> Result<Vec<SessionExport>> {
        let sessions = self.list_sessions(source, usize::MAX)?;
        let mut exports = Vec::with_capacity(sessions.len());
        for session in sessions {
            let messages = self.get_messages(&session.id)?;
            exports.push(SessionExport { session, messages });
        }
        Ok(exports)
    }

    /// Delete ended sessions older than `older_than_days` and their messages.
    /// Only prunes sessions where `ended_at IS NOT NULL`.
    /// Returns the count of deleted sessions.
    pub fn prune_sessions(&mut self, older_than_days: u32, source: Option<&str>) -> Result<usize> {
        let cutoff = unix_now() - (older_than_days as f64 * 86400.0);

        // Build the session selection subquery
        let (session_sql, delete_sql) = if source.is_some() {
            (
                "SELECT id FROM sessions WHERE ended_at IS NOT NULL AND ended_at < ?1 AND source = ?2",
                "DELETE FROM sessions WHERE ended_at IS NOT NULL AND ended_at < ?1 AND source = ?2",
            )
        } else {
            (
                "SELECT id FROM sessions WHERE ended_at IS NOT NULL AND ended_at < ?1",
                "DELETE FROM sessions WHERE ended_at IS NOT NULL AND ended_at < ?1",
            )
        };

        // Delete messages first (no CASCADE in schema)
        let msg_sql = format!("DELETE FROM messages WHERE session_id IN ({session_sql})");
        if let Some(src) = source {
            self.conn.execute(&msg_sql, params![cutoff, src])?;
        } else {
            self.conn.execute(&msg_sql, params![cutoff])?;
        }

        // Delete sessions
        let deleted = if let Some(src) = source {
            self.conn.execute(delete_sql, params![cutoff, src])?
        } else {
            self.conn.execute(delete_sql, params![cutoff])?
        };

        debug!("pruned {deleted} ended session(s) older than {older_than_days} days");
        Ok(deleted)
    }
}

// ---------------------------------------------------------------------------
// FTS5 sanitization
// ---------------------------------------------------------------------------

/// Render a `UsageFilter` + rollup list as a human-readable table string.
///
/// Phase 36.2 Plan 10 (D-USAGE-03): the single, canonical text renderer used
/// by the `/usage` slash command across every platform (CLI, TUI, gateway,
/// web UI). Display is the ONLY place i64 micro-USD becomes f64 — every
/// upstream arithmetic operation stays integer (Pitfall 5: float drift).
///
/// The "Cost" column prints with `${:.4}` (four decimal places) so a 234_000
/// micro-USD value renders as `$0.2340`. Total cost line at the bottom uses
/// the same precision so column totals match the row totals byte-for-byte.
pub fn format_usage_rollups(rollups: &[UsageRollup], filter: &UsageFilter) -> String {
    if rollups.is_empty() {
        return "No usage data found for this filter.".to_string();
    }
    let mut out = String::new();
    out.push_str("Usage\n");
    if let Some(sid) = &filter.session_id {
        out.push_str(&format!("Session: {sid}\n"));
    }
    out.push_str(&format!(
        "{:<10} {:<24} {:>8} {:>8} {:>8} {:>8} {:>11}\n",
        "Provider", "Model", "In tok", "Out tok", "Cache R", "Cache C", "Cost"
    ));
    let mut total_cost: i64 = 0;
    for r in rollups {
        let cost_usd = r.cost_usd_micros as f64 / 1_000_000.0;
        out.push_str(&format!(
            "{:<10} {:<24} {:>8} {:>8} {:>8} {:>8} ${:>10.4}\n",
            r.provider, r.model, r.in_tok, r.out_tok, r.cache_read, r.cache_create, cost_usd
        ));
        total_cost += r.cost_usd_micros;
    }
    out.push_str(&format!(
        "Total cost: ${:.4}\n",
        total_cost as f64 / 1_000_000.0
    ));
    out
}

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
///
/// NOTE: pre-existing helper kept for future SESS-13 wiring; #[allow] added
/// in Phase 36.2 Plan 02 so the new acceptance criterion `clippy -D warnings`
/// passes. Re-enable as part of any future SQLITE_BUSY retry phase.
#[allow(dead_code)]
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
#[allow(dead_code)]
fn is_busy(e: &StateError) -> bool {
    if let StateError::Sqlite(sq) = e {
        matches!(
            sq.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        )
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

/// Generate an opaque `catt_` + 16 hex-char attachment id (Phase 46.7).
/// Mirrors `ironhermes-kanban::Store::new_id`'s v4-UUID-simple-truncated
/// convention.
fn new_attachment_id() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("catt_{}", &id[..16])
}

fn role_str(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn chat_attachment_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChatAttachmentRow> {
    Ok(ChatAttachmentRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        message_id: r.get(2)?,
        filename: r.get(3)?,
        content_type: r.get(4)?,
        size_bytes: r.get(5)?,
        stored_rel_path: r.get(6)?,
        created_at: r.get(7)?,
    })
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
        // Phase 25.3 D-W-1: workspace_root may be absent if the SELECT statement
        // uses a fixed column list. Tolerate either presence or absence so callers
        // that issue narrower SELECTs (no workspace_root in projection) keep working.
        workspace_root: r.get(14).ok(),
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

/// Convert a persisted [`StoredMessage`] back into a [`ChatMessage`] for
/// session rehydration (Phase 36.17.9). Returns `None` for an unrecognized
/// role so a corrupt row is skipped rather than breaking assistant↔tool
/// pairing on resume. `tool_calls` JSON that fails to parse is dropped (the
/// text content is still preserved). Canonical converter — surfaces shared by
/// the gateway resume path and the web UI session view.
pub fn chat_message_from_stored(row: &StoredMessage) -> Option<ChatMessage> {
    use ironhermes_core::types::MessageContent;
    let role = match row.role.as_str() {
        "system" => Role::System,
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => return None,
    };
    let tool_calls = row
        .tool_calls
        .as_ref()
        .and_then(|json| serde_json::from_str(json).ok());
    Some(ChatMessage {
        role,
        content: row.content.clone().map(MessageContent::Text),
        tool_calls,
        tool_call_id: row.tool_call_id.clone(),
        name: row.tool_name.clone(),
        is_recall_context: false,
    })
}

// ---------------------------------------------------------------------------
// Phase 36.2 Plan 07 (fixup): shared StateStoreHandle adapter
// ---------------------------------------------------------------------------
//
// Wraps `Arc<Mutex<StateStore>>` and implements the
// `ironhermes_core::commands::context::StateStoreHandle` trait so the slash
// command handlers — including `/usage` (Plan 10) — run against the real DB
// from every surface (CLI, TUI, web). Without this, `iron_hermes_ui` had no
// way to wire a `StateStoreHandle` into `CommandContext` and every slash
// command intercepted in `ws.rs` returned "Session storage not configured."
//
// Mirrors `tui_rata::commands::StateStoreAdapter` so both surfaces emit
// byte-identical text. Lives in `ironhermes-state` because that's where
// `UsageFilter` / `format_usage_rollups` live and adding the impl here
// avoids a circular dependency from `ironhermes-core`.

use ironhermes_core::commands::context::StateStoreHandle as CoreStateStoreHandle;

/// `Arc<Mutex<StateStore>>` → `dyn StateStoreHandle` adapter.
///
/// Wrap with `Arc::new(StateStoreHandleAdapter(store.clone()))` and pass into
/// `CommandContext::with_state_store(...)`. Synchronous; callers that hold
/// the tokio runtime should wrap dispatch in `tokio::task::block_in_place`.
pub struct StateStoreHandleAdapter(pub std::sync::Arc<std::sync::Mutex<StateStore>>);

impl CoreStateStoreHandle for StateStoreHandleAdapter {
    fn list_sessions_text(&self, limit: usize) -> String {
        self.list_sessions_text_filtered(limit, None)
    }

    fn list_sessions_text_filtered(&self, limit: usize, workspace_root: Option<&str>) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.list_sessions_filtered(None, limit, workspace_root) {
            Ok(sessions) if sessions.is_empty() => match workspace_root {
                Some(ws) => format!("No sessions found for workspace: {ws}"),
                None => "No sessions found.".to_string(),
            },
            Ok(sessions) => {
                let lines: Vec<String> = sessions.iter().map(|s| format!("  {}", s.id)).collect();
                let header = match workspace_root {
                    Some(ws) => format!("Recent sessions (workspace={ws}):"),
                    None => "Recent sessions:".to_string(),
                };
                format!("{header}\n{}", lines.join("\n"))
            }
            Err(e) => format!("Error listing sessions: {e}"),
        }
    }

    fn history_text(&self, session_id: &str) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.get_messages(session_id) {
            Ok(msgs) if msgs.is_empty() => "No messages in history.".to_string(),
            Ok(msgs) => {
                let lines: Vec<String> = msgs
                    .iter()
                    .map(|m| format!("  [{}] {}", m.role, m.content.as_deref().unwrap_or("")))
                    .collect();
                format!("History ({} messages):\n{}", msgs.len(), lines.join("\n"))
            }
            Err(e) => format!("Error loading history: {e}"),
        }
    }

    fn export_session_text(&self, session_id: &str) -> String {
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.export_session(session_id) {
            Ok(export) => format!("Session exported: {} messages.", export.messages.len()),
            Err(e) => format!("Error exporting session: {e}"),
        }
    }

    fn update_title(&self, session_id: &str, title: &str) -> Result<(), String> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| "StateStore lock poisoned.".to_string())?;
        guard
            .update_session_title(session_id, title)
            .map_err(|e| e.to_string())
    }

    fn get_session_id(&self, name_or_id: &str) -> Option<String> {
        let guard = self.0.lock().ok()?;
        if let Ok(Some(s)) = guard.get_session(name_or_id) {
            return Some(s.id);
        }
        guard
            .get_session_by_title(name_or_id)
            .ok()
            .flatten()
            .map(|s| s.id)
    }

    fn usage_text(
        &self,
        session_id: Option<&str>,
        today_only: bool,
        provider: Option<&str>,
        model: Option<&str>,
        since_seconds: Option<i64>,
    ) -> String {
        let filter = UsageFilter {
            session_id: session_id.map(|s| s.to_string()),
            today_only,
            provider: provider.map(|s| s.to_string()),
            model: model.map(|s| s.to_string()),
            since_seconds,
        };
        let guard = match self.0.lock() {
            Ok(g) => g,
            Err(_) => return "StateStore lock poisoned.".to_string(),
        };
        match guard.query_usage_events(&filter) {
            Ok(rows) => format_usage_rollups(&rows, &filter),
            Err(e) => format!("Usage query failed: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 25.3 Plan 10 Task 2: list_sessions_filtered tests (D-W-2)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod list_sessions_filtered_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_sessions_filtered_by_workspace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("a", "cli", None, None, None, Some("/repo/x"))
            .unwrap();
        store
            .create_session("b", "cli", None, None, None, Some("/repo/y"))
            .unwrap();
        store
            .create_session("c", "cli", None, None, None, None)
            .unwrap();

        let all = store.list_sessions(None, 100).unwrap();
        assert_eq!(all.len(), 3, "no filter -> all sessions");

        let only_x = store
            .list_sessions_filtered(None, 100, Some("/repo/x"))
            .unwrap();
        assert_eq!(only_x.len(), 1);
        assert_eq!(only_x[0].id, "a");

        // empty-string filter does NOT match NULL workspace_root
        let only_global = store.list_sessions_filtered(None, 100, Some("")).unwrap();
        assert_eq!(only_global.len(), 0);

        // None filter behaves identically to list_sessions(None, ...)
        let unfiltered = store.list_sessions_filtered(None, 100, None).unwrap();
        assert_eq!(unfiltered.len(), 3);

        // source filter still works alongside workspace filter
        let cli_x = store
            .list_sessions_filtered(Some("cli"), 100, Some("/repo/x"))
            .unwrap();
        assert_eq!(cli_x.len(), 1);
        assert_eq!(cli_x[0].id, "a");
    }
}

// ---------------------------------------------------------------------------
// Phase 25.3 Plan 0 Task 2: Schema migration v8 — workspace_root column (D-W-1)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod schema_migration_v8_tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    // NOTE: the SCHEMA_VERSION pin previously lived here as
    // `schema_version_constant_is_10`. Removed (Phase 46.7) because
    // `assert_eq!(SCHEMA_VERSION, N)` against the CURRENT compile-time
    // constant becomes `clippy::assertions_on_constants` once it's rewritten
    // as a plain bool comparison, and duplicating the exact-value pin across
    // two modules just churns on every future bump. The single source of
    // truth for the current SCHEMA_VERSION is now
    // `chat_attachment_tests::schema_version_constant_is_11` (history: v8
    // workspace_root -> v9 cache tokens + usage_events -> v10 gateway_routes
    // -> v11 chat_attachments).

    #[test]
    fn fresh_install_has_workspace_root_column() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let _store = StateStore::new(&path).expect("open fresh store");
        let conn = Connection::open(&path).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(sessions)").unwrap();
        let cols: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            cols.iter()
                .any(|(n, t)| n == "workspace_root" && t.eq_ignore_ascii_case("TEXT")),
            "v8 fresh install must include workspace_root TEXT in sessions; got cols={:?}",
            cols
        );
    }

    #[test]
    fn v7_db_upgrades_to_latest_preserving_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        // Manually create a v7 DB shape (no workspace_root, no cache/cost cols)
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL); \
                 INSERT INTO schema_version (version) VALUES (7); \
                 CREATE TABLE sessions ( \
                   id TEXT PRIMARY KEY, source TEXT NOT NULL, user_id TEXT, model TEXT, \
                   system_prompt TEXT, parent_session_id TEXT, started_at REAL NOT NULL, \
                   ended_at REAL, end_reason TEXT, message_count INTEGER DEFAULT 0, \
                   tool_call_count INTEGER DEFAULT 0, input_tokens INTEGER DEFAULT 0, \
                   output_tokens INTEGER DEFAULT 0, title TEXT \
                 ); \
                 INSERT INTO sessions (id, source, started_at) VALUES ('legacy-1', 'cli', 1.0);",
            )
            .unwrap();
        }
        // Open with the new code — should ALTER + bump to current SCHEMA_VERSION
        let _store = StateStore::new(&path).expect("upgrade open");
        let conn = Connection::open(&path).unwrap();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            v, SCHEMA_VERSION,
            "schema_version must be SCHEMA_VERSION after upgrade"
        );
        // Phase 25.3 invariant: workspace_root added with NULL default for pre-v8 rows.
        let wr: Option<String> = conn
            .query_row(
                "SELECT workspace_root FROM sessions WHERE id = 'legacy-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            wr.is_none(),
            "pre-v8 row must have NULL workspace_root after upgrade"
        );
        // Phase 36.2 invariant: cache + cost cols added with 0 default for pre-v9 rows.
        let (cr, cc, cost): (i64, i64, i64) = conn
            .query_row(
                "SELECT cache_read_tokens, cache_creation_tokens, cost_usd_micros \
                 FROM sessions WHERE id = 'legacy-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (cr, cc, cost),
            (0, 0, 0),
            "pre-v9 row must default to 0 on new cache/cost cols"
        );
        // Phase 36.2 invariant: usage_events table created during v9 migration.
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='usage_events'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            exists, 1,
            "usage_events table must exist after v9 migration"
        );
        // Phase 36.17.9 invariant: gateway_routes table created during v10 migration.
        let routes_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='gateway_routes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            routes_exists, 1,
            "gateway_routes table must exist after v10 migration"
        );
    }

    #[test]
    fn gateway_route_upsert_get_and_voice_mode_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        let key = "Telegram:12345:678";

        // Missing key → None.
        assert_eq!(store.get_route(key).unwrap(), None);

        // Upsert points the key at a session; voice_mode defaults to "off".
        store.upsert_route(key, "sess-aaa").unwrap();
        let r = store.get_route(key).unwrap().expect("route present");
        assert_eq!(r.session_id, "sess-aaa");
        assert_eq!(r.voice_mode, "off");

        // Setting voice mode preserves the session_id.
        store.set_route_voice_mode(key, "tts").unwrap();
        let r = store.get_route(key).unwrap().unwrap();
        assert_eq!(
            r.session_id, "sess-aaa",
            "voice update must keep session_id"
        );
        assert_eq!(r.voice_mode, "tts");

        // Re-pointing the route to a new session preserves the voice mode.
        store.upsert_route(key, "sess-bbb").unwrap();
        let r = store.get_route(key).unwrap().unwrap();
        assert_eq!(r.session_id, "sess-bbb", "upsert must update session_id");
        assert_eq!(r.voice_mode, "tts", "upsert must preserve voice_mode");
    }

    #[test]
    fn set_voice_mode_before_route_then_upsert_fills_session_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        let key = "Discord:room-9";

        // Voice toggled before any session persisted → placeholder row.
        store.set_route_voice_mode(key, "on").unwrap();
        let r = store.get_route(key).unwrap().unwrap();
        assert_eq!(r.session_id, "", "no session yet → empty placeholder");
        assert_eq!(r.voice_mode, "on");

        // First turn persists the session id, keeping the chosen voice mode.
        store.upsert_route(key, "sess-ccc").unwrap();
        let r = store.get_route(key).unwrap().unwrap();
        assert_eq!(r.session_id, "sess-ccc");
        assert_eq!(r.voice_mode, "on");
    }

    #[test]
    fn gateway_route_delete_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        let key = "Telegram:99999:111";

        store.upsert_route(key, "sess-zzz").unwrap();
        assert!(
            store.get_route(key).unwrap().is_some(),
            "route present before delete"
        );

        store.delete_route(key).unwrap();
        assert_eq!(
            store.get_route(key).unwrap(),
            None,
            "route must resolve to None after delete"
        );
    }

    #[test]
    fn gateway_route_delete_missing_key_is_noop() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();

        // Deleting a key that was never written must not error.
        store.delete_route("never-written-key").unwrap();
        assert_eq!(store.get_route("never-written-key").unwrap(), None);
    }

    #[test]
    fn get_chat_messages_rehydrates_with_tool_pairing() {
        use ironhermes_core::types::{MessageContent, ToolCall};
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("sess-1", "telegram", Some("m"), None, None, None)
            .unwrap();

        // user → assistant(tool_call) → tool(result) — the pairing-sensitive trio.
        let user = ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Text("hi".into())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        };
        let assistant = ChatMessage {
            role: Role::Assistant,
            content: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".into(),
                call_type: "function".into(),
                function: ironhermes_core::types::FunctionCall {
                    name: "ping".into(),
                    arguments: "{}".into(),
                },
            }]),
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        };
        let tool = ChatMessage {
            role: Role::Tool,
            content: Some(MessageContent::Text("pong".into())),
            tool_calls: None,
            tool_call_id: Some("call_1".into()),
            name: Some("ping".into()),
            is_recall_context: false,
        };
        store.add_message("sess-1", &user).unwrap();
        store.add_message("sess-1", &assistant).unwrap();
        store.add_message("sess-1", &tool).unwrap();

        let msgs = store.get_chat_messages("sess-1").unwrap();
        assert_eq!(msgs.len(), 3, "all three turns rehydrate in order");
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
        assert_eq!(
            msgs[1].tool_calls.as_ref().map(|t| t[0].id.clone()),
            Some("call_1".to_string()),
            "assistant tool_call id survives rehydration"
        );
        assert_eq!(msgs[2].role, Role::Tool);
        assert_eq!(
            msgs[2].tool_call_id.as_deref(),
            Some("call_1"),
            "tool result keeps its tool_call_id (pairing intact)"
        );
    }

    #[test]
    fn create_session_with_workspace_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("s1", "cli", None, None, None, Some("/repo/foo"))
            .unwrap();
        store
            .create_session("s2", "cli", None, None, None, None)
            .unwrap();
        let conn = Connection::open(&path).unwrap();
        let r1: Option<String> = conn
            .query_row(
                "SELECT workspace_root FROM sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let r2: Option<String> = conn
            .query_row(
                "SELECT workspace_root FROM sessions WHERE id = 's2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(r1.as_deref(), Some("/repo/foo"));
        assert!(r2.is_none());
    }

    /// Phase 36.2 follow-up: backfill recomputes cost_usd_micros and resyncs
    /// the sessions aggregate column. Dry-run rolls back; non-dry-run commits.
    #[test]
    fn backfill_usage_costs_dry_run_then_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();

        // Seed a session and three usage_events rows with cost=0 (the
        // pre-fix state). Plus one orphan row (session_id has no matching
        // sessions.id) to test the orphan counter.
        store
            .create_session("sess-1", "telegram", Some("gpt-4o"), None, None, None)
            .unwrap();
        let conn = store.conn_for_test();
        conn.execute(
            "INSERT INTO usage_events (session_id, ts, provider, model, in_tok, out_tok, \
             cache_read, cache_create, cost_usd_micros, error_kind) \
             VALUES ('sess-1', 1, 'openai', 'gpt-4o', 1000, 500, 0, 0, 0, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (session_id, ts, provider, model, in_tok, out_tok, \
             cache_read, cache_create, cost_usd_micros, error_kind) \
             VALUES ('sess-1', 2, 'openai', 'gpt-4o', 2000, 1000, 0, 0, 0, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (session_id, ts, provider, model, in_tok, out_tok, \
             cache_read, cache_create, cost_usd_micros, error_kind) \
             VALUES ('orphan-sid', 3, 'openai', 'gpt-4o', 100, 50, 0, 0, 0, NULL)",
            [],
        )
        .unwrap();

        // Recompute closure: stub pricing — every row gets new_cost = in_tok * 2.
        let recompute =
            |_model: &str, in_tok: i64, _out: i64, _cr: i64, _cc: i64| -> i64 { in_tok * 2 };

        // Dry-run: report what would change without persisting.
        let stats = store.backfill_usage_costs(recompute, true, false).unwrap();
        assert_eq!(stats.rows_examined, 3);
        assert_eq!(stats.rows_updated, 3);
        assert_eq!(stats.orphan_rows, 1);
        assert_eq!(
            stats.orphans_deleted, 0,
            "dry-run without clean-orphans deletes nothing"
        );
        // Cost delta = (2000 + 4000 + 200) - 0 = 6200
        assert_eq!(stats.total_cost_delta_micros, 6200);

        // Verify dry-run did NOT commit — rows still at cost=0.
        let conn = store.conn_for_test();
        let cost_after_dry_run: i64 = conn
            .query_row("SELECT SUM(cost_usd_micros) FROM usage_events", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(cost_after_dry_run, 0, "dry-run must not commit");

        // Apply for real.
        let stats = store.backfill_usage_costs(recompute, false, false).unwrap();
        assert_eq!(stats.rows_updated, 3);
        assert_eq!(stats.total_cost_delta_micros, 6200);
        assert_eq!(stats.orphans_deleted, 0);

        // Verify rows now have new cost.
        let conn = store.conn_for_test();
        let cost_after_apply: i64 = conn
            .query_row("SELECT SUM(cost_usd_micros) FROM usage_events", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(cost_after_apply, 6200);

        // sess-1 aggregate = sum of its two non-orphan rows = 2000 + 4000 = 6000.
        let session_cost: i64 = conn
            .query_row(
                "SELECT cost_usd_micros FROM sessions WHERE id = 'sess-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(session_cost, 6000);
    }

    /// Backfill is idempotent: running twice on the same closure produces
    /// rows_updated = 0 on the second pass.
    #[test]
    fn backfill_usage_costs_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("sess-1", "telegram", Some("gpt-4o"), None, None, None)
            .unwrap();
        store
            .conn_for_test()
            .execute(
                "INSERT INTO usage_events (session_id, ts, provider, model, in_tok, out_tok, \
                 cache_read, cache_create, cost_usd_micros, error_kind) \
                 VALUES ('sess-1', 1, 'openai', 'gpt-4o', 1000, 500, 0, 0, 0, NULL)",
                [],
            )
            .unwrap();
        let recompute = |_m: &str, in_tok: i64, _o: i64, _cr: i64, _cc: i64| in_tok * 2;

        let s1 = store.backfill_usage_costs(recompute, false, false).unwrap();
        assert_eq!(s1.rows_updated, 1);

        let s2 = store.backfill_usage_costs(recompute, false, false).unwrap();
        assert_eq!(s2.rows_updated, 0, "second run should be a no-op");
        assert_eq!(s2.total_cost_delta_micros, 0);
    }

    /// Phase 36.2 follow-up: --clean-orphans deletes usage_events rows whose
    /// session_id has no matching sessions.id and reports the count.
    #[test]
    fn backfill_usage_costs_clean_orphans_deletes_unmatched_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("sess-1", "telegram", Some("gpt-4o"), None, None, None)
            .unwrap();
        let conn = store.conn_for_test();
        // One valid row, two orphan rows.
        conn.execute(
            "INSERT INTO usage_events (session_id, ts, provider, model, in_tok, out_tok, \
             cache_read, cache_create, cost_usd_micros, error_kind) \
             VALUES ('sess-1', 1, 'openai', 'gpt-4o', 100, 50, 0, 0, 0, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (session_id, ts, provider, model, in_tok, out_tok, \
             cache_read, cache_create, cost_usd_micros, error_kind) \
             VALUES ('gw:7018:7018', 2, 'openai', 'gpt-4o', 100, 50, 0, 0, 999, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events (session_id, ts, provider, model, in_tok, out_tok, \
             cache_read, cache_create, cost_usd_micros, error_kind) \
             VALUES ('Telegram:1:1', 3, 'openai', 'gpt-4o', 100, 50, 0, 0, 888, NULL)",
            [],
        )
        .unwrap();

        let identity = |_: &str, _: i64, _: i64, _: i64, _: i64| -> i64 { 0 };

        // Dry-run with clean_orphans=true: report counts, no DB mutation.
        let stats = store.backfill_usage_costs(identity, true, true).unwrap();
        assert_eq!(stats.orphan_rows, 2);
        assert_eq!(stats.orphans_deleted, 2);
        let count_after_dry: i64 = store
            .conn_for_test()
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after_dry, 3, "dry-run must not delete");

        // Apply for real.
        let stats = store.backfill_usage_costs(identity, false, true).unwrap();
        assert_eq!(stats.orphans_deleted, 2);
        let count_after_apply: i64 = store
            .conn_for_test()
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after_apply, 1, "orphans deleted, valid row preserved");

        // A second pass finds 0 orphans (already cleaned).
        let stats = store.backfill_usage_costs(identity, false, true).unwrap();
        assert_eq!(stats.orphan_rows, 0);
        assert_eq!(stats.orphans_deleted, 0);
    }
}

// ---------------------------------------------------------------------------
// Phase 46.7 Plan 01 Task 2: Schema migration v11 — chat_attachments (D-10/D-11)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod chat_attachment_tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;
    use tempfile::tempdir;

    /// Tests that manipulate IRONHERMES_HOME must hold this lock to avoid
    /// env var races (Rust tests run in parallel). Mirrors the
    /// `ironhermes-agent::prompt_builder` ENV_MUTEX convention.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn schema_version_constant_is_11() {
        assert_eq!(
            SCHEMA_VERSION, 11,
            "Phase 46.7: SCHEMA_VERSION must be bumped to 11 (chat_attachments)"
        );
    }

    #[test]
    fn v10_db_migrates_to_v11_and_creates_chat_attachments() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        // Stamp a v10-shaped fixture: schema_version=10, no chat_attachments table.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL); \
                 INSERT INTO schema_version (version) VALUES (10); \
                 CREATE TABLE sessions ( \
                   id TEXT PRIMARY KEY, source TEXT NOT NULL, user_id TEXT, model TEXT, \
                   system_prompt TEXT, parent_session_id TEXT, started_at REAL NOT NULL, \
                   ended_at REAL, end_reason TEXT, message_count INTEGER DEFAULT 0, \
                   tool_call_count INTEGER DEFAULT 0, input_tokens INTEGER DEFAULT 0, \
                   output_tokens INTEGER DEFAULT 0, title TEXT, workspace_root TEXT, \
                   cache_read_tokens INTEGER DEFAULT 0, cache_creation_tokens INTEGER DEFAULT 0, \
                   cost_usd_micros INTEGER DEFAULT 0 \
                 ); \
                 CREATE TABLE gateway_routes ( \
                   session_key TEXT PRIMARY KEY, session_id TEXT NOT NULL, \
                   voice_mode TEXT NOT NULL DEFAULT 'off', updated_at REAL NOT NULL \
                 );",
            )
            .unwrap();
        }

        let _store = StateStore::new(&path).expect("upgrade open from v10 fixture");
        let conn = Connection::open(&path).unwrap();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, 11, "schema_version must be 11 after v10->v11 migration");

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_attachments'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            exists, 1,
            "chat_attachments table must exist after v11 migration"
        );
    }

    #[test]
    fn v8_db_skips_intermediate_versions_and_still_creates_chat_attachments() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        // Stamp a v8-shaped fixture (skips v9/v10 entirely) — each migration
        // block must be independently gated, never assume the immediately
        // prior version ran.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL); \
                 INSERT INTO schema_version (version) VALUES (8); \
                 CREATE TABLE sessions ( \
                   id TEXT PRIMARY KEY, source TEXT NOT NULL, user_id TEXT, model TEXT, \
                   system_prompt TEXT, parent_session_id TEXT, started_at REAL NOT NULL, \
                   ended_at REAL, end_reason TEXT, message_count INTEGER DEFAULT 0, \
                   tool_call_count INTEGER DEFAULT 0, input_tokens INTEGER DEFAULT 0, \
                   output_tokens INTEGER DEFAULT 0, title TEXT, workspace_root TEXT \
                 );",
            )
            .unwrap();
        }

        let _store = StateStore::new(&path).expect("upgrade open from v8 fixture");
        let conn = Connection::open(&path).unwrap();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION, "must land on current SCHEMA_VERSION");

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_attachments'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            exists, 1,
            "chat_attachments must exist even skipping v9/v10 fixtures (each block independently gated)"
        );
    }

    #[test]
    fn add_and_list_chat_attachments_round_trips_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("sess-att-1", "web", None, None, None, None)
            .unwrap();

        let row = store
            .add_chat_attachment(
                "sess-att-1",
                None,
                "photo.png",
                Some("image/png"),
                1234,
                "att_abc/photo.png",
            )
            .unwrap();
        assert_eq!(row.session_id, "sess-att-1");
        assert_eq!(row.filename, "photo.png");

        let listed = store.list_chat_attachments("sess-att-1").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].filename, "photo.png");
        assert_eq!(listed[0].content_type.as_deref(), Some("image/png"));
        assert_eq!(listed[0].size_bytes, 1234);
        assert_eq!(listed[0].stored_rel_path, "att_abc/photo.png");
    }

    #[test]
    fn list_chat_attachments_for_message_filters_by_message_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("sess-att-2", "web", None, None, None, None)
            .unwrap();

        store
            .add_chat_attachment(
                "sess-att-2",
                Some("msg-1"),
                "a.txt",
                Some("text/plain"),
                10,
                "att_a/a.txt",
            )
            .unwrap();
        store
            .add_chat_attachment(
                "sess-att-2",
                Some("msg-2"),
                "b.txt",
                Some("text/plain"),
                20,
                "att_b/b.txt",
            )
            .unwrap();
        store
            .add_chat_attachment(
                "sess-att-2",
                None,
                "c.txt",
                Some("text/plain"),
                30,
                "att_c/c.txt",
            )
            .unwrap();

        let for_msg1 = store.list_chat_attachments_for_message("msg-1").unwrap();
        assert_eq!(for_msg1.len(), 1);
        assert_eq!(for_msg1[0].filename, "a.txt");

        let for_msg2 = store.list_chat_attachments_for_message("msg-2").unwrap();
        assert_eq!(for_msg2.len(), 1);
        assert_eq!(for_msg2[0].filename, "b.txt");
    }

    /// SEC-02 regression: this fn `remove_dir_all`s two session-keyed dirs.
    /// An unvalidated traversal id would recursively delete OUTSIDE the sessions
    /// root. It has no non-test callers yet — the guard exists so the first one
    /// cannot reintroduce CR-01 with a deletion blast radius.
    #[test]
    fn delete_chat_attachments_rejects_a_traversal_session_id() {
        let dir = tempfile::tempdir().unwrap();
        // No IRONHERMES_HOME redirect needed (and so no ENV_MUTEX): rejection
        // must happen BEFORE any path is resolved or any fs call is made. If
        // this test ever needs the env, the guard has regressed.
        let mut store = StateStore::new(dir.path().join("state.db")).unwrap();

        let canary = dir.path().join("canary.txt");
        std::fs::write(&canary, b"must survive").unwrap();

        for evil in ["../../../tmp/evil", "..", "a/b", "a\\b", "~root", ""] {
            let err = store.delete_chat_attachments_for_session(evil);
            assert!(
                err.is_err(),
                "traversal session id {evil:?} must be rejected before any remove_dir_all"
            );
        }
        assert!(
            canary.exists(),
            "nothing outside the sessions root may be deleted"
        );
    }

    #[test]
    fn delete_chat_attachments_for_session_removes_rows_and_dirs_and_is_noop_when_empty() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        // SAFETY: env var mutation is guarded by ENV_MUTEX above so no other
        // test observes a torn IRONHERMES_HOME value concurrently.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", dir.path());
        }
        let mut store = StateStore::new(&path).unwrap();
        store
            .create_session("sess-att-3", "web", None, None, None, None)
            .unwrap();

        store
            .add_chat_attachment(
                "sess-att-3",
                None,
                "photo.png",
                Some("image/png"),
                100,
                "att_x/photo.png",
            )
            .unwrap();

        // Create the on-disk dirs the way a real upload path would.
        let att_dir = session_attachments_dir("sess-att-3");
        let ws_dir = session_workspace_dir("sess-att-3");
        std::fs::create_dir_all(&att_dir).unwrap();
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(att_dir.join("marker.txt"), b"x").unwrap();
        assert!(att_dir.exists());
        assert!(ws_dir.exists());

        let deleted = store
            .delete_chat_attachments_for_session("sess-att-3")
            .unwrap();
        assert_eq!(deleted, 1);
        assert!(
            store
                .list_chat_attachments("sess-att-3")
                .unwrap()
                .is_empty()
        );
        assert!(!att_dir.exists(), "attachments dir must be removed");
        assert!(!ws_dir.exists(), "workspace dir must be removed");

        // No-op for a session with no attachments/dirs — must not error.
        let deleted_again = store
            .delete_chat_attachments_for_session("sess-att-3")
            .unwrap();
        assert_eq!(deleted_again, 0);

        // SAFETY: still holding ENV_MUTEX from above.
        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }
    }
}
