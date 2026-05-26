---
phase: 18-context-compression
plan: 02
subsystem: api
tags: [rust, tool-pairs, context-compression, invariants]

requires:
  - phase: 18-context-compression
    provides: ContextEngine trait + LocalPruningEngine (18-01)
provides:
  - tool_pair module (detect_tool_pairs, apply_adaptive_shift, check_orphan_invariant)
  - ContextCompressor::compute_protect_start helper
  - LocalPruningEngine orphan-invariant enforcement
affects: [18-03, 18-06]

tech-stack:
  added: []
  patterns: [post-compression invariant guard + adaptive 500-token shift rule]

key-files:
  created:
    - crates/ironhermes-agent/src/tool_pair.rs
  modified:
    - crates/ironhermes-agent/src/context_engine.rs
    - crates/ironhermes-agent/src/context_compressor.rs
    - crates/ironhermes-agent/src/lib.rs

key-decisions:
  - "Adaptive shift is pure-functional; LocalPruningEngine calls it before ContextCompressor.compress"
  - "Orphan invariant runs after pruning and returns ContextError::OrphanedToolPair per D-16"
  - "Prose replacement caps tool args at 80 chars to prevent exfiltration in summary (T-18-02b)"

patterns-established:
  - "Tool-pair atomicity: detect → adaptive-shift → prune → invariant-check"

requirements-completed: [PRMT-13]

duration: ~25min
completed: 2026-04-12
---

# Phase 18 Plan 02: Tool-Pair Atomicity + Orphan Invariant

**Tool-pair detection, adaptive 500-token shift, and post-compression orphan invariant wired into LocalPruningEngine**

## Performance

- **Tasks:** 2
- **Files modified:** 4
- **Completed:** 2026-04-12

## Accomplishments
- tool_pair module: detect_tool_pairs supporting parallel tool_calls, apply_adaptive_shift (forward/backward), check_orphan_invariant
- ContextCompressor::compute_protect_start exposed for boundary math
- LocalPruningEngine.compress runs adaptive shift pre-prune, invariant check post-prune; surfaces ContextError::OrphanedToolPair
- 14 tests green (7 tool_pair + 7 context_engine including 3 new)

## Task Commits

1. **Task 1 + Task 2 (bundled): tool_pair module + LocalPruningEngine wiring** — `e9514fe` (feat)

## Files Created/Modified
- `crates/ironhermes-agent/src/tool_pair.rs` — ToolPair, detect_tool_pairs, apply_adaptive_shift, check_orphan_invariant
- `crates/ironhermes-agent/src/context_engine.rs` — LocalPruningEngine wired with pair guards
- `crates/ironhermes-agent/src/context_compressor.rs` — compute_protect_start helper
- `crates/ironhermes-agent/src/lib.rs` — pub mod tool_pair

## Decisions Made
- Bundled Task 1 and Task 2 into a single commit since both landed together with 14 tests green
- Adaptive shift mutates in place for backward mode; forward mode only shifts protect_start index

## Deviations from Plan
None structural — tasks were bundled into one commit instead of two.

## Issues Encountered
None.

## Next Phase Readiness
- LocalPruningEngine now blocks API 400s from orphaned tool pairs (T-18-02)
- 18-03 SummarizingEngine can reuse check_orphan_invariant
- 18-06 wiring has a complete local engine ready to route to

---
*Phase: 18-context-compression*
*Completed: 2026-04-12*
