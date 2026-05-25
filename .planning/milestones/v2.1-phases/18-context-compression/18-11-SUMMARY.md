---
phase: 18
plan: 11
status: complete
subsystem: context-compression
tags: [rust, context-compression, tool-pair-atomicity, protect-first-n, uat-gap-closure]
requirements: [PRMT-13, PRMT-14]
gap_closure: true
closes_uat_gaps:
  - "Default compression.protect_first_n=3 is safe for single-tool-pair conversations"
requires: [18-02, 18-03, 18-07, 18-08, 18-09, 18-10]
provides: [compute_effective_protect_first_n, front-boundary-autoshrink, effective-vs-configured-log-discriminant]
tech-stack:
  added: []
  patterns: [safety-over-recovery-monotonic-downward, configured-vs-effective-split, log-outcome-discriminant]
key-files:
  created:
    - .planning/phases/18-context-compression/18-11-SUMMARY.md
  modified:
    - crates/ironhermes-agent/src/tool_pair.rs
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-core/src/config.rs
decisions:
  - compute-effective-protect-first-n-is-pure-helper-not-struct-field
  - effective-value-monotonic-downward-never-grows
  - log-configured-and-effective-fields-on-success-path
  - preserve-pair-atomicity-collapsed-range-warn-for-unsolvable-cases
metrics:
  tasks: 3
  commits: 3
  tests_added: 13
  tests_total: 177
  duration_minutes: ~35
completed: 2026-04-13
tests_added: 13
commits:
  - 558530d
  - c3b6cd7
  - 877849b
---

# Phase 18 Plan 11: Tool-pair-aware `protect_first_n` autocorrection

Close the high-severity 18-UAT gap "Default compression.protect_first_n=3 is safe for single-tool-pair conversations" by making `SummarizingEngine::compress` auto-shrink its effective `protect_first_n` when a front-protected assistant tool_call has ≥1 tool_result outside the front-protected region. The configured value remains an upper bound on operator intent; the effective value is a per-call boundary that can shrink (never grow) to release a pinned tool-pair into the prunable range.

## Defect Closed

Live UAT 2026-04-13T23:44 passed Tests 5 & 6 only after lowering `compression.protect_first_n` from the documented default `3` to `2`. Under the default with minimal shape `[sys, user, asst(tool_use), tool_result]`:

- `asst_tool_calls` sits at idx 2 (< `protect_first_n=3`, inside front-protect).
- `tool_result` sits at idx 3 (only prunable message).
- `protect_last_tokens=100` can't fit the ~3K-token tool_result body → tail walker takes 0, `protect_start=msgs.len()=4`, prune range = `[3, 4)`.
- 18-10 front-straddle guard pushes `prune_start=max(tool_result_idx)+1=4` → `prune_end (4) <= prune_start (4)` → `pair_atomicity_collapsed_range` warn, zero compression lands, agent hits `MAX_COMPRESSION_PASSES=10`, unbounded token growth.

Root cause: `protect_first_n` is a pure index count. It did not consider tool-pair atomicity when selecting its boundary. The documented default was unsafe for the most common real message shape.

## Fix Shape (Safety > Recovery)

Pure helper `tool_pair::compute_effective_protect_first_n(messages, configured, pairs)` computes the effective boundary BEFORE `compute_protect_start` runs. Rule:

- `configured_first_n == 0` → return `0` (short-circuit, no shrink possible).
- For every pair where `asst_idx < configured_first_n` AND `max(result_idx) >= configured_first_n` (asst front-protected, ≥1 result outside): `effective = min(effective, asst_idx)`.
- With multiple conflicting pairs, picks `min(asst_idx)` (most protective).
- Never grows above configured (monotonic downward).

`SummarizingEngine::compress` substitutes `effective_first_n` at all 4 downstream reference sites inside the method body:

1. `compute_protect_start` arg.
2. Early-return guard `if protect_start <= effective_first_n`.
3. `let mut prune_start = effective_first_n;`.
4. `pin_idx = effective_first_n.min(new_messages.len())`.

The struct field `self.protect_first_n` stays the CONFIGURED value (operator intent is preserved; only the per-call boundary changes).

Observability: a new `tracing::info!` fires with `reason = "tool_pair_front_boundary_autoshrink"` and both `configured_protect_first_n` and `effective_protect_first_n` fields when a shrink occurs. The success-path `outcome = "compressed"` info! also gains `effective_protect_first_n` + `configured_protect_first_n` fields — regression detectable from logs alone.

## Diff Surface

| File | Change | Nature |
|------|-------:|--------|
| `crates/ironhermes-agent/src/tool_pair.rs` | +46 (fn) +128 (tests) | New pure helper + 8 unit tests |
| `crates/ironhermes-agent/src/summarizing_engine.rs` | +41 wire-in / ~4 substitutions + 5 tests (~231 lines) | Early compute + all downstream substitutions + 2 RED-to-GREEN + 3 regression tests |
| `crates/ironhermes-core/src/config.rs` | +9 doc comment | Documents configured-vs-effective auto-shrink |

## Test Count Delta

| Metric | Before 18-11 | After 18-11 | Delta |
|--------|------:|------:|------:|
| `ironhermes-agent --lib` total | 164 | 177 | +13 |
| failed | 0 | 0 | 0 |

Breakdown:
- 2 RED-to-GREEN tests (Task 1): `compress_fails_default_protect_first_n_single_pair_RED_then_GREEN`, `compress_fails_default_protect_first_n_parallel_tool_calls_RED_then_GREEN`.
- 8 `tool_pair::tests::effective_protect_first_n_*` unit tests (Task 2).
- 3 regression tests (Task 3): `compress_ok_no_shrink_when_pair_fully_prunable`, `compress_ok_no_shrink_when_configured_first_n_zero`, `compress_ok_multiple_front_straddling_pairs_shrinks_to_minimum_asst`.

## Grep Acceptance Results

| Pattern | Path | Required | Actual |
|---------|------|---------:|-------:|
| `pub fn compute_effective_protect_first_n` | `crates/ironhermes-agent/src/tool_pair.rs` | 1 | 1 |
| `fn effective_protect_first_n_` (test fns) | `crates/ironhermes-agent/src/tool_pair.rs` | ≥7 | 8 |
| `compute_effective_protect_first_n` | `crates/ironhermes-agent/src/summarizing_engine.rs` | ≥1 (call site) | 6 (call + doc refs + RED-GREEN test copies exercising helper) |
| `effective_protect_first_n` | `crates/ironhermes-agent/src/summarizing_engine.rs` | ≥3 | 10 |
| `auto-shrink` | `crates/ironhermes-core/src/config.rs` | ≥1 | 1 |
| `compress_fails_default_protect_first_n` (tests) | `crates/ironhermes-agent/src/summarizing_engine.rs` | 2 | 2 |
| `pair_atomicity_collapsed_range` | `crates/ironhermes-agent/src/summarizing_engine.rs` | unchanged from 18-10 | 2 (log path retained) |
| `assert_pair_front_straddles` usages | `crates/ironhermes-agent/src/summarizing_engine.rs` | increase by 2 | increase by 2 (verified via RED tests) |

All downstream verification commands pass:

- `cargo test -p ironhermes-agent --lib` → `177 passed; 0 failed`.
- `cargo test -p ironhermes-agent --lib tool_pair::tests::effective_protect_first_n` → `8 passed`.
- `cargo test -p ironhermes-agent --lib compress_fails_default_protect_first_n` → `2 passed`.
- `cargo test -p ironhermes-agent --lib compress_noop_when_only_pair_fills_entire_prunable_range` → `1 passed` (no-op contract preserved).
- `cargo check --workspace` → exit 0.

## Commits

1. `558530d` — `test(18-11): reproduce default protect_first_n=3 deadlock for single and parallel tool pairs (RED)`
2. `c3b6cd7` — `fix(18-11): auto-shrink effective protect_first_n when asst(tool_use) front-protected with result outside (GREEN)`
3. `877849b` — `test(18-11): regression matrix + additive shrink-path coverage`

## Deviations from Plan

None — plan executed exactly as written. One minor addition: a bonus 8th unit test `effective_protect_first_n_zero_configured_returns_zero` pins the `configured == 0` short-circuit path explicitly (plan required ≥7 tests; acceptance criterion ≥7 satisfied at 8).

## Decisions Made

1. **`compute_effective_protect_first_n` is a pure helper, not a new struct field.** Keeping `SummarizingEngine::protect_first_n` as the CONFIGURED value preserves operator intent in all observers (logs, debugging, `with_protect`, `fallback` engine). The effective value is per-call, stack-local.
2. **Monotonic downward.** Effective never exceeds configured. Operator sets an UPPER bound on front-protection; the fix only releases pinned pairs, never extends protection.
3. **Both `configured_*` and `effective_*` fields on the success `info!`.** Regressions (silent failure to shrink, or unexpected shrink when no conflict exists) are greppable from production logs without a rebuild.
4. **`pair_atomicity_collapsed_range` warn log retained.** The code path is now unreachable under default single-pair config, but genuinely-unsolvable cases (e.g., a malformed message list pre-validated elsewhere) would still benefit from the warning.

## UAT Re-run Status

Live UAT Tests 7 (aux fallback) and 8 (memory flush ordering) are **UNBLOCKED** for immediate re-run under default `compression.protect_first_n = 3` config. 18-12 will cover the live re-run verification.

Verification checklist for operator:
- `cargo run -p ironhermes-cli` with `compression.protect_first_n = 3`, `agent.compression_threshold = 0.001`, aux=claude-haiku-4-5.
- Drive a web_read loop identical to the 2026-04-13T23:44 UAT session.
- Grep `effective_protect_first_n` in the log → present on compression turns (fix path active).
- Grep `pair_atomicity_collapsed_range` in the log → MUST be 0 on healthy sessions.
- Grep `outcome = "compressed"` → appears on every compression attempt.

## Self-Check: PASSED

**Files verified exist:**
- `crates/ironhermes-agent/src/tool_pair.rs` — contains `compute_effective_protect_first_n` (line ~120) + 8 unit tests.
- `crates/ironhermes-agent/src/summarizing_engine.rs` — contains `effective_first_n` wire-in + 5 new tests (2 RED-to-GREEN + 3 regression).
- `crates/ironhermes-core/src/config.rs` — contains updated `CompressionConfig::protect_first_n` doc comment.
- `.planning/phases/18-context-compression/18-11-SUMMARY.md` — this file.

**Commits verified on branch `worktree-agent-adb9163a`:**
- `558530d` (Task 1 RED) — `test(18-11): reproduce default protect_first_n=3 deadlock for single and parallel tool pairs (RED)`
- `c3b6cd7` (Task 2 GREEN) — `fix(18-11): auto-shrink effective protect_first_n when asst(tool_use) front-protected with result outside (GREEN)`
- `877849b` (Task 3 regression) — `test(18-11): regression matrix + additive shrink-path coverage`
