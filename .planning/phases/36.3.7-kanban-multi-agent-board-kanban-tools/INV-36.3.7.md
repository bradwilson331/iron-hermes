# INV-36.3.7 — Kanban Kernel Invariant Ledger

**Phase:** 36.3.7 — Kanban / multi-agent board (kanban_* tools)
**Created:** 2026-05-29
**Precedent:** Phase 22.4.2.2, 27.1.4.x INV ledgers

---

## Critical Invariants from VALIDATION.md

These 10 invariants are drawn verbatim from `36.3.7-VALIDATION.md § Critical Invariants`.
Each row maps the invariant to its source plan, test file, and specific test name.

| # | Invariant | Source plan | Test file | Test name |
|---|-----------|-------------|-----------|-----------|
| INV-36.3.7-01 | Atomic claim race: only one winner via `BEGIN IMMEDIATE` | 02 | `cas_concurrency.rs` | `concurrent_claims_exactly_one_winner` |
| INV-36.3.7-02 | `expected_run_id` rejection of stale writes (impostor worker) | 02, 04, 09 | `tools_smoke.rs` · `protocol_violation.rs` | `kanban_complete_rejects_stale_run_id` · `impostor_worker_run_id_mismatch_emits_rejection` |
| INV-36.3.7-03 | `created_cards` phantom-id and wrong-profile rejection (permanent `completion_rejected` event) | 02, 04, 09 | `tools_smoke.rs` · `protocol_violation.rs` | `kanban_complete_rejects_phantom_created_cards` · `phantom_created_cards_emits_completion_rejected_event` · `wrong_profile_created_cards_emits_completion_rejected_event` |
| INV-36.3.7-04 | Dispatcher respawn-guard correctness (`blocker_auth` / `recent_success` / `active_pr`) | 03 | `dispatcher_logic.rs` | `respawn_guard_blocker_auth` · `respawn_guard_recent_success` · `respawn_guard_active_pr` |
| INV-36.3.7-05 | Live-PID extension vs dead-PID reclaim at TTL expiry | 03 | `dispatcher_logic.rs` | `live_pid_extends_when_alive` · `dead_pid_triggers_reclaim` |
| INV-36.3.7-06 | Stranded-task diagnostic with severity escalation (Warn/Error/Critical at 1×/2×/6× threshold) | 03 | `dispatcher_logic.rs` | `stranded_task_diagnostic_severity_escalation` |
| INV-36.3.7-07 | Env scrub: worker subprocess receives only `build_safe_env()` allowlist + 9 kanban env vars | 03 | `invariants_36_3_7.rs` | `dispatcher_calls_build_kanban_worker_env` · `worker_spawn_calls_env_clear` |
| INV-36.3.7-08 | Skills sync idempotency / preserve user edits on second run | 07 | `skills_sync.rs` | `second_run_preserves_user_edits` |
| INV-36.3.7-09 | `claim_lock`-gated worker writes no-op + emit `claim_expired` advisory event | 02, 09 | `cas_concurrency.rs` · `protocol_violation.rs` | `claim_lock_gates_writes` · `claim_lock_gated_write_emits_claim_expired` |
| INV-36.3.7-10 | Protocol violation auto-block (worker exits 0 with task still `running`) | 03, 09 | `protocol_violation.rs` | **manual** — `protocol_violation_distinguished_from_crashed_requires_state_file` is `#[ignore]`'d; see DEFECT note below |

### INV-36.3.7-10 Phase Defect Notice

**Status:** ENFORCED-BUT-NOT-AUTOMATICALLY-TESTED (v1 heuristic limit)

**Reason:** The v1 dispatcher (plan 03) detects dead PIDs but cannot determine exit code across tick boundaries without a `dispatcher_state.json` file. A worker that exits 0 without calling `kanban_complete` is observationally identical to a crashed worker at the next tick — both appear as "dead PID with task still running". The dispatcher emits `crashed` in both cases.

**What IS enforced:**
- `KanbanEventKind::ProtocolViolation` exists as a distinct enum variant (serializes to `"protocol_violation"`) — tested by `crashed_and_protocol_violation_are_distinct_event_kinds` in `protocol_violation.rs`.
- The store layer emits `protocol_violation`-equivalent rejections via D-22 gates (`completion_rejected` event on `created_cards` violations).
- The manual checkpoint in plan 09 Task 3 (live worker spawn smoke) covers the runtime path.

**To lift the `#[ignore]`:** Implement `dispatcher_state.json` reconciliation — write the child's exit code before `exec`; read it at the next tick to distinguish `exit 0` from `SIGKILL`. Target: Phase 36.3.7.0 (fix-forward sub-phase if smoke fails).

---

## Static-Grep Regression Gates (`invariants_36_3_7.rs`)

These 10 gates survive as `#[test]` assertions in the `invariants_36_3_7.rs` file. They prevent
protocol-correctness source literals from being silently removed during refactors.

| # | Gate name | Source file checked | Literal / condition asserted | Added in plan |
|---|-----------|---------------------|------------------------------|---------------|
| INV-36.3.7-SG-01 | `atomic_claim_uses_begin_immediate` | `crates/ironhermes-kanban/src/cas.rs` | Contains `"Immediate"` (TransactionBehavior::Immediate for the CAS claim) | 01 |
| INV-36.3.7-SG-02 | `cas_inserts_task_run_in_same_transaction` | `crates/ironhermes-kanban/src/cas.rs` | Contains `"task_runs"` (INSERT in same tx as UPDATE) | 01 |
| INV-36.3.7-SG-03 | `dispatcher_calls_build_kanban_worker_env` | `crates/ironhermes-kanban/src/worker_spawn.rs` | Contains `"build_kanban_worker_env"` | 01 / un-ignored in 03 |
| INV-36.3.7-SG-04 | `worker_spawn_calls_env_clear` | `crates/ironhermes-kanban/src/worker_spawn.rs` | Contains `"env_clear"` | 01 / un-ignored in 03 |
| INV-36.3.7-SG-05 | `kanban_is_in_bypass_list` | `crates/ironhermes-core/src/commands/running_agent.rs` | Contains `"\"kanban\""` (D-36 mid-run bypass) | 01 / un-ignored in 06 |
| INV-36.3.7-SG-06 | `kanban_subcommand_registered_in_main` | `crates/ironhermes-cli/src/main.rs` | Contains `"Commands::Kanban"` (D-35 CLI subcommand) | 09 |
| INV-36.3.7-SG-07 | `kanban_commanddef_universal` | `crates/ironhermes-core/src/commands/registry.rs` | Contains `CommandDef::new("kanban"` AND `"Universal"` within 200 chars (D-36 platform scope) | 09 |
| INV-36.3.7-SG-08 | `kanban_dispatcher_spawned_in_gateway` | `crates/ironhermes-gateway/src/runner.rs` | Contains `"ironhermes_kanban"` AND `"run_dispatch_loop"` (D-09 gateway embed) | 09 |
| INV-36.3.7-SG-09 | `chat_subcommand_has_q_flag` | `crates/ironhermes-cli/src/main.rs` | Contains `long = "query"` AND `short = 'q'` (D-15 worker spawn shape) | 09 |
| INV-36.3.7-SG-10 | `kanban_guidance_is_static_const` | `crates/ironhermes-kanban/src/kanban_guidance.rs` | Contains `pub const KANBAN_GUIDANCE: &str` AND does NOT contain `format!(` or `concat!(` (D-26 cache-stability) | 09 |

---

## Test File Cross-Reference

| Test file | Plan | Tests active | Tests ignored | Critical invariants covered |
|-----------|------|-------------|---------------|-----------------------------|
| `invariants_36_3_7.rs` | 01, 03, 06, 08, 09 | 14 | 0 | SG-01 through SG-10 |
| `store_smoke.rs` | 02 | 6 | 0 | INV-36.3.7-01 (smoke), INV-36.3.7-09 (smoke) |
| `cas_concurrency.rs` | 02 | 2 | 0 | INV-36.3.7-01, INV-36.3.7-09 |
| `dispatcher_logic.rs` | 03 | 10 | 0 | INV-36.3.7-04, INV-36.3.7-05, INV-36.3.7-06 |
| `tools_smoke.rs` | 04 | 14 | 0 | INV-36.3.7-02, INV-36.3.7-03 |
| `guidance_static.rs` | 05 | 5 | 0 | (D-26 KANBAN_GUIDANCE content correctness) |
| `skills_sync.rs` | 07 | ≥8 | 0 | INV-36.3.7-08 |
| `end_to_end.rs` | 09 | 2 | 0 | Cross-plan composition (store → dispatcher → tool layer) |
| `protocol_violation.rs` | 09 | 6 | 1 | INV-36.3.7-02, INV-36.3.7-03, INV-36.3.7-09; INV-36.3.7-10 (1 ignored — v1 limit) |

---

## Deferred Invariants (not covered in v1)

| # | Invariant | Reason deferred | Target phase |
|---|-----------|-----------------|-------------|
| INV-36.3.7-10 (auto) | Protocol violation auto-block via dispatcher (exit-0 path) | v1 heuristic cannot distinguish exit 0 from crash without dispatcher_state.json | 36.3.7.0 or 36.3.7.1 |
| INV-HEARTBEAT | `kanban_heartbeat` tool available in worker mode | Tool deferred to 36.3.7.1 | 36.3.7.1 |
| INV-LINK | Cross-task dependency via `kanban_link` LLM tool | Tool deferred to 36.3.7.1 | 36.3.7.1 |
| INV-MULTIBOARD | Multi-board isolation (`--board <slug>`) | Multi-board CLI deferred to 36.3.7.3 | 36.3.7.3 |
| INV-NOTIFIER | Gateway notifier subscription + polling loop | Deferred to 36.3.7.5 | 36.3.7.5 |

---

## Manual Checkpoint Coverage

| Checkpoint | Task | What it covers | Outcome |
|------------|------|----------------|---------|
| Task 3: Live worker spawn smoke | Plan 09 | INV-36.3.7-10 runtime path; end-to-end dispatcher → real worker process → kanban_complete | Pending human sign-off |
| Task 4: Live /kanban gateway bypass | Plan 09 | INV-36.3.7-SG-05 runtime path; D-36 mid-run bypass from real gateway session | Pending human sign-off |
