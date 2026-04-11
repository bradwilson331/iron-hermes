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
- [x] Context file loading — SOUL.md, AGENTS.md loaded into system prompt with priority chain, security scanning, and truncation. Validated in Phase 1: Context File Loading

### Active

<!-- Current scope. Building toward these. -->
- [ ] Scheduled tasks — extend cron with natural language scheduling, skill attachment, multi-platform delivery
- [ ] Event hooks — gateway hooks (logging, alerts, webhooks) + plugin hooks (tool interception, guardrails)
- [ ] Skills system — on-demand knowledge documents with progressive disclosure, agentskills.io compatible
- [ ] Code execution — execute_code tool for Python scripts calling Hermes tools via sandboxed RPC
- [ ] Subagent delegation — delegate_task tool spawning child agents with isolated context and restricted toolsets
- [ ] Batch processing — parallel prompt execution generating ShareGPT-format trajectory data

### Out of Scope

- Discord/Slack adapters — foundation first, additional platforms after Telegram is solid
- Web UI — CLI and Telegram cover the primary use cases
- Multi-user auth system — single-operator deployment for now
- Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity

## Context

- Ported from a ~277K line Python project; core architecture maps 1:1 but Rust version is ~4,250 lines
- The Python hermes-agent has working Telegram, Discord, Slack gateways — IronHermes needs to reach Telegram parity first
- PromptBuilder fully wired: loads SOUL.md from IRONHERMES_HOME, discovers project context via priority chain, scans for injection, assembles in correct order
- TelegramAdapter has Bot API types and message methods but polling isn't connected to the agent loop
- The "self-improving" aspect is the project's differentiator — the agent edits its own SOUL.md/AGENTS.md to refine its personality and capabilities over time
- 31 tests passing across cron and agent crates (20 in agent after Phase 1); state, tools, CLI, and gateway lack test coverage

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

## Current Milestone: v1.1 Automation

**Goal:** Add automation, orchestration, and knowledge capabilities — scheduled tasks, event hooks, skills system, code execution, subagent delegation, and batch processing.

**Target features:**
- Scheduled Tasks — extend existing cron with natural language scheduling, skill attachment, multi-platform delivery
- Event Hooks — gateway + plugin hooks for logging, alerts, tool interception, guardrails
- Skills System — on-demand knowledge documents with progressive disclosure, agentskills.io compatible
- Code Execution — execute_code for Python scripts calling Hermes tools via sandboxed RPC
- Subagent Delegation — delegate_task spawning child agents with isolated context (up to 3 concurrent)
- Batch Processing — parallel prompt execution with ShareGPT-format output

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-04-11 — Phase 10.1 complete (fixed active_skills Arc wiring so skill activation via SkillsTool in Telegram conversations correctly feeds into AgentLoop's allowed_tools enforcement, closing SKILL-06 gap)*
