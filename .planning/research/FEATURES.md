# Feature Research

**Domain:** AI agent intelligence & identity layer (v2.0 milestone)
**Researched:** 2026-04-11
**Confidence:** HIGH (live codebase inspection + hermes-agent architecture docs)

---

## Current State Baseline

Before categorizing features, what is already fully implemented vs. partially implemented is
recorded here so roadmap phases are scoped correctly.

### Already Fully Implemented

| Component | Location | Notes |
|-----------|----------|-------|
| MemoryStore (MEMORY.md/USER.md) | `ironhermes-core/memory_store.rs` | Bounded, add/replace/remove, injection scan, atomic write, file lock |
| MemoryTool (agent-facing) | `ironhermes-tools/memory_tool.rs` | Read-only subagent mode, full write mode |
| StateStore SQLite sessions | `ironhermes-state/lib.rs` | WAL mode, sessions + messages tables, schema migrations v1-v6 |
| FTS5 full-text search | `ironhermes-state/lib.rs` | `messages_fts` virtual table, triggers, `search_messages()` |
| Session lineage | `ironhermes-state/lib.rs` | `parent_session_id` FK, `list_sessions()` with source filter |
| PromptBuilder (base layers) | `ironhermes-agent/prompt_builder.rs` | SOUL.md, AGENTS.md, project context priority chain, memory snapshot, skills catalog |
| Context file priority chain | `ironhermes-agent/prompt_builder.rs` | `.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules`, first-match wins |
| ContextCompressor (local) | `ironhermes-agent/context_compressor.rs` | Threshold-based, protects first N + last N tokens, prunes tool results |
| SOUL.md personality load | `ironhermes-agent/prompt_builder.rs` | Loads from `HERMES_HOME`, falls back to hardcoded default |
| Skill framework (SKILL.md) | `ironhermes-core/skills.rs` | Subdirectory discovery, frontmatter parse, name validation, catalog |
| CLI hooks + guardrails | `ironhermes-cli/main.rs` | BlocklistGuardrail, HookRegistry, webhooks, JSONL log — full parity with gateway |
| CLI execute_code | `ironhermes-cli/main.rs` | RPC sandbox registered in CLI as well as gateway |
| Slash command dispatch | `ironhermes-gateway/handler.rs` | `/` prefix intercepted pre-agent — routing stub exists |

### Partially Implemented

| Component | Gap |
|-----------|-----|
| ContextCompressor | No LLM-based summarization path; no dual threshold (agent 50% vs gateway hygiene 85%) |
| PromptBuilder | No `cache_control` breakpoints; no timestamp/datetime injection; not full 10-layer hermes-agent assembly; no `/personality` overlay application |
| Skill framework | No conditional activation (env var checks, config requirements); no credential file mounting; no Skills Hub |
| Slash commands | Gateway dispatches `/` prefix but zero commands are registered (`/help`, `/model`, `/personality`, `/reset`, `/session` all missing) |
| Context file loading | No progressive subdirectory discovery — only cwd checked, no parent-directory walk |

### Not Yet Started

| Component | Notes |
|-----------|-------|
| Memory provider trait | No `MemoryProvider` trait; no SQLite/Grafeo/DuckDB backend switching |
| Prompt caching | No `cache_control` on any API call; Anthropic `system_and_3` strategy not implemented |
| Gateway hygiene compression | Separate 85%-threshold compressor for gateway not wired |
| `session_search` tool | FTS5 exists in StateStore but not exposed as an agent tool |
| `/personality` overlays | Per-session identity overlays not implemented |
| Setup wizard | No interactive onboarding or tool check functions |
| Tool check functions | `is_available()` on `Tool` trait exists but always returns `true`; no env/config checks |

---

## Feature Landscape

### Table Stakes (Users Expect These)

Features users of a self-improving AI agent assume exist. Missing them makes the agent feel
incomplete relative to hermes-agent's documented capabilities.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Session continuity (resume prior conversation) | Users expect "same agent" across restarts | MEDIUM | StateStore fully built; gap is loading prior messages into context on session resume |
| `session_search` tool | Agent must recall past conversations when user asks "what did we discuss about X" | LOW | FTS5 built in StateStore; needs only a tool wrapper + registration |
| Prompt caching (`cache_control`) | 80-90% cost reduction on repeated system prompts; expected for any production Anthropic client | MEDIUM | No `cache_control` implemented; need to mark system prompt + tools as cached prefix; Anthropic allows up to 4 breakpoints; `system_and_3` strategy covers system + last 3 user turns |
| Complete slash command set | `/help`, `/reset`, `/session`, `/model` are standard bot UX primitives | MEDIUM | Dispatch stub exists; each command is small; the full set is the scope |
| SOUL.md as durable identity source | Users expect personality to persist and be self-editable | LOW | Already loading from HERMES_HOME; gap is `/personality` session overlay |
| Tool `is_available()` checks | Tools should silently not appear when prerequisites (API keys, env vars) are absent | LOW | Trait method exists but always returns `true`; needs per-tool env/config checks wired |
| Context file priority chain (full) | `.hermes.md`/`AGENTS.md`/`CLAUDE.md` hierarchy is a documented feature | LOW | Priority chain implemented; gap is subdirectory/parent-walk discovery |

### Differentiators (Competitive Advantage)

Features that distinguish IronHermes from a generic chatbot wrapper and deliver on the
"self-improving AI agent" core value.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Dual-mode context compression | Gateway hygiene (85%) prevents OOM on long-lived sessions; agent ContextEngine (50%) prevents context degradation on complex tasks — two different failure modes, one solution | MEDIUM | Current compressor is single-mode; parameterize threshold at construction and wire gateway to use 85% instance; LLM summarization is HIGH complexity — start without it |
| `/personality` session overlay | Per-conversation identity without editing SOUL.md — adopt a "focused assistant" or "code reviewer" persona per task | LOW | Session-scoped SOUL.md override; straightforward once slash commands exist |
| Progressive subdirectory context discovery | Loading `.hermes.md` from parent directories up to HERMES_HOME gives project-aware context when run from nested dirs — matches Claude Code / Cursor behavior users now expect | LOW | Walk cwd upward, first-match wins; low effort, high perceived quality |
| Conditional skill activation (env var / config) | Skills requiring API keys do not appear in the catalog when keys are absent; skills self-declare requirements | MEDIUM | Need `required_env` and `required_config` frontmatter fields + activation check in SkillRegistry; addresses SKILL-13 backlog from v1.1 |
| 10-layer system prompt assembly | Full hermes-agent prompt hierarchy with datetime, model-specific guidance, token budget hint, and cached/ephemeral separation | MEDIUM | PromptBuilder has 6 layers; add: current datetime, model hints, token budget, cache breakpoint placement |
| Memory provider trait (pluggable backends) | Enables power users to swap to Grafeo graph DB or DuckDB analytics without code changes | HIGH | Python hermes-agent has 8 providers; for v2.0 implement the trait + wire existing file store as default; Grafeo/DuckDB are v2.x scope unless explicitly prioritized |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Multiple simultaneous memory providers | "Use both Grafeo and SQLite together" | Conflicting facts across providers cause agent confusion; synchronization is very high complexity | Single-active-provider model with clear switching; hermes-agent enforces this explicitly |
| LLM-based context summarization in hot path | Produces higher-quality summaries | Adds a full LLM round-trip at the worst moment (context pressure = already slow); doubles latency and cost | Local compression (prune tool results, drop middle turns) as primary; LLM summarization as optional async background path only, deferred to v2.x |
| Real-time SOUL.md hot-reload mid-session | "Agent changes personality immediately when SOUL.md is edited" | Invalidates the Anthropic cached prefix on every edit; causes non-deterministic persona drift during a task | Load SOUL.md once at session start; self-improvement writes to SOUL.md but the change takes effect on the next session |
| Unlimited memory store size | "Let the agent store everything" | Unbounded files bloat the system prompt; Anthropic charges per token; injection risk grows with file size | Bounded stores (2200 chars MEMORY.md, 1375 chars USER.md); provider trait lets power users move to DB-backed storage |
| Slash commands for every tool | "I want `/web_search` as a slash command" | Duplicates the tool interface; two code paths; confuses users about when to use tools vs commands | Slash commands are session-control only (`/help`, `/model`, `/personality`, `/reset`, `/session`); all task execution goes through the agent tool loop |

---

## Feature Dependencies

```
[session_search tool]
    └──requires──> [StateStore FTS5]  (already built — trivial gap)

[Prompt caching]
    └──requires──> [10-layer PromptBuilder]  (stable system prompt is prerequisite)
    └──requires──> [Dual-mode ContextCompressor]  (hygiene mode keeps cached prefix stable)

[/personality overlay]
    └──requires──> [Slash command registry]  (handler stub built, commands not)

[Conditional skill activation]
    └──requires──> [SkillFrontmatter env/config fields]
    └──enhances──> [Tool is_available() checks]

[Memory provider trait]
    └──wraps──> [existing MemoryStore]  (becomes SQLite default backend)
    └──enables──> [Grafeo backend]  (future)
    └──enables──> [DuckDB backend]  (future)

[Dual-mode ContextCompressor]
    └──requires──> [threshold parameterization at construction]
    └──enhances──> [Prompt caching]  (hygiene keeps prefix stable for cache hits)

[Gateway hygiene compression (85%)]
    └──requires──> [Dual-mode ContextCompressor]

[Progressive subdirectory context discovery]
    └──extends──> [Context file priority chain]  (already built — additive change)

[Setup wizard]
    └──requires──> [Tool is_available() checks]
    └──enhances──> [Conditional skill activation]
```

### Dependency Notes

- **Prompt caching requires a stable system prompt prefix.** If the system prompt changes every
  turn (e.g., SOUL.md hot-reload, live memory injection), cache hits drop to zero. Gateway hygiene
  compression and the 10-layer PromptBuilder must stabilize the prefix before prompt caching is
  wired. These three features must be designed together in a single phase.

- **`session_search` tool is trivially low-effort.** `StateStore.search_messages()` already
  exists. This is a ~100-line tool wrapper + registration — highest value-to-effort ratio in v2.0.

- **Memory provider trait is a refactor, not a net-new feature.** The current file-based
  `MemoryStore` is the SQLite default. The trait wraps it so backends are swappable. Do not start
  Grafeo or DuckDB until the trait exists and the file store is proven as the default backend.

- **Slash commands unlock `/personality`.** The gateway already routes `/` prefixed messages to
  `handle_slash_command()`. Zero commands are registered. `/personality` is the highest-value
  first command; implement the registry and all core commands together in one phase.

---

## MVP Definition for v2.0

### Launch With (core intelligence & identity)

- [ ] `session_search` tool — exposes existing FTS5; trivial effort, high value
- [ ] Slash command registry + core set (`/help`, `/reset`, `/model`, `/session`, `/personality`)
- [ ] `/personality` session overlay — primary differentiator; uses slash command registry
- [ ] 10-layer PromptBuilder (add: datetime, model hint, token budget, cached/ephemeral split)
- [ ] Prompt caching (`cache_control` on system + tools, `system_and_3` strategy)
- [ ] Dual-mode ContextCompressor (agent at 50%, gateway hygiene at 85%)
- [ ] Progressive subdirectory context file discovery (parent-walk up to HERMES_HOME)
- [ ] Tool `is_available()` checks wired per tool
- [ ] Conditional skill activation (env var / config checks in `SkillFrontmatter`)

### Add After Core (provider abstraction)

- [ ] Memory provider trait + SQLite default backend (wraps existing MemoryStore)
- [ ] Setup wizard (interactive first-run config; depends on `is_available()` checks)
- [ ] LLM-based context summarization as opt-in background path (defer until local compression
      proves insufficient in production)

### Future Consideration (v2.x+)

- [ ] Grafeo graph DB memory backend
- [ ] DuckDB analytics memory backend
- [ ] Skills Hub / remote install from agentskills.io

---

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| `session_search` tool | HIGH | LOW | P1 |
| Slash command registry + core set | HIGH | MEDIUM | P1 |
| `/personality` overlay | HIGH | LOW | P1 |
| 10-layer PromptBuilder | HIGH | MEDIUM | P1 |
| Prompt caching (`cache_control`) | HIGH | MEDIUM | P1 |
| Dual-mode ContextCompressor | MEDIUM | MEDIUM | P1 |
| Progressive subdirectory discovery | MEDIUM | LOW | P1 |
| Tool `is_available()` checks | MEDIUM | LOW | P2 |
| Conditional skill activation | MEDIUM | MEDIUM | P2 |
| Memory provider trait + SQLite backend | LOW | MEDIUM | P2 |
| Setup wizard | MEDIUM | MEDIUM | P2 |
| LLM-based context summarization | MEDIUM | HIGH | P3 |
| Grafeo memory backend | LOW | HIGH | P3 |
| DuckDB memory backend | LOW | HIGH | P3 |
| Skills Hub / remote install | LOW | HIGH | P3 |

**Priority key:**
- P1: Required for v2.0 milestone ("faithful hermes-agent port, intelligence & identity")
- P2: Should have in v2.0 if schedule allows; not blocking the milestone
- P3: v2.x or later; do not start until P1/P2 are complete and proven in production

---

## Reference Feature Comparison

| Feature | hermes-agent (Python) | IronHermes v1.1 | v2.0 target |
|---------|----------------------|-----------------|-------------|
| Memory bounded stores | MEMORY.md + USER.md | Full | Already done |
| Memory providers | 8 providers (Holographic/SQLite default) | None | Provider trait + SQLite default |
| Session SQLite + FTS5 | Yes | Full | Already done |
| Session search tool | `session_search` tool | Missing | Add tool wrapper |
| Context compression | Dual (gateway 85% + agent 50%) | Agent only (local, single-mode) | Add gateway 85% mode |
| LLM summarization | Yes (`compress_with_summary`) | No | Defer to v2.x |
| Prompt caching | `system_and_3` strategy | None | Implement |
| System prompt layers | 10 layers | 6 layers | Complete to 10 |
| SOUL.md personality | Yes | Load only | Add `/personality` overlay |
| Progressive context discovery | Yes (parent walk) | cwd only | Add parent walk |
| Slash commands | Full COMMAND_REGISTRY | Dispatch stub, no commands | Implement core set |
| Conditional skill activation | env/config checks | Not implemented | Implement |
| Skills Hub | agentskills.io | Not implemented | Defer |

---

## Sources

- hermes-agent architecture: https://hermes-agent.nousresearch.com/docs/developer-guide/architecture
- hermes-agent memory providers: https://hermes-agent.nousresearch.com/docs/user-guide/features/memory-providers
- Anthropic prompt caching: https://platform.claude.com/docs/en/build-with-claude/prompt-caching
- Anthropic `system_and_3` caching pattern: https://spring.io/blog/2025/10/27/spring-ai-anthropic-prompt-caching-blog
- IronHermes codebase (inspected directly): `ironhermes-core/memory_store.rs`, `ironhermes-state/lib.rs`, `ironhermes-agent/prompt_builder.rs`, `ironhermes-agent/context_compressor.rs`, `ironhermes-core/skills.rs`, `ironhermes-gateway/handler.rs`, `ironhermes-cli/main.rs`

---
*Feature research for: IronHermes v2.0 Intelligence & Identity*
*Researched: 2026-04-11*
