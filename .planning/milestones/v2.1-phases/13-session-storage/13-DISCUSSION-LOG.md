# Phase 13: Session Storage - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-11
**Phase:** 13-session-storage
**Areas discussed:** Write-through cache design, Search & filtering, Export & pruning, Write contention & WAL

---

## Write-through cache design

| Option | Description | Selected |
|--------|-------------|----------|
| Thin cache wrapper | SessionStore wraps StateStore — every write goes to SQLite immediately, keeps recent messages in memory for fast access | ✓ |
| Async write-behind | Buffer writes in memory, flush to SQLite periodically or on session end | |
| SQLite-only, no cache | Remove in-memory SessionStore entirely, all reads/writes through StateStore | |
| You decide | Claude has discretion | |

**User's choice:** Thin cache wrapper
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Active (not ended) only | Only hydrate sessions where ended_at IS NULL | |
| Recent N sessions | Load last N sessions regardless of end status | |
| None — cold start | Don't hydrate on restart | |

**User's choice:** No auto-recovery — matches hermes-agent approach. All sessions persist to SQLite but retrieval is manual via session_search tool. In-memory cache is only for current running session.
**Notes:** User provided detailed description of hermes-agent's approach: sessions stored but not auto-loaded into LLM context to keep prompt manageable and save tokens.

---

| Option | Description | Selected |
|--------|-------------|----------|
| All sources share state.db | CLI, gateway, cron, ACP all write to same SQLite database | ✓ |
| Gateway-only persistence | Only gateway sessions persist, CLI ephemeral | |
| Separate databases per source | Each source gets own state.db | |

**User's choice:** All sources share state.db
**Notes:** None

---

## Search & filtering

| Option | Description | Selected |
|--------|-------------|----------|
| Snippets with markers | FTS5 snippet() with <<match>> markers + 1 message context window | ✓ |
| Full message content | Entire matching message, no truncation | |
| Snippet only, no context | FTS5 snippet output only, no surrounding messages | |
| You decide | Claude has discretion | |

**User's choice:** Snippets with markers
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Strip FTS5 operators | Remove/escape special chars by default, opt-in raw mode | ✓ |
| Always allow FTS5 syntax | Pass queries directly to FTS5 | |
| You decide | Claude has discretion | |

**User's choice:** Strip FTS5 operators
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| SQL WHERE clauses on JOIN | FTS5 MATCH + composable WHERE clauses, SearchFilter struct | ✓ |
| Separate filter methods | Different methods for each filter combination | |
| You decide | Claude has discretion | |

**User's choice:** SQL WHERE clauses on JOIN
**Notes:** None

---

## Export & pruning

| Option | Description | Selected |
|--------|-------------|----------|
| JSON | Single session as JSON object, bulk as JSON array | ✓ |
| JSONL | Each session+messages as single JSON line | |
| You decide | Claude has discretion | |

**User's choice:** JSON
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Manual only | Explicit API call with age threshold + optional source filter | ✓ |
| Auto + manual | Background prune on startup + manual | |
| You decide | Claude has discretion | |

**User's choice:** Manual only
**Notes:** Only prunes ended sessions. Returns count deleted. Cascade deletes messages.

---

## Write contention & WAL

| Option | Description | Selected |
|--------|-------------|----------|
| Busy timeout + app retry | 5000ms busy_timeout, 3 retries with random jitter (50-200ms) | ✓ |
| Single-writer lock | Application-level Mutex for all writes | |
| You decide | Claude has discretion | |

**User's choice:** Busy timeout + app retry
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Periodic background | PRAGMA wal_checkpoint(PASSIVE) every 5 minutes via tokio timer | ✓ |
| On session end only | Checkpoint when session ends | |
| SQLite auto-checkpoint only | Rely on built-in auto-checkpoint | |
| You decide | Claude has discretion | |

**User's choice:** Periodic background
**Notes:** Uses spawn_blocking for sync rusqlite call.

---

| Option | Description | Selected |
|--------|-------------|----------|
| Separate connections, same file | Each process opens own Connection to state.db, WAL handles concurrency | ✓ |
| Shared via Arc<Mutex> | Single connection shared within process | |
| You decide | Claude has discretion | |

**User's choice:** Separate connections, same file
**Notes:** None

---

## Claude's Discretion

- Schema migration strategy for new columns/indexes
- Retry wrapper internals (backoff curve, error classification)
- spawn_blocking bridge details
- Filter query indexes
- FTS5 snippet parameters
- Session lineage recording integration

## Deferred Ideas

None — discussion stayed within phase scope.
