# Phase 9: Subagent Delegation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-10 (initial), 2026-04-11 (gap review update)
**Phase:** 09-subagent-delegation
**Areas discussed:** Tool filtering strategy, Subagent lifecycle & result format, Session & terminal isolation, Concurrency & queueing, Batch API, Blocked tools, Progress display, Interrupt propagation, Model override, Config namespace, Credential inheritance, Toolset naming, Memory access

---

## Initial Discussion (2026-04-10)

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
**Notes:** Matches AGENT-01 requirement language. **Superseded** in gap review — see Gap 7 below.

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
**Notes:** Prevents subagent from corrupting parent's memory. **Superseded** in gap review — see Gap 11 below.

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

## Gap Review Session (2026-04-11)

User provided detailed subagent delegation requirements spec. 11 gaps identified against existing context. All resolved below.

---

## Gap 1: Batch API

| Option | Description | Selected |
|--------|-------------|----------|
| Add batch mode now | delegate_task accepts single or batch (tasks array). Concurrent execution, result ordering by index, truncate to 3. | ✓ |
| Single-task only | Phase 9 only supports single goal. Batch deferred. | |
| Simplified batch | Support tasks array but without tree-view progress display. | |

**User's choice:** Add batch mode now
**Notes:** Full batch API with tasks array, concurrent execution, result ordering by task index.

---

## Gap 2: Blocked Tools

| Option | Description | Selected |
|--------|-------------|----------|
| Full block list | Block all 5: delegate_task, clarify, memory (read+write), execute_code, send_message. | ✓ |
| Block with read-only memory | Block 4, allow memory reads but not writes. | |
| Minimal blocks | Only block delegate_task and clarify. | |

**User's choice:** Full block list
**Notes:** Changed from original (read-only memory) to fully blocked. Subagent works only with goal/context fields.

---

## Gap 3: Progress Display

| Option | Description | Selected |
|--------|-------------|----------|
| Full tree-view | CLI shows real-time tree of tool calls per subagent with per-task completion lines. Gateway batches progress. | ✓ |
| Simple status lines | Just show 'Subagent 1/3 running...' / 'Subagent 1/3 complete'. | |
| Defer progress UI | No special progress display in Phase 9. | |

**User's choice:** Full tree-view
**Notes:** Real-time CLI tree-view showing per-subagent tool calls. Gateway mode batches progress to parent callback.

---

## Gap 4: Interrupt Propagation

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, propagate | Interrupting parent cancels all children via CancellationToken. | |
| No, let children finish | Children run to completion even if parent interrupted. | |
| Configurable | Default propagate, but allow 'detach' flag to let specific children survive. | ✓ |

**User's choice:** Configurable
**Notes:** Default propagate via CancellationToken (Phase 8 pattern). Optional `detach: true` flag per task.

---

## Gap 5: Model Override

| Option | Description | Selected |
|--------|-------------|----------|
| Full model override | delegation.model, delegation.provider, delegation.base_url, delegation.api_key in config.yaml. | ✓ |
| Model name only | delegation.model only, reuse parent's provider/API key. | |
| Same as parent | No override. Subagents always use parent's model. | |

**User's choice:** Full model override
**Notes:** Complete model/provider override including custom endpoint support.

---

## Gap 6: Max Iterations Default

| Option | Description | Selected |
|--------|-------------|----------|
| 50 turns (spec) | Matches user spec. Allows complex multi-step tasks. | ✓ |
| 25 turns (middle) | Compromise between safety and capability. | |
| 10 turns (existing) | Conservative. Forces tightly scoped subagents. | |

**User's choice:** 50 turns (spec)
**Notes:** Changed from original (10 turns) to 50. Per-task max_iterations param can still override.

---

## Gap 7: Result Format

| Option | Description | Selected |
|--------|-------------|----------|
| Structured summary | System prompt instructs child to return structured output: actions taken, files modified, issues found. | ✓ |
| Plain final_response | Return child's last message as-is. | |
| Structured with fallback | Request structured format, accept plain text if child doesn't comply. | |

**User's choice:** Structured summary
**Notes:** Changed from original (plain final_response) to structured summary with defined sections. Supersedes initial decision.

---

## Gap 8: Config Namespace

| Option | Description | Selected |
|--------|-------------|----------|
| Top-level delegation: | delegation.max_iterations, delegation.default_toolsets, delegation.model, etc. | ✓ |
| Keep agent.subagent_* | agent.subagent_timeout, agent.max_subagents, etc. Nested under agent config. | |

**User's choice:** Top-level delegation:
**Notes:** Changed from agent.subagent_* to top-level delegation: section.

---

## Gap 9: Credential Inheritance

| Option | Description | Selected |
|--------|-------------|----------|
| Document explicitly | Add decision: subagents inherit parent's LlmClient config, credential pool, key rotation. | ✓ |
| Implicit | Don't document — implementation detail. | |

**User's choice:** Document explicitly
**Notes:** Added as D-24 in CONTEXT.md.

---

## Gap 10: Toolset Naming

| Option | Description | Selected |
|--------|-------------|----------|
| Named groups | Toolset groups (terminal, file, web) mapping to tool bundles. | ✓ |
| Individual tools | Exact tool names. More precise but verbose. | |
| Both | Named groups as shorthand, also accept individual names. | |

**User's choice:** Named groups
**Notes:** Changed from individual tool names to named toolset groups. Cleaner API matching user spec.

---

## Gap 11: Memory Access

| Option | Description | Selected |
|--------|-------------|----------|
| Fully blocked | No memory reads or writes. Subagent works only with goal/context fields. | ✓ |
| Read-only allowed | Can read MEMORY.md facts but not write. | |

**User's choice:** Fully blocked
**Notes:** Changed from original (read-only) to fully blocked. Memory is in the always-blocked list. Supersedes initial decision.

---

## Claude's Discretion

- DelegateTaskTool crate location
- System prompt format details
- "Waiting for slot" message surfacing method
- Temp directory naming convention
- Tree-view rendering details
- LlmClient reuse strategy

## Deferred Ideas

None — discussion stayed within phase scope.
