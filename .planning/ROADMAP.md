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

### Phase 34b: Context-system parity (@-references + ContextEngine lifecycle + Compressor reset)

**Goal:** Close the parity gap with three hermes-agent context-system modules, wired into the post-28.1 `AgentRuntime::run_turn` chokepoint. (1) `@`-reference expansion (`context_references.py`): users write `@file:/@folder:/@diff/@staged/@git:N/@url:` in chat; tokens are parsed, expanded into a bounded `--- Attached Context ---` footer, and stripped from the inline message — preprocessed ONCE centrally in `run_turn` (D-09/D-11) with a sensitive-path blocklist (.ssh/.aws/.env/etc.) and a 50% hard / 25% soft token budget; expansion warnings ride back on `AgentResult.context_warnings` so all three surfaces render the `--- Context Warnings ---` block. (2) `ContextEngine` lifecycle hook parity (`context_engine.py`): 5 additive default-no-op hooks (`on_session_start`, `on_session_reset`, `update_from_response`, `update_model`, `has_content_to_compress`); per-turn hooks fire once centrally in `run_turn`, per-session reset stays at the surfaces. (3) `ContextCompressor` counter reset on `/new` + memory-authority reminder ("MEMORY.md … ALWAYS authoritative") in the compaction header. D-10 resolved via the existing `compression_count` state-threading precedent (surface-owned durable counter; engine rebuilt fresh per turn).
**Requirements**: CTX-REF-W0, CTX-ENG-W0, CTX-REF-01, CTX-REF-02, CTX-ENG-01, CTX-ENG-02, CTX-ENG-03, CTX-ENG-04 (phase-local; defined during /gsd:discuss-phase 34b)
**Depends on:** Phase 34a (read-side memory parity), Phase 28.1 (AgentRuntime run_turn chokepoint)
**Plans:** 4/4 plans complete

Plans:
**Wave 0**

- [x] 34B-00-PLAN.md — Test scaffolds: context_refs module stub, invariants_34b, #[ignore] reset + memory-authority placeholders

**Wave 1** *(depends on Wave 0)*

- [x] 34B-01-PLAN.md — @-reference expansion module (parser + expander + sensitive-path blocklist + 50%/25% budget) + central run_turn preprocessing + AgentResult.context_warnings carrier (D-09/D-11)

**Wave 2** *(depends on Wave 1)*

- [x] 34B-02-PLAN.md — ContextEngine 5 lifecycle hooks + ContextCompressor reset + memory-authority reminder + central per-turn hook in run_turn + surface session-reset wiring (D-09/D-10)

**Wave 3** *(gap closure — WR-01)*

- [x] 34b-03-PLAN.md — Close WR-01: stop in-message `--- Context Warnings ---` embedding, wire CLI/gateway/web to render `AgentResult.context_warnings` out-of-band, correct doc comments, source-guard test

### Phase 35: Per-subagent independent iteration budgets (retire PROV-10; T-28.1-16)

**Goal:** Replace IronHermes' PROV-10 shared parent↔child budget with **per-subagent independent iteration budgets**, matching the hermes-agent reference. Each subagent (interactive and cron) is given a fresh `BudgetHandle::new(delegation.max_iterations)` (already default 50) in `AgentSubagentRunner` instead of a clone of the parent's budget Arc, so a child can no longer decrement its parent's counter. Runaway delegation is bounded by `max_spawn_depth × max_concurrent_children × delegation.max_iterations` rather than one shared counter; the threat model and PROV-10 regression tests are updated accordingly. T-28.1-16 (cron subagents draining the interactive budget via the shared `ToolRegistry` delegate runner) is resolved as a consequence — with no shared parent/child counter, cron fan-out cannot touch interactive headroom.
**Requirements**: T-28.1-16 (from Phase 28.1). NOTE: §8's cron-specific fix is superseded by the global per-subagent model — see 35-CONTEXT.md. Gap described in docs/AGENT-RUNTIME-DESIGN.md §6.4 / §8.
**Depends on:** Phase 28.1 (AgentRuntime channel migration — cron distinct top-level budget shipped in 28.1-06)
**Plans:** 4 plans (3 complete + 1 gap-closure pending)

Plans:
**Wave 1**

- [x] 35-01-PLAN.md — Clamp delegate_task max_iterations to the config ceiling (D-03 Option B) + rewrite override test
- [x] 35-02-PLAN.md — Fresh per-child BudgetHandle at the runner change site; retire PROV-10 parent↔child counter; D-07.1 independence test

**Wave 2** *(blocked on Wave 1 completion)*

- [x] 35-03-PLAN.md — Subagent-layer cron independence test (T-28.1-16 acceptance); amend AGENT-RUNTIME-DESIGN.md §6.4/§8 + threat model

### Phase 35.1: hermes-agent install and setup parity (INSERTED)

**Goal:** Make `ironhermes setup` result in a fully working agent — matching the hermes-agent end-to-end setup experience. Close the six actual gaps in the existing wizard/preflight code (CFG-02 and CFG-03 already shipped in `config_cli.rs` and are NOT re-implemented): (1) replace the `bail!("section deferred to Phase 28")` at `setup.rs:122` with a real `run_skills_section` mirroring `run_tools_section` (D-01); (2) add a `run_terminal_section` that prompts for `cwd` only (D-02); (3) add a "Quick vs Full" choice prompt at wizard entry (D-11) so the fast path is the default; (4) call `doctor::run_doctor_check()` automatically at wizard exit as the final preflight gate (D-03), extracting `cmd_doctor` from `main.rs` into a new `src/doctor.rs` module; (5) print a completion summary with configured provider, model, enabled platforms, and a next-step hint (D-12); (6) add D-07/D-08 first-run LLM detection to `preflight.rs` — auto-launch wizard when no API key (OPENROUTER/ANTHROPIC/OPENAI) is set AND no localhost/127.0.0.1 base_url is configured, with `l.len() > key.len()` guard against empty-value bypass (T-35.1-01). The Phase 23 preflight outer gate condition (`Chat | Gateway | None`) stays byte-for-byte LOCKED — D-08 is added inside the existing valid-config branch only.
**Requirements**: CFG-01 (active wizard work), CFG-02 (already satisfied in `config_cli.rs::cmd_config_set/get/show`), CFG-03 (already satisfied in `config_cli.rs::cmd_config_migrate`)
**Depends on:** Phase 35
**Plans:** 5/6 plans executed

Plans:
**Wave 0**

- [x] 35.1-00-PLAN.md — Extract `cmd_doctor` to `src/doctor.rs` module + create Wave 0 test scaffolds (`tests/setup_wizard.rs` with 6 #[ignore] stubs; extend `tests/doctor_integration.rs` with d07_d08 stub)

**Wave 1** *(depends on Wave 0)*

- [x] 35.1-01-PLAN.md — Implement `run_skills_section` (D-01) + `run_terminal_section` (D-02) + `apply_skills_prereq_answers` testability seam; replace the bail line at setup.rs:122; un-ignore D-01 and D-02 tests

**Wave 2** *(parallel: 02 and 03 have zero `files_modified` overlap — 02 touches setup.rs + tests/setup_wizard.rs; 03 touches preflight.rs + tests/doctor_integration.rs)*

- [x] 35.1-02-PLAN.md — Wire D-11 fast/full choice + D-03 in-process doctor call + D-12 completion summary into `run_setup` None arm; un-ignore d11/d03/d12 source-text invariant tests
- [x] 35.1-03-PLAN.md — Implement `has_runnable_llm` helper (env-vars → raw .env → local base_url ordering) + integrate into `preflight.rs` Ok(config) arm (D-07/D-08); un-ignore d07_d08 integration tests; verify main.rs Phase 23 gate stays byte-for-byte unchanged
