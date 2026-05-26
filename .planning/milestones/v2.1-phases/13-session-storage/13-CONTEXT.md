# Phase 13: Session Storage - Context

**Gathered:** 2026-04-11
**Status:** Ready for planning

<domain>
## Phase Boundary

Durable SQLite-backed session persistence with FTS5 full-text search, lineage tracking, write-through caching, and migrations — wired as source of truth across CLI, gateway, and ACP. The `ironhermes-state` crate already has a substantial `StateStore` with SQLite+WAL, sessions/messages tables, FTS5 triggers, schema migrations (v1-v6), session titles, and parent_session_id column. This phase completes the missing functionality and integrates StateStore into all call sites.

</domain>

<decisions>
## Implementation Decisions

### Write-through cache architecture
- **D-01:** SessionStore becomes a thin cache wrapper around StateStore. Every `add_message`/`create_session` writes to SQLite immediately via StateStore, while keeping recent messages in memory (HashMap) for fast gateway access. The struct composes both: `SessionStore { state: StateStore, cache: HashMap<String, GatewaySession> }`.
- **D-02:** No auto-recovery of sessions into LLM context on restart. All sessions persist to SQLite but retrieval is manual via the `session_search` tool (Phase 17). The in-memory cache holds only the current running session — on restart, a fresh session starts and old data is query-only. This matches hermes-agent's approach.
- **D-03:** All sources share a single `state.db` — CLI, gateway, cron, and future ACP all write to the same SQLite database at `~/.ironhermes/state.db`. The `source` column distinguishes session origin.

### Search & filtering
- **D-04:** Search results use FTS5 `snippet()` function to return text fragments with `<<match>>` markers around hits, plus 1 message of surrounding context (`context_before`/`context_after` fields). SearchResult includes snippet, session_title, source, and timestamp.
- **D-05:** FTS5 input is sanitized by default — strip FTS5 operators (AND, OR, NOT, NEAR, *, ^, quotes) from user input. Pass through raw only if the caller explicitly opts in via a `raw: bool` parameter. Prevents syntax errors from crashing searches.
- **D-06:** Search filters use composable SQL WHERE clauses on JOINed tables. A `SearchFilter` struct carries optional fields: `query`, `source`, `role`, `after` (unix timestamp), `before` (unix timestamp), `limit` (default 20). All filters are optional and compose additively.

### Export & pruning
- **D-07:** Session export uses JSON format. Single session exported as `{ "session": {...}, "messages": [...] }`. Bulk export as an array of the same structure. Matches hermes-agent's export format.
- **D-08:** Pruning is manual only — explicit API call with `older_than_days` threshold and optional `source` filter. No automatic background pruning. Only prunes ended sessions (`ended_at IS NOT NULL`). Returns count of deleted sessions. Cascade deletes associated messages.

### Write contention & WAL
- **D-09:** SQLite `busy_timeout` set to 5000ms on connection open. If still SQLITE_BUSY after timeout, application-level retry with random jitter (50-200ms) for up to 3 attempts. WAL mode (already configured) enables concurrent reads.
- **D-10:** Periodic WAL checkpoints via `PRAGMA wal_checkpoint(PASSIVE)` every 5 minutes from a tokio background timer (using `spawn_blocking` for the sync rusqlite call). PASSIVE mode doesn't block writers. Keeps WAL file from growing unbounded.
- **D-11:** CLI and gateway use separate `Connection` instances to the same `state.db` file. No shared connection or IPC needed — WAL mode handles concurrent access safely (multiple concurrent readers, one writer at a time with busy_timeout).

### Claude's Discretion
- Schema migration strategy for new columns/indexes needed by this phase (e.g., snippet support, filter indexes)
- Internal details of the retry wrapper (exact backoff curve, error classification)
- How `spawn_blocking` bridges sync StateStore calls into async gateway/CLI code
- Whether to add indexes for common filter queries (source+timestamp, role+timestamp)
- FTS5 snippet parameters (prefix/suffix markers, fragment size)
- How session lineage recording integrates with future context compression (Phase 18)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Session storage requirements
- `.planning/REQUIREMENTS.md` — SESS-01 through SESS-11 (all 11 session storage requirements are in scope)

### Existing implementation (primary code references)
- `crates/ironhermes-state/src/lib.rs` — Current StateStore: SQLite+WAL, sessions/messages tables, FTS5 triggers, schema migrations v1-v6, session titles, parent_session_id, basic search
- `crates/ironhermes-gateway/src/session.rs` — Current in-memory SessionStore: HashMap-based GatewaySession with SessionKey, no SQLite integration

### Integration points
- `crates/ironhermes-gateway/src/runner.rs` — Gateway runner where StateStore and SessionStore need to be wired together
- `crates/ironhermes-gateway/src/handler.rs` — Gateway message handler that creates/uses sessions
- `crates/ironhermes-cli/src/main.rs` — CLI entry point that needs StateStore integration for session persistence

### Architecture
- `.planning/codebase/ARCH.md` — Crate dependency graph, concurrency model
- `.planning/ROADMAP.md` — Phase 13 success criteria, downstream dependencies (Phase 15, 17, 20 depend on 13)

### Prior phase context
- `.planning/phases/11-memory-provider-trait/11-CONTEXT.md` — Established async_trait + Send + Sync pattern, spawn_blocking bridge, config-driven selection with hard error
- `.planning/phases/12-provider-resolution/12-CONTEXT.md` — Established resolver-builds-client pattern, enum dispatch over trait objects

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `StateStore` (`ironhermes-state/src/lib.rs`): Already has create_session, add_message, get_session, get_messages, list_sessions, search_messages, update_session_title, update_session_stats, end_session, and schema migrations. Substantial foundation — this phase extends rather than replaces.
- `SessionStore` + `GatewaySession` (`ironhermes-gateway/src/session.rs`): In-memory HashMap with SessionKey routing, expiry, message management. Becomes the cache layer wrapping StateStore.
- `SearchResult` struct already exists with message_id, session_id, role, content, timestamp, session_source, session_title — needs extension for snippets and context.

### Established Patterns
- `spawn_blocking` for sync-in-async (used with rusqlite throughout)
- `Arc<Mutex<>>` sharing for stateful resources (MemoryStore, Tool registry)
- Config-driven selection with hard error on misconfiguration (Phase 11)
- WAL mode already set via `PRAGMA journal_mode=WAL` in schema SQL

### Integration Points
- Gateway `runner.rs` constructs SessionStore — needs to also construct/inject StateStore
- Gateway `handler.rs` calls `session_store.get_or_create()` — needs write-through to StateStore
- CLI `main.rs` — needs StateStore construction and session creation/persistence
- `Cargo.toml` workspace — `ironhermes-state` already a dependency of gateway; CLI needs it too

</code_context>

<specifics>
## Specific Ideas

- hermes-agent's approach to session recovery: all sessions stored but NOT auto-loaded into context on restart. Manual retrieval via session_search tool queries SQLite and summarizes relevant past discussions. This is explicitly the model to follow.
- Export format matches hermes-agent: JSON with session metadata + messages array.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

### Reviewed Todos (not folded)
- "Add setup wizard and config scaffolding for gateway testing" — belongs in Phase 23 (Configuration & Setup Wizard). Already reviewed and deferred in Phase 12.

</deferred>

---

*Phase: 13-session-storage*
*Context gathered: 2026-04-11*
