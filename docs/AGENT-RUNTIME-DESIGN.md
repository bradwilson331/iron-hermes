# AgentRuntime — channel-facing agent API

Status: **IMPLEMENTED** — foundation + channel migration shipped to `develop`
(GSD Phase 28.1, completed 2026-05-21). One scoped follow-up remains: cron
subagent budget isolation (T-28.1-16, see §6.4 / §8).
Author: pairing session, 2026-05-21.

## 0. Implementation status (2026-05-21)

### Shipped (committed to `develop`)

- **`367eaa79` — gateway budget-latch fix (band-aid).** `BudgetHandle::reset()`
  added; `handler.rs::run_agent` calls it per top-level turn. This fixes the
  production bug (agent returning `turns_used=0` after the first
  budget-exhausting conversation) independent of the refactor. **Removed once the
  gateway migrates to `run_turn` (§5 stage 2).**
- **`e4c2ca48` — config collapse.** `config.agent.max_iterations` is now the
  single canonical per-turn cap (default unified to `DEFAULT_MAX_ITERATIONS`=90);
  `agent.max_turns` is a deprecated alias folded in by `AgentConfig::normalize()`
  with a warning. (Decision §6.1.)
- **`0f8807c6` — `AgentRuntime` built.** New `crates/ironhermes-agent/src/agent_runtime.rs`:
  owns client/registry/skills/browser/hooks + the shared `BudgetHandle`;
  `from_config` creates the budget and builds the subagent runner with a clone of
  it (decision A / §6.2); `run_turn(TurnRequest)` resets the budget at the turn
  boundary and assembles the loop (budget, hooks, skills, browser, memory,
  fallback, compression, `attach_context_engine`). Compiles clean; budget-reset
  semantics unit-tested at the `BudgetHandle` level. **Not yet wired into any
  channel.**

### Channel migration (GSD Phase 28.1 — completed 2026-05-21)

All four interactive channels and cron now go through `AgentRuntime`. Shipped in
plans 28.1-01..06 on `develop`:

1. **Gateway → `run_turn`** ✅ (§5 stage 2): `GatewayMessageHandler` sources its
   shared Arcs from the runtime; `run_agent` builds a `TurnRequest` and calls
   `run_turn`; the `budget_handle` field/setter and the `367eaa79` band-aid are
   gone. Trajectory writer, `gw:{chat}:{sender}` session-id format, and the
   LEARN-01 post-turn nudge are preserved.
2. **Web / `run_chat` / `run_single` / TUI → `run_turn`** ✅ (§5 stage 3): each
   builds one `AgentRuntime` and routes turns through `run_turn`; no channel
   constructs `BudgetHandle::new` or `.with_budget` by hand. Fixes the latent
   `run_chat`/TUI latches and the web top-level-loop budget gap.
3. **Cron distinct budget** ✅ (§6.4): `run_cron_job` builds a fresh per-job
   `BudgetHandle`, independent of any interactive runtime budget.
4. **Verification:** automated checks 7/7 (workspace tests, `--features server`
   build, wasm32 build all green); operator live UAT confirmed the Stop100 latch
   fix on the Telegram gateway (consecutive turns each get a fresh budget).

### Known limitation (open follow-up)

- **T-28.1-16 — cron subagent budget isolation.** Top-level cron turns are
  budget-isolated, but a cron job that invokes `delegate_task` reaches the
  interactive runtime's delegate runner via the shared `ToolRegistry`, so its
  subagents draw on (and can drain) the interactive budget rather than the
  cron-scoped one. Deferred per the threat model; tracked for a future phase.
  See §6.4 / §8.

---

## Original proposal follows.

## 1. Problem

Every channel (Telegram gateway, web UI, CLI `run_chat`/`run_single`, TUI) feeds
the same `AgentLoop` (`crates/ironhermes-agent/src/agent_loop.rs`), but each
channel independently:

- constructs its own `BudgetHandle::new(...)`,
- threads it into the per-turn `AgentLoop` via `.with_budget(...)`,
- wires the same handle into its `AgentSubagentRunner`,
- re-assembles the per-turn `AgentLoop` by hand (`.with_streaming`,
  `.with_active_skills`, `.with_memory_manager`, `.with_hook_registry`,
  `.with_browser_session`, fallback, …).

There is no single owner of "the agent." The limit is enforced in one place
(`AgentLoop::run`), but the **lifecycle is managed in four places**, each subtly
different. Consequences already observed:

- **Gateway budget latch (fixed with a band-aid):** the gateway built one
  `BudgetHandle` at startup and never reset it. After the first long
  conversation drained it to 0, every later message hit `Stop100` instantly and
  returned `turns_used=0`. `/new` did not recover it. (Patched by
  `BudgetHandle::reset()` called per turn in `handler.rs::run_agent` — commit
  `367eaa79`. This proposal removes that band-aid.)
- **Same latent latch in `run_chat` (CLI interactive) and the TUI event loop** —
  both are long-lived multi-turn and reuse one handle.
- **Web UI** never installs the budget on its top-level loop at all; only its
  subagents share a (never-reset) handle.
- **Config drift:** the gateway sizes the budget from `config.agent.max_iterations`
  while the TUI uses `config.agent.max_turns` — two knobs for one concept.

With more channels coming, this pattern multiplies. The same critique applies to
the **tool registry** and **skills**, which are likewise assembled per channel.

## 2. Current topology (facts)

- `AgentLoop::new(client, registry, max_iterations)` then `.with_*` builders,
  then `.run(messages) -> Result<AgentResult>`.
- The budget must be **shared parent↔child** (PROV-10): a runaway delegation
  tree is bounded by one counter. `AgentSubagentRunner` holds a `budget` field
  and clones it into each child loop (`subagent_runner.rs:283`).
- `SubagentRunner::run_child` (`delegate_task.rs:114`) does **not** take a
  budget — children read the runner's field. So the shared handle must be owned
  by something that outlives a single `AgentLoop` but is scoped to one agent
  unit. Today that "something" is the channel (process-lifetime) → the latch.
- `build_app_runtime_bundle` (`app_runtime_factory.rs`) already assembles a
  `ToolRegistry` (+ memory tool, delegate_task tool, cron, browser, MCP),
  `SkillRegistry`, `active_skills`, `browser_session`, `job_store`. It does
  **not** own the budget, the client/resolver, fallback, or a run API. The web
  UI uses it; the gateway does not (it holds those pieces on
  `GatewayMessageHandler` directly).

## 3. Proposed API surface

Introduce `AgentRuntime` in `ironhermes-agent`: the single unit that owns the
durable agent resources and exposes one turn API. Channels build it once and
call `run_turn`.

```rust
/// Durable, channel-agnostic agent unit. One per logical agent (per gateway
/// process, per web server, per TUI session). Owns the budget, tool registry,
/// subagent runner, skills, and the model client; channels supply only per-turn
/// data (messages, callbacks, cancel token).
pub struct AgentRuntime {
    client: AnyClient,
    fallback: Option<AnyClient>,
    registry: Arc<RwLock<ToolRegistry>>,
    budget: BudgetHandle,            // shared with the subagent runner's Arc
    max_iterations: usize,           // the single turn cap (see §6 config)
    active_skills: Arc<Mutex<Vec<SkillRecord>>>,
    memory_manager: Option<SharedMemoryManager>,
    hook_registry: Arc<HookRegistry>,
    browser_session: Arc<tokio::sync::Mutex<Option<BrowserSession>>>,
    // … any other durable wiring currently re-applied per turn …
}

/// Everything that legitimately varies turn-to-turn. Built by the channel.
pub struct TurnRequest {
    pub messages: Vec<ChatMessage>,
    pub session_id: Option<String>,
    pub compression_count: usize,
    pub cancel_token: Option<CancellationToken>,
    pub stream: Option<StreamCallback>,
    pub tool_progress: Option<ToolProgressCallback>,
    pub tool_result: Option<ToolResultCallback>,
}

impl AgentRuntime {
    /// Construct from config — absorbs build_app_runtime_bundle and additionally
    /// owns the budget (sized once from config) + client/fallback. The subagent
    /// runner registered into `registry` is given a clone of `self.budget` so
    /// parent and children share the counter.
    pub async fn from_config(input: AgentRuntimeInput) -> anyhow::Result<Self>;

    /// One top-level agent turn. THIS is the run boundary that owns budget
    /// lifecycle: reset the shared budget to full, build the AgentLoop with all
    /// durable + per-turn wiring, run it, return the result. Subagents spawned
    /// during the turn share the just-reset budget via the runner's Arc.
    pub async fn run_turn(&self, req: TurnRequest) -> anyhow::Result<AgentResult>;

    // Read-only accessors channels still need (status surface, /agents, etc.):
    pub fn budget(&self) -> &BudgetHandle;
    pub fn registry(&self) -> &Arc<RwLock<ToolRegistry>>;
    pub fn active_skills(&self) -> &Arc<Mutex<Vec<SkillRecord>>>;
}
```

### Budget mechanics (preserves parent↔child sharing)

- `AgentRuntime` holds the `BudgetHandle`; the subagent runner holds a clone of
  the **same `Arc`** (set at `from_config`).
- `run_turn` calls `self.budget.reset()` **before** building the top-level
  `AgentLoop`. The loop and any subagents it spawns therefore start full and
  share one counter for that turn.
- `AgentLoop` stays oblivious to parent/child lifecycle — it just consumes the
  handle it's given. No `as_subagent` flag needed; the runtime resets, not the
  loop.
- Sequential dispatch assumption is removed *as a correctness requirement*: if a
  future channel runs concurrent top-level turns, the right move is one
  `AgentRuntime` (and thus one budget) **per concurrent session**, which falls
  out naturally from "construct one runtime per logical agent." (Documented as a
  constraint; not solved by a shared global counter.)

## 4. How skills + tool registry generalize (the pattern)

This is the first instance of "durable agent resources live with the agent;
channels are thin clients." The same shape extends:

- **Tool registry:** already inside `AgentRuntime`; channels stop holding their
  own `Arc<RwLock<ToolRegistry>>` and go through runtime accessors / `run_turn`.
- **Skills:** `active_skills` + `SkillRegistry` move under `AgentRuntime`;
  `/skills activate` mutates runtime-owned state, so every channel (and every
  turn) sees the same active set. (This also touches the
  `Tool not found: website-to-hyperframes` issue — activation and tool exposure
  would have one owner.)
- **Future channels** implement only: build a `TurnRequest`, call `run_turn`,
  render `AgentResult`. No agent-assembly code.

`build_app_runtime_bundle` becomes an internal detail of
`AgentRuntime::from_config` (or is folded into it), not a per-channel call site.

## 5. Migration staging

1. **Introduce `AgentRuntime`** (budget + runner + bundle pieces + client),
   `run_turn`, unit test: "second turn after a budget-exhausting first turn
   starts fresh" (the regression that started this).
2. **Gateway → `run_turn`**: delete its `BudgetHandle::new`, its `.with_budget`,
   and the `handler.rs::run_agent` `reset()` band-aid.
3. **Web / `run_chat` / TUI → `run_turn`**: delete their `BudgetHandle::new` +
   `.with_budget`; fixes the latent `run_chat`/TUI latches and the web top-level
   gap. Reconcile `max_turns` vs `max_iterations` here.
4. **(Separate PR) skills + tool-registry ownership** fully into `AgentRuntime`
   as in §4.

Each stage is independently shippable and leaves the tree green.

## 6. Resolved decisions (2026-05-21)

1. **Single turn cap.** Collapse `max_iterations` and `max_turns` into one config
   knob that sizes both the loop turn cap and the budget. Keep `max_iterations`
   as the canonical field; treat `max_turns` as a deprecated alias mapped onto it
   (warn on use) so existing configs keep working. The TUI and gateway then read
   the same value — no more drift.
2. **`AgentRuntime` lives in `ironhermes-agent`** (next to `AgentLoop`) and
   absorbs `app_runtime_factory` (`build_app_runtime_bundle` becomes an internal
   detail of `AgentRuntime::from_config`).
3. **Gateway delegates.** `GatewayMessageHandler` stays as the *platform adapter*
   (slash commands, attachments, Telegram specifics) but hands the actual agent
   turn to `AgentRuntime::run_turn` instead of assembling an `AgentLoop` itself.
4. **Cron is a distinct unit.** The cron tick gets its own runtime/budget so
   scheduled turns don't compete with or drain the interactive chat budget.
   *Shipped at the top-level turn boundary (28.1-06).* **Resolved (T-28.1-16,
   Phase 35):** the subagent-layer contamination vector is closed by the global
   per-subagent independent-budget model — see §8.
5. **All four channels in one pass.** Stages 1–3 below (gateway, web, `run_chat`,
   TUI) land together; stage 4 (skills/tool-registry ownership) is a follow-up.

## 7. Implementation notes (from the decisions)

- **Config:** `config.agent.max_iterations` is canonical. If `max_turns` is
  present in a loaded config, map it onto `max_iterations` and `warn!` once.
  Audit all readers of both fields and point them at the single value.
- **One runtime per logical agent:** gateway = one interactive `AgentRuntime` +
  one separate cron `AgentRuntime`; web = one per server; CLI `run_chat`/TUI =
  one per session; CLI `run_single` = one per process (per-process == per-run,
  so it was never buggy, but it uses the same API for consistency).
- **Band-aid removal:** delete `handler.rs::run_agent`'s per-turn
  `BudgetHandle::reset()` (commit `367eaa79`) once `run_turn` owns the reset.
  Keep `BudgetHandle::reset()` itself — `run_turn` calls it.

## 8. Resolved — T-28.1-16: global per-subagent independent-budget model (Phase 35)

**Status:** resolved in Phase 35 (plans 35-02 / 35-03).

### What was the gap (historical context)

Before Phase 35, `AgentSubagentRunner::run_child` gave each child loop a
**clone** of the runner's `budget` field — the PROV-10 shared parent↔child
`Arc<AtomicUsize>`. This meant:

- Every subagent (interactive or cron) decremented the **same** counter as its
  parent.
- A cron job delegating via `delegate_task` reached the shared `ToolRegistry`
  delegate runner (built by the interactive `AgentRuntime`), so cron-spawned
  subagents charged iterations against the **interactive** budget — the
  cross-contamination T-28.1-16 identified.

The cron-specific "give cron its own AgentRuntime / own delegate runner" fix
sketched in earlier drafts of this section is **superseded** and was never
implemented. The chosen solution is global (see below).

### The fix: global per-subagent independent budgets (D-01 / D-04)

`AgentSubagentRunner::run_child` now constructs a **fresh**
`BudgetHandle::new(max_iterations)` for every child loop, replacing the
`budget.clone()` that shared the parent's `Arc<AtomicUsize>`. This applies to
**all** subagents — interactive and cron alike — matching the hermes-agent
reference implementation (each subagent has its own budget; total parent+child
iterations can exceed any single agent's cap).

**PROV-10 retired (D-04).** The shared parent↔child counter invariant is no
longer in effect. `BudgetHandle::clone` still shares one `Arc` and is still used
for gateway/CommandContext reset visibility; it is simply no longer used for
parent↔child subagent wiring.

### Clamp-to-ceiling policy (D-03)

`config.delegation.max_iterations` (default 50) is a hard **ceiling** on the
per-child budget. The `delegate_task` tool's `execute` / `execute_batch` paths
resolve an `effective_max_iterations` value:

- A caller/model-supplied per-call `max_iterations` that is **≤ ceiling** is
  honored verbatim.
- A value **exceeding** the ceiling is clamped down to the ceiling and a
  `tracing::warn!` records the clamp.

This preserves the Phase 32.2 per-call override feature for *shrinking* budgets
while capping DoS exposure from runaway delegation.

### DoS containment bound (D-05)

PROV-10's shared counter provided a single tree-wide backstop. Its retirement
shifts DoS containment to the **reference-style structural bound**:

> **Maximum tree-wide iterations = `max_spawn_depth` × `max_concurrent_children` × `max_iterations`**
>
> = 1 × 3 × 50 = **150** (default config values).

Both `max_spawn_depth` (default 1, `config.rs:994-997`) and
`max_concurrent_children` (default 3, `config.rs:981`) guards were enforced in
Phase 32.2 and remain in effect. No separate aggregate tree-wide budget backstop
is added; the depth × concurrency × per-subagent product is the security
boundary (documented here per D-05 — DEFCON-relevant).

### Behavioral change (D-02)

A parent's budget is **no longer decremented by its children**. For interactive
chat, a parent that spawns subagents via `delegate_task` will see its own
remaining counter unaffected by child iteration consumption. This matches the
hermes-agent reference and is the deliberate, user-chosen wider scope of Phase 35.

### Verification (D-07)

Three regression tests cover the resolved gap:

1. **D-07.1** — `test_independent_budget_child_drain_does_not_affect_parent`
   (`crates/ironhermes-agent/src/agent_loop.rs`): a child draining its budget to
   exhaustion leaves the parent budget unchanged.
2. **D-07.2** — `cron_subagent_budget_independence_from_interactive`
   (`crates/ironhermes-cron-runner/src/runner.rs`): a cron-spawned subagent
   draining its per-child budget to exhaustion leaves the interactive budget at
   full headroom (the original T-28.1-16 acceptance criterion at the subagent
   layer). The shared `ToolRegistry` delegate runner is confirmed not to be a
   contamination vector.
3. **D-07.3** — clamp-to-ceiling test (`crates/ironhermes-tools/src/delegate_task.rs`):
   a caller-supplied `max_iterations` exceeding the ceiling is clamped; a value at
   or below the ceiling is honored verbatim.
