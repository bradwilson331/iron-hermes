//! DuckDB schema for the memory_facts table (D-04: flat columnar table).

/// DDL for the memory_facts table.
/// DuckDB uses SEQUENCE for auto-increment (not AUTOINCREMENT like SQLite).
/// The target column is constrained to 'memory' or 'user'.
pub const CREATE_SCHEMA: &str = "
CREATE SEQUENCE IF NOT EXISTS memory_facts_seq START 1;
CREATE TABLE IF NOT EXISTS memory_facts (
    id          BIGINT PRIMARY KEY DEFAULT nextval('memory_facts_seq'),
    target      VARCHAR NOT NULL CHECK(target IN ('memory', 'user')),
    content     VARCHAR NOT NULL,
    created_at  TIMESTAMP NOT NULL DEFAULT current_timestamp
);
CREATE TABLE IF NOT EXISTS conversation_facts (
    id          BIGINT DEFAULT nextval('memory_facts_seq'),
    content     VARCHAR NOT NULL,
    category    VARCHAR DEFAULT 'general',
    created_at  TIMESTAMP NOT NULL DEFAULT current_timestamp
);
";
