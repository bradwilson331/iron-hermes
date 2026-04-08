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
- **SELF-08**: Skills subsystem: procedural knowledge stored in SKILL.md directories
- **SELF-09**: Session-end reflection: agent evaluates performance and optionally updates context
- **SELF-10**: Skill auto-creation: agent saves multi-step procedures as reusable skills after 5+ tool calls

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
| SCHED-01 | Phase 5 | Pending |
| SCHED-02 | Phase 5 | Pending |
| SCHED-03 | Phase 5 | Pending |
| SCHED-04 | Phase 5 | Pending |
| HOOK-01 | Phase 6 | Pending |
| HOOK-02 | Phase 6 | Pending |
| HOOK-03 | Phase 6 | Pending |
| SKILL-01 | Phase 7 | Pending |
| SKILL-02 | Phase 7 | Pending |
| SKILL-03 | Phase 7 | Pending |
| SKILL-04 | Phase 7 | Pending |
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
- v1.1 requirements: 23 total
- Mapped to phases: 29 (v1.0) + 23 (v1.1)
- Unmapped: 0

---
*Requirements defined: 2026-04-01*
*Last updated: 2026-04-08 after v1.1 Automation roadmap created (Phases 5-10)*
