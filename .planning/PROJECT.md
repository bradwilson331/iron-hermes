# IronHermes

## What This Is

A Rust rewrite of [hermes-agent](https://github.com/NousResearch/hermes-agent), the self-improving AI agent by Nous Research. IronHermes is a single-binary, high-performance conversational AI agent that runs as a CLI tool and Telegram bot — with automation capabilities including scheduled tasks, event hooks, a skills system, Python code execution, subagent delegation, and batch processing.

## Core Value

A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.

## Requirements

### Validated

- ✓ Context file loading (SOUL.md, AGENTS.md, project context) with priority chain assembly — v1.0
- ✓ Telegram gateway with streaming, concurrent users, graceful shutdown, error recovery — v1.0
- ✓ Self-improvement: agent reads/edits own context files with injection scanning — v1.0
- ✓ Memory subsystem with bounded facts in MEMORY.md — v1.0
- ✓ Web scraping via Firecrawl + local fallback with SSRF protection — v1.0
- ✓ Security: SSRF validation, threat scanning, gateway rate limiting — v1.0
- ✓ Scheduled tasks with natural language parsing, skill attachment, multi-platform delivery — v1.1
- ✓ Event hooks with lifecycle logging, guardrail tool interception, webhook forwarding — v1.1
- ✓ Skills system with progressive disclosure, agentskills.io compatibility, allowed_tools enforcement — v1.1
- ✓ Code execution sandbox with Python RPC tool bridge, env stripping, resource limits — v1.1
- ✓ Subagent delegation with isolated context, concurrency control, batch mode, cancellation — v1.1
- ✓ Batch processing with parallel ShareGPT output, checkpointing, quality filtering — v1.1

### Active

<!-- Next milestone scope. -->

(None yet — run `/gsd-new-milestone` to define v2 scope)

### Out of Scope

- Discord/Slack adapters — foundation first, additional platforms after Telegram is solid
- Web UI — CLI and Telegram cover the primary use cases
- Multi-user auth system — single-operator deployment for now
- Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity

## Context

- Ported from a ~277K line Python project; Rust version is ~360K lines across 7 workspace crates
- v1.0 shipped 2026-04-08: core agent loop, Telegram gateway, self-improvement, web scraping
- v1.1 shipped 2026-04-11: scheduled tasks, event hooks, skills, code execution, subagents, batch processing
- 382+ workspace tests passing
- The "self-improving" aspect is the project's differentiator — the agent edits its own SOUL.md/AGENTS.md to refine its personality and capabilities over time
- Tech stack: Rust 2024 edition, tokio async, SQLite (rusqlite), OpenAI-compatible LLM API

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
| Cargo workspace with 7 crates | Mirrors Python module structure, enables independent compilation and testing | ✓ Good |
| anyhow for all error handling | Speed of development over typed errors; HermesError enum exists but unused | ✓ Good |
| In-memory SessionStore for gateway | Simple start; can migrate to SQLite StateStore later | ✓ Good |
| Telegram first, other platforms later | Python hermes-agent's primary deployment is Telegram | ✓ Good |
| Context files over database for personality | Matches Python approach; files are git-trackable and agent-editable | ✓ Good |
| Phase ordering SCHED → HOOK → SKILL → EXEC → AGENT → BATCH | Hooks early for observability of later features | ✓ Good |
| New crates: ironhermes-hooks, ironhermes-exec | Clean separation of concerns for hooks and code execution | ✓ Good |
| Skills in ironhermes-core, SkillsTool in ironhermes-tools | No new crate deps needed | ✓ Good |
| Pattern-based env exclusion for exec sandbox | Forward compatible with new env vars | ✓ Good |
| delegate_task excluded from child toolsets | Structural recursion prevention | ✓ Good |
| Gateway-only for execute_code/hooks/guardrails | CLI is minimal interactive mode; gateway is full-featured | ⚠️ Revisit for v2 |

## Current State

**Shipped:** v1.1 Automation (2026-04-11)
**Next:** Planning v2 — run `/gsd-new-milestone` to begin

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
*Last updated: 2026-04-11 after v1.1 milestone*
