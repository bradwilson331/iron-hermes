# Roadmap: IronHermes

## Overview

IronHermes is a Rust rewrite of hermes-agent that needs four capabilities wired up: context file loading (foundation), Telegram gateway (highest risk async work), self-improvement with security scanning (novel and security-critical), and web scraping tools (well-understood patterns). The roadmap is risk-ordered -- the hardest, most uncertain work ships earliest while respecting hard dependencies. Phase 1 is a low-risk dependency gate; Phase 2 tackles the async concurrency beast; Phase 3 handles the novel self-modification security problem; Phase 4 is the straightforward finish.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3, 4): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

**Strategy:** Risk-ordered. Hardest/most uncertain work earliest, respecting hard dependencies.

- [ ] **Phase 1: Context File Loading** - Load SOUL.md, AGENTS.md, and project context into system prompt with priority assembly
- [ ] **Phase 2: Telegram Gateway** - Wire long polling to agent loop with streaming, concurrency, and error recovery
- [ ] **Phase 3: Self-Improvement + Security** - Agent can safely modify its own context files with security scanning and memory subsystem
- [ ] **Phase 4: Web Scraping Tools** - Agent can fetch and read web page content via Firecrawl API with local fallback

## Phase Details

### Phase 1: Context File Loading
**Goal**: Agent loads personality and project context files into the system prompt so every conversation reflects the configured identity and project awareness
**Depends on**: Nothing (foundation phase -- blocks all other phases)
**Requirements**: CTX-01, CTX-02, CTX-03, CTX-04, CTX-05
**Risk**: LOW -- File I/O and string assembly. PromptBuilder skeleton already exists with `load_context_files()`. Straightforward port of hermes-agent's layered prompt pattern.
**Mitigation**: Existing PromptBuilder has platform-hint scaffolding and path resolution. The work is wiring, not invention.
**Key technical decisions**:
  - SOUL.md loaded from `$IRONHERMES_HOME` (not working directory), matching hermes-agent
  - Priority-based project context: `.hermes.md` > `AGENTS.md` > `CLAUDE.md` > `.cursorrules` (first match wins)
  - Frozen-snapshot pattern: context files loaded once at session start, never mutated mid-session (preserves LLM prefix cache)
  - Content truncation at 20K chars per file
**Estimated complexity**: S
**Success Criteria** (what must be TRUE):
  1. Running `cargo run --bin ironhermes` with a SOUL.md in IRONHERMES_HOME produces agent responses reflecting that personality
  2. AGENTS.md content appears in the system prompt after SOUL.md content
  3. Project-level context files from the working directory are discovered and loaded using the priority chain
  4. Context files are loaded once at session start and do not change if the underlying files are edited mid-session
  5. Assembly order is SOUL.md > project context > AGENTS.md, matching hermes-agent's prompt layering
**Plans**: 2 plans
Plans:
- [ ] 01-01-PLAN.md — Context scanner + PromptBuilder rewrite with layered loading
- [ ] 01-02-PLAN.md — CLI wiring + full build verification

### Phase 2: Telegram Gateway
**Goal**: A working Telegram bot that receives messages via long polling, runs them through the agent loop with tool use, streams responses back with progressive message editing, and handles multiple concurrent users reliably
**Depends on**: Phase 1 (agent needs context/personality loaded before serving users)
**Requirements**: TG-01, TG-02, TG-03, TG-04, TG-05, TG-06, TG-07, TG-08, ASYNC-01, ASYNC-02, ASYNC-03
**Risk**: HIGH -- This is the most technically complex phase. Async wiring between polling, agent loop, and streaming message edits. Concurrency control across multiple chat sessions. Error recovery across network failures, Telegram API rate limits, and 409 conflicts. The existing TelegramAdapter has Bot API types but polling is not connected to the agent loop.
**Mitigation**: Research doc provides complete architecture (channel-based bridge, StreamConsumer, BackoffState). hermes-agent's Python implementation is a proven reference. Existing TelegramAdapter is 90% complete for Bot API calls -- the gap is the wiring, not the Telegram client.
**Key technical decisions**:
  - Keep hand-rolled Telegram client (not teloxide/frankenstein) -- already works, zero new dependencies
  - CancellationToken-based cooperative shutdown replacing AtomicBool + handle.abort()
  - Arc<RwLock<SessionStore>> for safe session sharing across tokio tasks
  - Arc<ToolRegistry> shared across concurrent agent runs
  - Semaphore-bounded concurrency (default 4-8 concurrent agent runs)
  - Supervisor pattern: JoinSet tracks active agent runs, drains on shutdown
  - StreamConsumer with 300ms edit interval, cursor indicator, 4096-char overflow handling
  - Exponential backoff with jitter (1s base, 60s cap) for polling failures
  - 409 conflict detection (fatal after 5 retries)
  - Channel-based message dispatch (mpsc) decoupling polling from processing
**Estimated complexity**: L
**Success Criteria** (what must be TRUE):
  1. Sending a message to the Telegram bot produces an agent response with tool use results in the same chat
  2. Responses stream progressively -- the message is edited as LLM chunks arrive, with a cursor indicator during generation
  3. Multiple users can chat with the bot simultaneously without blocking each other
  4. Bot reconnects automatically after network interruptions with exponential backoff
  5. Sending ctrl+c gracefully stops the bot, waiting for in-flight agent runs to complete before exiting
**Plans**: 5 plans
Plans:
- [x] 02-01-PLAN.md — Async foundation: tokio-util dep, config extensions, trait redesign, TelegramAdapter refactor
- [x] 02-02-PLAN.md — StreamConsumer + BackoffState utility modules with tests
- [ ] 02-03-PLAN.md — Core wiring: polling loop, channel dispatch, user queue, handler, runner
- [ ] 02-04-PLAN.md — Slash commands (/start, /new, /clear, /help) and error recovery
- [ ] 02-05-PLAN.md — Multimodal input (images, PDFs, documents) + gateway CLI subcommand

### Phase 3: Self-Improvement + Security
**Goal**: Agent can safely read, edit, and extend its own context files (SOUL.md, AGENTS.md) and maintain a persistent memory of facts, with security scanning that prevents prompt injection or self-destructive modifications
**Depends on**: Phase 2 (self-improvement is exercised through the Telegram gateway; security scanning patterns also apply to Telegram rate limiting)
**Requirements**: SELF-01, SELF-02, SELF-03, SELF-04, SELF-05, SELF-06, SEC-01, SEC-02, SEC-03
**Risk**: MEDIUM-HIGH -- Novel territory: the agent modifying its own prompt is powerful but dangerous. Security scanning correctness is critical -- false negatives allow prompt injection, false positives block legitimate edits. The frozen-snapshot pattern must be airtight to prevent mid-session prompt corruption. Memory subsystem is new code (MemoryStore with bounded entries, atomic writes, file locking).
**Mitigation**: hermes-agent has a battle-tested implementation to port from. Regex-based threat patterns are language-agnostic. Atomic file I/O pattern (temp file + rename) already exists in the cron crate. Memory format (section-sign delimited entries with char limits) is simple and proven.
**Key technical decisions**:
  - Self-modification uses existing file tools (read_file, write_file, patch) -- no special API
  - Security scanning via RegexSet: injection overrides, deception patterns, exfiltration attempts, invisible Unicode
  - Frozen-snapshot pattern: disk writes are immediate but prompt only updates on next session start
  - Memory subsystem: MEMORY.md with bounded char limit (2,200 chars), section-sign delimited entries
  - Atomic file I/O: tempfile + std::fs::rename for crash-safe writes
  - Dedicated memory tool with add/replace/remove actions
  - SEC-01 (SSRF validation) built here because it is a prerequisite for Phase 4 web tools
  - SEC-03 (Telegram rate limiting) protects the gateway built in Phase 2
**Estimated complexity**: M
**Success Criteria** (what must be TRUE):
  1. Agent can read its own SOUL.md via the read_file tool and describe its personality
  2. Agent can edit SOUL.md via write_file/patch and the change is reflected in the next session
  3. Writing content containing prompt injection patterns (e.g., "ignore previous instructions") to a context file is blocked with a warning
  4. Agent can save facts to memory ("remember that I prefer Rust") and those facts appear in the system prompt on the next session
  5. Memory entries respect the character limit -- adding beyond the cap fails gracefully or requires removing existing entries
**Plans**: TBD

### Phase 4: Web Scraping Tools
**Goal**: Agent can fetch and read web page content via a web_read tool, with SSRF protection and content truncation for context-window safety
**Depends on**: Phase 3 (SEC-01 SSRF validation must exist before any URL fetching; SEC-02 scanning patterns reused for content safety)
**Requirements**: WEB-01, WEB-02, WEB-03, WEB-04
**Risk**: LOW -- Well-understood patterns. Firecrawl scrape API is nearly identical to the already-integrated search API. Local fallback with scraper crate is straightforward HTML parsing. SSRF protection is ported from hermes-agent's url_safety.py (already researched).
**Mitigation**: API-first approach means most complexity is server-side. Local fallback is a ~50-line heuristic extractor. reqwest is already in the workspace.
**Key technical decisions**:
  - Firecrawl scrape API as primary backend (already have API key, search endpoint integrated)
  - Local fallback: reqwest GET + scraper crate with semantic selector heuristic (article/main/role=main)
  - SSRF validation runs before every fetch (resolve hostname, block private IPs, localhost, CGNAT, metadata endpoints)
  - Content truncation at configurable limit (default 50K chars) with notice
  - Single high-level tool (web_read) not fine-grained tools -- LLMs prefer fewer decision points
  - Add gzip/brotli/deflate features to reqwest for compressed responses
**Estimated complexity**: S
**Success Criteria** (what must be TRUE):
  1. Agent can use the web_read tool to fetch a public URL and receive extracted text content
  2. Attempting to fetch a private/internal IP address (127.0.0.1, 10.x.x.x, 169.254.x.x) is blocked with a clear error
  3. Content longer than the configured limit is truncated with a notice indicating the truncation
  4. When Firecrawl is unavailable (no API key or service down), the local scraper fallback extracts readable content from static HTML pages
**Plans**: TBD

## Coverage

### Requirement-to-Phase Mapping

| Requirement | Phase | Category |
|-------------|-------|----------|
| CTX-01 | Phase 1 | Context Files |
| CTX-02 | Phase 1 | Context Files |
| CTX-03 | Phase 1 | Context Files |
| CTX-04 | Phase 1 | Context Files |
| CTX-05 | Phase 1 | Context Files |
| TG-01 | Phase 2 | Telegram Gateway |
| TG-02 | Phase 2 | Telegram Gateway |
| TG-03 | Phase 2 | Telegram Gateway |
| TG-04 | Phase 2 | Telegram Gateway |
| TG-05 | Phase 2 | Telegram Gateway |
| TG-06 | Phase 2 | Telegram Gateway |
| TG-07 | Phase 2 | Telegram Gateway |
| TG-08 | Phase 2 | Telegram Gateway |
| ASYNC-01 | Phase 2 | Async Infrastructure |
| ASYNC-02 | Phase 2 | Async Infrastructure |
| ASYNC-03 | Phase 2 | Async Infrastructure |
| SELF-01 | Phase 3 | Self-Improvement |
| SELF-02 | Phase 3 | Self-Improvement |
| SELF-03 | Phase 3 | Self-Improvement |
| SELF-04 | Phase 3 | Self-Improvement |
| SELF-05 | Phase 3 | Self-Improvement |
| SELF-06 | Phase 3 | Self-Improvement |
| SEC-01 | Phase 3 | Security |
| SEC-02 | Phase 3 | Security |
| SEC-03 | Phase 3 | Security |
| WEB-01 | Phase 4 | Web Scraping |
| WEB-02 | Phase 4 | Web Scraping |
| WEB-03 | Phase 4 | Web Scraping |
| WEB-04 | Phase 4 | Web Scraping |

**Coverage: 29/29 v1 requirements mapped. No orphans. No duplicates.**

## Progress

**Execution Order:**
Phases execute in numeric order: 1 > 2 > 3 > 4

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Context File Loading | 0/2 | Planned    |  |
| 2. Telegram Gateway | 2/5 | In Progress|  |
| 3. Self-Improvement + Security | 0/TBD | Not started | - |
| 4. Web Scraping Tools | 0/TBD | Not started | - |
