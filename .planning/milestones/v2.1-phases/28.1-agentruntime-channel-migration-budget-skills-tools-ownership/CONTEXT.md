# Phase 28.1 — AgentRuntime channel migration (CONTEXT)

**Source spec:** `docs/AGENT-RUNTIME-DESIGN.md` (authoritative — locked decisions + staging live there).

## Goal

Make `AgentRuntime` the single channel-facing agent API. Every channel
(Telegram gateway, web UI, CLI `run_chat`/`run_single`, TUI) builds one
`AgentRuntime` and calls `run_turn(TurnRequest)` per top-level turn. No channel
constructs `BudgetHandle`s, assembles `AgentLoop`s by hand, or manages budget
lifecycle. This removes four copies of the same wiring and makes the
run-boundary the single owner of budget reset — permanently fixing the
`Stop100` latch class of bug for current and future channels.

## Already shipped this session (DO NOT re-plan)

- `e4c2ca48` — config collapse: single canonical `config.agent.max_iterations`
  cap (default unified to `DEFAULT_MAX_ITERATIONS`=90); `max_turns` deprecated
  alias folded by `AgentConfig::normalize()`.
- `0f8807c6` — `crates/ironhermes-agent/src/agent_runtime.rs` built and exported:
  `AgentRuntime`, `AgentRuntimeInput`, `TurnRequest`. `from_config` creates the
  shared `BudgetHandle`, builds the subagent runner with a clone of it, and
  assembles the tool bundle (decision A). `run_turn` resets the budget, assembles
  the loop (budget/hooks/skills/browser/memory/fallback/compression +
  `attach_context_engine`), and runs. Compiles clean; not yet wired into any
  channel.
- `367eaa79` — gateway per-turn `BudgetHandle::reset()` band-aid (to be REMOVED
  by stage 1 once the gateway uses `run_turn`).

## Scope of THIS phase (remaining work)

1. **Gateway → `run_turn`** (design §5 stage 2): build `AgentRuntime` in
   `main.rs run_gateway`; source `GatewayMessageHandler`'s shared Arcs
   (`tool_registry`, `hook_registry`, `active_skills`, `browser_session`,
   `memory_manager`, `subagent_registry`, `process_registry`) FROM the runtime so
   slash/toolset/status paths and `run_turn` operate on the same instances;
   replace `run_agent`'s hand-assembly (handler.rs ~985-1082) with a
   `TurnRequest` + `runtime.run_turn`; remove the `budget_handle` field/setter and
   the `367eaa79` band-aid. **Preserve:** per-session trajectory writer, the
   `gw:<chat>:<sender>` session-id format, and the LEARN-01 post-turn nudge
   (these stay in the handler around the `run_turn` call).
2. **Web UI → `run_turn`** (`iron_hermes_ui/src/server/state.rs`,
   `run_web_turn`): replace the hand-assembled loop; pass `state_store` via
   `TurnRequest` for the session_search intercept; delete `BudgetHandle::new` +
   `.with_budget`. Fixes the latent top-level-loop budget gap.
3. **CLI `run_chat` + `run_single` → `run_turn`** (`crates/ironhermes-cli/src/main.rs`):
   delete their `BudgetHandle::new` + `.with_budget`; fixes the latent `run_chat`
   multi-turn latch.
4. **TUI → `run_turn`** (`crates/ironhermes-cli/src/tui_rata/event_loop.rs`):
   delete `BudgetHandle::new` + `.with_budget`; fixes the latent TUI latch.
5. **Cron distinct runtime** (design §6.4): the gateway cron tick + the
   `ironhermes-cron-runner` get their OWN `AgentRuntime`/budget so scheduled
   turns don't drain the interactive chat budget.
6. **(Stage 4) Skills + tool-registry ownership** (design §4): channels stop
   holding their own registry/active_skills Arcs and go through runtime
   accessors. May be split to a follow-up if it bloats the phase — planner's
   call.

## Out of scope

- Re-deriving the AgentRuntime API (built + decided in `0f8807c6`).
- Changing budget/PROV-10 semantics (parent↔child sharing is preserved as-is).
- The five unrelated gateway-log issues (skill-as-tool registration, `~` path
  expansion, vision 404, terminal timeout, browser profile lock) — separate work.

## Locked decisions (from design doc §6)

1. Single `max_iterations` cap (done).
2. `AgentRuntime` lives in `ironhermes-agent`, absorbs `app_runtime_factory` (done).
3. Gateway delegates: handler stays the platform adapter; turn goes to `run_turn`.
4. Cron is a distinct runtime/budget.
5. All four channels migrate in this phase.
6. Construction approach **A**: `from_config` owns runner+budget construction.

## Success criteria (what must be TRUE)

1. No channel calls `BudgetHandle::new` or `AgentLoop::with_budget` directly;
   `grep` for these in the four channel crates returns only `agent_runtime.rs`.
2. Each interactive channel: a second top-level turn after a budget-exhausting
   first turn starts with a full budget (no `Stop100` latch). Covered by a
   regression test at the `AgentRuntime`/budget level.
3. Gateway behavior preserved: trajectory writer, `gw:` session-id, LEARN-01
   nudge still fire; gateway unit tests green.
4. Web `run_web_turn`, CLI `run_chat`/`run_single`, TUI all produce the same
   observable turn behavior via `run_turn`.
5. Cron turns use a separate budget from interactive chat.
6. `cargo build --workspace` + `cargo clippy` clean of new warnings; existing
   workspace tests stay green (the 3 env-dependent browser tests excepted).
7. The `367eaa79` band-aid is removed; `BudgetHandle::reset()` remains (called by
   `run_turn`).

## Verification constraint

The Telegram gateway and the GUIs cannot be end-to-end tested in-session — only
`cargo build` + unit tests + clippy. Plans must lean on compile + unit coverage
and call out the live-test checklist for the operator.
