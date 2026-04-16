# Requirements: IronHermes

**Defined:** 2026-04-11
**Core Value:** A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.

## v2.0 Requirements

Requirements for v2.0: Intelligence & Identity. Each maps to roadmap phases.

### Memory

- [x] **MEM-01**: Agent can add, replace, and remove entries in bounded MEMORY.md store (2200 char limit) via memory tool
- [x] **MEM-02**: Agent can add, replace, and remove entries in bounded USER.md store (1375 char limit) via memory tool
- [x] **MEM-03**: Memory tool supports substring matching for replace/remove operations (old_text identifies target entry)
- [x] **MEM-04**: Memory stores display capacity usage in system prompt header (e.g., "67% — 1,474/2,200 chars")
- [x] **MEM-05**: Memory entries are security scanned for injection/exfiltration patterns before acceptance
- [ ] **MEM-06**: Memory snapshots are frozen at session start and injected into system prompt — mid-session writes persist to disk but do not mutate the active prompt
- [x] **MEM-07**: MemoryProvider trait defines lifecycle hooks (initialize, prefetch, sync_turn, on_session_end, shutdown) with Send + Sync + 'static bounds
- [x] **MEM-08**: Built-in file-based MemoryStore implements MemoryProvider as the default backend
- [x] **MEM-09**: SQLite memory provider stores facts with FTS5 search capability
- [x] **MEM-10**: Grafeo graph database memory provider stores facts as nodes/edges with relationship queries (feature-gated build)
- [x] **MEM-11**: DuckDB memory provider stores facts with analytical query capability (feature-gated build, async bridge for !Send Connection)
- [ ] **MEM-12**: Only one external memory provider can be active at a time — single-provider selection via config
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
- [ ] **PRMT-10**: Context pressure warnings emitted at 85% of compression threshold
- [ ] **PRMT-11**: Dual-mode context compression: agent ContextCompressor at 50% threshold (configurable), gateway hygiene at 85% threshold
- [ ] **PRMT-12**: ContextEngine trait enables pluggable compression strategies (default: local prune + structured summary)
- [ ] **PRMT-13**: Compression preserves tool_call/tool_result pairs atomically — never splits a pair across summary boundary
- [ ] **PRMT-14**: Compression protects first N messages (system + first exchange) and last N messages (configurable, default 20)
- [ ] **PRMT-15**: Iterative re-compression updates previous summary rather than summarizing from scratch
- [ ] **PRMT-16**: Memory is flushed to disk before compression to prevent data loss

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
- [ ] **SKILL-08**: Skills Hub: publish skills to external repos, install from GitHub/skills.sh/well-known endpoints
- [ ] **SKILL-09**: Trust levels for installed skills: builtin (shipped), official (optional-skills/), trusted (known repos), community (security-gated)
- [x] **SKILL-10**: Platform-specific skills restricted via platforms field (macos/linux/windows); hidden on incompatible platforms
- [x] **SKILL-11**: Skill env vars automatically passed through to execute_code and terminal sandboxes when set
- [ ] **SKILL-12**: Slash command router intercepts / prefixed messages before AgentLoop — platform-agnostic (CLI + gateway + ACP)
- [ ] **SKILL-13**: Core slash commands implemented: /help, /reset, /personality, /skills, /memory, /sessions, /search, /model, /stop, /new
- [ ] **SKILL-14**: Slash command resolution supports aliases and prefix matching via resolve_command()

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
- [ ] **PROV-09**: Iteration budget with 2-tier pressure: caution at 70% (consolidate), warning at 90% (respond now), stop at 100%
- [ ] **PROV-10**: Budget shared across parent and child agents — subagent consumes from parent's budget

### Gateway Architecture

- [ ] **GW-01**: GatewayRunner architecture: platform adapters produce MessageEvents, agent dispatcher routes to per-session AgentLoop instances
- [ ] **GW-02**: Session key format: agent:main:{platform}:{chat_type}:{chat_id} — constructed via build_session_key()
- [ ] **GW-03**: Two-level message guard: base adapter queues messages and sets interrupt when agent is active; gateway runner intercepts control commands (/stop, /approve, /deny)
- [ ] **GW-04**: Authorization: per-platform allowlists, DM pairing flow with codes, global allow-all flag, default deny
- [ ] **GW-05**: Gateway slash command dispatch via resolve_command() with running-agent guard (blocks /model while agent active, bypasses /stop /approve /deny)
- [ ] **GW-06**: Gateway hook lifecycle events: gateway:startup, session:start/end/reset, agent:start/step/end, command:*
- [ ] **GW-07**: Delivery routing: direct reply, home channel, explicit target (telegram:chat_id), cross-platform delivery
- [ ] **GW-08**: Cron job deliveries NOT mirrored into gateway session history (prevents message alternation violations)
- [ ] **GW-09**: Token locks via acquire_scoped_lock()/release_scoped_lock() prevent two profiles using same bot token
- [ ] **GW-10**: Background maintenance: cron ticking, session expiry, memory flush on session end/reset, cache refresh
- [ ] **GW-11**: Memory provider integration: MemoryManager initialized per session, provider tools routed through handle_tool_call, on_session_end fires cleanup

### CLI & ACP

- [ ] **CLI-01**: CLI registers execute_code, hooks, and guardrails (feature parity with gateway)
- [ ] **CLI-02**: CLI extension hooks: _get_extra_tui_widgets(), _register_extra_tui_keybindings(), _build_tui_layout_children(), process_command(), _build_tui_style_dict()
- [ ] **CLI-03**: ACP adapter: JSON-RPC stdio server wrapping AgentLoop for VS Code / Zed / JetBrains integration
- [ ] **CLI-04**: ACP SessionManager with create/get/remove/fork/list/cleanup operations
- [ ] **CLI-05**: ACP event bridge converts AgentLoop callbacks (tool_progress, thinking, reasoning, step, stream_delta) into ACP session_update events via run_coroutine_threadsafe
- [ ] **CLI-06**: ACP permission bridge maps dangerous command approval to ACP permission requests (allow_once/allow_always/reject)
- [ ] **CLI-07**: ACP tool rendering maps Hermes tools to editor-facing content (file diffs, shell commands, text previews)
- [ ] **CLI-08**: ACP sessions carry editor cwd bound to session ID for file/terminal tool context

### Configuration

- [ ] **CFG-01**: Interactive setup wizard guides first-run configuration (provider selection, API keys, model choice, tool availability)
- [ ] **CFG-02**: hermes config set/get/show for managing config.yaml values
- [ ] **CFG-03**: hermes config migrate scans skills for unconfigured settings and prompts user
- [ ] **CFG-04**: Profile isolation: each profile gets own HERMES_HOME, config, memory, sessions, gateway PID

## Future Requirements

Deferred to v2.1+. Tracked but not in current roadmap.

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
| MEM-06 | Phase 15 | Pending |
| MEM-07 | Phase 11 | Complete |
| MEM-08 | Phase 11 | Complete |
| MEM-09 | Phase 17 | Complete |
| MEM-10 | Phase 17 | Complete |
| MEM-11 | Phase 17 | Complete |
| MEM-12 | Phase 11 | Pending |
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
| PRMT-08 | Phase 16 | Pending |
| PRMT-09 | Phase 16 | Pending |
| PRMT-10 | Phase 18 | Pending |
| PRMT-11 | Phase 18 | Pending |
| PRMT-12 | Phase 18 | Pending |
| PRMT-13 | Phase 18 | Pending |
| PRMT-14 | Phase 18 | Pending |
| PRMT-15 | Phase 18 | Pending |
| PRMT-16 | Phase 18 | Pending |
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
| SKILL-08 | Phase 19.1 | Pending |
| SKILL-09 | Phase 19.1 | Pending |
| SKILL-10 | Phase 19 | Complete |
| SKILL-11 | Phase 19 | Complete |
| SKILL-12 | Phase 20 | Pending |
| SKILL-13 | Phase 20 | Pending |
| SKILL-14 | Phase 20 | Pending |
| TOOL-01 | Phase 20 | Pending |
| TOOL-02 | Phase 20 | Pending |
| TOOL-03 | Phase 20 | Pending |
| TOOL-04 | Phase 20 | Pending |
| TOOL-05 | Phase 20 | Pending |
| PROV-01 | Phase 12 | Complete |
| PROV-02 | Phase 12 | Complete |
| PROV-03 | Phase 12 | Complete |
| PROV-04 | Phase 12 | Pending |
| PROV-05 | Phase 12 | Complete |
| PROV-06 | Phase 12 | Pending |
| PROV-07 | Phase 12 | Complete |
| PROV-08 | Phase 12 | Pending |
| PROV-09 | Phase 12 | Pending |
| PROV-10 | Phase 12 | Pending |
| GW-01 | Phase 21 | Pending |
| GW-02 | Phase 21 | Pending |
| GW-03 | Phase 21 | Pending |
| GW-04 | Phase 21 | Pending |
| GW-05 | Phase 21 | Pending |
| GW-06 | Phase 21 | Pending |
| GW-07 | Phase 21 | Pending |
| GW-08 | Phase 21 | Pending |
| GW-09 | Phase 21 | Pending |
| GW-10 | Phase 21 | Pending |
| GW-11 | Phase 21 | Pending |
| CLI-01 | Phase 22 | Pending |
| CLI-02 | Phase 22 | Pending |
| CLI-03 | Phase 22 | Pending |
| CLI-04 | Phase 22 | Pending |
| CLI-05 | Phase 22 | Pending |
| CLI-06 | Phase 22 | Pending |
| CLI-07 | Phase 22 | Pending |
| CLI-08 | Phase 22 | Pending |
| CFG-01 | Phase 23 | Pending |
| CFG-02 | Phase 23 | Pending |
| CFG-03 | Phase 23 | Pending |
| CFG-04 | Phase 23 | Pending |

**Coverage:**
- v2.0 requirements: 99 total
- Mapped to phases: 99
- Unmapped: 0

---
*Requirements defined: 2026-04-11*
*Last updated: 2026-04-11 after roadmap creation (v2.0 phases 11-23)*
