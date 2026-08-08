---
name: kanban-worker
description: Pitfalls, examples, and edge cases for Hermes Kanban workers. The lifecycle itself is auto-injected into every worker's system prompt as KANBAN_GUIDANCE (defined in crates/ironhermes-kanban/src/kanban_guidance.rs, injected via crates/ironhermes-cli/src/main.rs); this skill is what you load when you want deeper detail on specific scenarios.
version: 2.1.0
platforms: [linux, macos, windows]
metadata:
  ironhermes:
        tags: [kanban, multi-agent, collaboration, workflow, pitfalls]
        related_skills: [kanban-orchestrator]
---

# Kanban Worker — Pitfalls and Examples

> You're seeing this skill because the IronHermes Kanban dispatcher spawned you as a worker with `--skills kanban-worker` — it's loaded automatically for every dispatched worker. The **lifecycle** (6 steps: orient → work → heartbeat → block → complete → terminate) also lives in the `KANBAN_GUIDANCE` block that's auto-injected into your system prompt. This skill is the deeper detail: good handoff shapes, the terminator/run-id contract, swarm root discovery, retry diagnostics, edge cases.

## Workspace handling

Your workspace kind determines how you should behave inside `$IRONHERMES_KANBAN_WORKSPACE`:

| Kind | What it is | How to work |
|---|---|---|
| `scratch` | Fresh tmp dir, yours alone | Read/write freely; it gets GC'd when the task is archived. |
| `dir:<path>` | Shared persistent directory | Other runs will read what you write. Treat it like long-lived state. Path is guaranteed absolute (the kernel rejects relative paths). |
| `project:<repo>` | Isolated git worktree of `<repo>` | The dispatcher already created this worktree (`git worktree add`) and handed it to you as your current directory — never create or `cd` into a worktree yourself. The referenced repo itself is never mutated in-place; commit your work in this worktree normally. |

Any files attached to the task (via the web UI or `kanban attach`) are already copied into your workspace root before you start — no fetch step needed, for any of the three kinds above.

## Tenant isolation

If `$IRONHERMES_TENANT` is set, the task belongs to a tenant namespace. When reading or writing persistent memory, prefix memory entries with the tenant so context doesn't leak across tenants:

- Good: `business-a: Acme is our biggest customer`
- Bad (leaks): `Acme is our biggest customer`

## Good summary + metadata shapes

The `kanban_complete(summary=..., metadata=...)` handoff is how downstream workers read what you did. Patterns that work:

**Coding task:**
```python
kanban_complete(
    summary="shipped rate limiter — token bucket, keys on user_id with IP fallback, 14 tests pass",
    metadata={
        "changed_files": ["rate_limiter.py", "tests/test_rate_limiter.py"],
        "tests_run": 14,
        "tests_passed": 14,
        "decisions": ["user_id primary, IP fallback for unauthenticated requests"],
    },
)
```

**Coding task that needs human review (review-required):**

For most code-changing tasks, the work isn't truly *done* until a human reviewer has eyes on it. Block instead of complete, with `reason` prefixed `review-required: ` so the dashboard surfaces the row as needing review. Drop the structured metadata (changed files, test counts, diff/PR url) into a comment first, since `kanban_block` only carries the human-readable reason — comments are the durable annotation channel. Reviewer either approves and runs `ironhermes kanban unblock <id>` (which re-spawns you with the comment thread for any follow-ups) or asks for changes via another comment.

```python
import json

kanban_comment(
    body="review-required handoff:\n" + json.dumps({
        "changed_files": ["rate_limiter.py", "tests/test_rate_limiter.py"],
        "tests_run": 14,
        "tests_passed": 14,
        "diff_path": "/path/to/worktree",  # or PR url if pushed
        "decisions": ["user_id primary, IP fallback for unauthenticated requests"],
    }, indent=2),
)
kanban_block(
    reason="review-required: rate limiter shipped, 14/14 tests pass — needs eyes on the user_id/IP fallback choice before merging",
)
```

Use `kanban_complete` only when the task is genuinely terminal — e.g. a one-line typo fix, a docs change with no functional consequences, or a research task where the artifact IS the writeup itself.

**Research task:**
```python
kanban_complete(
    summary="3 competing libraries reviewed; vLLM wins on throughput, SGLang on latency, Tensorrt-LLM on memory efficiency",
    metadata={
        "sources_read": 12,
        "recommendation": "vLLM",
        "benchmarks": {"vllm": 1.0, "sglang": 0.87, "trtllm": 0.72},
    },
)
```

**Review task:**
```python
kanban_complete(
    summary="reviewed PR #123; 2 blocking issues found (SQL injection in /search, missing CSRF on /settings)",
    metadata={
        "pr_number": 123,
        "findings": [
            {"severity": "critical", "file": "api/search.py", "line": 42, "issue": "raw SQL concat"},
            {"severity": "high", "file": "api/settings.py", "issue": "missing CSRF middleware"},
        ],
        "approved": False,
    },
)
```

Shape `metadata` so downstream parsers (reviewers, aggregators, schedulers) can use it without re-reading your prose.

## Claiming cards you actually created

If your run produced new kanban tasks (via `kanban_create`), pass the ids in `created_cards` on `kanban_complete`. The kernel verifies each id exists and was created by your profile; any phantom id blocks the completion with an error listing what went wrong, and the rejected attempt is permanently recorded on the task's event log. **Only list ids you captured from a successful `kanban_create` return value — never invent ids from prose, never paste ids from earlier runs, never claim cards another worker created.**

```python
# GOOD — capture return values, then claim them.
c1 = kanban_create(title="remediate SQL injection", assignee="security-worker")
c2 = kanban_create(title="fix CSRF middleware", assignee="web-worker")

kanban_complete(
    summary="Review done; spawned remediations for both findings.",
    metadata={"pr_number": 123, "approved": False},
    created_cards=[c1["task_id"], c2["task_id"]],
)
```

```python
# BAD — claiming ids you don't have captured return values for.
kanban_complete(
    summary="Created remediation cards t_a1b2c3d4, t_deadbeef",  # hallucinated
    created_cards=["t_a1b2c3d4", "t_deadbeef"],                   # → gate rejects
)
```

If a `kanban_create` call fails (exception, tool_error), the card was NOT created — do not include a phantom id for it. Retry the create, or omit the id and mention the failure in your summary. The prose-scan pass also catches `t_<hex>` references in your free-form summary that don't resolve; these don't block the completion but show up as advisory warnings on the task in the dashboard.

## Block reasons that get answered fast

Bad: `"stuck"` — the human has no context.

Good: one sentence naming the specific decision you need. Leave longer context as a comment instead.

```python
kanban_comment(
    task_id=os.environ["IRONHERMES_KANBAN_TASK"],
    body="Full context: I have user IPs from Cloudflare headers but some users are behind NATs with thousands of peers. Keying on IP alone causes false positives.",
)
kanban_block(reason="Rate limit key choice: IP (simple, NAT-unsafe) or user_id (requires auth, skips anonymous endpoints)?")
```

The block message is what appears in the dashboard / gateway notifier. The comment is the deeper context a human reads when they open the task.

## Heartbeats worth sending

`kanban_heartbeat` is **deferred in v1** (per `KANBAN_GUIDANCE`). Until it lands, the substitute is to *say* when you expect a long run — in your work and in your final summary — so the operator knows you're alive rather than hung. The rules below apply to any status note you emit today and to `kanban_heartbeat` once it's active:

Good heartbeats name progress: `"epoch 12/50, loss 0.31"`, `"scanned 1.2M/2.4M rows"`, `"uploaded 47/120 videos"`.

Bad heartbeats: `"still working"`, empty notes, sub-second intervals. Every few minutes max; skip entirely for tasks under ~2 minutes.

## Protocol terminator contract

Your run ends with **exactly one** terminator call — `kanban_complete` or `kanban_block`. There is no implicit "done": if your process exits without calling one, the kernel records an exit-without-terminator and auto-blocks the task as a protocol violation, so the operator sees a stalled card instead of silent loss.

Terminating tools take an `expected_run_id`, which is `$IRONHERMES_KANBAN_RUN_ID`. If it no longer matches the task's active run, the terminator is rejected with a structured error — your run was superseded (reclaimed, or a newer worker took over). Don't retry the terminator; stop and exit cleanly. Whoever holds the live run owns the handoff now.

## Retry scenarios

If you open the task and `kanban_show` returns `runs: [...]` with one or more closed runs, you're a retry. The prior runs' `outcome` / `summary` / `error` tell you what didn't work. Don't repeat that path. Typical retry diagnostics:

- `outcome: "timed_out"` — the previous attempt hit `max_runtime_seconds`. You may need to chunk the work or shorten it.
- `outcome: "crashed"` — OOM or segfault. Reduce memory footprint.
- `outcome: "spawn_failed"` + `error: "..."` — usually a profile config issue (missing credential, bad PATH). Ask the human via `kanban_block` instead of retrying blindly.
- `outcome: "reclaimed"` + `summary: "task archived..."` — operator archived the task out from under the previous run; you probably shouldn't be running at all, check status carefully.
- `outcome: "blocked"` — a previous attempt blocked; the unblock comment should be in the thread by now.

## Notification routing

You can configure the gateway to receive cross-profile Kanban task notifications by adding `notification_sources` to `~/.ironhermes/config.yaml`.
- `notification_sources: ['*']` accepts subscriptions from all profiles.
- `notification_sources: ['default', 'zilor-ppt']` or `"default,zilor-ppt"` restricts subscriptions to specified profiles.
- Omitting the key keeps the default behavior (profile isolation).

## Do NOT

- Call `delegate_task` as a substitute for `kanban_create`. `delegate_task` is for short reasoning subtasks inside YOUR run; `kanban_create` is for cross-agent handoffs that outlive one API loop.
- Modify files outside `$IRONHERMES_KANBAN_WORKSPACE` unless the task body says to.
- Create follow-up tasks assigned to yourself — assign to the right specialist.
- Complete a task you didn't actually finish. Block it instead.

## Pitfalls

**Task state can change between dispatch and your startup.** Between when the dispatcher claimed and when your process actually booted, the task may have been blocked, reassigned, or archived. Always `kanban_show` first. If it reports `blocked` or `archived`, stop — you shouldn't be running.

**Workspace may have stale artifacts.** Especially `dir:` and `worktree` workspaces can have files from previous runs. Read the comment thread — it usually explains why you're running again and what state the workspace is in.

**Don't rely on the CLI when the guidance is available.** The `kanban_*` tools work across all terminal backends (Docker, Modal, SSH). `ironhermes kanban <verb>` from your terminal tool will fail in containerized backends because the CLI isn't installed there. When in doubt, use the tool.

## CLI fallback (for scripting)

Every tool has a CLI equivalent for human operators and scripts:
- `kanban_show` ↔ `ironhermes kanban show <id> --json`
- `kanban_complete` ↔ `ironhermes kanban complete <id> --summary "..." --metadata '{...}'`
- `kanban_block` ↔ `ironhermes kanban block <id> "reason"`
- `kanban_create` ↔ `ironhermes kanban create "title" --assignee <profile> [--parent <id>]`
- etc.

Use the tools from inside an agent; the CLI exists for the human at the terminal.

## Swarm graph root discovery

If you were spawned as part of a swarm graph, you find the shared root card from your own handoffs — no separate env var carries it:

1. `kanban_show()` (your own task) and read `parent_handoffs`. The root card is your parent — one hop up.
2. `kanban_show(<root_id>)` and read `comments`. The first comment is the swarm blackboard (author `swarm`); treat it as shared state for the whole graph.

In the 4-tier shape, verifier and synthesizer cards sit two hops from root (verifier via any worker, synthesizer via the verifier). This rides the existing 9-env worker contract — there's no extra spawn-env variable to read.

## Goal mode

Cards created with `goal_mode=true` (CLI flag `--goal`, LLM-tool arg
`kanban_create(..., goal_mode=true)`, default `goal_max_turns=20`) opt into an
in-session worker loop with automatic judge LLM evaluation. The dispatcher
claims goal-mode cards the normal way and spawns the worker the normal way.
The worker shell detects goal mode via two env vars set by the spawner:

- `IRONHERMES_KANBAN_GOAL_MODE=1` — flag indicating the worker should wrap
  `AgentLoop::run` in a budget-bounded loop.
- `IRONHERMES_KANBAN_GOAL_MAX_TURNS=<N>` — per-card turn budget. Default 20 (a
  caller passing `0` is coerced to 20 at two layers: producer-side in
  `KanbanStore::create_task` and again at `build_kanban_worker_env`).

On each turn the worker:

1. Runs one `AgentLoop::run` iteration in the same session.
2. Bumps the per-card turn counter under a CAS gate — a reclaimed worker
   cannot bump against a stale run.
3. Invokes an auxiliary judge LLM that evaluates the worker's output against
   the card's `title + body` (literal acceptance criteria; the `body` is
   load-bearing, not advisory).
4. On judge-met → loop exits cleanly; the worker LLM is expected to call
   `kanban_complete` next.
5. On judge-not-met → a synthetic user message carrying the judge's reason
   is appended to the chat and the next turn begins.
6. On budget exhaustion → the worker shell emits a synthetic
   `kanban_block(reason="goal_max_turns exhausted; needs human review")` so
   the card lands in BLOCKED for human review, never silently dropped.

Two consecutive judge errors → synthetic
`kanban_block(reason="judge unavailable")`. The 2-strike block is independent
of the budget path.

Goal-loop events ride on `KanbanEventKind::Edited` with JSON `subkind` tags:
`judge_verdict`, `judge_error`, `goal_turn_advanced`, `goal_budget_exhausted`.
No new event variants are introduced (frozen surface preserved per Phase
36.3.7.6).

### Reclaim contract (in-place handoff)

If your worker dies mid-loop (heartbeat timeout, crash, SIGKILL), the
dispatcher's reclaim path resets `goal_turns_used` to 0 so the next spawn
starts with a fresh budget. The card body (acceptance criteria) is
byte-stable across reclaim, and prior runs' `judge_verdict` events are NOT
deleted — you can `kanban_show` to read what the previous worker tried and
why the judge rejected it. The existing `failure_limit` circuit breaker
still applies: a goal_mode card that keeps killing its worker mid-loop will
trip the breaker at `consecutive_failures = failure_limit` and transition
to `blocked` with a `gave_up` event, never infinite re-spawn.

### Budget-exhaustion-as-BLOCKED contract

The worker shell guarantees that a card whose budget is exhausted lands in
BLOCKED, never silently exits with a `protocol_violation` event. The
RAII `BudgetSentinel` in the worker process emits the synthetic
`kanban_block` call before the worker process unwinds — so even a panic
in the goal loop's tail keeps the contract.
