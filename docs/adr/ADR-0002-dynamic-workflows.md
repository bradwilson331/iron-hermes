---
type: adr
id: ADR-0002
title: Dynamic Workflows — a durable DAG orchestration engine for IronHermes
status: Proposed
date: 2026-08-11
owners: ["@bradwilson331"]
links:
  - ./ADR-0001-okf-codebase-knowledge-graph.md
  - ../ARCHITECTURE.md
  - ../DELEGATION.md
  - ../kanban/reference.md
---

# ADR-0002: Dynamic Workflows — a durable DAG orchestration engine for IronHermes

- **Status:** Proposed
- **Date:** 2026-08-11
- **Deciders:** operator (@bradwilson331), dev agent (kanban task `t_e04641cc9264406f`)
- **Supersedes:** none · **Superseded by:** none
- **Reference:** Claude Code Dynamic Workflows, GA in v2.1.154 (released 2026-05-28; announced 2026-06-02) — <https://docs.claude.com/en/docs/claude-code/workflows>, <https://claude.com/blog/a-harness-for-every-task-dynamic-workflows-in-claude-code>

---

## Context

### What IronHermes has today (verified by code audit, 2026-08-11)

IronHermes already ships four workflow-*adjacent* primitives. Each was read at
source; claims below carry file:line references.

**1. Cron jobs with `context_from` chaining** — `crates/ironhermes-cron`,
`crates/ironhermes-cron-runner`.

- `CronJob.context_from: Option<Vec<String>>` — `crates/ironhermes-cron/src/job.rs:135`.
- Resolution semantics: `resolve_context_from` —
  `crates/ironhermes-cron-runner/src/prompt_builder.rs:106-170`. For each source
  id: (a) **UUID-guard** — non-UUID ids are skipped with a warning (line 121);
  (b) read `${IRONHERMES_HOME}/cron/output/{id}/`, take the **lexicographically
  last file** (i.e. most-recent-by-timestamp-filename, lines 134-143); (c)
  truncate at **8,000 bytes** (line 17, `CONTEXT_FROM_MAX_BYTES`); (d) splice
  into the prompt as `## Output from job '{id}'` blocks.
- Assembly order is fixed: banner → skills → script output → `context_from`
  blocks → user prompt (`prompt_builder.rs:188-224`), with a post-assembly
  injection rescan (`scan_cron_prompt`, line 217).

**Chaining is therefore "most-recent-completed output of a named upstream job,
prompt-spliced."** There is no dependency *graph*: the cron tick fires each job
on its own schedule regardless of whether its `context_from` sources have run
since the last tick, failed, or ever run at all (missing output dir = silently
skipped, line 136). There is no branching — the prompt cannot conditionally
include/exclude upstream blocks — and no fan-out join: N sources are simply
concatenated.

**2. In-process subagent fan-out** — `delegate_task`,
`crates/ironhermes-tools/src/delegate_task.rs`.

- Tool impl at line 1036; single-task `execute` at 1140; **batch mode**
  `execute_batch` at 452 (spawns one `tokio::spawn` per task, line 607).
- Concurrency: shared `tokio::sync::Semaphore` sized by
  `SubagentConfig.max_concurrent_children` — **default 3**
  (`crates/ironhermes-core/src/config.rs:3478,3511`). Batch submissions larger
  than the semaphore are rejected outright (`delegate_task.rs:462-470`), so
  parallel batch fan-out is effectively **capped at 3**.
- Limits: `child_timeout_seconds` **300 s** (config.rs:3468,3507),
  `max_iterations` **20** (3483,3512), `max_spawn_depth` **1** (3497) —
  flat delegation only by default.
- **No data-flow wiring between siblings**: batch tasks are independent;
  results are collected and re-sorted by index (743-752). Dependency tracking
  is a cancellation-token tree (parent→child kill propagation, 1266-1273), not
  an output→input DAG.

**3. Durable kanban orchestration** — `crates/ironhermes-kanban`.

- `kanban_swarm` (`src/tools/swarm.rs:48`, store primitive
  `KanbanStore::create_swarm` at `src/store.rs:1025`) materializes
  root → workers → verifier → synthesizer graphs in **one atomic SQLite
  transaction** (1076). Four documented shapes (P1 fan-out, fan-out+verify,
  4-tier, P3 quorum) — `swarm.rs:11-17`.
- A **real dependency edge table exists**: `task_links` (`types.rs:139`), with
  recursive-CTE cycle rejection (`insert_link_checked`, `store.rs:897,953-970`)
  and parent-gating (children stay `todo` until all parents are `done`).
- Handoff between tasks is the **completed run's summary + metadata**, read by
  downstream children via `build_worker_context` (`docs/kanban/reference.md`
  §"Runs — one row per attempt") — again prompt-splicing, not structured state.
- Execution is dispatcher-driven, **one OS process per worker**
  (`src/dispatcher.rs`, `worker_spawn.rs`). There is no in-memory scheduler,
  no per-run concurrency budget, and no aggregate token/spend cap beyond the
  generation ledger (`try_reserve_generation_slot`, `store.rs:1477`).
- Goal mode (`goal_mode` on `kanban_create`) gives a single task an
  in-session judge loop (`crates/ironhermes-cli/src/kanban/goal_loop.rs:180-385`),
  but it does not coordinate *between* tasks.

**4. Hooks** — `crates/ironhermes-hooks`. Seven lifecycle events
(`src/event.rs:19-73`: `MessageReceived`, `ToolCalled`, `ToolCompleted`,
`ResponseSent`, `SkillActivated`, `ContextPreCompress`, `ContextPressure`) with
webhook delivery + HMAC + retry (`webhook.rs`), and the guardrail chokepoint
`gated_exec.rs`. Hooks *react* to agent events; they do not *sequence* work.

### The reference model: Claude Code Dynamic Workflows (v2.1.154+, May 2026)

Verified against primary Anthropic sources (docs + launch blog + changelog):

- A workflow is a **JavaScript orchestration script** (plain JS, top-level
  `await`, no `import()`, no direct fs/shell — agents do I/O). Claude writes
  the script from a natural-language task; a **runtime executes it in the
  background, isolated from the conversation**.
- Special functions: `agent(prompt, options)` spawns a subagent (returns
  `null` on stop/unrecoverable error); `pipeline(list, fn)` maps one agent per
  item. Options include JSON `schema` for structured output, per-stage `model`,
  `label`, worktree isolation.
- **Plan state lives outside the context window**: the script holds the loop,
  branching, and intermediate results in **script variables**; the model's
  context holds only the final answer. (Docs comparison table: subagents/skills
  → intermediates in "Claude's context window"; workflows → "script variables".)
  Runs are resumable in-session (cached agent results replay to the first
  unfinished agent).
- **Limits: up to 16 concurrent agents** (fewer on low-core machines), **1,000
  agents total per run**; "Large workflow" advisory >25 agents or >1.5M
  projected tokens; configurable size guideline (default `medium` <50 since
  v2.1.219).
- **Adversarial verification & fold-back**: named patterns include
  fan-out-and-synthesize, adversarial verification (a separate agent verifies
  each worker's output against a rubric), generate-and-filter, tournament,
  classify-and-act, loop-until-done. The bundled `/deep-research` workflow
  cross-checks sources and votes on claims.
- **Surfaces**: CLI, Desktop, VS Code extension, headless (`claude -p`), Agent
  SDK. Managed via `/workflows` (list/pause/stop/restart/save); scripts saved
  as commands in `.claude/workflows/` and distributed via plugins.
- Adjacent primitives: subagents (Markdown+YAML in `.claude/agents/`), agent
  teams (experimental; the *lead agent* holds the plan, vs. workflows where the
  *script* holds it), `/loop` (session-scoped cron), hooks (fire for
  workflow-spawned subagent tool calls too).

### The gap

| Capability | Claude Code | IronHermes today |
|---|---|---|
| Orchestration program (loops, branches, joins) | JS script | none — cron prompts are static text |
| True dependency DAG with conditional edges | script `await` graph | `task_links` gates promotion only; no conditionals, no data edges |
| Plan state outside context | script variables | none — all handoff is prompt-spliced (8 KB cap for cron) |
| Parallel fan-out with bounded concurrency | 16 concurrent / 1,000 per run | `delegate_task`: 3 concurrent, in-process only; swarm: unbounded process-per-worker |
| Structured inter-step results | JSON schema per `agent()` call | free-text summaries on runs |
| Adversarial verify / fold-back | first-class patterns | verifier/synthesizer card *shapes* exist; no rubric loop, no per-item verify |
| Resumability | replay to first unfinished agent | task retries restart from scratch |
| Single run identity spanning steps | one workflow run | none — cron jobs and kanban graphs are unrelated stores |

The gap is not "IronHermes lacks orchestration" — it is that orchestration
state is **either in the model's context (cron prompts, summaries) or in the
scheduling layer (task status)**, never in an executable plan with its own
state store. That is precisely the deficiency Dynamic Workflows were designed
to remove (motivation per the launch blog: agentic laziness, self-preferential
bias, goal drift after compaction).

## Decision

**Adopt a Dynamic Workflows engine for IronHermes, built on the kanban store
and dispatcher rather than on cron or on an embedded JS runtime.** The design
has five pillars; each is anchored to an existing IronHermes primitive.

### D-1. Workflow = a DAG of steps persisted in the kanban store

A new table pair in `ironhermes-kanban`'s SQLite store (same DB as
`task_links`):

- `workflows` — one row per workflow *definition*: id (UUID), name, spec
  (YAML/JSON, see D-2), created_by, created_at, version.
- `workflow_runs` — one row per *execution*: id, workflow_id + version,
  status (`running`/`paused`/`completed`/`failed`/`cancelled`), a **JSON plan
  state blob** (D-3), concurrency budget, counters, timestamps.
- `workflow_steps` — one row per step instance in a run: run_id, step key,
  kind (`agent`, `map`, `gate`, `join`, `verify`), assignee profile, prompt
  template, `depends_on: [step keys]`, `when` expression, status, and a
  **link to the kanban task id** that executes it.

Every `agent`-kind step is materialized as a **kanban task** with
`parents = [tasks of its depends_on steps]`. This reuses — unchanged — the
existing parent-gating, cycle rejection (`insert_link_checked`,
`store.rs:897`), retry/failure accounting (`task_runs`), claim CAS,
heartbeats, and notification plumbing. The workflow engine never implements
its own scheduler for agent work; it *projects* the DAG onto the kanban
dependency graph and lets the dispatcher run it.

### D-2. Orchestration spec = declarative YAML/JSON, not a Turing-complete script

Claude Code uses a JS script. IronHermes deliberately does **not** embed a JS
interpreter (no runtime in the workspace today; a sandboxed JS engine is a new
supply-chain and escape-audit surface — cf. the v2.1.223 `import()` fix). The
IronHermes spec is a declarative document that an LLM can author and a human
can review:

```yaml
name: route-audit
concurrency: { max_parallel: 8, max_steps: 200 }   # see D-4
steps:
  - key: enumerate
    kind: agent
    assignee: scout
    prompt: "List every route handler under src/ as JSON {routes: [...]}"
    output_schema: { type: object, required: [routes] }

  - key: audit
    kind: map                    # one agent task per element of state.enumerate.routes
    over: steps.enumerate.output.routes
    assignee: auditor
    prompt: "Audit {{ item }} for injection flaws. Return {file, findings: [...]}"

  - key: verify
    kind: verify                 # adversarial pass per audit result, against rubric
    over: steps.audit.output
    assignee: reviewer
    rubric: "Confirm each finding reproduces from the cited code; reject speculation"
    on_reject: annotate          # annotate | retry | drop

  - key: fold
    kind: join
    depends_on: [verify]
    assignee: writer
    prompt: "Rank and merge verified findings into one report"
    when: "steps.verify.rejected_count < steps.verify.total * 0.5"
```

Branching is expressed by `when` expressions evaluated over plan state (D-3)
at scheduling time — not by control flow in a general-purpose language. Loop
patterns (`loop-until-done`) are expressed as a `gate` step whose `when`
spawns a bounded `repeat` block (hard-capped by `max_steps`). This covers the
documented Claude Code pattern vocabulary (fan-out-and-synthesize, verify,
generate-and-filter, tournament, classify-and-act) while keeping every
execution decision inspectable in the store.

Authoring flow mirrors Claude Code: the user describes the goal; an
orchestrator-profile agent drafts the spec; `hermes workflow run` (or
`workflow_run` LLM tool, or gateway `/workflow`) validates and starts it.
Specs are saved under `~/.ironhermes/workflows/` (and project
`.ironhermes/workflows/`), matching the skills two-level load paths
(`crates/ironhermes-core/src/skills.rs:494-506`).

### D-3. Plan state = a versioned JSON blob on the run, addressed by step key

Each step's **structured output** (validated against its `output_schema`,
enforced the way `output_schema` is enforced for delegate_task batch results)
is written to the run's state blob at `steps.<key>.output` on task completion
— written by the workflow engine in a `task_events` listener, not by the
worker. Downstream prompt templates resolve `{{ steps.audit.output }}` from
the blob at task-claim time, so **intermediate results never enter any
model's context unless a template explicitly injects them** — the direct
analog of "script variables, context holds only the final answer."

- Blob is capped (default 1 MiB) with per-step truncation marks; oversized
  artifacts are spilled to `${IRONHERMES_HOME}/workflow/output/{run_id}/` and
  referenced by path — generalizing the existing cron output-dir convention
  (`prompt_builder.rs:129-143`) instead of inventing a second one.
- State transitions are single-writer (the engine) with optimistic version
  checks, same discipline as the dispatcher's claim CAS.
- **Resumability**: a crashed run restarts by re-projecting unfinished steps;
  completed step outputs are already in the blob, so no agent work repeats —
  matching Claude Code's replay-to-first-unfinished-agent semantics.

### D-4. Parallel scheduling = a semaphore-budgeted projector over the dispatcher

A new `workflow-runner` loop (alongside `cron-runner`'s `tick_loop.rs` as a
sibling crate, `ironhermes-workflow`) runs inside the gateway/dispatcher
process:

1. For each `running` workflow run, find steps whose `depends_on` are all
   satisfied and whose `when` evaluates true.
2. Admit steps until the run's `max_parallel` **or** a global
   `workflow.max_parallel` (default **16**, mirroring the Claude Code cap;
   scaled down by host core count the same way) is reached; each admitted step
   becomes a kanban task (`todo`, parents set, idempotency key
   `wf:{run}:{step}` so projector crashes never double-materialize — same
   replay scheme as `create_swarm`, `store.rs:1064-1072`).
3. `max_steps` per run (default **1,000**) is a hard ceiling enforced at
   admission, backstopped by the existing generation ledger
   (`try_reserve_generation_slot`, `store.rs:1477`) keyed on the run id for
   cross-process spend bounding.
4. Projected token estimate + step count feed a "large workflow" advisory
   (thresholds 25 steps / 1.5M tokens, mirroring v2.1.203) delivered as a
   kanban comment + gateway notification before admission.

`map` steps chunk large lists (default 25 per admission wave) for
backpressure; workers remain one-process-per-task under the dispatcher, so
resource isolation is unchanged from today's swarm model. `delegate_task`
stays what it is — the *in-session* fan-out tool — and is the recommended
mechanism *inside* a step when a step itself needs a burst of ≤3 children.

### D-5. Verification and fold-back are step kinds, not conventions

- `verify` steps materialize one reviewer task per worker output, with the
  rubric and the worker's structured output in the prompt; the reviewer's
  schema is `{verdict: accept|reject, reason}`. Rejections follow
  `on_reject: annotate | retry | drop` (retry re-queues the producing step
  with the rejection reason appended — reusing `task_runs` attempt history).
- `join` steps are the fold-back barrier: they cannot be admitted until all
  upstream `map`/`verify` instances are `done` (enforced by kanban
  parent-gating, not by polling), and their template receives the aggregated
  outputs.
- Hooks integration: `workflow_step_completed` / `workflow_run_completed`
  become new `HookEventKind`s (`event.rs:19-73` is an enum — extension point
  already exists), so existing webhook consumers can observe runs; workflow
  agent tasks' tool calls flow through `gated_exec` unchanged.

### Surfaces

- **CLI**: `hermes workflow new|run|list|show|pause|resume|cancel|save`.
- **LLM tools** (kanban toolset): `workflow_run`, `workflow_list`,
  `workflow_show`, `workflow_cancel` — registered beside `kanban_swarm`.
- **Gateway**: `/workflow` command family on Telegram/Discord/Slack; terminal
  run events delivered via the existing notifier
  (`docs/kanban/reference.md` §Gateway notifications).
- **Kanban UI**: a run viewer reusing the board's task cards (each step *is* a
  task) plus a DAG overlay read from `workflow_steps`.
- **Headless/SDK parity**: workflows start immediately in non-interactive
  mode (no approval prompt), mirroring the Claude Code `-p`/SDK behavior.

### What this is NOT

- Not a replacement for cron. Cron remains the right tool for "fire this
  prompt on a schedule." A cron job may *trigger* a workflow run
  (`deliver`-adjacent target), and P4 long-running-journal patterns stay on
  cron.
- Not a replacement for `kanban_swarm`. Swarm remains the one-shot,
  human-clickable fan-out; workflows add branching, maps, verify loops,
  state, and resume. Internally the workflow projector *uses* the same
  atomic-graph insert path.
- Not an in-context planner. No agent "holds" the plan; the store does.

## Alternatives considered

### Alt-1. Extend `context_from` cron chaining into a DAG

Add `depends_on`, `when`, and a state file to `CronJob`. **Rejected.** Cron
jobs are schedule-driven, not completion-driven: the tick loop
(`ironhermes-cron-runner/src/tick_loop.rs`) fires on wall-clock time, so a
dependency edge would either be a polling hack or require the runner to become
a second dispatcher. The 8 KB prompt-splice state model
(`prompt_builder.rs:17`) is fundamentally the wrong substrate for structured
intermediate state. Cron's store has no run/step identity to hang resume on.
Every added feature would be a worse copy of what `task_links` already does.

### Alt-2. Embed a JavaScript engine and copy Claude Code verbatim

Sandboxed `deno_core`/`boa` executing an orchestration script with `agent()`
and `pipeline()` intrinsics. **Rejected (with prejudice toward later
reconsideration).** (a) New dependency + sandbox escape-audit surface in a
security-sensitive codebase that just finished hardening prompt assembly
against injection. (b) A script's state is opaque to the operator between
checkpoints; IronHermes's operating model (kanban UI, gateway notifications,
run postmortems) wants every step to be a first-class, inspectable task.
(c) The differentiating value — plan-outside-context, parallel limits,
adversarial verify — lives in the *semantics*, not in JS syntax; the
declarative spec captures all of it and is directly diffable/reviewable in
git. If ecosystem compatibility with Claude Code workflow files ever matters,
a JS-spec *importer* (transpile to the YAML DAG) is a smaller, safer bridge
than a runtime.

### Alt-3. Do nothing; document swarm + goal-mode recipes

**Rejected.** Swarm shapes are fixed at creation — no branching, no maps over
runtime data, no retry-with-rubric loop, no resume. Goal mode loops *one*
task against a judge; it cannot coordinate a graph. The gap analysis above
shows these are compositional, not incremental, deficiencies.

## Consequences

### Positive

- True DAG execution with branching and runtime-sized fan-out, reusing the
  battle-tested dispatcher, claim CAS, retries, and notifications.
- Plan state leaves the context window: intermediate results live in the run
  blob, cutting token spend and goal drift on long orchestrations (the exact
  failure mode the Claude Code launch blog cites).
- Concurrency and spend become operator-controllable per run and globally,
  with a single ledger (generation ledger) already cross-process safe.
- Every step is a kanban task: UI, notifications, postmortems, and the eight
  documented collaboration patterns (P1–P9) compose with workflows for free.
- Cron is unburdened; `context_from` stays the simple 80% case it is today.

### Negative / costs

- New crate (`ironhermes-workflow`), two new tables, a projector loop, and an
  expression evaluator for `when` (keep it tiny: comparisons, arithmetic,
  `len()`, `steps.*` paths — no arbitrary eval).
- Dual write-path discipline: step status lives on both the kanban task and
  `workflow_steps`; the event listener must reconcile (task is authoritative
  for execution, step row is authoritative for the DAG view).
- Spec validation and prompt-template resolution become new injection
  boundaries — templates resolve at claim time and must pass through the same
  `scan_cron_prompt`-class rescan before dispatch.
- Operator learning curve: one more noun ("workflow") alongside cron, swarm,
  and goal mode.

### Migration & compatibility

- **Nothing is removed.** Cron, `context_from`, swarm, goal mode, hooks all
  keep current behavior.
- Phase 1 (read-only): ship tables + `hermes workflow list/show`; projector in
  dry-run mode logging what it *would* admit.
- Phase 2: enable admission behind `workflow.enabled: true` (default false);
  ship three ported reference specs mirroring the documented swarm shapes
  (fan-out, fan-out+verify, 4-tier) so users can A/B a swarm against the
  equivalent workflow.
- Phase 3: `hermes workflow import-cron <job-id>` — wraps a
  `context_from`-chained job set into a linear workflow spec (one step per
  job, `depends_on` from `context_from` order), giving existing chained crons
  a mechanical upgrade path. Documented in `docs/CRON-TEMPLATES.md`.
- `docs/kanban/reference.md` gains a "P10 Workflow DAG" pattern row;
  `docs/DELEGATION.md` gains "when to delegate vs. swarm vs. workflow."

### Risks (from the task's register, dispositioned)

- **Feature drift** → every pillar above names the IronHermes primitive it
  reuses; Alt-2 documents why verbatim copy was rejected.
- **Scope creep** → this ADR fixes decisions and crate boundaries only;
  implementation tickets are deferred to follow-up kanban tasks.
- **State model complexity** → the blob is single-writer with version checks;
  the compatibility note (task authoritative for execution) is explicit.
- **Concurrency safety** → 16-concurrent default, 1,000-step ceiling, token
  advisory, and ledger backstop mirror the reference model's published limits
  and reuse IronHermes's existing spend guard.
- **Stale assumptions** → all capability claims carry file:line references
  verified 2026-08-11; the code, not the triage summary, was treated as truth
  (notably: `context_from` resolution is most-recent-file, and delegate_task
  batch is hard-capped at 3 — both confirmed in source).
