---
status: root_cause_found
trigger: "Phase 21.7 UAT 1: /agents list returns 'No active subagents.' while delegate_task subagents are actively running"
created: "2026-04-23T15:45:00Z"
updated: "2026-04-23T16:10:00Z"
phase: 21.7
gap_id: GAP-21.7-01
---

# Debug: /agents list empty while subagents live

## Symptoms

**Expected behavior:**
When `delegate_task` subagents are actively running (ticker shows `[subagent-1] Running: web_read`, `[subagent-2] Running: web_read`, `[subagent-3] Running: web_read`), typing `/agents list` in the REPL should return a list of active subagent IDs with task summaries and uptimes.

**Actual behavior:**
`/agents list` returns `"No active subagents."` while the subagent ticker is clearly streaming live tool-call events.

**Error messages:**
None. Two additional observations from the same session:
- `Error: /agents kill <id>: missing id` — user invoked /agents kill without an ID; expected usage error
- `Error: /agents logs <id>: missing id` — user invoked /agents logs without an ID; expected usage error

**Timeline:**
First observation — this is the first live UAT of the /agents handler since Plan 21.7-07 wired the SubagentRegistry in Wave 2 and Plan 21.7-08 filled in the slash handlers in Wave 3. No prior baseline.

**Reproduction:**
1. Launch `hermes` in chat mode (interactive TUI)
2. Prompt the AI to spawn multiple delegate_task subagents (e.g. "research LoRA training across 3 angles")
3. While the subagent ticker shows them running tool calls (`[subagent-N] Running: web_read`), type `/agents list`
4. Observe: "No active subagents."

## Current Focus

```yaml
hypothesis: "CONFIRMED: The REPL is strictly sequential. rl.readline() only unblocks after run_agent_turn() fully completes (main.rs:1192). run_agent_turn awaits all delegate_task subagents via handle.await (delegate_task.rs:327). By the time the user can type /agents list, every subagent has already unregistered. The registry is legitimately empty at query time."
test: "Code walk confirmed — no concurrent input path exists. The ticker output is eprintln! from SubagentProgressCallback firing during the in-progress .await, printing to stderr while rustyline blocks stdin. It appears concurrent but stdin is not read until the agent turn finishes."
expecting: "N/A — root cause confirmed by static analysis"
next_action: "Fix: wire concurrent slash-command input during agent turns"
evidence_needed: "N/A"
reasoning_checkpoint:
  arc_identity: "CLEARED — both subagent_runner.with_subagent_registry(subagent_registry.clone()) at main.rs:737 and SubagentRegistryHandle::new(subagent_registry.clone()) at main.rs:1032 clone the SAME Arc. Arc identity is correct."
  register_fires: "CLEARED — AgentSubagentRunner::run_child at subagent_runner.rs:176 does call reg.write().await.register(info) when self.subagent_registry is Some. It is Some because with_subagent_registry() was called at main.rs:737."
  block_in_place: "CLEARED — tokio::main defaults to multi-thread runtime, so block_in_place in SubagentRegistryHandle::list_summary is safe."
  actual_bug: "CONFIRMED: Structural — rl.readline() is sequential with run_agent_turn(). User cannot type /agents list while subagents run. The detach=true flag only skips cancel-token propagation (delegate_task.rs:210), it does NOT make execute_batch return early — handle.await still blocks on all tasks (delegate_task.rs:327)."
tdd_checkpoint: {}
```

## Code anchors (starting points — not conclusions)

- **Handler (reads registry):** `crates/ironhermes-core/src/commands/handlers.rs:144-213` — `cmd_agents(args, ctx)`. Calls `ctx.subagent_registry.as_ref().unwrap().list_summary()`. If `subagent_registry` were None, would return "Subagent registry not wired." — user saw "No active subagents.", so the trait object IS present.
- **Registry handle (bridges async to sync):** `crates/ironhermes-agent/src/subagent_registry.rs:81-124` — `SubagentRegistryHandle(Arc<RwLock<SubagentRegistry>>)` uses `tokio::task::block_in_place` + `Handle::current().block_on` to call `.read().await.list()`.
- **Registry writer (delegate_task spawn path):** `crates/ironhermes-agent/src/subagent_runner.rs:132-176` — `AgentSubagentRunner::run_child`. Comment says "Register in the SubagentRegistry, if attached. The D-03/D-04 pill refresh in main.rs reads `active_count()` after this registration lands." The `if let Some(ref reg) = self.subagent_registry` gate fires correctly — runner IS built with `.with_subagent_registry(...)` at main.rs:737.
- **Runner wiring in run_chat:** `crates/ironhermes-cli/src/main.rs:682-739` — creates `Arc<RwLock<SubagentRegistry>>` once, passes `.clone()` to `AgentSubagentRunner.with_subagent_registry()`, then separately wraps `.clone()` in `SubagentRegistryHandle` for `CommandContext.with_subagent_registry()` at line 1030-1032. Arc identity is correct.
- **Delegate tool spawn call:** `crates/ironhermes-tools/src/delegate_task.rs:280-291` — calls `runner.run_child(...)`. The runner passed to `register_delegate_task_tool(subagent_runner, ...)` at main.rs:807-814 IS the one wired with `.with_subagent_registry(...)`.
- **REPL sequential block:** `crates/ironhermes-cli/src/main.rs:1131-1196` — `agent_running.store(true)` at 1131, `tokio::select!` blocks until `run_fut` resolves at 1192, then `agent_running.store(false)` at 1196. No concurrent input path.
- **Batch awaits all handles:** `crates/ironhermes-tools/src/delegate_task.rs:326-332` — `for handle in handles { handle.await }`. Even with `detach=true`, results are awaited. `detach` only affects cancel-token propagation (line 210), NOT whether execute_batch waits for tasks.

## Evidence

- timestamp: 2026-04-23T16:10:00Z
  type: static_analysis
  finding: "Arc identity confirmed correct — same Arc<RwLock<SubagentRegistry>> cloned to runner (main.rs:737) and CommandContext (main.rs:1032)"

- timestamp: 2026-04-23T16:10:00Z
  type: static_analysis
  finding: "register() fires correctly — with_subagent_registry() called at main.rs:737 sets self.subagent_registry = Some(reg), so the if-let guard at subagent_runner.rs:162 is always true on the run_chat path"

- timestamp: 2026-04-23T16:10:00Z
  type: static_analysis
  finding: "REPL is sequential: rl.readline() at main.rs:994 only runs after run_fut resolves at main.rs:1192. User cannot type /agents list while subagents run. By the time input is accepted, all subagents have called unregister() and the registry is empty."

- timestamp: 2026-04-23T16:10:00Z
  type: static_analysis
  finding: "detach=true does NOT make execute_batch return early. handle.await at delegate_task.rs:327 blocks on all spawned tasks regardless of detach flag. detach only skips cancel-token wiring."

- timestamp: 2026-04-23T16:10:00Z
  type: static_analysis
  finding: "Ticker output is eprintln! from SubagentProgressCallback (main.rs:769-784) which fires during the awaited agent turn — these print to stderr while rustyline holds stdin. Creates appearance of concurrent activity but stdin is blocked until turn completes."

## Eliminated hypotheses

- **Wrong runner instance** — ELIMINATED. `register_delegate_task_tool` at main.rs:807 receives the same `subagent_runner` Arc that was built with `.with_subagent_registry(subagent_registry.clone())` at main.rs:731-738.
- **Different Arc instances** — ELIMINATED. Both the runner and the CommandContext's SubagentRegistryHandle wrap `.clone()` of the same `subagent_registry` Arc created at main.rs:682.
- **register() never fires** — ELIMINATED. `self.subagent_registry` is `Some` (set at main.rs:737), so the `if let Some(ref reg)` guard at subagent_runner.rs:162 always succeeds on the run_chat path.
- **block_in_place deadlock** — ELIMINATED. tokio::main uses multi-thread runtime by default; block_in_place is safe.
- **Timing window / race condition** — ELIMINATED. This is not a race. The registry is empty because by structural necessity, all subagents complete before the REPL accepts the /agents list command.

## Resolution

**Root cause:** The REPL loop in `run_chat` is strictly sequential. `rl.readline()` only unblocks after the full agent turn (`run_agent_turn`) completes (main.rs:1192). The agent turn itself awaits all `delegate_task` subagents (via `handle.await` in `execute_batch`, delegate_task.rs:327). Therefore, by the time the user can type `/agents list`, every subagent has already completed and called `unregister()`. The registry is legitimately empty at query time.

The visible ticker output (`[subagent-N] Running: web_read`) is from `eprintln!` calls in `SubagentProgressCallback` firing synchronously within the awaited agent-turn future — they print to stderr while rustyline blocks stdin, creating the visual impression of concurrent activity. But stdin is not actually read until the turn completes.

**Fix direction:** Two options:

**Option A (minimal, lower risk):** Make `execute_batch` with `detach=true` actually detach — spawn all tasks and return immediately with task IDs rather than awaiting results. Subagents would remain registered while running, and the REPL loop would return to `rl.readline()` while they're live. The `/agents list` command would then work as designed. Requires adding a session-scoped store for detached task handles and a way to retrieve results later.

**Option B (correct, higher effort):** Wire concurrent slash-command input during agent turns by running the readline loop in a `tokio::select!` alongside `run_fut`. This would allow `/agents list`, `/agents kill`, and other commands to be dispatched mid-turn without changing the `delegate_task` API. This is architecturally cleaner but requires careful handling of the shared `messages` state and `rl` editor (which is `!Send`).

**Recommended fix:** Option A is the faster gap-closure fix. The `detach` parameter already exists in the schema and the code; the only missing piece is actually returning early from `execute_batch` when `detach=true` and storing handle references for later retrieval.
