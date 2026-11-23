//! SQLite claim store for Tier 2 (`<home>/tier2/tier2.db`).
//!
//! The schema created here is the on-disk contract every later 58.2 plan reads. Every query binds
//! parameters; nothing model-written is formatted into SQL (T-58.2-01).

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use anyhow::Context;
use ironhermes_core::tier2::TIER2_EMBEDDING_DIM;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: &str = "1";

/// Full Tier 2 schema (RESEARCH "Recommended SQLite schema"). Idempotent.
pub const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS claims(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    claim_id TEXT NOT NULL UNIQUE,
    text TEXT NOT NULL,
    text_hash TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('world','experience','opinion','reflection')),
    subject TEXT NOT NULL,
    origin TEXT NOT NULL CHECK(origin IN ('consolidation','tier1')),
    confidence REAL NOT NULL,
    durable INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','superseded','staged_forget')),
    superseded_by INTEGER REFERENCES claims(id),
    source_url TEXT,
    session_id TEXT,
    source_surface TEXT,
    user_id TEXT,
    tier1_target TEXT,
    tier1_ordinal INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_claims_text_hash ON claims(text_hash);
CREATE INDEX IF NOT EXISTS idx_claims_origin ON claims(origin);

CREATE TABLE IF NOT EXISTS evidence(
    id INTEGER PRIMARY KEY,
    claim_pk INTEGER NOT NULL REFERENCES claims(id) ON DELETE CASCADE,
    source_url TEXT NOT NULL,
    quote TEXT NOT NULL,
    session_id TEXT,
    message_id INTEGER,
    ts TEXT NOT NULL,
    UNIQUE(claim_pk, message_id, source_url)
);

CREATE TABLE IF NOT EXISTS embeddings(
    claim_pk INTEGER PRIMARY KEY REFERENCES claims(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    vec BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS tombstones(text_hash TEXT PRIMARY KEY, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tombstone_messages(message_id INTEGER PRIMARY KEY);

CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(
    text,
    content=claims,
    content_rowid=id
);

CREATE TRIGGER IF NOT EXISTS claims_fts_insert AFTER INSERT ON claims BEGIN
    INSERT INTO claims_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER IF NOT EXISTS claims_fts_delete AFTER DELETE ON claims BEGIN
    INSERT INTO claims_fts(claims_fts, rowid, text) VALUES('delete', old.id, old.text);
END;

CREATE TRIGGER IF NOT EXISTS claims_fts_update AFTER UPDATE ON claims BEGIN
    INSERT INTO claims_fts(claims_fts, rowid, text) VALUES('delete', old.id, old.text);
    INSERT INTO claims_fts(rowid, text) VALUES (new.id, new.text);
END;
";

/// Trim, collapse internal whitespace, lowercase.
pub fn normalize_for_hash(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// sha256 hex of the normalized text (tombstone + dedup key).
pub fn text_hash(text: &str) -> String {
    let digest = Sha256::digest(normalize_for_hash(text).as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Public claim id: `claim:` + 12 lowercase hex.
pub fn new_claim_id() -> String {
    let simple = uuid::Uuid::new_v4().simple().to_string();
    format!("claim:{}", &simple[..12])
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub struct NewClaim {
    pub text: String,
    pub kind: String,
    pub subject: String,
    pub origin: String,
    pub confidence: f64,
    pub source_url: Option<String>,
    pub tier1_target: Option<String>,
    pub tier1_ordinal: Option<i64>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClaimRow {
    pub id: i64,
    pub claim_id: String,
    pub text: String,
    pub text_hash: String,
    pub kind: String,
    pub subject: String,
    pub origin: String,
    pub confidence: f64,
    pub durable: bool,
    pub status: String,
    pub superseded_by: Option<i64>,
    pub source_url: Option<String>,
    pub session_id: Option<String>,
    pub source_surface: Option<String>,
    pub user_id: Option<String>,
    pub tier1_target: Option<String>,
    pub tier1_ordinal: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

const CLAIM_COLS: &str = "c.id, c.claim_id, c.text, c.text_hash, c.kind, c.subject, c.origin, \
    c.confidence, c.durable, c.status, c.superseded_by, c.source_url, c.session_id, \
    c.source_surface, c.user_id, c.tier1_target, c.tier1_ordinal, c.created_at, c.updated_at";

fn claim_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ClaimRow> {
    Ok(ClaimRow {
        id: r.get(0)?,
        claim_id: r.get(1)?,
        text: r.get(2)?,
        text_hash: r.get(3)?,
        kind: r.get(4)?,
        subject: r.get(5)?,
        origin: r.get(6)?,
        confidence: r.get(7)?,
        durable: r.get::<_, i64>(8)? != 0,
        status: r.get(9)?,
        superseded_by: r.get(10)?,
        source_url: r.get(11)?,
        session_id: r.get(12)?,
        source_surface: r.get(13)?,
        user_id: r.get(14)?,
        tier1_target: r.get(15)?,
        tier1_ordinal: r.get(16)?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
    })
}

/// One evidence row backing a claim.
#[derive(Debug, Clone)]
pub struct EvidenceRow {
    pub source_url: String,
    pub quote: String,
    pub session_id: Option<String>,
    pub message_id: Option<i64>,
    pub ts: String,
}

/// A claim, its evidence (oldest first), and the claim ids it superseded.
pub type ClaimExplain = (ClaimRow, Vec<EvidenceRow>, Vec<String>);

/// `meta` key: highest state.db message id consolidation has processed (D-15).
pub const WATERMARK_KEY: &str = "watermark_message_id";
/// `meta` key: unix seconds until which a consolidation run holds the single-flight lease.
pub const LEASE_KEY: &str = "consolidation_lease_until";

/// `meta` key: unix seconds until which a dreams run holds its own single-flight lease.
pub const DREAMS_LEASE_KEY: &str = "dreams_lease_until";

/// One evidence row of a consolidated claim: a cited conversation message.
pub struct EvidenceInput {
    pub source_url: String,
    pub quote: String,
    pub session_id: Option<String>,
    pub message_id: i64,
    pub ts: String,
}

/// A claim accepted by consolidation. `existing_pk: Some` merges evidence into that active claim
/// (exact text-hash match); `None` inserts a new claim under `claim_id`.
pub struct ConsolidatedClaim {
    pub claim_id: String,
    pub existing_pk: Option<i64>,
    pub text: String,
    pub kind: String,
    pub subject: String,
    pub confidence: f64,
    pub durable: bool,
    pub source_url: String,
    pub session_id: Option<String>,
    pub evidence: Vec<EvidenceInput>,
    /// `(model_id, vector)` stored with a NEW claim in the same transaction.
    pub embedding: Option<(String, Vec<f32>)>,
    /// Active claim (pk) this NEW claim supersedes (D-18).
    pub supersedes_pk: Option<i64>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct CommitOutcome {
    pub added: usize,
    pub merged: usize,
    pub superseded: usize,
    /// `(pk, vector)` of each new claim stored with an embedding: what the index must now add.
    pub embedded: Vec<(i64, Vec<f32>)>,
}

/// `meta` key bumped (under `BEGIN IMMEDIATE`) every time `index.usearch` is rewritten.
pub const INDEX_GENERATION_KEY: &str = "index_generation";

/// All `(claim_pk, vector)` rows stored for `model_id`, ordered by claim pk.
pub(crate) fn embeddings_on(
    conn: &Connection,
    model_id: &str,
) -> anyhow::Result<Vec<(i64, Vec<f32>)>> {
    let mut stmt =
        conn.prepare("SELECT claim_pk, vec FROM embeddings WHERE model_id = ?1 ORDER BY claim_pk")?;
    let rows = stmt.query_map(params![model_id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (pk, blob) = row?;
        out.extend(decode_vec(&blob).map(|v| (pk, v)));
    }
    Ok(out)
}

/// A blob of the wrong length yields `None`: it can only come from a foreign writer, and an
/// index must never be handed a vector it would reject (D-06).
fn decode_vec(blob: &[u8]) -> Option<Vec<f32>> {
    (blob.len() == TIER2_EMBEDDING_DIM * 4).then(|| {
        blob.as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    })
}

fn generation_on(conn: &Connection) -> anyhow::Result<u64> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![INDEX_GENERATION_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening tier2 db {}", path.display()))?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // RESEARCH Pitfall 1: a DELETE must overwrite the freed pages (plan-09 purge).
        conn.pragma_update(None, "secure_delete", "ON")?;
        conn.execute_batch(SCHEMA_SQL)?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES('schema_version', ?1)",
            params![SCHEMA_VERSION],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> anyhow::Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("tier2 store mutex poisoned"))
    }

    /// Run `f` on the store's connection (one statement sequence at a time). `pub(crate)` for the
    /// purge's scrub, which issues FTS5 and WAL maintenance statements.
    pub(crate) fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let conn = self.conn()?;
        f(&conn)
    }

    pub fn insert_claim(&self, c: &NewClaim) -> anyhow::Result<ClaimRow> {
        let conn = self.conn()?;
        let ts = now();
        conn.execute(
            "INSERT INTO claims(claim_id, text, text_hash, kind, subject, origin, confidence, \
             source_url, session_id, tier1_target, tier1_ordinal, created_at, updated_at) \
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![
                new_claim_id(),
                c.text,
                text_hash(&c.text),
                c.kind,
                c.subject,
                c.origin,
                c.confidence,
                c.source_url,
                c.session_id,
                c.tier1_target,
                c.tier1_ordinal,
                ts,
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(conn.query_row(
            &format!("SELECT {CLAIM_COLS} FROM claims c WHERE c.id = ?1"),
            params![id],
            claim_row,
        )?)
    }

    /// Re-point a Tier 1 claim at its current entry (file and ordinal): entries shift when an
    /// earlier one is removed, and a moved claim re-links when its text is re-added, possibly to
    /// the other file. The claim id and text are unchanged.
    pub fn set_tier1_position(
        &self,
        id: i64,
        target: &str,
        ordinal: i64,
        source_url: &str,
    ) -> anyhow::Result<()> {
        self.conn()?.execute(
            "UPDATE claims SET tier1_target = ?2, tier1_ordinal = ?3, source_url = ?4, \
             updated_at = ?5 WHERE id = ?1",
            params![id, target, ordinal, source_url, now()],
        )?;
        Ok(())
    }

    /// D-29: move Tier 1 claims whose line left Tier 1 onto the live log. Each tuple is
    /// `(claim pk, live URL, quote)`. The claim keeps its id, text, confidence, embedding and its
    /// original `memory://tier1` evidence; it gains a live-log evidence row. The WHERE clause makes
    /// a second, concurrent mover a no-op. Returns the number of claims re-pointed.
    pub fn move_tier1_to_live(&self, moves: &[(i64, String, String)]) -> anyhow::Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = now();
        let mut moved = 0;
        for (pk, url, quote) in moves {
            let n = tx.execute(
                "UPDATE claims SET tier1_ordinal = NULL, source_url = ?2, updated_at = ?3 \
                 WHERE id = ?1 AND tier1_ordinal IS NOT NULL AND status = 'active'",
                params![pk, url, ts],
            )?;
            if n == 1 {
                tx.execute(
                    "INSERT OR IGNORE INTO evidence(claim_pk, source_url, quote, session_id, message_id, ts) \
                     VALUES(?1, ?2, ?3, NULL, NULL, ?4)",
                    params![pk, url, quote, ts],
                )?;
                moved += 1;
            }
        }
        tx.commit()?;
        Ok(moved)
    }

    /// D-31 daily note: one claim with the given id plus one evidence row, in one transaction.
    /// Day-to-day detail is non-durable (`durable = 0`), so derived files, dreams synthesis and
    /// promotion skip it while search finds it. Returns the claim pk.
    pub fn insert_note(&self, claim_id: &str, text: &str, source_url: &str) -> anyhow::Result<i64> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = now();
        tx.execute(
            "INSERT INTO claims(claim_id, text, text_hash, kind, subject, origin, confidence, \
             durable, source_url, created_at, updated_at) \
             VALUES(?1, ?2, ?3, 'experience', 'user', 'consolidation', 0.9, 0, ?4, ?5, ?5)",
            params![claim_id, text, text_hash(text), source_url, ts],
        )?;
        let pk = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO evidence(claim_pk, source_url, quote, session_id, message_id, ts) \
             VALUES(?1, ?2, ?3, NULL, NULL, ?4)",
            params![pk, source_url, text, ts],
        )?;
        tx.commit()?;
        Ok(pk)
    }

    /// Hard-delete claims by primary key (evidence/embeddings cascade; FTS via trigger).
    pub fn delete_claims(&self, ids: &[i64]) -> anyhow::Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM claims WHERE id = ?1", params![id])?;
        }
        Ok(tx.commit()?)
    }

    /// Attach one evidence row to a claim. A repeat of `(claim, message_id, source)` is ignored.
    pub fn insert_evidence(
        &self,
        claim_pk: i64,
        source_url: &str,
        quote: &str,
        session_id: Option<&str>,
        message_id: Option<i64>,
    ) -> anyhow::Result<()> {
        self.conn()?.execute(
            "INSERT OR IGNORE INTO evidence(claim_pk, source_url, quote, session_id, message_id, ts) \
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![claim_pk, source_url, quote, session_id, message_id, now()],
        )?;
        Ok(())
    }

    /// An active claim with its evidence (oldest first) and the claim ids it superseded.
    /// Superseded and staged-for-forget claims are invisible here (T-58.2-14).
    pub fn explain_row(&self, claim_id: &str) -> anyhow::Result<Option<ClaimExplain>> {
        let conn = self.conn()?;
        let Some(row) = conn
            .query_row(
                &format!("SELECT {CLAIM_COLS} FROM claims c WHERE c.claim_id = ?1 AND c.status = 'active'"),
                params![claim_id],
                claim_row,
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut stmt = conn.prepare(
            "SELECT source_url, quote, session_id, message_id, ts FROM evidence \
             WHERE claim_pk = ?1 ORDER BY ts, id",
        )?;
        let evidence = stmt
            .query_map(params![row.id], |r| {
                Ok(EvidenceRow {
                    source_url: r.get(0)?,
                    quote: r.get(1)?,
                    session_id: r.get(2)?,
                    message_id: r.get(3)?,
                    ts: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt =
            conn.prepare("SELECT claim_id FROM claims WHERE superseded_by = ?1 ORDER BY id")?;
        let supersedes = stmt
            .query_map(params![row.id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(Some((row, evidence, supersedes)))
    }

    /// Active claims, newest first.
    pub fn list_recent(&self, limit: usize) -> anyhow::Result<Vec<ClaimRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CLAIM_COLS} FROM claims c WHERE c.status = 'active' \
             ORDER BY c.created_at DESC, c.id DESC LIMIT ?1"
        ))?;
        Ok(stmt
            .query_map(params![limit as i64], claim_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Forbid re-indexing any Tier 1 entry whose normalized text hashes to `text_hash`.
    pub fn insert_tombstone(&self, text_hash: &str) -> anyhow::Result<()> {
        self.conn()?.execute(
            "INSERT OR IGNORE INTO tombstones(text_hash, created_at) VALUES(?1, ?2)",
            params![text_hash, now()],
        )?;
        Ok(())
    }

    /// Every claim linked to, moved out of, or claimed for a Tier 1 line, any origin, by id
    /// (D-29/D-30). Every tier1-origin row carries a `tier1_target`.
    pub fn tier1_claims(&self) -> anyhow::Result<Vec<ClaimRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CLAIM_COLS} FROM claims c WHERE c.tier1_target IS NOT NULL ORDER BY c.id"
        ))?;
        Ok(stmt
            .query_map([], claim_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// D-30: claim up to `limit` promotion candidates as `(claim_id, text)`: active, durable,
    /// direct (>= 0.95) `world`/`opinion` consolidation claims about `user` that are not yet in
    /// Tier 1, not tombstoned and not already linked. Select and mark share one IMMEDIATE
    /// transaction, so two processes cannot both claim one fact.
    pub fn claim_promotions(&self, limit: usize) -> anyhow::Result<Vec<(String, String)>> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let picked: Vec<(i64, String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT c.id, c.claim_id, c.text FROM claims c \
                 WHERE c.origin = 'consolidation' AND c.status = 'active' AND c.durable = 1 \
                 AND c.confidence >= 0.95 AND c.kind IN ('world','opinion') \
                 AND c.subject = 'user' AND c.tier1_target IS NULL \
                 AND NOT EXISTS (SELECT 1 FROM claims o WHERE o.text_hash = c.text_hash \
                                 AND o.tier1_ordinal IS NOT NULL) \
                 AND NOT EXISTS (SELECT 1 FROM tombstones t WHERE t.text_hash = c.text_hash) \
                 ORDER BY c.confidence DESC, c.updated_at DESC, c.id LIMIT ?1",
            )?;
            stmt.query_map(params![limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        // The mark puts the claim in `tier1_claims`, so the sync after the Tier 1 write links it
        // instead of inserting a duplicate. 'user' is provisional: the hash fallback in
        // `sync_tier1` re-targets it if the line lands in MEMORY.md.
        for (id, _, _) in &picked {
            tx.execute(
                "UPDATE claims SET tier1_target = 'user' WHERE id = ?1 AND tier1_target IS NULL",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(picked.into_iter().map(|(_, id, text)| (id, text)).collect())
    }

    /// D-30: un-mark a claimed candidate whose Tier 1 add found no room, so a later run offers it
    /// again. Never unlinks a claim whose line landed, never touches a tier1-origin claim.
    pub fn release_promotion(&self, claim_id: &str) -> anyhow::Result<bool> {
        Ok(self.conn()?.execute(
            "UPDATE claims SET tier1_target = NULL WHERE claim_id = ?1 \
             AND tier1_ordinal IS NULL AND origin = 'consolidation'",
            params![claim_id],
        )? > 0)
    }

    /// Active, durable claims written by consolidation (never Tier 1 mirrors), oldest first:
    /// the input of the derived bank/people/groups files.
    pub fn derived_claims(&self) -> anyhow::Result<Vec<ClaimRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CLAIM_COLS} FROM claims c WHERE c.origin = 'consolidation' \
             AND c.status = 'active' AND c.durable = 1 ORDER BY c.created_at, c.id"
        ))?;
        Ok(stmt
            .query_map([], claim_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Active consolidation claims created or updated at or after `since` (an RFC 3339 UTC
    /// timestamp as written by this store): the dreams review window.
    pub fn consolidation_claims_since(&self, since: &str) -> anyhow::Result<Vec<ClaimRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CLAIM_COLS} FROM claims c WHERE c.origin = 'consolidation' \
             AND c.status = 'active' AND (c.created_at >= ?1 OR c.updated_at >= ?1) \
             ORDER BY c.created_at, c.id"
        ))?;
        Ok(stmt
            .query_map(params![since], claim_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Up to `limit` active, durable consolidation claims, most confident then most recent first:
    /// the dreams synthesis input.
    pub fn top_durable_claims(&self, limit: usize) -> anyhow::Result<Vec<ClaimRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CLAIM_COLS} FROM claims c WHERE c.origin = 'consolidation' \
             AND c.status = 'active' AND c.durable = 1 \
             ORDER BY c.confidence DESC, c.updated_at DESC, c.id DESC LIMIT ?1"
        ))?;
        Ok(stmt
            .query_map(params![limit as i64], claim_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// True when this text must not be (re-)learned: purged (tombstone) or staged to forget.
    /// The staged check matters while the purge is deferred (D-27): no tombstone is ever written.
    pub fn is_tombstoned(&self, text_hash: &str) -> anyhow::Result<bool> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                "SELECT 1 FROM tombstones WHERE text_hash = ?1 \
                 UNION ALL SELECT 1 FROM claims WHERE text_hash = ?1 AND status = 'staged_forget' \
                 LIMIT 1",
                params![text_hash],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Store (or replace) the embedding of a claim. The vector must be exactly
    /// `TIER2_EMBEDDING_DIM` long (D-06); `model_id` records which embedder produced it.
    pub fn upsert_embedding(&self, claim_pk: i64, model_id: &str, v: &[f32]) -> anyhow::Result<()> {
        anyhow::ensure!(
            v.len() == TIER2_EMBEDDING_DIM,
            "embedding has {} dimensions, expected {TIER2_EMBEDDING_DIM}",
            v.len()
        );
        let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.conn()?.execute(
            "INSERT INTO embeddings(claim_pk, model_id, vec) VALUES(?1, ?2, ?3) \
             ON CONFLICT(claim_pk) DO UPDATE SET model_id = excluded.model_id, vec = excluded.vec",
            params![claim_pk, model_id, blob],
        )?;
        Ok(())
    }

    pub fn embeddings_for_model(&self, model_id: &str) -> anyhow::Result<Vec<(i64, Vec<f32>)>> {
        let conn = self.conn()?;
        embeddings_on(&conn, model_id)
    }

    pub fn count_embeddings(&self, model_id: &str) -> anyhow::Result<i64> {
        Ok(self.conn()?.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE model_id = ?1",
            params![model_id],
            |r| r.get(0),
        )?)
    }

    /// Up to `limit` `(claim pk, text)` pairs of active claims with no embedding for `model_id`
    /// (none at all, or one produced by a different model).
    pub fn claims_needing_embedding(
        &self,
        model_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(i64, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.text FROM claims c \
             WHERE c.status = 'active' AND NOT EXISTS \
             (SELECT 1 FROM embeddings e WHERE e.claim_pk = c.id AND e.model_id = ?1) \
             ORDER BY c.id LIMIT ?2",
        )?;
        Ok(stmt
            .query_map(params![model_id, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// `(active claims without a `model_id` embedding, embeddings made by any other model)`.
    pub fn embedding_backlog(&self, model_id: &str) -> anyhow::Result<(i64, i64)> {
        let conn = self.conn()?;
        let pending = conn.query_row(
            "SELECT COUNT(*) FROM claims c WHERE c.status = 'active' AND NOT EXISTS \
             (SELECT 1 FROM embeddings e WHERE e.claim_pk = c.id AND e.model_id = ?1)",
            params![model_id],
            |r| r.get(0),
        )?;
        let stale = conn.query_row(
            // Only active claims are ever re-embedded; counting superseded/staged rows' old
            // vectors would keep the backlog above zero forever after a model switch.
            "SELECT COUNT(*) FROM embeddings e JOIN claims c ON c.id = e.claim_pk \
             WHERE c.status = 'active' AND e.model_id != ?1",
            params![model_id],
            |r| r.get(0),
        )?;
        Ok((pending, stale))
    }

    /// Primary keys of active claims embedded with `model_id`: the only vectors a search may score.
    pub fn active_embedded_pks(&self, model_id: &str) -> anyhow::Result<HashSet<u64>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT e.claim_pk FROM embeddings e JOIN claims c ON c.id = e.claim_pk \
             WHERE e.model_id = ?1 AND c.status = 'active'",
        )?;
        let rows = stmt.query_map(params![model_id], |r| r.get::<_, i64>(0))?;
        let mut out = HashSet::new();
        for pk in rows {
            out.insert(pk? as u64);
        }
        Ok(out)
    }

    pub fn claim_by_pk(&self, pk: i64) -> anyhow::Result<Option<ClaimRow>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {CLAIM_COLS} FROM claims c WHERE c.id = ?1"),
                params![pk],
                claim_row,
            )
            .optional()?)
    }

    /// A claim by its public id, whatever its status (the forget flow must see staged rows).
    pub fn claim_by_claim_id(&self, claim_id: &str) -> anyhow::Result<Option<ClaimRow>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {CLAIM_COLS} FROM claims c WHERE c.claim_id = ?1"),
                params![claim_id],
                claim_row,
            )
            .optional()?)
    }

    /// Set a claim's status without touching `updated_at`, so a cancelled forget restores the row
    /// exactly. Returns whether a row matched.
    pub fn set_claim_status(&self, claim_id: &str, status: &str) -> anyhow::Result<bool> {
        Ok(self.conn()?.execute(
            "UPDATE claims SET status = ?2 WHERE claim_id = ?1",
            params![claim_id, status],
        )? > 0)
    }

    /// D-20: every claim whose `superseded_by` chain reaches `pk` (excluding `pk`), oldest first.
    /// Those predecessors are purged with it.
    pub(crate) fn supersession_chain(&self, pk: i64) -> anyhow::Result<Vec<ClaimRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "WITH RECURSIVE chain(id) AS (\
               SELECT id FROM claims WHERE superseded_by = ?1 \
               UNION SELECT c.id FROM claims c JOIN chain ON c.superseded_by = chain.id) \
             SELECT {CLAIM_COLS} FROM claims c JOIN chain ON c.id = chain.id \
             WHERE c.id != ?1 ORDER BY c.id"
        ))?;
        Ok(stmt
            .query_map(params![pk], claim_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Source message ids cited by the evidence of these claims.
    pub(crate) fn evidence_message_ids(&self, pks: &[i64]) -> anyhow::Result<Vec<i64>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT message_id FROM evidence \
             WHERE claim_pk = ?1 AND message_id IS NOT NULL",
        )?;
        let mut out = std::collections::BTreeSet::new();
        for pk in pks {
            for mid in stmt.query_map(params![pk], |r| r.get::<_, i64>(0))? {
                out.insert(mid?);
            }
        }
        Ok(out.into_iter().collect())
    }

    /// Evidence rows of claims outside `pks` that cite one of `mids` and are not yet retracted:
    /// the quotes a purge of `pks` blanks.
    fn shared_evidence_ids(
        conn: &Connection,
        pks: &[i64],
        mids: &[i64],
    ) -> anyhow::Result<Vec<i64>> {
        let mut stmt = conn.prepare(
            "SELECT id, claim_pk FROM evidence WHERE message_id = ?1 AND quote != '[retracted]'",
        )?;
        let mut out = Vec::new();
        for mid in mids {
            for row in stmt.query_map(params![mid], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })? {
                let (id, owner) = row?;
                if !pks.contains(&owner) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// How many other claims' quotes a purge of `pks` would turn into `[retracted]` (read-only).
    pub(crate) fn shared_quote_count(&self, pks: &[i64], mids: &[i64]) -> anyhow::Result<usize> {
        let conn = self.conn()?;
        Ok(Self::shared_evidence_ids(&conn, pks, mids)?.len())
    }

    /// D-20/D-28: the irreversible delete, in one transaction. `rows` are `(pk, text_hash)` of the
    /// staged claim (first) and its superseded predecessors; `mids` are the source message ids of
    /// their evidence. Writes content-free tombstones (hashes, message ids), blanks other claims'
    /// quotes of those messages to `[retracted]`, then deletes the rows (evidence and embeddings
    /// cascade, the FTS trigger fires). `Ok(None)` and no change when the first row is no longer
    /// staged (a cancel won the race). Otherwise the number of quotes retracted.
    ///
    /// `pub(crate)`: only `forget::purge` may hard-delete (D-28).
    pub(crate) fn purge_rows(
        &self,
        rows: &[(i64, String)],
        mids: &[i64],
    ) -> anyhow::Result<Option<usize>> {
        let Some((main_pk, _)) = rows.first() else {
            return Ok(None);
        };
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM claims WHERE id = ?1",
                params![main_pk],
                |r| r.get(0),
            )
            .optional()?;
        if status.as_deref() != Some("staged_forget") {
            return Ok(None);
        }
        // Predecessors point at the claim through `superseded_by`; check the FK at commit.
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let ts = now();
        for (_, hash) in rows {
            tx.execute(
                "INSERT OR IGNORE INTO tombstones(text_hash, created_at) VALUES(?1, ?2)",
                params![hash, ts],
            )?;
        }
        for mid in mids {
            tx.execute(
                "INSERT OR IGNORE INTO tombstone_messages(message_id) VALUES(?1)",
                params![mid],
            )?;
        }
        let pks: Vec<i64> = rows.iter().map(|(pk, _)| *pk).collect();
        let shared = Self::shared_evidence_ids(&tx, &pks, mids)?;
        for id in &shared {
            tx.execute(
                "UPDATE evidence SET quote = '[retracted]' WHERE id = ?1",
                params![id],
            )?;
        }
        for pk in &pks {
            tx.execute("DELETE FROM claims WHERE id = ?1", params![pk])?;
        }
        tx.commit()?;
        Ok(Some(shared.len()))
    }

    /// Every claim staged for forgetting.
    pub fn staged_claims(&self) -> anyhow::Result<Vec<ClaimRow>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CLAIM_COLS} FROM claims c WHERE c.status = 'staged_forget' ORDER BY c.id DESC"
        ))?;
        Ok(stmt
            .query_map([], claim_row)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn meta_get(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self
            .conn()?
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.conn()?.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Take the consolidation lease for `ttl_secs` unless an unexpired one exists (compare-and-set
    /// under `BEGIN IMMEDIATE`, so it also serialises separate processes).
    pub fn acquire_lease(&self, ttl_secs: i64) -> anyhow::Result<bool> {
        self.acquire_lease_for(LEASE_KEY, ttl_secs)
    }

    /// [`Self::acquire_lease`] on a named `meta` key (dreams use [`DREAMS_LEASE_KEY`]).
    pub fn acquire_lease_for(&self, key: &str, ttl_secs: i64) -> anyhow::Result<bool> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = chrono::Utc::now().timestamp();
        let held: Option<String> = tx
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?;
        if held
            .and_then(|v| v.parse::<i64>().ok())
            .is_some_and(|until| until > now)
        {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, (now + ttl_secs).to_string()],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn release_lease(&self) -> anyhow::Result<()> {
        self.release_lease_for(LEASE_KEY)
    }

    pub fn release_lease_for(&self, key: &str) -> anyhow::Result<()> {
        self.conn()?
            .execute("DELETE FROM meta WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// The subset of `ids` that are in `tombstone_messages` (forgotten conversation turns).
    pub fn tombstoned_message_ids(&self, ids: &[i64]) -> anyhow::Result<HashSet<i64>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT 1 FROM tombstone_messages WHERE message_id = ?1")?;
        let mut out = HashSet::new();
        for id in ids {
            if stmt
                .query_row(params![id], |_| Ok(()))
                .optional()?
                .is_some()
            {
                out.insert(*id);
            }
        }
        Ok(out)
    }

    /// Primary key and public id of the active claim with this text hash, subject and kind.
    pub fn active_by_hash(
        &self,
        text_hash: &str,
        subject: &str,
        kind: &str,
    ) -> anyhow::Result<Option<(i64, String)>> {
        Ok(self
            .conn()?
            .query_row(
                "SELECT id, claim_id FROM claims WHERE text_hash = ?1 AND subject = ?2 \
                 AND kind = ?3 AND status = 'active' ORDER BY id LIMIT 1",
                params![text_hash, subject, kind],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// `(pk, vector)` of every active claim with this subject and kind that is embedded with
    /// `model_id`: the only claims a new claim may supersede (D-18).
    pub fn active_embedded_for(
        &self,
        model_id: &str,
        subject: &str,
        kind: &str,
    ) -> anyhow::Result<Vec<(i64, Vec<f32>)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT e.claim_pk, e.vec FROM embeddings e JOIN claims c ON c.id = e.claim_pk \
             WHERE e.model_id = ?1 AND c.status = 'active' AND c.subject = ?2 AND c.kind = ?3 \
             ORDER BY e.claim_pk",
        )?;
        let rows = stmt.query_map(params![model_id, subject, kind], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (pk, blob) = row?;
            out.extend(decode_vec(&blob).map(|v| (pk, v)));
        }
        Ok(out)
    }

    /// Insert/merge a consolidation batch and advance the watermark in ONE transaction: either
    /// everything lands or nothing does (D-15).
    pub fn commit_consolidation(
        &self,
        claims: &[ConsolidatedClaim],
        watermark: i64,
    ) -> anyhow::Result<CommitOutcome> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = now();
        let mut out = CommitOutcome::default();
        for c in claims {
            let pk = match c.existing_pk {
                Some(pk) => {
                    out.merged += 1;
                    pk
                }
                None => {
                    tx.execute(
                        "INSERT INTO claims(claim_id, text, text_hash, kind, subject, origin, \
                         confidence, durable, source_url, session_id, created_at, updated_at) \
                         VALUES(?1, ?2, ?3, ?4, ?5, 'consolidation', ?6, ?7, ?8, ?9, ?10, ?10)",
                        params![
                            c.claim_id,
                            c.text,
                            text_hash(&c.text),
                            c.kind,
                            c.subject,
                            c.confidence,
                            c.durable,
                            c.source_url,
                            c.session_id,
                            ts,
                        ],
                    )?;
                    out.added += 1;
                    let new_pk = tx.last_insert_rowid();
                    if let Some((model, v)) = &c.embedding {
                        anyhow::ensure!(
                            v.len() == TIER2_EMBEDDING_DIM,
                            "embedding has {} dimensions, expected {TIER2_EMBEDDING_DIM}",
                            v.len()
                        );
                        let blob: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
                        tx.execute(
                            "INSERT INTO embeddings(claim_pk, model_id, vec) VALUES(?1, ?2, ?3)",
                            params![new_pk, model, blob],
                        )?;
                        out.embedded.push((new_pk, v.clone()));
                    }
                    if let Some(old_pk) = c.supersedes_pk {
                        // Only a still-active claim can be superseded (an earlier claim of this
                        // batch may already have taken it).
                        out.superseded += tx.execute(
                            "UPDATE claims SET status = 'superseded', superseded_by = ?2, \
                             updated_at = ?3 WHERE id = ?1 AND status = 'active'",
                            params![old_pk, new_pk, ts],
                        )?;
                    }
                    new_pk
                }
            };
            for e in &c.evidence {
                tx.execute(
                    "INSERT OR IGNORE INTO evidence(claim_pk, source_url, quote, session_id, \
                     message_id, ts) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![pk, e.source_url, e.quote, e.session_id, e.message_id, e.ts],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![WATERMARK_KEY, watermark.to_string()],
        )?;
        tx.commit()?;
        Ok(out)
    }

    /// Generation of the on-disk vector index; 0 before the first write.
    pub fn index_generation(&self) -> anyhow::Result<u64> {
        let conn = self.conn()?;
        generation_on(&conn)
    }

    /// Serialise an index-file write across processes: take the SQLite write lock
    /// (`BEGIN IMMEDIATE`), run `write(current_generation, conn)` (which rewrites the file), then
    /// bump `meta.index_generation`. If `write` fails the transaction rolls back and the
    /// generation is unchanged. Returns the new generation.
    pub(crate) fn commit_index_write(
        &self,
        write: impl FnOnce(u64, &Connection) -> anyhow::Result<()>,
    ) -> anyhow::Result<u64> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cur = generation_on(&tx)?;
        write(cur, &tx)?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![INDEX_GENERATION_KEY, (cur + 1).to_string()],
        )?;
        tx.commit()?;
        Ok(cur + 1)
    }

    /// FTS5 bm25 search over active claims. An empty sanitized query yields no rows.
    pub fn keyword_search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<ClaimRow>> {
        // sanitize_fts_query drops FTS5 operators but not control characters (a NUL errors FTS5)
        // or punctuation (`don't`, `c++`, `a/b`), which is an FTS5 syntax error as a bare word.
        // Blank the controls, then quote each token (sanitize removed every `"`) so the MATCH
        // string is always a conjunction of phrases.
        let cleaned: String = query
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let q = ironhermes_state::sanitize_fts_query(&cleaned)
            .split_whitespace()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" ");
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CLAIM_COLS} FROM claims_fts f JOIN claims c ON c.id = f.rowid \
             WHERE claims_fts MATCH ?1 AND c.status = 'active' \
             ORDER BY bm25(claims_fts) LIMIT ?2"
        ))?;
        Ok(stmt
            .query_map(params![q, limit as i64], claim_row)?
            .collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(text: &str) -> NewClaim {
        NewClaim {
            text: text.into(),
            kind: "world".into(),
            subject: "user".into(),
            origin: "tier1".into(),
            confidence: 0.95,
            source_url: Some("memory://tier1/MEMORY.md#L1".into()),
            tier1_target: Some("memory".into()),
            tier1_ordinal: Some(1),
            session_id: None,
        }
    }

    #[test]
    fn hash_ignores_case_and_whitespace() {
        assert_eq!(text_hash("  Hello   World "), text_hash("hello world"));
        assert_ne!(text_hash("hello"), text_hash("world"));
    }

    #[test]
    fn insert_search_delete_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open(&tmp.path().join("t.db")).unwrap();
        let row = s
            .insert_claim(&claim("User prefers concise Telegram replies"))
            .unwrap();
        assert!(ironhermes_core::tier2::is_valid_claim_id(&row.claim_id));
        assert_eq!(s.keyword_search("telegram", 10).unwrap().len(), 1);
        s.delete_claims(&[row.id]).unwrap();
        assert!(s.keyword_search("telegram", 10).unwrap().is_empty());
    }

    #[test]
    fn hostile_or_empty_queries_never_error() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Store::open(&tmp.path().join("t.db")).unwrap();
        s.insert_claim(&claim("alpha beta")).unwrap();
        for q in [
            "",
            "   ",
            "\"",
            "AND",
            "(((",
            "a OR b NOT",
            "x:y*",
            "-",
            "don't",
            "c++",
            "a/b",
            "alpha?",
            "'",
            ".,;!",
            "alpha\u{0}beta",
        ] {
            assert!(s.keyword_search(q, 5).is_ok(), "query {q:?} errored");
        }
        assert_eq!(s.keyword_search("alpha?", 5).unwrap().len(), 1);
    }

    #[test]
    fn reopen_is_idempotent_and_secure_delete_is_on() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.db");
        drop(Store::open(&p).unwrap());
        let s = Store::open(&p).unwrap();
        let on: i64 = s
            .conn()
            .unwrap()
            .query_row("PRAGMA secure_delete", [], |r| r.get(0))
            .unwrap();
        assert_eq!(on, 1);
    }
}
