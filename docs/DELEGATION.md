# Subagent Delegation

The `delegate_task` tool spawns child `AgentLoop` instances with isolated context, a restricted toolset, and their own temp directory. Each child gets a fresh conversation and works independently — only its final structured summary enters the parent's context.

---

## Single Task

```
delegate_task(
    task="Debug why tests fail in crates/ironhermes-cron",
    toolsets=["terminal", "file"]
)
```

The `task` field (not `goal`) is the only required parameter. The child receives a system prompt built from `task` + optional `context`, then a single user message: `"Complete the task described in the system prompt."` Everything else starts blank.

---

## Parallel Batch

Up to 3 concurrent subagents by default (configurable via `delegation.max_concurrent_children`):

```
delegate_task(tasks=[
    {"goal": "Research Tokio async patterns",   "toolsets": ["web"]},
    {"goal": "Research tower middleware design", "toolsets": ["web"]},
    {"goal": "Fix the failing clippy lint",      "toolsets": ["terminal", "file"]}
])
```

Batch mode uses `tasks` (array of objects with `goal`, optional `context`, optional `toolsets`). Single mode uses `task` (string). The two are mutually exclusive.

Results are sorted by original task index regardless of completion order. Batches larger than `max_concurrent_children` return a tool error rather than being silently truncated.

---

## How Subagent Context Works

**Subagents know nothing.** They start with a completely fresh conversation. Zero knowledge of the parent's history, prior tool calls, or anything discussed before delegation. The only context comes from the `task` and `context` fields the parent populates.

```
# BAD — subagent has no idea what "the error" is
delegate_task(task="Fix the error")

# GOOD — subagent has all context it needs
delegate_task(
    task="Fix the TypeError in crates/ironhermes-cron/src/parser.rs line 47",
    context="""The file has a TypeError: 'None' returned from parse_duration()
    when the input string starts with 'every '. The function is expected to
    return Ok(ScheduleParsed::Interval { ... }). The workspace is at
    ~/code/ironhermes and builds with 'cargo build'. Run 'cargo test -p
    ironhermes-cron' after fixing to verify.""",
    toolsets=["terminal", "file"]
)
```

The subagent's system prompt is assembled as:

```
You are a focused assistant. Complete the following task:

{task}

{context}

When you complete the task, provide a structured summary with these sections:
- **Actions Taken**: What you did step by step
- **Files Modified**: Any files created or changed (with paths)
- **Findings**: Key results or information discovered
- **Issues Encountered**: Any problems or blockers (or 'None')
```

---

## Toolsets

The `toolsets` parameter controls which tools the child has access to. It takes precedence over `allowed_tools`.

| Toolset | Expands To |
|---------|-----------|
| `"terminal"` | `terminal` (isolated temp CWD — AGENT-04) |
| `"file"` | `read_file`, `write_file`, `patch`, `search_files` |
| `"web"` | `web_search`, `web_read` |

**Default (no toolsets or allowed_tools specified):**
`read_file`, `write_file`, `patch`, `search_files`, `web_search`, `web_read`, `memory`

**Toolset selection by task type:**

| Pattern | Toolsets |
|---------|---------|
| Code work, debugging, builds | `["terminal", "file"]` |
| Research, doc lookup | `["web"]` |
| Full-stack tasks | `["terminal", "file", "web"]` (config default) |
| Read-only analysis | `["file"]` |
| System administration | `["terminal"]` |

### Blocked Tools

These are silently stripped from every child registry regardless of what you specify:

| Tool | Reason |
|------|--------|
| `delegate_task` | Excluded for `leaf` role children; re-added for `orchestrator` children within depth limit |
| `skills` | Not available to subagents |
| `execute_code` | Not available to subagents |
| `cronjob` | Not available to subagents |
| `clarify` | No `clarify_callback` channel in children (Python: `_NEVER_PARALLEL_TOOLS`) |
| `send_message` | No platform session in children |

`memory` is available but **read-only** in child context — children can read shared memory but cannot write to it (D-12).

Unknown tool names in `allowed_tools` cause an immediate error (fail-early, D-04).

---

## Per-Call Schema Fields

### `role` — leaf or orchestrator

Controls whether the child can itself delegate further:

| Value | Behaviour |
|-------|-----------|
| `"leaf"` (default) | Child cannot delegate. `delegate_task` is excluded from its registry. |
| `"orchestrator"` | Child retains `delegate_task` in its registry up to `max_spawn_depth`. |

When `role="orchestrator"` is requested but the configured `max_spawn_depth` would be exceeded (or `orchestrator_enabled=false`), the child is **silently downgraded to `leaf`** and a `tracing::warn!` is emitted. No error is returned to the caller.

```
delegate_task(
    task="Orchestrate the multi-step migration",
    role="orchestrator",
    toolsets=["terminal", "file"]
)
```

### `max_iterations` — per-call override

Override the number of LLM iterations allowed for this specific call. Falls back to `delegation.max_iterations` from config (default 50).

```
delegate_task(
    task="Quick file existence check",
    max_iterations=5,
    toolsets=["terminal"]
)
```

In batch mode, `max_iterations` can be specified per-task inside the `tasks` array:

```
delegate_task(tasks=[
    {"goal": "Short task",  "max_iterations": 5,  "toolsets": ["file"]},
    {"goal": "Longer task", "max_iterations": 30, "toolsets": ["terminal", "file"]}
])
```

### `stale_warn_seconds` — per-call soft-warn threshold

Seconds of inactivity before this subagent is flagged `[stale]` in `/agents` output. Falls back to `delegation.stale_warn_seconds` from config (default 120s). Soft signal only — operator decides whether to `/agents kill` (Phase 32.3).

Activity is bumped on each LLM API call, each streamed content delta, and each tool call (before + after). A subagent humming along on a long-running tool call is NOT stale; one waiting on a wedged LLM stream for >120s IS stale.

```
delegate_task(
    task="Research LoRA training pipeline — read repos, summarize findings",
    stale_warn_seconds=1800,
    toolsets=["web"]
)
```

In batch mode, `stale_warn_seconds` is per-task:

```
delegate_task(tasks=[
    {"goal": "Quick lookup",            "stale_warn_seconds": 30,   "toolsets": ["file"]},
    {"goal": "Long-running research",   "stale_warn_seconds": 1800, "toolsets": ["web"]}
])
```

The hard-kill ceiling remains `child_timeout_seconds` (default 300s). `stale_warn_seconds` is a soft signal; the timeout is the actual safety net.

---

## Practical Examples

### Parallel Research

```
delegate_task(tasks=[
    {
        "goal": "Research Rust async cancellation patterns with tokio-util CancellationToken",
        "context": "Focus on: propagating cancellation through nested async tasks, select! vs token.cancelled(), cleanup on drop.",
        "toolsets": ["web"]
    },
    {
        "goal": "Research serde_json zero-copy deserialization techniques",
        "context": "Focus on: borrowed lifetimes, RawValue, avoiding allocations for large payloads.",
        "toolsets": ["web"]
    },
    {
        "goal": "Research tower Service trait middleware composition patterns",
        "context": "Focus on: BoxCloneService, ServiceBuilder, how to wrap an existing service without cloning.",
        "toolsets": ["web"]
    }
])
```

### Code Review + Fix

```
delegate_task(
    task="Review crates/ironhermes-cron/src/scanner.rs for security issues and fix any found",
    context="""Workspace at ~/code/ironhermes. The scanner validates cron job
    prompts before scheduling. Focus on: prompt injection vectors, path traversal
    in script names, unbounded input lengths, regex DoS patterns.
    Fix any issues found and run 'cargo test -p ironhermes-cron' after.""",
    toolsets=["terminal", "file"]
)
```

### Multi-File Refactoring

```
delegate_task(
    task="Refactor all unwrap() calls in crates/ironhermes-cron/src/ to proper error propagation",
    context="""Workspace at ~/code/ironhermes. Replace .unwrap() with ? where
    the function already returns Result, or .expect("reason") with a descriptive
    message where unwrap is genuinely unreachable. Do NOT change test files.
    Run 'cargo clippy -p ironhermes-cron' after to confirm no new warnings.""",
    toolsets=["terminal", "file"]
)
```

### Write Structured Output to Files

```
delegate_task(
    task="Audit the ironhermes-tools crate and write a findings report",
    context="""Workspace at ~/code/ironhermes/crates/ironhermes-tools.
    1. Run: cargo clippy -p ironhermes-tools 2>&1
    2. Run: find src -name '*.rs' | xargs grep -n 'unwrap()' | head -40
    3. Count public functions without /// doc comments
    4. Write a findings report to ~/code/ironhermes/docs/tools-audit.md with:
       - Total clippy warnings
       - Top 5 unwrap() locations with file:line
       - Count of undocumented public functions
       - Three highest-priority cleanup recommendations""",
    toolsets=["terminal", "file"]
)
```

---

## Detach Mode

By default, interrupting the parent (user sends a new message, `/stop`) cancels all active children immediately.

Set `detach=true` to give the child its own independent cancel token — it continues running even if the parent turn is interrupted:

```
delegate_task(
    task="Run the full test suite and write results to /tmp/test-results.txt",
    context="cd ~/code/ironhermes && cargo test 2>&1 | tee /tmp/test-results.txt",
    toolsets=["terminal"],
    detach=true
)
```

Use `detach=true` for long-running tasks (builds, full test runs) where you want the work to complete regardless of user interaction. Use the default `detach=false` for tasks where a parent interrupt should cleanly stop everything.

---

## Per-Call Timeout

Override the global `child_timeout_seconds` for a single call:

```
delegate_task(
    task="Quick file existence check",
    context="Check if ~/code/ironhermes/docs/DELEGATION.md exists and print first 5 lines.",
    toolsets=["terminal"],
    timeout_seconds=30
)
```

The timer resets on every tool call or API call — only genuinely idle children trigger a kill.

---

## Subagent Identity and Transcripts

Each subagent gets a unique ID in the format `sub_<12 hex chars>` (e.g. `sub_3f9a12c8d041`). Transcripts are written to:

```
~/.ironhermes/subagent-transcripts/<session_id>/<subagent_id>.jsonl
```

Each `.jsonl` file contains one line per event: tool calls during execution, plus a terminal `Done` or `Cancelled` marker. The file is created before the subagent is registered in the live registry, so `/agents logs <id>` can read it immediately.

The `task_summary` shown in `/agents list` is the first 80 characters of the goal, clipped at a UTF-8 character boundary.

---

## Monitoring Active Subagents

The `/agents` command (alias `/tasks`) shows the live subagent registry:

```
/agents list          # show all running subagents with id, summary, uptime
/agents logs <id>     # stream transcript for a specific subagent
/agents kill <id>     # cancel a specific subagent without interrupting siblings
```

The registry drops the entry the moment the subagent writes its terminal transcript line — the pill in the status bar reflects this immediately.

### Tree view: /agents

When subagents are spawned with `role="orchestrator"`, `/agents list` renders an indented ASCII tree showing the parent–child relationship:

```
Active subagents:
├── sub_3f9a12  orchestrate the migration pipeline  42s  [running]
│   ├── sub_a1b2c3  fetch and parse data            30s  [running]
│   └── sub_d4e5f6  write output files              25s  [killed]
└── sub_7g8h9i  run full test suite                 18s  [running]
```

- **id prefix**: first 8 characters of the subagent UUID
- **summary clip**: task summary truncated to 80 characters with `…` if longer
- **uptime**: wall-clock seconds since the subagent was registered
- **status pill**: `[running]` or `[killed]` (cancelled via token)

When no nesting exists (all agents are root-level), the legacy flat-list format is used:

```
Active subagents:
- subagent-1 (sub_3f9a12c8d041) (orchestrate the migration pipeline) — 42s
- subagent-2 (sub_a1b2c3d4e5f6) (fetch and parse data) — 30s
```

The JSON shape produced by `AppState::subagent_tree_json` for the same 3-node tree:

```json
[
  {
    "id": "sub_root0000abcd",
    "task_summary": "orchestrate the migration pipeline",
    "uptime_secs": 42,
    "started_at_unix_ms": null,
    "children": [
      {
        "id": "sub_child1111ef",
        "task_summary": "fetch and parse data",
        "uptime_secs": 30,
        "started_at_unix_ms": null,
        "children": []
      },
      {
        "id": "sub_child2222gh",
        "task_summary": "write output files",
        "uptime_secs": 25,
        "started_at_unix_ms": null,
        "children": []
      }
    ]
  }
]
```

`started_at_unix_ms` is `null` because `std::time::Instant` carries no wall-clock epoch. A future plan will thread the session-start `SystemTime` to expose absolute timestamps.

---

## Stale-Child Detection

Phase 32.3 introduced a soft-warning system that flags subagents which have gone idle long enough to look stuck without yet hitting their hard-kill ceiling. The mechanism is purely observational — no automatic action is taken at the warn threshold.

### How it works

Every subagent's `AgentLoop` carries an `ActivityTracker` (landed in Phase 32.1) that bumps a shared `last_activity` clock on four observable events:

1. Before each LLM API call.
2. On each streamed content delta (token-by-token).
3. Before each tool dispatch.
4. After each tool dispatch returns.

The registry derives status at read time by comparing `now - last_activity` against the subagent's effective `stale_warn_seconds`:

| Condition | Status pill |
|-----------|-------------|
| `cancel_token.is_cancelled()` (operator killed or parent-cancel propagated) | `[killed]` |
| `now - last_activity > stale_warn_seconds` | `[stale]` |
| Otherwise | `[running]` |

The `[killed] > [stale] > [running]` priority order means a killed subagent never renders as stale, even if it was already past the threshold when killed.

### What you see

```text
Active subagents:
├── sub_3f9a12  orchestrate the migration pipeline  42s   [running]
│   ├── sub_a1b2c3  fetch and parse data            30s   [running]
│   └── sub_d4e5f6  write output files              180s  [stale]
└── sub_7g8h9i  run full test suite                 18s   [running]
```

In the flat-list fallback (when no nesting exists), the pill is appended only when status is not `[running]` — preserving byte-identical legacy output for the common non-orchestrated case:

```text
Active subagents:
- subagent-1 (sub_3f9a12c8d041) (orchestrate the migration pipeline) — 42s
- subagent-2 (sub_d4e5f6a7b8c9) (write output files) — 180s [stale]
```

### Tracing emission

When a subagent first crosses the stale threshold, a single `tracing::warn!` fires with `subagent_id`, `idle_secs`, and `stale_warn_seconds`. The emission is deduplicated per-id — the warn fires exactly once per subagent crossing, not once per render frame. The dedup state is cleared when the subagent deregisters, so a long-lived id that re-registers (never happens in practice — subagent ids are unique nonces) would start fresh.

### Operator response

`[stale]` is informational. The recommended response is:

- **Tail the transcript** with `/agents logs <id>` to see what the child was doing.
- **Decide:** if the work is recoverable (a research subagent waiting on a slow API), leave it alone — the hard-kill ceiling (`child_timeout_seconds`, default 300s) will eventually fire if it never recovers.
- **Or terminate:** if you want it gone now, use `/agents kill <id>` (Shrike Service below).

The stale threshold defaults to 120s. Raise it per-call (`stale_warn_seconds`) for legitimately long-running research tasks where 120s of idle time is normal.

---

## Shrike Service — Operator Termination Surface

Phase 32.3 added a coherent termination/diagnostic subcommand family on `/agents`, available across all three surfaces (TUI, web, gateway/Telegram). The internal Rust module is `crates/ironhermes-agent/src/shrike.rs`; the operator-facing name "shrike" is the bird-of-prey metaphor for the surface that impales runaway children cleanly.

All four subcommands route through the same `ShrikeService` library, so behavior is identical regardless of which surface you issue the command from.

### `/agents kill <id>` — hard kill

Cancels the cancellation token AND aborts the child's `JoinHandle`. The RAII `RegistrationGuard` (see the 6.7-hour ghost note below) fires on the dropped future and deregisters atomically — no leak path.

```text
> /agents kill sub_3f9a12
Killed sub_3f9a12 (ran 42s, 0 turns).
```

Use when:
- A subagent is wedged on a tool call that ignores its cancel token.
- You want immediate cleanup, not graceful finalization.

### `/agents interrupt <id>` — soft cancel

Cancels the cancellation token only — does NOT abort the `JoinHandle`. The child observes the cancel at its next iteration boundary and finalizes naturally (writing any pending transcript output, returning a structured result).

```text
> /agents interrupt sub_3f9a12
Interrupted sub_3f9a12 — finalizing...
```

Use when:
- You want to stop the work but keep whatever partial output the child has accumulated.
- The child is in the middle of a clean tool sequence that should finish its current step.

### `/agents prune` — sweep stale entries

Walks the registry, finds every entry idle longer than the configured stale threshold (120s default), cancels each one's token, and aborts each `JoinHandle`. Returns the list of pruned ids.

```text
> /agents prune
Pruned 2 stale entries: [sub_d4e5f6, sub_7g8h9i]
```

Or if nothing is stale:

```text
> /agents prune
No stale entries to prune.
```

Use when:
- You see multiple `[stale]` entries and want to clean them up in one shot.
- You suspect a leak (pre-32.3 6.7-hour-ghost class) — `prune` is the one-shot manual override that fixes already-leaked ghosts without restart. (Phase 32.3 RAII guard makes leaks structurally impossible going forward, but `prune` remains a useful belt-and-braces tool.)

### `/agents status <id>` — diagnostic snapshot

Returns a key/value block (TUI/gateway) or JSON (web) with every observable field on the subagent:

```text
> /agents status sub_3f9a12
id:                 sub_3f9a12c8d041
parent_id:          (root)
task_summary:       orchestrate the migration pipeline
role:               orchestrator
depth:              0
uptime_secs:        42
last_activity_secs: 3
turns_used:         (not yet wired)
transcript_path:    ~/.ironhermes/subagent-transcripts/abc/sub_3f9a12c8d041.jsonl
status:             running
```

Fields that the underlying registry doesn't yet capture (`turns_used` is a Phase 32.4+ ActivityTracker counter wiring) render as `(not yet wired)`. The field is reserved in the struct so the diagnostic shape stays stable as the registry grows.

### Gateway Confirm Token (Telegram only)

Telegram messages can come from spoofed message-edit replays, and the gateway has no synchronous user presence. To resist replay attacks, destructive operations on the gateway surface require a literal `confirm` token as the LAST argument:

```text
/agents kill sub_3f9a12 confirm
/agents prune confirm
```

`/agents interrupt` and `/agents status` are non-destructive and do NOT require `confirm` on any surface.

The TUI and web surfaces do NOT require `confirm` for any subcommand — the operator is clearly synchronously present at the keyboard. The Telegram divergence is the only behavioral asymmetry between surfaces.

If you forget the `confirm` token on Telegram, the gateway returns an explanatory error rather than silently accepting the destructive op:

```text
> /agents kill sub_3f9a12
Destructive op requires 'confirm' on Telegram: /agents kill sub_3f9a12 confirm
```

---

## The 6.7-Hour Ghost Bug (Phase 32.3 Fix Reference)

If you saw `/agents` reporting a subagent as Active for hours after its output files were written, this was a pre-32.3 registry-leak bug. The canonical live repro was `sub_20667cb71808` remaining Active for 24,150s (~6.7 hours) after writing its final files at 02:52 AM — the registry entry leaked because the `tokio::time::timeout` wrapper dropped the `run_child` future before its explicit `unregister` call could run.

Phase 32.3 fixed it with a RAII `RegistrationGuard` whose `Drop` impl calls `unregister_internal` synchronously. Every exit path — natural completion, timeout-induced drop, panic, cancel, `JoinHandle::abort` — now deregisters atomically. The fix is at the type level: Rust's drop semantics guarantee the guard fires when the future is dropped, so no code path can bypass cleanup.

The relevant invariants are locked by regression tests:
- `test_guard_deregisters_on_natural_completion`
- `test_guard_deregisters_on_timeout` — **THE canonical 6.7-hour ghost regression**
- `test_guard_deregisters_on_panic`
- `test_guard_deregisters_on_cancel`
- `test_registration_counter_balanced`

If you ever see a stuck registry entry post-32.3, run `/agents prune` to clear it and file an issue — the structural guarantee should make this class of bug impossible, so a recurrence indicates a regression worth investigating. See `.planning/phases/32.3-delegation-agent-runaway/32.3-CONTEXT.md` and `32.3-RESEARCH.md` for the engineering detail.

---

## Configuration

Set global defaults in `~/.ironhermes/config.yaml` under the `delegation:` key:

```yaml
delegation:
  # Wall-clock timeout per child agent in seconds (default: 300)
  child_timeout_seconds: 300

  # Stale-warn threshold; soft signal only. Hard kill ceiling is child_timeout_seconds.
  # Seconds of inactivity before a subagent is flagged [stale] in /agents output (default: 120)
  stale_warn_seconds: 120

  # Maximum concurrent children per batch (default: 3)
  max_concurrent_children: 3

  # Maximum LLM iterations per child agent (default: 50)
  max_iterations: 50

  # Maximum spawn depth for orchestrator children (default: 1 = flat)
  # 1 → only root agents can delegate (orchestrator children are downgraded to leaf)
  # 2 → one level of orchestrator children allowed
  # 3 → two levels (maximum supported value)
  max_spawn_depth: 1

  # Global kill switch: set false to force all children to leaf regardless of role= (default: true)
  orchestrator_enabled: true

  # Default toolset groups when none are specified per-call (default: all three)
  default_toolsets: ["terminal", "file", "web"]

  # Route children to a cheaper/faster model (default: inherit parent's model)
  # model: "google/gemini-flash-2.0"
  # provider: "openrouter"

  # Or point to a local endpoint
  # model: "qwen2.5-coder"
  # base_url: "http://localhost:1234/v1"
  # api_key: "local-key"
```

The semaphore emits a `tracing::warn!` at target `ironhermes_tools::delegate_task` if any task waits more than 2 seconds for a slot — useful for diagnosing concurrency pressure under high `max_concurrent_children` values.

---

## Key Properties

- **Flat delegation by default.** At `max_spawn_depth: 1` (the default), `role="orchestrator"` is a no-op — the child is silently downgraded to leaf and `delegate_task` remains excluded from its registry. Raise `max_spawn_depth` to 2 or 3 to allow orchestrator children to spawn their own subagents.
- **Isolated terminal CWD.** Each child's `terminal` tool starts in a fresh temp directory, not the parent's working directory (AGENT-04).
- **Read-only memory.** Children can read shared memory but cannot write to it (D-12).
- **Cancel propagation.** Interrupting the parent cancels all `detach=false` children. `detach=true` children run to completion.
- **Inherit credentials.** Children inherit the parent's API key, provider, and rate-limit pool. Model can be overridden per-call or globally via config.
- **Only the summary lands in parent context.** The full child turn history stays in the transcript file — the parent sees only the structured `**Actions Taken / Files Modified / Findings / Issues Encountered**` block.

---

## Migration from `subagent:` to `delegation:`

Phase 32.2 (D-07) renamed the config key and several fields. If your `~/.ironhermes/config.yaml` still uses the old `subagent:` key, IronHermes will refuse to start with a clear error:

```
Config key 'subagent:' is deprecated and no longer supported.
Rename it to 'delegation:' in your config.yaml.
```

Apply this diff to your config file:

```yaml
# BEFORE (old subagent: key)
subagent:
  timeout_secs: 300
  max_subagents: 3
  max_iterations: 10
  default_toolsets: ["terminal", "file", "web"]

# AFTER (new delegation: key)
delegation:
  child_timeout_seconds: 300
  max_concurrent_children: 3
  max_iterations: 50
  max_spawn_depth: 1
  orchestrator_enabled: true
  default_toolsets: ["terminal", "file", "web"]
```

Field rename summary:

| Old field | New field | Notes |
|-----------|-----------|-------|
| `timeout_secs` | `child_timeout_seconds` | Same semantics |
| `max_subagents` | `max_concurrent_children` | Same semantics |
| `max_iterations` | `max_iterations` | Default changed: 10 → 50 |
| _(new)_ | `max_spawn_depth` | Default: 1 (flat) |
| _(new)_ | `orchestrator_enabled` | Default: true |
| _(new — Phase 32.3)_ | `stale_warn_seconds` | Default: 120. Soft-warn threshold; absent in pre-32.3 configs is treated as 120 via serde default. Per-call override on `delegate_task`. |
