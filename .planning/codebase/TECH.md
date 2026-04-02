# Technology Stack

**Analysis Date:** 2026-04-01

## Language & Edition

**Primary:** Rust, Edition 2024
- Workspace version: `0.1.0`
- Authors: Nous Research
- License: MIT
- No `rust-toolchain.toml` present; relies on the developer's default toolchain

**Workspace resolver:** 2 (Cargo edition-2021+ resolver)

## Build System

**Cargo Workspace** with 7 crates defined in `/Users/twilson/code/ironhermes/Cargo.toml`:

| Crate | Type | Description |
|-------|------|-------------|
| `ironhermes-core` | lib | Core types, config, constants, error types |
| `ironhermes-state` | lib | SQLite state store for sessions and messages |
| `ironhermes-tools` | lib | Tool registry and built-in tool implementations |
| `ironhermes-agent` | lib | Agent loop, LLM client, streaming, context compression |
| `ironhermes-cli` | bin (`ironhermes`) | Interactive CLI entry point |
| `ironhermes-gateway` | lib | Multi-platform messaging gateway (Telegram, etc.) |
| `ironhermes-cron` | lib | Cron scheduler with file-based job persistence |

**Lockfile:** `Cargo.lock` is present and committed.

**Binary target:** `crates/ironhermes-cli/src/main.rs` produces the `ironhermes` binary.

**Dependency graph (inter-crate):**
```
ironhermes-cli
  ├── ironhermes-core
  ├── ironhermes-agent
  │     ├── ironhermes-core
  │     ├── ironhermes-tools
  │     │     └── ironhermes-core
  │     └── ironhermes-state
  │           └── ironhermes-core
  ├── ironhermes-tools
  └── ironhermes-state

ironhermes-gateway
  ├── ironhermes-core
  ├── ironhermes-agent
  ├── ironhermes-tools
  └── ironhermes-state

ironhermes-cron
  └── ironhermes-core
```

## Key Dependencies (with versions)

All versions are pinned in `[workspace.dependencies]` in the root `Cargo.toml`.

### Async Runtime
| Dependency | Version | Features | Used By |
|------------|---------|----------|---------|
| `tokio` | 1 | `full` | agent, tools, cli, gateway, cron |
| `tokio-stream` | 0.1 | — | agent (SSE streaming) |
| `futures` | 0.3 | — | agent (StreamExt for byte streams) |
| `async-trait` | 0.1 | — | core, tools, agent, gateway |

### HTTP
| Dependency | Version | Features | Used By |
|------------|---------|----------|---------|
| `reqwest` | 0.12 | `json`, `stream`, `rustls-tls` (no default features) | agent (LLM API), tools (web search), gateway (Telegram API) |

### Serialization
| Dependency | Version | Features | Used By |
|------------|---------|----------|---------|
| `serde` | 1 | `derive` | all crates |
| `serde_json` | 1 | — | all crates |
| `serde_yaml` | 0.9 | — | core (config file parsing) |

### Database
| Dependency | Version | Features | Used By |
|------------|---------|----------|---------|
| `rusqlite` | 0.32 | `bundled`, `backup` | state |

### CLI & Terminal UI
| Dependency | Version | Used By |
|------------|---------|---------|
| `clap` | 4 (derive) | cli |
| `crossterm` | 0.28 | cli |
| `colored` | 3 | cli |
| `indicatif` | 0.17 | cli |
| `rustyline` | 15 | cli (readline/REPL) |

### Error Handling
| Dependency | Version | Used By |
|------------|---------|---------|
| `anyhow` | 1 | all crates (application errors) |
| `thiserror` | 2 | core, state, tools, gateway, cron (typed errors) |

### Logging / Tracing
| Dependency | Version | Features | Used By |
|------------|---------|----------|---------|
| `tracing` | 0.1 | — | all crates |
| `tracing-subscriber` | 0.3 | `env-filter` | cli |

### Utilities
| Dependency | Version | Purpose |
|------------|---------|---------|
| `chrono` | 0.4 (`serde`) | Timestamps throughout |
| `uuid` | 1 (`v4`) | Session and job IDs |
| `regex` | 1 | File search tool |
| `glob` | 0.3 | File search tool |
| `cron` | 0.13 | Cron expression parsing |
| `dotenvy` | 0.15 | `.env` loading |
| `dirs` | 6 | Home directory resolution |

### Dev Dependencies
| Dependency | Version | Crate |
|------------|---------|-------|
| `tempfile` | 3 | cron (test fixtures) |

## Runtime (Async / Tokio)

**Tokio with `full` feature set** is the async runtime. The binary entry point uses `#[tokio::main]` in `crates/ironhermes-cli/src/main.rs`.

Key async patterns:
- **`async-trait`** for all trait-based async interfaces (`Tool`, `PlatformAdapter`, `MessageHandler`)
- **`tokio::spawn`** for concurrent tasks (SSE stream processing in `crates/ironhermes-agent/src/client.rs`, Telegram long-polling in `crates/ironhermes-gateway/src/telegram.rs`)
- **`tokio::sync::mpsc`** channels for streaming LLM responses (256-element buffer)
- **`tokio::sync::Mutex`** for shared context compressor state
- **`tokio::process::Command`** for shell execution in the terminal tool (`crates/ironhermes-tools/src/terminal.rs`)
- **`tokio::time::timeout`** for command execution timeouts

Note: `rusqlite` is synchronous. The `StateStore` in `crates/ironhermes-state/src/lib.rs` performs blocking I/O on the calling thread. There is no `spawn_blocking` wrapper currently.

## Database

**SQLite** via `rusqlite` 0.32 with `bundled` feature (statically links SQLite).

- Default location: `~/.ironhermes/state.db`
- Schema version: 6 (with migration support from v1 through v6)
- WAL journal mode enabled
- Foreign keys enabled
- FTS5 virtual table for full-text message search

**Tables:**
- `sessions` — agent conversation sessions with token/tool stats
- `messages` — individual chat messages with role, content, tool calls
- `messages_fts` — FTS5 index on message content (auto-populated via triggers)
- `schema_version` — single-row version tracker

**Cron job persistence** uses a separate JSON file at `~/.ironhermes/cron/jobs.json` (not SQLite), with atomic write via temp-file-and-rename in `crates/ironhermes-cron/src/lib.rs`.

## External APIs

### LLM Providers (OpenAI-compatible chat completions)
- **OpenRouter** (default): `https://openrouter.ai/api/v1` — env var: `OPENROUTER_API_KEY`
- **Anthropic**: `https://api.anthropic.com` — env var: `ANTHROPIC_API_KEY`
- **OpenAI**: configurable base URL — env var: `OPENAI_API_KEY`
- **Nous Research**: `https://inference-api.nousresearch.com/v1` — env var: uses OpenRouter key path
- Default model: `anthropic/claude-sonnet-4-20250514`
- Client: `crates/ironhermes-agent/src/client.rs` (`LlmClient`)
- Supports both non-streaming and SSE streaming completions
- All providers use the OpenAI-compatible `/chat/completions` endpoint format

### Firecrawl (Web Search)
- Endpoint: `https://api.firecrawl.dev/v1/search`
- Env var: `FIRECRAWL_API_KEY`
- Implementation: `crates/ironhermes-tools/src/web_search.rs`
- Conditionally available (checks env var at runtime via `is_available()`)

### Telegram Bot API
- Base URL: `https://api.telegram.org`
- Env var: configured per-platform in `config.yaml` gateway section
- Long polling via `getUpdates`
- Implementation: `crates/ironhermes-gateway/src/telegram.rs`

### Planned/Enumerated Platforms (not yet implemented)
The `Platform` enum in `crates/ironhermes-core/src/types.rs` lists: Discord, WhatsApp, Slack, Signal, Matrix, Mattermost, Email, SMS, DingTalk, Feishu, WeCom, Home Assistant, Webhook, API Server. Only Telegram has an adapter implementation.

## Testing Frameworks

**Built-in Rust test framework** (`#[test]`, `#[cfg(test)]`). No external test runner.

**Test locations:**
- `crates/ironhermes-cron/src/lib.rs` — 9 unit tests (job store CRUD, cron parsing, tick lock)
- `crates/ironhermes-agent/src/context_compressor.rs` — 2 unit tests (token estimation, compression threshold)

**Test command:**
```bash
cargo test                    # Run all tests
cargo test -p ironhermes-cron # Run tests for a specific crate
```

**Dev dependencies:**
- `tempfile` 3 — used in cron tests for temporary directories

**No integration tests, no E2E tests, no test harness beyond `cargo test`.**

## Configuration

**Config file:** `~/.ironhermes/config.yaml` (YAML via `serde_yaml`)
- Defined in `crates/ironhermes-core/src/config.rs`
- Sections: `model`, `agent`, `terminal`, `web`, `gateway`, `cron`, `security`
- All sections have `Default` implementations; config file is optional

**Environment variables:**
- `.env` file loaded from `~/.ironhermes/.env` via `dotenvy`
- `IRONHERMES_HOME` — override home directory (default: `~/.ironhermes`)
- `OPENROUTER_API_KEY` / `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` — LLM provider auth
- `OPENAI_BASE_URL` — custom LLM endpoint override
- `FIRECRAWL_API_KEY` — web search tool activation
- `RUST_LOG` — tracing/log level filter (via `tracing-subscriber` env-filter)

## Platform Requirements

**Development:**
- Rust toolchain (edition 2024 requires rustc 1.85+)
- No additional system dependencies (SQLite is bundled)

**Production:**
- Single static binary (`ironhermes`)
- TLS via rustls (no OpenSSL dependency)
- Filesystem access for `~/.ironhermes/` (config, state.db, cron jobs)

---

*Stack analysis: 2026-04-01*
