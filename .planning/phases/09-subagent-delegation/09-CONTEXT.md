# Phase 9: Subagent Delegation - Context

**Gathered:** 2026-04-10
**Updated:** 2026-04-11
**Status:** Ready for planning

<domain>
## Phase Boundary

This phase delivers the `delegate_task` agent tool: the parent agent can delegate tasks to isolated child agent instances with restricted toolsets and their own terminal sessions. Supports both single-task and parallel batch modes (up to 3 concurrent). Each child gets a fresh conversation with only the goal and context fields as input — no parent history. Only the child's structured summary enters the parent's context.

A global semaphore enforces a configurable concurrency limit (default 3), and recursive delegation is structurally prevented by excluding `delegate_task` from all child toolsets at registry build time.

**Out of scope:**
- Interactive mid-task communication between parent and child (Out of Scope table)
- Persistent subagent state across sessions (Out of Scope table)
- Fire-and-forget / async delegation with polling
- Subagent access to skills system
- Nested delegation (depth > 1) — children cannot delegate further

</domain>

<decisions>
## Implementation Decisions

### Tool Filtering Strategy
- **D-01:** Named toolset groups — parent passes toolset group names (`["terminal", "file"]`, `["web"]`, `["terminal", "file", "web"]`) that map to bundles of individual tools. Cleaner API than listing individual tool names.
  - `terminal` → terminal tool
  - `file` → read_file, write_file, patch, search_files
  - `web` → web_search, web_read
  - Default toolsets when not specified: `["terminal", "file", "web"]` (configurable via `delegation.default_toolsets`)
- **D-02:** Always-blocked tools — regardless of what toolsets the parent specifies, these are always excluded from child registries:
  - `delegate_task` — no recursive delegation (AGENT-05)
  - `clarify` — subagents cannot interact with the user
  - `memory` — no reads or writes; subagent works only with goal/context fields
  - `execute_code` — children should reason step-by-step, not run scripts
  - `send_message` — no cross-platform side effects (e.g., Telegram messages)
- **D-03:** Validation at build time — when constructing the child's ToolRegistry, validate the toolset names against known groups, apply the always-blocked list, and fail early if an unknown toolset is requested. The child never sees blocked tools in its schemas.
- **D-04:** No skills for subagents — SkillsTool is excluded from child toolsets. Subagents are focused work units with a task description and tools, not skill discovery.

### Single-Task and Batch API
- **D-05:** Dual-mode tool — `delegate_task` accepts either:
  - Single task: `delegate_task(goal="...", context="...", toolsets=["terminal", "file"])`
  - Batch: `delegate_task(tasks=[{goal, context, toolsets}, ...])` for parallel execution
- **D-06:** Batch concurrency — tasks array is truncated to 3 if longer. Uses tokio task spawning (not ThreadPoolExecutor — this is Rust). All tasks in a batch share the global semaphore.
- **D-07:** Result ordering — batch results are sorted by task index to match input order regardless of completion order.
- **D-08:** Single-task delegation runs directly without spawning overhead.

### Subagent Lifecycle & Result Format
- **D-09:** Blocking tool call — `delegate_task` blocks like any other tool call. Parent sends task, waits for child AgentLoop to finish, gets result as the tool response.
- **D-10:** Structured summary — child's system prompt instructs it to return a structured summary: what actions were taken, what was found, any files modified, and any issues encountered. Parent gets a parseable, consistent result format.
- **D-11:** Configurable timeout via `delegation.timeout` in config.yaml, default 300 seconds (5 min). `tokio::time::timeout` wraps the child `AgentLoop::run()`.
- **D-12:** Max iterations default 50 turns — child gets configurable `delegation.max_iterations` (default 50). Belt and suspenders with timeout to prevent runaway loops.

### Session & Terminal Isolation
- **D-13:** Separate TerminalTool instance with unique temp CWD per subagent. Commands run in the temp directory, not in the parent's CWD. Each subagent operates in its own terminal session scope (AGENT-04).
- **D-14:** Fresh conversation with task as system prompt — child starts with a system prompt built from the goal and context fields. No parent conversation history inherited. Clean slate matching "isolated context" from AGENT-01.
- **D-15:** Temp working directory cleaned up on completion — deleted when subagent finishes. Files created by the subagent are lost unless written to a path the parent specified in the task description.

### Concurrency & Queueing
- **D-16:** Global concurrency limit — a single `tokio::sync::Semaphore` shared across all agent runs (CLI and gateway). In gateway mode with multiple chats, this prevents resource exhaustion.
- **D-17:** Block and wait with message when limit is hit — `Semaphore::acquire()` blocks until a slot opens. Before blocking, surface a message: "Waiting for a subagent slot (3/3 in use)".
- **D-18:** Configurable concurrency limit via `delegation.max_subagents` in config.yaml, default 3.

### Progress Display
- **D-19:** CLI tree-view — in CLI mode, display a real-time tree showing tool calls from each subagent with per-task completion lines. Shows which subagent is running and what tools it's calling.
- **D-20:** Gateway progress batching — in gateway mode, progress is batched and relayed to the parent's progress callback rather than streamed per-tool-call.

### Interrupt Propagation
- **D-21:** Default propagate — interrupting the parent (e.g., user sends new message) cancels all active children via CancellationToken (Phase 8 pattern). Children stop ASAP.
- **D-22:** Detach flag — `delegate_task` accepts an optional `detach: true` flag that lets specific children survive parent interrupt. Default is `false` (propagate).

### Model Override
- **D-23:** Full model override — subagents can use a different model/provider than the parent, configured in config.yaml:
  ```yaml
  delegation:
    model: "google/gemini-flash-2.0"
    provider: "openrouter"
    # Or custom endpoint:
    base_url: "http://localhost:1234/v1"
    api_key: "local-key"
  ```
  If omitted, subagents use the same model as the parent.

### Credential Inheritance
- **D-24:** Subagents inherit the parent's API key, provider configuration, and credential pool. Key rotation on rate limits applies to children. Model override (D-23) takes precedence when configured.

### Configuration
- **D-25:** Top-level `delegation:` config namespace:
  ```yaml
  delegation:
    max_iterations: 50
    timeout: 300
    max_subagents: 3
    default_toolsets: ["terminal", "file", "web"]
    model: null          # Optional override
    provider: null       # Optional override
    base_url: null       # Optional custom endpoint
    api_key: null        # Optional custom key
  ```

### Claude's Discretion
- Whether `DelegateTaskTool` lives in `ironhermes-tools` or needs a new crate
- System prompt format details (how goal/context/structured-summary instructions are framed)
- How the "waiting for slot" message is surfaced (tool progress callback vs inline text)
- Temp directory naming convention and location
- Tree-view rendering details (colors, indentation, update frequency)
- Whether the child's LlmClient reuses the parent's instance or creates a new one

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — Lines 94-100 define AGENT-01 through AGENT-05. Lines 216-220 in traceability table map all five to Phase 9.
- `.planning/ROADMAP.md` — Phase 9 section has the 5 success criteria and plan estimate.

### Existing patterns (templates for this phase)
- `crates/ironhermes-tools/src/registry.rs` — ToolRegistry with `register_execute_code_tool()` showing the `Arc<ToolRegistry>` filtered subset pattern. Direct template for D-01/D-03.
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop struct, `::new()` constructor taking `Arc<ToolRegistry>`, and `::run()` method. Child agent reuses this same loop.
- `crates/ironhermes-tools/src/terminal.rs` — TerminalTool with CWD configuration. Template for D-13 isolated terminal instances.

### Architecture reference
- `.planning/codebase/ARCH.md` — Crate dependency graph, Tool trait definition, concurrency model, shared state patterns (`Arc<ToolRegistry>`, semaphore usage).

### Prior phase context (code execution isolation patterns)
- `.planning/phases/08-code-execution/08-CONTEXT.md` — Documents sandbox strategy, safe tool subset, timeout pattern, auto-shutdown. Direct precedent for subagent isolation decisions.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `ToolRegistry` with allowlist filtering: `get_definitions(enabled_tools: Option<&[String]>)` already filters tool schemas. Dispatch needs matching filter for enforcement.
- `register_execute_code_tool(rpc_registry: Arc<ToolRegistry>)`: proven pattern for building a restricted registry subset and passing it to a tool.
- `AgentLoop::new(client, registry, max_iterations)`: child agent can be instantiated with the filtered registry and a separate max_iterations value.
- `tokio::sync::Semaphore`: standard Tokio primitive for the concurrency limit.
- `CancellationToken` from Phase 8: reusable for interrupt propagation to children (D-21).

### Established Patterns
- All tools implement `Tool` trait, return `String` via `anyhow::Result<String>`
- Tools registered via `registry.register(Box::new(MyTool))` with shared state passed at construction
- Config sections use `Default` impl — add `DelegationConfig` with all D-25 fields
- Child process timeout via `tokio::time::timeout` wrapping async work (TerminalTool, ExecuteCodeTool precedent)

### Integration Points
- `ToolRegistry::register()` — where DelegateTaskTool gets registered
- `config.yaml` — new top-level `delegation:` section (D-25)
- `Arc<Semaphore>` — needs to be created at startup and passed to DelegateTaskTool constructor
- `CancellationToken` — needs to be passed to DelegateTaskTool and forwarded to child AgentLoops
- Hook events fire automatically through AgentLoop dispatch (child's tool calls also trigger hooks)

</code_context>

<specifics>
## Specific Ideas

1. **Toolset group mapping**: Define a `ToolsetGroup` enum or map that translates group names to individual tool names. Built at compile time or startup. Always-blocked list applied as a post-filter regardless of groups.

2. **Batch execution flow**: For `tasks` array, spawn up to 3 tokio tasks, each acquiring a semaphore permit. Collect results into a Vec, sort by original index, return as combined structured response.

3. **Structured summary system prompt**: Child's system prompt ends with instructions like: "When you complete the task, provide a structured summary with sections: Actions Taken, Files Modified, Findings, Issues Encountered."

4. **Tree-view progress**: CLI adapter maintains a per-subagent line in the terminal, updating in-place as tool calls happen. Similar to multi-progress-bar crates. Gateway adapter batches updates.

5. **Config structure**: Top-level `delegation:` section with `DelegationConfig` struct implementing `Default`:
   ```yaml
   delegation:
     max_iterations: 50
     timeout: 300
     max_subagents: 3
     default_toolsets: ["terminal", "file", "web"]
   ```

6. **CancellationToken propagation**: Parent creates a child token from its own. On interrupt, parent cancels its token, which cascades to all children. `detach: true` tasks get an independent token.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 09-subagent-delegation*
*Context gathered: 2026-04-10*
*Context updated: 2026-04-11*
