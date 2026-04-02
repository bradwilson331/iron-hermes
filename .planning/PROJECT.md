# IronHermes

## What This Is

A Rust rewrite of [hermes-agent](https://github.com/NousResearch/hermes-agent), the self-improving AI agent by Nous Research. IronHermes is the foundation codebase going forward, replacing the Python original with a single-binary, high-performance agent that runs as a CLI tool and Telegram bot.

## Core Value

A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.

## Requirements

### Validated

<!-- Shipped and confirmed valuable. -->

- [x] Cargo workspace with modular crate architecture (core, state, tools, agent, cli, gateway, cron)
- [x] OpenAI-compatible chat completions client with streaming SSE support
- [x] Tool registry with trait-based dispatch (terminal, read_file, write_file, patch, search_files, web_search)
- [x] Agent loop: LLM call -> tool extraction -> dispatch -> append results -> repeat
- [x] SQLite state store with WAL mode and FTS5 search for session persistence
- [x] Interactive CLI with rustyline, slash commands, streaming output
- [x] Cron job scheduler with file-based persistence and tick locking
- [x] Context compression (token estimation, tool result pruning, middle message dropping)
- [x] Telegram adapter skeleton (Bot API types, send/edit/delete/reactions)

### Active

<!-- Current scope. Building toward these. -->

- [ ] Context file loading — SOUL.md, AGENTS.md loaded into system prompt, configurable paths
- [ ] Telegram gateway — long polling wired to agent loop, full conversational bot with tool use
- [ ] Self-improvement loop — agent can modify its own context files, prompts, and behavior
- [ ] Web scraping tools — page content reading, URL fetching beyond Firecrawl search

### Out of Scope

- Discord/Slack adapters — foundation first, additional platforms after Telegram is solid
- Web UI — CLI and Telegram cover the primary use cases
- Multi-user auth system — single-operator deployment for now
- Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity

## Context

- Ported from a ~277K line Python project; core architecture maps 1:1 but Rust version is ~4,250 lines
- The Python hermes-agent has working Telegram, Discord, Slack gateways — IronHermes needs to reach Telegram parity first
- PromptBuilder already has platform-hint scaffolding and context file path resolution — needs the actual file loading wired up
- TelegramAdapter has Bot API types and message methods but polling isn't connected to the agent loop
- The "self-improving" aspect is the project's differentiator — the agent edits its own SOUL.md/AGENTS.md to refine its personality and capabilities over time
- 11 tests passing across cron and agent crates; state, tools, CLI, and gateway lack test coverage

## Constraints

- **Language**: Rust 2024 edition — committed, no mixed Python/Rust
- **Async runtime**: tokio — already threaded through all crates
- **Database**: SQLite via rusqlite — embedded, no external DB dependency
- **LLM API**: OpenAI-compatible endpoints (OpenRouter, Nous, Anthropic) — no vendor lock-in
- **Deployment**: Single binary — `cargo build --release` produces one artifact
- **Config**: YAML + .env at `~/.ironhermes/` — established pattern, don't change

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Cargo workspace with 7 crates | Mirrors Python module structure, enables independent compilation and testing | -- Pending |
| anyhow for all error handling | Speed of development over typed errors; HermesError enum exists but unused | -- Pending |
| In-memory SessionStore for gateway | Simple start; can migrate to SQLite StateStore later | -- Pending |
| Telegram first, other platforms later | Python hermes-agent's primary deployment is Telegram | -- Pending |
| Context files over database for personality | Matches Python approach; files are git-trackable and agent-editable | -- Pending |

---
*Last updated: 2026-04-01 after initial project setup and codebase mapping*
