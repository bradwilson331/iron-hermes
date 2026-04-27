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

<!-- Current scope for v2.0: Intelligence & Identity -->

- [ ] Persistent memory (MEMORY.md/USER.md bounded stores, memory tool with add/replace/remove, substring matching, capacity management, security scanning)
- [x] Memory provider trait with pluggable backend abstraction, lifecycle hooks, and config-driven factory — Validated in Phase 11: Memory Provider Trait
- [ ] Memory provider backends: SQLite, Grafeo (graph DB), and DuckDB; single-provider selection
- [ ] Session storage (SQLite state.db, WAL mode, sessions/messages tables, FTS5 full-text search, schema migrations, write contention handling, session lineage, session_search tool)
- [x] Context compression (dual system: gateway hygiene at 85%, agent ContextEngine at 50%; pluggable ContextEngine trait, structured summaries, iterative re-compression) — Validated in Phase 18: Context Compression (live UAT Test 2 re-run pending)
- [ ] Prompt caching (Anthropic cache_control breakpoints, system_and_3 strategy, cached/ephemeral separation)
- [x] Prompt assembly (10-layer system prompt builder matching hermes-agent: SOUL.md identity, tool-aware guidance, memory snapshots, skills index, context files, timestamps, platform hints) — Validated in Phase 15: 10-Layer Prompt Assembly
- [x] Context file loading (.hermes.md > HERMES.md > AGENTS.md > CLAUDE.md > .cursorrules priority chain, .cursor/rules/*.mdc fallback, progressive subdirectory discovery with 8K cap, security scanning, truncation) — Validated in Phase 15: 10-Layer Prompt Assembly
- [x] SOUL.md personality system (durable identity from HERMES_HOME, default fallback, /personality session overlays, 14 built-in presets, custom preset loading) — Validated in Phase 15: 10-Layer Prompt Assembly
- [ ] Skill framework (SKILL.md format, category-based discovery, progressive disclosure, conditional activation, env var/config requirements, credential file mounting, security scanning, Skills Hub)
- [x] Slash command integration (platform-agnostic CommandRouter with 49 commands, three-stage resolution, unified CLI+gateway dispatch) — Validated in Phase 21.1: Slash Commands
- [ ] Tool registry improvements (toolset management, check functions, setup wizard integration)
- [x] CLI feature parity (execute_code, hooks, guardrails available in CLI mode) — Validated in Phase 22: CLI Feature Parity
- [ ] Configuration/setup wizard improvements

### Out of Scope

- Discord/Slack adapters — foundation first, additional platforms after Telegram is solid
- Web UI — CLI and Telegram cover the primary use cases
- Multi-user auth system — single-operator deployment for now
- Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity

## Context

- Ported from a ~277K line Python project; Rust version is ~360K lines across 7 workspace crates
- v1.0 shipped 2026-04-08: core agent loop, Telegram gateway, self-improvement, web scraping
- v1.1 shipped 2026-04-11: scheduled tasks, event hooks, skills, code execution, subagents, batch processing
- v2.0 Phase 21 complete 2026-04-17: CLI TUI polish (status bar, knight-rider scanner, graceful double ctrl-c)
- v2.0 Phase 22 complete 2026-04-17: CLI feature parity (cron, skills, execute_code, guardrails, HookRegistry in both CLI paths)
- v2.0 Phase 22.1 complete 2026-04-17: TUI extension hooks (TuiExtension trait, widget slot compositing, keybinding registry, command dispatch chain, render/REPL integration)
- v2.0 Phase 21.1 complete 2026-04-18: Slash commands (CommandRouter with 49 commands, three-stage resolve, unified CLI+gateway dispatch replacing hardcoded handlers)
- v2.0 Phase 21.3 complete 2026-04-20: Model metadata (ModelRegistry with 37-model static table + disk cache, tiktoken-rs token estimation, models.dev/OpenRouter API fetch, CLI subcommands + slash commands, D-06 context_length precedence chain)
- v2.0 Phase 21.4 complete 2026-04-20: Persistent memory gap closure (memory_manager wired into AgentLoop/context engine across CLI+gateway, memory_enabled/user_profile_enabled config toggles, `hermes memory status/off` subcommands, on_session_end in clean exit paths, MEM-06 verified)
- v2.0 Phase 21.5 complete 2026-04-21: Memory provider plugin (factory config loading, SQLite FTS5 memory_recall, Grafeo entity extraction, DuckDB ILIKE bridge, agent loop wiring for memory provider tools)
- v2.0 Phase 21.6 complete 2026-04-22: Deployment setup files (.env.example, cli-config.yaml.example, Dockerfile with multi-stage Rust build, docker/entrypoint.sh, install.sh curl-pipe installer, setup-ironhermes.sh dev setup, ensure_home_dirs() first-run scaffolding)
- v2.0 Phase 22.4.2.2 complete 2026-04-27: Cron create defaults to TG origin when gateway active — both `hermes cron create` (CLI) and the LLM `cronjob` tool auto-route to `deliver=origin` for the configured single-chat whitelist; multi-chat falls back to `local` with operator hint (stderr from CLI, `tracing::warn` from LLM tool); explicit `--deliver` flag/JSON arg preserved as full bypass; OriginDecision enum lives in `ironhermes-core` with plain-String fields to avoid a circular crate dep; INV ledger advanced 62 → 64
- v2.0 Phase 22.4.2.3 complete 2026-04-27: Fix pre-existing INV-22.3-02 banner-bleed regression test on `develop` — relaxed `assert_eq!(count, 1)` to `assert!(count >= 1)` and tightened ordering via `match_indices` so every `print_banner();` call site is asserted strictly before `TuiHandle::new_with_extensions`; accepts the three legitimate Plan 22.4-11 ratatui-arm + run_chat sites without losing regression intent; structural-test-only edit, `main.rs` byte-identical; 6/6 invariants_22_3 tests green
- 400+ workspace tests passing
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
| Gateway-only for execute_code/hooks/guardrails | CLI is minimal interactive mode; gateway is full-featured | ⚠️ Revisit — v2 brings CLI parity |
| Cross-crate transport types use plain Strings (no embedded downstream types) | `OriginDecision` in `ironhermes-core` carries `String` fields, not `ironhermes_cron::JobOrigin` — embedding would create a circular crate dep. Consumers (CLI + LLM tool) construct `JobOrigin` at the call site where both crates are in scope. Pattern applies to any future enum that returns "what platform/route to use" data from `ironhermes-core` to a downstream crate. | ✓ Good (Phase 22.4.2.2) |

## Current Milestone: v2.0 Intelligence & Identity

**Goal:** Give the agent persistent memory, session continuity, a customizable identity, context management, and a complete skill/tool framework — faithful to hermes-agent's architecture.

**Target features:**
- Persistent memory (MEMORY.md/USER.md) with memory providers (SQLite, Grafeo, DuckDB)
- Session storage with SQLite + FTS5 search + session lineage
- Context compression (dual system) + prompt caching (Anthropic breakpoints)
- Full 10-layer prompt assembly with cached/ephemeral separation
- Context files (.hermes.md/AGENTS.md/CLAUDE.md/.cursorrules) with progressive discovery
- SOUL.md personality system with /personality overlays
- Skill framework (SKILL.md format, discovery, conditional activation, env vars, security, Hub)
- Slash commands (SKILL-13), tool registry improvements, CLI feature parity
- Configuration/setup wizard improvements

**Architectural constraint:** All implementation must align to hermes-agent's architecture (see hermes-agent Architecture docs). Port faithfully, deviate only with documented rationale.

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
*Last updated: 2026-04-27 after Phase 22.4.2.3 (fix pre-existing INV-22.3-02 banner-bleed regression test) complete*
