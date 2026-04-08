# Domain Pitfalls

**Domain:** Rust AI agent — adding subagent delegation, code execution (Python RPC), event hooks, and batch processing to existing async system
**Researched:** 2026-04-07
**Confidence:** HIGH (grounded in IronHermes codebase analysis + established patterns in tokio/async Rust)

---

## Critical Pitfalls

Mistakes that cause rewrites, security breaches, or cascading system failures.

---

### Pitfall 1: Subagent Infinite Recursion (Delegation Cycle)

**What goes wrong:** A subagent spawned by `delegate_task` is given the full parent toolset including `delegate_task` itself. It spawns a child, which spawns a child, creating an unbounded tree of processes and LLM API calls until the host OOMs or the API rate limiter cuts the connection.

**Why it happens:** The parent agent's `ToolRegistry` is cloned or shared with the child without stripping orchestration tools. The child has no awareness it is a child.

**Consequences:** Runaway LLM API costs (each tree level doubles calls), process table exhaustion, OOM kill of the main process, lost in-flight Telegram responses.

**Prevention:**
- The `delegate_task` tool MUST pass a restricted `ToolRegistry` to child `AgentLoop` instances — one that explicitly excludes `delegate_task` and any other orchestration-level tools.
- Enforce a hard depth limit (max depth = 1 for v1.1; no grandchildren). Pass a `depth: u32` through the child context and reject `delegate_task` calls at depth >= 1.
- Cap concurrent subagents system-wide with a `Semaphore` (start at 3 as specified). Acquisition must happen before spawning.

**Detection:** Child processes that themselves make LLM calls with tool lists containing `delegate_task`. Log the restricted tool list at child spawn time.

**Phase:** Subagent Delegation phase. This is the first design decision to get right before any code is written.

---

### Pitfall 2: Python Sandbox Escape via RPC Tool Forwarding

**What goes wrong:** The Python code execution sandbox calls Hermes tools via RPC. Those tools include `terminal`, `write_file`, and `patch` — tools that can modify the filesystem, execute arbitrary shell commands, and reach the network. A malicious or LLM-hallucinated Python script calls `rpc.terminal("rm -rf /")` and the RPC server forwards it without checking whether the caller is allowed to invoke that tool in that context.

**Why it happens:** RPC tool dispatch re-uses the parent `ToolRegistry` which was built for a trusted agent context. The Python sandbox is not a trusted agent context.

**Consequences:** Arbitrary command execution on the host, filesystem destruction, exfiltration of credentials in `~/.ironhermes/`, bypassing of all SSRF protections (since `terminal` can run `curl` directly).

**Prevention:**
- The Python RPC server MUST expose a separate, minimal tool registry — `read_file` (read-only paths only), `web_search`, `web_read`. Explicitly block `terminal`, `write_file`, `patch`, `delegate_task`, all memory tools.
- Validate every RPC tool call against an allowlist before dispatch, not just at registry construction. Defense in depth.
- Run the Python interpreter with restricted OS permissions: no network namespace access (or SSRF-validated egress only), read-only bind mounts except a temp scratch dir, `rlimit` on CPU and memory.
- The existing SSRF guard in `ironhermes-core/src/ssrf.rs` must be applied to all URLs originating from RPC calls, not just from web_read/web_search tools.

**Detection:** Any RPC call to `terminal`, `write_file`, `patch` should be logged as a security event and rejected with an explicit error, not silently dropped.

**Phase:** Code Execution phase. The RPC tool allowlist must be defined before the RPC server accepts any connections.

---

### Pitfall 3: Subagent Process Leaks on Parent Cancellation

**What goes wrong:** A subagent is spawned as a child `tokio::process::Child` or a separate `tokio::task`. The parent task is cancelled (Telegram timeout, user sends /reset, graceful shutdown). The child process keeps running, consuming CPU, making LLM API calls, and holding open file handles — indefinitely, because it has no cancellation signal.

**Why it happens:** `tokio::spawn` returns a `JoinHandle` that, when dropped, detaches the task (does not abort it). `tokio::process::Child`, when dropped, does not kill the child process. Both are "fire and forget" by default.

**Consequences:** Ghost subagent processes accumulate across sessions. On shutdown, the main process exits but subagents keep calling the LLM API. Costs accrue invisibly.

**Prevention:**
- Track all subagent `JoinHandle`s in a `JoinSet` owned by the parent task. When the parent's `CancellationToken` fires, call `join_set.abort_all()` before returning.
- For OS-level child processes (Python interpreter), use `tokio::process::Command::kill_on_drop(true)` so the `Child` handle automatically sends SIGKILL on drop.
- The existing gateway uses `JoinSet` + `CancellationToken` (from Phase 1 research). The subagent spawner must follow the same pattern, not invent its own.
- Register all subagent semaphore permits with RAII guards so the Semaphore slot is released even if the task panics.

**Detection:** Monitor active subagent count via an `AtomicU32` counter incremented on spawn, decremented in a `Drop` impl. Alert if count exceeds the configured maximum after a session ends.

**Phase:** Subagent Delegation phase.

---

### Pitfall 4: Hook Ordering Deadlock (Recursive Hook Invocation)

**What goes wrong:** A `pre_tool` hook is registered that itself calls a tool (e.g., a logging hook that writes to a file via the `write_file` tool). The `write_file` tool fires its own `pre_tool` hook. If the hook system holds a lock while dispatching hooks, the second hook invocation deadlocks on the same lock.

**Why it happens:** Hook registries are natural candidates for `Arc<Mutex<Vec<Hook>>>`. Calling hooks while holding that mutex, then having a hook re-enter the tool dispatch path, creates a lock cycle. Even without explicit locks, async re-entrancy via `tokio::sync::Mutex` produces the same result if the hook awaits anything that re-acquires the same mutex.

**Consequences:** Silent deadlock. The agent loop hangs indefinitely. No error is returned. The Telegram user sees a bot that never responds. The parent session's timeout eventually kills the run, but the deadlock cause is invisible in logs.

**Prevention:**
- Snapshot the hook list before calling hooks: `let hooks = self.hooks.read().clone()` (using `RwLock`, not `Mutex`). Release the lock before invoking any hook. Never hold the registry lock across an async await point.
- Define a strict hook execution context: hooks MUST NOT call tools directly. Hooks receive read-only event data and return an `Action` enum (`Allow`, `Block(reason)`, `Modify(new_args)`). Side effects (logging, webhooks) must be dispatched to a background channel, not executed inline.
- Mark the hook trait with a `#[must_not_recurse]` convention (not a Rust primitive, but a code review rule). Document it prominently.
- Reentrancy guard: track the current tool being dispatched in a `thread_local!` or task-local. If a hook attempts to invoke the same tool, return an error immediately.

**Detection:** Add a `hook_depth: AtomicU32` counter. If depth exceeds 2, log an error and short-circuit rather than deadlocking.

**Phase:** Event Hooks phase.

---

### Pitfall 5: Batch Processing Memory Pressure from Concurrent Message History

**What goes wrong:** Batch processing runs N prompts in parallel. Each prompt instantiates an `AgentLoop` with its own `Vec<ChatMessage>` conversation history. A conversation history can grow to tens of thousands of tokens (the existing `ContextCompressor` targets a configurable limit). With N=20 parallel batch items, peak memory usage is `N * max_context_size * bytes_per_token`. At 128K token contexts and 4 bytes/token, 20 parallel runs consume ~10GB of heap.

**Why it happens:** The batch runner treats each item as independent and maxes out the concurrency limit. Context compression runs per-session but does not account for system-wide memory pressure. The `ContextCompressor` in `ironhermes-agent` compresses within a session but does not signal back-pressure to the batch scheduler.

**Consequences:** OOM kill of the main process, killing all in-flight batch items and producing no output for any of them. On a 16GB server, this happens well before N=20.

**Prevention:**
- Batch concurrency must have a second limit: not just "max N parallel" but "max M tokens in flight across all parallel runs." Implement a `TokenBudgetSemaphore` that acquires `estimated_tokens` permits before starting each batch item.
- Default batch concurrency to 4 (conservative) not the full N. Make it configurable. Document the memory math in the config comments.
- Batch items should use aggressive context compression settings: smaller `max_tokens` limit than interactive sessions, tool result truncation at a lower threshold.
- Stream batch results to disk (ShareGPT JSONL, one object per line) as each item completes rather than accumulating all results in memory before writing.
- Use a `tokio::sync::Semaphore` for the batch concurrency cap, not `FuturesUnordered` with all items submitted at once.

**Detection:** Log `rss_bytes` (from `/proc/self/status` on Linux or `task_info` on macOS) at each batch item start. If RSS exceeds 80% of available memory, stop accepting new batch items and drain existing ones.

**Phase:** Batch Processing phase.

---

## Moderate Pitfalls

---

### Pitfall 6: Python RPC Channel Deadlock (Blocking stdin/stdout)

**What goes wrong:** The Python RPC protocol uses stdin/stdout for communication (common pattern for subprocess IPC). The Rust parent writes a request to the child's stdin, then blocks waiting for a response on stdout. Meanwhile, the Python process writes a large result to stdout, fills the OS pipe buffer (typically 64KB on Linux), and blocks waiting for the parent to drain it. Both sides are blocked — classic pipe deadlock.

**Why it happens:** Naive `child.stdin.write_all()` followed by `child.stdout.read_to_end()` in sequence, without concurrent reading.

**Prevention:**
- Use `tokio::io::AsyncWriteExt` for stdin and `tokio::io::AsyncReadExt` for stdout, with both running concurrently via `tokio::join!`. Never read and write the same child's pipes sequentially.
- Alternatively, use Unix domain sockets or a named pipe instead of stdin/stdout for RPC — avoids the pipe buffer limit entirely and is easier to frame messages on.
- Set explicit size limits on Python tool output before it reaches the RPC response (the Python side truncates, the Rust side has a fallback max read size).
- Add a `timeout` on the RPC call (the existing `terminal.rs` uses `tokio::time::timeout` — use the same pattern).

**Phase:** Code Execution phase.

---

### Pitfall 7: Hook Side Effects Breaking Tool Result Consistency

**What goes wrong:** A `post_tool` hook modifies tool output (e.g., a guardrails hook redacts PII from `read_file` output). The modified output is appended to the conversation history. The LLM sees redacted content and hallucinates what the missing sections contained. Worse, a subsequent `read_file` call returns the real content (hooks only fire in the agent loop, not in raw file access), creating inconsistency the LLM cannot reconcile.

**Why it happens:** Hooks that modify tool results are treating symptoms rather than root causes. The LLM's context now contains a lie about what the file contains.

**Prevention:**
- `post_tool` hooks should return `Observe(modified_output)` only for logging/telemetry — the modified output goes to the hook caller, not to the conversation history.
- For guardrail use cases, prefer `pre_tool` hooks that block the call entirely (`Block(reason)`) rather than `post_tool` hooks that quietly alter results.
- If output modification is truly required, document it explicitly in the tool result: prepend `[GUARDRAIL: sections redacted]` so the LLM knows the result is partial.

**Phase:** Event Hooks phase.

---

### Pitfall 8: Subagent Context Inheritance Leaking Sensitive State

**What goes wrong:** The parent agent's conversation history (which may contain API keys, user PII, internal tool results) is passed verbatim to the child agent as its starting context. The child's task requires none of this information. If the child is a future `delegate_task` call used in batch processing, that history ends up in ShareGPT training data.

**Why it happens:** It is convenient to give the child "full context" to explain why it is doing what it is doing. The implementation copies or references the parent's `Vec<ChatMessage>`.

**Prevention:**
- Child agents receive ONLY the task description and the minimal facts needed to complete it — not the parent's conversation history.
- Construct the child's initial messages from scratch: a system prompt (derived from the parent's SOUL.md config, not the parent's accumulated messages) plus a single user message describing the task.
- Before any messages enter ShareGPT batch output, run a secrets scan (the existing `context_scanner.rs` patterns) to detect API keys, tokens, and credentials.

**Phase:** Subagent Delegation phase; also relevant to Batch Processing phase.

---

### Pitfall 9: Cron Scheduler Interaction with Subagent Concurrency Limit

**What goes wrong:** The existing cron scheduler fires tasks at scheduled intervals. If a cron task uses `delegate_task`, it competes for the same subagent `Semaphore` slots as interactive user sessions. A cron burst (e.g., 5 tasks firing simultaneously at the top of the hour) exhausts all 3 subagent slots, blocking interactive users from getting responses for the duration of those tasks.

**Why it happens:** The cron scheduler and the gateway handler share the same subagent concurrency pool without priority differentiation.

**Prevention:**
- Maintain separate semaphore pools for interactive (user-initiated) and background (cron-initiated) subagents. Interactive pool: 3 slots. Background pool: 1 slot (expandable via config).
- Cron tasks that spawn subagents should use a lower-priority acquisition path: check if interactive slots are available before consuming background slots, and add jitter to cron firing times to prevent burst clustering.
- Add a `task_source: TaskSource` enum (`Interactive | Cron | Batch`) to the subagent context so resource accounting can distinguish usage.

**Phase:** Subagent Delegation phase; interacts with existing Cron crate.

---

### Pitfall 10: Batch Processing Writing to Same Output File Concurrently

**What goes wrong:** Multiple batch items complete around the same time and all attempt to append to the same JSONL output file. Without coordination, partial writes interleave and produce malformed JSON lines. On macOS, `O_APPEND` provides atomic appends only up to `PIPE_BUF` bytes (~512 bytes); larger writes are not atomic.

**Why it happens:** Naive `OpenOptions::new().append(true).open(path)` from multiple `tokio::task`s without a serialization primitive.

**Prevention:**
- Serialize all output writes through a single `tokio::sync::mpsc` channel consumed by a dedicated writer task. Batch items send completed `TrajectoryRecord` values to the channel; the writer task appends them one at a time.
- This also enables clean shutdown: drain the channel before closing the file, ensuring no records are lost when the batch completes.

**Phase:** Batch Processing phase.

---

## Minor Pitfalls

---

### Pitfall 11: Subagent Tool Result Size Amplification

**What goes wrong:** A subagent calls `web_read` on a large page, gets 50K characters back, includes it in its response to the parent. The parent appends this as a tool result in its own conversation history. Token count spikes suddenly. Context compressor runs, drops earlier conversation turns, loses important context. The parent's next LLM call is expensive and low-quality.

**Prevention:** Cap subagent response length (max 4K characters by default). Subagents should summarize their findings, not return raw tool output verbatim. Enforce this in the `delegate_task` tool's response extraction logic.

**Phase:** Subagent Delegation phase.

---

### Pitfall 12: Hook Registration Order Producing Non-Deterministic Behavior

**What goes wrong:** Two hooks both handle `pre_tool` for `write_file`. Hook A validates the path is within allowed directories. Hook B logs the write attempt. If Hook B runs before Hook A and A blocks the call, B has already logged a write that never happened. In the other order, logging correctly reflects only allowed writes.

**Prevention:** Define an explicit priority field on hooks (`priority: i32`, lower runs first). Security/validation hooks always run before observability/logging hooks. Document the convention. Use a `BTreeMap<i32, Vec<Hook>>` for ordered dispatch.

**Phase:** Event Hooks phase.

---

### Pitfall 13: Python Interpreter Startup Latency on Every `execute_code` Call

**What goes wrong:** Each `execute_code` call spawns a fresh Python interpreter, which takes 200-400ms for startup before the script executes. For short scripts called in a tight agent loop, this overhead dominates. The user perceives the agent as slow.

**Prevention:** Keep a warm Python interpreter process alive as a daemon with a simple request/response loop (the RPC server itself serves this purpose if it stays running between calls rather than exiting). Use a pool of 1-2 warm interpreters with a short idle timeout (30 seconds) before shutdown. This is more complex than spawn-per-call but necessary for acceptable latency.

**Phase:** Code Execution phase.

---

### Pitfall 14: Event Hook Panics Crashing the Agent Loop

**What goes wrong:** A user-registered hook (or a buggy built-in hook) panics during dispatch. Because hooks are called inside the agent loop task, the panic propagates, kills the task, and the Telegram user gets no response. Worse, a `JoinHandle` propagating a panic can take down a larger scope if not caught.

**Prevention:** Wrap each hook call in `std::panic::catch_unwind` (for sync hooks) or a `tokio::task::spawn` with panic catching (for async hooks). Log the panic, mark the hook as errored, and continue dispatching remaining hooks. A single hook failure should never abort the agent run. Consider automatically disabling hooks that panic more than 3 times.

**Phase:** Event Hooks phase.

---

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|-------------|---------------|------------|
| Subagent Delegation | Infinite recursion via `delegate_task` in child toolset | Strip orchestration tools from child registry; enforce depth=1 |
| Subagent Delegation | Process leak when parent cancelled | `kill_on_drop(true)` + `JoinSet::abort_all()` on parent cancel |
| Subagent Delegation | Cron competing for interactive subagent slots | Separate semaphore pools per task source |
| Subagent Delegation | Parent context leaking to child (PII, keys) | Build child context from scratch; never copy parent message history |
| Code Execution (Python RPC) | Sandbox escape via RPC tool forwarding to `terminal` | Strict RPC allowlist; never expose `terminal`, `write_file`, `patch` |
| Code Execution (Python RPC) | Pipe deadlock on large outputs | Concurrent stdin write + stdout read via `tokio::join!` |
| Code Execution (Python RPC) | Startup latency per call | Warm interpreter daemon with idle timeout |
| Event Hooks | Deadlock from hook re-entering tool dispatch | Snapshot hook list before calling; never hold registry lock across await |
| Event Hooks | Hook output modification breaking LLM consistency | Prefer `Block` over silent `Modify`; label any modifications in tool result |
| Event Hooks | Hook panics killing agent loop | Wrap in `catch_unwind`; isolate per-hook failures |
| Event Hooks | Non-deterministic hook ordering | Priority field on hooks; security before observability |
| Batch Processing | OOM from N parallel contexts | `TokenBudgetSemaphore`; default concurrency=4; stream results to disk |
| Batch Processing | Concurrent JSONL output corruption | Single writer task consuming mpsc channel |
| Batch Processing | Sensitive data in ShareGPT output | Pre-export secrets scan using `context_scanner.rs` patterns |
| Integration (all) | New features bypassing existing SSRF guard | Route all URL-originating calls through `ironhermes-core::ssrf` regardless of source |

## Sources

- IronHermes codebase direct analysis: `agent_loop.rs`, `terminal.rs`, `ssrf.rs`, `context_scanner.rs`, `runner.rs` — HIGH confidence
- Previous IronHermes research (SUMMARY.md, async-patterns.md) — HIGH confidence (grounded in codebase)
- Tokio documentation on `JoinSet`, `CancellationToken`, `Semaphore`, pipe deadlock patterns — HIGH confidence (stable APIs)
- OS pipe buffer behavior (Linux 64KB `PIPE_BUF`) — HIGH confidence (POSIX spec)
- Python interpreter startup latency benchmarks — MEDIUM confidence (varies by system, Python version, import graph)
