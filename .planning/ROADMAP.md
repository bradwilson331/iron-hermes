# Roadmap: IronHermes

## Milestones

- ✅ **v1.0 MVP** - Phases 1-4 (shipped 2026-04-08)
- 🚧 **v1.1 Automation** - Phases 5-10 + subphases 07.1-07.5 (in progress)

## Phases

<details>
<summary>✅ v1.0 MVP (Phases 1-4) - SHIPPED 2026-04-08</summary>

### Phase 1: Context File Loading
**Goal**: Agent loads personality and project context files into the system prompt so every conversation reflects the configured identity and project awareness
**Depends on**: Nothing (foundation phase)
**Requirements**: CTX-01, CTX-02, CTX-03, CTX-04, CTX-05
**Success Criteria** (what must be TRUE):
  1. Running `cargo run --bin ironhermes` with a SOUL.md in IRONHERMES_HOME produces agent responses reflecting that personality
  2. AGENTS.md content appears in the system prompt after SOUL.md content
  3. Project-level context files from the working directory are discovered and loaded using the priority chain
  4. Context files are loaded once at session start and do not change if the underlying files are edited mid-session
  5. Assembly order is SOUL.md > project context > AGENTS.md, matching hermes-agent's prompt layering
**Plans**: 2 plans

Plans:
- [x] 01-01-PLAN.md — Context scanner + PromptBuilder rewrite with layered loading
- [x] 01-02-PLAN.md — CLI wiring + full build verification

### Phase 2: Telegram Gateway
**Goal**: A working Telegram bot that receives messages via long polling, runs them through the agent loop with tool use, streams responses back with progressive message editing, and handles multiple concurrent users reliably
**Depends on**: Phase 1
**Requirements**: TG-01, TG-02, TG-03, TG-04, TG-05, TG-06, TG-07, TG-08, ASYNC-01, ASYNC-02, ASYNC-03
**Success Criteria** (what must be TRUE):
  1. Sending a message to the Telegram bot produces an agent response with tool use results in the same chat
  2. Responses stream progressively — the message is edited as LLM chunks arrive, with a cursor indicator during generation
  3. Multiple users can chat with the bot simultaneously without blocking each other
  4. Bot reconnects automatically after network interruptions with exponential backoff
  5. Sending ctrl+c gracefully stops the bot, waiting for in-flight agent runs to complete before exiting
**Plans**: 5 plans

Plans:
- [x] 02-01-PLAN.md — Async foundation: tokio-util dep, config extensions, trait redesign, TelegramAdapter refactor
- [x] 02-02-PLAN.md — StreamConsumer + BackoffState utility modules with tests
- [x] 02-03-PLAN.md — Core wiring: polling loop, channel dispatch, user queue, handler, runner
- [x] 02-04-PLAN.md — Slash commands (/start, /new, /clear, /help) and error recovery
- [ ] 02-05-PLAN.md — Multimodal input (images, PDFs, documents) + gateway CLI subcommand

### Phase 3: Self-Improvement + Security
**Goal**: Agent can safely read, edit, and extend its own context files (SOUL.md, AGENTS.md) and maintain a persistent memory of facts, with security scanning that prevents prompt injection or self-destructive modifications
**Depends on**: Phase 2
**Requirements**: SELF-01, SELF-02, SELF-03, SELF-04, SELF-05, SELF-06, SEC-01, SEC-02, SEC-03
**Success Criteria** (what must be TRUE):
  1. Agent can read its own SOUL.md via the read_file tool and describe its personality
  2. Agent can edit SOUL.md via write_file/patch and the change is reflected in the next session
  3. Writing content containing prompt injection patterns (e.g., "ignore previous instructions") to a context file is blocked with a warning
  4. Agent can save facts to memory and those facts appear in the system prompt on the next session
  5. Memory entries respect the character limit — adding beyond the cap fails gracefully or requires removing existing entries
**Plans**: 3 plans

Plans:
- [x] 03-01-PLAN.md — Core surgery: move context_scanner to core + file tool scanning integration
- [x] 03-02-PLAN.md — Memory subsystem: MemoryStore, MemoryTool, PromptBuilder injection
- [x] 03-03-PLAN.md — SSRF validator + per-user gateway rate limiting

### Phase 4: Web Scraping Tools
**Goal**: Agent can fetch and read web page content via a web_read tool, with SSRF protection and content truncation for context-window safety
**Depends on**: Phase 3
**Requirements**: WEB-01, WEB-02, WEB-03, WEB-04
**Success Criteria** (what must be TRUE):
  1. Agent can use the web_read tool to fetch a public URL and receive extracted text content
  2. Attempting to fetch a private/internal IP address (127.0.0.1, 10.x.x.x, 169.254.x.x) is blocked with a clear error
  3. Content longer than the configured limit is truncated with a notice indicating the truncation
  4. When Firecrawl is unavailable (no API key or service down), the local scraper fallback extracts readable content from static HTML pages
**Plans**: 2 plans

Plans:
- [x] 04-01-PLAN.md — Dependencies, config extension, and full WebReadTool implementation
- [x] 04-02-PLAN.md — Unit tests for truncation and local fallback extraction

</details>

### 🚧 v1.1 Automation (In Progress)

**Milestone Goal:** Add automation, orchestration, and knowledge capabilities — scheduled tasks, event hooks, skills system, code execution, subagent delegation, and batch processing.

#### Phase 5: Scheduled Tasks
**Goal**: Users can schedule recurring tasks using natural language, attach skills to them, and receive output on their preferred platform
**Depends on**: Phase 4 (stable agent loop, cron crate foundation exists)
**Requirements**: SCHED-01, SCHED-02, SCHED-03, SCHED-04
**Success Criteria** (what must be TRUE):
  1. User can create a scheduled task with natural language like "every morning at 9am" and the agent correctly interprets it to a cron expression
  2. User can pause, resume, or edit a scheduled task without deleting and recreating it
  3. User can attach a named skill to a scheduled task so the task runs with skill-provided context and instructions
  4. Scheduled task output is delivered to the configured platform (Telegram chat, CLI stdout, or webhook URL)
**Plans**: 3 plans

Plans:
- [x] 05-01-PLAN.md — Data model, ScheduleParsed enum, parse_schedule(), JobStore refactor
- [x] 05-02-PLAN.md — CronjobTool (agent tool) + cron prompt security scanner
- [x] 05-03-PLAN.md — Delivery routing, tick runner, gateway integration, CLI subcommands

#### Phase 6: Event Hooks
**Goal**: Agent lifecycle events are observable and interceptable — hooks log every significant event, guardrails can block tool calls, and events can be forwarded to external systems
**Depends on**: Phase 5 (hooks provide observability for all subsequent phases)
**Requirements**: HOOK-01, HOOK-02, HOOK-03
**Success Criteria** (what must be TRUE):
  1. Every message received, tool called, and response sent produces a structured log entry via the hook registry
  2. A configured guardrail hook can intercept a tool call before dispatch and block it, returning a clear error to the agent
  3. A configured webhook endpoint receives hook events as HTTP POST requests when events fire
**Plans**: 3 plans

Plans:
- [ ] 06-01-PLAN.md — HookEvent model, HookRegistry, hooks.toml config, JSONL logging, AgentLoop wiring
- [ ] 06-02-PLAN.md — GuardrailHook trait, BlocklistGuardrail, ToolRegistry intercept
- [ ] 06-03-PLAN.md — WebhookDelivery with auth/retry, hot-reload, gateway/CLI integration

#### Phase 7: Skills System
**Goal**: Agent discovers, catalogs, and activates skill documents on demand — loading only what's needed via progressive disclosure
**Depends on**: Phase 6 (hooks instrument skill activation events)
**Requirements**: SKILL-01, SKILL-02, SKILL-03, SKILL-04
**Success Criteria** (what must be TRUE):
  1. Agent discovers skill directories from ~/.ironhermes/skills/ and ~/.agents/skills/ at startup and includes a compact catalog (names + descriptions only) in the system prompt
  2. Full skill content is NOT loaded at startup — only the description is visible until the agent explicitly activates a skill
  3. A skill document follows the agentskills.io format (SKILL.md with YAML frontmatter containing name and description) and is correctly parsed and cataloged
  4. Agent can call the skills tool with list, view, or activate actions to browse and load skill content during a conversation
**Plans**: 3 plans

Plans:
- [x] 07-01-PLAN.md — SkillRegistry: discovery, parsing, and catalog in ironhermes-core
- [x] 07-02-PLAN.md — SkillsTool: list/view/activate tool in ironhermes-tools
- [x] 07-03-PLAN.md — Wiring: PromptBuilder catalog, CLI/gateway registration, cron skill resolution

#### Phase 07.3: Cron Tick Agent Execution + Hook Instrumentation
**Goal**: Replace the cron tick-runner placeholder so scheduled jobs construct a real AgentLoop, execute with attached skill content, deliver actual LLM output, and fire hook lifecycle events for every cron-triggered run
**Depends on**: Phase 7 (skill resolution already wired), Phase 6 (HookRegistry exists)
**Requirements**: SCHED-01, SCHED-03, SCHED-04, HOOK-01
**Gap Closure**: Closes v1.1 audit critical integrations #1 (runner.rs ↔ AgentLoop) and #2 (runner.rs ↔ HookRegistry), plus broken flows "scheduled job with skill executes and delivers output" and "HOOK-01 lifecycle events for cron-triggered runs"
**Success Criteria** (what must be TRUE):
  1. `gateway/runner.rs` tick task constructs an `AgentLoop`, passes real `full_input` (skill content + user prompt), and delivers the agent's real response through the existing delivery routing
  2. A scheduled job with an attached skill produces an LLM response that reflects the skill content (integration test)
  3. Cron-triggered agent runs fire `MessageReceived` / `ToolCalled` / `ResponseSent` hook events to the same registry as Telegram-triggered runs
  4. `ironhermes-cron` crate captures a `HookRegistry` handle in the tick task closure
  5. After landing, retroactive `/gsd-verify-work 05` produces Phase 05 VERIFICATION.md with all SCHED-01..04 marked satisfied
**Plans**: TBD (single phase per user direction — combine AgentLoop wiring + hook capture in one plan if practical)

Plans:
- [ ] 07.3-01-PLAN.md — TBD

#### Phase 07.4: Hook Ordering & Duplicate Event Fixes
**Goal**: Resolve the two "warning" severity integration issues from the v1.1 audit so hook event streams are accurate and single-source
**Depends on**: Phase 07.3
**Requirements**: HOOK-01, HOOK-02 (correctness refinement, no new reqs)
**Gap Closure**: Closes v1.1 audit integration warnings #3 (ToolCalled fires before guardrail dispatch) and #4 (duplicate MessageReceived/ResponseSent from gateway/handler.rs and agent_loop.rs)
**Success Criteria** (what must be TRUE):
  1. `agent_loop.rs` fires `ToolCalled` only after the guardrail chain has permitted the call; blocked tools never emit `tool_called` events
  2. A single Telegram message produces exactly one `MessageReceived` and one `ResponseSent` event in JSONL logs and webhook deliveries (no duplicates)
  3. Test coverage asserts event counts for a canonical Telegram round-trip
**Plans**: TBD

Plans:
- [ ] 07.4-01-PLAN.md — TBD

#### Phase 07.5: Skills System Housekeeping (SKILL-06 enforcement + traceability cleanup)
**Goal**: Close 07.2 tech debt: enforce SKILL-06 `allowed_tools` at tool dispatch, relocate SKILL-09 to v2, and align ROADMAP/traceability with deferred scope decisions
**Depends on**: Phase 07.2
**Requirements**: SKILL-06 (enforcement)
**Gap Closure**: Closes 07.2 tech debt items from v1.1 audit: `allowed_tools` parsed but not enforced; SKILL-09 still mapped to 07.2 in REQUIREMENTS.md; ROADMAP/phase-directory title still references "SKILL-05..09"
**Success Criteria** (what must be TRUE):
  1. Tool dispatch honors the active skill set's `allowed_tools` intersection — a tool not in the union of any active skill's allow-list is rejected with a clear error
  2. Regression test: activating a skill with a restrictive `allowed_tools` list blocks other tool calls for the remainder of that agent turn
  3. REQUIREMENTS.md: SKILL-09 is moved to the v2 section and the traceability table reflects it as a v2 requirement
  4. SKILL-13 remains in the Backlog bucket in REQUIREMENTS.md with an explicit note
**Plans**: TBD

Plans:
- [ ] 07.5-01-PLAN.md — TBD

#### Phase 8: Code Execution
**Goal**: Agent can execute Python scripts in an isolated child process, with sandboxed access to agent tools via JSON-RPC and enforced resource limits
**Depends on**: Phase 7 (skills can provide Python scripting patterns; hooks instrument exec events)
**Requirements**: EXEC-01, EXEC-02, EXEC-03, EXEC-04
**Success Criteria** (what must be TRUE):
  1. Agent can call execute_code with a Python script and receive the script's stdout as the tool result
  2. A Python script running in the child process can call agent tools (e.g., web_search, read_file) via JSON-RPC and receive real results back
  3. The child process environment has no API keys or secrets — environment variable stripping is verified by inspection
  4. A script that runs longer than 5 minutes is killed and returns a timeout error; a script exceeding 50KB of output is truncated
**Plans**: 3 plans

Plans:
- [ ] 08-01-PLAN.md — TBD

#### Phase 9: Subagent Delegation
**Goal**: Agent can delegate tasks to isolated child agents with restricted toolsets, enforcing concurrency limits and preventing recursive delegation
**Depends on**: Phase 8 (code execution patterns inform child process isolation; exec crate reused)
**Requirements**: AGENT-01, AGENT-02, AGENT-03, AGENT-04, AGENT-05
**Success Criteria** (what must be TRUE):
  1. Agent can call delegate_task with a task description and receive the child agent's final response as the tool result
  2. Parent agent specifies allowed tools for the child and the child cannot call tools outside that list
  3. Attempting to spawn more than 3 concurrent subagents blocks until a slot is available, with a clear message when the limit is hit
  4. Each subagent operates in its own terminal session scope and cannot read or affect another subagent's terminal state
  5. A child agent's toolset never includes delegate_task — recursive delegation is structurally impossible
**Plans**: 3 plans

Plans:
- [ ] 09-01-PLAN.md — TBD

#### Phase 10: Batch Processing
**Goal**: User can run parallel batch prompt execution from JSONL input, producing ShareGPT-format trajectory data with checkpointing and quality filtering
**Depends on**: Phase 9 (reuses stable agent loop and subagent concurrency patterns)
**Requirements**: BATCH-01, BATCH-02, BATCH-03, BATCH-04
**Success Criteria** (what must be TRUE):
  1. User can run a batch job from a JSONL file and multiple prompts execute in parallel up to a configurable worker limit
  2. Batch output is written in ShareGPT format (human/assistant/tool roles) that loads correctly into a HuggingFace dataset viewer
  3. Restarting a batch job mid-run resumes from where it stopped — already-completed entries (identified by content hash) are not re-run
  4. Trajectories where the agent hallucinated a tool name or produced a response with no reasoning steps are automatically filtered from output
**Plans**: 3 plans

Plans:
- [ ] 10-01-PLAN.md — TBD

## Coverage

### v1.1 Requirement-to-Phase Mapping

| Requirement | Phase | Category |
|-------------|-------|----------|
| SCHED-01 | Phase 5 | Scheduled Tasks |
| SCHED-02 | Phase 5 | Scheduled Tasks |
| SCHED-03 | Phase 5 | Scheduled Tasks |
| SCHED-04 | Phase 5 | Scheduled Tasks |
| HOOK-01 | Phase 6 | Event Hooks |
| HOOK-02 | Phase 6 | Event Hooks |
| HOOK-03 | Phase 6 | Event Hooks |
| SKILL-01 | Phase 7 | Skills System |
| SKILL-02 | Phase 7 | Skills System |
| SKILL-03 | Phase 7 | Skills System |
| SKILL-04 | Phase 7 | Skills System |
| EXEC-01 | Phase 8 | Code Execution |
| EXEC-02 | Phase 8 | Code Execution |
| EXEC-03 | Phase 8 | Code Execution |
| EXEC-04 | Phase 8 | Code Execution |
| AGENT-01 | Phase 9 | Subagent Delegation |
| AGENT-02 | Phase 9 | Subagent Delegation |
| AGENT-03 | Phase 9 | Subagent Delegation |
| AGENT-04 | Phase 9 | Subagent Delegation |
| AGENT-05 | Phase 9 | Subagent Delegation |
| BATCH-01 | Phase 10 | Batch Processing |
| BATCH-02 | Phase 10 | Batch Processing |
| BATCH-03 | Phase 10 | Batch Processing |
| BATCH-04 | Phase 10 | Batch Processing |

**Coverage: 23/23 v1.1 requirements mapped. No orphans. No duplicates.**

## Progress

**Execution Order:**
Phases execute in numeric order: 5 → 6 → 7 → 8 → 9 → 10

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Context File Loading | v1.0 | 2/2 | Complete | 2026-04-08 |
| 2. Telegram Gateway | v1.0 | 4/5 | In Progress | - |
| 3. Self-Improvement + Security | v1.0 | 3/3 | Complete | 2026-04-08 |
| 4. Web Scraping Tools | v1.0 | 2/2 | Complete | 2026-04-08 |
| 5. Scheduled Tasks | v1.1 | 3/3 | Gaps (see 07.3) | - |
| 6. Event Hooks | v1.1 | 3/3 | Gaps (see 07.4) | - |
| 7. Skills System | v1.1 | 3/3 | Complete | 2026-04-09 |
| 07.1. Skills Gap Analysis | v1.1 | 1/1 | Complete | 2026-04-09 |
| 07.2. Skills Spec Compliance | v1.1 | 4/4 | Complete | 2026-04-09 |
| 07.3. Cron Tick Agent Exec + Hooks | v1.1 | 0/? | Not started (gap closure) | - |
| 07.4. Hook Ordering & Dedup | v1.1 | 0/? | Not started (gap closure) | - |
| 07.5. Skills Housekeeping | v1.1 | 0/? | Not started (gap closure) | - |
| 8. Code Execution | v1.1 | 0/? | Not started | - |
| 9. Subagent Delegation | v1.1 | 0/? | Not started | - |
| 10. Batch Processing | v1.1 | 0/? | Not started | - |
