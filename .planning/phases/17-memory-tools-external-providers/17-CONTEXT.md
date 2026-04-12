# Phase 17: Memory Tools & External Providers - Context

**Gathered:** 2026-04-12
**Status:** Ready for planning

<domain>
## Phase Boundary

Memory tool operations (add/replace/remove with capacity tracking, security scanning, and substring matching) for MEMORY.md and USER.md bounded stores. External memory provider backends (SQLite, Grafeo, DuckDB) as separate per-provider crates implementing the existing MemoryProvider trait. session_search tool wrapping StateStore FTS5 for past conversation retrieval. Provider factory relocation from core to agent crate with Cargo feature-gated compilation.

</domain>

<decisions>
## Implementation Decisions

### External provider data model
- **D-01:** SQLite memory provider mirrors the file format — one row per entry with target, content, created_at columns. FTS5 virtual table for full-text search across memory entries. Straightforward migration path from file-based provider.
- **D-02:** Grafeo uses embedded library (not external HTTP service). Memory entries stored as graph nodes. Metadata keys become edge labels (e.g., `Node(User) -[INTEREST]-> Node(MemoryEntry)`). Enables multi-hop relationship queries that standard SQL cannot express. External HTTP Grafeo may come later.
- **D-03:** DuckDB uses a dedicated OS thread owning the Connection (which is `!Send`). Commands sent via mpsc channel, results returned back. Same pattern as spawn_blocking but with a persistent thread for clean Send/Sync boundary.
- **D-04:** DuckDB uses a flat columnar table for memory facts. Optimized for analytical queries (e.g., "How many times did I mention 'Rust' in the last 30 days?"). Vectorized execution for fast aggregations.

### session_search tool
- **D-05:** Tool schema: `session_search` with parameters `query` (required, FTS5 string), `role_filter` (optional array of user/assistant/system/tool), `source_filter` (optional array of cli/telegram/etc.), `limit` (optional integer, default 5).
- **D-06:** Results surface: FTS5-generated snippets with `>>>match<<<` markers, 1 message before and 1 after the match (each truncated to 200 chars), plus metadata (session_id, role, timestamp, source, model, session_started).
- **D-07:** session_search is intercepted in the agent loop before registry dispatch (same pattern as memory tool). Both tools require access to internal state (StateStore/MemoryProvider) and should not go through the tool registry.
- **D-08:** FTS5 query sanitization follows hermes-agent pattern: strip unmatched quotes, wrap hyphenated terms in quotes (prevents FTS5 treating hyphen as NOT), remove dangling boolean operators, strip special characters except `*`, `"`, `-`.

### Feature gates & crate structure
- **D-09:** Per-provider crates under a `providers/` directory at workspace root: `providers/memory-sqlite/`, `providers/memory-duckdb/`, `providers/memory-grafeo/`. Each is a separate workspace member.
- **D-10:** MemoryProvider trait stays in `ironhermes-core/src/memory_provider.rs` (already there from Phase 11). Provider crates depend on `ironhermes-core` for the trait. No circular dependencies — entry-point crates (agent, cli, gateway) depend on provider crates, not the other way around.
- **D-11:** Factory relocates from `ironhermes-core` to `ironhermes-agent/src/memory/factory.rs`. Uses `#[cfg(feature = "memory-sqlite")]` etc. to conditionally import provider crates and instantiate the configured provider as `Box<dyn MemoryProvider>`.
- **D-12:** Entry-point crates (CLI, gateway) use Cargo features to select providers: `features = ["memory-sqlite", "memory-grafeo", "memory-duckdb"]`. Each feature adds the corresponding provider crate as an optional dependency. Default build includes file-based provider only.

### Capacity display & tool UX
- **D-13:** Capacity appears as header line per store in system prompt: `## Memory (67% -- 1,474/2,200 chars)` and `## User Profile (42% -- 578/1,375 chars)`. Agent sees usage at a glance in the frozen snapshot (MEM-04).
- **D-14:** Memory tool success response: confirmation + updated capacity. Example: `"Added to memory. Memory: 72% -- 1,584/2,200 chars (3 entries)"`.
- **D-15:** Errors use structured error envelopes matching hermes-agent pattern. Capacity overflow: `{"error": "capacity_exceeded", "current": 2150, "limit": 2200, "entry_size": 180, "suggestion": "Remove an entry first"}`. Security rejection: `{"error": "content_rejected", "reason": "injection_pattern_detected"}`.

### Provider data migration
- **D-16:** Manual-triggered automatic migration when config changes provider. Agent detects provider mismatch and prompts user: "Existing memory found in [File]. Migrate entries to [SQLite]? (y/n)". Migration uses trait operations: `provider_a.dump()` into `provider_b.add_batch()`. Data loss is NOT the default.

### Testing strategy
- **D-17:** Mock trait implementations (via mockall or manual mock structs) in ironhermes-core for testing agent logic independently of database backends.
- **D-18:** Docker-based integration tests live in provider crates (`providers/memory-duckdb/tests/`) behind `#[cfg(feature = "integration-tests")]` gate. Heavy containers only spin up during dedicated CI runs, not local development.

### Claude's Discretion
- SQLite memory provider schema details (indexes, FTS5 trigger setup, migration versioning)
- Grafeo library selection and specific graph schema (crate availability may constrain design)
- DuckDB table schema and analytical query patterns
- session_search result formatting (exact text layout returned to agent)
- Migration utility implementation details (dump/load batch size, progress reporting)
- Whether `build_memory_provider` returns `Box<dyn MemoryProvider>` or `Arc<Mutex<dyn MemoryProvider>>` based on current usage patterns

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Memory requirements
- `.planning/REQUIREMENTS.md` -- MEM-01 (add/replace/remove MEMORY.md), MEM-02 (USER.md), MEM-03 (substring matching), MEM-04 (capacity in prompt), MEM-05 (security scanning), MEM-09 (SQLite provider + FTS5), MEM-10 (Grafeo provider), MEM-11 (DuckDB provider), MEM-13 (session_search tool)

### hermes-agent architecture (user-provided during discussion)
- hermes-agent `hermes_state.py` -- Session storage architecture: SQLite+WAL, FTS5 with `>>>match<<<` snippet markers, SearchFilter composable WHERE clauses, query sanitization, session lineage, write contention handling
- hermes-agent `memory_manager.py` -- Memory orchestration pattern, provider plugin architecture
- hermes-agent `plugins/memory/` -- Provider implementation pattern (trait + separate plugin modules)

### Existing implementation (primary code references)
- `crates/ironhermes-core/src/memory_provider.rs` -- MemoryProvider trait (Phase 11), MemoryProviderConfig, MemoryEntries, MemoryStore impl
- `crates/ironhermes-core/src/memory_store.rs` -- MemoryStore: file-based add/replace/remove, security scanning, frozen snapshots, file locking
- `crates/ironhermes-tools/src/memory_tool.rs` -- MemoryTool wrapping `Arc<Mutex<dyn MemoryProvider>>`, read-only subagent mode, existing tool schema
- `crates/ironhermes-state/src/lib.rs` -- StateStore with FTS5 search, SearchFilter, snippet(), sanitization (Phase 13)
- `crates/ironhermes-core/src/constants.rs` -- MEMORY_FILENAME, USER_FILENAME, char limits, ENTRY_DELIMITER
- `crates/ironhermes-core/src/context_scanner.rs` -- `scan_context_content()` for security scanning

### Prior phase context
- `.planning/phases/11-memory-provider-trait/11-CONTEXT.md` -- Trait design, lifecycle hooks, config-driven selection, error semantics
- `.planning/phases/13-session-storage/13-CONTEXT.md` -- StateStore architecture, FTS5 search, SearchFilter, write contention
- `.planning/phases/15-10-layer-prompt-assembly/15-CONTEXT.md` -- Frozen memory snapshots in slot 3, capacity headers, PromptSlot ordering

### Architecture
- `.planning/codebase/ARCH.md` -- Crate dependency graph, Tool trait pattern
- `.planning/ROADMAP.md` -- Phase 17 success criteria, downstream dependencies (Phase 18, 21 depend on 17)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `MemoryProvider` trait (`memory_provider.rs`): Already defined with all 5 lifecycle hooks + operational methods (add, replace, remove, format_for_system_prompt, to_memory_entries). External providers implement this directly.
- `MemoryStore` (`memory_store.rs`): Full file-based implementation with security scanning, file locking, frozen snapshots. Stays as default provider.
- `MemoryTool` (`memory_tool.rs`): Already wraps `Arc<Mutex<dyn MemoryProvider>>` with add/replace/remove/get actions and read-only subagent mode. May need response format updates for capacity feedback.
- `StateStore` (`ironhermes-state/src/lib.rs`): FTS5 search, SearchFilter, snippet(), sanitization already implemented in Phase 13. session_search tool wraps this.
- `build_memory_provider()` factory (`memory_provider.rs`): Currently in core, relocates to agent crate.
- `scan_context_content()` (`context_scanner.rs`): Security scanning reused by all providers for MEM-05.

### Established Patterns
- Agent-loop tool interception: memory tool calls already intercepted before registry dispatch in `agent_loop.rs`
- `Arc<Mutex<dyn MemoryProvider>>` sharing pattern for MemoryTool
- `spawn_blocking` for sync-in-async (rusqlite operations throughout codebase)
- Config-driven provider selection with hard error on missing feature-gated provider (Phase 11 D-09)
- Frozen-snapshot pattern: memory loaded once at session start, mutations persist to disk but don't change active prompt

### Integration Points
- `agent_loop.rs`: Add session_search intercept alongside existing memory intercept
- `prompt_builder.rs`: Update format_for_system_prompt to include capacity headers (MEM-04)
- `Cargo.toml` workspace: Add `providers/memory-sqlite`, `providers/memory-duckdb`, `providers/memory-grafeo` as members
- Entry-point Cargo.toml files (cli, gateway): Add optional provider deps behind feature flags
- `ironhermes-agent/src/memory/factory.rs`: New module for relocated provider factory

</code_context>

<specifics>
## Specific Ideas

- hermes-agent `hermes_state.py` documentation provided by user as canonical reference for session storage architecture, FTS5 search patterns, query sanitization, and search result format. Port faithfully.
- hermes-agent `plugins/memory/` directory structure as inspiration for per-provider crate layout under `providers/` directory.
- Provider migration is explicit but guided -- user is prompted on config change, not silently migrated or data-lost.
- Both memory and session_search tools intercepted before registry dispatch to keep the Tool System focused on external capabilities.
- DuckDB's `!Send` Connection is handled via dedicated OS thread + mpsc channel, not per-call spawn_blocking.

</specifics>

<deferred>
## Deferred Ideas

None -- discussion stayed within phase scope.

### Reviewed Todos (not folded)
- "Add setup wizard and config scaffolding for gateway testing" -- belongs in Phase 23 (Configuration & Setup Wizard). Already reviewed and deferred in Phases 12, 13, 14, 15.

</deferred>

---

*Phase: 17-memory-tools-external-providers*
*Context gathered: 2026-04-12*
