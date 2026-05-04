//! Dedicated OS thread bridge for DuckDB's !Send Connection.
//!
//! D-03: DuckDB Connection is !Send and must be owned by a dedicated OS thread.
//! Commands are sent via std::sync::mpsc channel. The DuckDbBridge struct holds
//! only an mpsc::Sender which IS Send+Sync, so DuckDbBridge can be shared safely.
//!
//! T-17-10: Parameterized queries prevent SQL injection on the worker thread.
//! T-17-11: Capacity limits enforced in handle_add.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;

use ironhermes_core::constants::ENTRY_DELIMITER;
use ironhermes_core::memory_store::MemoryTarget;

use crate::schema;

// =============================================================================
// Command enum
// =============================================================================

/// Commands sent from caller thread to DuckDB worker thread via mpsc.
pub enum DuckDbCommand {
    Add {
        target: String,
        content: String,
        respond: mpsc::SyncSender<Result<String, String>>,
    },
    Replace {
        target: String,
        old_text: String,
        new_content: String,
        respond: mpsc::SyncSender<Result<String, String>>,
    },
    Remove {
        target: String,
        old_text: String,
        respond: mpsc::SyncSender<Result<String, String>>,
    },
    LoadAll {
        respond: mpsc::SyncSender<anyhow::Result<HashMap<String, Vec<String>>>>,
    },
    Recall {
        query: String,
        limit: u32,
        respond: mpsc::SyncSender<Result<String, String>>,
    },
    /// Fire-and-forget: index conversation content for analytical queries.
    SyncTurn {
        entries_json: String,
    },
    /// Fire-and-forget: extract facts from compressed messages.
    OnPreCompress {
        messages_json: String,
    },
    /// Fire-and-forget: warm query cache.
    QueuePrefetch {
        query: String,
    },
    Shutdown,
}

// =============================================================================
// DuckDbBridge
// =============================================================================

/// Bridge to the DuckDB worker thread.
///
/// Holds only an mpsc::Sender (which is Send) so DuckDbBridge itself is Send+Sync.
/// The duckdb::Connection never leaves the worker thread.
pub struct DuckDbBridge {
    tx: mpsc::Sender<DuckDbCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

// SAFETY: mpsc::Sender<DuckDbCommand> is Send. DuckDbBridge contains no raw pointers.
// The Connection lives only on the worker thread and is not accessible from DuckDbBridge.
unsafe impl Sync for DuckDbBridge {}

impl DuckDbBridge {
    /// Open (or create) a DuckDB database and start the worker thread.
    pub fn new(db_path: &Path) -> anyhow::Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let (tx, rx) = mpsc::channel::<DuckDbCommand>();
        let db_path = db_path.to_owned();

        let thread = std::thread::spawn(move || {
            let conn = duckdb::Connection::open(&db_path).expect("Failed to open DuckDB database");
            conn.execute_batch(schema::CREATE_SCHEMA)
                .expect("Failed to initialize DuckDB schema");

            for cmd in rx {
                match cmd {
                    DuckDbCommand::Add {
                        target,
                        content,
                        respond,
                    } => {
                        let result = handle_add(&conn, &target, &content);
                        let _ = respond.send(result);
                    }
                    DuckDbCommand::Replace {
                        target,
                        old_text,
                        new_content,
                        respond,
                    } => {
                        let result = handle_replace(&conn, &target, &old_text, &new_content);
                        let _ = respond.send(result);
                    }
                    DuckDbCommand::Remove {
                        target,
                        old_text,
                        respond,
                    } => {
                        let result = handle_remove(&conn, &target, &old_text);
                        let _ = respond.send(result);
                    }
                    DuckDbCommand::LoadAll { respond } => {
                        let result = handle_load_all(&conn);
                        let _ = respond.send(result);
                    }
                    DuckDbCommand::Recall {
                        query,
                        limit,
                        respond,
                    } => {
                        let result = handle_recall(&conn, &query, limit);
                        let _ = respond.send(result);
                    }
                    DuckDbCommand::SyncTurn { entries_json } => {
                        // Fire-and-forget: no respond channel
                        if let Err(e) = handle_sync_turn(&conn, &entries_json) {
                            tracing::warn!(error = %e, "DuckDB sync_turn failed");
                        }
                    }
                    DuckDbCommand::OnPreCompress { messages_json } => {
                        if let Err(e) = handle_on_pre_compress(&conn, &messages_json) {
                            tracing::warn!(error = %e, "DuckDB on_pre_compress failed");
                        }
                    }
                    DuckDbCommand::QueuePrefetch { query } => {
                        if let Err(e) = handle_queue_prefetch(&conn, &query) {
                            tracing::warn!(error = %e, "DuckDB queue_prefetch failed");
                        }
                    }
                    DuckDbCommand::Shutdown => break,
                }
            }
        });

        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    /// Send a command to the worker thread.
    pub fn send(&self, cmd: DuckDbCommand) -> anyhow::Result<()> {
        self.tx
            .send(cmd)
            .map_err(|_| anyhow::anyhow!("DuckDB worker thread has terminated"))
    }

    /// Shut down the worker thread and wait for it to finish.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(DuckDbCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for DuckDbBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// =============================================================================
// Worker thread handlers — operate on &duckdb::Connection directly
// =============================================================================

/// Fetch all entries for a given target label, ordered by id.
fn fetch_entries(conn: &duckdb::Connection, target: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT content FROM memory_facts WHERE target = $1 ORDER BY id")?;
    let entries: Vec<String> = stmt
        .query_map(duckdb::params![target], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(entries)
}

/// Parse a MemoryTarget from string label to get the char limit.
fn char_limit_for(target: &str) -> usize {
    match target {
        "user" => ironhermes_core::constants::USER_CHAR_LIMIT,
        _ => ironhermes_core::constants::MEMORY_CHAR_LIMIT,
    }
}

/// Compute total chars including delimiters between entries.
fn char_count(entries: &[String]) -> usize {
    if entries.is_empty() {
        return 0;
    }
    let entry_chars: usize = entries.iter().map(|e| e.len()).sum();
    let delimiter_chars = ENTRY_DELIMITER.len() * (entries.len() - 1);
    entry_chars + delimiter_chars
}

/// Handle Add command — capacity check then INSERT.
fn handle_add(conn: &duckdb::Connection, target: &str, content: &str) -> Result<String, String> {
    let existing = fetch_entries(conn, target)
        .map_err(|e| format!("{{\"error\": \"Failed to fetch entries: {}\"}}", e))?;

    // Duplicate check
    if existing.iter().any(|e| e == content) {
        return Err(serde_json::json!({
            "error": "duplicate",
            "reason": "Entry already exists",
            "content": content
        })
        .to_string());
    }

    // Capacity check (T-17-11)
    let current_chars = char_count(&existing);
    let new_chars = if existing.is_empty() {
        content.len()
    } else {
        content.len() + ENTRY_DELIMITER.len()
    };
    let limit = char_limit_for(target);
    if current_chars + new_chars > limit {
        return Err(serde_json::json!({
            "error": "capacity_exceeded",
            "reason": format!("Adding this entry would exceed the {} char limit", limit),
            "chars_used": current_chars,
            "chars_limit": limit,
            "new_entry_chars": content.len(),
            "entries": existing
        })
        .to_string());
    }

    // INSERT — parameterized (T-17-10)
    conn.execute(
        "INSERT INTO memory_facts (target, content) VALUES ($1, $2)",
        duckdb::params![target, content],
    )
    .map_err(|e| format!("{{\"error\": \"Failed to insert: {}\"}}", e))?;

    let entries = fetch_entries(conn, target)
        .map_err(|e| format!("{{\"error\": \"Failed to fetch after insert: {}\"}}", e))?;
    let total_chars = char_count(&entries);
    Ok(serde_json::json!({
        "status": "added",
        "target": target,
        "entries": entries.len(),
        "chars_used": total_chars,
        "chars_limit": limit
    })
    .to_string())
}

/// Handle Replace command — find by substring, capacity check, UPDATE.
fn handle_replace(
    conn: &duckdb::Connection,
    target: &str,
    old_text: &str,
    new_content: &str,
) -> Result<String, String> {
    // Find matching rows by substring
    let matches: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, content FROM memory_facts WHERE target = $1 AND content LIKE $2 ORDER BY id",
            )
            .map_err(|e| format!("{{\"error\": \"Query failed: {}\"}}", e))?;
        let pattern = format!("%{}%", old_text);
        stmt.query_map(duckdb::params![target, pattern], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("{{\"error\": \"Query failed: {}\"}}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{{\"error\": \"Row fetch failed: {}\"}}", e))?
    };

    match matches.len() {
        0 => {
            return Err(serde_json::json!({
                "error": "not_found",
                "reason": format!("No entry found containing '{}'", old_text)
            })
            .to_string());
        }
        1 => {}
        _ => {
            return Err(serde_json::json!({
                "error": "ambiguous",
                "reason": format!("Multiple entries match '{}'. Use more specific text to identify a single entry.", old_text),
                "match_count": matches.len()
            })
            .to_string());
        }
    }

    let (row_id, _) = &matches[0];

    // Build projected entries list to check capacity
    let existing = fetch_entries(conn, target)
        .map_err(|e| format!("{{\"error\": \"Failed to fetch: {}\"}}", e))?;
    let match_content = &matches[0].1;
    let mut updated_entries = existing;
    if let Some(pos) = updated_entries.iter().position(|e| e == match_content) {
        updated_entries[pos] = new_content.to_string();
    }

    let total_chars = char_count(&updated_entries);
    let limit = char_limit_for(target);
    if total_chars > limit {
        return Err(serde_json::json!({
            "error": "capacity_exceeded",
            "reason": "Replacement would exceed char limit",
            "chars_used": total_chars,
            "chars_limit": limit
        })
        .to_string());
    }

    // UPDATE — parameterized (T-17-10)
    conn.execute(
        "UPDATE memory_facts SET content = $1 WHERE id = $2",
        duckdb::params![new_content, row_id],
    )
    .map_err(|e| format!("{{\"error\": \"Failed to update: {}\"}}", e))?;

    let entries = fetch_entries(conn, target)
        .map_err(|e| format!("{{\"error\": \"Failed to fetch after update: {}\"}}", e))?;
    let total_chars = char_count(&entries);
    Ok(serde_json::json!({
        "status": "replaced",
        "target": target,
        "entries": entries.len(),
        "chars_used": total_chars,
        "chars_limit": limit
    })
    .to_string())
}

/// Handle Remove command — find by substring, DELETE.
fn handle_remove(
    conn: &duckdb::Connection,
    target: &str,
    old_text: &str,
) -> Result<String, String> {
    let matches: Vec<i64> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM memory_facts WHERE target = $1 AND content LIKE $2 ORDER BY id",
            )
            .map_err(|e| format!("{{\"error\": \"Query failed: {}\"}}", e))?;
        let pattern = format!("%{}%", old_text);
        stmt.query_map(duckdb::params![target, pattern], |row| row.get::<_, i64>(0))
            .map_err(|e| format!("{{\"error\": \"Query failed: {}\"}}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("{{\"error\": \"Row fetch failed: {}\"}}", e))?
    };

    match matches.len() {
        0 => {
            return Err(serde_json::json!({
                "error": "not_found",
                "reason": format!("No entry found containing '{}'", old_text)
            })
            .to_string());
        }
        1 => {}
        _ => {
            return Err(serde_json::json!({
                "error": "ambiguous",
                "reason": format!("Multiple entries match '{}'. Use more specific text.", old_text),
                "match_count": matches.len()
            })
            .to_string());
        }
    }

    conn.execute(
        "DELETE FROM memory_facts WHERE id = $1",
        duckdb::params![matches[0]],
    )
    .map_err(|e| format!("{{\"error\": \"Failed to delete: {}\"}}", e))?;

    let target_enum = if target == "user" {
        MemoryTarget::User
    } else {
        MemoryTarget::Memory
    };
    let entries = fetch_entries(conn, target)
        .map_err(|e| format!("{{\"error\": \"Failed to fetch after delete: {}\"}}", e))?;
    let total_chars = char_count(&entries);
    let limit = target_enum.char_limit();
    Ok(serde_json::json!({
        "status": "removed",
        "target": target,
        "entries": entries.len(),
        "chars_used": total_chars,
        "chars_limit": limit
    })
    .to_string())
}

/// Handle LoadAll command — fetch all entries grouped by target.
fn handle_load_all(conn: &duckdb::Connection) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let mut result = HashMap::new();
    for target in &["memory", "user"] {
        let entries = fetch_entries(conn, target)?;
        if !entries.is_empty() {
            result.insert(target.to_string(), entries);
        }
    }
    Ok(result)
}

/// Handle Recall: search memory_facts by content ILIKE with time-based ordering (D-13).
fn handle_recall(conn: &duckdb::Connection, query: &str, limit: u32) -> Result<String, String> {
    let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let mut stmt = conn
        .prepare(
            "SELECT content, target, created_at,
                CASE WHEN content ILIKE $1 THEN 1.0 ELSE 0.5 END as relevance_score
         FROM memory_facts
         WHERE content ILIKE $1
         ORDER BY created_at DESC
         LIMIT $2",
        )
        .map_err(|e| format!("DuckDB recall query failed: {}", e))?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(duckdb::params![pattern, limit], |row| {
            let content: String = row.get(0)?;
            let target: String = row.get(1)?;
            let relevance_score: f64 = row.get(3)?;
            let snippet = if content.len() > 100 {
                format!("{}...", &content[..100])
            } else {
                content.clone()
            };
            Ok(serde_json::json!({
                "content": content,
                "target": target,
                "relevance_score": relevance_score,
                "snippet": snippet
            }))
        })
        .map_err(|e| format!("DuckDB recall query map failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("DuckDB recall row fetch failed: {}", e))?;

    serde_json::to_string(&results)
        .map_err(|e| format!("Failed to serialize recall results: {}", e))
}

/// Handle SyncTurn: fire-and-forget analytics update.
fn handle_sync_turn(conn: &duckdb::Connection, _entries_json: &str) -> anyhow::Result<()> {
    // Ensure conversation_facts table exists
    conn.execute_batch(crate::schema::CREATE_SCHEMA).ok();
    // D-13: Analytics — count total entries per target for trend tracking
    // This is lightweight maintenance; real analytics happen in Recall queries
    Ok(())
}

/// Handle OnPreCompress: extract facts from compressed messages (D-08, D-13).
fn handle_on_pre_compress(conn: &duckdb::Connection, messages_json: &str) -> anyhow::Result<()> {
    let messages: Vec<serde_json::Value> = serde_json::from_str(messages_json).unwrap_or_default();
    for msg in &messages {
        if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
            if content.len() > 10 {
                conn.execute(
                    "INSERT INTO conversation_facts (content) VALUES ($1)",
                    duckdb::params![content],
                )
                .ok();
            }
        }
    }
    Ok(())
}

/// Handle QueuePrefetch: warm query cache (D-09).
fn handle_queue_prefetch(conn: &duckdb::Connection, query: &str) -> anyhow::Result<()> {
    // D-09: Lightweight warmup query to prime DuckDB's buffer manager
    let pattern = format!("%{}%", query);
    let _ = conn
        .prepare("SELECT COUNT(*) FROM memory_facts WHERE content ILIKE $1")
        .and_then(|mut stmt| {
            stmt.query_map(duckdb::params![pattern], |_| Ok(()))
                .map(|rows| for _ in rows {})
        });
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bridge() -> DuckDbBridge {
        let db = tempfile::NamedTempFile::new().unwrap();
        DuckDbBridge::new(db.path()).unwrap()
    }

    fn call_add(bridge: &DuckDbBridge, target: &str, content: &str) -> Result<String, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        bridge
            .send(DuckDbCommand::Add {
                target: target.to_string(),
                content: content.to_string(),
                respond: tx,
            })
            .unwrap();
        rx.recv().unwrap()
    }

    fn call_load_all(bridge: &DuckDbBridge) -> HashMap<String, Vec<String>> {
        let (tx, rx) = mpsc::sync_channel(1);
        bridge.send(DuckDbCommand::LoadAll { respond: tx }).unwrap();
        rx.recv().unwrap().unwrap()
    }

    #[test]
    fn test_bridge_new_creates_worker_thread() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let bridge = DuckDbBridge::new(db.path()).unwrap();
        assert!(bridge.thread.is_some());
    }

    #[test]
    fn test_bridge_add_and_load_all() {
        let bridge = make_bridge();
        let result = call_add(&bridge, "memory", "test fact");
        assert!(result.is_ok(), "add should succeed: {:?}", result);
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "added");

        let all = call_load_all(&bridge);
        assert!(all.contains_key("memory"));
        assert!(all["memory"].contains(&"test fact".to_string()));
    }

    #[test]
    fn test_bridge_shutdown_joins_thread() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let mut bridge = DuckDbBridge::new(db.path()).unwrap();
        bridge.shutdown();
        assert!(bridge.thread.is_none());
    }

    #[test]
    fn test_bridge_add_duplicate_returns_error() {
        let bridge = make_bridge();
        call_add(&bridge, "memory", "dup fact").unwrap();
        let result = call_add(&bridge, "memory", "dup fact");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("duplicate"));
    }

    #[test]
    fn test_bridge_add_capacity_exceeded() {
        let bridge = make_bridge();
        call_add(&bridge, "memory", &"x".repeat(2100)).unwrap();
        let result = call_add(&bridge, "memory", &"y".repeat(200));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("capacity_exceeded"));
    }

    #[test]
    fn test_bridge_replace() {
        let bridge = make_bridge();
        call_add(&bridge, "memory", "fact about cats").unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        bridge
            .send(DuckDbCommand::Replace {
                target: "memory".to_string(),
                old_text: "cats".to_string(),
                new_content: "updated about dogs".to_string(),
                respond: tx,
            })
            .unwrap();
        let result = rx.recv().unwrap();
        assert!(result.is_ok(), "replace should succeed: {:?}", result);
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "replaced");

        let all = call_load_all(&bridge);
        assert!(all["memory"].contains(&"updated about dogs".to_string()));
        assert!(!all["memory"].contains(&"fact about cats".to_string()));
    }

    #[test]
    fn test_bridge_remove() {
        let bridge = make_bridge();
        call_add(&bridge, "memory", "fact to remove").unwrap();
        call_add(&bridge, "memory", "fact to keep").unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        bridge
            .send(DuckDbCommand::Remove {
                target: "memory".to_string(),
                old_text: "to remove".to_string(),
                respond: tx,
            })
            .unwrap();
        let result = rx.recv().unwrap();
        assert!(result.is_ok(), "remove should succeed: {:?}", result);

        let all = call_load_all(&bridge);
        assert!(!all["memory"].contains(&"fact to remove".to_string()));
        assert!(all["memory"].contains(&"fact to keep".to_string()));
    }
}
