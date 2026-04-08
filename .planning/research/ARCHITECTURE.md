# Architecture Patterns: v1.1 Automation Integration

**Domain:** Rust agent automation — subagent delegation, Python code execution, event hooks, batch processing
**Researched:** 2026-04-07
**Overall confidence:** HIGH (derived from direct codebase analysis + known Rust async patterns)

---

## Executive Summary

IronHermes already has the right architectural bones for automation. The `Tool` trait, `Arc<ToolRegistry>`, and `AgentLoop` are the three integration points everything else plugs into. All four automation features (subagent delegation, code execution, event hooks, batch processing) follow the same pattern: implement `Tool`, register in the registry, and the agent loop invokes them like any other tool. No new agent loop changes are required for basic integration.

What varies per feature is what lives *inside* the tool implementation and whether it needs additional supporting infrastructure (a new crate, a background task, a side-channel channel).

---

## Recommended Architecture

### Component Boundaries (post-v1.1)

```
Workspace
├── ironhermes-core          [no changes needed]
│   └── Config, ChatMessage, PromptBuilder, ContextScanner, MemoryStore
│
├── ironhermes-state         [no changes needed]
│   └── SQLite StateStore, FTS5 search, WAL mode
│
├── ironhermes-tools         [ADD: 3 new tool impls]
│   ├── registry.rs          Tool trait + ToolRegistry (no changes)
│   ├── delegate_task.rs     NEW — subagent delegation tool
│   ├── execute_code.rs      NEW — Python RPC tool
│   └── batch.rs             NEW — batch processing tool
│
├── ironhermes-agent         [MODIFY: hook points + CancellationToken awareness]
│   ├── agent_loop.rs        ADD pre/post-tool hook invocation
│   └── client.rs            no changes
│
├── ironhermes-hooks         NEW CRATE
│   ├── lib.rs               HookRegistry, HookEvent enum, HookHandler trait
│   ├── logging.rs           Structured logging hook
│   ├── alert.rs             Threshold-based alert hook
│   └── webhook.rs           HTTP POST hook
│
├── ironhermes-exec          NEW CRATE
│   ├── lib.rs               PythonRpcServer — stdio JSON-RPC bridge
│   ├── sandbox.rs           Process spawning, resource limits, timeout
│   └── protocol.rs          Request/Response types (serde_json)
│
├── ironhermes-cron          [MODIFY: attach skill/tool to jobs]
│   └── lib.rs               ADD skill_name field to CronJob
│
├── ironhermes-gateway       [MODIFY: hook invocation at message boundaries]
│   └── handler.rs           ADD pre-handle / post-handle hook calls
│
└── ironhermes-cli           [MODIFY: add automation subcommands]
    └── main.rs              ADD batch run, hook list, delegate test commands
```

---

## Feature Integration Details

### 1. Subagent Delegation (`delegate_task`)

**Integration point:** `ironhermes-tools`, registered as a standard `Tool`.

**How it works:**

The `DelegateTaskTool` receives JSON args: `{ task, toolset, max_turns, model? }`. It:
1. Builds a restricted `ToolRegistry` using the `enabled_tools` filter already present in `get_definitions(enabled_tools: Option<&[String]>)` — no registry changes needed, the filter is already there.
2. Constructs a child `AgentLoop` with a fresh `LlmClient`, the restricted registry, and a lower `max_iterations`.
3. Runs the child loop with a synthesized system prompt (no SOUL.md, task-focused only).
4. Returns the `AgentResult.final_response` as the tool result string.

**Concurrency:** Use `tokio::sync::Semaphore` with a permit count of 3 to cap concurrent subagents. The semaphore lives in the tool struct behind an `Arc`.

**No changes to AgentLoop.** `DelegateTaskTool` is self-contained; the parent loop sees it as any other tool call.

```
DelegateTaskTool {
    client_config: (base_url, api_key),
    semaphore: Arc<Semaphore>,         // permits = 3
}

impl Tool for DelegateTaskTool {
    async fn execute(&self, args) -> Result<String> {
        let _permit = self.semaphore.acquire().await?;
        let restricted_registry = build_restricted_registry(&args.toolset);
        let child = AgentLoop::new(client, Arc::new(restricted_registry), max_turns);
        let result = child.run(messages).await?;
        Ok(result.final_response.unwrap_or_default())
    }
}
```

**Key constraint:** The child registry must NOT contain `delegate_task` itself — prevents recursive delegation. Enforce this in the toolset allowlist, not the registry.

---

### 2. Code Execution (`execute_code`) — Python RPC

**Integration point:** `ironhermes-tools` tool + new `ironhermes-exec` crate.

**Architecture decision:** stdio JSON-RPC (not TCP, not Unix socket). Rationale: no port management, works in any environment, easy to audit.

**Protocol flow:**
```
execute_code tool (Rust)
  → spawn Python subprocess (ironhermes_exec/__main__.py)
  → write JSON request to child stdin
  → read JSON response from child stdout
  → kill process after timeout or on drop
```

**Python side:** A thin Python file at `~/.ironhermes/exec/runner.py` (or embedded as a string constant in `ironhermes-exec`). It reads a JSON request, calls the appropriate Hermes tool function, writes a JSON response. No network involved.

**`ironhermes-exec` crate responsibilities:**
- `PythonRpcClient` — manages subprocess lifecycle via `tokio::process::Command`
- `protocol.rs` — `ExecRequest { code: String, timeout_secs: u64 }` / `ExecResponse { stdout, stderr, exit_code }`
- `sandbox.rs` — enforces `ulimit`-style resource limits via `std::os::unix::process::CommandExt::pre_exec` (Linux) or equivalent; timeout via `tokio::time::timeout`

**`ExecuteCodeTool` in `ironhermes-tools`:**
```
ExecuteCodeTool {
    exec_client: Arc<PythonRpcClient>,
}

impl Tool for ExecuteCodeTool {
    async fn execute(&self, args) -> Result<String> {
        // args: { code, timeout_secs? }
        let resp = self.exec_client.run(args.code, timeout).await?;
        Ok(format!("exit={}\n{}\n{}", resp.exit_code, resp.stdout, resp.stderr))
    }
}
```

**Security boundary:** The Python subprocess has no network access unless granted. Enforce with `seccomp` or `unshare` on Linux; document the limitation on macOS.

---

### 3. Event Hooks

**Integration points:** `ironhermes-agent` (tool execution hooks) + `ironhermes-gateway` (message lifecycle hooks). Both call into a new `ironhermes-hooks` crate.

**Two hook namespaces:**

| Namespace | Where called | Events |
|-----------|-------------|--------|
| Gateway hooks | `handler.rs` | message_received, response_sent, error |
| Tool hooks | `agent_loop.rs` | before_tool, after_tool, tool_error |

**`ironhermes-hooks` crate:**

```rust
pub enum HookEvent {
    MessageReceived { chat_id: String, content: String },
    ResponseSent { chat_id: String, content: String },
    BeforeTool { name: String, args: serde_json::Value },
    AfterTool { name: String, result: String, duration_ms: u64 },
    ToolError { name: String, error: String },
    AgentError { chat_id: String, error: String },
}

#[async_trait]
pub trait HookHandler: Send + Sync {
    fn name(&self) -> &str;
    async fn on_event(&self, event: &HookEvent) -> anyhow::Result<()>;
}

pub struct HookRegistry {
    handlers: Vec<Box<dyn HookHandler>>,
}

impl HookRegistry {
    pub async fn fire(&self, event: HookEvent) { ... }
}
```

**Wiring into AgentLoop:** Add an `Option<Arc<HookRegistry>>` field. In `execute_tool_call`, fire `BeforeTool` before dispatch and `AfterTool`/`ToolError` after. Hook failures must not fail the agent — fire-and-log pattern.

**Wiring into GatewayMessageHandler:** Add `Option<Arc<HookRegistry>>` field (same pattern as `memory_store`). Fire `MessageReceived` at top of `handle_with_multimodal`, `ResponseSent` after agent result, `AgentError` on error path.

**Hook implementations in `ironhermes-hooks`:**
- `LoggingHook` — structured `tracing::info!` with all event fields
- `AlertHook` — fires when error count threshold exceeded in a rolling window
- `WebhookHook` — HTTP POST via `reqwest` (already in workspace deps)

**IMPORTANT:** Hooks are fire-and-forget. The main flow does not await hook completion or propagate hook errors. Use `tokio::spawn` for webhook calls to prevent slow HTTP from blocking the agent.

---

### 4. Batch Processing

**Integration point:** New subcommand in `ironhermes-cli`. Does not touch gateway or agent loop internals.

**Architecture:**

```
BatchRunner {
    concurrency: usize,               // semaphore permits
    tool_registry: Arc<ToolRegistry>,
    client_config: ClientConfig,
}

impl BatchRunner {
    pub async fn run(&self, prompts: Vec<String>) -> Vec<BatchResult> {
        let sem = Arc::new(Semaphore::new(self.concurrency));
        let handles: Vec<_> = prompts.into_iter().map(|prompt| {
            let sem = sem.clone();
            tokio::spawn(async move {
                let _permit = sem.acquire().await;
                // run AgentLoop, collect messages
            })
        }).collect();
        futures::future::join_all(handles).await
    }
}
```

**Output format:** ShareGPT JSON — array of `{ conversations: [{ from: "human"|"gpt", value: String }] }`. Write to `~/.ironhermes/batch/{timestamp}.jsonl`.

**CLI subcommand:** `ironhermes batch run --prompts prompts.txt --concurrency 4 --output trajectories.jsonl`

**BatchResult collects:** the full `AgentResult.messages` transcript, serialized into ShareGPT format. Tool calls are represented as `gpt` turns with structured content showing tool name + result.

---

## Data Flow Changes

### Current flow (v1.0):
```
Telegram update → GatewayMessageHandler → AgentLoop → ToolRegistry → tool.execute()
```

### v1.1 flow:
```
Telegram update
  → HookRegistry.fire(MessageReceived)         [NEW — gateway hook]
  → GatewayMessageHandler
  → AgentLoop
    → HookRegistry.fire(BeforeTool)            [NEW — tool hook]
    → ToolRegistry.dispatch()
      → delegate_task → child AgentLoop        [NEW — recursive but bounded]
      → execute_code → PythonRpcClient         [NEW — subprocess]
    → HookRegistry.fire(AfterTool/ToolError)   [NEW — tool hook]
  → HookRegistry.fire(ResponseSent/AgentError) [NEW — gateway hook]
```

### CronJob flow (extended):
```
CronScheduler tick
  → get_due_jobs()
  → for each job: AgentLoop.run(job.agent_input)    [currently: output to local]
  → deliver to: local | origin | platform:<chat_id>  [NEW — multi-platform delivery]
```

---

## What Existing Components Need Modification

| Component | File | Change | Scope |
|-----------|------|--------|-------|
| `AgentLoop` | `agent_loop.rs` | Add `Option<Arc<HookRegistry>>` field; fire hooks in `execute_tool_call` | Small — 2 method changes |
| `GatewayMessageHandler` | `handler.rs` | Add `Option<Arc<HookRegistry>>` field (like `memory_store`); fire hooks at message boundaries | Small — mirror `set_memory_store` pattern |
| `CronJob` | `cron/src/lib.rs` | Add `skill_name: Option<String>` and `deliver_platform: Option<String>` fields to struct; update serialization | Small — additive fields |
| `ToolRegistry` | `registry.rs` | Add `register_delegate_tool`, `register_exec_tool` convenience methods (optional) | Trivial — keep consistent with `register_memory_tool` |
| `Cargo.toml` (workspace) | root | Add `ironhermes-hooks` and `ironhermes-exec` to `[workspace.members]` | Trivial |

**Nothing in `ironhermes-core`, `ironhermes-state`, or `ironhermes-cli` (beyond new subcommands) requires structural changes.**

---

## New Crates Required

### `ironhermes-hooks`

**Dependencies:** `async-trait`, `tokio`, `tracing`, `serde`, `serde_json`, `reqwest`, `anyhow`

**Does NOT depend on:** `ironhermes-agent` (avoid circular dep). Agent and gateway depend on hooks, not the reverse.

**Dependency direction:** `ironhermes-hooks` is a leaf crate that only knows about `ironhermes-core` types (for event payloads like `ChatMessage`).

### `ironhermes-exec`

**Dependencies:** `tokio` (process, timeout), `serde`, `serde_json`, `anyhow`, `tracing`

**Does NOT depend on:** any ironhermes crate except `ironhermes-core` for shared types.

**Dependency direction:** `ironhermes-tools` depends on `ironhermes-exec`. Nothing depends on `ironhermes-exec` except tools.

---

## Dependency Graph (v1.1)

```
ironhermes-core
  ↑
ironhermes-state     ironhermes-hooks     ironhermes-exec
  ↑                       ↑                    ↑
ironhermes-tools ──────────┘                   │
  ↑               (hooks optional)             │
ironhermes-agent ──────────────────────────────┘
  ↑               (hooks optional)
ironhermes-gateway ── ironhermes-hooks (hooks optional)
  ↑
ironhermes-cron
  ↑
ironhermes-cli
```

All new dependencies are optional (`Option<Arc<...>>`) — the system degrades gracefully when hooks or exec are not configured.

---

## Suggested Build Order (Dependency-first)

### Phase 1: `ironhermes-hooks` crate (no blockers)
- Define `HookEvent`, `HookHandler`, `HookRegistry`
- Implement `LoggingHook`
- Wire into `AgentLoop` and `GatewayMessageHandler`
- This unblocks alert and webhook hooks but those can come later

**Why first:** No dependencies on new features. Establishes observability before adding complexity. `WebhookHook` and `AlertHook` are additive.

### Phase 2: `delegate_task` tool
- Implement `DelegateTaskTool` in `ironhermes-tools`
- Uses only existing `AgentLoop`, `LlmClient`, `ToolRegistry` — no new crates
- Add `Semaphore`-based concurrency cap

**Why second:** Highest value automation feature; depends only on crates that already exist.

### Phase 3: Cron extension (skill attachment + multi-platform delivery)
- Add `skill_name` and `deliver_platform` fields to `CronJob`
- Wire cron runner to invoke `AgentLoop` for due jobs
- Add delivery routing: local stdout / Telegram chat_id

**Why third:** Builds on cron infrastructure that already works. Does not need hooks or exec.

### Phase 4: `ironhermes-exec` crate + `execute_code` tool
- Build `ironhermes-exec` with stdio JSON-RPC bridge
- Write the Python runner script
- Implement `ExecuteCodeTool` in `ironhermes-tools`
- Add security boundary enforcement

**Why fourth:** Most complex feature, most security surface. Build after simpler tools prove the pattern.

### Phase 5: Batch processing CLI
- Add `BatchRunner` in `ironhermes-cli` (or a small module)
- Add `ironhermes batch run` subcommand
- Implement ShareGPT output serialization

**Why last:** Pure output feature, no blocking dependencies. Can be built any time after the agent loop is stable, but benefits from delegation and hooks being in place to generate richer trajectories.

---

## Anti-Patterns to Avoid

### Anti-Pattern 1: Hook failures blocking the agent
**What:** `await`-ing hook results and propagating errors to the agent loop.
**Why bad:** A misconfigured webhook kills every agent response.
**Instead:** Fire hooks with `tokio::spawn`; log failures with `tracing::warn!`; never return `Err` from `HookRegistry::fire`.

### Anti-Pattern 2: Subagent recursive delegation
**What:** Child agents receiving the `delegate_task` tool in their toolset.
**Why bad:** Unbounded recursion; N concurrent agents each spawning 3 more = exponential resource usage.
**Instead:** Never include `delegate_task` in the allowlist passed to child registry. Enforce in `DelegateTaskTool::execute`, not by convention.

### Anti-Pattern 3: Shared mutable `ToolRegistry` across subagents
**What:** Passing `Arc<ToolRegistry>` (the parent's registry) directly to child agents.
**Why bad:** Child gets all parent tools including delegation. No isolation.
**Instead:** Build a new `ToolRegistry` for each child from scratch with `register_defaults()` filtered to the allowlist.

### Anti-Pattern 4: Blocking the tokio runtime in `execute_code`
**What:** Using `std::process::Command` (blocking) instead of `tokio::process::Command`.
**Why bad:** Blocks the async runtime thread during Python subprocess execution.
**Instead:** Use `tokio::process::Command` throughout `ironhermes-exec`. Use `tokio::time::timeout` for the deadline.

### Anti-Pattern 5: Adding `skill_name` to `CronJob` as a plain String
**What:** Storing the LLM prompt directly as the "skill" and calling it a skill name.
**Why bad:** No separation between schedule metadata and agent instructions.
**Instead:** Keep `agent_input` as the prompt. Add `skill_name: Option<String>` as a human-readable label only. The cron runner always sends `agent_input` to the agent loop regardless.

---

## Scalability Considerations

| Concern | Current (v1.0) | After v1.1 |
|---------|---------------|------------|
| Concurrent agent sessions | One per Telegram chat (per-chat worker queue) | Same — delegation adds bounded subagents per session |
| Tool execution | Sequential within a turn | Sequential within a turn; subagents run concurrently with Semaphore cap |
| Hook overhead | N/A | Fire-and-forget; logging hook is synchronous and fast; webhook hook is async |
| Cron job execution | Not connected to agent loop | One AgentLoop per due job; jobs run sequentially in cron tick |
| Batch processing | N/A | Configurable semaphore; default 4 concurrent |

No changes to the gateway's per-chat worker architecture are needed for v1.1. Subagent concurrency is bounded independently per parent session.

---

## Sources

- Direct analysis: `crates/ironhermes-tools/src/registry.rs` — Tool trait, ToolRegistry, `get_definitions` filter
- Direct analysis: `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop structure, callback pattern, execute_tool_call
- Direct analysis: `crates/ironhermes-gateway/src/handler.rs` — GatewayMessageHandler, optional field pattern (`memory_store`), streaming bridge
- Direct analysis: `crates/ironhermes-cron/src/lib.rs` — CronJob, JobStore, tick locking
- Direct analysis: `Cargo.toml` — workspace dependencies (tokio, reqwest, serde_json all present; no new deps needed for hooks or delegation)
- Confidence: HIGH — all integration points are from direct code reading, not inference
