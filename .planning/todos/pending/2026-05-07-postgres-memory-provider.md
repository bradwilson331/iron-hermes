---
created: 2026-05-07T00:00:00.000Z
title: Add PostgreSQL memory provider
area: memory
files:
  - providers/memory-postgres/src/lib.rs
  - crates/ironhermes-agent/src/memory/factory.rs
  - Cargo.toml
---

## Problem

The memory system supports `file`, `sqlite`, `duckdb`, and `grafeo` providers but not PostgreSQL. Teams running IronHermes in containerized or cloud environments often already have a Postgres instance and want memory to persist there rather than via local files or an embedded DB.

## Solution

Create a new `providers/memory-postgres/` crate implementing the `MemoryProvider` trait, following the same pattern as `providers/memory-sqlite/`.

Key steps:

1. **New crate** — `providers/memory-postgres/src/lib.rs` with `PostgresMemoryProvider` implementing all required `MemoryProvider` methods. Use `sqlx` or `tokio-postgres` for async access.

2. **Config** — accept connection config from `$IRONHERMES_HOME/postgres.json` (loaded automatically by the factory). Suggested fields:
   ```json
   { "connection_url": "postgres://user:pass@host/dbname" }
   ```
   Connection URL can also be sourced from an env var via `ConfigField.env_var`.

3. **Factory wiring** — add `"postgres"` arm in `build_memory_provider` and `build_tokio_provider` in `crates/ironhermes-agent/src/memory/factory.rs`, behind a `memory-postgres` cargo feature.

4. **Security** — follow T-20-01: do not accept path-like strings from provider_config without validation. Validate the connection URL is not a local-path traversal.

5. **Schema** — create a `memory_entries` table on `initialize()` if absent (idempotent). Store `session_id`, `target` (memory/user), `content`, `updated_at`.

6. **Config docs** — add `memory-postgres` to the `MemoryConfig.provider` doc comment in `crates/ironhermes-core/src/config.rs`.

## Usage (once implemented)

```sh
cargo build --features memory-postgres
```

```yaml
# ~/.ironhermes/config.yaml
memory:
  provider: postgres
```

```json
// ~/.ironhermes/postgres.json
{ "connection_url": "postgres://user:pass@localhost/ironhermes" }
```
