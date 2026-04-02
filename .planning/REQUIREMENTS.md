# Requirements: IronHermes

**Defined:** 2026-04-01
**Core Value:** A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.

## v1 Requirements

### Context Files

- [ ] **CTX-01**: Agent loads SOUL.md from IRONHERMES_HOME into system prompt as personality/identity
- [ ] **CTX-02**: Agent loads AGENTS.md from IRONHERMES_HOME into system prompt as capability definitions
- [ ] **CTX-03**: Agent loads project-level context files from working directory (.ironhermes/ or configurable paths)
- [ ] **CTX-04**: Context files are loaded once at session start (frozen-snapshot pattern for LLM cache stability)
- [ ] **CTX-05**: Priority-based context assembly: SOUL.md > project context > AGENTS.md (matching hermes-agent order)

### Telegram Gateway

- [ ] **TG-01**: Telegram long polling runs continuously, receives messages, and dispatches to agent loop
- [ ] **TG-02**: Agent responses (including tool use results) are sent back to the originating Telegram chat
- [ ] **TG-03**: Streaming responses: progressive message editing as LLM chunks arrive
- [ ] **TG-04**: Session management: chat_id maps to persistent conversation history via SessionStore
- [ ] **TG-05**: Graceful shutdown: CancellationToken-based cooperative shutdown of polling and in-flight agent runs
- [ ] **TG-06**: Concurrency limiting: Semaphore bounds maximum concurrent agent runs (default 4-8)
- [ ] **TG-07**: Error recovery: exponential backoff on polling failures, automatic reconnection
- [ ] **TG-08**: Typing indicator sent while agent is processing

### Async Infrastructure

- [ ] **ASYNC-01**: SessionStore wrapped in Arc<RwLock> for safe sharing across tokio tasks
- [ ] **ASYNC-02**: ToolRegistry wrapped in Arc for sharing across concurrent agent runs
- [ ] **ASYNC-03**: Supervisor pattern for gateway subsystems with restart on transient failures

### Self-Improvement

- [ ] **SELF-01**: Agent can read its own context files (SOUL.md, AGENTS.md) via existing read_file tool
- [ ] **SELF-02**: Agent can edit its own context files via existing write_file/patch tools
- [ ] **SELF-03**: Security scanning on all context file writes (injection detection, exfiltration patterns, invisible Unicode)
- [ ] **SELF-04**: Memory subsystem: bounded declarative facts stored in MEMORY.md, loaded into context
- [ ] **SELF-05**: Memory tool: agent can save, query, and forget facts via a dedicated memory tool
- [ ] **SELF-06**: Atomic file I/O for all context/memory writes (temp file + rename, matching cron pattern)

### Web Scraping

- [ ] **WEB-01**: web_read tool: fetch URL content via Firecrawl scrape API, return extracted text
- [ ] **WEB-02**: SSRF protection: validate URLs before fetching (block private IPs, localhost, internal ranges)
- [ ] **WEB-03**: Content truncation: cap extracted text to context-window-safe length (configurable, default 50K chars)
- [ ] **WEB-04**: Local HTML fallback: scraper crate for content extraction when Firecrawl is unavailable

### Security

- [ ] **SEC-01**: Port url_safety.py SSRF validation from hermes-agent to Rust
- [ ] **SEC-02**: Regex-based threat scanning for context file writes (prevent prompt injection via self-modification)
- [ ] **SEC-03**: Rate limiting on Telegram message processing to prevent abuse

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

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| CTX-01 | Phase 1 | Pending |
| CTX-02 | Phase 1 | Pending |
| CTX-03 | Phase 1 | Pending |
| CTX-04 | Phase 1 | Pending |
| CTX-05 | Phase 1 | Pending |
| ASYNC-01 | Phase 2 | Pending |
| ASYNC-02 | Phase 2 | Pending |
| ASYNC-03 | Phase 2 | Pending |
| TG-01 | Phase 2 | Pending |
| TG-02 | Phase 2 | Pending |
| TG-03 | Phase 2 | Pending |
| TG-04 | Phase 2 | Pending |
| TG-05 | Phase 2 | Pending |
| TG-06 | Phase 2 | Pending |
| TG-07 | Phase 2 | Pending |
| TG-08 | Phase 2 | Pending |
| SEC-01 | Phase 3 | Pending |
| SEC-02 | Phase 3 | Pending |
| SEC-03 | Phase 3 | Pending |
| SELF-01 | Phase 3 | Pending |
| SELF-02 | Phase 3 | Pending |
| SELF-03 | Phase 3 | Pending |
| SELF-04 | Phase 3 | Pending |
| SELF-05 | Phase 3 | Pending |
| SELF-06 | Phase 3 | Pending |
| WEB-01 | Phase 4 | Pending |
| WEB-02 | Phase 4 | Pending |
| WEB-03 | Phase 4 | Pending |
| WEB-04 | Phase 4 | Pending |

**Coverage:**
- v1 requirements: 29 total
- Mapped to phases: 29
- Unmapped: 0

---
*Requirements defined: 2026-04-01*
*Last updated: 2026-04-01 after research synthesis*
