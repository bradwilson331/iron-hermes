# Phase 9: Subagent Delegation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-10
**Phase:** 09-subagent-delegation
**Areas discussed:** Tool filtering strategy, Subagent lifecycle & result format, Session & terminal isolation, Concurrency & queueing

---

## Tool Filtering Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Allowlist pattern | Parent passes tool names, new registry built with only those tools | ✓ |
| Denylist pattern | Start with full registry, remove specific tools | |
| You decide | Claude chooses | |

**User's choice:** Allowlist pattern
**Notes:** Same proven pattern as Phase 8's rpc_registry

---

| Option | Description | Selected |
|--------|-------------|----------|
| Same safe subset as execute_code | read_file, write_file, patch, search_files, web_search, web_read, memory | ✓ |
| All tools except delegate_task | More powerful but less isolated | |
| No default — parent must always specify | Safer but more verbose | |

**User's choice:** Same safe subset as execute_code
**Notes:** Proven safe from Phase 8

---

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, parent can override | Any tool except delegate_task via allowlist | ✓ |
| No, hard cap at safe subset | Maximum safety, less flexibility | |

**User's choice:** Yes, parent can override
**Notes:** Flexible for power users

---

| Option | Description | Selected |
|--------|-------------|----------|
| Build time | Validate allowlist and strip delegate_task before child starts | ✓ |
| Dispatch time | Intercept at call time, child sees delegate_task in schema | |
| You decide | Claude picks | |

**User's choice:** Build time
**Notes:** Fail early if unknown tool requested

---

| Option | Description | Selected |
|--------|-------------|----------|
| No skills for subagents | Focused work units, no skill discovery | ✓ |
| Skills available if parent allows | More capable but complex | |
| You decide | Claude determines | |

**User's choice:** No skills for subagents
**Notes:** Keeps context small and execution predictable

---

## Subagent Lifecycle & Result Format

| Option | Description | Selected |
|--------|-------------|----------|
| Blocking tool call | Parent waits for child to finish, gets result as tool response | ✓ |
| Fire-and-forget with poll | Returns task ID, parent polls for results | |
| You decide | Claude picks | |

**User's choice:** Blocking tool call
**Notes:** Fits existing AgentLoop dispatch pattern

---

| Option | Description | Selected |
|--------|-------------|----------|
| Final text response only | Child's final_response string, black box | ✓ |
| Structured result with metadata | Response + turns + tool calls + usage | |
| You decide | Claude picks | |

**User's choice:** Final text response only
**Notes:** Matches AGENT-01 requirement language

---

| Option | Description | Selected |
|--------|-------------|----------|
| 5 minutes fixed | Same as execute_code | |
| Configurable via config.yaml | agent.subagent_timeout, default 5 min | ✓ |
| Match parent's max_iterations | No wall-clock timeout | |

**User's choice:** Configurable via config.yaml
**Notes:** Consistent with other configurable settings

---

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, both timeout and turn limit | Wall-clock + max_iterations, belt and suspenders | ✓ |
| Timeout only | Just wall-clock timeout | |
| You decide | Claude determines | |

**User's choice:** Yes, both timeout and turn limit
**Notes:** Prevents runaway loops even if each turn is fast

---

## Session & Terminal Isolation

| Option | Description | Selected |
|--------|-------------|----------|
| Separate TerminalTool with unique CWD | Commands run in temp dir, not parent's CWD | ✓ |
| Shared CWD, separate shell state | Less isolation, same files | |
| No terminal access by default | Terminal excluded from safe subset | |

**User's choice:** Separate TerminalTool instance with unique CWD
**Notes:** Effective isolation per AGENT-04

---

| Option | Description | Selected |
|--------|-------------|----------|
| Fresh conversation with task as system prompt | No parent history, clean slate | ✓ |
| Inherit parent's system prompt + task | More capable but heavier | |
| You decide | Claude determines | |

**User's choice:** Fresh conversation with task as system prompt
**Notes:** Matches "isolated context" from AGENT-01

---

| Option | Description | Selected |
|--------|-------------|----------|
| Read-only memory access | Can read MEMORY.md but no persistent writes | ✓ |
| Full memory access | Read and write same MEMORY.md | |
| No memory access | Strip memory entirely | |

**User's choice:** Read-only memory access
**Notes:** Prevents subagent from corrupting parent's memory

---

| Option | Description | Selected |
|--------|-------------|----------|
| Clean up on completion | Temp dir deleted when subagent finishes | ✓ |
| Preserve until parent requests cleanup | Dir path returned, parent inspects | |
| You decide | Claude picks | |

**User's choice:** Clean up on completion
**Notes:** Matches ephemeral work unit philosophy

---

## Concurrency & Queueing

| Option | Description | Selected |
|--------|-------------|----------|
| Global limit | Single semaphore shared across all agent runs | ✓ |
| Per-parent limit | Each parent gets own semaphore | |

**User's choice:** Global limit
**Notes:** Prevents resource exhaustion in gateway mode

---

| Option | Description | Selected |
|--------|-------------|----------|
| Block and wait with message | Semaphore::acquire() blocks, surfaces waiting message | ✓ |
| Fail immediately with error | Return error, LLM retries | |
| You decide | Claude picks | |

**User's choice:** Block and wait with message
**Notes:** Matches success criterion #3

---

| Option | Description | Selected |
|--------|-------------|----------|
| Configurable via config.yaml | agent.max_subagents, default 3 | ✓ |
| Hardcoded at 3 | Simple, matches requirement | |

**User's choice:** Configurable via config.yaml
**Notes:** Consistent with configurable timeout decision

---

## Claude's Discretion

- Whether DelegateTaskTool lives in ironhermes-tools or needs a new crate
- System prompt format for child agent
- How "waiting for slot" message is surfaced
- Temp directory naming convention
- Whether child's LlmClient reuses parent's or creates new
- Number of plans

## Deferred Ideas

None — discussion stayed within phase scope.
