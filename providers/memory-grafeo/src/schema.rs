//! Graph schema constants for the Grafeo memory provider.
//!
//! D-02: Memory entries stored as graph nodes with properties.
//! Node label: "MemoryEntry"
//! Properties:
//!   - "content"  : String — the raw text of the memory entry
//!   - "target"   : String — "memory" or "user"
//!   - "created_at": Int64 — Unix timestamp at insertion time

/// Label applied to every memory-entry node.
pub const NODE_LABEL: &str = "MemoryEntry";

/// Property key for the entry's text content.
pub const PROP_CONTENT: &str = "content";

/// Property key for the MemoryTarget label ("memory" or "user").
pub const PROP_TARGET: &str = "target";

/// Property key for the insertion timestamp (Unix seconds, i64).
pub const PROP_CREATED_AT: &str = "created_at";

/// Property index key used by find_nodes_by_property for exact content lookups.
///
/// We create a property index on PROP_CONTENT at initialization so that exact
/// duplicate detection is fast. Substring matching (replace / remove) falls back
/// to a linear scan over iter_nodes().
pub const INDEXED_PROPS: &[&str] = &[PROP_CONTENT, PROP_TARGET];
