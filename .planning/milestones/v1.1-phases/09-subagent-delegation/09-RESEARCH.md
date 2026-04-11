# Phase 9: Subagent Delegation - Research

**Researched:** 2026-04-10
**Domain:** Rust async, tokio::sync::Semaphore, AgentLoop composition, ToolRegistry filtering
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Tool Filtering Strategy**
- D-01: Allowlist pattern — parent passes a list of tool names, a new ToolRegistry is built containing only those tools. Same proven pattern as Phase 8's `rpc_registry` for execute_code.
- D-02: Default safe subset when parent doesn't specify tools: `read_file`, `write_file`, `patch`, `search_files`, `web_search`, `web_read`, `memory` — same as Phase 8's RPC safe subset, excluding `terminal`, `execute_code`, and `delegate_task`.
- D-03: Parent can override the default and grant any tool except `delegate_task`.
- D-04: Validation at build time — validate the allowlist against available tools and strip `delegate_task` before child starts. Fail early if an unknown tool is requested.
- D-05: No skills for subagents — SkillsTool excluded from child toolsets.

**Subagent Lifecycle & Result Format**
- D-06: Blocking tool call — `delegate_task` blocks like any other tool call.
- D-07: Final text response only — returns `AgentResult.final_response`.
- D-08: Configurable timeout via `agent.subagent_timeout` in config.yaml, default 300 seconds.
- D-09: Both wall-clock timeout AND turn limit — configurable `max_iterations` default 10 turns.

**Session & Terminal Isolation**
- D-10: Separate TerminalTool instance with unique temp CWD per subagent.
- D-11: Fresh conversation with task as system prompt — no parent history inherited.
- D-12: Read-only memory access — subagent can read from MEMORY.md but cannot write.
- D-13: Temp working directory cleaned up on completion.

**Concurrency & Queueing**
- D-14: Global concurrency limit — single `tokio::sync::Semaphore` shared across CLI and gateway.
- D-15: Block and wait with message when limit is hit — emit "Waiting for a subagent slot (3/3 in use)" before blocking.
- D-16: Configurable concurrency limit via `agent.max_subagents` in config.yaml, default 3.

### Claude's Discretion
- Whether `DelegateTaskTool` lives in `ironhermes-tools` or needs a new crate
- System prompt format for the child agent
- How the "waiting for slot" message is surfaced (tool progress callback vs inline text)
- Temp directory naming convention and location
- Whether the child's LlmClient reuses the parent's or creates a new one
- Number of plans — default to 3 per ROADMAP, but planner may adjust

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| AGENT-01 | Agent can delegate tasks to child agents via a delegate_task tool with isolated context | DelegateTaskTool wraps a fresh AgentLoop; child starts from system prompt only |
| AGENT-02 | Parent agent specifies which tools the child agent can use via a filtered ToolRegistry | ToolRegistry already supports filtered dispatch; same pattern as rpc_registry in Phase 8 |
| AGENT-03 | Maximum 3 concurrent subagents enforced via semaphore | tokio::sync::Semaphore with configurable permits; gateway already uses this pattern |
| AGENT-04 | Each subagent gets its own terminal session scope to prevent state bleed | TerminalTool CWD is passed at construction; tempdir per subagent gives isolation |
| AGENT-05 | Recursive delegation is prevented — delegate_task is excluded from child agent toolsets | Child registry built without registering DelegateTaskTool; structural not runtime check |
</phase_requirements>

---

## Summary

Phase 9 delivers `delegate_task`: a blocking agent tool that spawns a fresh `AgentLoop` with a filtered `ToolRegistry`, enforces a global concurrency semaphore, and returns the child's `final_response` string. The implementation is an almost-direct extension of two existing patterns: the `rpc_registry` restricted-registry pattern from Phase 8 (`register_execute_code_tool`) and the `tokio::sync::Semaphore` pattern already used in `GatewayRunner`.

The key structural insight is that the `ToolRegistry` does not clone tools — tools stored as `Box<dyn Tool>` are not `Clone`. The child registry must be built by re-registering tool instances (or factory functions), not by cloning from the parent. `DelegateTaskTool::new()` must receive whatever shared state it needs at construction time (LlmClient, MemoryStore, Semaphore) so it can build a child registry on each `execute()` call. This is the same pattern `ExecuteCodeTool` uses with its `rpc_registry: Arc<ToolRegistry>`.

`AgentLoop::new(client, child_registry, max_iterations)` takes ownership of a fresh registry and runs to completion. The tool returns `result.final_response.unwrap_or_default()`. The semaphore permit is held for the entire duration of the child run and dropped automatically on permit drop.

**Primary recommendation:** Place `DelegateTaskTool` in `ironhermes-tools` (no new crate needed). Build child registry by re-constructing tool instances from shared `Arc<...>` state stored in `DelegateTaskTool`. Use `tokio::time::timeout` wrapping `agent_loop.run()`, mirroring the sandbox pattern in `ironhermes-exec`.

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| tokio::sync::Semaphore | tokio 1 (workspace) | Concurrency limit enforcement | Already in workspace; used in gateway runner |
| tokio::time::timeout | tokio 1 (workspace) | Wall-clock timeout on child run | Already used in sandbox.rs and terminal.rs |
| tempfile::TempDir | tempfile 3 (dev-dep, also in exec) | Isolated CWD for child terminal | Already used in sandbox.rs for temp dirs |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| Arc\<ToolRegistry\> | internal | Share parent registry reference in DelegateTaskTool | DelegateTaskTool holds parent's Arc to enumerate available tools |
| LlmClient: Clone | internal | Cloneable HTTP client (reqwest::Client is Arc internally) | Child gets client.clone() — no new HTTP connection pool |

**Installation:** No new dependencies. `tempfile` is already in `ironhermes-exec`; add it to `ironhermes-tools` dev-dependencies if not already present. All other dependencies are workspace-pinned.

**Version verification:** [VERIFIED: Cargo.toml workspace] tokio 1, tempfile 3 in exec sandbox.

---

## Architecture Patterns

### Recommended Project Structure

```
crates/ironhermes-tools/src/
├── delegate_task.rs    # DelegateTaskTool — new file, mirrors execute_code.rs structure
└── registry.rs         # Add register_delegate_task_tool() after register_execute_code_tool()

crates/ironhermes-core/src/
└── config.rs           # Add SubagentConfig to AgentConfig (or extend AgentConfig directly)

crates/ironhermes-cli/src/
└── main.rs             # Wire Semaphore creation + pass to register_delegate_task_tool()
```

### Pattern 1: Child Registry Construction (Allowlist)

**What:** Build a filtered ToolRegistry for the child by re-registering tools from shared state. `delegate_task` is never registered.

**When to use:** Inside `DelegateTaskTool::execute()` on every invocation.

```rust
// Source: codebase — mirrors rpc_registry pattern in run_gateway() in main.rs
fn build_child_registry(
    allowed_tools: &[String],
    memory_store: Option<Arc<Mutex<MemoryStore>>>,
    // ...other shared state
) -> anyhow::Result<ToolRegistry> {
    let mut registry = ToolRegistry::new();

    // Register each allowed tool by name; skip delegate_task unconditionally
    for tool_name in allowed_tools {
        match tool_name.as_str() {
            "delegate_task" => { /* silently skip — AGENT-05 */ }
            "read_file" => registry.register(Box::new(ReadFileTool)),
            "write_file" => registry.register(Box::new(WriteFileTool)),
            "patch" => registry.register(Box::new(PatchFileTool)),
            "search_files" => registry.register(Box::new(SearchFilesTool)),
            "web_search" => registry.register(Box::new(WebSearchTool)),
            "web_read" => registry.register(Box::new(WebReadTool)),
            "memory" => {
                if let Some(ref store) = memory_store {
                    registry.register_memory_tool(store.clone());
                }
            }
            "terminal" => registry.register(Box::new(TerminalTool)),
            other => anyhow::bail!("Unknown tool in allowed_tools: {}", other),
        }
    }
    Ok(registry)
}
```

### Pattern 2: Semaphore Acquire with "Waiting" Message

**What:** Acquire semaphore permit, emitting a user-visible message before blocking. Permit is held for child agent lifetime.

**When to use:** Inside `DelegateTaskTool::execute()`, before child AgentLoop construction.

```rust
// Source: codebase — mirrors gateway runner.rs Semaphore::new(max_concurrent)
// D-15: emit waiting message before blocking
let available = self.semaphore.available_permits();
let total = self.max_subagents;
if available == 0 {
    // Surface message — tool_progress_callback not available inside Tool::execute()
    // Return this as part of the tool result prefix, or emit via tracing::info!
    tracing::info!("Waiting for a subagent slot ({}/{} in use)", total, total);
}
let _permit = self.semaphore.acquire().await
    .map_err(|e| anyhow::anyhow!("Semaphore closed: {}", e))?;
// permit held until dropped at end of scope
```

**Note on surfacing the waiting message:** `Tool::execute()` returns `anyhow::Result<String>` — there is no streaming callback. The waiting message must be surfaced via `tracing::info!` (appears in logs) or prepended to the final result string. The planner should decide between these — both are acceptable per "Claude's Discretion".

### Pattern 3: Child AgentLoop Construction and Run

**What:** Construct a fresh AgentLoop with the child registry and run to completion under timeout.

**When to use:** After acquiring the semaphore permit.

```rust
// Source: codebase — AgentLoop::new signature from agent_loop.rs line 63
// D-11: task description becomes sole system prompt
let system_msg = ChatMessage::system(&task_description);
let messages = vec![system_msg];

let child_loop = AgentLoop::new(
    self.client.clone(),       // LlmClient is Clone (reqwest::Client is Arc internally)
    Arc::new(child_registry),  // freshly built filtered registry
    self.subagent_max_iterations, // D-09: turn limit
);

// D-08: wall-clock timeout wrapping run()
let result = tokio::time::timeout(
    Duration::from_secs(self.subagent_timeout_secs),
    child_loop.run(messages),
)
.await
.map_err(|_| anyhow::anyhow!("Subagent timed out after {}s", self.subagent_timeout_secs))??;

// D-07: return final_response string only
Ok(result.final_response.unwrap_or_else(|| "(no response)".to_string()))
```

### Pattern 4: DelegateTaskTool Constructor

**What:** Store all shared state needed to build child registries on each call.

```rust
// Source: codebase — mirrors ExecuteCodeTool::new pattern
pub struct DelegateTaskTool {
    client: LlmClient,
    semaphore: Arc<Semaphore>,
    memory_store: Option<Arc<Mutex<MemoryStore>>>,
    subagent_timeout_secs: u64,
    subagent_max_iterations: usize,
    max_subagents: usize,
}

impl DelegateTaskTool {
    pub fn new(
        client: LlmClient,
        semaphore: Arc<Semaphore>,
        memory_store: Option<Arc<Mutex<MemoryStore>>>,
        config: SubagentConfig,
    ) -> Self { ... }
}
```

### Pattern 5: Temp CWD for Terminal Isolation (AGENT-04)

**What:** Create a unique TempDir for each child's TerminalTool. The temp dir keeps child filesystem work separate.

```rust
// Source: codebase — mirrors Sandbox::run() in sandbox.rs line 45
// D-10: separate TerminalTool instance per child with temp CWD
let child_tempdir = tempfile::TempDir::new()?;
// child_tempdir kept alive until end of execute() — D-13: cleaned up on completion
// Pass child_tempdir.path() as the CWD to a TerminalTool instance
// Note: current TerminalTool has no CWD field — see Pitfall 1 below
```

### Pattern 6: Config Extension

**What:** Add `SubagentConfig` to `ironhermes-core/src/config.rs` and wire into `AgentConfig` or as a top-level field.

```rust
// Source: codebase — mirrors ExecConfig pattern in config.rs line 222
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentConfig {
    pub timeout_secs: u64,       // D-08: default 300
    pub max_subagents: usize,    // D-16: default 3
    pub max_iterations: usize,   // D-09: default 10
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 300,
            max_subagents: 3,
            max_iterations: 10,
        }
    }
}
```

Add to `Config`:
```rust
pub struct Config {
    // ... existing fields
    pub subagent: SubagentConfig,  // new field
}
```

### Pattern 7: Registration in main.rs / run_gateway

**What:** Create the shared Semaphore at startup and pass to `register_delegate_task_tool()`.

```rust
// Source: codebase — mirrors register_execute_code_tool call in run_gateway (main.rs ~line 423)
let subagent_semaphore = Arc::new(Semaphore::new(config.subagent.max_subagents));
registry.register_delegate_task_tool(
    client.clone(),
    subagent_semaphore,
    Some(memory_store.clone()),
    config.subagent.clone(),
);
// NOTE: memory_store passed as read-only — D-12 enforced by building child
// registry with MemoryTool in read-only mode (see Pitfall 3)
```

### Anti-Patterns to Avoid

- **Cloning ToolRegistry:** `Box<dyn Tool>` is not `Clone`. Never attempt `registry.clone()` — it won't compile. Always re-construct tool instances for child registry.
- **Returning mid-execution on permit failure:** `Semaphore::acquire()` returns `Err` only if the semaphore is closed (dropped). In normal operation this should never fail. Do not silently swallow this error.
- **Spawning child in a tokio::spawn task:** `delegate_task` is a blocking tool call (D-06). Run the child loop directly in `execute()` with `.await`. Using `spawn` would return immediately before the child completes.
- **Registering DelegateTaskTool in the child:** The only reliable prevention of AGENT-05 is to never call `register_delegate_task_tool()` when building the child registry. No runtime check is needed.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Concurrency limiting | Custom counter + Mutex | `tokio::sync::Semaphore` | Semaphore handles backpressure, wait queuing, and drop-based release correctly |
| Wall-clock timeout | tokio::spawn + sleep + channel | `tokio::time::timeout` | One-liner, composes with async, proven in sandbox.rs and terminal.rs |
| Isolated temp directory | Manual mkdir + manual cleanup | `tempfile::TempDir` | RAII cleanup on drop, handles cleanup even on panic |
| Tool schema filtering | Custom schema list | `ToolRegistry::get_definitions(enabled_tools)` | Already exists; filters schema list by name |

**Key insight:** The `ToolRegistry` dispatch path (`execute_tool`) already supports filtering at the schema level (`get_definitions`) but NOT at the dispatch level — an unknown tool name returns an error from `dispatch()`. The child registry enforces access control at registry construction time, which is stronger than dispatch-time filtering.

---

## Common Pitfalls

### Pitfall 1: TerminalTool Has No CWD Field

**What goes wrong:** The current `TerminalTool` struct (terminal.rs line 12) is a unit struct — `pub struct TerminalTool;`. There is no CWD configuration on the struct. All terminal commands run in the process's current working directory, which is shared.

**Why it happens:** TerminalTool was built for single-agent CLI use where CWD is the user's shell CWD.

**How to avoid:** For AGENT-04 isolation, one of:
1. Add a `cwd: Option<PathBuf>` field to `TerminalTool` (preferred — clean, configurable)
2. Prepend `cd /tmp/subagent-xyz && ` to every command at the DelegateTaskTool level (fragile)
3. If `terminal` is excluded from the child's default safe subset (D-02 excludes it), this only matters when the parent explicitly grants `terminal` to the child (D-03)

**Warning signs:** Two subagents running `cd /tmp/work` in parallel both affect shared CWD.

**Planner note:** The plan should include adding `cwd: Option<PathBuf>` to `TerminalTool` and constructing it with the tempdir path when building child registries that include `terminal`.

### Pitfall 2: Memory Read-Only Enforcement (D-12)

**What goes wrong:** `MemoryTool` currently has full read/write access. If the child gets the same `MemoryTool` instance pointing to the same `MemoryStore`, it can corrupt the parent's persistent memory.

**Why it happens:** `MemoryStore` is a shared `Arc<Mutex<MemoryStore>>`. Whoever holds it can write.

**How to avoid:** Two options:
1. Add a `read_only: bool` flag to `MemoryTool` — when true, allow `query`/`get` actions but return an error for `save`/`forget`
2. Give the child a separate in-memory `MemoryStore` pre-populated with the parent's facts (heavier)

**Recommended:** Option 1 (read_only flag) is minimal and precise. The `MemoryTool::new()` constructor gets a new `read_only` parameter.

**Warning signs:** Test: child calling `memory save "bad fact"` should return an error when read-only is enforced.

### Pitfall 3: Semaphore Must Be Created Before Arc Wrapping Registry

**What goes wrong:** The `Semaphore` must be created at startup and passed to `DelegateTaskTool` at construction time. If created inside `execute()`, each call gets its own semaphore with fresh permits — concurrency limit is never enforced.

**Why it happens:** Easy to misplace initialization if following the `Sandbox::new()` pattern (which creates fresh resources per call).

**How to avoid:** Create `Arc<Semaphore>` once in `run_gateway()` and `run_chat()` / `run_single()` (for CLI), then pass to `register_delegate_task_tool()`. Same initialization site as `ExecConfig`.

**Warning signs:** 10 concurrent subagents completing without any "Waiting" messages when limit is set to 3.

### Pitfall 4: AgentLoop active_skills Field

**What goes wrong:** `AgentLoop::new()` initializes `active_skills` with an empty `Arc<Mutex<Vec<SkillRecord>>>` by default. The child loop will not inherit parent skills. This is correct behavior (D-05 excludes skills from subagents), but if the planner adds a `.with_active_skills()` call, skills could leak into the child.

**Why it happens:** Convenience method `.with_active_skills()` exists on `AgentLoop`.

**How to avoid:** Never call `.with_active_skills()` on the child loop. The default empty vec is correct.

### Pitfall 5: tool_progress_callback Not Available in Tool::execute()

**What goes wrong:** The "Waiting for a subagent slot" message (D-15) cannot be delivered via the parent's `ToolProgressCallback` because `Tool::execute()` receives only `serde_json::Value` args and returns `anyhow::Result<String>`. There is no callback channel.

**Why it happens:** The progress callback is held by `AgentLoop`, not passed through the tool dispatch chain.

**How to avoid:** Emit the waiting message via `tracing::info!` (visible in logs) and optionally prepend a status line to the tool result if the wait occurred. The planner should make a clear call on this — both approaches are acceptable.

### Pitfall 6: Unknown Tool Names in Allowlist Should Fail Early

**What goes wrong:** If the parent passes `allowed_tools: ["read_file", "typo_tool"]`, and `typo_tool` doesn't exist in the match arms, the child silently gets a registry without it. The child then fails at runtime when it tries to call the unknown tool.

**Why it happens:** Silent skip vs fail-early not specified.

**How to avoid:** Per D-04 "fail early if an unknown tool is requested" — return `Err` from `build_child_registry()` if any tool name is unrecognized. The parent's `execute()` returns this as an error string to the LLM.

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Separate registry crate for each isolated tool subset | Single ToolRegistry with allowlist filtering (same as rpc_registry pattern) | Phase 8 | No new crate needed for Phase 9 |
| Recursive delegation via runtime check | Structural prevention — never register delegate_task in child registry | Phase 9 design | Stronger: no runtime bypass possible |

---

## Code Examples

Verified patterns from codebase:

### AgentLoop Constructor (from agent_loop.rs line 63)
```rust
pub fn new(client: LlmClient, registry: Arc<ToolRegistry>, max_iterations: usize) -> Self
```

### Semaphore Creation (from gateway/runner.rs line 125)
```rust
let semaphore = Arc::new(Semaphore::new(max_concurrent));
```

### tokio::time::timeout (from exec/sandbox.rs pattern, terminal.rs line 92)
```rust
let result = timeout(Duration::from_secs(timeout_secs), fut)
    .await
    .map_err(|_| anyhow::anyhow!("Timed out after {}s", timeout_secs))??;
```

### TempDir (from exec/sandbox.rs line 45)
```rust
let dir = tempfile::TempDir::new()?;
// dir lives until end of scope; cleaned up on drop (D-13)
```

### register_execute_code_tool pattern (from registry.rs line 246-260)
```rust
pub fn register_execute_code_tool(
    &mut self,
    rpc_registry: Arc<ToolRegistry>,
    config: ironhermes_core::ExecConfig,
) {
    use crate::execute_code::ExecuteCodeTool;
    self.register(Box::new(ExecuteCodeTool::new(rpc_registry, config)));
}
```

The `register_delegate_task_tool` method will follow this same signature shape.

### tool schema argument for delegate_task
```rust
json!({
    "type": "object",
    "properties": {
        "task": {
            "type": "string",
            "description": "Task description for the child agent to complete."
        },
        "allowed_tools": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Tools the child agent may use. Defaults to safe subset if omitted."
        }
    },
    "required": ["task"]
})
```

---

## Environment Availability

Step 2.6: SKIPPED — Phase 9 is purely code changes (Rust crate additions). No external CLI tools, services, or runtimes beyond the existing Tokio runtime are required. `tempfile` crate is already a dependency in `ironhermes-exec`; it needs to be added to `ironhermes-tools` `[dependencies]` (not dev-dependencies) if TerminalTool CWD isolation uses it at runtime.

**One dependency to add:** `tempfile = "3"` to `crates/ironhermes-tools/Cargo.toml` `[dependencies]` section (currently only in exec crate).

---

## Validation Architecture

Config: `workflow.nyquist_validation` not set to false — validation section included.

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[tokio::test]` via `cargo test` |
| Config file | none — standard cargo test runner |
| Quick run command | `cargo test -p ironhermes-tools delegate_task` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| AGENT-01 | delegate_task tool returns child's final_response string | unit | `cargo test -p ironhermes-tools delegate_task::tests::test_delegate_task_returns_response` | No — Wave 0 |
| AGENT-02 | Child registry only contains allowed tools; unlisted tools fail | unit | `cargo test -p ironhermes-tools delegate_task::tests::test_child_registry_filtered` | No — Wave 0 |
| AGENT-03 | 4th concurrent delegate_task blocks until one completes | unit | `cargo test -p ironhermes-tools delegate_task::tests::test_semaphore_blocks_at_limit` | No — Wave 0 |
| AGENT-04 | Two concurrent children have separate terminal CWDs | unit | `cargo test -p ironhermes-tools delegate_task::tests::test_terminal_isolation` | No — Wave 0 |
| AGENT-05 | Child registry never contains delegate_task tool | unit | `cargo test -p ironhermes-tools delegate_task::tests::test_no_recursive_delegation` | No — Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-tools delegate`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full workspace green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-tools/src/delegate_task.rs` — covers AGENT-01..05 (new file)
- [ ] Tests can use a mock `LlmClient` that returns a canned response — check if `LlmClient` is mockable or if tests need a real client (existing tests in agent_loop.rs construct `LlmClient::new("http://localhost", "", "mock-model")` and test only the dispatch path without real LLM calls)

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `TerminalTool` needs a `cwd` field added to support per-subagent isolation | Common Pitfalls #1 | If terminal is excluded from the default child toolset and the planner decides to only grant it explicitly, this may be deferred — low risk |
| A2 | `MemoryTool` needs a `read_only` flag added to enforce D-12 | Common Pitfalls #2 | If the MemoryStore is not wired into child registries at all (simplest D-12 enforcement), no code change to MemoryTool is needed |
| A3 | `tempfile` needs to be added to ironhermes-tools `[dependencies]` (not dev-deps) | Environment Availability | If TerminalTool CWD isolation is implemented differently (e.g., env var, not TempDir), tempfile may only be needed in tests |

---

## Open Questions

1. **TerminalTool CWD isolation mechanism**
   - What we know: Current `TerminalTool` is a unit struct with no CWD field
   - What's unclear: Whether the planner should add `cwd: Option<PathBuf>` to `TerminalTool` in this phase or handle isolation differently (e.g., exclude terminal from child defaults and treat it as an advanced opt-in that inherits process CWD)
   - Recommendation: Add `cwd: Option<PathBuf>` to `TerminalTool` in Plan 1 (struct setup). The CONTEXT.md decision D-10 explicitly calls for separate TerminalTool instance with unique temp CWD — this requires a CWD field.

2. **Waiting message surfacing**
   - What we know: `Tool::execute()` has no callback channel; progress callbacks live on `AgentLoop`
   - What's unclear: Whether a `tracing::info!` log is sufficient for D-15, or if the final result string should be prefixed with the wait notification
   - Recommendation: Emit `tracing::info!` before acquiring, and if wait occurs (available_permits == 0 at check time), prepend `"[Waited for a subagent slot]\n"` to the result string so the LLM sees it.

3. **LlmClient for child agent**
   - What we know: `LlmClient: Clone`, `LlmClient` wraps `reqwest::Client` which is `Arc` internally
   - What's unclear: Whether `DelegateTaskTool` receives the client at construction or extracts config and constructs a fresh one
   - Recommendation: Accept `LlmClient` at construction and clone per child invocation. No new HTTP connection pool is created — `reqwest::Client` shares the pool via its internal `Arc`.

---

## Security Domain

Security enforcement is enabled (not explicitly set to false in config.json).

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | N/A — no user auth in tool layer |
| V3 Session Management | no | Child session is ephemeral, no persistence |
| V4 Access Control | yes | Allowlist filtering — child cannot call unlisted tools |
| V5 Input Validation | yes | Validate `allowed_tools` list against known tools (D-04); fail early on unknown tool names |
| V6 Cryptography | no | No crypto in subagent delegation |

### Known Threat Patterns for this Stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via task description | Tampering | No mitigation possible at this layer — task comes from parent LLM; parent is trusted |
| Recursive delegation (delegate_task calling delegate_task) | Elevation of privilege | Structural prevention: never register delegate_task in child registry (AGENT-05) |
| Resource exhaustion via unlimited subagents | Denial of service | Semaphore with configurable max_subagents (default 3) and timeout (default 300s) |
| Child writing to parent memory store | Tampering | D-12: read-only MemoryTool in child; `save`/`forget` actions return error |
| Child accessing parent terminal state | Information disclosure | D-10: isolated TempDir per child; unique CWD for TerminalTool instance |

---

## Sources

### Primary (HIGH confidence)
- `crates/ironhermes-tools/src/registry.rs` — ToolRegistry struct, Tool trait, register_execute_code_tool pattern (directly read)
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop::new(), ::run(), AgentResult struct (directly read)
- `crates/ironhermes-tools/src/terminal.rs` — TerminalTool unit struct, CWD absence confirmed (directly read)
- `crates/ironhermes-tools/src/execute_code.rs` — ExecuteCodeTool pattern, DelegateTaskTool template (directly read)
- `crates/ironhermes-core/src/config.rs` — Config struct, ExecConfig pattern, AgentConfig (directly read)
- `crates/ironhermes-gateway/src/runner.rs` — Semaphore creation and usage pattern (directly read)
- `crates/ironhermes-exec/src/sandbox.rs` — tokio::time::timeout wrapping async work, TempDir usage (directly read)
- `crates/ironhermes-cli/src/main.rs` — build_registry(), register_execute_code_tool() call site, Semaphore not yet wired in CLI path (directly read)
- `Cargo.toml` — workspace dependencies, confirmed tokio 1, tempfile 3 (directly read)

### Secondary (MEDIUM confidence)
- `09-CONTEXT.md` — All decisions D-01..D-16, specific ideas section (directly read)
- `.planning/codebase/ARCH.md` — Crate dependency graph, concurrency model (directly read)

### Tertiary (LOW confidence)
None — all claims verified from codebase or CONTEXT.md.

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all libraries verified in Cargo.toml and codebase
- Architecture: HIGH — patterns traced directly from execute_code and gateway runner code
- Pitfalls: HIGH for TerminalTool CWD (unit struct confirmed), MEDIUM for memory read-only (MemoryTool code not directly inspected in this session)
- Security: HIGH — ASVS categories derived from known tech stack

**Research date:** 2026-04-10
**Valid until:** 2026-05-10 (stable Rust async patterns; no fast-moving dependencies)
