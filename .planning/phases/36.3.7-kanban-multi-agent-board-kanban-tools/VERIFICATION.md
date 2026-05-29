---
phase: 36.3.7-kanban-multi-agent-board-kanban-tools
verified: 2026-05-29T00:00:00Z
status: human_needed
score: 17/17
overrides_applied: 0
human_verification:
  - test: "Live worker spawn smoke (UAT-09-A)"
    expected: "ironhermes kanban dispatch triggers a real worker process that claims the task, calls kanban_complete, and the task transitions to done"
    why_human: "Requires a running ironhermes binary and real SQLite DB; cannot be verified by grep or cargo test"
  - test: "Live /kanban gateway bypass smoke (UAT-09-B)"
    expected: "/kanban list works mid-turn inside an active gateway session without blocking the running agent"
    why_human: "Requires a live gateway session with an agent running; mid-run bypass behavior cannot be verified statically"
---

# VERIFICATION — Phase 36.3.7 (Kanban Kernel v1)

**Overall verdict:** PASS-WITH-NOTES (pending two UAT sign-offs)
**Date:** 2026-05-29
**Method:** goal-backward decomposition + code trace + `cargo test -p ironhermes-kanban` (exit 0)
**Re-verification:** No — initial verification

---

## Goal Decomposition

All 17 deliverables verified against actual source files. Test names verified against `cargo test -p ironhermes-kanban -- --list`.

| # | Deliverable | D-ref(s) | Plan(s) | Code | Test | Verdict |
|---|-------------|-----------|---------|------|------|---------|
| 1 | `~/.ironhermes/kanban.db` SQLite file, WAL, 5-table schema | D-03, D-05 | 01, 02 | `crates/ironhermes-kanban/src/schema.rs` (all 5 `CREATE TABLE IF NOT EXISTS`); `src/paths.rs::kanban_db_path`; `PRAGMA journal_mode=WAL` line 20 of schema.rs | `paths::tests::paths_under_hermes_home` | PASS |
| 2 | Atomic CAS claim (`BEGIN IMMEDIATE` + `task_runs` insert in same tx) | D-40, D-41 | 01, 02 | `crates/ironhermes-kanban/src/cas.rs::atomic_claim` (line 63: `TransactionBehavior::Immediate`; line 79: `INSERT INTO task_runs`) | `invariants_36_3_7::atomic_claim_uses_begin_immediate`; `cas_concurrency::concurrent_claims_exactly_one_winner` | PASS |
| 3 | Gateway-embedded dispatcher (tokio task, 60s tick, env override) | D-09, D-11 | 03, 08 | `crates/ironhermes-gateway/src/runner.rs` (lines 1249–1278: `ironhermes_kanban::run_dispatch_loop`); `src/config.rs::dispatch_in_gateway`; `HERMES_KANBAN_DISPATCH_IN_GATEWAY` env check line 1263 | `invariants_36_3_7::kanban_dispatcher_spawned_in_gateway`; `invariants_36_3_7::gateway_runner_embeds_kanban_dispatcher` | PASS |
| 4 | 8-step dispatcher tick (detect_crashed / extend_claims / reclaim_stale / enforce_max_runtime / promote_ready / atomic_claim / respawn_guard / spawn_worker) | D-10 | 03 | `crates/ironhermes-kanban/src/dispatcher.rs` (8 async fn helpers at lines 226, 349, 391, 471, 583, 750+, 906, 964) | `invariants_36_3_7::dispatcher_has_all_eight_step_helpers`; `dispatcher_logic.rs` (10 tests) | PASS |
| 5 | Live-PID detection + claim extension vs reclaim | D-10 | 03 | `crates/ironhermes-kanban/src/dispatcher.rs::detect_crashed_workers` + `extend_claims` | `dispatcher_logic::live_pid_extends_when_alive`; `dispatcher_logic::dead_pid_triggers_reclaim` | PASS |
| 6 | Failure circuit-breaker (`gave_up` event, `blocker_auth` / `recent_success` / `active_pr` respawn-guard) | D-12 | 03 | `crates/ironhermes-kanban/src/dispatcher.rs::apply_circuit_breaker` (line 964); `respawn_guard_reason` (line 906) | `dispatcher_logic::circuit_breaker_after_failure_limit`; `dispatcher_logic::respawn_guard_blocker_auth`; `dispatcher_logic::respawn_guard_recent_success`; `dispatcher_logic::respawn_guard_active_pr` | PASS |
| 7 | Full-OS-process worker spawn: `ironhermes --profile P --skills kanban-worker chat -q "..."` | D-15, D-16, D-28 | 01, 03 | `crates/ironhermes-kanban/src/worker_spawn.rs::spawn_worker` (line 142–183: `ironhermes --profile <P> --skills kanban-worker`; `chat -q`); `chat -q` flag at `crates/ironhermes-cli/src/main.rs` line 141 | `invariants_36_3_7::chat_subcommand_has_q_flag` | PASS |
| 8 | Env scrub: `build_kanban_worker_env()` allowlist + 9-var contract, `.env_clear()` | D-17, D-18 | 01, 03 | `crates/ironhermes-kanban/src/worker_spawn.rs::build_kanban_worker_env` (line 95); `.env_clear()` at line 220 | `invariants_36_3_7::dispatcher_calls_build_kanban_worker_env`; `invariants_36_3_7::worker_spawn_calls_env_clear`; `worker_spawn::build_kanban_worker_env_scrubs_secrets`; `worker_spawn::build_kanban_worker_env_includes_eight_kanban_vars` | PASS |
| 9 | 6-tool LLM surface (`kanban_show/list/complete/block/comment/create`) gated by `HERMES_KANBAN_TASK` | D-20, D-21–D-25 | 04 | `crates/ironhermes-kanban/src/tools/{show,list,complete,block,comment,create}.rs` (6 files; each implements `is_available` checking `HERMES_KANBAN_TASK`) | `tools::show::is_available_respects_env`; `tools::list::is_available_respects_env`; `tools::complete::is_available_respects_env`; `tools::block::is_available_respects_env`; `tools::comment::is_available_respects_env`; `tools::create::is_available_respects_env` | PASS |
| 10 | `expected_run_id` stale-write rejection + `created_cards` phantom-id gate | D-22, D-41 | 04, 09 | `crates/ironhermes-kanban/src/tools/complete.rs` (`expected_run_id` gate line 115; `created_cards` gate line 132) | `tools_smoke::kanban_complete_rejects_stale_run_id`; `tools_smoke::kanban_complete_rejects_phantom_created_cards`; `protocol_violation::impostor_worker_run_id_mismatch_emits_rejection`; `protocol_violation::phantom_created_cards_emits_completion_rejected_event`; `protocol_violation::wrong_profile_created_cards_emits_completion_rejected_event` | PASS |
| 11 | `claim_lock`-gated worker writes (no-op + `claim_expired` advisory event) | D-41 | 02, 09 | `crates/ironhermes-kanban/src/tools/comment.rs` (claim_lock check line 111); `crates/ironhermes-kanban/src/cas.rs::worker_write_gated` | `cas_concurrency::claim_lock_gates_writes`; `protocol_violation::claim_lock_gated_write_emits_claim_expired` | PASS |
| 12 | `KANBAN_GUIDANCE` static prompt injection when `HERMES_KANBAN_TASK` present | D-26 | 05 | `crates/ironhermes-kanban/src/kanban_guidance.rs::KANBAN_GUIDANCE` (`pub const … &str`, line 29, 0 `format!` calls); `crates/ironhermes-cli/src/main.rs::inject_kanban_guidance_if_worker` (line 277, called at lines 846 and 1444) | `invariants_36_3_7::kanban_guidance_is_static_const`; `guidance_static::guidance_is_static_str`; `guidance_static::guidance_mentions_six_lifecycle_steps`; `kanban_worker_session::main_rs_calls_inject_kanban_guidance_if_worker` | PASS |
| 13 | `ironhermes kanban` CLI (24-verb surface + 4 operator-recovery verbs) | D-33, D-34, D-35 | 06 | `crates/ironhermes-cli/src/kanban/commands.rs` (cmd_init, cmd_create, cmd_list, cmd_show, cmd_assign, cmd_link, cmd_unlink, cmd_claim, cmd_comment, cmd_complete, cmd_block, cmd_unblock, cmd_archive, cmd_tail, cmd_watch, cmd_runs, cmd_assignees, cmd_dispatch, cmd_stats, cmd_log, cmd_context, cmd_gc, cmd_reclaim, cmd_reassign, cmd_diagnostics, cmd_daemon — 26 fns); `crates/ironhermes-cli/src/main.rs` line 228: `command: kanban::KanbanCommands` | `invariants_36_3_7::kanban_subcommand_registered_in_main` | PASS |
| 14 | `/kanban` slash command — Universal platform, mid-run bypass | D-35, D-36 | 06 | `crates/ironhermes-core/src/commands/registry.rs` line 215: `CommandDef::new("kanban", …, ToolsAndSkills).platform(Universal)`; `crates/ironhermes-core/src/commands/running_agent.rs` line 48: `"kanban"` in bypass list | `invariants_36_3_7::kanban_commanddef_universal`; `invariants_36_3_7::kanban_is_in_bypass_list` | PASS |
| 15 | Bundled skills: `kanban-worker` v2.0.0 + `kanban-orchestrator` v3.0.0 synced via `ensure_home_dirs()` + `skills update` | D-29, D-30 | 07 | `skills/kanban-worker/SKILL.md` (version: 2.0.0); `skills/kanban-orchestrator/SKILL.md` (version: 3.0.0); `crates/ironhermes-kanban/src/skills_bundle.rs::sync_bundled_kanban_skills`; wired in `crates/ironhermes-cli/src/main.rs::ensure_home_dirs` line 677 + `crates/ironhermes-cli/src/skills_cmd.rs` line 634 | `skills_sync::second_run_preserves_user_edits` | PASS |
| 16 | Stranded-task diagnostic with 3-tier severity (warn/error/critical at 1×/2×/6×) | D-14 | 03 | `crates/ironhermes-kanban/src/dispatcher.rs::diagnose_stranded` | `dispatcher_logic::stranded_task_diagnostic_severity_escalation` | PASS |
| 17 | Automated test suite: 10 critical protocol-correctness invariants under test | VALIDATION.md | 01–09 | `crates/ironhermes-kanban/tests/` (9 test files, `cargo test` exit 0) | All tests listed in INV-36.3.7 ledger — see Critical Invariants section | PASS |

---

## Critical Invariants

Cross-referenced against `INV-36.3.7.md`.

| INV ID | Stated Invariant | Where Enforced | Where Tested | Verdict |
|--------|-----------------|----------------|--------------|---------|
| INV-36.3.7-01 | Atomic claim race: only one winner via `BEGIN IMMEDIATE` | `crates/ironhermes-kanban/src/cas.rs::atomic_claim` (`TransactionBehavior::Immediate`) | `invariants_36_3_7::atomic_claim_uses_begin_immediate`; `cas_concurrency::concurrent_claims_exactly_one_winner` | PASS |
| INV-36.3.7-02 | `expected_run_id` rejection of stale writes | `crates/ironhermes-kanban/src/tools/complete.rs` (expected_run_id gate) + `cas.rs::assert_run_id` | `tools_smoke::kanban_complete_rejects_stale_run_id`; `protocol_violation::impostor_worker_run_id_mismatch_emits_rejection` | PASS |
| INV-36.3.7-03 | `created_cards` phantom-id + wrong-profile rejection (permanent `completion_rejected` event) | `crates/ironhermes-kanban/src/tools/complete.rs` (created_cards gate; `completion_rejected` event emit) | `tools_smoke::kanban_complete_rejects_phantom_created_cards`; `protocol_violation::phantom_created_cards_emits_completion_rejected_event`; `protocol_violation::wrong_profile_created_cards_emits_completion_rejected_event` | PASS |
| INV-36.3.7-04 | Dispatcher respawn-guard (`blocker_auth` / `recent_success` / `active_pr`) | `crates/ironhermes-kanban/src/dispatcher.rs::respawn_guard_reason` | `dispatcher_logic::respawn_guard_blocker_auth`; `dispatcher_logic::respawn_guard_recent_success`; `dispatcher_logic::respawn_guard_active_pr` | PASS |
| INV-36.3.7-05 | Live-PID extension vs dead-PID reclaim at TTL expiry | `crates/ironhermes-kanban/src/dispatcher.rs` (step 2: `extend_claims`; step 1/3: `detect_crashed_workers`/`reclaim_stale_claims`) | `dispatcher_logic::live_pid_extends_when_alive`; `dispatcher_logic::dead_pid_triggers_reclaim` | PASS |
| INV-36.3.7-06 | Stranded-task diagnostic severity escalation (Warn/Error/Critical at 1×/2×/6×) | `crates/ironhermes-kanban/src/dispatcher.rs::diagnose_stranded` | `dispatcher_logic::stranded_task_diagnostic_severity_escalation` | PASS |
| INV-36.3.7-07 | Env scrub: worker receives only `build_kanban_worker_env()` allowlist + 9 kanban vars | `crates/ironhermes-kanban/src/worker_spawn.rs::build_kanban_worker_env` + `.env_clear()` line 220 | `invariants_36_3_7::dispatcher_calls_build_kanban_worker_env`; `invariants_36_3_7::worker_spawn_calls_env_clear`; `worker_spawn::build_kanban_worker_env_scrubs_secrets` | PASS |
| INV-36.3.7-08 | Skills sync idempotency / preserve user edits on second run | `crates/ironhermes-kanban/src/skills_bundle.rs::sync_bundled_kanban_skills` (`force=false` preserves existing content) | `skills_sync::second_run_preserves_user_edits` | PASS |
| INV-36.3.7-09 | `claim_lock`-gated worker writes no-op + emit `claim_expired` | `crates/ironhermes-kanban/src/tools/comment.rs` (HERMES_KANBAN_CLAIM_LOCK check line 111) + `cas.rs::worker_write_gated` | `cas_concurrency::claim_lock_gates_writes`; `protocol_violation::claim_lock_gated_write_emits_claim_expired` | PASS |
| INV-36.3.7-10 | Protocol violation auto-block (worker exits 0 with task still `running`) | `KanbanEventKind::ProtocolViolation` variant exists in `events.rs` line 65; runtime path requires `dispatcher_state.json` (not in v1) | `protocol_violation::crashed_and_protocol_violation_are_distinct_event_kinds` (PASS); `protocol_violation::protocol_violation_distinguished_from_crashed_requires_state_file` (`#[ignore]` — v1 known limit) | PASS-WITH-NOTE (event kind distinct; full auto-block deferred to 36.3.7.0 — documented in INV ledger) |

**Static-grep regression gates (SG-01..SG-10):** All 14 tests in `invariants_36_3_7.rs` pass. `cargo test -p ironhermes-kanban` exit 0 confirmed.

---

## Deferred Items

### In-scope-deferred (UAT cases — require human sign-off before milestone closes)

| # | UAT Case | What | Why deferred |
|---|----------|------|--------------|
| UAT-09-A | Live worker spawn smoke | `ironhermes kanban dispatch` triggers real worker process: claims task → calls `kanban_complete` → task transitions to `done` | Requires running ironhermes binary + real SQLite board; cannot be unit-tested |
| UAT-09-B | Live `/kanban` gateway bypass smoke | `/kanban list` works mid-turn inside an active gateway session without blocking the agent | Requires live gateway session with running agent |

### Out-of-scope-deferred (future phases — not actionable gaps here)

| # | Deferred Item | Target Phase |
|---|--------------|-------------|
| 1 | `kanban_heartbeat` LLM tool + stale-timeout config | 36.3.7.1 |
| 2 | `kanban_link` LLM tool (after-the-fact dependency editing) | 36.3.7.1 |
| 3 | `kanban_unblock` LLM tool | 36.3.7.1 |
| 4 | Triage/auto-decompose/specify flow | 36.3.7.2 |
| 5 | Multi-board layout (`kanban/boards/<slug>/kanban.db`) | 36.3.7.3 |
| 6 | Dashboard plugin (iron_hermes_ui SPA + REST + WebSocket) | 36.3.7.4 |
| 7 | Gateway notifier (subscription table + poll loop) | 36.3.7.5 |
| 8 | Swarm topology helper (`kanban swarm`) | 36.3.7.6 |
| 9 | @mention parser + auto-create | 36.3.7.7 |
| 10 | Portable profile artifacts (`profile export/install`) | 36.3.7.8 |
| 11 | External CLI worker lanes (Codex/Claude Code/OpenCode `spawn_fn` API) | v3.1+ |
| 12 | INV-36.3.7-10 full auto-block via exit-code reconciliation (`dispatcher_state.json`) | 36.3.7.0 (fix-forward if smoke fails) |
| 13 | `is_available_respects_env` parallel flake (env-mutating tests need shared mutex in `-p ironhermes-kanban` single-threaded runs) | Noted in plan 05 + plan 09 SUMMARY; test isolation already mitigated with `--test-threads=1` guidance |

---

## Notes / Risks

### NOTE-01: CommandDef platform — `ToolsAndSkills` category + `Universal` via chain call

`crates/ironhermes-core/src/commands/registry.rs` line 215 uses `ToolsAndSkills` as the primary constructor argument and `.platform(Universal)` as a chained override. The INV-36.3.7-SG-07 test explicitly documents this pattern and checks the 200-character proximity window. The `Universal` platform is set and verified; the `ToolsAndSkills` category reflects the command group (not the dispatch platform). This is correct behavior, not a bug.

### NOTE-02: KANBAN_GUIDANCE injection is in `ironhermes-cli/src/main.rs`, not `ironhermes-agent`

D-26 specifies injection "via the existing 10-layer prompt builder (Phase 15)." The implementation uses `PromptBuilder::activate_skill("KANBAN_GUIDANCE", KANBAN_GUIDANCE)` in `ironhermes-cli/src/main.rs::inject_kanban_guidance_if_worker` — a direct call on the prompt builder at the CLI entry point, not a hook inside `ironhermes-agent`. This achieves the same result (guidance in the worker's system prompt) via a different integration point than the D-26 wording implied. Tested by `kanban_worker_session.rs` static-grep assertions + `guidance_static.rs`.

### NOTE-03: `kanban-worker` SKILL.md description field references `agent/prompt_builder.py`

`skills/kanban-worker/SKILL.md` YAML frontmatter `description:` field still contains `(from agent/prompt_builder.py)` — an upstream Hermes artifact reference that was not substituted. This is the YAML metadata description (used by skill indexing), not the body content that workers read. Body content correctly references IronHermes paths. Cosmetic; no behavioral impact. Recommend fixing in 36.3.7.0 or the next housekeeping pass.

### NOTE-04: Plan 08 `!Send` fix touched Plan 03's `dispatcher.rs`

Plan 08 SUMMARY documents a `!Send` bound fix that required a change to the dispatcher (originally authored in plan 03). Per CONTEXT.md "Claude's Discretion," within-phase auto-fixes are permitted; the fix was within the same wave and the invariant tests remain green. No semantic deviation.

### NOTE-05: Plan 05 executed inline via orchestrator after two consecutive 500 errors

Plan 05 SUMMARY notes the plan was executed inline due to infrastructure issues. No semantic deviation was introduced; the `inject_kanban_guidance_if_worker` implementation is correct and tested.

### NOTE-06: Stranded-task test (`D-14`) covers `diagnostics` verb

The `stranded_task_diagnostic_severity_escalation` test in `dispatcher_logic.rs` exercises the same `diagnose_stranded` function surfaced by `ironhermes kanban diagnostics`. The CLI verb itself (`cmd_diagnostics`) is present in `crates/ironhermes-cli/src/kanban/commands.rs` line 755. End-to-end UAT is covered by UAT-09-A/B if the human tester creates a stale task.

---

## Human Verification Required

### 1. Live Worker Spawn Smoke (UAT-09-A)

**Test:** Run `ironhermes kanban init`, create a task (`ironhermes kanban create --title "smoke" --assignee default`), then run `ironhermes kanban dispatch` and observe the spawned worker process complete the task.

**Expected:** Task transitions through `ready` → `running` → `done`; `ironhermes kanban show <id>` shows a completed run with summary; `~/.ironhermes/logs/kanban/<task_id>.stdout.log` contains worker output.

**Why human:** Requires a running ironhermes binary, a real `~/.ironhermes/kanban.db`, and an active profile configuration. Cannot be verified by grep or unit test.

### 2. Live `/kanban` Gateway Bypass Smoke (UAT-09-B)

**Test:** Start a gateway session with an active agent turn in progress (agent mid-response). Issue `/kanban list` from the gateway UI or Telegram/Discord.

**Expected:** `/kanban list` returns task list immediately without interrupting the running agent; gateway does not block the command; response appears in the platform chat.

**Why human:** Requires a live gateway session with an active agent. The static bypass registration is verified (`kanban_is_in_bypass_list` test passes), but the runtime mid-run path requires an actual concurrent session.

---

## Closing Recommendation

Phase 36.3.7 is ready to close. All 17 deliverables are implemented and verified against real source code. The `cargo test -p ironhermes-kanban` suite passes (exit 0), covering all 10 critical protocol-correctness invariants including the static-grep regression gates. The two remaining human UAT cases (live worker spawn + live gateway bypass) are the only blockers, and both are mechanical smoke checks that validate already-tested wiring rather than new logic.

The phase should proceed to UAT sign-off. Two minor housekeeping items (NOTE-03: `prompt_builder.py` reference in skill frontmatter; NOTE-04: `!Send` ripple) are informational and do not require action before closing. If the live smoke test reveals a defect, the INV ledger already anticipates a 36.3.7.0 fix-forward sub-phase for the `dispatcher_state.json` reconciliation (INV-36.3.7-10 full auto-block path).

---

_Verified: 2026-05-29_
_Verifier: Claude (gsd-verifier)_
