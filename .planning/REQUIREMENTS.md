# Requirements: IronHermes

**Defined:** 2026-04-01
**Updated:** 2026-04-08 (v1.1 requirements added)
**Core Value:** A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.

## v1.0 Requirements (Complete)

### Context Files

- [x] **CTX-01**: Agent loads SOUL.md from IRONHERMES_HOME into system prompt as personality/identity
- [x] **CTX-02**: Agent loads AGENTS.md from IRONHERMES_HOME into system prompt as capability definitions
- [x] **CTX-03**: Agent loads project-level context files from working directory (.ironhermes/ or configurable paths)
- [x] **CTX-04**: Context files are loaded once at session start (frozen-snapshot pattern for LLM cache stability)
- [x] **CTX-05**: Priority-based context assembly: SOUL.md > project context > AGENTS.md (matching hermes-agent order)

### Telegram Gateway

- [x] **TG-01**: Telegram long polling runs continuously, receives messages, and dispatches to agent loop
- [x] **TG-02**: Agent responses (including tool use results) are sent back to the originating Telegram chat
- [x] **TG-03**: Streaming responses: progressive message editing as LLM chunks arrive
- [x] **TG-04**: Session management: chat_id maps to persistent conversation history via SessionStore
- [x] **TG-05**: Graceful shutdown: CancellationToken-based cooperative shutdown of polling and in-flight agent runs
- [x] **TG-06**: Concurrency limiting: Semaphore bounds maximum concurrent agent runs (default 4-8)
- [x] **TG-07**: Error recovery: exponential backoff on polling failures, automatic reconnection
- [x] **TG-08**: Typing indicator sent while agent is processing

### Async Infrastructure

- [x] **ASYNC-01**: SessionStore wrapped in Arc<RwLock> for safe sharing across tokio tasks
- [x] **ASYNC-02**: ToolRegistry wrapped in Arc for sharing across concurrent agent runs
- [x] **ASYNC-03**: Supervisor pattern for gateway subsystems with restart on transient failures

### Self-Improvement

- [x] **SELF-01**: Agent can read its own context files (SOUL.md, AGENTS.md) via existing read_file tool
- [x] **SELF-02**: Agent can edit its own context files via existing write_file/patch tools
- [x] **SELF-03**: Security scanning on all context file writes (injection detection, exfiltration patterns, invisible Unicode)
- [x] **SELF-04**: Memory subsystem: bounded declarative facts stored in MEMORY.md, loaded into context
- [x] **SELF-05**: Memory tool: agent can save, query, and forget facts via a dedicated memory tool
- [x] **SELF-06**: Atomic file I/O for all context/memory writes (temp file + rename, matching cron pattern)

### Web Scraping

- [x] **WEB-01**: web_read tool: fetch URL content via Firecrawl scrape API, return extracted text
- [x] **WEB-02**: SSRF protection: validate URLs before fetching (block private IPs, localhost, internal ranges)
- [x] **WEB-03**: Content truncation: cap extracted text to context-window-safe length (configurable, default 50K chars)
- [x] **WEB-04**: Local HTML fallback: scraper crate for content extraction when Firecrawl is unavailable

### Security

- [x] **SEC-01**: Port url_safety.py SSRF validation from hermes-agent to Rust
- [x] **SEC-02**: Regex-based threat scanning for context file writes (prevent prompt injection via self-modification)
- [x] **SEC-03**: Rate limiting on Telegram message processing to prevent abuse

## v1.1 Requirements

Requirements for the Automation milestone. Each maps to roadmap phases.

### Scheduled Tasks

- [ ] **SCHED-01**: User can create scheduled tasks using natural language ("every morning at 9am") which the agent interprets to cron expressions
- [ ] **SCHED-02**: User can pause, resume, and edit existing scheduled tasks without delete+recreate
- [ ] **SCHED-03**: User can attach named skills to scheduled tasks for reliable, inspectable recurring jobs
- [ ] **SCHED-04**: Scheduled task output routes to configured platform (Telegram, CLI, or webhook)

### Event Hooks

- [ ] **HOOK-01**: Agent lifecycle events (message received, tool called, response sent) are logged via a hook registry
- [ ] **HOOK-02**: Guardrail hooks can intercept and block tool calls before dispatch (e.g., block terminal in untrusted contexts)
- [ ] **HOOK-03**: Hook events can be forwarded to external HTTP endpoints via webhook delivery

### Skills System

- [ ] **SKILL-01**: Agent discovers skill documents from skills directories (~/.ironhermes/skills/, ~/.agents/skills/, project-level)
- [ ] **SKILL-02**: Skills use progressive disclosure — catalog (name+description) loaded at session start, full content loaded only on activation
- [ ] **SKILL-03**: Skill documents follow the agentskills.io open standard (SKILL.md with name/description frontmatter, Markdown body)
- [ ] **SKILL-04**: Agent can list, view, and activate skills via a dedicated skills tool during conversation
- [ ] **SKILL-05**: Skills with a `platforms` frontmatter field are filtered at discovery time — skills listing platforms that do not match the current OS are skipped (agentskills.io spec + hermes-agent parity)
- [ ] **SKILL-06**: Extended frontmatter fields are parsed and stored: `compatibility`, `allowed-tools`, and `metadata` (including `metadata.hermes.*` extensions) for hermes-agent backward compat
- [ ] **SKILL-07**: SKILL.md name validation enforced at load time: lowercase alphanumeric + hyphens, 1-64 chars; description 1-1024 chars; directory name must match skill name (warn-but-load on mismatch)
- [ ] **SKILL-08**: `SkillsConfig` section in `config.yaml` allows configuring custom scan paths beyond the three default directories
- [ ] **SKILL-13**: Slash-command integration — `/skill-name` in CLI chat activates the named skill directly, injecting its body before the next LLM call (backlog)

> **SKILL-09** was moved to **v2 Requirements** during v1.1 gap closure (2026-04-09). It was explicitly deferred in Phase 07.2 per `07.2-CONTEXT.md D-01` and will not ship in v1.1.

### Code Execution

- [ ] **EXEC-01**: Agent can execute Python scripts in an isolated child process via an execute_code tool
- [ ] **EXEC-02**: Python scripts can call agent tools (web_search, read_file, etc.) via JSON-RPC over a socket
- [ ] **EXEC-03**: Child process environment has API keys and secrets stripped for safety
- [ ] **EXEC-04**: Code execution enforces timeout (5 min), call limit (50), and stdout cap (50KB)

### Subagent Delegation

- [ ] **AGENT-01**: Agent can delegate tasks to child agents via a delegate_task tool with isolated context
- [ ] **AGENT-02**: Parent agent specifies which tools the child agent can use via a filtered ToolRegistry
- [ ] **AGENT-03**: Maximum 3 concurrent subagents enforced via semaphore
- [ ] **AGENT-04**: Each subagent gets its own terminal session scope to prevent state bleed
- [ ] **AGENT-05**: Recursive delegation is prevented — delegate_task is excluded from child agent toolsets

### Batch Processing

- [ ] **BATCH-01**: User can run batch prompt execution from JSONL input with semaphore-bounded parallel workers
- [ ] **BATCH-02**: Batch output is in ShareGPT format (human/assistant/tool roles) for HuggingFace compatibility
- [ ] **BATCH-03**: Batch jobs support checkpointing — survive restarts by tracking completed entries by content hash
- [ ] **BATCH-04**: Automatic quality filtering discards trajectories with hallucinated tool names or missing reasoning

## v2 Requirements

### Self-Improvement (Advanced)

- **SELF-07**: Version history for context files with rollback capability
- **SELF-08**: Skills subsystem: procedural knowledge stored in SKILL.md directories; agent can create/edit/delete its own skills via a `skill_manage` tool (create, edit, patch, delete, write_file, remove_file actions — hermes-agent `skill_manager_tool.py` parity)
- **SELF-09**: Session-end reflection: agent evaluates performance and optionally updates context
- **SELF-10**: Skill auto-creation: agent saves multi-step procedures as reusable skills after 5+ tool calls; `skill_manage(action='create')` is the write mechanism

### Skills Hub (Advanced)

- **SKILL-09**: Skills that declare required env vars via `metadata.hermes.config` prompt the user at first activation if those vars are absent (setup-needed flow) — _moved from v1.1 during gap closure (2026-04-09); deferred per Phase 07.2 decision D-01_
- **SKILL-10**: Skills Hub with multi-source registry — install skills from GitHub repos, local paths, and remote tarballs via `GitHubSource` adapter and hub lock file tracking provenance
- **SKILL-11**: Update lifecycle — manifest-based hash tracking for installed skills; `install`, `update`, `remove` CLI subcommands; bundled-skill seeding on first run
- **SKILL-12**: Trust levels (builtin / trusted / community / agent-created) and security scanning of externally-sourced skills (prompt injection, exfiltration, destructive command detection) before installation

### Web Scraping (Advanced)

- **WEB-05**: LLM-based content summarization for long pages (reuse existing LLM client)
- **WEB-06**: Jina Reader API as backup content extraction service
- **WEB-07**: web_crawl tool: follow links and extract content from multiple pages
- **WEB-08**: Website blocklist policy (configurable domains to never scrape)

### Platform Expansion

- **PLAT-01**: Discord adapter with bot token authentication
- **PLAT-02**: Slack adapter with OAuth/Bot token
- **PLAT-03**: Group chat support: respond only when @mentioned

### Observability

- **OBS-01**: Structured logging with tracing spans per agent run
- **OBS-02**: Metrics: agent runs, tool calls, LLM latency, token usage
- **OBS-03**: Health check endpoint for monitoring

## Out of Scope

| Feature | Reason |
|---------|--------|
| Web UI / dashboard | CLI and Telegram are the primary interfaces; web adds frontend complexity |
| Multi-user authentication | Single-operator deployment; Telegram auth handles user identity |
| Dynamic plugin/extension loading | Tools are compiled-in; dynamic loading is premature abstraction |
| Webhook mode for Telegram | Long polling is simpler and sufficient for single-instance deployment |
| Database-backed memory | File-based memory matches hermes-agent pattern and is git-trackable |
| JavaScript rendering for scraping | Firecrawl API handles JS server-side; no need for headless browser |
| Per-prompt container images for batch | Enormous operational complexity (Docker daemon dependency) |
| Discord/Slack delivery for scheduled tasks | Out of scope until Telegram is solid; delivery abstraction left open |
| Persistent subagent state across sessions | Subagents are ephemeral work units; parent handles continuity |
| Interactive subagent communication | Subagents receive a task and return a result; no mid-task steering |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| CTX-01 | Phase 1 | Complete |
| CTX-02 | Phase 1 | Complete |
| CTX-03 | Phase 1 | Complete |
| CTX-04 | Phase 1 | Complete |
| CTX-05 | Phase 1 | Complete |
| ASYNC-01 | Phase 2 | Complete |
| ASYNC-02 | Phase 2 | Complete |
| ASYNC-03 | Phase 2 | Complete |
| TG-01 | Phase 2 | Complete |
| TG-02 | Phase 2 | Complete |
| TG-03 | Phase 2 | Complete |
| TG-04 | Phase 2 | Complete |
| TG-05 | Phase 2 | Complete |
| TG-06 | Phase 2 | Complete |
| TG-07 | Phase 2 | Complete |
| TG-08 | Phase 2 | Complete |
| SEC-01 | Phase 3 | Complete |
| SEC-02 | Phase 3 | Complete |
| SEC-03 | Phase 3 | Complete |
| SELF-01 | Phase 3 | Complete |
| SELF-02 | Phase 3 | Complete |
| SELF-03 | Phase 3 | Complete |
| SELF-04 | Phase 3 | Complete |
| SELF-05 | Phase 3 | Complete |
| SELF-06 | Phase 3 | Complete |
| WEB-01 | Phase 4 | Complete |
| WEB-02 | Phase 4 | Complete |
| WEB-03 | Phase 4 | Complete |
| WEB-04 | Phase 4 | Complete |
| SCHED-01 | Phase 5 → 07.3 (gap closure) | Pending |
| SCHED-02 | Phase 5 | Pending (verification pending) |
| SCHED-03 | Phase 5 → 07.3 (gap closure) | Pending |
| SCHED-04 | Phase 5 → 07.3 (gap closure) | Pending |
| HOOK-01 | Phase 6 → 07.3 (cron tick gap) + 07.4 (ordering) | Pending |
| HOOK-02 | Phase 6 | Pending |
| HOOK-03 | Phase 6 | Pending |
| SKILL-01 | Phase 7 | Pending |
| SKILL-02 | Phase 7 | Pending |
| SKILL-03 | Phase 7 | Pending |
| SKILL-04 | Phase 7 | Pending |
| SKILL-05 | Phase 07.2 | Pending |
| SKILL-06 | Phase 07.2 → 07.5 (enforcement) | Pending |
| SKILL-07 | Phase 07.2 | Pending |
| SKILL-08 | Phase 07.2 | Pending |
| SKILL-09 | v2 (moved from 07.2 during gap closure 2026-04-09) | Deferred |
| SKILL-10 | Phase v2 | Pending |
| SKILL-11 | Phase v2 | Pending |
| SKILL-12 | Phase v2 | Pending |
| SKILL-13 | Backlog | Pending |
| EXEC-01 | Phase 8 | Pending |
| EXEC-02 | Phase 8 | Pending |
| EXEC-03 | Phase 8 | Pending |
| EXEC-04 | Phase 8 | Pending |
| AGENT-01 | Phase 9 | Pending |
| AGENT-02 | Phase 9 | Pending |
| AGENT-03 | Phase 9 | Pending |
| AGENT-04 | Phase 9 | Pending |
| AGENT-05 | Phase 9 | Pending |
| BATCH-01 | Phase 10 | Pending |
| BATCH-02 | Phase 10 | Pending |
| BATCH-03 | Phase 10 | Pending |
| BATCH-04 | Phase 10 | Pending |

**Coverage:**
- v1.0 requirements: 29 total (all complete)
- v1.1 requirements: 28 total (23 original + 5 from Phase 07.1 gap analysis: SKILL-05..SKILL-08, SKILL-13; SKILL-09 moved to v2 during v1.1 gap closure)
- v2 requirements: 4 (SKILL-09 relocated + SKILL-10, SKILL-11, SKILL-12 from Phase 07.1)
- Mapped to phases: 29 (v1.0) + 28 (v1.1) + v2 additions
- Unmapped: 0

---
*Requirements defined: 2026-04-01*
*Last updated: 2026-04-09 after v1.1 milestone audit — added gap closure phases 07.3/07.4/07.5, relocated SKILL-09 to v2*
