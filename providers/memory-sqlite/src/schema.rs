//! SQLite schema for the memory-sqlite provider.
//!
//! D-01: memory_facts table with FTS5 virtual table and change-tracking triggers.
//! Pattern from Phase 13 (ironhermes-state) messages_fts schema.

/// SQL statements to create the memory_facts table, FTS5 virtual table, and triggers.
/// Uses execute_batch so all statements run in a single call.
pub const CREATE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS memory_facts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    target      TEXT NOT NULL CHECK(target IN ('memory', 'user')),
    content     TEXT NOT NULL,
    created_at  REAL NOT NULL DEFAULT (julianday('now'))
);

CREATE VIRTUAL TABLE IF NOT EXISTS memory_facts_fts USING fts5(
    content,
    content=memory_facts,
    content_rowid=id
);

CREATE TRIGGER IF NOT EXISTS memory_fts_insert AFTER INSERT ON memory_facts BEGIN
    INSERT INTO memory_facts_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS memory_fts_delete AFTER DELETE ON memory_facts BEGIN
    INSERT INTO memory_facts_fts(memory_facts_fts, rowid, content) VALUES('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS memory_fts_update AFTER UPDATE ON memory_facts BEGIN
    INSERT INTO memory_facts_fts(memory_facts_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO memory_facts_fts(rowid, content) VALUES (new.id, new.content);
END;
";
