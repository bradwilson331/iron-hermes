# Pitfalls Research

**Domain:** Rust AI agent — adding persistent memory, session storage (SQLite/FTS5), context compression, prompt caching, context files, SOUL.md, skill framework, and memory provider backends (SQLite, Grafeo, DuckDB) to an existing async Rust system
**Researched:** 2026-04-11
**Confidence:** HIGH (grounded in IronHermes codebase analysis, official Anthropic docs, duckdb-rs issues, hermes-agent architecture docs, FTS5 SQLite forum threads)

---

## Critical Pitfalls

Mistakes that cause rewrites, API rejections, data corruption, or security breaches.

---

### Pitfall 1: Orphaned tool_result Blocks After Context Compression

**What goes wrong:** The context compressor drops an assistant message that contains a `tool_use` block, but the subsequent user message containing the paired `tool_result` block remains. The Anthropic API validates the full message sequence on every call and returns HTTP 400 ("unexpected tool_use_id in tool_result") when a `tool_result` references a `tool_use` ID that no longer exists earlier in the history. The agent session is permanently broken — the error repeats on every subsequent turn.

**Why it happens:** The existing `ContextCompressor::drop_middle_messages` in `ironhermes-agent/src/context_compressor.rs` operates on individual message indices without treating `(assistant tool_use) + (user tool_result)` as an atomic unit. Any slice boundary that falls between the two halves of a tool pair produces orphaned `tool_result` blocks. This is the most frequently reported compaction bug across Claude Code and other Anthropic API consumers in 2025-2026 (confirmed in anthropics/claude-code#8484, anthropics/claude-code#14173, openclaw issues#29103, #3462, #40305).

**How to avoid:**
- Before dropping any slice of messages, scan both the candidate drop region AND the messages immediately after it for `tool_result` blocks whose `tool_call_id` matches a `tool_use` in the drop region.
- Drop the `tool_use` message AND its corresponding `tool_result` message atomically, or keep both.
- Never split across a tool pair boundary. When selecting `tail_start`, advance it forward past any `tool_result` at `messages[tail_start]` whose paired `tool_use` was in the dropped range.
- Add a pre-flight validation function that walks the final message list and asserts every `tool_result.tool_call_id` has a matching `tool_use` earlier in the list. Call it after every compression pass in debug builds.

**Warning signs:** HTTP 400 responses from the LLM API mid-session. "tool_use_id" appearing in error messages. Sessions that survive one turn after compression then fail permanently on the next.

**Phase to address:** Context Compression phase. This is the first design constraint for `ContextCompressor` — write the atomicity invariant before implementing the new dual-system (gateway hygiene at 85%, agent ContextEngine at 50%).

---

### Pitfall 2: System Prompt Mutation Destroys Cache Stability

**What goes wrong:** Prompt caching on Anthropic's API works by matching a hash of the exact prefix up to a `cache_control` breakpoint. If the system prompt changes between turns — even by a single character — every request is a cache miss. The project pays full input token costs on every message, negating the purpose of caching entirely.

**Why it happens:** The current `PromptBuilder::build()` in `ironhermes-agent/src/prompt_builder.rs` is called per-session (frozen at `load_context` time), which is correct. The danger arises in v2.0 when adding the 10-layer prompt including timestamps, memory snapshots, skills catalog, and platform hints. Developers commonly inject a `Current time: {timestamp}` line or include per-user dynamic data inside the cached section. Any of these invalidates the cache on every request. Confirmed Anthropic doc pitfall: cache breakpoints must be placed on the last block whose content is identical across all requests.

**How to avoid:**
- The system prompt must be structured as: `[static layers] → cache_control breakpoint → [dynamic layers]`. Static layers: SOUL.md identity, tool-use guidance, AGENTS.md, skill catalog, MEMORY.md/USER.md snapshot (these are frozen at session start). Dynamic layers after the breakpoint: current timestamp, session-specific context, per-turn injections.
- Memory snapshots are frozen at `load_from_disk()` time (the `MemoryStore::snapshot` field already does this — preserve that invariant through v2). Since they do not change mid-session, they are safe to include before the breakpoint.
- Do NOT add a `cache_control` breakpoint on any block that contains a timestamp, user ID, or per-turn context.
- Tool definitions change the entire cache hierarchy (tools → system → messages). If the toolset changes between session setup and mid-session (e.g. skill activation adds tools), the tools-level cache is busted. Accept this cost at skill activation time; do not mutate tools silently per-turn.
- Verify cache hits by checking `cache_read_input_tokens > 0` in the API response. A zero value means caching is silently failing.

**Warning signs:** `cache_read_input_tokens` always 0 in API responses. Token costs not decreasing after first turn in a session. Timestamps, user IDs, or dynamic content appearing before a `cache_control` marker.

**Phase to address:** Prompt Caching + Prompt Assembly phases, which must be designed together. The 10-layer assembly order decision must account for cache stability from the start.

---

### Pitfall 3: Gateway In-Memory SessionStore Not Migrated — Dual State Split

**What goes wrong:** The gateway keeps `GatewaySession` (with its `Vec<ChatMessage>`) in the in-memory `SessionStore` in `ironhermes-gateway/src/session.rs`. The new `StateStore` in `ironhermes-state/src/lib.rs` keeps a separate SQLite-backed session record. If v2.0 adds persistent session storage without migrating the gateway's in-memory store, there are two authoritative sources for conversation state: the gateway's `HashMap` and the SQLite `messages` table. After a restart, the gateway loses all in-memory sessions but the SQLite store has the history — and nothing reconciles them. The `session_search` tool will return results from SQLite sessions that the gateway has no matching in-memory state for.

**Why it happens:** Both stores exist in the codebase right now and serve different purposes. The migration step is easy to defer because everything works — until a restart, or until session_search returns a session ID the gateway doesn't know about.

**How to avoid:**
- Define a migration contract: on startup, the gateway loads all non-expired SQLite sessions and reconstructs in-memory state (or, preferably, makes the in-memory store a write-through cache backed by SQLite). The `StateStore` becomes the source of truth; `SessionStore` becomes a performance layer.
- Add a `session_id` field to `GatewaySession` that matches the `StateStore` UUID — this field already exists (`session_id: String`) but must be written to SQLite on creation, not lazily.
- Every `add_message` call to `GatewaySession` must also call `StateStore::add_message`. Use a wrapper type that enforces this invariant rather than two separate calls at each call site.

**Warning signs:** Sessions that exist in `session_search` results but the gateway has no memory of. Empty message history after agent restart. `GatewaySession::session_id` UUIDs that do not appear in `SELECT id FROM sessions`.

**Phase to address:** Session Storage phase. The migration architecture must be specified before implementing the StateStore integration into the gateway.

---

### Pitfall 4: FTS5 Trigger Drift — Index Becomes Stale or Corrupt

**What goes wrong:** The `messages_fts` content table is maintained by three triggers (INSERT, UPDATE, DELETE on `messages`). If any write path bypasses these triggers — bulk imports, direct SQLite writes during migration, `REPLACE INTO` without `PRAGMA recursive_triggers=ON`, or schema migrations that backfill rows — the FTS5 index becomes out of sync with the `messages` table. Queries return wrong results or no results. Running `INSERT INTO messages_fts(messages_fts) VALUES('integrity-check')` then reports corruption (confirmed SQLite forum thread on FTS5 extra _fts_docsize rows).

**Why it happens:** The triggers defined in `FTS_SQL` in `ironhermes-state/src/lib.rs` only fire for DML through the normal SQLite path. Any code that:
- Runs `INSERT OR REPLACE` (which fires DELETE then INSERT, but only if `PRAGMA recursive_triggers=1`)
- Imports rows directly via `rusqlite::Connection::execute_batch` during migration
- Rebuilds the table from the Python hermes-agent's state.db import
- Uses `UPDATE OR REPLACE` semantics

will silently break FTS5 consistency.

**How to avoid:**
- Never INSERT rows into `messages` outside the `StateStore::add_message` method. All migration code must go through the same method.
- After any bulk migration, run `INSERT INTO messages_fts(messages_fts) VALUES('rebuild')` to rebuild the index from the content table.
- Add an integrity check to the `StateStore` test suite: after every insert, verify `SELECT COUNT(*) FROM messages` equals `SELECT COUNT(*) FROM messages_fts`.
- Set `PRAGMA recursive_triggers=ON` at connection open time as a precaution.
- The `FTS_SQL` block checks for FTS table existence before creating it (`fts_exists` check) — preserve this pattern in schema migrations.

**Warning signs:** `session_search` returning 0 results for text that is visibly present in `get_messages` output. SQLite integrity-check returning "database disk image is malformed" for `messages_fts`.

**Phase to address:** Session Storage phase (schema migration design) and any phase that bulk-imports data from Python hermes-agent history.

---

### Pitfall 5: DuckDB Connection Non-Send + Tokio Runtime Blocking

**What goes wrong:** `duckdb::Connection` wraps internal state in a `RefCell`, which is `!Send`. It cannot be stored in an `Arc`, shared across tokio tasks, or moved into `spawn_blocking` closures without unsafe code. If a developer naively stores a `Connection` in a shared `MemoryProvider` struct and passes that struct across `.await` boundaries or into tokio tasks, compilation fails or (worse) the unsafe workaround is reached for and causes undefined behavior. Additionally, DuckDB's C++ engine performs synchronous disk I/O — calling it directly in an async context blocks the tokio executor thread, degrading latency for all concurrent Telegram users.

**Why it happens:** DuckDB was designed for analytical workloads, not embedded use in async OLTP-style applications. The Rust binding exposes the synchronous C API directly. The `!Send` constraint surfaces immediately in practice.

**How to avoid:**
- Use `async-duckdb` (crates.io) as the adapter layer. It wraps `duckdb::Connection` on a dedicated background thread and exposes async methods safe for tokio, eliminating both the `!Send` problem and the blocking executor problem.
- Alternatively, follow the channel-based pattern: one background thread owns the single DuckDB connection; all memory provider operations are sent via `mpsc` channel and results returned via oneshot channel. This is essentially what `async-duckdb` implements.
- The `MemoryProvider` trait must be `Send + Sync + 'static` for use in Arc. Design the DuckDB backend to implement this via an internal channel handle, not by exposing `Connection` directly.
- DuckDB is column-oriented and optimized for analytical scans. Single-row writes (inserting one memory fact at a time) are slower than SQLite for this pattern. Batch writes when possible. For the memory provider workload (read at session start, write occasionally), the latency difference is acceptable, but do not use DuckDB as a substitute for the message-per-message `StateStore` role.

**Warning signs:** Compile errors: "dyn MemoryProvider cannot be shared between threads safely" or "the trait `Send` is not implemented for `duckdb::Connection`". Tokio task spawn failures. Executor thread stalls under concurrent user load.

**Phase to address:** Memory Provider Trait phase. The trait signature (including `Send + Sync` bounds) must be locked in before implementing any backend.

---

### Pitfall 6: Grafeo GDB Not a Drop-in Replacement for SQL Patterns

**What goes wrong:** Grafeo is a graph database with a string-based query API (`db.execute("INSERT (:Person {name: 'X'})")`) that returns `serde_json::Value`. There are no typed result structs, no prepared statements equivalent to rusqlite's parameter binding, and no row-mapping closures. Developers who model the Grafeo backend after the SQLite `MemoryStore` patterns (file-locked writes, delimiter-separated entries, row queries) will find none of those primitives apply. Memory facts stored as nodes with edges require a graph query to retrieve, not a `SELECT * FROM` pattern.

**Why it happens:** Grafeo is a young crate (MSRV 1.91.1, early 2026 active development). Its Rust API surface is minimal — essentially `GrafeoDB::new_in_memory()` / `GrafeoDB::open(path)` and `db.execute(query_string) -> Result<Value>`. There is no explicit async interface; all operations are synchronous. The `Send + Sync` status of `GrafeoDB` is not explicitly documented and must be verified before crossing async boundaries.

**How to avoid:**
- The `MemoryProvider` trait's Grafeo backend must own a `GrafeoDB` instance wrapped in `Arc<Mutex<GrafeoDB>>` with all calls dispatched through `tokio::task::spawn_blocking` — same bridge pattern as rusqlite.
- Model memory facts as nodes with a `content` property: `INSERT (:Memory {content: '...', target: 'memory', ts: ...})`. Retrieval uses `MATCH (m:Memory {target: 'memory'}) RETURN m`. Test this query shape against the actual Grafeo version in use — query syntax compatibility between GQL/Cypher varies by minor version.
- Do not rely on Grafeo's string-based query API for injection safety. Interpolating user-supplied content directly into query strings is SQL injection at the graph layer. All user-facing content must be passed as property map parameters, not string-interpolated into the query body. Verify the Grafeo API exposes parameterized queries before trusting it for user content.
- The Grafeo backend is strictly for the `MemoryProvider` trait, not for `StateStore`. Session message history stays in SQLite.

**Warning signs:** Compilation errors about `GrafeoDB` not being `Send`. Runtime panics on `execute()` with graph query strings that contain unescaped content. Tests that pass in memory mode but fail in persistent mode (different storage path behavior).

**Phase to address:** Memory Provider Trait phase, Grafeo backend sub-phase. Spike a working `GrafeoDB::open` + round-trip query before committing to the Grafeo backend in the trait design.

---

### Pitfall 7: SOUL.md / Context File Injection Not Scanned at Load Time

**What goes wrong:** SOUL.md defines the agent's identity and is injected directly into the system prompt. If a file fetched from the internet (a shared skill, a downloaded SOUL.md template, or a maliciously modified project context file) contains prompt injection payloads, they execute with system-prompt-level authority. The "soul-evil" attack class (documented February 2026, 400+ malicious packages found on skill registries) specifically targets this vector: an attacker replaces or modifies SOUL.md to redirect agent behavior — exfiltrating env vars, leaking API keys via URL parameters, or changing persona entirely.

**Why it happens:** The existing `PromptBuilder::load_soul_md()` already calls `scan_context_content()` before inclusion (confirmed in codebase). The risk increases in v2.0 because the skill framework adds a new attack surface: SKILL.md files are loaded from directories, potentially downloaded from Skills Hub or installed by users. If skill bodies are injected into the system prompt without scanning (for conditional activation display or catalog), they become injection vectors.

**How to avoid:**
- Every file loaded into the system prompt — SOUL.md, AGENTS.md, .hermes.md, CLAUDE.md, .cursorrules, SKILL.md body, USER.md, MEMORY.md — must pass through `scan_context_content()` before inclusion. This is non-negotiable.
- For SKILL.md files specifically: scan both frontmatter fields (name, description, compatibility) and the Markdown body. The description field is used as the LLM's skill activation trigger — it is a privileged injection point if not scanned.
- Skills loaded from the Skills Hub or any external source should be treated as untrusted until scanned. Present a visual warning to the user for externally sourced skills.
- The `context_scanner` must handle Unicode homoglyphs, right-to-left override characters, and multi-language injection patterns (Korean, Chinese, Japanese injection has been confirmed in wild samples).

**Warning signs:** SOUL.md or SKILL.md files that include phrases like "ignore previous instructions", "you are now", "your new primary directive", "export $ANTHROPIC_API_KEY". Skills from external sources that activate under unexpected conditions.

**Phase to address:** Skill Framework phase (SKILL.md loading) and SOUL.md personality phase. Security scanning must be in place before external skill installation is supported.

---

### Pitfall 8: Blocking StateStore Calls Inside Async Gateway Handlers

**What goes wrong:** `StateStore` wraps `rusqlite::Connection`, which is synchronous. Every call to `add_message`, `create_session`, `search_messages` performs blocking I/O. If these are called directly inside tokio async handlers (e.g., in `ironhermes-gateway/src/handler.rs`), they block the tokio executor thread for the duration of the SQLite write. Under concurrent Telegram users, this serializes all message handling through a single blocked thread, causing latency spikes and potential handler timeouts.

**Why it happens:** `rusqlite` provides no async API. The pattern of calling sync code in async context is trivially easy to write and works correctly in single-user tests. The problem only manifests under concurrency.

**How to avoid:**
- All `StateStore` calls from async code must go through `tokio::task::spawn_blocking`. The `StateStore` itself should be wrapped in a struct that provides async methods delegating to `spawn_blocking`.
- Alternatively, adopt `tokio-rusqlite` which wraps the connection on a dedicated background thread with an async call interface.
- The `StateStore` instance is not `Clone` (it holds a `Connection`). Wrap it in `Arc<Mutex<StateStore>>` for sharing across gateway sessions, but Mutex contention under load is a secondary concern — `spawn_blocking` eliminates the executor-blocking problem first.
- WAL mode (already set in `SCHEMA_SQL`) allows concurrent readers with one writer. Still, writes from multiple concurrent users will queue at the `Mutex` level — this is acceptable for v2.0 single-operator deployment.

**Warning signs:** Telegram response latency increasing proportionally with concurrent user count. Tokio runtime warnings about "blocking in async context". Profiling showing gateway handler tasks spending most time inside SQLite calls.

**Phase to address:** Session Storage phase, specifically the gateway integration design.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Single `Arc<Mutex<StateStore>>` for all users | Simple to implement | Mutex contention under multi-user load; all writes serialized | Acceptable for v2.0 single-operator deployment; revisit if multi-user |
| MemoryStore snapshot stays frozen for entire session | Correct cache stability | Memory updates mid-session not visible until next session | Never change — this is the documented D-12 invariant; mid-session mutations defeat caching |
| Grafeo backend using `Arc<Mutex<GrafeoDB>>` with `spawn_blocking` | Works correctly | Per-query thread context switch overhead | Acceptable; Grafeo is not high-frequency |
| DuckDB backend using `async-duckdb` single connection pool | Simplest async-safe approach | Analytical queries on large datasets will serialize | Acceptable for v2.0; DuckDB memory provider is for structured retrieval not bulk analytics |
| Skip `session_search` integration in CLI for v2 | Faster CLI parity | CLI users cannot search session history | Acceptable as deferred feature if CLI parity scope is constrained |
| FTS5 integrity check only in test builds | No runtime overhead | Silent FTS drift undetectable in production | Never — add a startup integrity check at minimum |

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Anthropic cache_control | Placing breakpoint on last system block which includes memory snapshot | Memory snapshot is frozen at session start — safe to cache. Timestamp/user-id injections must go after the breakpoint |
| Anthropic cache_control | Not checking `cache_read_input_tokens` in response | Verify cache hits by asserting `cache_read_input_tokens > 0` after first turn; silent miss = paying full price |
| Anthropic cache_control | Using more than 4 breakpoints | Hard API limit; if using automatic caching + explicit breakpoints, auto-caching consumes one slot — stay at 3 explicit |
| duckdb-rs | Storing `Connection` in shared struct across async boundary | `Connection` is `!Send`; use `async-duckdb` or channel-based single-thread pattern |
| duckdb-rs `try_clone()` | Creating cloned connections that each try to lock the file | `try_clone()` creates independent connections to the same DB; safe for multi-threaded read but watch write contention |
| Grafeo `execute()` | String-interpolating user content into query strings | Graph query injection is real; use parameterized property maps, not string concat |
| Grafeo persistent mode | Opening same file from multiple processes | ACID transactions guarantee consistency within a process; cross-process behavior depends on Grafeo's lock model (not fully documented) — keep to single process |
| FTS5 content table | Calling `VACUUM` without rebuilding FTS index after bulk deletes | `VACUUM` on an FTS5 content table with external content can corrupt the index; run `INSERT INTO messages_fts(messages_fts) VALUES('rebuild')` after any bulk operation |
| rusqlite + tokio | Calling `StateStore` methods directly in async functions | Blocks executor thread; wrap in `spawn_blocking` or use `tokio-rusqlite` |
| MemoryProvider trait | Defining `async fn` methods on trait for `dyn` dispatch | `async fn` in traits is not object-safe without `Box<dyn Future>` return or the `async-trait` macro; use `async-trait` crate for v2.0 |

---

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Per-message StateStore writes blocking async handler | Telegram response latency spikes under 2+ concurrent users | `spawn_blocking` wrapper for all StateStore calls | At 2+ concurrent users |
| FTS5 `MATCH` query on unoptimized index | `search_messages` queries taking >100ms | Run `INSERT INTO messages_fts(messages_fts) VALUES('optimize')` periodically; WAL checkpoint every 50 writes (match hermes-agent pattern) | After ~10,000 messages |
| Grafeo `execute()` full graph scan | Memory retrieval latency increases with fact count | Index nodes with labels; use `MATCH (m:Memory {target: '...'}) RETURN m` with label pushdown, not open graph scans | After ~1,000 facts |
| ContextCompressor triggering too frequently | LLM called for every user message to re-summarize | Compression thresholds (85%/50%) are percentages of context window — ensure `context_length` passed to constructors matches the actual model's context window, not a default | Immediately if context_length is wrong |
| DuckDB per-row writes via `async-duckdb` | Memory provider writes slower than SQLite | Batch memory writes; DuckDB shines on bulk analytics, not singleton inserts | At >10 facts/second write rate |

---

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Including user-supplied content in Grafeo/DuckDB query strings | Graph/SQL injection; agent executing attacker-controlled queries | Parameterize all queries; never string-interpolate content from memory facts, user messages, or context files |
| SKILL.md body injected into system prompt without scanning | System-prompt-level prompt injection from malicious skill | Scan all SKILL.md content with `scan_context_content` before injection; skills from external sources flagged as untrusted |
| SOUL.md loaded from user-specified path without validation | Remote file inclusion; persona hijack | Only load SOUL.md from `HERMES_HOME`; if path is user-configurable, SSRF-validate the path and scan the content |
| Session lineage (parent_session_id) accepting user-supplied values | Session confusion; exfiltration of another user's session history via fabricated lineage | Generate `parent_session_id` server-side only; never accept from tool call arguments |
| Memory snapshot not frozen — live MemoryStore used in prompt | Agent-injected memory visible immediately in same turn | The `snapshot` field in `MemoryStore` is frozen at `load_from_disk()` time; `format_for_system_prompt` returns the frozen snapshot, not live entries. This invariant must be preserved in all v2.0 memory provider backends |

---

## "Looks Done But Isn't" Checklist

- [ ] **Context compression:** Tool call/result pairs validated as atomic units before any drop — verify with a test that produces an assistant message with `tool_calls` followed immediately by a `tool_result` user message, then compresses, and confirms both are either kept or both dropped
- [ ] **Prompt caching:** `cache_read_input_tokens` is non-zero on the second turn of a session with a static system prompt — if it is 0, the cache is silently not working
- [ ] **FTS5 sync:** After 100 messages inserted via `StateStore::add_message`, `SELECT COUNT(*) FROM messages_fts` equals `SELECT COUNT(*) FROM messages` — verify in integration test
- [ ] **StateStore async bridge:** A load test with 5 simulated concurrent Telegram users shows no executor thread stalls — verify tokio runtime stays responsive throughout
- [ ] **SOUL.md scanning:** A SOUL.md file containing "ignore previous instructions" is blocked before inclusion in system prompt — verify `scan_context_content` returns a `[BLOCKED:...]` marker
- [ ] **MemoryProvider trait object safety:** `Box<dyn MemoryProvider>` compiles and can be stored in an Arc — verify at compile time with a test instantiation of each backend
- [ ] **Grafeo backend:** A round-trip test (write fact → query all facts → verify content) passes against a persistent (not in-memory) Grafeo database — in-memory tests mask file-open errors
- [ ] **DuckDB backend:** The `async-duckdb` connection is initialized from a `spawn_blocking`-safe context and all calls from async code compile without `unsafe` — verify no `RefCell`/`!Send` workarounds

---

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Orphaned tool_result breaks session | MEDIUM | Delete the orphaned `tool_result` message from the SQLite `messages` table for that session; run `INSERT INTO messages_fts(messages_fts) VALUES('rebuild')` to resync FTS; restart session |
| FTS5 index corrupt | LOW | `INSERT INTO messages_fts(messages_fts) VALUES('rebuild')` rebuilds from content table; no data loss, only downtime |
| Cache_control on mutable block — all turns pay full price | LOW | Move `cache_control` to stable block, restart session; costs stop accumulating |
| DuckDB Connection !Send compilation failure | MEDIUM | Add `async-duckdb` dependency, refactor backend to use `Client` instead of raw `Connection`; interface remains same |
| GatewaySession / StateStore split state after restart | HIGH | Implement write-through: `GatewaySession::add_message` writes to both in-memory Vec and StateStore atomically; on startup, load active sessions from SQLite into memory |
| Grafeo query returning wrong results after content injection | HIGH | Switch to parameterized property maps; audit all existing Grafeo query strings for interpolated content; rebuild Grafeo database from MemoryStore MEMORY.md source of truth |

---

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| Orphaned tool_result after compression | Context Compression | Test: compress a conversation containing tool pairs; assert no orphaned tool_results in result |
| System prompt mutation breaks cache | Prompt Caching + Prompt Assembly | Test: two consecutive API calls with same static system prompt; assert `cache_read_input_tokens > 0` on second call |
| Gateway/StateStore dual state split | Session Storage | Test: restart gateway mid-session; assert messages recoverable from SQLite and session resumes correctly |
| FTS5 trigger drift | Session Storage (schema) | Test: insert 10 messages, assert FTS count matches messages count; bulk migrate, assert FTS rebuild works |
| DuckDB !Send in async context | Memory Provider Trait | Compile-time: `Box<dyn MemoryProvider>: Send + Sync` assertion in test module |
| Grafeo non-parameterized queries | Memory Provider (Grafeo backend) | Security test: insert fact with injection payload; assert query string is not interpolated |
| SOUL.md / SKILL.md injection | Skill Framework + SOUL.md phase | Test: load SKILL.md with "ignore previous instructions" in body; assert `[BLOCKED:]` in scan result before prompt inclusion |
| Blocking StateStore in async handlers | Session Storage (gateway integration) | Load test: 5 concurrent users; assert tokio executor thread not blocked; verify with `tokio-console` |

---

## Sources

- Anthropic Prompt Caching documentation (official): https://platform.claude.com/docs/en/build-with-claude/prompt-caching
- anthropics/claude-code#8484, #14173 — Compaction corrupts tool_use/tool_result pairs (confirmed 2025-2026)
- openclaw issues #29103, #3462, #40305 — Orphaned tool_result after compaction (confirmed 2025-2026)
- duckdb/duckdb-rs#378 — Multiple connections and thread safety limitations
- async-duckdb crate: https://docs.rs/async-duckdb
- tokio-rusqlite crate: https://docs.rs/tokio-rusqlite
- SQLite FTS5 forum: trigger sync issues, REPLACE and recursive_triggers, _fts_docsize orphan rows: https://sqlite.org/forum/info/da59bf102d7a7951740bd01c4942b1119512a86bfa1b11d4f762056c8eb7fc4e
- FTS5 external content + VACUUM interaction: https://sqlite.work/optimizing-fts5-external-content-tables-and-vacuum-interactions/
- Grafeo GitHub: https://github.com/GrafeoDB/grafeo
- Grafeo DEV.to overview: https://dev.to/alanwest/grafeo-an-embeddable-graph-database-in-rust-that-actually-makes-sense-1nik
- hermes-agent session storage architecture: https://hermes-agent.nousresearch.com/docs/developer-guide/session-storage
- hermes-agent architecture (single-select plugin pattern): https://hermes-agent.nousresearch.com/docs/developer-guide/architecture
- Soul-Evil attack class and SOUL.md as attack surface: https://dev.to/tomleelive/the-soul-evil-attack-how-malicious-personas-hijack-ai-agents-and-how-to-stop-them-48ae
- OWASP Agentic Skills Top 10: https://owasp.org/www-project-agentic-skills-top-10/
- SKILL.md to shell access threat modeling: https://snyk.io/articles/skill-md-shell-access/
- IronHermes codebase: `crates/ironhermes-state/src/lib.rs`, `crates/ironhermes-gateway/src/session.rs`, `crates/ironhermes-agent/src/context_compressor.rs`, `crates/ironhermes-agent/src/prompt_builder.rs`, `crates/ironhermes-core/src/memory_store.rs`

---

*Pitfalls research for: IronHermes v2.0 Intelligence & Identity milestone*
*Researched: 2026-04-11*
