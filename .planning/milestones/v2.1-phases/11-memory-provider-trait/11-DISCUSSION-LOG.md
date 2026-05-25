# Phase 11: Memory Provider Trait - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-11
**Phase:** 11-memory-provider-trait
**Areas discussed:** Trait async model, Hook data flow, Provider selection, Error semantics

---

## Trait async model

| Option | Description | Selected |
|--------|-------------|----------|
| Async trait (Recommended) | All 5 lifecycle hooks are async fn. File-based MemoryStore returns immediately. Future network providers work naturally. Uses async_trait. | ✓ |
| Sync trait | All hooks are plain fn. Network-backed providers must use block_on() or spawn_blocking() internally. | |
| You decide | Claude picks the approach. | |

**User's choice:** Async trait (Recommended)
**Notes:** None — straightforward selection.

---

## Hook data flow

### initialize() signature

| Option | Description | Selected |
|--------|-------------|----------|
| Config only (Recommended) | initialize(&mut self, config: &MemoryProviderConfig) — typed config struct with provider settings, memory dir, char limits. | ✓ |
| Config + memory dir | initialize(&mut self, memory_dir: &Path, config: &Value) — explicit path plus raw YAML. | |
| You decide | Claude designs the signature. | |

**User's choice:** Config only (Recommended)
**Notes:** None.

### prefetch() and sync_turn() signatures

| Option | Description | Selected |
|--------|-------------|----------|
| Session ID + entries (Recommended) | prefetch(session_id) returns MemoryEntries. sync_turn(session_id, entries) receives current state. | ✓ |
| Minimal — session ID only | Both hooks just get session_id. Provider manages its own internal state. | |
| You decide | Claude designs signatures. | |

**User's choice:** Session ID + entries (Recommended)
**Notes:** None.

### on_session_end() signature

| Option | Description | Selected |
|--------|-------------|----------|
| Session ID + final entries (Recommended) | on_session_end(session_id, entries) — provider gets final state to persist/flush. | ✓ |
| Session ID only | Provider handles its own cleanup from tracked state. | |
| You decide | Claude picks. | |

**User's choice:** Session ID + final entries (Recommended)
**Notes:** None.

---

## Provider selection

### Config mechanism

| Option | Description | Selected |
|--------|-------------|----------|
| Named provider key (Recommended) | memory.provider: "file" / "sqlite" / "grafeo" / "duckdb" in config.yaml. Provider-specific settings under memory.<name>. | ✓ |
| Feature-flag only | Provider selected at compile time via Cargo features. Requires recompilation to switch. | |
| You decide | Claude picks. | |

**User's choice:** Named provider key (Recommended)
**Notes:** None.

### Missing provider behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Error at startup (Recommended) | Hard error with clear message listing available providers and required feature flag. No silent fallback. | ✓ |
| Warn and fall back to file | Log warning, silently use file provider. | |
| You decide | Claude picks. | |

**User's choice:** Error at startup (Recommended)
**Notes:** None.

---

## Error semantics

| Option | Description | Selected |
|--------|-------------|----------|
| Log and continue (Recommended) | initialize/shutdown fatal; prefetch/sync_turn/on_session_end log warnings and continue. Frozen-snapshot keeps session usable. | ✓ |
| All errors fatal | Any hook failure terminates the session. | |
| Configurable per-hook | Config specifies which hooks are fatal vs warn-and-continue. | |
| You decide | Claude picks. | |

**User's choice:** Log and continue (Recommended)
**Notes:** None.

---

## Claude's Discretion

- Crate placement for the trait
- MemoryProviderConfig struct design
- MemoryEntries wrapper type design
- MemoryStore refactoring approach
- async_trait vs native async traits (MSRV)
- Provider factory/registry pattern

## Deferred Ideas

None — discussion stayed within phase scope.
