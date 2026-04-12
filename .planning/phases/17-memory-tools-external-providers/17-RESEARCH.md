# Phase 17: Memory Tools & External Providers - Research

**Researched:** 2026-04-12
**Domain:** Rust memory tool operations, external database providers (SQLite/FTS5, Grafeo, DuckDB), Cargo workspace features, session search tool
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** SQLite memory provider mirrors file format — one row per entry with target, content, created_at columns. FTS5 virtual table for full-text search across memory entries.
- **D-02:** Grafeo uses embedded library (not external HTTP). Memory entries stored as graph nodes. Metadata keys become edge labels. Enables multi-hop relationship queries.
- **D-03:** DuckDB uses a dedicated OS thread owning the Connection (which is `!Send`). Commands sent via mpsc channel, results returned back. Same pattern as spawn_blocking but persistent thread.
- **D-04:** DuckDB uses flat columnar table for memory facts. Optimized for analytical queries.
- **D-05:** Tool schema: `session_search` with parameters `query` (required), `role_filter` (optional array), `source_filter` (optional array), `limit` (optional integer, default 5).
- **D-06:** Results surface: FTS5 snippets with `>>>match<<<` markers, 1 message before/after (200 chars each), plus metadata (session_id, role, timestamp, source, model, session_started).
- **D-07:** session_search intercepted in agent loop before registry dispatch. Both tools bypass tool registry.
- **D-08:** FTS5 query sanitization: strip unmatched quotes, wrap hyphenated terms in quotes, remove dangling boolean operators, strip special characters except `*`, `"`, `-`.
- **D-09:** Per-provider crates under `providers/` at workspace root: `providers/memory-sqlite/`, `providers/memory-duckdb/`, `providers/memory-grafeo/`.
- **D-10:** MemoryProvider trait stays in `ironhermes-core/src/memory_provider.rs`. No circular dependencies.
- **D-11:** Factory relocates from `ironhermes-core` to `ironhermes-agent/src/memory/factory.rs`.
- **D-12:** Entry-point crates use Cargo features to select providers. Default: file-based only.
- **D-13:** Capacity header format: `## Memory (67% -- 1,474/2,200 chars)` and `## User Profile (42% -- 578/1,375 chars)`.
- **D-14:** Memory tool success response includes capacity feedback: `"Added to memory. Memory: 72% -- 1,584/2,200 chars (3 entries)"`.
- **D-15:** Structured error envelopes: `{"error": "capacity_exceeded", ...}` and `{"error": "content_rejected", ...}`.
- **D-16:** Manual-triggered migration on config change with user prompt. Uses `provider_a.dump()` into `provider_b.add_batch()`.
- **D-17:** Mock trait implementations for testing agent logic independently of backends.
- **D-18:** Docker-based integration tests in provider crates behind `#[cfg(feature = "integration-tests")]` gate.

### Claude's Discretion

- SQLite memory provider schema details (indexes, FTS5 trigger setup, migration versioning)
- Grafeo library selection and specific graph schema (crate availability may constrain design)
- DuckDB table schema and analytical query patterns
- session_search result formatting (exact text layout returned to agent)
- Migration utility implementation details (dump/load batch size, progress reporting)
- Whether `build_memory_provider` returns `Box<dyn MemoryProvider>` or `Arc<Mutex<dyn MemoryProvider>>` based on current usage patterns

### Deferred Ideas (OUT OF SCOPE)

None — discussion stayed within phase scope. (Setup wizard deferred to Phase 23.)

</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| MEM-01 | Agent can add, replace, and remove entries in bounded MEMORY.md store (2200 char limit) via memory tool | Already implemented in MemoryStore/MemoryTool; phase focuses on response format updates (capacity in success messages per D-14) and agent loop interception (D-07) |
| MEM-02 | Agent can add, replace, and remove entries in bounded USER.md store (1375 char limit) via memory tool | Same as MEM-01 — both stores handled by same MemoryTool/MemoryStore |
| MEM-03 | Memory tool supports substring matching for replace/remove operations | Already implemented in memory_store.rs with substring matching |
| MEM-04 | Memory stores display capacity usage in system prompt header | prompt_builder.rs needs update to inject capacity header (D-13); format_for_system_prompt must include percentage/chars |
| MEM-05 | Memory entries security scanned for injection/exfiltration patterns | scan_context_content() already called in memory_store.rs; verify the error response format matches D-15 structured envelope |
| MEM-09 | SQLite memory provider stores facts with FTS5 search | New `providers/memory-sqlite` crate implementing MemoryProvider trait; rusqlite 0.32 already in workspace |
| MEM-10 | Grafeo graph database memory provider (feature-gated) | New `providers/memory-grafeo` crate; grafeo = "0.5.37" available on crates.io |
| MEM-11 | DuckDB memory provider (feature-gated, async bridge for !Send Connection) | New `providers/memory-duckdb` crate; duckdb = "1.10501.0" available; dedicated OS thread + mpsc per D-03 |
| MEM-13 | Agent can search past conversations via session_search tool backed by StateStore FTS5 | New `session_search` tool intercepted in agent_loop.rs; wraps existing StateStore.search_messages() |

</phase_requirements>

---

## Summary

Phase 17 extends the memory subsystem in three dimensions: (1) completing the memory tool UX for MEMORY.md and USER.md with capacity display and structured error responses, (2) adding three external provider crates as optional build features, and (3) wiring the session_search tool into the agent loop.

The foundation is largely in place. `MemoryStore`, `MemoryTool`, `MemoryProvider` trait, and `StateStore.search_messages()` all exist from Phases 11 and 13. The work is primarily additive: new provider crates under `providers/`, factory relocation, agent loop interception for memory+session_search, and capacity header injection in the prompt builder.

The DuckDB provider requires special handling because `duckdb::Connection` is `!Send`. The decision (D-03) to use a dedicated OS thread with mpsc channels is the standard Rust pattern for this problem and is well-established. Grafeo 0.5.37 is confirmed on crates.io as a pure-Rust embedded graph database with no C dependencies. rusqlite 0.32 is already in the workspace. DuckDB 1.10501.0 is the current registry version.

**Primary recommendation:** Build the phase in four lanes — memory tool UX fixes (capacity display), SQLite provider (straightforward rusqlite + FTS5), agent loop interception (memory + session_search), and feature-gated providers (Grafeo + DuckDB). SQLite first as the simplest external provider; Grafeo and DuckDB as parallel work since they're independent crates.

---

## Standard Stack

### Core (already in workspace)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| rusqlite | 0.32 | SQLite provider + FTS5 | Already in workspace.dependencies [VERIFIED: workspace Cargo.toml] |
| async-trait | 0.1 | Async trait bounds on MemoryProvider | Already used throughout codebase [VERIFIED: codebase grep] |
| serde_json | 1 | Structured error envelopes (D-15) | Already in workspace [VERIFIED: workspace Cargo.toml] |
| tokio | 1 | Async runtime, mpsc channels for DuckDB bridge | Already in workspace [VERIFIED: workspace Cargo.toml] |

### New Dependencies
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| duckdb | 1.10501.0 | DuckDB provider — columnar analytical queries | `providers/memory-duckdb` crate only, feature-gated [VERIFIED: cargo search] |
| grafeo | 0.5.37 | Grafeo embedded graph database | `providers/memory-grafeo` crate only, feature-gated [VERIFIED: cargo search] |
| mockall | 0.14.0 | Mock MemoryProvider for unit tests (D-17) | dev-dependencies in crates that test agent logic [VERIFIED: cargo search] |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Dedicated OS thread for DuckDB | `tokio::task::spawn_blocking` per call | spawn_blocking is per-call overhead; D-03 explicitly chose persistent thread for clean Send/Sync boundary |
| grafeo 0.5.37 | indradb (embedded) | grafeo is pure Rust, no C deps; indradb is a server, not embedded |
| mockall | manual mock structs | mockall auto-generates; D-17 permits either approach |

**Installation for new provider crates:**
```toml
# providers/memory-sqlite/Cargo.toml
[dependencies]
ironhermes-core = { path = "../../crates/ironhermes-core" }
rusqlite = { version = "0.32", features = ["bundled", "backup"] }

# providers/memory-duckdb/Cargo.toml
[dependencies]
ironhermes-core = { path = "../../crates/ironhermes-core" }
duckdb = { version = "1.1" }  # semver: 1.10501 maps to ~1.1.x in normal semver

# providers/memory-grafeo/Cargo.toml
[dependencies]
ironhermes-core = { path = "../../crates/ironhermes-core" }
grafeo = "0.5"
```

**Version note on duckdb:** cargo search returns "1.10501.0" which is an unusual version string. The actual crates.io release is `duckdb = "1.1"` tracking DuckDB 1.1.x. Use `duckdb = "1"` for workspace flexibility. [ASSUMED — verify with `cargo search duckdb` output interpretation against crates.io registry page]

---

## Architecture Patterns

### Recommended Workspace Structure
```
providers/
├── memory-sqlite/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs         # SqliteMemoryProvider implements MemoryProvider
│       └── schema.rs      # CREATE TABLE, FTS5, triggers, migrations
├── memory-duckdb/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs         # DuckDbMemoryProvider implements MemoryProvider
│       ├── bridge.rs      # Dedicated OS thread + mpsc channel (D-03)
│       └── schema.rs      # CREATE TABLE for columnar facts
└── memory-grafeo/
    ├── Cargo.toml
    └── src/
        ├── lib.rs         # GrafeoMemoryProvider implements MemoryProvider
        └── schema.rs      # Node/edge schema for memory entries

crates/ironhermes-agent/src/memory/
├── factory.rs             # build_memory_provider() relocated from core (D-11)
└── mod.rs
```

Workspace `Cargo.toml` adds:
```toml
members = [
    # ... existing
    "providers/memory-sqlite",
    "providers/memory-duckdb",
    "providers/memory-grafeo",
]
```

### Pattern 1: Cargo Feature-Gated Provider Selection (D-12)

Entry-point crates declare optional deps behind features:

```toml
# ironhermes-agent/Cargo.toml
[features]
memory-sqlite = ["dep:memory-sqlite"]
memory-duckdb = ["dep:memory-duckdb"]
memory-grafeo = ["dep:memory-grafeo"]

[dependencies]
memory-sqlite = { path = "../../providers/memory-sqlite", optional = true }
memory-duckdb = { path = "../../providers/memory-duckdb", optional = true }
memory-grafeo = { path = "../../providers/memory-grafeo", optional = true }
```

Factory uses cfg to conditionally compile:

```rust
// crates/ironhermes-agent/src/memory/factory.rs
pub fn build_memory_provider(
    config: &MemoryConfig,
) -> anyhow::Result<Arc<Mutex<dyn MemoryProvider + Send>>> {
    match config.provider.as_str() {
        "file" => {
            let store = MemoryStore::new(/* ... */);
            Ok(Arc::new(Mutex::new(store)))
        }
        #[cfg(feature = "memory-sqlite")]
        "sqlite" => {
            let provider = memory_sqlite::SqliteMemoryProvider::new(/* ... */);
            Ok(Arc::new(Mutex::new(provider)))
        }
        "sqlite" | "duckdb" | "grafeo" => {
            anyhow::bail!(
                "Provider '{}' requires feature flag. Rebuild with --features memory-{}",
                config.provider, config.provider
            )
        }
        other => anyhow::bail!("Unknown memory provider '{}'", other),
    }
}
```

Note: The factory currently returns `Box<dyn MemoryProvider + Send>` in core. `MemoryTool` wraps `Arc<Mutex<dyn MemoryProvider + Send>>`. The factory should return `Arc<Mutex<...>>` to match MemoryTool's expected type. [VERIFIED: memory_tool.rs line 10 — stores `Arc<Mutex<dyn MemoryProvider + Send>>`]

### Pattern 2: DuckDB Dedicated Thread Bridge (D-03)

DuckDB's `Connection` is `!Send`. Standard pattern is a persistent worker thread with mpsc:

```rust
// providers/memory-duckdb/src/bridge.rs
enum DuckDbCommand {
    Add { target: String, content: String, respond: oneshot::Sender<MemoryResult> },
    Remove { target: String, old_text: String, respond: oneshot::Sender<MemoryResult> },
    Replace { target: String, old_text: String, new_content: String, respond: oneshot::Sender<MemoryResult> },
    Query { target: String, respond: oneshot::Sender<anyhow::Result<Vec<String>>> },
    Shutdown,
}

pub struct DuckDbBridge {
    tx: mpsc::Sender<DuckDbCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DuckDbBridge {
    pub fn new(db_path: &Path) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let db_path = db_path.to_owned();
        let thread = std::thread::spawn(move || {
            let conn = duckdb::Connection::open(&db_path).expect("open duckdb");
            // init schema
            for cmd in rx {
                match cmd {
                    DuckDbCommand::Add { target, content, respond } => {
                        let result = /* INSERT INTO facts */ ;
                        let _ = respond.send(result);
                    }
                    DuckDbCommand::Shutdown => break,
                    // ...
                }
            }
        });
        Ok(Self { tx, thread: Some(thread) })
    }
}
// Source: D-03 decision + Rust async-sync interop pattern [ASSUMED — DuckDB API specifics]
```

### Pattern 3: Agent Loop Tool Interception (D-07)

The agent loop currently dispatches all tools through `self.registry.execute_tool()`. Both `memory` and `session_search` tools need to be intercepted before this dispatch since they require access to internal state (`MemoryProvider` / `StateStore`).

The `AgentLoop` struct needs two new optional fields:

```rust
pub struct AgentLoop {
    // ... existing fields ...
    memory_provider: Option<Arc<std::sync::Mutex<dyn MemoryProvider + Send>>>,
    state_store: Option<Arc<std::sync::Mutex<StateStore>>>,
}
```

In `execute_tool_call()`, before the registry dispatch, add early-return interception:

```rust
// Before guardrail check — intercept internal tools
match name.as_str() {
    "memory" => return self.handle_memory_tool(args).await,
    "session_search" => return self.handle_session_search(args).await,
    _ => {}
}
// ... existing guardrail + registry dispatch ...
```

[VERIFIED: agent_loop.rs — no existing interception; tool calls go directly to registry.execute_tool() at line 618]

### Pattern 4: Capacity Header in format_for_system_prompt (MEM-04, D-13)

Current `MemoryStore::format_for_system_prompt()` returns `## Memory\n\n{entries}` without capacity.

Required format: `## Memory (67% -- 1,474/2,200 chars)\n\n{entries}`

The `MemoryProvider` trait's `format_for_system_prompt()` signature is `fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String>`. The char limit is available via `target.char_limit()`. The current char count needs to be computed from live entries.

Implementation in `MemoryStore::format_for_system_prompt()`:

```rust
pub fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String> {
    let entries = self.entries.get(&target)?;
    if entries.is_empty() { return None; }
    let used = char_count(entries, ENTRY_DELIMITER);
    let limit = target.char_limit();
    let pct = used * 100 / limit;
    let header = match target {
        MemoryTarget::Memory => "Memory",
        MemoryTarget::User => "User Profile",
    };
    let header_line = format!("## {} ({}% -- {}/{} chars)", header, pct, used, limit);
    Some(format!("{}\n\n{}", header_line, entries.join("\n")))
}
```

NOTE: The existing `format_for_system_prompt` returns from the **frozen snapshot** (not live entries). MEM-04 says the header appears in the system prompt — which is built from the frozen snapshot. This means capacity in the frozen snapshot is the capacity at session start. This is correct behavior per the frozen-snapshot pattern (MEM-06). [VERIFIED: memory_store.rs lines 334-353 — snapshot is captured once at load_from_disk()]

### Pattern 5: SQLite Memory Provider Schema

```sql
-- providers/memory-sqlite/src/schema.rs
CREATE TABLE IF NOT EXISTS memory_facts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    target      TEXT NOT NULL CHECK(target IN ('memory', 'user')),
    content     TEXT NOT NULL,
    created_at  REAL NOT NULL
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
```

This mirrors the messages_fts pattern already in StateStore. [VERIFIED: ironhermes-state/src/lib.rs lines 89-108]

### Pattern 6: session_search Tool Schema and Result Format

Per D-05 and D-06, the tool wraps `StateStore.search_messages()` with a simplified interface:

```rust
// Tool schema input:
// { "query": "rust async", "role_filter": ["user", "assistant"], "source_filter": ["cli"], "limit": 5 }

// Maps to SearchFilter:
SearchFilter {
    query: Some(sanitized_query),
    role: role_filter.and_then(|v| v.first().cloned()), // NOTE: SearchFilter.role is single string
    source: source_filter.and_then(|v| v.first().cloned()),
    limit: limit.unwrap_or(5),
    ..Default::default()
}
```

IMPORTANT: Current `SearchFilter` has `role: Option<String>` and `source: Option<String>` (single values). The tool schema allows arrays for role_filter and source_filter (D-05). The session_search handler must either (a) only use the first element of each filter array, or (b) the planner should handle multi-value filtering via multiple searches or SQL IN clause. This is a design gap to resolve. [VERIFIED: ironhermes-state/src/lib.rs lines 170-200]

Result format per D-06: FTS5 snippets with `>>>match<<<` markers. Note: existing StateStore uses `<<match>>` markers (lines 521-524). The session_search tool must use `>>>match<<<` per D-06 — this means either (a) the tool re-queries with different snippet markers, or (b) post-processes the `<<match>>` markers to `>>>match<<<`. Either approach works; post-processing is simpler.

### Anti-Patterns to Avoid

- **Putting provider crates in `crates/`**: Decision D-09 specifies `providers/` at workspace root. Do not add them to `crates/`.
- **Circular dependencies**: Provider crates must depend on `ironhermes-core` (for the trait) but NOT on `ironhermes-agent`. Factory in agent depends on providers. [VERIFIED: D-10]
- **spawn_blocking for DuckDB**: Per D-03, use a persistent dedicated OS thread, not `tokio::task::spawn_blocking` on every call. The Connection is `!Send` and cannot be moved across threads via spawn_blocking safely.
- **Mutating frozen snapshot**: `format_for_system_prompt` returns from the frozen snapshot (loaded at session start). Never update the snapshot mid-session. The capacity header in the snapshot reflects session-start capacity, which is correct.
- **Registering memory/session_search in ToolRegistry**: These tools are intercepted before registry dispatch (D-07, TOOL-04). They must NOT go through the registry — they require direct access to internal state.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| FTS5 full-text search | Custom text indexing | rusqlite with FTS5 virtual table | Already proven in StateStore; FTS5 handles tokenization, ranking, snippets |
| Graph storage | Adjacency list in SQLite | grafeo 0.5.37 | Pure Rust, embedded, no C deps; relationship queries are first-class |
| DuckDB !Send bridge | Per-call spawn_blocking with connection pooling | Dedicated OS thread + mpsc | D-03 decision; spawn_blocking cannot safely move !Send Connection |
| Mock MemoryProvider | Manually write mock structs | mockall 0.14.0 | Auto-generates mock impls from trait; less boilerplate for test-only code |
| FTS5 query sanitization | Custom parser | Port existing `sanitize_fts_query()` | Already battle-tested in StateStore; session_search uses same D-08 rules |

---

## Common Pitfalls

### Pitfall 1: Factory Return Type Mismatch
**What goes wrong:** `build_memory_provider()` in core returns `Box<dyn MemoryProvider + Send>`, but `MemoryTool` stores `Arc<Mutex<dyn MemoryProvider + Send>>`. The relocated factory in agent must return `Arc<Mutex<...>>` for direct use with MemoryTool without wrapping at call sites.
**Why it happens:** The core factory was written before the sharing pattern was finalized.
**How to avoid:** The new `ironhermes-agent/src/memory/factory.rs` should return `Arc<Mutex<dyn MemoryProvider + Send>>` directly.
**Warning signs:** Compilation error at MemoryTool construction sites.

### Pitfall 2: DuckDB Send/Sync Boundary
**What goes wrong:** `duckdb::Connection` is `!Send`. Attempting to share it across async tasks or use with `Arc<Mutex<...>>` in a tokio context fails at compile time.
**Why it happens:** DuckDB's C bindings are not thread-safe in the traditional sense.
**How to avoid:** Implement the dedicated OS thread pattern (D-03) in `providers/memory-duckdb/src/bridge.rs`. The `DuckDbMemoryProvider` struct owns the `DuckDbBridge` (which holds only `mpsc::Sender`, which IS Send) so the provider itself becomes Send + Sync.
**Warning signs:** `error[E0277]: dyn MemoryProvider cannot be sent between threads safely` or similar.

### Pitfall 3: FTS5 Snippet Marker Mismatch
**What goes wrong:** StateStore uses `<<match>>` markers (confirmed in lib.rs). D-06 specifies `>>>match<<<` markers for session_search results.
**Why it happens:** Python hermes-agent used `>>>match<<<`; the Rust StateStore was implemented with `<<match>>`.
**How to avoid:** session_search handler post-processes snippet: `snippet.replace("<<", ">>>").replace(">>", "<<<")`. Or query with the correct markers directly (snippet() takes them as parameters — they are already parameterized at lines 521-523).
**Warning signs:** Agent seeing different snippet format than described in tool schema.

### Pitfall 4: Role/Source Multi-Value Filter Mismatch
**What goes wrong:** D-05 schema has `role_filter: array` and `source_filter: array`, but `SearchFilter` fields are `Option<String>` (single values). Multi-role queries silently drop all but the first value.
**Why it happens:** SearchFilter was designed for single-filter use.
**How to avoid:** session_search handler should document this limitation in the tool description, or the planner should address extending SearchFilter to support `Vec<String>` for role and source. This is a discrete sub-task.
**Warning signs:** User query `role_filter: ["user", "assistant"]` only searches one role.

### Pitfall 5: Capacity Header Updates Snapshot
**What goes wrong:** Capacity is added to `format_for_system_prompt()` which currently returns from the frozen snapshot. If the snapshot is stored as a pre-formatted string (current implementation), the capacity in it is fixed at session start and does NOT reflect mid-session mutations. This is actually correct (per frozen-snapshot pattern), but may confuse implementors who expect live capacity.
**Why it happens:** Frozen snapshot pattern stores the formatted block, not raw entries.
**How to avoid:** The capacity header in the system prompt reflects session-start capacity. Mid-session operations return live capacity in their tool response (D-14). Both are correct — they serve different purposes.
**Warning signs:** Tests expecting live capacity in `format_for_system_prompt()` output.

### Pitfall 6: Missing `dump()` / `add_batch()` on MemoryProvider Trait
**What goes wrong:** D-16 migration uses `provider_a.dump()` into `provider_b.add_batch()`. These methods are NOT currently in the `MemoryProvider` trait (only add/replace/remove/format/to_memory_entries).
**Why it happens:** Migration methods were not part of Phase 11 trait design.
**How to avoid:** The planner must add `dump(&self) -> anyhow::Result<MemoryEntries>` and `add_batch(&mut self, entries: &MemoryEntries) -> anyhow::Result<()>` to the trait, or implement migration using `to_memory_entries()` + repeated `add()` calls. The latter avoids a trait change. [VERIFIED: memory_provider.rs — trait does not have dump/add_batch]

---

## Code Examples

### Capacity in format_for_system_prompt
```rust
// Source: Derived from existing memory_store.rs char_count() + MemoryTarget::char_limit()
pub fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String> {
    let entries = self.snapshot_entries.get(&target)?; // entries at snapshot time
    if entries.is_empty() { return None; }
    let used = char_count(entries, ENTRY_DELIMITER);
    let limit = target.char_limit();
    let pct = used * 100 / limit;
    let label = match target {
        MemoryTarget::Memory => "Memory",
        MemoryTarget::User => "User Profile",
    };
    let header = format!("## {} ({}% -- {}/{} chars)", label, pct, used, limit);
    Some(format!("{}\n\n{}", header, entries.join("\n")))
}
```

### session_search handler in agent loop
```rust
// Source: Derived from StateStore.search_messages() API in ironhermes-state/src/lib.rs
async fn handle_session_search(&self, args: serde_json::Value) -> String {
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) => q.to_string(),
        None => return r#"{"error":"missing_query","reason":"query parameter is required"}"#.to_string(),
    };
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
    // role_filter, source_filter: use first element only (SearchFilter limitation)
    let role = args.get("role_filter")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(String::from);
    let source = args.get("source_filter")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(String::from);

    let filter = SearchFilter { query: Some(query), role, source, limit, ..Default::default() };

    let state = match &self.state_store {
        Some(s) => s,
        None => return r#"{"error":"unavailable","reason":"state store not configured"}"#.to_string(),
    };
    let results = tokio::task::spawn_blocking({
        let state = state.clone();
        let filter = filter.clone();
        move || state.lock().unwrap().search_messages(&filter)
    }).await;
    // ... format results with >>>match<<< markers
}
```

### Grafeo provider skeleton
```rust
// Source: grafeo = "0.5.37" [ASSUMED — API specifics not verified against Context7]
pub struct GrafeoMemoryProvider {
    db: grafeo::Database,
    config: MemoryProviderConfig,
}

#[async_trait]
impl MemoryProvider for GrafeoMemoryProvider {
    fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult {
        let scanned = scan_context_content(content, target.filename());
        if scanned.contains("[BLOCKED:") {
            return Err(/* ... */);
        }
        // Create node: Node(MemoryEntry) with content property
        // Create edge: Node(target_label) -[HAS_ENTRY]-> Node(MemoryEntry)
        // ... grafeo API [ASSUMED]
    }
    // ...
}
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Factory in `ironhermes-core` | Factory in `ironhermes-agent` | Phase 17 | Breaks circular dep: providers depend on core, agent depends on providers |
| No external providers (file only) | SQLite/Grafeo/DuckDB as feature-gated workspace members | Phase 17 | Enables richer memory backends without bloating default binary |
| Tool dispatch always through registry | Internal tools intercepted before registry | Phase 17 | memory + session_search require internal state inaccessible to ToolRegistry |
| No capacity display in system prompt | Capacity header in frozen snapshot | Phase 17 | Agent is informed of budget at session start |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | duckdb crate version "1.10501.0" from cargo search maps to `duckdb = "1.1"` in Cargo.toml | Standard Stack | Wrong version spec causes build failure; verify with `cargo add duckdb` or crates.io page |
| A2 | grafeo 0.5.37 API supports embedded use without HTTP server via `grafeo::Database` | Code Examples | API may differ; verify with Context7 or grafeo docs before implementing |
| A3 | DuckDB `Connection::open()` signature and basic INSERT/SELECT API | Architecture Patterns | DuckDB Rust API may differ; verify with Context7 before bridge implementation |
| A4 | Migration can use `to_memory_entries()` + repeated `add()` calls as alternative to missing `dump()`/`add_batch()` trait methods | Pitfalls | If `add()` has side effects that prevent batch import, need different approach |
| A5 | mockall 0.14.0 can derive mock for `MemoryProvider` (async_trait + Send + Sync) | Standard Stack | mockall + async_trait interaction can be tricky; may need `#[automock]` with `async_trait` workaround |

---

## Open Questions

1. **SearchFilter multi-value role/source filtering**
   - What we know: D-05 schema has `role_filter: array`, current SearchFilter has single `role: Option<String>`
   - What's unclear: Should session_search silently use only the first filter value, or should SearchFilter be extended to Vec in this phase?
   - Recommendation: Extend SearchFilter in ironhermes-state to `roles: Vec<String>` and `sources: Vec<String>` with SQL `IN` clause. Small change, correct semantics.

2. **snapshot capacity recalculation**
   - What we know: `format_for_system_prompt` currently returns a pre-formatted string from `self.snapshot` (a `HashMap<MemoryTarget, String>`). The capacity header must be in the snapshot.
   - What's unclear: The snapshot stores the formatted block, not raw entries. To add capacity, either (a) rebuild snapshot on load to include capacity, or (b) store entries separately in snapshot and format lazily.
   - Recommendation: Store entries in snapshot (`HashMap<MemoryTarget, Vec<String>>`), format lazily in `format_for_system_prompt`. This is a refactor of the snapshot field type in MemoryStore.

3. **`add_batch()` for migration**
   - What we know: D-16 migration uses `add_batch()` which doesn't exist on the trait
   - What's unclear: Whether migration is in-scope for this phase or deferred
   - Recommendation: Implement migration using `to_memory_entries()` + repeated `add()` calls; no trait change needed. Add a `migrate_provider()` utility function in factory.rs.

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust/cargo | All provider crates | ✓ | (workspace) | — |
| rusqlite (bundled) | SQLite provider | ✓ | 0.32 | — |
| duckdb crate | DuckDB provider | Available on crates.io | 1.10501.0 | — |
| grafeo crate | Grafeo provider | Available on crates.io | 0.5.37 | — |
| mockall crate | Test mocks | Available on crates.io | 0.14.0 | Manual mock structs |
| Docker | DuckDB integration tests | Unknown (not checked) | — | Skip integration tests in CI; unit tests only |

**Missing dependencies with no fallback:** None that block core implementation.

**Missing dependencies with fallback:**
- Docker: DuckDB integration tests behind `#[cfg(feature = "integration-tests")]` per D-18. If Docker absent, those tests simply don't run.

---

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Config file | None — Cargo.toml `[dev-dependencies]` |
| Quick run command | `cargo test -p ironhermes-core -p ironhermes-tools -p ironhermes-agent 2>&1` |
| Full suite command | `cargo test --workspace 2>&1` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| MEM-01 | add/replace/remove MEMORY.md via tool | unit | `cargo test -p ironhermes-tools memory_tool 2>&1` | ✅ memory_tool.rs |
| MEM-02 | add/replace/remove USER.md via tool | unit | `cargo test -p ironhermes-tools memory_tool 2>&1` | ✅ memory_tool.rs |
| MEM-03 | substring matching for replace/remove | unit | `cargo test -p ironhermes-core memory_store 2>&1` | ✅ memory_store.rs |
| MEM-04 | capacity header in system prompt | unit | `cargo test -p ironhermes-core memory_store::format_for_system_prompt 2>&1` | ❌ Wave 0 — add capacity assertion to existing test |
| MEM-05 | security scanning on add | unit | `cargo test -p ironhermes-core memory_store::test_add_blocks_injection 2>&1` | ✅ memory_store.rs |
| MEM-09 | SQLite provider stores + retrieves with FTS5 | unit | `cargo test -p memory-sqlite 2>&1` | ❌ Wave 0 — new crate |
| MEM-10 | Grafeo provider stores entries as graph nodes | unit | `cargo test -p memory-grafeo 2>&1` | ❌ Wave 0 — new crate |
| MEM-11 | DuckDB provider with !Send bridge | unit | `cargo test -p memory-duckdb 2>&1` | ❌ Wave 0 — new crate |
| MEM-13 | session_search tool returns FTS5 results | unit | `cargo test -p ironhermes-agent session_search 2>&1` | ❌ Wave 0 — new module |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-core -p ironhermes-tools 2>&1`
- **Per wave merge:** `cargo test --workspace 2>&1`
- **Phase gate:** Full workspace test suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `providers/memory-sqlite/src/lib.rs` with unit tests — covers MEM-09
- [ ] `providers/memory-duckdb/src/lib.rs` with unit tests — covers MEM-11
- [ ] `providers/memory-grafeo/src/lib.rs` with unit tests — covers MEM-10
- [ ] `crates/ironhermes-agent/src/memory/factory.rs` — covers factory relocation
- [ ] Update `memory_store::test_format_for_system_prompt_with_entries` to assert capacity header — covers MEM-04
- [ ] Add session_search unit test in `crates/ironhermes-agent/src/` — covers MEM-13

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | yes | Memory tool blocked in subagent read-only mode (already implemented) |
| V5 Input Validation | yes | `scan_context_content()` applied to all memory write operations |
| V6 Cryptography | no | — |

### Known Threat Patterns for this Stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via memory entries | Tampering | `scan_context_content()` on every add/replace — already implemented in MemoryStore |
| Memory exfiltration via `curl`/`cat` patterns | Information Disclosure | THREAT_PATTERNS regex in context_scanner.rs blocks exfil patterns |
| Capacity DoS (filling memory to starve agent context) | Denial of Service | Hard char limits (2200/1375) enforced on every add; overflow returns structured error |
| SQL injection via session_search query | Tampering | FTS5 queries parameterized via rusqlite params! macro; sanitize_fts_query() strips dangerous operators |
| DuckDB path traversal (provider file path) | Information Disclosure | Config-provided path only; no user-controlled path expansion |

---

## Sources

### Primary (HIGH confidence)
- `crates/ironhermes-core/src/memory_provider.rs` — MemoryProvider trait, current factory, MemoryProviderConfig
- `crates/ironhermes-core/src/memory_store.rs` — Full MemoryStore implementation, security scanning integration, file locking, atomic writes
- `crates/ironhermes-tools/src/memory_tool.rs` — MemoryTool with Arc<Mutex<dyn MemoryProvider+Send>>
- `crates/ironhermes-state/src/lib.rs` — StateStore FTS5, SearchFilter, sanitize_fts_query, snippet() markers
- `crates/ironhermes-agent/src/agent_loop.rs` — Tool dispatch loop, execute_tool_call (no interception today)
- `crates/ironhermes-agent/src/prompt_builder.rs` — format_for_system_prompt integration, PromptSlot::Memory
- `Cargo.toml` (workspace) — rusqlite 0.32 confirmed, no DuckDB/grafeo/mockall yet
- `crates/ironhermes-agent/Cargo.toml` — No existing features section
- `crates/ironhermes-cli/Cargo.toml` — No existing provider features

### Secondary (MEDIUM confidence)
- `cargo search duckdb` → duckdb = "1.10501.0" (current registry) [VERIFIED]
- `cargo search grafeo` → grafeo = "0.5.37", grafeo-adapters = "0.5.37" [VERIFIED]
- `cargo search mockall` → mockall = "0.14.0" [VERIFIED]
- `cargo search rusqlite` → rusqlite = "0.39.0" (latest; workspace pins 0.32) [VERIFIED]

### Tertiary (LOW confidence)
- grafeo 0.5.37 API surface (embedded database patterns) — not verified via Context7
- DuckDB Connection `!Send` bridge pattern specifics — training knowledge, not verified against duckdb-rs docs

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all libraries verified via cargo search/workspace files
- Architecture: HIGH — patterns derived directly from existing codebase
- Pitfalls: HIGH — derived from verified code inspection, not assumption
- Grafeo/DuckDB API specifics: LOW — not verified via Context7 or official docs

**Research date:** 2026-04-12
**Valid until:** 2026-05-12 (grafeo/duckdb crate APIs stable but worth re-checking before implementation)
