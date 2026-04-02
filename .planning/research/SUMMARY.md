# Project Research Summary

**Project:** IronHermes
**Domain:** Rust-based AI agent runtime with Telegram gateway, self-improvement, and web tooling
**Researched:** 2026-04-01
**Confidence:** MEDIUM-HIGH

## Executive Summary

IronHermes is a Rust port of the Python hermes-agent, and the research across all four areas converges on one theme: **the Python codebase is the blueprint, but Rust's ownership model demands specific architectural adaptations**. The Telegram gateway needs a channel-based pipeline (not inline handlers) to decouple polling from agent execution. The self-improvement system needs a frozen-snapshot pattern for context files to preserve LLM prompt cache stability. Web scraping should be API-first with a lightweight local fallback. And all of it must be wired together with structured concurrency primitives (CancellationToken, JoinSet, Semaphore) that replace the current fire-and-forget `tokio::spawn` pattern.

The recommended approach is to build in strict dependency order: async infrastructure and gateway wiring first (everything else depends on a working message pipeline), then web tooling (the simplest new tool to add), then the self-improvement subsystem (most complex, benefits from a stable agent loop). The existing codebase is further along than it appears -- the Telegram client, session store, prompt builder, and agent loop all exist in workable form. The primary gap is not missing code but missing *wiring*: `GatewayRunner::start()` creates the adapter but never connects it to the agent loop.

The top risk is **unstructured concurrency in the gateway**. The current pattern of unbounded `tokio::spawn` with `handle.abort()` shutdown will cause orphaned agent runs, lost responses, and resource exhaustion under load. This must be fixed before adding any new features. The second risk is **prompt cache invalidation** from the self-improvement system -- mid-session context file changes will destroy Anthropic prompt caching and dramatically increase costs.

## Key Findings

### Recommended Stack

The research unanimously recommends against adding heavy framework dependencies. The existing hand-rolled Telegram client, reqwest HTTP client, and serde-based serialization cover 90% of needs. New crate additions should be minimal and targeted.

**Core technologies (keep):**
- **reqwest 0.12**: HTTP client for Telegram API, Firecrawl, web fetching -- already in workspace, add `gzip`/`cookies` features
- **tokio + tokio-util**: Async runtime -- add `CancellationToken` from tokio-util for structured shutdown
- **serde + serde_json + serde_yaml**: Serialization for API responses, config, skill frontmatter
- **rusqlite**: Session persistence and FTS5 search -- already available

**New crates to add:**
- **scraper** (~0.22): CSS-selector HTML parsing for local web content extraction fallback
- **tokio-util** (0.7, `rt` feature): CancellationToken for hierarchical cancellation
- **mockall** (0.13, dev-only): Async trait mocking for tests
- **rand** (0.8): Jitter for exponential backoff
- **fs2**: Cross-platform file locking for memory/SOUL.md atomic writes

**Explicitly rejected:**
- teloxide (fights existing PlatformAdapter architecture)
- frankenstein (type system mismatch, no functional gain)
- headless_chrome (JS rendering handled by Firecrawl API)
- readability crates (undermaintained; API-based extraction is better)
- actor frameworks like actix/ractor (channels + spawn are sufficient)

### Expected Features

**Must have (table stakes):**
- Telegram message receive/respond with conversation sessions
- Streaming responses with progressive message editing (cursor indicator)
- Exponential backoff and 409 conflict detection on polling
- Graceful shutdown that drains in-flight agent runs
- SSRF protection on all URL-fetching tools
- SOUL.md identity loading from IRONHERMES_HOME
- web_read tool (Firecrawl scrape + local fallback)

**Should have (competitive):**
- Per-user rate limiting and concurrent agent run capping (Semaphore)
- Memory subsystem (MEMORY.md/USER.md with frozen snapshots)
- Skills subsystem (directory tree with YAML frontmatter, skill_view/skill_manage tools)
- Session persistence to SQLite
- Per-user sessions in group chats
- Content summarization for large web pages via fast LLM

**Defer (v2+):**
- Webhook mode (vs polling) for cloud deployment
- Multi-bot support (multiple Telegram tokens)
- MarkdownV2 formatting (use plain text initially)
- Cross-session performance metrics
- Periodic background reflection loops
- A/B testing of prompt variants
- web_crawl multi-page tool

### Architecture Approach

The architecture is a three-stage pipeline: Telegram poller produces MessageEvents into a bounded mpsc channel, an agent dispatcher consumes events (capped by Semaphore), and response delivery happens via oneshot channels or a stream consumer for progressive edits. CancellationToken trees propagate shutdown from parent to all children. A supervisor pattern auto-restarts crashed subsystems with circuit-breaking. The self-improvement system uses a 10-layer prompt assembly with frozen snapshots -- context files are read once at session start and never mutated mid-session.

**Major components:**
1. **TelegramPoller** -- Long-polls Telegram API, produces MessageEvents, handles backoff/conflict/reconnection
2. **AgentDispatcher** -- Routes events to per-session agent runs, manages JoinSet of active runs, enforces concurrency limits
3. **StreamConsumer** -- Bridges sync streaming callback to async Telegram message edits with 300ms rate limiting
4. **SessionStore** -- Arc<RwLock<HashMap>> with get_or_create, idle timeout, reset commands
5. **PromptBuilder** -- 10-layer system prompt assembly: SOUL.md -> guidance -> memory snapshot -> skills index -> context files -> platform hints
6. **MemoryStore** -- Bounded declarative memory (2200 chars) with add/replace/remove, atomic file writes, frozen snapshots
7. **WebReadTool** -- Firecrawl scrape API primary, scraper-based local fallback, SSRF validation on all URLs

### Critical Pitfalls

1. **Unstructured concurrency (CRITICAL)** -- Current `tokio::spawn` + `abort()` orphans agent runs on shutdown. Replace with CancellationToken + JoinSet for tracked, drainable task groups. This is prerequisite to everything else.

2. **Prompt cache invalidation from self-modification (CRITICAL)** -- If context files change mid-session, every subsequent LLM call cache-misses, multiplying costs. Use the frozen-snapshot pattern: build system prompt once at session start, changes take effect next session only.

3. **SSRF via web tools (CRITICAL)** -- An agent fetching arbitrary URLs is a server-side request forgery vector. Resolve hostnames to IPs before fetching, block private/internal ranges, block metadata endpoints. Port hermes-agent's url_safety.py patterns.

4. **Unbounded agent runs under load (HIGH)** -- Without Semaphore-based concurrency limiting, spam or multiple users can exhaust memory and LLM API quotas. Cap at 4-8 concurrent agent runs with explicit overflow messaging.

5. **Runaway self-modification (MODERATE)** -- Agent could rewrite SOUL.md into something non-functional. Mitigate with version history (keep 10 snapshots), security scanning (regex-based injection detection), per-session rate limits (max 5 edits), and minimum content validation.

## Implications for Roadmap

### Phase 1: Gateway Infrastructure
**Rationale:** Everything depends on a working message pipeline. The gateway wiring gap (adapter exists but is not connected to agent loop) must be closed first. Structured concurrency primitives are foundational.
**Delivers:** Messages flow from Telegram through the agent loop and responses come back. Non-streaming initially.
**Addresses:** CancellationToken shutdown, Semaphore concurrency limits, SessionStore with Arc<RwLock>, exponential backoff, 409 conflict detection, typing indicators, basic error classification on HermesError
**Avoids:** Unstructured concurrency pitfall, unbounded agent runs

### Phase 2: Streaming Responses
**Rationale:** Streaming is the difference between a bot that feels dead for 30 seconds and one that feels responsive. Depends on Phase 1 pipeline being wired.
**Delivers:** Progressive message editing with cursor indicator, 300ms rate-limited edits, message overflow handling (>4096 chars), 429 retry-after handling
**Addresses:** StreamConsumer port from Python GatewayStreamConsumer, oneshot response pairing, Telegram rate limit handling
**Avoids:** Telegram rate limit violations (300ms floor on edits)

### Phase 3: Web Read Tool
**Rationale:** Least complex new capability to add. Firecrawl integration is nearly identical to existing web_search. Delivers immediate user value. Independent of self-improvement system.
**Delivers:** web_read tool with Firecrawl scrape primary, local scraper fallback, SSRF protection, content truncation
**Uses:** reqwest (existing), scraper (new), Firecrawl API key (existing)
**Avoids:** SSRF pitfall via URL validation before fetch

### Phase 4: Context File Architecture
**Rationale:** Foundation for self-improvement. Must get the prompt assembly and frozen-snapshot pattern right before building memory/skills on top.
**Delivers:** SOUL.md loading from IRONHERMES_HOME, priority-based project context discovery, security scanning (regex injection detection), 10-layer prompt assembly, content truncation (20K char limit)
**Avoids:** Prompt cache invalidation pitfall

### Phase 5: Memory and Skills
**Rationale:** Depends on Phase 4 context architecture. Most complex subsystem. Benefits from a stable, tested agent loop (Phases 1-2).
**Delivers:** MemoryStore with MEMORY.md/USER.md, memory tool (add/replace/remove), skills directory scanning + index, skill_view and skill_manage tools, version history for SOUL.md, atomic file writes with locking
**Avoids:** Runaway self-modification (version history + scanning), memory bloat (character limits), skill rot (last_used timestamps)

### Phase 6: Robustness and Polish
**Rationale:** Hardening layer. Not needed for functionality but critical for reliability.
**Delivers:** Supervisor pattern for auto-restart, session persistence to SQLite, per-user rate limiting, group chat per-user sessions, idle timeout session reset, /reset and /new commands, content summarization for large web pages
**Addresses:** Production reliability, multi-user scenarios

### Phase Ordering Rationale

- Phases 1-2 are strictly sequential: streaming depends on the pipeline existing
- Phase 3 (web read) is independent of Phases 4-5 and can be parallelized with Phase 2 if resources allow
- Phase 4 must precede Phase 5: memory and skills are *consumers* of the prompt assembly system
- Phase 6 is a grab-bag of hardening that can be done incrementally alongside later phases
- This order ensures each phase delivers testable, user-visible value

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 2 (Streaming):** The sync-to-async bridge for StreamCallback is non-trivial in Rust. The mpsc::try_send pattern needs careful testing for backpressure behavior when the Telegram edit rate limit causes the consumer to slow down.
- **Phase 5 (Memory/Skills):** Security scanning regex patterns need to be ported from Python and validated. The skill YAML frontmatter schema needs finalization. File locking semantics with fs2 on macOS vs Linux should be verified.

Phases with standard patterns (skip deep research):
- **Phase 1 (Gateway):** All patterns (CancellationToken, JoinSet, Semaphore, mpsc channels) are well-documented tokio idioms.
- **Phase 3 (Web Read):** Firecrawl scrape API is nearly identical to already-integrated search API. scraper crate has straightforward CSS-selector API.
- **Phase 4 (Context Files):** Mostly file I/O with regex scanning. Patterns are clear from hermes-agent source.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Telegram Gateway | MEDIUM | Training-data-only for crate comparisons, but recommendation to keep existing client is HIGH confidence (based on direct code analysis) |
| Async Patterns | MEDIUM-HIGH | Core tokio patterns are stable and well-documented; verify specific API signatures against docs.rs before use |
| Self-Improvement | HIGH | All findings from direct hermes-agent source code analysis, proven architecture |
| Web Scraping | MEDIUM-HIGH | Direct codebase analysis for hermes-agent patterns; scraper crate status from training data (verify version) |

**Overall confidence:** MEDIUM-HIGH

### Gaps to Address

- **Telegram MarkdownV2 escaping:** Notoriously tricky. Deferred to Phase 6 but will need dedicated research when tackled. Use plain text until then.
- **AgentLoop cancellation support:** Phase 1 needs CancellationToken in the gateway, but the AgentLoop itself does not yet support mid-run cancellation. This limits interrupt support until AgentLoop is refactored.
- **DNS rebinding in SSRF protection:** The URL safety check has a TOCTOU race (DNS resolves safely during check, then to private IP for actual connection). Document the limitation; connection-level validation is complex.
- **tokio-util / mockall version compatibility with Rust edition 2024:** Verify current versions compile cleanly before depending on them.
- **Firecrawl scrape API exact request/response format:** Inferred from search integration and hermes-agent usage. Verify against live API documentation before implementing Phase 3.

## Sources

### Primary (HIGH confidence)
- hermes-agent codebase: prompt_builder.py, memory_tool.py, skill_manager_tool.py, run_agent.py, url_safety.py, gateway/ -- direct source analysis
- IronHermes codebase: telegram.rs, session.rs, prompt_builder.rs, agent_loop.rs, web_search.rs -- direct source analysis

### Secondary (MEDIUM confidence)
- tokio / tokio-util documentation patterns -- from training data, stable APIs
- scraper crate capabilities -- from training data, widely used
- Firecrawl API format -- inferred from existing integration + hermes-agent

### Tertiary (LOW confidence)
- readability crate maturity assessment -- training data only, verify before depending
- Jina Reader free tier availability -- verify current limits
- mockall 0.13 compatibility with async_trait on edition 2024 -- needs verification

---
*Research completed: 2026-04-01*
*Ready for roadmap: yes*
