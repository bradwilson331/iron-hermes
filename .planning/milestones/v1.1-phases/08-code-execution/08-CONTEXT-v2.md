# Phase 8: Code Execution - Context (v2 — Spec Alignment)

**Gathered:** 2026-04-10
**Updated:** 2026-04-10
**Status:** Ready for planning (gap closure)

<domain>
## Phase Boundary

This update brings the existing `execute_code` implementation in line with the product spec. The core sandbox, RPC server, and hermes_tools.py already work. These changes refine behavior to match the documented API contract.

**In scope (6 gaps to close):**
1. hermes_tools.py function signatures — match spec parameter shapes
2. Response format — structured with status/tool_calls_made/duration_seconds
3. Stderr cap — 10KB truncation limit
4. Kill strategy — SIGTERM → 5s grace → SIGKILL, own process group
5. Env stripping — pattern-based exclusion instead of strict allowlist
6. User interruption — terminate script when user sends a new message

**NOT in scope (accepted as-is):**
- Tool subset stays as currently implemented (web_read, not web_extract; terminal excluded; memory included)
- Config section name stays `exec` (not `code_execution`)
- Skill env passthrough (future work)
- Platform detection / Windows auto-disable (future work)

</domain>

<decisions>
## Implementation Decisions

### hermes_tools.py Function Signatures (Gap 2)
- **D-20:** Update `patch(path, diff)` to `patch(path, old_string, new_string, replace_all=False)` — matches the actual `patch` tool's parameters
- **D-21:** Update `web_search(query)` to `web_search(query, limit=10)` — add optional `limit` parameter
- **D-22:** Update `search_files(pattern, path)` to `search_files(pattern, path=".", file_glob=None, limit=None)` — add optional `file_glob` and `limit` parameters
- **D-23:** Update `web_read(url)` to accept both single URL string and list of URLs — `web_read(urls)` where urls can be str or list
- **D-24:** All function signatures must pass their parameters through to the RPC call — the Rust side already accepts these via serde_json::Value

### Response Format (Gap 6)
- **D-25:** `SandboxResult` adds two new fields: `tool_calls_made: u32` (from RPC server counter) and `duration_seconds: f64` (wall clock time)
- **D-26:** `ExecuteCodeTool::execute()` returns a JSON string (not text sections) with fields: `status` (success/error/timeout/interrupted), `output` (combined stdout text), `stderr` (on non-zero exit), `tool_calls_made`, `duration_seconds`, `exit_code`
- **D-27:** Status values: `"success"` (exit 0), `"error"` (non-zero exit), `"timeout"` (killed by timeout), `"interrupted"` (killed by user message)

### Stderr Cap (Gap 4)
- **D-28:** Truncate stderr at 10KB (10,240 bytes) with `[stderr truncated at 10KB]` notice
- **D-29:** `SandboxConfig` gets a new field `max_stderr_bytes: usize` defaulting to 10,240
- **D-30:** Stderr is always included in the response for debugging (not just on non-zero exit) but truncated independently of stdout

### Kill Strategy (Gap 5)
- **D-31:** On timeout: send SIGTERM to the process group first, wait 5 seconds for graceful shutdown, then SIGKILL the process group
- **D-32:** The child process runs in its own process group (set via `pre_exec` with `libc::setpgid(0, 0)`)
- **D-33:** Timeout message: `"Script timed out after {N}s and was killed."`
- **D-34:** Add `libc` as a dependency of `ironhermes-exec` for `setpgid` and `killpg`

### Env Stripping (Gap 3)
- **D-35:** Instead of `env_clear()` + explicit allowlist, use pattern-based exclusion: start with the full environment, then strip any variable whose name contains KEY, TOKEN, SECRET, PASSWORD, CREDENTIAL, PASSWD, or AUTH (case-insensitive)
- **D-36:** Always pass through safe system vars regardless of pattern match: PATH, HOME, LANG, SHELL, USER, LOGNAME, TERM, PYTHONPATH, VIRTUAL_ENV, PYTHONDONTWRITEBYTECODE, TMPDIR, XDG_*
- **D-37:** Always inject: IRONHERMES_RPC_ADDR (socket path), IRONHERMES_SESSION_ID, PYTHONPATH (prepend hermes_tools dir)

### User Interruption (Gap 7)
- **D-38:** The sandbox run must accept an optional `CancellationToken` (or `tokio::sync::watch` channel) that, when triggered, kills the child process immediately
- **D-39:** On interruption, return `SandboxResult` with status `"interrupted"` and stderr `"[execution interrupted — user sent a new message]"`
- **D-40:** The `ExecuteCodeTool` receives the cancellation signal from the agent loop. The agent loop integration point is in `handle_function_call` or wherever tool execution is awaited.

### Claude's Discretion
- Exact implementation of the cancellation plumbing (watch channel vs CancellationToken vs abort handle)
- Whether `libc::setpgid` is called in an unsafe `pre_exec` block or via the `nix` crate
- How `tool_calls_made` is surfaced from `RpcServer` back to `SandboxResult` (Arc<AtomicU32> shared reference, or returned via join handle)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Existing implementation (modify in place)
- `crates/ironhermes-exec/src/sandbox.rs` — Sandbox::run(), SandboxResult, env setup, timeout logic
- `crates/ironhermes-exec/src/rpc_server.rs` — RpcServer, call_count AtomicU32, ALLOWED_TOOLS
- `crates/ironhermes-exec/src/hermes_tools.py` — Python helper module with tool functions
- `crates/ironhermes-exec/src/lib.rs` — Crate root, SandboxConfig, ToolDispatch trait
- `crates/ironhermes-tools/src/execute_code.rs` — ExecuteCodeTool, response formatting
- `crates/ironhermes-core/src/config.rs` — ExecConfig struct

### Patterns to follow
- `crates/ironhermes-tools/src/terminal.rs` — TerminalTool for process management patterns
- `crates/ironhermes-agent/src/agent_loop.rs` — Agent loop for interruption integration point

### Requirements
- `.planning/REQUIREMENTS.md` — EXEC-01 through EXEC-04

</canonical_refs>

<code_context>
## Existing Code Insights

### What Already Works (don't break)
- UDS socket lifecycle and JSON-RPC 2.0 protocol
- Call limit enforcement with -32000 error code
- HermesCallLimitError / HermesRpcError in Python
- Concurrent stdout/stderr draining
- UTF-8 boundary truncation for stdout
- execute_code excluded from RPC allowlist
- Temp dir per execution with RAII cleanup
- All existing tests in sandbox.rs, rpc_server.rs, execute_code.rs

### Integration Points for Changes
- `SandboxResult` struct — add `tool_calls_made` and `duration_seconds` fields
- `SandboxConfig` — add `max_stderr_bytes` field
- `Sandbox::run()` — change env setup, kill strategy, add cancellation support
- `ExecuteCodeTool::execute()` — change response format from text to JSON
- `Cargo.toml` — add `libc` dependency
- Tests — update all assertions that check `[stdout]/[stderr]/[exit_code]` format

</code_context>

<specifics>
## Specific Ideas

1. **Env stripping pattern list:** `["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL", "PASSWD", "AUTH"]` — check case-insensitive via `name.to_uppercase().contains(pattern)`

2. **Response JSON format:**
   ```json
   {
     "status": "success",
     "output": "script stdout here",
     "stderr": "any stderr",
     "exit_code": 0,
     "tool_calls_made": 3,
     "duration_seconds": 1.234
   }
   ```

3. **Process group kill sequence:**
   ```rust
   // SIGTERM to process group
   unsafe { libc::killpg(pgid, libc::SIGTERM); }
   // Wait 5 seconds
   tokio::time::sleep(Duration::from_secs(5)).await;
   // SIGKILL to process group
   unsafe { libc::killpg(pgid, libc::SIGKILL); }
   ```

4. **Cancellation integration:** The agent loop already has a message-processing loop. When a new user message arrives during tool execution, signal the cancellation token. The sandbox select!s on the token alongside the child process wait.

</specifics>

<deferred>
## Deferred Ideas

- Tool subset changes (add terminal foreground-only, rename web_read→web_extract, remove memory) — separate decision
- Config section rename from `exec` to `code_execution` — cosmetic, low priority
- Skill env passthrough — requires skill system integration
- Windows platform detection / auto-disable — future phase
- `env_passthrough` config list (terminal.env_passthrough) — future config enhancement

</deferred>

---

*Phase: 08-code-execution (v2 gap closure)*
*Context gathered: 2026-04-10*
