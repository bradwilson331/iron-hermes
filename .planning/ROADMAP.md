# Roadmap: IronHermes

## Milestones

- ✅ **v1.0 MVP** — Phases 1-4 (shipped 2026-04-08)
- ✅ **v1.1 Automation** — Phases 5-10.1 (shipped 2026-04-11)
- 🚧 **v2.0 Intelligence & Identity** — Phases 11-23 (in progress)

## Phases

<details>
<summary>✅ v1.0 MVP (Phases 1-4) — SHIPPED 2026-04-08</summary>

- [x] Phase 1: Context File Loading (2/2 plans) — completed 2026-04-08
- [x] Phase 2: Telegram Gateway (4/5 plans) — completed 2026-04-08
- [x] Phase 3: Self-Improvement + Security (3/3 plans) — completed 2026-04-08
- [x] Phase 4: Web Scraping Tools (2/2 plans) — completed 2026-04-08

</details>

<details>
<summary>✅ v1.1 Automation (Phases 5-10.1) — SHIPPED 2026-04-11</summary>

- [x] Phase 5: Scheduled Tasks (3/3 plans) — completed 2026-04-09
- [x] Phase 6: Event Hooks (3/3 plans) — completed 2026-04-09
- [x] Phase 7: Skills System (3/3 plans) — completed 2026-04-09
- [x] Phase 07.1: Skills Gap Analysis (2/2 plans) — completed 2026-04-09
- [x] Phase 07.2: Skills Spec Compliance (4/4 plans) — completed 2026-04-09
- [x] Phase 07.3: Cron Tick Agent Exec + Hooks (1/1 plan) — completed 2026-04-10
- [x] Phase 07.4: Hook Ordering & Dedup (3/3 plans) — completed 2026-04-10
- [x] Phase 07.5: Skills Housekeeping (2/2 plans) — completed 2026-04-10
- [x] Phase 8: Code Execution (4/4 plans) — completed 2026-04-10
- [x] Phase 9: Subagent Delegation (4/4 plans) — completed 2026-04-11
- [x] Phase 10: Batch Processing (4/4 plans) — completed 2026-04-10
- [x] Phase 10.1: Gateway active_skills Fix (1/1 plan) — completed 2026-04-11

</details>

### v2.0 Intelligence & Identity (In Progress)

**Milestone Goal:** Give the agent persistent memory, session continuity, a customizable identity, context management, and a complete skill/tool framework — faithful to hermes-agent's architecture.

- [x] **Phase 11: Memory Provider Trait** — Pluggable memory backend abstraction with lifecycle hooks (completed 2026-04-11)
- [x] **Phase 12: Provider Resolution** — Unified runtime resolver for CLI, gateway, cron, and ACP with multi-API-mode support (completed 2026-04-11)
- [x] **Phase 13: Session Storage** — SQLite StateStore with WAL, FTS5 full-text search, lineage, and migrations (completed 2026-04-12)
- [x] **Phase 14: Context Files & SOUL.md** — Priority-chain context file loading with SOUL.md identity and security scanning (completed 2026-04-12)
- [x] **Phase 15: 10-Layer Prompt Assembly** — Full hermes-agent prompt builder with frozen memory snapshots and personality overlays (completed 2026-04-12)
- [ ] **Phase 16: Prompt Caching** — DEFERRED (Anthropic blocking non-Anthropic clients)
- [ ] **Phase 17: Memory Tools & External Providers** — Memory tool (add/replace/remove), capacity tracking, SQLite/Grafeo/DuckDB backends, session search
- [ ] **Phase 18: Context Compression** — Dual compression system (agent at 50%, gateway at 85%) with pluggable ContextEngine trait
- [ ] **Phase 19: Skills Framework** — SKILL.md format, category discovery, conditional activation, env vars, credentials, security, and Hub
- [ ] **Phase 20: Tool Registry & Slash Commands** — Tool availability checks, toolset management, slash command router, and core command implementations
- [ ] **Phase 21: Gateway Architecture Alignment** — GatewayRunner with MemoryProvider integration, session key standard, authorization, hook lifecycle, and maintenance
- [ ] **Phase 22: CLI Feature Parity & ACP Adapter** — CLI gets execute_code/hooks/guardrails; ACP JSON-RPC stdio adapter for editor integration
- [ ] **Phase 23: Configuration & Setup Wizard** — Interactive first-run wizard, config commands, skill migration, profile isolation

## Phase Details

### Phase 11: Memory Provider Trait
**Goal**: A pluggable MemoryProvider trait is in place so memory backends can be swapped without changing agent code
**Depends on**: Phase 10.1 (v1.1 complete)
**Requirements**: MEM-07, MEM-08, MEM-12
**Success Criteria** (what must be TRUE):
  1. MemoryProvider trait compiles with Send + Sync + 'static bounds and all five lifecycle hooks (initialize, prefetch, sync_turn, on_session_end, shutdown)
  2. The default file-based MemoryStore implements MemoryProvider and all existing memory behavior is preserved
  3. Config accepts a single provider selection and the system rejects attempts to activate more than one external provider simultaneously
  4. Existing tests pass with no behavioral regression
**Plans:** 2/2 plans complete
Plans:
- [x] 11-01-PLAN.md — Define MemoryProvider trait, types, config extension, and provider factory
- [x] 11-02-PLAN.md — Verify workspace integration and add round-trip integration tests

### Phase 12: Provider Resolution
**Goal**: A single shared runtime resolver maps (provider, model) to (api_mode, api_key, base_url) for every call site in the system
**Depends on**: Phase 11
**Requirements**: PROV-01, PROV-02, PROV-03, PROV-04, PROV-05, PROV-06, PROV-07, PROV-08, PROV-09, PROV-10
**Success Criteria** (what must be TRUE):
  1. CLI, gateway, cron, ACP, and auxiliary calls all resolve provider credentials through the shared resolver — no duplicate resolution logic
  2. All three API modes (chat_completions, codex_responses, anthropic_messages) are reachable by config; native Anthropic path uses anthropic_adapter for format conversion
  3. Fallback provider chain kicks in automatically on 429/5xx/401 errors and tries configured fallback_providers in order
  4. Iteration budget (2-tier: 70% caution, 90% warning, 100% stop) is enforced and shared between parent and child agents
  5. Custom named providers configurable in config.yaml route to correct endpoint with correct scoped API key
**Plans:** 4/4 plans complete
Plans:
- [x] 12-01-PLAN.md — ProviderResolver, ApiMode, ResolvedEndpoint types and Config extension
- [x] 12-02-PLAN.md — AnthropicClient, message format adapter, AnyClient enum dispatch
- [x] 12-03-PLAN.md — Shared iteration budget and one-shot fallback provider switching
- [x] 12-04-PLAN.md — Call-site migration to ProviderResolver, remove old resolution methods

### Phase 13: Session Storage
**Goal**: All session data is durably persisted in SQLite and recoverable across restarts, with full-text search and lineage tracking
**Depends on**: Phase 11
**Requirements**: SESS-01, SESS-02, SESS-03, SESS-04, SESS-05, SESS-06, SESS-07, SESS-08, SESS-09, SESS-10, SESS-11
**Success Criteria** (what must be TRUE):
  1. Sessions and messages survive a process restart — agent resumes from SQLite without data loss
  2. User can name a session and later retrieve it by title; session search returns FTS5 snippets with match markers for keyword, phrase, boolean, and prefix queries
  3. Search results can be filtered by source (cli/telegram), role, and date range; old sessions can be exported or pruned by age
  4. Context compression triggering a session split records parent_session_id so lineage can be traced
  5. Schema migrations run sequentially on init; write contention is handled with retry+jitter and periodic WAL checkpoints
**Plans:** 3/3 plans complete
Plans:
- [x] 13-01-PLAN.md — Extend StateStore: busy_timeout, retry, v7 migration, lineage, title lookup, SearchFilter, FTS5 snippet search, sanitization
- [x] 13-02-PLAN.md — Add export/prune methods and comprehensive integration tests for SESS-01 through SESS-11
- [x] 13-03-PLAN.md — Wire write-through SessionStore, WAL checkpoint timer, CLI session persistence

### Phase 14: Context Files & SOUL.md
**Goal**: The agent loads its project context and identity from the filesystem using hermes-agent's priority chain, with security scanning and truncation
**Depends on**: Phase 11
**Requirements**: CTX-01, CTX-02, CTX-03, CTX-04, CTX-05, CTX-06, CTX-07
**Success Criteria** (what must be TRUE):
  1. Context file selection follows the priority chain (.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules) — only the first match loads, and .hermes.md walks from CWD to git root
  2. As the agent navigates into subdirectories, new context files are discovered and injected into tool results (each directory checked at most once, up to 5 parent directories)
  3. All context files are security scanned for injection patterns; files over 20,000 characters are truncated using the 70/20 head/tail ratio with a truncation marker
  4. .hermes.md YAML frontmatter is stripped before injection
**Plans:** 2/2 plans complete
Plans:
- [x] 14-01-PLAN.md — ContextLoader module + PromptBuilder priority chain, git-root walk, frontmatter stripping, skip_context_files
- [x] 14-02-PLAN.md — SubdirDiscovery module + AgentLoop wiring for progressive subdirectory context injection

### Phase 15: 10-Layer Prompt Assembly
**Goal**: The system prompt is assembled in 9 ordered slots matching hermes-agent, with frozen memory snapshots and session-level personality overlays
**Depends on**: Phase 12, Phase 13, Phase 14
**Requirements**: PRMT-01, PRMT-02, PRMT-03, PRMT-04, PRMT-05, PRMT-06, PRMT-07, MEM-06
**Success Criteria** (what must be TRUE):
  1. System prompt assembles 9 slots in order: Identity, ToolGuidance, Memory, Skills, ContextFiles, Timestamp, PlatformHints, SessionOverlay, UserMessage
  2. Memory snapshots are frozen at session start — mid-session memory writes persist to disk but do not alter the active prompt for that session
  3. SOUL.md loads from HERMES_HOME with a 20K char cap and security scan; falls back to DEFAULT_AGENT_IDENTITY when absent; subagent delegation skips SOUL.md and uses default identity
  4. /personality command applies a session-level overlay (selecting a built-in or custom preset) without modifying SOUL.md on disk
  5. Slots 1-5 are durable (stable across turns); slots 6-9 are ephemeral — this separation is maintained for prompt caching correctness
**Plans:** 3/3 plans complete
Plans:
- [x] 15-01-PLAN.md — PromptSlot enum, BTreeMap migration, build_split() durable/ephemeral split
- [x] 15-02-PLAN.md — PersonalityRegistry with 14 built-in presets + custom loading, config extension
- [x] 15-03-PLAN.md — CONTEXT_CANDIDATES update, subdirectory truncation cap, call site migration

### Phase 16: Prompt Caching
**Goal**: Anthropic Claude API calls use cache_control breakpoints so the stable system prompt prefix is reused across turns
**Depends on**: Phase 15
**Requirements**: PRMT-08, PRMT-09
**Success Criteria** (what must be TRUE):
  1. Anthropic models automatically receive cache_control breakpoints placed using the system_and_3 strategy (system prompt + last 3 non-system messages)
  2. Prompt caching activates automatically for Anthropic Claude models and respects the configured TTL (5m or 1h); non-Anthropic providers are unaffected
**Plans**: DEFERRED — Anthropic blocking non-Anthropic clients from cache_control

### Phase 17: Memory Tools & External Providers
**Goal**: The agent can manage persistent memory entries via tool calls, and optional SQLite/Grafeo/DuckDB backends are available for richer memory storage
**Depends on**: Phase 13, Phase 15
**Requirements**: MEM-01, MEM-02, MEM-03, MEM-04, MEM-05, MEM-09, MEM-10, MEM-11, MEM-13
**Success Criteria** (what must be TRUE):
  1. Agent can add, replace (by substring match), and remove entries in MEMORY.md (2200 char limit) and USER.md (1375 char limit) via the memory tool
  2. Memory store header shows capacity usage (e.g., "67% — 1,474/2,200 chars") in the system prompt
  3. Memory entries are rejected if they match security scanning patterns (injection/exfiltration)
  4. SQLite memory provider stores and retrieves facts with FTS5 search; Grafeo and DuckDB providers are available as feature-gated build options
  5. Agent can search past conversations using the session_search tool, which queries the FTS5 index on the StateStore
**Plans:** 3/5 plans executed
Plans:
- [x] 17-01-PLAN.md — Memory tool UX: capacity headers in snapshots, human-readable response format
- [x] 17-02-PLAN.md — session_search tool and agent loop interception
- [x] 17-03-PLAN.md — Provider infrastructure, factory relocation, SQLite memory provider
- [ ] 17-04-PLAN.md — Grafeo graph database memory provider
- [ ] 17-05-PLAN.md — DuckDB columnar memory provider with thread bridge

### Phase 18: Context Compression
**Goal**: The agent manages context window pressure through dual-mode compression that preserves tool pairs and protects critical message boundaries
**Depends on**: Phase 16, Phase 17
**Requirements**: PRMT-10, PRMT-11, PRMT-12, PRMT-13, PRMT-14, PRMT-15, PRMT-16
**Success Criteria** (what must be TRUE):
  1. Context pressure warning is emitted at 85% of the compression threshold; agent ContextCompressor triggers at 50% (configurable), gateway hygiene at 85%
  2. Compression never splits a tool_call/tool_result pair — atomic pairs are always preserved together across the summary boundary
  3. Compression protects the first N messages (system + first exchange) and last N messages (default 20); iterative re-compression updates the previous summary rather than summarizing from scratch
  4. Memory is flushed to disk before compression runs to prevent data loss
  5. ContextEngine trait is pluggable — the default strategy (local prune + structured summary) can be replaced via trait implementation
**Plans:** 12 plans
Plans:
- [x] 18-01..18-10 — shipped (see phase SUMMARYs)
- [ ] 18-11-PLAN.md — Default-config compression safety: auto-shrink effective protect_first_n when asst(tool_use) front-protected with tool_result outside (closes UAT gap: default protect_first_n=3 deadlock)
- [ ] 18-12-PLAN.md — Compression preserves tool-call outcome signal: enriched summary prompt + COMPLETED_TOOLS_SENTINEL in pinned [CONTEXT HISTORY] body (closes UAT gap: agent retries tool calls post-compression)

### Phase 19: Skills Framework
**Goal**: Skills are discoverable from a structured directory, conditionally activated based on toolsets and platform, and securely injected into the system prompt
**Depends on**: Phase 15
**Requirements**: SKILL-01, SKILL-02, SKILL-03, SKILL-04, SKILL-05, SKILL-06, SKILL-07, SKILL-08, SKILL-09, SKILL-10, SKILL-11
**Success Criteria** (what must be TRUE):
  1. Skills use SKILL.md format with YAML frontmatter; the catalog is listed at startup and full skill content loads only on activation (progressive disclosure)
  2. Skills with unmet requires_toolsets or requires_tools are hidden; skills with fallback_for_toolsets/fallback_for_tools are hidden when primaries are present
  3. Missing required_environment_variables trigger a setup prompt on skill load; credential files declared in required_credential_files are checked for existence and mounted into sandboxes
  4. Skills are restricted to declared platforms (macos/linux/windows) and hidden on incompatible platforms; skill env vars pass through to execute_code and terminal sandboxes
  5. Skills Hub allows publishing to and installing from external repos (GitHub, skills.sh, well-known endpoints) with trust levels (builtin, official, trusted, community)
  6. All skill content is security scanned before injection into the system prompt
**Plans**: TBD

### Phase 20: Tool Registry & Slash Commands
**Goal**: Tools declare their own availability checks and are excluded from the schema when prerequisites are absent; slash commands provide session-control across all platforms
**Depends on**: Phase 19, Phase 13
**Requirements**: TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05, SKILL-12, SKILL-13, SKILL-14
**Success Criteria** (what must be TRUE):
  1. Tool trait includes is_available() — tools are silently excluded from the LLM schema when their prerequisites (env vars, config) are absent
  2. Tools are organized into named toolsets with platform-specific presets; registration requires only a registration call at init time
  3. Agent-intercepted tools (memory, session_search, delegate_task, todo) are handled before registry dispatch
  4. Slash command router intercepts / prefixed messages before AgentLoop on all platforms (CLI, gateway, ACP); prefix matching and aliases work via resolve_command()
  5. All core slash commands are implemented: /help, /reset, /personality, /skills, /memory, /sessions, /search, /model, /stop, /new
  6. Setup wizard checks tool availability and guides users through missing prerequisites
**Plans**: TBD

### Phase 21: Gateway Architecture Alignment
**Goal**: The GatewayRunner fully implements hermes-agent's session key standard, authorization, hook lifecycle, message guard, and MemoryProvider integration
**Depends on**: Phase 17, Phase 20
**Requirements**: GW-01, GW-02, GW-03, GW-04, GW-05, GW-06, GW-07, GW-08, GW-09, GW-10, GW-11
**Success Criteria** (what must be TRUE):
  1. Session keys follow the agent:main:{platform}:{chat_type}:{chat_id} format constructed via build_session_key(); platform adapters produce MessageEvents consumed by the agent dispatcher
  2. Two-level message guard is in place: base adapter queues messages and sets interrupt when agent is active; gateway runner intercepts control commands (/stop, /approve, /deny) before dispatch
  3. Authorization covers per-platform allowlists, DM pairing flow with codes, global allow-all flag, and default-deny behavior
  4. Gateway hook lifecycle events fire correctly: gateway:startup, session:start/end/reset, agent:start/step/end, command:*
  5. Cron deliveries are NOT recorded in gateway session history; background maintenance (cron ticking, session expiry, memory flush, cache refresh) runs on schedule; token locks prevent two profiles using the same bot token
  6. MemoryManager is initialized per session and on_session_end fires cleanup via provider lifecycle hooks
**Plans**: TBD

### Phase 22: CLI Feature Parity & ACP Adapter
**Goal**: CLI has full feature parity with gateway for execute_code, hooks, and guardrails; ACP adapter enables editor integration via JSON-RPC stdio
**Depends on**: Phase 21
**Requirements**: CLI-01, CLI-02, CLI-03, CLI-04, CLI-05, CLI-06, CLI-07, CLI-08
**Success Criteria** (what must be TRUE):
  1. CLI registers execute_code, hooks, and guardrails — the same features available in gateway mode are available in CLI mode
  2. ACP adapter runs as a JSON-RPC stdio server wrapping AgentLoop; create/get/remove/fork/list/cleanup session operations work via ACP SessionManager
  3. AgentLoop callbacks (tool_progress, thinking, reasoning, step, stream_delta) are converted into ACP session_update events via run_coroutine_threadsafe
  4. Dangerous command approval maps to ACP permission requests (allow_once/allow_always/reject); tools render as editor-facing content (file diffs, shell commands, text previews)
  5. ACP sessions carry editor cwd bound to session ID for correct file/terminal tool context
**Plans**: TBD

### Phase 23: Configuration & Setup Wizard
**Goal**: First-run setup is guided, config is manageable via CLI commands, skill settings can be migrated, and profiles are fully isolated
**Depends on**: Phase 20, Phase 22
**Requirements**: CFG-01, CFG-02, CFG-03, CFG-04
**Success Criteria** (what must be TRUE):
  1. Interactive setup wizard runs on first launch, guiding the user through provider selection, API keys, model choice, and tool availability checks
  2. hermes config set/get/show commands read and write config.yaml values correctly
  3. hermes config migrate scans installed skills for unconfigured settings and prompts the user to fill them in
  4. Each profile has its own isolated HERMES_HOME, config, memory, sessions, and gateway PID — switching profiles does not bleed state
**Plans**: TBD

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Context File Loading | v1.0 | 2/2 | Complete | 2026-04-08 |
| 2. Telegram Gateway | v1.0 | 4/5 | Complete | 2026-04-08 |
| 3. Self-Improvement + Security | v1.0 | 3/3 | Complete | 2026-04-08 |
| 4. Web Scraping Tools | v1.0 | 2/2 | Complete | 2026-04-08 |
| 5. Scheduled Tasks | v1.1 | 3/3 | Complete | 2026-04-09 |
| 6. Event Hooks | v1.1 | 3/3 | Complete | 2026-04-09 |
| 7. Skills System | v1.1 | 3/3 | Complete | 2026-04-09 |
| 07.1. Skills Gap Analysis | v1.1 | 2/2 | Complete | 2026-04-09 |
| 07.2. Skills Spec Compliance | v1.1 | 4/4 | Complete | 2026-04-09 |
| 07.3. Cron Tick Agent Exec + Hooks | v1.1 | 1/1 | Complete | 2026-04-10 |
| 07.4. Hook Ordering & Dedup | v1.1 | 3/3 | Complete | 2026-04-10 |
| 07.5. Skills Housekeeping | v1.1 | 2/2 | Complete | 2026-04-10 |
| 8. Code Execution | v1.1 | 4/4 | Complete | 2026-04-10 |
| 9. Subagent Delegation | v1.1 | 4/4 | Complete | 2026-04-11 |
| 10. Batch Processing | v1.1 | 4/4 | Complete | 2026-04-10 |
| 10.1. Gateway active_skills Fix | v1.1 | 1/1 | Complete | 2026-04-11 |
| 11. Memory Provider Trait | v2.0 | 2/2 | Complete    | 2026-04-11 |
| 12. Provider Resolution | v2.0 | 4/4 | Complete   | 2026-04-11 |
| 13. Session Storage | v2.0 | 3/3 | Complete   | 2026-04-12 |
| 14. Context Files & SOUL.md | v2.0 | 2/2 | Complete   | 2026-04-12 |
| 15. 10-Layer Prompt Assembly | v2.0 | 3/3 | Complete    | 2026-04-12 |
| 16. Prompt Caching | v2.0 | 0/TBD | Deferred | - |
| 17. Memory Tools & External Providers | v2.0 | 3/5 | In Progress|  |
| 18. Context Compression | v2.0 | 10/12 | In Progress|  |
| 19. Skills Framework | v2.0 | 0/TBD | Not started | - |
| 20. Tool Registry & Slash Commands | v2.0 | 0/TBD | Not started | - |
| 21. Gateway Architecture Alignment | v2.0 | 0/TBD | Not started | - |
| 22. CLI Feature Parity & ACP Adapter | v2.0 | 0/TBD | Not started | - |
| 23. Configuration & Setup Wizard | v2.0 | 0/TBD | Not started | - |
