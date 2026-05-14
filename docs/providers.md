<!-- generated-by: gsd-doc-writer -->
# Memory Providers

IronHermes ships three pluggable memory provider crates in `providers/` that back the agent's persistent memory subsystem. Each provider implements the `MemoryProvider` trait from `ironhermes-core` and stores facts across two scopes: `memory` (general agent knowledge, 2,200 character limit) and `user` (user profile, 1,375 character limit).

All three providers share the same security posture: every `add` and `replace` operation runs `scan_context_content` to block prompt-injection attempts before any data reaches storage. Substring-matched `replace` and `remove` operations are unambiguous — they error if zero or more than one entry matches.

---

## Provider Overview

| Provider | Crate | Name token | Storage model | Best for |
|---|---|---|---|---|
| SQLite | `memory-sqlite` | `sqlite` | Relational table + FTS5 | Default production use; fast full-text search |
| Grafeo | `memory-grafeo` | `grafeo` | Labeled property graph | Relationship-aware knowledge; entity extraction |
| DuckDB | `memory-duckdb` | `duckdb` | Columnar flat table | Analytical queries; recency-ordered recall |

---

## Selecting a Provider

Set `memory.provider` in `~/.ironhermes/config.yaml`:

```yaml
memory:
  provider: "sqlite"   # "file" | "sqlite" | "grafeo" | "duckdb"
```

The default provider when no value is set is `"file"` (flat Markdown files). Changing the value to one of the three crate-backed providers requires that the corresponding Cargo feature be compiled in (see Build Requirements below for each provider).

An optional write-only mirror can be set alongside the primary provider:

```yaml
memory:
  provider: "sqlite"
  mirror_provider: "grafeo"   # receives writes but is not read from
```

---

## Provider Registration in the Agent Runtime

Provider construction is handled by `crates/ironhermes-agent/src/memory/factory.rs`. The factory function `build_memory_manager` is called at agent startup and performs these steps in order:

1. Reads `memory.provider` from the loaded `MemoryConfig`.
2. Matches the name string against compile-time feature gates (`#[cfg(feature = "memory-sqlite")]`, etc.). If the matching feature is not compiled in, the factory returns an error naming the required feature and the rebuild command.
3. Constructs the provider via `Provider::new(db_path)`.
4. Calls `provider.initialize("factory-boot", hermes_home, provider_config).await`.
5. Calls `provider.load_from_disk()` to populate the frozen snapshot used for system-prompt injection.
6. Calls `provider.is_available()`. If it returns `false`, logs a warning with `unavailable_reason()` and falls back to the `file` provider automatically.
7. Wraps the provider in `Arc<tokio::sync::Mutex<MemoryManager>>` for sharing across the agent loop, tool registry, and context engine.

Provider-specific JSON configuration is loaded from `$IRONHERMES_HOME/<provider_name>.json` at startup. This file is optional; if absent, all fields use their defaults.

---

## memory-sqlite

**Crate:** `providers/memory-sqlite`  
**Name token:** `sqlite`  
**Dependency:** `rusqlite` with the `bundled` feature (SQLite compiled into the binary — no system SQLite required)

### What it does

Stores memory entries in a `memory_facts` relational table with WAL journal mode and a 5-second busy timeout. A FTS5 virtual table (`memory_facts_fts`) is maintained by triggers on every insert, update, and delete, enabling ranked full-text search via the `memory_recall` tool. On context compression, text from discarded messages is indexed into a separate `conversation_extracts_fts` table so facts from compressed-away conversation remain searchable.

The provider uses a frozen-snapshot pattern: `load_from_disk()` captures current entries into an in-memory `HashMap`; `format_for_system_prompt` and `to_memory_entries` read from this snapshot. Mutations write to SQLite immediately but do not update the snapshot until the next `load_from_disk()` call.

### When to use it

Use `sqlite` as the default production provider. It is the most mature backend with the richest search capability (BM25-ranked FTS5 with snippet extraction), handles concurrent read/write safely via WAL mode, and requires no external services.

### Configuration keys

Configured via `$IRONHERMES_HOME/sqlite.json`:

| Key | Required | Default | Description |
|---|---|---|---|
| `db_path` | No | `$HERMES_HOME/memory.db` | Path to the SQLite database file. Created on first run if absent. |

### Environment variables

None. The provider has no environment variable dependencies.

### Build requirements

Enable the `memory-sqlite` Cargo feature:

```bash
cargo build --features memory-sqlite
```

The `rusqlite` dependency uses the `bundled` feature, so no system SQLite installation is needed.

### Known limitations and performance characteristics

- The FTS5 index is maintained by triggers; writes incur trigger overhead on every mutation.
- `sync_turn` fires a fire-and-forget FTS5 `rebuild` on each turn to ensure index consistency after any out-of-band changes.
- `queue_prefetch` warms the FTS5 tokenizer cache with likely query terms between turns.
- The `Arc<Mutex<Connection>>` pattern means all SQLite operations serialize at the mutex; this is acceptable for single-user workloads.
- The frozen-snapshot pattern means `format_for_system_prompt` does not reflect mutations made after the last `load_from_disk()` call within the same session.

---

## memory-grafeo

**Crate:** `providers/memory-grafeo`  
**Name token:** `grafeo`  
**Dependency:** `grafeo 0.5` and `grafeo-common 0.5`

### What it does

Stores memory entries as `MemoryEntry` nodes in a Grafeo labeled property graph (LPG). Each node carries `content`, `target`, and `created_at` properties. Beyond flat entry storage, the provider performs entity-relationship extraction: on every `sync_turn` and `on_pre_compress` call it scans entry text and conversation content for subject-relation-object patterns (e.g. "X is Y", "X has Y", "X uses Y") and stores them as `Entity` nodes connected by `RELATES_TO` edges with a `relation_type` edge property.

Property indexes are created on `content` and `target` at initialization for fast duplicate detection. Substring matching for `replace` and `remove` falls back to a full `iter_nodes` scan.

The provider uses the same frozen-snapshot pattern as `memory-sqlite`: `load_from_disk()` captures a snapshot; `format_for_system_prompt` reads from it.

`recall` uses substring matching against `content` with a term-match relevance score (matching terms / total query terms), not FTS5. The `system_prompt_block` includes a count of entity nodes in the graph.

### When to use it

Use `grafeo` when relationship-aware memory is valuable — for example, when the agent needs to reason about how entities relate to each other over long sessions. Entity extraction is heuristic and lightweight; it works best on declarative statements ("Brad likes Rust", "IronHermes is a Rust project").

### Configuration keys

Configured via `$IRONHERMES_HOME/grafeo.json`:

| Key | Required | Default | Description |
|---|---|---|---|
| `graph_dir` | No | `$HERMES_HOME/grafeo` | Path to the Grafeo graph database file or directory. Created on first run if absent. |

The factory opens the database at `$IRONHERMES_HOME/memory_graph.grafeo`. The `.grafeo` extension is required for persistence to survive process restarts.

### Environment variables

None. The provider has no environment variable dependencies.

### Build requirements

Enable the `memory-grafeo` Cargo feature:

```bash
cargo build --features memory-grafeo
```

### Known limitations and performance characteristics

- `GrafeoDB` uses interior mutability; all mutation methods take `&self`, so `sync_turn` and `on_pre_compress` do not require `&mut self` or a `Mutex`. No fire-and-forget spawn is needed — operations complete inline.
- Entity extraction is heuristic (simple pattern matching on whitespace-split sentences). It will miss complex sentence structures and produce false positives on some inputs.
- `recall` is a linear scan over all nodes — it does not use an index. Performance degrades with large numbers of nodes.
- Because `GrafeoDB` is not `Send`, it cannot be placed in a `tokio::spawn` closure. All graph operations run synchronously on the caller thread.

---

## memory-duckdb

**Crate:** `providers/memory-duckdb`  
**Name token:** `duckdb`  
**Dependency:** `duckdb 1` with the `bundled` feature (DuckDB compiled into the binary)

### What it does

Stores memory entries in a flat columnar `memory_facts` table using DuckDB. A secondary `conversation_facts` table receives content extracted from compressed-away messages via `on_pre_compress`. The `memory_recall` tool uses `ILIKE` case-insensitive pattern matching ordered by `created_at DESC`, giving recency-weighted results.

Because `duckdb::Connection` is `!Send`, the provider uses a dedicated OS thread bridge (`DuckDbBridge`). All database operations are dispatched as typed commands (`DuckDbCommand`) over an `mpsc::channel` to a worker thread that owns the `Connection`. The bridge struct holds only an `mpsc::Sender` (which is `Send`), making `DuckDbBridge` safe to hold in async contexts. Commands that need a response use `mpsc::SyncSender` for synchronous round-trip; fire-and-forget commands (`SyncTurn`, `OnPreCompress`, `QueuePrefetch`) have no response channel.

Security scanning runs on the caller thread before commands are sent to the bridge worker.

### When to use it

Use `duckdb` when analytical query patterns matter — for example, time-based recall ("what did I say recently about X?"). The columnar storage is not a significant advantage at the entry counts typical of the memory subsystem, but DuckDB's `ILIKE` operator gives case-insensitive recall without a separate FTS index.

### Configuration keys

Configured via `$IRONHERMES_HOME/duckdb.json`:

| Key | Required | Default | Description |
|---|---|---|---|
| `db_path` | No | `$HERMES_HOME/memory.duckdb` | Path to the DuckDB database file. Created on first run if absent. |
| `threads` | No | `1` | Number of worker threads DuckDB may use. Default of 1 is appropriate for single-user workloads. |

The factory opens the database at `$IRONHERMES_HOME/memory_duckdb.db` (note: the factory path does not match the config schema default exactly — the factory hardcodes `memory_duckdb.db` while the schema default shows `memory.duckdb`).

### Environment variables

None. The provider has no environment variable dependencies.

### Build requirements

Enable the `memory-duckdb` Cargo feature:

```bash
cargo build --features memory-duckdb
```

The `duckdb` dependency uses the `bundled` feature, so no system DuckDB installation is needed.

### Known limitations and performance characteristics

- All DuckDB operations block the calling thread while waiting for the worker to respond via `mpsc::sync_channel`. This is intentional — the bridge is a thin synchronous adapter over an inherently sync API.
- The bridge thread joins on `Drop` via the `DuckDbBridge::drop` implementation, so shutdown is always clean.
- `recall` uses `ILIKE` with `%query%` substring matching — not ranked FTS. Results are ordered by insertion timestamp, not relevance score.
- The `threads` config key is declared in the schema but is not currently wired to a DuckDB pragma in the bridge constructor. It is reserved for future use.
- `sync_turn` is fire-and-forget and performs lightweight maintenance only (ensuring the `conversation_facts` schema exists). No analytical aggregation runs automatically between turns.

---

## Capacity Limits

All three providers enforce the same character limits defined in `ironhermes-core`:

| Scope | Limit | Description |
|---|---|---|
| `memory` | 2,200 characters | General agent knowledge entries |
| `user` | 1,375 characters | User profile entries |

Limits are checked before every `add` and `replace` operation. If adding an entry would exceed the limit, the operation returns a `capacity_exceeded` error with the current usage, the limit, and the new entry size.

---

## Common Behavior Across All Providers

- **Prompt injection blocking:** All providers call `scan_context_content` on every write. Content containing injection patterns is rejected with a `blocked` error.
- **Duplicate detection:** Exact-match duplicates are rejected before insertion.
- **Substring-matched mutations:** `replace` and `remove` match by substring. If zero entries match, the operation returns `not_found`. If more than one entry matches, the operation returns `ambiguous` with a match count.
- **Frozen snapshot:** `format_for_system_prompt` and `to_memory_entries` read from a snapshot captured at `load_from_disk()` time, not from live storage. This ensures system-prompt content is stable within a session.
- **Fallback behavior:** If a provider's `is_available()` returns `false` at startup, the factory logs a warning and falls back to the `file` provider automatically.
