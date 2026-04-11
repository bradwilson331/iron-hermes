# Phase 8: Code Execution - Context

**Gathered:** 2026-04-10
**Status:** Ready for planning

<domain>
## Phase Boundary

This phase delivers the `execute_code` agent tool: the agent can execute Python scripts in an isolated child process, with sandboxed access to agent tools via JSON-RPC over a Unix domain socket, and enforced resource limits (timeout, call cap, output cap). A new `ironhermes-exec` crate provides the sandbox runtime and RPC server; `ExecuteCodeTool` in `ironhermes-tools` is the agent-facing tool.

**Out of scope:**
- Other languages (only Python for now)
- OS-level sandboxing (filesystem, network restrictions)
- Async Python / asyncio support
- PyPI package for the helper module

</domain>

<decisions>
## Implementation Decisions

### Sandbox Strategy
- **D-01:** Allowlist environment variables — start with a clean env, only pass through explicitly safe vars (PATH, HOME, PYTHONPATH, LANG). All API keys, tokens, and secrets are excluded by default. This satisfies EXEC-03.
- **D-02:** Secrets-only isolation — no filesystem or network restrictions beyond env stripping. Python can read/write files and make HTTP requests. Keeps complexity low for v1.1.
- **D-03:** Python interpreter path is configurable via `exec.python_path` in config.yaml, defaulting to `python3`. Lets users point to a venv or specific interpreter.
- **D-04:** Python only — no language parameter. The tool is called `execute_code` and implicitly runs Python. Other languages can be added later without redesign.
- **D-05:** Minimal context passed to scripts — working directory as CWD, plus safe env vars: `IRONHERMES_SESSION_ID` and `IRONHERMES_RPC_ADDR`. No chat_id or platform info leaks into the sandbox.

### JSON-RPC Bridge
- **D-06:** Unix domain socket for IPC — create a temp UDS per execution. Path passed to Python via `IRONHERMES_RPC_ADDR` env var. No port conflicts, no network exposure. Cleaned up when execution completes.
- **D-07:** Safe tool subset exposed via RPC — `read_file`, `write_file`, `patch`, `search_files`, `web_search`, `web_read`, `memory`. Excluded: `terminal` (defeats sandbox isolation) and `execute_code` (prevents recursion). Hardcoded subset, not configurable.
- **D-08:** Bundled helper module — ship a `hermes_tools.py` that the Rust parent writes to a temp directory and adds to PYTHONPATH. Scripts import via `from hermes_tools import web_search, read_file`. Zero pip install required.
- **D-09:** Synchronous RPC calls — Python calls like `result = web_search("query")` block until the Rust side returns. The Rust parent handles async internally. Simple for script authors.
- **D-10:** JSON-RPC 2.0 protocol — standard protocol with id, method, params, result/error fields. Newline-delimited messages over the UDS. Matches what EXEC-02 specifies.
- **D-11:** Auto-shutdown on process exit — RPC server is tied to the child process lifetime. When Python exits or is killed, the UDS listener shuts down and the temp socket file is cleaned up. No orphaned listeners.

### Resource Limits
- **D-12:** Timeout via `tokio::time::timeout` + SIGKILL — wrap the child process wait in tokio timeout (300s). On expiry, SIGKILL the child process and its process group. Same proven pattern as TerminalTool. Satisfies EXEC-04 timeout requirement.
- **D-13:** Server-side call counter — the Rust RPC server tracks call count. After 50 calls, it returns a JSON-RPC error for subsequent requests. The Python helper surfaces this as an exception. No way to bypass from Python. Satisfies EXEC-04 call limit.
- **D-14:** Truncate stdout with notice — keep the first 50KB of stdout, append `[truncated: output exceeded 50KB limit]`. Script continues running. Satisfies EXEC-04 output cap.
- **D-15:** Separate stdout and stderr — return both as distinct fields in the tool result so the LLM can distinguish normal output from errors.

### Claude's Discretion
- Crate structure: what lives in `ironhermes-exec` vs `ironhermes-tools` — planner decides the minimal split
- Whether `hermes_tools.py` is embedded as a `include_str!` constant or generated at runtime
- Process group management details for the SIGKILL cleanup
- UDS path format (temp dir naming convention)
- Number of plans — default to 3 per ROADMAP, but planner may adjust

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — Lines 89-93 define EXEC-01 through EXEC-04. Lines 212-215 in traceability table map all four to Phase 8.
- `.planning/ROADMAP.md` — Phase 8 section has the 4 success criteria and 3-plan estimate.

### Existing patterns (templates for this phase)
- `crates/ironhermes-tools/src/terminal.rs` — TerminalTool is the structural template for child process execution with tokio::process::Command, timeout, and output truncation. D-12 and D-14 follow this pattern.
- `crates/ironhermes-tools/src/registry.rs` — Tool trait, ToolRegistry, register pattern. ExecuteCodeTool follows this interface.
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop dispatches tools and fires hooks. execute_code will flow through the same path.

### Architecture reference
- `.planning/codebase/ARCH.md` — Crate dependency graph, Tool trait definition, concurrency model.
- `.planning/codebase/TECH.md` — Dependency versions, async patterns, config structure.

### Prior phase context (hooks and skills integration)
- `.planning/phases/07.3-cron-tick-agent-exec-hook-instrumentation/07.3-CONTEXT.md` — Documents how AgentLoop + HookRegistry are wired. Same wiring applies to execute_code invocations.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `TerminalTool` (`terminal.rs`): Child process execution with `tokio::process::Command`, timeout via `tokio::time::timeout`, output truncation at 50KB — direct template for execute_code's process management
- `Tool` trait + `ToolRegistry`: Established registration pattern — `ExecuteCodeTool` implements `Tool`, registered via a `register_execute_code_tool()` method
- Config system (`config.rs`): YAML sections with `Default` impls — add `ExecConfig` with `python_path` field
- Guardrail hooks: execute_code flows through the existing guardrail chain automatically

### Established Patterns
- All tools return `String` results via `anyhow::Result<String>`
- Tools are registered in `register_defaults()` or via separate `register_*_tool()` methods when they need shared state
- `tokio::process::Command` for child process spawning with `.output().await`
- Temp file/directory patterns used in cron (atomic writes via rename)

### Integration Points
- `ToolRegistry::register()` — where ExecuteCodeTool gets registered
- `config.yaml` — new `exec` section for `python_path` and potentially other exec config
- `Cargo.toml` workspace — new `ironhermes-exec` crate member
- Hook events fire automatically through AgentLoop dispatch (no special wiring needed)

</code_context>

<specifics>
## Specific Ideas

1. **hermes_tools.py API surface**: Functions like `web_search(query)`, `read_file(path)`, `write_file(path, content)`, etc. that map 1:1 to agent tool names. Each function opens UDS connection, sends JSON-RPC 2.0 request, blocks on response, returns result string or raises exception.

2. **UDS lifecycle**: Rust creates temp dir → creates UDS listener → spawns Python with `IRONHERMES_RPC_ADDR=<socket_path>` → Python connects on first tool call → Rust accepts connection → serves requests → Python exits → Rust drops listener → temp dir cleaned up.

3. **Call limit error**: JSON-RPC error response with code `-32000` and message `"RPC call limit exceeded (50 calls)"`. The hermes_tools.py helper raises `HermesCallLimitError` so scripts can catch it gracefully.

4. **Tool result format**: Since D-15 says separate stdout/stderr, the tool result string should be structured like:
   ```
   [stdout]
   <script output>
   [stderr]
   <error output>
   [exit_code: 0]
   ```

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 08-code-execution*
*Context gathered: 2026-04-10*
