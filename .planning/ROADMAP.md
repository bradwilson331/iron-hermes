### Phase 28.1: AgentRuntime channel migration (budget/skills/tools ownership) per docs/AGENT-RUNTIME-DESIGN.md (INSERTED)

**Goal:** `AgentRuntime` is the single channel-facing agent API: every channel (Telegram gateway, web UI, CLI `run_chat`/`run_single`, TUI) builds one `AgentRuntime` and calls `run_turn(TurnRequest)` per top-level turn. No channel constructs `BudgetHandle`s or assembles `AgentLoop`s by hand; the run-boundary owns budget reset, permanently fixing the `Stop100` latch class for current and future channels. Cron gets a separate runtime/budget so scheduled turns do not drain interactive chat.
**Requirements**: AGENT-RUNTIME-MIGRATION (scope + locked decisions §6 in docs/AGENT-RUNTIME-DESIGN.md and 28.1-CONTEXT.md)
**Depends on:** Phase 28
**Plans:** 6/6 plans complete

Plans:
- [x] 28.1-01-PLAN.md — AgentRuntime budget-reset regression test (foundational proof; agent crate)
- [x] 28.1-02-PLAN.md — Gateway → run_turn; remove 367eaa79 band-aid (highest value)
- [x] 28.1-03-PLAN.md — Web UI → run_turn; close top-level-loop budget gap
- [x] 28.1-04-PLAN.md — CLI run_chat + run_single → run_turn; fix run_chat latch
- [x] 28.1-05-PLAN.md — TUI → run_turn; fix latch + max_turns/max_iterations drift
- [x] 28.1-06-PLAN.md — Cron distinct runtime/budget (§6.4); preserve per-job overrides

**Note:** Stage 4 (skills + tool-registry ownership fully into AgentRuntime, design §4) is intentionally DEFERRED to a follow-up phase — see planning summary. It would edit the same channel files this phase migrates and is independently shippable per §5.

### Phase 36: Gateway running-agent guard wiring — completes GW-05

**Goal:** Wire per-session running-agent state on the gateway so `/stop`, `/approve`, `/deny` bypass while `/model` and other state-mutating commands are queued during an active agent turn. Cross-AI review of Phase 21.1 (2026-05-24, codex HIGH-1) confirmed `crates/ironhermes-gateway/src/handler.rs:377-380` hardcodes `agent_running = AtomicBool::new(false)` with the comment "running-agent guard is a future enhancement using per-session state" — leaving GW-05 only partially satisfied.
**Requirements**: GW-05
**Depends on:** Phase 35
**Plans:** 3 plans

Plans:
**Wave 1**

- [x] 36-01-PLAN.md — Wave 0 test scaffold: running_agent_guard_tests.rs with 11 #[ignore] GW-05 sub-behavior stubs + helpers (RecordingPlatformAdapter, build_test_session_store, d02_error_message)

**Wave 2** *(depends on Wave 1)*

- [x] 36-02-PLAN.md — Core implementation: add running: Arc<AtomicBool> to GatewaySession (D-03/D-05) + SessionStore::get_running_flag accessor; add RunningAgentGuard RAII (D-06) and is_bypass (D-01) to handler.rs; wire guard at run_agent top + handle_slash_command + non-slash MessageHandler::handle AND handle_with_multimodal (Pitfall 1); un-ignore all 11 tests

**Wave 3** *(depends on Wave 2)*

- [ ] 36-03-PLAN.md — Cleanup: stale comment removal, REQUIREMENTS.md + ROADMAP.md updates, deferred-work backlog, UAT
