# Phase 17: Memory Tools & External Providers - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md -- this log preserves the alternatives considered.

**Date:** 2026-04-12
**Phase:** 17-memory-tools-external-providers
**Areas discussed:** External provider data model, session_search tool, Feature gates & crate structure, Capacity display & tool UX, Provider data migration, Memory tool agent-loop intercept, Grafeo/DuckDB schema design, Testing strategy

---

## External Provider Data Model

### SQLite Storage Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Mirror file format | Same MEMORY.md/USER.md content model in SQLite rows. One row per entry with target, content, created_at. FTS5 for search. | |
| Structured facts table | Richer schema: id, target, key, value, category, created_at, updated_at. More queryable. | |
| Single document per target | Full text as single row per target. FTS5 on whole document. | |

**User's choice:** Mirror file format -- with per-entry rows and FTS5 search. User provided extensive hermes_state.py documentation as canonical reference.
**Notes:** User emphasized trait-based architecture where MemoryProvider defines operations while each backend implements specific storage strategy.

### Grafeo Integration

| Option | Description | Selected |
|--------|-------------|----------|
| HTTP client to external Grafeo | Separate service, HTTP/gRPC connection. | |
| Embedded Grafeo library | In-process graph DB on disk. No external service. | |
| You decide | Claude determines during research. | |

**User's choice:** Embedded Grafeo library. External HTTP may come later.

### DuckDB Async Bridge

| Option | Description | Selected |
|--------|-------------|----------|
| Dedicated thread | Persistent OS thread owning Connection. Commands via mpsc channel. | |
| spawn_blocking per call | tokio::task::spawn_blocking for each operation. | |
| You decide | Claude determines during research. | |

**User's choice:** Dedicated thread with mpsc channel.

---

## session_search Tool

### Result Format

| Option | Description | Selected |
|--------|-------------|----------|
| Snippets + metadata | FTS5 snippets with >>>match<<< markers, 1 message context, metadata. | |
| Full message content | Complete message content, no truncation. | |
| Summary only | Session titles and match counts only. | |

**User's choice:** Snippets + metadata matching hermes-agent pattern. User provided full technical specification including tool schema JSON, SQL query pattern, sanitization implementation in Rust, and agent loop intercept pattern.

---

## Feature Gates & Crate Structure

### Provider Crate Location

| Option | Description | Selected |
|--------|-------------|----------|
| In ironhermes-core | Feature-gated modules in core crate. | |
| New ironhermes-memory crate | Dedicated crate for all providers. | |
| You decide | Claude determines during planning. | |

**User's choice:** Per-provider crates under `providers/` directory (user's own suggestion via "Other"). Detailed layout: `providers/memory-sqlite/`, `providers/memory-duckdb/`, `providers/memory-grafeo/` as separate workspace members.

### Provider Compilation Selection

| Option | Description | Selected |
|--------|-------------|----------|
| Cargo features on CLI/gateway | Feature flags with optional deps. Default file-based. | |
| Always compile all providers | All providers always included. Runtime selection only. | |
| You decide | Claude determines during planning. | |

**User's choice:** Cargo features on CLI/gateway.

### Factory Location

| Option | Description | Selected |
|--------|-------------|----------|
| Move to ironhermes-agent | Factory in agent/src/memory/factory.rs with cfg(feature) gates. | |
| Keep in core with cfg stubs | Factory stays in core with stub errors. | |
| You decide | Claude determines during planning. | |

**User's choice:** Move to ironhermes-agent/src/memory/factory.rs.

---

## Capacity Display & Tool UX

### System Prompt Display

| Option | Description | Selected |
|--------|-------------|----------|
| Header line per store | "## Memory (67% -- 1,474/2,200 chars)" per section. | |
| Footer summary | Single line after all content. | |
| No prompt display | Capacity only in tool responses. | |

**User's choice:** Header line per store.

### Tool Success Response

| Option | Description | Selected |
|--------|-------------|----------|
| Confirmation + updated capacity | "Added to memory. Memory: 72% -- 1,584/2,200 chars (3 entries)". | |
| Confirmation only | Simple success message. | |
| Full store contents | Entire updated store after mutation. | |

**User's choice:** Confirmation + updated capacity.

### Error Format

| Option | Description | Selected |
|--------|-------------|----------|
| Structured JSON errors | Machine-readable error envelopes. | |
| Plain text errors | Human-readable error messages. | |
| You decide | Claude determines during planning. | |

**User's choice:** Structured error envelopes matching hermes-agent's formatted error pattern. User provided extensive hermes-agent error handling documentation.

---

## Additional Areas (Round 2)

### Provider Data Migration
**User's choice:** Manual-triggered automatic migration. CLI prompts user when provider config changes. Migration via trait operations (dump/add_batch). Data loss is NOT the default.

### Agent-Loop Intercept
**User's choice:** Both memory and session_search intercepted before registry dispatch. Keeps Tool System focused on external capabilities.

### Grafeo/DuckDB Schema
**User's choice:** Grafeo: entries as nodes, metadata as edge labels, multi-hop relationship queries. DuckDB: flat columnar table, analytical aggregation queries.

### Testing Strategy
**User's choice:** Mock trait impls for unit tests. Docker-based integration tests in provider crates behind `#[cfg(feature = "integration-tests")]`.

---

## Claude's Discretion

- SQLite memory provider schema details
- Grafeo library selection and graph schema
- DuckDB table schema and query patterns
- session_search result text formatting
- Migration utility implementation details
- Whether factory returns Box or Arc<Mutex> based on usage patterns

## Deferred Ideas

None -- discussion stayed within phase scope.
