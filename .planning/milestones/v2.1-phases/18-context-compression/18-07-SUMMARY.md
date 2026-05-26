---
phase: 18-context-compression
plan: 07
subsystem: api
tags: [rust, prompt-assembly, system-message, prompt-slot]

requires:
  - phase: 15-10-layer-prompt-assembly
    provides: PromptSlot enum + build_split durable/ephemeral assembly
provides:
  - PromptSlot::SystemMessage = 2 durable slot
  - Symbolic durable boundary (<= ContextFiles) / ephemeral (>= Timestamp)
  - SystemMessage population from config.agent.system_message with security scan + 20K cap
affects: [18-03, 18-06]

tech-stack:
  added: []
  patterns: [symbolic slot boundary comparisons instead of numeric]

key-files:
  modified:
    - crates/ironhermes-agent/src/prompt_builder.rs

key-decisions:
  - "Shift all downstream slots +1: ToolGuidance=3..UserMessage=10"
  - "Durable/ephemeral boundary is symbolic via PromptSlot ordering (no numeric persistence)"
  - "system_message passed through scan_context_content and capped at 20K chars (T-18-11)"

patterns-established:
  - "New durable slots read config + scan + cap + set_slot"

requirements-completed: [PRMT-11, PRMT-14]

duration: ~10min
completed: 2026-04-12
---

# Phase 18 Plan 07: PromptSlot::SystemMessage slot 2

**Insert SystemMessage = 2 between Identity and ToolGuidance; shift downstream discriminants; wire config.agent.system_message with security scan + 20K cap**

## Performance

- **Tasks:** 1
- **Files modified:** 1
- **Tests:** 26 prompt_builder tests green (4 new + 22 regression)
- **Completed:** 2026-04-12

## Accomplishments
- PromptSlot layout matches D-01 exactly (SystemMessage=2, ToolGuidance=3, ..., UserMessage=10)
- is_ephemeral remains symbolic (self >= Timestamp), auto-adjusts to new layout
- with_system_message() setter scans via scan_context_content and caps at 20K chars; empty input omits slot
- All Phase 15 build_split tests still pass

## Task Commits

1. **Task 1: Shift PromptSlot enum + wire SystemMessage assembly** — `e902706` (feat)

## Files Created/Modified
- `crates/ironhermes-agent/src/prompt_builder.rs` — PromptSlot shift + SystemMessage assembly

## Decisions Made
- Security scan via scan_context_content (Phases 3/14 scanner) for durable-slot consistency
- Empty system_message → slot omitted (not an empty block)

## Deviations from Plan
None — plan executed as written.

## Issues Encountered
None.

## Next Phase Readiness
- 18-03 SummarizingEngine can inject [CONTEXT HISTORY] as a durable slot peer
- 18-06 wiring can set config.agent.system_message and trust the durable boundary

---
*Phase: 18-context-compression*
*Completed: 2026-04-12*
