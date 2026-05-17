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

Up to 3 concurrent subagents by default (configurable via `subagent.max_subagents`):

```
delegate_task(tasks=[
    {"goal": "Research Tokio async patterns",   "toolsets": ["web"]},
    {"goal": "Research tower middleware design", "toolsets": ["web"]},
    {"goal": "Fix the failing clippy lint",      "toolsets": ["terminal", "file"]}
])
```

Batch mode uses `tasks` (array of objects with `goal`, optional `context`, optional `toolsets`). Single mode uses `task` (string). The two are mutually exclusive.

Results are sorted by original task index regardless of completion order. Batches larger than `max_subagents` are truncated with a `tracing::warn!` — they are not silently dropped.

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
| `delegate_task` | No recursion — flat delegation only (AGENT-05) |
| `skills` | Not available to subagents |
| `execute_code` | Not available to subagents |
| `cronjob` | Not available to subagents |

`memory` is available but **read-only** in child context — children can read shared memory but cannot write to it (D-12).

Unknown tool names in `allowed_tools` cause an immediate error (fail-early, D-04).

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

Override the global `timeout_secs` for a single call:

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

---

## Configuration

Set global defaults in `~/.ironhermes/config.yaml` under the `subagent:` key:

```yaml
subagent:
  # Wall-clock timeout per subagent in seconds (default: 300)
  timeout_secs: 300

  # Maximum concurrent subagents per batch (default: 3)
  max_subagents: 3

  # Maximum LLM iterations per subagent (default: 10)
  max_iterations: 10

  # Default toolset groups when none are specified per-call (default: all three)
  default_toolsets: ["terminal", "file", "web"]

  # Route subagents to a cheaper/faster model (default: inherit parent's model)
  # model: "google/gemini-flash-2.0"
  # provider: "openrouter"

  # Or point to a local endpoint
  # model: "qwen2.5-coder"
  # base_url: "http://localhost:1234/v1"
  # api_key: "local-key"
```

The semaphore emits a `tracing::warn!` at target `ironhermes_tools::delegate_task` if any task waits more than 2 seconds for a slot — useful for diagnosing concurrency pressure under high `max_subagents` values.

---

## Key Properties

- **Flat delegation only.** `delegate_task` is structurally excluded from every child registry (AGENT-05). Children cannot spawn grandchildren — there is no `role` parameter or `max_spawn_depth` setting.
- **Isolated terminal CWD.** Each child's `terminal` tool starts in a fresh temp directory, not the parent's working directory (AGENT-04).
- **Read-only memory.** Children can read shared memory but cannot write to it (D-12).
- **Cancel propagation.** Interrupting the parent cancels all `detach=false` children. `detach=true` children run to completion.
- **Inherit credentials.** Children inherit the parent's API key, provider, and rate-limit pool. Model can be overridden per-call or globally via config.
- **Only the summary lands in parent context.** The full child turn history stays in the transcript file — the parent sees only the structured `**Actions Taken / Files Modified / Findings / Issues Encountered**` block.
