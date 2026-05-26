# Phase 11: Memory Provider Trait - Context

**Gathered:** 2026-04-11
**Status:** Ready for planning

<domain>
## Phase Boundary

Pluggable MemoryProvider trait with lifecycle hooks so memory backends can be swapped without changing agent code. Built-in file-based MemoryStore implements the trait as the default backend. Single-provider selection via config. External providers (SQLite, Grafeo, DuckDB) are Phase 17 scope — this phase defines the trait and wraps the existing implementation.

</domain>

<decisions>
## Implementation Decisions

### Trait async model
- **D-01:** MemoryProvider trait uses async methods via `#[async_trait]` crate. All 5 lifecycle hooks are async fn. File-based MemoryStore returns immediately from async hooks (trivial async). Future network-backed providers (Grafeo HTTP, DuckDB, SQLite via spawn_blocking) work naturally without caller-side workarounds.
- **D-02:** Trait bounds: `Send + Sync + 'static` as specified by MEM-07.

### Hook data flow
- **D-03:** `initialize(&mut self, config: &MemoryProviderConfig)` — receives a typed config struct containing provider name/type, provider-specific settings, memory dir path, and char limits. Provider sets up its resources during this hook.
- **D-04:** `prefetch(&self, session_id: &str) -> Result<MemoryEntries>` — loads provider state for the given session. Returns a `MemoryEntries` wrapper (around `HashMap<MemoryTarget, Vec<String>>`).
- **D-05:** `sync_turn(&self, session_id: &str, entries: &MemoryEntries)` — receives the current memory entries after a mutation so the provider can index, cache, or sync.
- **D-06:** `on_session_end(&self, session_id: &str, entries: &MemoryEntries)` — receives session ID and final memory state. Provider persists/flushes as needed. Avoids requiring the provider to independently track state.
- **D-07:** `shutdown(&mut self)` — clean teardown of provider resources. No session context needed.

### Provider selection
- **D-08:** Config-driven via `memory.provider` key in config.yaml. Values: `"file"` (default), `"sqlite"`, `"grafeo"`, `"duckdb"`. Provider-specific settings live under `memory.<provider_name>:` namespace (e.g., `memory.sqlite.path`, `memory.grafeo.url`).
- **D-09:** If config specifies a provider that isn't compiled in (feature-gated), hard error at startup with a clear message listing available providers and the required feature flag. No silent fallback.
- **D-10:** Default provider is `"file"` when `memory.provider` is absent from config. The file-based provider requires no additional configuration beyond the default memory directory.

### Error semantics
- **D-11:** `initialize()` and `shutdown()` errors are fatal — propagated up, prevent startup or leave a logged error on teardown. Can't operate without a working provider.
- **D-12:** `prefetch()`, `sync_turn()`, and `on_session_end()` errors are logged as warnings but do not crash the agent or terminate the session. On prefetch failure, return empty entries. On sync_turn/on_session_end failure, log and skip. The frozen-snapshot pattern means the session remains usable even if the provider has transient failures.

### Claude's Discretion
- Crate placement for the trait (likely ironhermes-core where MemoryStore already lives)
- MemoryProviderConfig struct design (fields, serde derive, validation)
- MemoryEntries wrapper type design
- How to refactor existing MemoryStore to implement the trait while preserving all current behavior and tests
- Whether to use Rust native async traits (RPITIT) or async_trait crate, based on MSRV compatibility
- Provider factory/registry pattern for instantiating the configured provider

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Memory provider requirements
- `.planning/REQUIREMENTS.md` — MEM-07 (trait bounds + lifecycle hooks), MEM-08 (file-based MemoryStore implements trait), MEM-12 (single-provider selection)

### Existing memory implementation
- `crates/ironhermes-core/src/memory_store.rs` — Current MemoryStore: file-based, synchronous, add/replace/remove, file locking, frozen snapshots
- `crates/ironhermes-tools/src/memory_tool.rs` — MemoryTool wrapping Arc<Mutex<MemoryStore>>, read-only subagent mode
- `crates/ironhermes-core/src/constants.rs` — MEMORY_FILENAME, USER_FILENAME, char limits, ENTRY_DELIMITER

### Architecture
- `.planning/codebase/ARCH.md` — Crate dependency graph, key abstractions (Tool trait pattern), concurrency model
- `.planning/ROADMAP.md` — Phase 11 success criteria, downstream phase dependencies (12, 13, 14 depend on 11)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `MemoryStore` (`ironhermes-core/src/memory_store.rs`): Full add/replace/remove with file locking, frozen snapshots, security scanning — becomes the file-based MemoryProvider implementation
- `MemoryTarget` enum: Already defines Memory vs User targets with filenames, char limits, labels
- `MemoryTool` (`ironhermes-tools/src/memory_tool.rs`): Wraps MemoryStore as Arc<Mutex<>>; will need updating to use `dyn MemoryProvider` instead of concrete MemoryStore
- `scan_context_content()` (`ironhermes-core/src/context_scanner.rs`): Security scanning already used by MemoryStore — trait implementations should continue using it
- `Config` (`ironhermes-core/src/config.rs`): Hierarchical YAML config with serde — add memory.provider section here

### Established Patterns
- `Tool` trait in `ironhermes-tools/src/registry.rs`: async_trait + Send + Sync pattern, HashMap-based registry — MemoryProvider follows the same architectural pattern
- `Arc<Mutex<MemoryStore>>` sharing pattern in MemoryTool — will become `Arc<Mutex<dyn MemoryProvider>>` or similar
- `StateStore` uses rusqlite synchronously — async bridge via spawn_blocking is the established pattern for sync-in-async

### Integration Points
- `MemoryTool::new()` takes `Arc<Mutex<MemoryStore>>` — needs to accept trait object
- `PromptBuilder` in `ironhermes-agent/src/prompt_builder.rs` reads memory for system prompt
- `agent_loop.rs` handles memory tool calls — intercepts before registry dispatch
- `runner.rs` in gateway and `main.rs` in CLI both construct MemoryStore — will construct provider from config instead

</code_context>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches. The trait design should mirror the existing Tool trait pattern (async_trait + Send + Sync) for consistency across the codebase.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 11-memory-provider-trait*
*Context gathered: 2026-04-11*
