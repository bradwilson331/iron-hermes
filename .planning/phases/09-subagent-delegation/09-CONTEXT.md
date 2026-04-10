# Phase 9: Subagent Delegation - Context

**Gathered:** 2026-04-10
**Status:** Ready for planning

<domain>
## Phase Boundary

This phase delivers the `delegate_task` agent tool: the parent agent can delegate a task description to an isolated child agent that runs its own AgentLoop with a restricted ToolRegistry. The child runs to completion and returns its final text response as the tool result. A global semaphore enforces a configurable concurrency limit (default 3), and recursive delegation is structurally prevented by excluding `delegate_task` from all child toolsets at registry build time.

**Out of scope:**
- Interactive mid-task communication between parent and child (Out of Scope table)
- Persistent subagent state across sessions (Out of Scope table)
- Fire-and-forget / async delegation with polling
- Subagent access to skills system

</domain>

<decisions>
## Implementation Decisions

### Tool Filtering Strategy
- **D-01:** Allowlist pattern — parent passes a list of tool names, a new ToolRegistry is built containing only those tools. Same proven pattern as Phase 8's `rpc_registry` for execute_code. Structurally prevents access to unlisted tools.
- **D-02:** Default safe subset when parent doesn't specify tools: `read_file`, `write_file`, `patch`, `search_files`, `web_search`, `web_read`, `memory` — same as Phase 8's RPC safe subset, excluding `terminal`, `execute_code`, and `delegate_task`.
- **D-03:** Parent can override the default and grant any tool except `delegate_task`. Passing `allowed_tools: ["terminal", "read_file"]` expands access beyond the safe subset for specific tasks.
- **D-04:** Validation at build time — when constructing the child's ToolRegistry, validate the allowlist against available tools and strip `delegate_task` before the child agent starts. Fail early if an unknown tool is requested. The child never sees `delegate_task` in its tool schemas (AGENT-05).
- **D-05:** No skills for subagents — SkillsTool is excluded from child toolsets. Subagents are focused work units with a task description and tools, not skill discovery. Keeps context small and execution predictable.

### Subagent Lifecycle & Result Format
- **D-06:** Blocking tool call — `delegate_task` blocks like any other tool call. Parent sends task, waits for child AgentLoop to finish, gets result as the tool response. Fits existing AgentLoop dispatch pattern.
- **D-07:** Final text response only — `delegate_task` returns the child agent's `final_response` string. Child is a black box to the parent. Matches AGENT-01: "receive the child agent's final response as the tool result".
- **D-08:** Configurable timeout via `agent.subagent_timeout` in config.yaml, default 300 seconds (5 min). `tokio::time::timeout` wraps the child `AgentLoop::run()`.
- **D-09:** Both wall-clock timeout AND turn limit — child gets configurable max_iterations (default 10 turns) in addition to the timeout. Belt and suspenders to prevent runaway loops even if each turn is fast.

### Session & Terminal Isolation
- **D-10:** Separate TerminalTool instance with unique temp CWD per subagent. Commands run in the temp directory, not in the parent's CWD. Each subagent operates in its own terminal session scope (AGENT-04).
- **D-11:** Fresh conversation with task as system prompt — child starts with a system prompt containing only the task description. No parent conversation history inherited. Clean slate matching "isolated context" from AGENT-01.
- **D-12:** Read-only memory access — subagent can read facts from MEMORY.md via the memory tool but cannot write persistent changes. Prevents subagent from corrupting parent's memory store.
- **D-13:** Temp working directory cleaned up on completion — deleted when subagent finishes. Files created by the subagent are lost unless written to a path the parent specified in the task description. Matches ephemeral work unit philosophy per Out of Scope table.

### Concurrency & Queueing
- **D-14:** Global concurrency limit — a single `tokio::sync::Semaphore` shared across all agent runs (CLI and gateway). In gateway mode with multiple chats, this prevents resource exhaustion.
- **D-15:** Block and wait with message when limit is hit — `Semaphore::acquire()` blocks until a slot opens. Before blocking, surface a message: "Waiting for a subagent slot (3/3 in use)". Matches success criterion #3.
- **D-16:** Configurable concurrency limit via `agent.max_subagents` in config.yaml, default 3. Power users can tune up or down.

### Claude's Discretion
- Whether `DelegateTaskTool` lives in `ironhermes-tools` or needs a new crate
- System prompt format for the child agent (how task description is framed)
- How the "waiting for slot" message is surfaced (tool progress callback vs inline text)
- Temp directory naming convention and location
- Whether the child's LlmClient reuses the parent's or creates a new one
- Number of plans — default to 3 per ROADMAP, but planner may adjust

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — Lines 94-100 define AGENT-01 through AGENT-05. Lines 216-220 in traceability table map all five to Phase 9.
- `.planning/ROADMAP.md` — Phase 9 section has the 5 success criteria and 3-plan estimate.

### Existing patterns (templates for this phase)
- `crates/ironhermes-tools/src/registry.rs` — ToolRegistry with `register_execute_code_tool()` showing the `Arc<ToolRegistry>` filtered subset pattern (lines 246-260). Direct template for D-01.
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop struct, `::new()` constructor taking `Arc<ToolRegistry>`, and `::run()` method. Child agent reuses this same loop.
- `crates/ironhermes-tools/src/terminal.rs` — TerminalTool with CWD configuration. Template for D-10 isolated terminal instances.

### Architecture reference
- `.planning/codebase/ARCH.md` — Crate dependency graph, Tool trait definition, concurrency model, shared state patterns (`Arc<ToolRegistry>`, semaphore usage).

### Prior phase context (code execution isolation patterns)
- `.planning/phases/08-code-execution/08-CONTEXT.md` — Documents sandbox strategy, safe tool subset (D-07), timeout pattern (D-12), auto-shutdown (D-11). Direct precedent for subagent isolation decisions.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `ToolRegistry` with allowlist filtering: `get_definitions(enabled_tools: Option<&[String]>)` already filters tool schemas. Dispatch needs matching filter for enforcement.
- `register_execute_code_tool(rpc_registry: Arc<ToolRegistry>)`: proven pattern for building a restricted registry subset and passing it to a tool.
- `AgentLoop::new(client, registry, max_iterations)`: child agent can be instantiated with the filtered registry and a separate max_iterations value.
- `tokio::sync::Semaphore`: standard Tokio primitive for the concurrency limit. Already used conceptually in gateway's concurrency limiting (TG-06).

### Established Patterns
- All tools implement `Tool` trait, return `String` via `anyhow::Result<String>`
- Tools registered via `registry.register(Box::new(MyTool))` with shared state passed at construction
- Config sections use `Default` impl — add `SubagentConfig` with `timeout`, `max_subagents`, `max_iterations` fields
- Child process timeout via `tokio::time::timeout` wrapping async work (TerminalTool, ExecuteCodeTool precedent)

### Integration Points
- `ToolRegistry::register()` — where DelegateTaskTool gets registered
- `config.yaml` — new `agent.subagent_timeout`, `agent.max_subagents`, `agent.subagent_max_iterations` fields
- `Arc<Semaphore>` — needs to be created at startup and passed to DelegateTaskTool constructor
- Hook events fire automatically through AgentLoop dispatch (child's tool calls also trigger hooks)

</code_context>

<specifics>
## Specific Ideas

1. **Registry building**: `DelegateTaskTool::new()` receives the parent's `Arc<ToolRegistry>` plus the global `Arc<Semaphore>`. When `execute()` is called, it builds a child registry by iterating the parent's tools and cloning/re-registering only the allowed ones. `delegate_task` is never registered.

2. **Child AgentLoop**: Construct a fresh `AgentLoop::new(client, child_registry, subagent_max_iterations)` with the filtered registry. Call `agent_loop.run(vec![system_message])` where the system message contains the task description. Return `agent_result.final_response`.

3. **Semaphore flow**: `acquire() -> run child -> drop permit`. The "waiting" message should be emitted before the acquire blocks, not after.

4. **Config structure**: Extend existing `AgentConfig` or add a nested section:
   ```yaml
   agent:
     subagent_timeout: 300
     max_subagents: 3
     subagent_max_iterations: 10
   ```

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 09-subagent-delegation*
*Context gathered: 2026-04-10*
