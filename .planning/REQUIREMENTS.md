# Requirements: IronHermes

**Defined:** 2026-04-11
**Core Value:** A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.

## Current Milestone: v2.1 Carry-Overs + Learning Loop

**Goal:** Close all v2.0 deferred requirements (29 carry-overs across 7 categories) **AND** land the Learning Loop foundation (5 new reqs in 2 phases) — the periodic nudge + autonomous skill creation that makes IronHermes self-improving rather than just feature-complete (per hermes-agent design philosophy).

**Active scope (34 reqs across 8 categories):**

| Category | Reqs in v2.1 |
|----------|--------------|
| ACP adapter | CLI-03, CLI-04, CLI-05, CLI-06, CLI-07, CLI-08 |
| Prompt caching | PRMT-08, PRMT-09 |
| Toolset management | TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05 |
| Provider polish | PROV-04, PROV-06, PROV-08 |
| Skills trust tiers | SKILL-09 |
| Gateway formal verification | GW-01, GW-02, GW-03, GW-04, GW-06, GW-07, GW-09, GW-10 |
| Configuration / setup wizard | CFG-01, CFG-02, CFG-03, CFG-04 |
| **Learning Loop (NEW for v2.1)** | **LEARN-01, LEARN-02, LEARN-03, LEARN-04, LEARN-05** |

### v2.1 Architectural Principles (carried through every phase)

These design principles, sourced from the canonical hermes-agent architecture, must be honored across all v2.1 phase implementations:

1. **The Learning Loop is the unifying philosophy.** Skills + Memory + Session Search are outputs of one continuous process — every phase must consider how it participates.
2. **Cache-awareness is load-bearing.** Three operations break the prompt cache: switching models mid-session, changing memory files mid-session, changing context files mid-session. Phase 27 enforces; Phases 23/25/26 surface warnings.
3. **3,575 char total memory limit** — already aligned (MEM-01 2,200 + MEM-02 1,375 = 3,575). Phase 32 must respect this when nudging.
4. **Patch-over-rewrite for skill self-improvement** — Phase 33's `skill_manage` defaults to patch action for token efficiency + correctness preservation.
5. **Progressive disclosure for token economy** — names + summaries always, full content on demand. Applies to skills (Phase 28) and is a design constraint Phase 33 must preserve when creating new skills.
6. **Sessions tied to ID, not platform.** Phase 29 verifies; Phases 30/31 implement for ACP.
7. **Gateway as same-loop participant**, not bolt-on delivery. Phase 29 verifies that incoming messages can trigger skill creation (Phase 33), automation outputs route back through gateway.

(CLI-03..08 entries previously moved to "Future Requirements → Deferred from v2.0" on 2026-04-27 are restored as v2.1 active here. No body-checkbox movement needed — categories remain canonical home.)

## v2.0 Requirements

Requirements originally defined for v2.0: Intelligence & Identity. v2.0 was audited 2026-04-27 and ready to close as `tech_debt` (77/93 active satisfied). Carry-overs above. Each maps to roadmap phases.

### Memory

- [x] **MEM-01**: Agent can add, replace, and remove entries in bounded MEMORY.md store (2200 char limit) via memory tool
- [x] **MEM-02**: Agent can add, replace, and remove entries in bounded USER.md store (1375 char limit) via memory tool
- [x] **MEM-03**: Memory tool supports substring matching for replace/remove operations (old_text identifies target entry)
- [x] **MEM-04**: Memory stores display capacity usage in system prompt header (e.g., "67% — 1,474/2,200 chars")
- [x] **MEM-05**: Memory entries are security scanned for injection/exfiltration patterns before acceptance
- [x] **MEM-06
**: Memory snapshots are frozen at session start and injected into system prompt — mid-session writes persist to disk but do not mutate the active prompt
- [x] **MEM-07**: MemoryProvider trait defines lifecycle hooks (initialize, prefetch, sync_turn, on_session_end, shutdown) with Send + Sync + 'static bounds
- [x] **MEM-08**: Built-in file-based MemoryStore implements MemoryProvider as the default backend
- [x] **MEM-09**: SQLite memory provider stores facts with FTS5 search capability
- [x] **MEM-10**: Grafeo graph database memory provider stores facts as nodes/edges with relationship queries (feature-gated build)
- [x] **MEM-11**: DuckDB memory provider stores facts with analytical query capability (feature-gated build, async bridge for !Send Connection)
- [x] **MEM-12**: Only one external memory provider can be active at a time — single-provider selection via config
- [x] **MEM-13**: Agent can search past conversations via session_search tool backed by StateStore FTS5

### Session Storage

- [x] **SESS-01**: StateStore (SQLite + WAL mode) is wired as source of truth for session persistence across CLI, gateway, and ACP
- [x] **SESS-02**: In-memory SessionStore acts as write-through cache; on restart, sessions recover from SQLite
- [x] **SESS-03**: Session lineage tracks parent_session_id chains when context compression triggers session splits
- [x] **SESS-04**: User can name sessions with unique titles and resolve sessions by title
- [x] **SESS-05**: FTS5 full-text search supports keyword, phrase, boolean, and prefix queries with automatic input sanitization
- [x] **SESS-06**: Search results include FTS5-generated snippets with match markers and 1-message context window
- [x] **SESS-07**: Search supports filtering by source (cli/telegram/etc.), role, and date range
- [x] **SESS-08**: Sessions can be exported (single or bulk with source filter) as structured data
- [x] **SESS-09**: Old ended sessions can be pruned by age and optional source filter
- [x] **SESS-10**: Schema migrations run sequentially on init with idempotent ALTER TABLE operations
- [x] **SESS-11**: Write contention handled with short SQLite timeout, application-level retry with random jitter, and periodic WAL checkpoints

### Prompt Assembly

- [ ] **PRMT-01**: System prompt assembles 10 layers in order: SOUL.md identity, tool-aware guidance, optional provider block, optional system message, frozen MEMORY snapshot, frozen USER snapshot, skills index, context files, timestamp/session, platform hint
- [ ] **PRMT-02**: Cached layers (1-8) are stable across turns; dynamic layers (9-10) are ephemeral — separation preserves prompt caching
- [ ] **PRMT-03**: SOUL.md loads from HERMES_HOME as agent identity (slot 1); falls back to hardcoded DEFAULT_AGENT_IDENTITY when absent
- [ ] **PRMT-04**: SOUL.md content is security scanned and truncated (20K char cap) before injection
- [ ] **PRMT-05**: When skip_context_files is set (subagent delegation), SOUL.md is not loaded and default identity is used
- [ ] **PRMT-06**: /personality slash command applies session-level identity overlay without modifying SOUL.md on disk
- [ ] **PRMT-07**: Built-in personality presets available (helpful, concise, technical, creative, teacher) plus custom presets from config
- [ ] **PRMT-08**: Anthropic cache_control breakpoints placed using system_and_3 strategy (system prompt + last 3 non-system messages)
- [ ] **PRMT-09**: Prompt caching automatically enabled for Anthropic Claude models with configurable TTL (5m/1h)
- [x] **PRMT-10**: Context pressure warnings emitted at 85% of compression threshold
- [x] **PRMT-11**: Dual-mode context compression: agent ContextCompressor at 50% threshold (configurable), gateway hygiene at 85% threshold
- [x] **PRMT-12**: ContextEngine trait enables pluggable compression strategies (default: local prune + structured summary)
- [x] **PRMT-13**: Compression preserves tool_call/tool_result pairs atomically — never splits a pair across summary boundary
- [x] **PRMT-14**: Compression protects first N messages (system + first exchange) and last N messages (configurable, default 20)
- [x] **PRMT-15**: Iterative re-compression updates previous summary rather than summarizing from scratch
- [x] **PRMT-16**: Memory is flushed to disk before compression to prevent data loss

### Context Files

- [ ] **CTX-01**: Context file priority chain: .hermes.md > AGENTS.md > CLAUDE.md > .cursorrules (first match wins, only one project context loaded)
- [ ] **CTX-02**: .hermes.md walks from CWD to git root; AGENTS.md/CLAUDE.md check CWD only; .cursorrules checks CWD only
- [ ] **CTX-03**: Progressive subdirectory discovery: as agent navigates into subdirectories via tool calls, context files are discovered and injected into tool results
- [ ] **CTX-04**: Each subdirectory checked at most once per session; discovery walks up to 5 parent directories
- [ ] **CTX-05**: All context files security scanned for prompt injection patterns (invisible unicode, instruction overrides, credential exfiltration)
- [ ] **CTX-06**: Context files truncated at 20,000 characters using 70/20 head/tail ratio with truncation marker
- [ ] **CTX-07**: .hermes.md YAML frontmatter stripped before injection (reserved for future config overrides)

### Skills

- [x] **SKILL-01**: Skills use SKILL.md format with YAML frontmatter (name, description, version, author, platforms, metadata)
- [x] **SKILL-02**: Skills organized by category in skills/ directory with progressive disclosure (catalog at startup, full content on activation)
- [x] **SKILL-03**: Conditional skill activation: requires_toolsets, requires_tools hide skills when dependencies absent; fallback_for_toolsets, fallback_for_tools hide skills when primaries present
- [x] **SKILL-04**: Skills declare required_environment_variables with prompt/help/required_for fields; missing vars trigger setup prompt on load
- [x] **SKILL-05**: Skills declare config settings (metadata.hermes.config) stored in config.yaml under skills.config namespace
- [x] **SKILL-06**: Skills declare required_credential_files for OAuth tokens; existence checked on load, files mounted into sandboxes
- [x] **SKILL-07**: Skill content security scanned before injection into system prompt (same patterns as context file scanning)
- [x] **SKILL-08
**: Skills Hub: publish skills to external repos, install from GitHub/skills.sh/well-known endpoints
- [ ] **SKILL-09**: Trust levels for installed skills: builtin (shipped), official (optional-skills/), trusted (known repos), community (security-gated)
- [x] **SKILL-10**: Platform-specific skills restricted via platforms field (macos/linux/windows); hidden on incompatible platforms
- [x] **SKILL-11**: Skill env vars automatically passed through to execute_code and terminal sandboxes when set
- [x] **SKILL-12**: Slash command router intercepts / prefixed messages before AgentLoop — platform-agnostic (CLI + gateway + ACP)
- [x] **SKILL-13**: Core slash commands implemented: /help, /reset, /personality, /skills, /memory, /sessions, /search, /model, /stop, /new
- [x] **SKILL-14**: Slash command resolution supports aliases and prefix matching via resolve_command()

### Tool Registry

- [ ] **TOOL-01**: Tool trait includes is_available() check function; tools silently excluded from schema when prerequisites (env vars, config) are absent
- [ ] **TOOL-02**: Tools organized into named toolsets with platform-specific presets
- [ ] **TOOL-03**: Tool registration happens at import/init time via registry pattern — adding a tool requires registration call only
- [ ] **TOOL-04**: Agent-intercepted tools (memory, session_search, delegate_task, todo) handled before registry dispatch
- [ ] **TOOL-05**: Setup wizard checks tool availability and guides users through missing prerequisites (API keys, env vars)

### Provider Resolution

- [x] **PROV-01**: Shared runtime resolver serves CLI, gateway, cron, ACP, and auxiliary calls — maps (provider, model) to (api_mode, api_key, base_url)
- [x] **PROV-02**: Three API modes supported: chat_completions (OpenAI-compatible), codex_responses (Codex/Responses API), anthropic_messages (native Anthropic Messages API)
- [x] **PROV-03**: Resolution precedence: explicit request > config.yaml > environment variables > provider defaults
- [ ] **PROV-04**: API keys scoped to their provider's base URL — prevents leaking wrong key to wrong endpoint
- [x] **PROV-05**: Native Anthropic path uses anthropic_adapter for message format conversion with refreshable credential support
- [ ] **PROV-06**: Auxiliary model routing: vision, compression, session search, skills hub, MCP helper tasks can use separate provider/model from main conversational model
- [x] **PROV-07**: Fallback model switching: on primary model failure (429/5xx/401), try configured fallback_providers in order with credential refresh
- [ ] **PROV-08**: Named custom providers configurable in config.yaml for any OpenAI-compatible endpoint
- [x] **PROV-09**: Iteration budget with 2-tier pressure: caution at 70% (consolidate), warning at 90% (respond now), stop at 100%
- [x] **PROV-10**: Budget shared across parent and child agents — subagent consumes from parent's budget

### Gateway Architecture

- [ ] **GW-01**: GatewayRunner architecture: platform adapters produce MessageEvents, agent dispatcher routes to per-session AgentLoop instances
- [ ] **GW-02**: Session key format: agent:main:{platform}:{chat_type}:{chat_id} — constructed via build_session_key()
- [ ] **GW-03**: Two-level message guard: base adapter queues messages and sets interrupt when agent is active; gateway runner intercepts control commands (/stop, /approve, /deny)
- [ ] **GW-04**: Authorization: per-platform allowlists, DM pairing flow with codes, global allow-all flag, default deny
- [x] **GW-05**: Gateway slash command dispatch via resolve_command() with running-agent guard (blocks /model while agent active, bypasses /stop /approve /deny)
- [ ] **GW-06**: Gateway hook lifecycle events: gateway:startup, session:start/end/reset, agent:start/step/end, command:*
- [ ] **GW-07**: Delivery routing: direct reply, home channel, explicit target (telegram:chat_id), cross-platform delivery
- [x] **GW-08**: Cron job deliveries NOT mirrored into gateway session history (prevents message alternation violations)
- [ ] **GW-09**: Token locks via acquire_scoped_lock()/release_scoped_lock() prevent two profiles using same bot token
- [ ] **GW-10**: Background maintenance: cron ticking, session expiry, memory flush on session end/reset, cache refresh
- [x] **GW-11**: Memory provider integration: MemoryManager initialized per session, provider tools routed through handle_tool_call, on_session_end fires cleanup

### CLI & ACP

- [x] **CLI-01**: CLI registers execute_code, hooks, and guardrails (feature parity with gateway)
- [x] **CLI-02**: CLI extension hooks: _get_extra_tui_widgets(), _register_extra_tui_keybindings(), _build_tui_layout_children(), process_command(), _build_tui_style_dict()
- [ ] **CLI-03**: ACP adapter: JSON-RPC stdio server wrapping AgentLoop for VS Code / Zed / JetBrains integration *(v2.1)*
- [ ] **CLI-04**: ACP SessionManager with create/get/remove/fork/list/cleanup operations *(v2.1)*
- [ ] **CLI-05**: ACP event bridge converts AgentLoop callbacks (tool_progress, thinking, reasoning, step, stream_delta) into ACP session_update events *(v2.1)*
- [ ] **CLI-06**: ACP permission bridge maps dangerous command approval to ACP permission requests (allow_once/allow_always/reject) *(v2.1)*
- [ ] **CLI-07**: ACP tool rendering maps Hermes tools to editor-facing content (file diffs, shell commands, text previews) *(v2.1)*
- [ ] **CLI-08**: ACP sessions carry editor cwd bound to session ID for file/terminal tool context *(v2.1)*

### Configuration

- [ ] **CFG-01**: Interactive setup wizard guides first-run configuration (provider selection, API keys, model choice, tool availability)
- [ ] **CFG-02**: hermes config set/get/show for managing config.yaml values
- [ ] **CFG-03**: hermes config migrate scans skills for unconfigured settings and prompts user
- [ ] **CFG-04**: Profile isolation: each profile gets own HERMES_HOME, config, memory, sessions, gateway PID

### Learning Loop

The Learning Loop is the unifying philosophy of v2.1 — Skills + Memory + Session Search are outputs of one continuous self-improvement process. These five reqs land the periodic-nudge + autonomous-skill-creation foundation that hermes-agent exposes as the differentiating feature.

- [ ] **LEARN-01**: Periodic nudge mechanism — at configurable intervals (default 5 minutes) during a session, agent receives an internal system-level prompt asking it to scan recent activity and evaluate whether anything is worth persisting to MEMORY.md/USER.md. Fires without user input. Honors PRMT-06 (mid-session writes persist to disk but do not mutate the active prompt — the new entries take effect at next session start).
- [ ] **LEARN-02**: Memory persistence judgment — during the nudge, agent decides per-item which memory layer information belongs in. Threshold: "important enough to be present in every future conversation" → prompt memory (MEMORY.md/USER.md, slots 5-6); "useful only when topic comes up" → session search (SQLite archive, retrieved on demand). Coordinates with the existing 3,575 char total memory cap (MEM-01 + MEM-02).
- [ ] **LEARN-03**: Autonomous skill creation triggers — at task completion, agent evaluates whether the path is worth documenting using a heuristic: (a) 5+ tool calls, (b) recovery from error, (c) user correction, (d) non-obvious workflow that worked. Any trigger fires → write a SKILL.md.
- [ ] **LEARN-04**: SKILL.md auto-creation format — written to `~/.hermes/skills/` (under a category subdirectory chosen by the agent) following the agentskills.io standard. Frontmatter: `name`, `description`, `version`, `platforms`, `metadata.hermes.{tags, category, fallback_for_toolsets, requires_toolsets}`. Default trust tier on creation: `Self-created` (a new tier added by Phase 28 SKILL-09 work — coordinates with that phase).
- [ ] **LEARN-05**: `skill_manage` tool with 6 actions — `create`, `patch`, `edit`, `delete`, `write_file`, `remove_file`. Agent defaults to `patch` for updates (passes only `old_string` + `new_string`, mirroring the existing memory tool's substring matching pattern from MEM-03). Token-efficient incremental updates; full rewrites reserved for `edit` action only. Coordinates with Phase 25 toolset registry (registers `skill_manage` as a new toolset entry).

## Future Requirements

Deferred to v2.2+. Tracked but not in current roadmap. CLI-03..CLI-08 (ACP adapter) were moved BACK to v2.1 active scope on 2026-04-27 — see "Current Milestone: v2.1 Carry-Overs" at the top of this file.

### v2.2 Reservation: Production Polish

Pre-reserved scope for v2.2 (the milestone after v2.1). Decided 2026-04-27 during v2.1 planning:

- **AUTH-01..N (TBD)**: Credential pools — multi-key rotation, exhaustion handling, `hermes auth add/list/remove/reset` CLI commands. Currently only Anthropic fallback exists.
- **AUTH-?? (TBD)**: Multi-provider OAuth login — `hermes login --provider <name>` for Nous, OpenAI Codex, etc. Currently only Anthropic OAuth via `~/.claude/credentials.json`.
- **UPDT-01 (TBD)**: `hermes update` self-update mechanism (live download/replace; test stubs exist in `crates/ironhermes-hub/tests/update_uninstall_test.rs` but no live logic).
- **UPDT-02 (TBD)**: `hermes uninstall` CLI uninstaller (logic designed, not wired to CLI).
- **ROUTE-01..N (TBD)**: Smart model routing — `smart_model_routing.cheap_model` config for intelligent cheap/expensive model selection based on task complexity.

These will be assigned firm REQ-IDs at v2.2 milestone planning.

### GAP-NEW (parity gaps from v2.1 planning, parked)

Identified 2026-04-27 by IronHermes ↔ hermes-agent parity analysis. Each is observable in the canonical Python `hermes-agent` but absent from the IronHermes Rust port. Reserved for future milestones; assigned firm REQ-IDs when scheduled.

- **VOICE-01..N**: Voice subsystem — STT (faster-whisper local + Groq + OpenAI fallback) + TTS (Edge default + ElevenLabs/OpenAI/Kokoro/Fish). Includes `/voice on|off|tts` slash command and `stt:`/`tts:` config sections. Currently `/voice` is a registered command stub returning "No TTS infrastructure" (see `crates/ironhermes-core/src/commands/handlers.rs` line 93-94).
- **VIS-01..N**: Vision toolset — image analysis tool. No `vision` toolset in tools registry.
- **IMG-01..N**: Image generation toolset. No `image_gen` toolset in tools registry.
- **BROW-01..N**: Browser automation toolset (Browserbase, Camofox, local Chromium). `/browser` command registered but handler is stub.
- **PROF-01..N**: Profile system — `hermes profile list/create/use/delete/show/alias/rename/export/import` with isolated HERMES_HOME per profile (CFG-04 in v2.1 covers config-level only; full profile lifecycle is a separate effort).
- **PLUG-01..N**: Plugin system — `hermes plugins list/install/remove`. Distinct from skills. `/plugins` command registered but handler stub.
- **PAIR-01..N**: Pairing / DM authorization — `hermes pairing list/approve/revoke` for multi-user gateway authorization. Different from `/approve`/`/deny` command-execution approval.
- **INSI-01..N**: Insights / analytics — `hermes insights [--days N]` usage analytics. `/insights` command stub returns "No analytics infrastructure".
- **MOA-01..N**: Mixture of Agents (MoA) toolset — orchestration of multiple model agents per turn.
- **TIRI-01..N**: Tirith security integration — `security.tirith_enabled` config flag (no references in current codebase).
- **HONC-01..N**: Honcho memory cloud-backend integration — `hermes honcho setup/status` CLI (current memory backends: file, SQLite, Grafeo, DuckDB only).
- **COMP-01..N**: Shell completions — `hermes completion bash|zsh` generators.
- **CLAR-01..N**: `clarify` toolset — agent-initiated clarification questions.

### Additional Platforms

- **PLAT-01**: Discord adapter
- **PLAT-02**: Slack adapter
- **PLAT-03**: WhatsApp adapter
- **PLAT-04**: Additional platform adapters (Signal, Matrix, Mattermost, email, SMS, etc.)

### Advanced Memory

- **AMEM-01**: LLM-based context summarization (async background path)
- **AMEM-02**: Additional memory provider backends beyond SQLite/Grafeo/DuckDB
- **AMEM-03**: Context engine plugin system (user-installable engines)

### Advanced Features

- **ADV-01**: Webhook mode for gateway (vs polling) for cloud deployment
- **ADV-02**: MarkdownV2 formatting for Telegram responses
- **ADV-03**: Cross-session performance metrics
- **ADV-04**: Background reflection loops
- **ADV-05**: Context references (@file:, @folder:, @diff, @staged, @git:, @url: inline expansion)

## Out of Scope

Explicitly excluded. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| Multiple simultaneous memory providers | Conflicting facts across providers; synchronization too complex; hermes-agent enforces single-provider |
| Real-time SOUL.md hot-reload mid-session | Invalidates Anthropic cached prefix; causes persona drift; frozen-snapshot pattern is correct |
| Unlimited memory store size | Unbounded files bloat system prompt; injection risk grows with size; bounded stores + provider trait is the answer |
| Slash commands for every tool | Duplicates tool interface; two code paths; slash commands are session-control only |
| Web UI | CLI, Telegram, and ACP cover primary use cases |
| Multi-user auth system | Single-operator deployment; gateway authorization handles access control |
| Plugin/extension system (dynamic loading) | Tools compiled-in; skills are the extension mechanism; dynamic loading is premature |
| LLM-based compression in hot path | Adds full round-trip at worst moment; local prune first, LLM summarization deferred |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| MEM-01 | Phase 17 | Complete |
| MEM-02 | Phase 17 | Complete |
| MEM-03 | Phase 17 | Complete |
| MEM-04 | Phase 17 | Complete |
| MEM-05 | Phase 17 | Complete |
| MEM-06 | Phase 15 | Complete |
| MEM-07 | Phase 11 | Complete |
| MEM-08 | Phase 11 | Complete |
| MEM-09 | Phase 17 | Complete |
| MEM-10 | Phase 17 | Complete |
| MEM-11 | Phase 17 | Complete |
| MEM-12 | Phase 11 | Complete |
| MEM-13 | Phase 17 | Complete |
| SESS-01 | Phase 13 | Complete |
| SESS-02 | Phase 13 | Complete |
| SESS-03 | Phase 13 | Complete |
| SESS-04 | Phase 13 | Complete |
| SESS-05 | Phase 13 | Complete |
| SESS-06 | Phase 13 | Complete |
| SESS-07 | Phase 13 | Complete |
| SESS-08 | Phase 13 | Complete |
| SESS-09 | Phase 13 | Complete |
| SESS-10 | Phase 13 | Complete |
| SESS-11 | Phase 13 | Complete |
| PRMT-01 | Phase 15 | Pending |
| PRMT-02 | Phase 15 | Pending |
| PRMT-03 | Phase 15 | Pending |
| PRMT-04 | Phase 15 | Pending |
| PRMT-05 | Phase 15 | Pending |
| PRMT-06 | Phase 15 | Pending |
| PRMT-07 | Phase 15 | Pending |
| PRMT-08 | Phase 27 | Pending |
| PRMT-09 | Phase 27 | Pending |
| PRMT-10 | Phase 18 | Complete |
| PRMT-11 | Phase 18 | Complete |
| PRMT-12 | Phase 18 | Complete |
| PRMT-13 | Phase 18 | Complete |
| PRMT-14 | Phase 18 | Complete |
| PRMT-15 | Phase 18 | Complete |
| PRMT-16 | Phase 18 | Complete |
| CTX-01 | Phase 14 | Pending |
| CTX-02 | Phase 14 | Pending |
| CTX-03 | Phase 14 | Pending |
| CTX-04 | Phase 14 | Pending |
| CTX-05 | Phase 14 | Pending |
| CTX-06 | Phase 14 | Pending |
| CTX-07 | Phase 14 | Pending |
| SKILL-01 | Phase 19 | Complete |
| SKILL-02 | Phase 19 | Complete |
| SKILL-03 | Phase 19 | Complete |
| SKILL-04 | Phase 19 | Complete |
| SKILL-05 | Phase 19 | Complete |
| SKILL-06 | Phase 19 | Complete |
| SKILL-07 | Phase 19 | Complete |
| SKILL-08 | Phase 19.1 | Complete |
| SKILL-09 | Phase 28 | Pending |
| SKILL-10 | Phase 19 | Complete |
| SKILL-11 | Phase 19 | Complete |
| SKILL-12 | Phase 20 | Complete |
| SKILL-13 | Phase 20 | Complete |
| SKILL-14 | Phase 20 | Complete |
| TOOL-01 | Phase 25 | Pending |
| TOOL-02 | Phase 25 | Pending |
| TOOL-03 | Phase 25 | Pending |
| TOOL-04 | Phase 25 | Pending |
| TOOL-05 | Phase 25 | Pending |
| PROV-01 | Phase 12 | Complete |
| PROV-02 | Phase 12 | Complete |
| PROV-03 | Phase 12 | Complete |
| PROV-04 | Phase 26 | Pending |
| PROV-05 | Phase 12 | Complete |
| PROV-06 | Phase 26 | Pending |
| PROV-07 | Phase 12 | Complete |
| PROV-08 | Phase 26 | Pending |
| PROV-09 | Phase 21.7 (was 12) | Complete |
| PROV-10 | Phase 21.7 (was 12) | Complete |
| GW-01 | Phase 29 | Pending |
| GW-02 | Phase 29 | Pending |
| GW-03 | Phase 29 | Pending |
| GW-04 | Phase 29 | Pending |
| GW-05 | Phase 21.1 (was 21) | Complete |
| GW-06 | Phase 29 | Pending |
| GW-07 | Phase 29 | Pending |
| GW-08 | Phase 22.4.2.1/22.4.2.2 (was 21) | Complete |
| GW-09 | Phase 29 | Pending |
| GW-10 | Phase 29 | Pending |
| GW-11 | Phase 21.4 (was 21) | Complete |
| CLI-01 | Phase 22 | Complete |
| CLI-02 | Phase 22.1 | Complete |
| CLI-03 | Phase 30 | Pending |
| CLI-04 | Phase 30 | Pending |
| CLI-05 | Phase 31 | Pending |
| CLI-06 | Phase 31 | Pending |
| CLI-07 | Phase 31 | Pending |
| CLI-08 | Phase 30 | Pending |
| CFG-01 | Phase 23 | Pending |
| CFG-02 | Phase 23 | Pending |
| CFG-03 | Phase 23 | Pending |
| CFG-04 | Phase 24 | Pending |
| LEARN-01 | Phase 32 | Pending |
| LEARN-02 | Phase 32 | Pending |
| LEARN-03 | Phase 33 | Pending |
| LEARN-04 | Phase 33 | Pending |
| LEARN-05 | Phase 33 | Pending |

**Coverage:**
- v2.0 requirements: 99 total (closed 2026-04-27 as `tech_debt`; 77 satisfied / 16 carried over to v2.1 / 6 ACP-specific carried over to v2.1)
- v2.1 active: 34 reqs (29 carry-overs across 7 categories + 5 NEW Learning Loop reqs across 1 new category)
  - Carry-overs: CLI-03..08 + PRMT-08/09 + TOOL-01..05 + PROV-04/06/08 + SKILL-09 + GW-01..04, GW-06, GW-07, GW-09, GW-10 + CFG-01..04
  - Learning Loop: LEARN-01..05
- Mapped to v2.1 phases: 34/34 (Phases 23-33)
- v2.2 reservation: ~5 categories (AUTH credential pools, OAuth multi-provider, UPDT self-update + uninstall, ROUTE smart routing) — REQ-IDs assigned at v2.2 planning
- Future Requirements (parked GAP-NEW from v2.1 planning): VOICE-*, VIS-*, IMG-*, BROW-*, PROF-*, PLUG-*, PAIR-*, INSI-*, MOA-*, TIRI-*, HONC-*, COMP-*, CLAR-* (13 categories, REQ-IDs assigned when scheduled)

---
*Requirements defined: 2026-04-11*
*Last updated: 2026-04-27 — v2.1 expanded scope: + 5 Learning Loop reqs (LEARN-01..05) + 2 phases (32 + 33). Total: 34 reqs across 11 phases.*
