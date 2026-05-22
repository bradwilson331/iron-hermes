---
phase: 35
slug: cron-subagent-budget-isolation-give-cron-its-own-delegate-ta
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-05-21
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

> Task IDs are placeholders until the planner assigns them; the behavior/threat/command columns are the binding contract.

| Behavior (Req) | Threat Ref | Secure Behavior | Test Type | Automated Command | File / Seam | Status |
|----------------|------------|-----------------|-----------|-------------------|-------------|--------|
| D-07.1 / D-02 — child drains its OWN budget → parent `remaining()` unchanged (independence; inverts old PROV-10 sharing) | — (correctness) | A child subagent cannot decrement its parent's iteration counter | unit | `cargo test -p ironhermes-agent independent_budget` | New test in `agent_loop.rs` budget_tests mod, sibling to repurposed `test_shared_budget_increment:2419` | ⬜ pending |
| D-07.2 / T-28.1-16 — cron job delegates to exhaustion → interactive budget at full headroom | T-28.1-16 (DoS) | Cron fan-out via shared `ToolRegistry` cannot drain interactive headroom | unit/integration | `cargo test -p ironhermes-cron-runner cron_budget` | Extend/mirror `cron-runner/runner.rs:638` to add a subagent-layer drain | ⬜ pending |
| D-07.3 / D-03 (Option B) — caller `max_iterations` > config ceiling is clamped down (+warn); ≤ ceiling honored verbatim | V5 input validation (model-controlled override) | Model cannot inflate its per-child budget above `config.delegation.max_iterations` | unit | `cargo test -p ironhermes-tools max_iterations` | **Rewrite** `test_per_call_max_iterations_overrides_config` (`delegate_task.rs:2064`) → assert clamp; add a ≤-ceiling honored case | ⬜ pending |
| DoS bound (D-05) — depth × concurrency guards still enforced after PROV-10 retirement | V11/V12 DoS | `max_spawn_depth` + `max_concurrent_children` still reject overflow | unit (regression) | `cargo test -p ironhermes-tools max_spawn` · `cargo test -p ironhermes-tools max_concurrent` | Existing tests `delegate_task.rs:1713` / `:2120` / `:2179` — confirm still green | ⬜ pending |
| PROV-10 doc/test retirement — shared-counter assertions inverted/retired; budget.rs SeqCst preserved | — (regression) | Budget atomic ordering unchanged (no `Relaxed` introduced) | unit (grep) | `cargo test -p ironhermes-agent --test budget_ordering_grep` | `tests/budget_ordering_grep.rs` must stay green after `budget.rs` doc rewrite | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] New independence test (D-07.1) in `agent_loop.rs::budget_tests` — covers the inverted PROV-10 assertion.
- [ ] Subagent-layer extension of `cron-runner/runner.rs:638` (D-07.2) — current test only proves top-level cron independence.
- [ ] Rewrite `delegate_task.rs:2064` (D-07.3) to clamp semantics — request > ceiling clamped, request ≤ ceiling honored.

*No framework install needed — Rust test harness + existing mock `SubagentRunner` impls already present (`delegate_task.rs:1149`, `:2071`).*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Threat-model / design doc update reflects the global per-subagent model + clamp-to-ceiling bound | D-05 | Doc prose, not code-assertable | Confirm `docs/AGENT-RUNTIME-DESIGN.md` §6.4/§8 amended: superseded cron-specific fix removed, new bound `max_spawn_depth × max_concurrent_children × max_iterations` documented, PROV-10 retirement + clamp policy recorded |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 120s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
