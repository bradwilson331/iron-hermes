# Milestones

## v1.0 MVP (Shipped: 2026-04-08)

**Phases completed:** 4 phases (1-4), 11 plans

**Delivered:** Rust rewrite of hermes-agent with context file loading, Telegram gateway with streaming and tool use, self-improvement with security scanning, and web scraping with SSRF protection.

**Key accomplishments:**

- Context file loading with priority-chain assembly (SOUL.md > project > AGENTS.md) and injection scanning
- Telegram gateway with long polling, progressive streaming, concurrent user handling, and graceful shutdown
- Self-improvement: agent reads/edits its own personality files with prompt injection detection
- Memory subsystem with bounded declarative facts persisted in MEMORY.md
- Web scraping via Firecrawl with SSRF protection and local HTML fallback
- Security: SSRF validation, threat scanning for context writes, gateway rate limiting

---

## v1.1 Automation (Shipped: 2026-04-11)

**Phases completed:** 12 phases (5-10.1), 34 plans, 204 commits

**Delivered:** Automation, orchestration, and knowledge capabilities — scheduled tasks, event hooks, skills system, code execution, subagent delegation, and batch processing.

**Key accomplishments:**

- Scheduled tasks with natural language parsing (cron/interval/once), skill attachment, and multi-platform delivery routing (Telegram, CLI, webhook)
- Event hooks with lifecycle logging (JSONL), guardrail tool interception (blocklist-based), and webhook forwarding with HMAC signing and retry queue
- Skills system with progressive disclosure (catalog at startup, full content on activation), agentskills.io compatibility, and allowed_tools enforcement at tool dispatch
- Python code execution sandbox with JSON-RPC 2.0 tool bridge over UDS, env stripping, timeout/call/output limits
- Subagent delegation with isolated context, semaphore-bounded concurrency (max 3), batch mode, cancellation propagation, and recursive delegation prevention
- Batch processing with parallel ShareGPT-format output, content-hash checkpointing, and 4-criteria quality filtering (hallucinated tools, no reasoning, secrets, empty responses)

### Known Gaps

- **SKILL-13** (slash-command integration) — explicit backlog item, not planned for v1.1
- **Nyquist sign-off** — 7 phases have validation scaffolds but formal sign-off pending
- **CLI feature disparity** — execute_code, hooks, guardrails are gateway-only (architectural decision)

---
