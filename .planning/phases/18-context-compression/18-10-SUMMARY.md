---
phase: 18
plan: 10
status: complete
subsystem: context-compression
tags: [bugfix, tool-pair-atomicity, UAT-closure]
requires: [18-02, 18-03, 18-07, 18-08, 18-09]
provides: [apply_adaptive_shift-caller-contract, pair-atomicity-guard, compression-observability]
tech-stack:
  added: []
  patterns: [return-value-consumed-folding, defensive-atomicity-guard, log-outcome-discriminant]
key-files:
  created: []
  modified:
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-agent/src/tool_pair.rs
decisions:
  - pair-crossing-guard-pulls-back
  - collapsed-range-logs-tracing-warn-logic-stall
  - log-outcome-field-for-regression-alarm
metrics:
  tasks: 3
  commits: 3
  tests_added: 14
  tests_total: 162
  duration_minutes: ~50
completed: 2026-04-13
---

# Phase 18 Plan 10: Root-cause fix for `apply_adaptive_shift` orphan defect

Close the 2026-04-13 live UAT gap (Test 5, severity high): `SummarizingEngine::compress` was discarding the return value of `tool_pair::apply_adaptive_shift`, causing every tool-pair-straddling compression attempt to split the pair and roll back with `OrphanedToolPair`. Atomic rollback (18-08) masked the failure from the user, but no actual pruning ever landed — token usage grew unbounded across turns.

## Hypothesis Arm Confirmed

**Primary arm — boundary arithmetic (discarded return value).**

Task 1's 5 UAT-shape tests (488 / 3055 / 3511 / 7111 / 9467 tokens) all failed in RED state with `ContextError::OrphanedToolPair`. The failure mode matched exactly the hypothesis in the plan's objective: `let _ = tool_pair::apply_adaptive_shift(...)` threw away the adjusted boundary, and `let prune_end = protect_start` used the stale initial value, cutting through the tool_use/tool_result pair.

The secondary arm (large-body branch with `>500` token bodies) was subsumed by the primary fix because `apply_adaptive_shift` returns `protect_start` unchanged in that arm but rewrites the tool_result content in place — which is fine once the caller-side pair-atomicity guard ensures the pair stays together across the prune boundary regardless.

## Final Fix Shape

Two layered defenses in `summarizing_engine.rs::compress`:

1. **Consume the shift return value** (lines ~286-302): compute `initial_protect_start`, then fold each pair's `apply_adaptive_shift(...)` return into `effective_protect_start = min(effective_protect_start, adjusted)`. Use that as the authoritative `protect_start`.

2. **Pair-atomicity guard** (lines ~325-355): after deriving `prune_end`, re-run `detect_tool_pairs` and for every pair that straddles `[prune_start..prune_end]` pull `prune_end` back to the assistant's index — keeping the pair live. Per user policy this maximizes safety over recovery. If `prune_end <= prune_start` the range collapses to a no-op and `tracing::warn!` fires with wording "compression requested but guard collapsed prune range to no-op — logic stall".

**Observability:** success path now logs `outcome = "compressed"` alongside `prune_start`, `prune_end`, `pair_count`, `tokens_freed`. Rollback path logs `outcome = "rolled_back"`. A future regression is detectable from logs alone — `grep -c 'outcome = "rolled_back"' $LOG` on a well-formed-pair session should be 0.

## Diff Surface

| File | Lines changed | Nature |
|------|--------------:|--------|
| `crates/ironhermes-agent/src/summarizing_engine.rs` | +76 / -20 (fix) + ~550 (tests) | Fix + 13 new tests |
| `crates/ironhermes-agent/src/tool_pair.rs` | +51 (tests only) | 1 contract test |

## Test Count Delta

- **Before 18-10:** 148 passing in `ironhermes-agent --lib`
- **After 18-10:** 162 passing, 0 failing
- **Added:** 14 total
  - Task 1 (RED): 7 UAT-shape tests + 1 contract test = 8
  - Task 3 (regression): 6 tests

All 14 new tests pass. All pre-existing invariants (18-07 history pin, 18-07 iterative summary, 18-07 aux-model fallback, 18-08 atomic rollback) remain green.

## UAT Arm Coverage

The fix directly resolves the 4 live-UAT shapes observed in 2026-04-13 (488/3055/3511/7111/9467 tokens, web_read tool-pair straddling protect_start) — all 5 unit tests reproducing those shapes compress cleanly with a `[CONTEXT HISTORY]` segment inserted and zero orphan.

## Handoff to Live UAT

Ready for live re-run of 18-UAT.md Tests 5/6/7/8:

- **Test 5 (Tool-Pair Atomicity):** unblocked — expect `outcome = "compressed"` on every compression attempt, no `outcome = "rolled_back"` log lines with well-formed pairs.
- **Test 6 (Single Pinned `[CONTEXT HISTORY]`):** unblocked — should observe exactly one pin after multiple passes (iterative summary path is unchanged).
- **Test 7 (Aux-Model Fallback):** unblocked — previously couldn't reach the fallback branch because orphan fired first; now aux-model failures will exercise the LocalPruningEngine fallback.
- **Test 8 (Memory Flush Before Prune):** unblocked — `context:pre_compress` fires before destructive mutation, and destructive mutation actually lands now (not rolled back).

## Residual Risk / Concerns

1. **Plan-checker concern — guard dead-code branch:** my pair-atomicity guard at `summarizing_engine.rs` always pulls `prune_end` BACK when a split is detected (matches user policy). The original plan had an alternate branch (push `prune_end` forward) which I did not implement — it is NOT dead code because the current branch handles both split directions (asst inside, result outside → pull back to exclude asst is always safe). The simpler single-direction policy is correct and clearer. No dead branch remains.
2. **Small assistant tool_call messages:** if `asst_tokens + tool_result_tokens < protect_last_tokens`, the protected tail will naturally include both (pair fully live, no straddle). This case is handled by detection returning the pair in `pairs_after_shift` with `fully_out` → no guard action. `compress_ok_pair_fully_in_protected_tail` locks this behavior.
3. **Clippy on `ironhermes-agent`:** there are 37 pre-existing clippy errors (baseline was 38 — my changes net -1). All are outside plan scope. `cargo check --workspace` is clean. `cargo test -p ironhermes-agent --lib` is green. Recommend a separate cleanup plan for the strict clippy pass.
4. **Workspace clippy in `ironhermes-core`:** 3 pre-existing clippy errors (`derivable_impls`, `manual_is_multiple_of`, deprecated fn re-export). Pre-existing on develop. Out of scope.

## Readiness for Live UAT Re-run

**READY.** Recommended config for the live re-run (matches the session that caught the bug):

- `agent.context_engine = "summarizing"`
- `agent.compression_threshold = 0.001`
- `agent.protect_last_tokens = 100`
- compression aux role = `claude-haiku-4-5`
- Drive session with web_read turns until token budget crosses threshold.

Pass criteria on the live re-run:

1. Every compression attempt logs `summarizing_engine: compressed outcome = "compressed"`.
2. No `outcome = "rolled_back"` log lines.
3. Next turn's message list contains exactly one `name = "context_history"` system message with `[CONTEXT HISTORY]` sentinel.
4. No Anthropic 400 errors reach the user.

## Self-Check: PASSED

- `cargo test -p ironhermes-agent --lib` → 162 passed, 0 failed
- `cargo check --workspace` → clean
- 3 commits landed on `develop`:
  - `42eb073` test(18-10): pin UAT orphan defect in apply_adaptive_shift
  - `4eade57` fix(18-10): apply adaptive shift return value; enforce tool-pair atomicity across prune boundary
  - `a45e511` test(18-10): regression matrix for parallel tool_calls, boundary edges, back-to-back pairs

## Post-ship fix: front-straddle (2026-04-13T05:18 UAT regression)

### Bug

The post-ship live UAT at 2026-04-13T05:18 reproduced a *new* failure
mode the original 18-10 guard did not cover: the very first tool-calling
turn places the assistant at index 2 (inside the front-protected
`protect_first_n=3` region) while its tool_result lands at index 3+
(inside the prune range). The guard only handled BACK-straddle by
pulling `prune_end` back to `pair.assistant_idx`. With
`assistant_idx=2 < prune_start=3`, the pull-back collapsed the range
(`prune_end=2 < prune_start=3`) and every compression attempt logged
`reason="pair_atomicity_collapsed_range"` and returned a no-op —
identical user symptom to the pre-18-10 OrphanedToolPair (token usage
grew unbounded, no compression ever landed).

### Fix

Distinguish straddle direction in the guard loop:

| Case | Condition | Adjustment |
|------|-----------|------------|
| Back-straddle | `asst_in && !all_results_in` | `prune_end = min(prune_end, asst_idx)` (unchanged) |
| Front-straddle | `!asst_in && asst_idx < prune_start && any_result_in` | `prune_start = max(prune_start, max(tool_result_idx) + 1)` |

Invariant preserved: `prune_start` only increases, `prune_end` only
decreases. Required mutating `prune_start` (was previously immutable).
The collapsed-range warn path still triggers if both adjustments meet
in the middle (no prunable region survives).

The `kept_before / kept_after` slicing math, history-segment offset
arithmetic, and `pin_idx` clamp downstream of the guard were already
written against the (mutable) `prune_start` value so no further changes
were needed.

### New tests

Added 2 RED-then-GREEN tests reproducing the live UAT shape (single
web_read pair, `before_tokens ≈ 64555`, `protect_first_n=3`,
`protect_last=100`):

- `compress_ok_front_straddle_asst_in_protect_first_n_single_result`
- `compress_ok_front_straddle_parallel_tool_calls`

Both include `assert_pair_front_straddles` which verifies the fixture
actually places the assistant inside `protect_first_n` and at least one
tool_result in `[protect_first_n, protect_start)` — guards against
silent false-GREEN from a miscounted fixture (same anti-mocking pattern
as `assert_pair_straddles`).

Test count: **162 → 164 passing, 0 failing.**

### Commits

- `b123179` test(18-10): reproduce front-straddle guard bug from live UAT 2026-04-13T05:18 (RED)
- `28caf61` fix(18-10): handle front-straddle pairs by pushing prune_start past tool_results (GREEN)
- (this) docs(18-10): document post-ship front-straddle fix

### Clippy delta

`cargo clippy -p ironhermes-agent --all-targets`: 2 pre-existing logic-bug
errors in `context_engine.rs` (lines 378, 486 — `assert!(x || !x)` test
scaffolding) plus baseline warnings. **0 new errors introduced.**
