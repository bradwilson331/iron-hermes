---
phase: 35
slug: cron-subagent-budget-isolation-give-cron-its-own-delegate-ta
status: complete
nyquist_compliant: true
wave_0_complete: true
created: 2026-05-21
validated: 2026-05-21
---

# Phase 35 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Derived from 35-RESEARCH.md §Validation Architecture. D-03 resolved to Option B (clamp-to-ceiling).

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in `#[test]` / `#[tokio::test]` (cargo test) + existing `async-trait` mock `SubagentRunner` impls |
| **Config file** | none — per-crate `Cargo.toml` in the workspace |
| **Quick run command** | `cargo test -p ironhermes-agent budget` · `cargo test -p ironhermes-tools delegate` · `cargo test -p ironhermes-cron-runner cron_budget` |
| **Full suite command** | `cargo test --workspace` then `cargo clippy --workspace --all-targets -- -D warnings` |
| **Estimated runtime** | ~60–120 seconds (workspace) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p <touched-crate> <filter>` + `cargo build -p <crate>`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd:verify-work`:** `cargo test --workspace` green + `cargo clippy --workspace --all-targets -- -D warnings` clean
- **Max feedback latency:** ~120 seconds

---

## Per-Task Verification Map

> The behavior/threat/command columns are the binding contract. Commands below were corrected during the 2026-05-21 audit to match the actual test names that landed during execution (two filters in the draft selected zero or the wrong test).

| Behavior (Req) | Threat Ref | Secure Behavior | Test Type | Automated Command | File / Seam | Status |
|----------------|------------|-----------------|-----------|-------------------|-------------|--------|
| D-07.1 / D-02 — child drains its OWN budget → parent `remaining()` unchanged (independence; inverts old PROV-10 sharing) | — (correctness) | A child subagent cannot decrement its parent's iteration counter | unit | `cargo test -p ironhermes-agent independent_budget` | `test_independent_budget_child_drain_does_not_affect_parent` — `agent_loop.rs:2451` budget_tests mod | ✅ green |
| D-07.2 / T-28.1-16 — cron job delegates to exhaustion → interactive budget at full headroom | T-28.1-16 (DoS) | Cron fan-out via shared `ToolRegistry` cannot drain interactive headroom | unit/integration | `cargo test -p ironhermes-cron-runner cron_subagent_budget` | `cron_subagent_budget_independence_from_interactive` — `runner.rs:748`, sibling to top-level `cron_budget_is_independent_from_interactive_budget:638` | ✅ green |
| D-07.3 / D-03 (Option B) — caller `max_iterations` > config ceiling is clamped down (+warn); ≤ ceiling honored verbatim | V5 input validation (model-controlled override) | Model cannot inflate its per-child budget above `config.delegation.max_iterations` | unit | `cargo test -p ironhermes-tools max_iterations` | `test_per_call_max_iterations_clamps_to_ceiling` — `delegate_task.rs` (clamp sites `:917` single / `:325` batch) | ✅ green |
| DoS bound (D-05) — depth × concurrency guards still enforced after PROV-10 retirement | V11/V12 DoS | `max_spawn_depth` + `max_concurrent_children` still reject overflow | unit (regression) | `cargo test -p ironhermes-tools test_batch_oversize_returns_err` · `cargo test -p ironhermes-tools test_orchestrator_at_max_depth_downgrades` | Phase 32.2 DoS-bound guards `delegate_task.rs::tests` — confirmed still green | ✅ green |
| PROV-10 doc/test retirement — shared-counter assertions inverted/retired; budget.rs SeqCst preserved | — (regression) | Budget atomic ordering unchanged (no `Relaxed` introduced) | unit (grep) | `cargo test -p ironhermes-agent --test budget_ordering_grep` | `budget_uses_only_seqcst_ordering` — `tests/budget_ordering_grep.rs` stays green after `budget.rs` doc rewrite | ✅ green |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [x] New independence test (D-07.1) in `agent_loop.rs::budget_tests` — covers the inverted PROV-10 assertion.
- [x] Subagent-layer extension of `cron-runner/runner.rs:638` (D-07.2) — current test only proves top-level cron independence.
- [x] Rewrite `delegate_task.rs:2064` (D-07.3) to clamp semantics — request > ceiling clamped, request ≤ ceiling honored.

*No framework install needed — Rust test harness + existing mock `SubagentRunner` impls already present (`delegate_task.rs:1149`, `:2071`).*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Threat-model / design doc update reflects the global per-subagent model + clamp-to-ceiling bound | D-05 | Doc prose, not code-assertable | Confirm `docs/AGENT-RUNTIME-DESIGN.md` §6.4/§8 amended: superseded cron-specific fix removed, new bound `max_spawn_depth × max_concurrent_children × max_iterations` documented, PROV-10 retirement + clamp policy recorded |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references
- [x] No watch-mode flags
- [x] Feedback latency < 120s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** approved 2026-05-21 (audit) — all 5 behaviors COVERED by green automated tests.

---

## Validation Audit 2026-05-21

| Metric | Count |
|--------|-------|
| Behaviors audited | 5 |
| COVERED (green test) | 5 |
| PARTIAL | 0 |
| MISSING | 0 |
| Gaps requiring new tests | 0 |
| Doc command filters corrected | 2 |

**Findings:** No behavior gaps — every row's test was authored during execution and runs green (re-run live 2026-05-21). No `gsd-nyquist-auditor` dispatch needed (no tests to generate). Two stale command filters in the draft were corrected to match the test names that actually landed:

- **D-07.2:** `cargo test … cron_budget` selected only the pre-existing top-level test, **not** the new subagent-layer test (`cron_subagent_budget_…` — `cron_budget` is not a substring). Corrected to `cron_subagent_budget`.
- **D-05 DoS bound:** `max_spawn` / `max_concurrent` filters selected **zero** tests. Corrected to the real guard tests `test_batch_oversize_returns_err` / `test_orchestrator_at_max_depth_downgrades`.
