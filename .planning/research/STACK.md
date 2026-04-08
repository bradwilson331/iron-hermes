# Technology Stack — v1.1 Automation

**Project:** IronHermes v1.1 Automation
**Researched:** 2026-04-07
**Scope:** NEW additions only — existing stack (tokio, reqwest, serde, rusqlite, anyhow, tracing, chrono, uuid, cron, clap, rustyline) is validated and NOT re-researched here.

---

## Feature-by-Feature Stack Additions

### 1. Subagent Delegation (`delegate_task` tool)

**What it does:** Spawns a child agent process with an isolated context (restricted toolset, separate message history, bounded turns) and returns the result to the parent.

**Approach: In-process async task, NOT subprocess.**

The agent loop (`AgentLoop`) is already `Send + Sync` and backed by tokio. Spawning an OS subprocess would require serializing the full LLM client config, tool registry, and context — significant complexity for no gain. Instead, create a `ChildAgentRunner` that constructs a fresh `AgentLoop` with a filtered `ToolRegistry` (subset of tools passed by name) and runs it in a `tokio::spawn` task.

Concurrency cap (max 3) is implemented with `tokio::sync::Semaphore` — already in `tokio`, no new crate needed.

**New crates required:** None.

**Integration point:** New tool `delegate_task` in `ironhermes-tools`. It receives `{ task: string, tools: string[], max_turns: int }`, constructs an `AgentLoop` with the filtered registry, awaits completion, returns `AgentResult.final_response`.

**What NOT to do:** Do not use `std::process::Command` or `tokio::process::Command`. Subprocess spawning requires re-serializing config, rebuilding the LLM client, and IPC plumbing — all complexity with no isolation benefit since the single-operator constraint means trust boundaries don't apply between parent and child.

---

### 2. Code Execution (`execute_code` tool — Python sandbox via RPC)

**What it does:** Accepts a Python script string, executes it in a sandboxed Python process, returns stdout/stderr.

**Approach: `tokio::process::Command` (stdlib, already in tokio) + Python subprocess with restricted env.**

The constraint is "Rust-only single binary." PyO3 embeds a Python interpreter INTO the Rust binary — it requires the Python shared library at link time, breaks `cargo build --release` portability, and makes the binary non-single-binary. It is explicitly out of scope.

The correct approach: spawn `python3 -c <script>` (or write script to a tempfile and execute) using `tokio::process::Command`. Sandbox by:
- Passing a restricted environment (no `HOME`, no network env vars, explicit `PATH` to system python only)
- Setting `current_dir` to an isolated temp directory
- Applying a wall-clock timeout via `tokio::time::timeout`

**New crate required:** `tempfile = "3"` — for creating an isolated working directory per execution and writing the script to a temp file (avoids shell injection from passing scripts directly to `-c`).

`tempfile` is already a dev-dependency in many Rust projects and the cron tests use `tempfile::tempdir()` already, confirming it is compatible with the workspace. It just needs to be added to `[workspace.dependencies]` and as a non-dev dependency in `ironhermes-tools`.

**Confidence:** HIGH — tokio::process is stable stdlib, tempfile 3.x is the standard Rust temp file crate.

**What NOT to do:** Do not use PyO3. Do not attempt WASM/WASI sandboxing (requires `wasmtime` ~50MB dependency, breaks single-binary constraint). Do not pass scripts via `-c` flag directly (shell injection risk).

---

### 3. Event Hooks (gateway hooks + plugin hooks)

**What it does:** Lifecycle interception at two layers:
- Gateway hooks: before/after message send (logging, alerts, webhooks)
- Plugin hooks: before/after tool dispatch (guardrails, interception)

**Approach: Trait-based hook chain, in-process, synchronous trait with async execution.**

The existing `Tool` trait pattern in `ironhermes-tools` is the model. Define a `Hook` trait:

```rust
#[async_trait]
pub trait Hook: Send + Sync {
    async fn before_tool(&self, name: &str, args: &serde_json::Value) -> anyhow::Result<()>;
    async fn after_tool(&self, name: &str, result: &str) -> anyhow::Result<()>;
}
```

A `HookChain` (Vec of `Arc<dyn Hook>`) runs hooks sequentially. Hooks that want to abort return `Err`. `AgentLoop` gains an optional `hook_chain` field. Gateway hooks follow the same pattern wrapping send/receive paths.

For outbound webhook delivery (alerting an external URL): `reqwest` is already in the workspace. No new crate needed.

**New crates required:** None.

**Integration points:**
- `ironhermes-agent`: `AgentLoop` gains `with_hooks(chain: HookChain)` builder method; `execute_tool_call` calls `chain.before_tool` / `chain.after_tool`
- `ironhermes-gateway`: `TelegramAdapter` gains a gateway-specific hook chain for message lifecycle
- New module `ironhermes-core` or `ironhermes-agent`: `Hook` trait + `HookChain` type

**What NOT to do:** Do not add an external event bus crate (e.g., `eventbus`, `bus`). The hook volume here is single-process and low-frequency — a trait vec is sufficient and avoids unnecessary abstraction.

---

### 4. Batch Processing (parallel prompt execution, ShareGPT output)

**What it does:** Accepts N prompts, runs them in parallel (up to a concurrency limit), collects `AgentResult` per prompt, serializes output as ShareGPT-format JSONL.

**Approach: `tokio::task::JoinSet` for parallel execution + bounded semaphore + `serde_json` for ShareGPT serialization.**

`JoinSet` (stable since tokio 1.21, 2022) allows spawning N tasks and collecting results as they complete, with clean cancellation on drop. Pair with `tokio::sync::Semaphore` to cap concurrency. Both are already in `tokio` — no new crate.

ShareGPT format is simple JSON with `conversations: [{from: "human"|"gpt", value: string}]`. Map `ChatMessage` role to ShareGPT `from` field. Serialize with existing `serde_json`. Output to a JSONL file using `std::fs::File` + `BufWriter`.

**New crates required:** None.

**Integration point:** New crate `ironhermes-batch` OR new module in `ironhermes-cli`. Given the workspace already has 7 crates and batch processing is a standalone workflow, prefer adding it as a module in `ironhermes-cli` (a `batch` subcommand) rather than a new crate — avoids workspace overhead for what is essentially a CLI command.

**What NOT to do:** Do not use `rayon` — this is async I/O-bound work (LLM API calls), not CPU-bound. Rayon's thread pool is wrong for async tasks. Stick with tokio task spawning.

---

## Summary Table: New Dependencies

| Crate | Version | Feature Flags | Purpose | Adds To |
|-------|---------|---------------|---------|---------|
| `tempfile` | `"3"` | (none needed) | Isolated temp dir + script files for Python sandbox | `[workspace.dependencies]`, `ironhermes-tools` |

**Everything else uses crates already in the workspace.**

---

## Crates Explicitly Ruled Out

| Crate | Reason |
|-------|--------|
| `pyo3` | Embeds Python interpreter — breaks single-binary constraint, requires Python shared lib at link time |
| `rayon` | CPU-thread parallelism — wrong for async I/O-bound LLM calls |
| `tokio-process` | Not a separate crate — it's `tokio::process`, already in `tokio = { features = ["full"] }` |
| `wasmtime` | WASM sandbox for code execution — 50MB+ dependency, extreme complexity, single-binary hostile |
| `eventbus` / `bus` | External event bus — overkill for in-process hook chain |
| `nix` | Low-level Unix process control — unnecessary when `tokio::process::Command` suffices |
| `subprocess` | Sync process crate — redundant with async `tokio::process` |

---

## Integration Map

```
ironhermes-tools/
  delegate_task.rs   — ChildAgentRunner (tokio::sync::Semaphore, AgentLoop)
  execute_code.rs    — Python subprocess (tokio::process, tempfile)
  batch.rs           — OR ironhermes-cli/src/batch.rs

ironhermes-agent/
  agent_loop.rs      — gains with_hooks(HookChain) builder
  hooks.rs           — Hook trait + HookChain (new file)

ironhermes-gateway/
  hooks.rs           — GatewayHook trait (mirrors Hook)

ironhermes-core/
  (no changes needed — Hook trait lives in agent or a new hooks module)
```

---

## Existing Crates That Cover Automation Needs

| Need | Already Present |
|------|----------------|
| Async task spawning | `tokio` (full features) — `tokio::spawn`, `JoinSet`, `Semaphore` |
| Process spawning | `tokio::process::Command` (in `tokio = { features = ["full"] }`) |
| Timeouts | `tokio::time::timeout` |
| JSON serialization (ShareGPT) | `serde_json = "1"` |
| HTTP webhook delivery | `reqwest = "0.12"` |
| Concurrency primitives | `tokio::sync::{Semaphore, Mutex, RwLock}` |
| Trait-based dispatch | `async-trait = "0.1"` |
| Unique IDs for tasks | `uuid = "1"` |

---

## Installation

```toml
# Add to [workspace.dependencies] in root Cargo.toml
tempfile = "3"

# Add to crates/ironhermes-tools/Cargo.toml [dependencies]
tempfile.workspace = true
```

No other dependency changes needed for the v1.1 automation milestone.

---

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Subagent (in-process) | HIGH | Pattern directly follows existing AgentLoop + ToolRegistry code |
| Code execution (subprocess) | HIGH | tokio::process is stable; tempfile 3.x is the standard crate |
| Event hooks (trait chain) | HIGH | Directly mirrors existing Tool trait pattern |
| Batch (JoinSet + ShareGPT) | HIGH | JoinSet stable since tokio 1.21; ShareGPT format is a well-known JSON schema |
| tempfile version | MEDIUM | Version 3.x is current as of August 2025 training cutoff; verify against crates.io before adding |

---

## Sources

- IronHermes codebase: `crates/ironhermes-agent/src/agent_loop.rs`, `crates/ironhermes-tools/src/registry.rs`, `crates/ironhermes-cron/src/lib.rs`
- tokio documentation: https://docs.rs/tokio/latest/tokio/process/index.html
- tempfile crate: https://docs.rs/tempfile/latest/tempfile/
- ShareGPT format reference: https://huggingface.co/datasets/anon8231489123/ShareGPT_Vicuna_unfiltered
