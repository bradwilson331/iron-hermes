---
phase: 17-memory-tools-external-providers
plan: 01
subsystem: memory
tags: [rust, memory-store, memory-tool, capacity, ux, frozen-snapshot]

# Dependency graph
requires:
  - phase: 15-prompt-assembly
    provides: prompt_builder.load_memory() calls format_for_system_prompt
provides:
  - MemoryStore.format_for_system_prompt returns capacity header in "## Memory (XX% -- N/2,200 chars)" format
  - MemoryTool.execute returns human-readable success messages with capacity feedback per D-14
  - MemoryTool.execute returns structured D-15 error envelopes for capacity_exceeded and content_rejected
affects: [memory-provider-backends, session-storage, prompt-assembly]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Lazy capacity header computation: snapshot stores raw Vec<String>, header computed at format_for_system_prompt call time"
    - "D-14 human-readable tool responses: parse JSON from store layer, format as plain English with capacity numbers"
    - "D-15 error envelopes: normalize store error JSON to structured envelopes before returning to agent"

key-files:
  created: []
  modified:
    - crates/ironhermes-core/src/memory_store.rs
    - crates/ironhermes-tools/src/memory_tool.rs

key-decisions:
  - "Snapshot field changed from HashMap<MemoryTarget, String> to HashMap<MemoryTarget, Vec<String>> - raw entries stored, header computed lazily"
  - "format_with_commas() helper added to both memory_store.rs and memory_tool.rs for thousands-separator formatting"
  - "Error transformation in MemoryTool: blocked -> content_rejected envelope; capacity_exceeded -> D-15 envelope with suggestion field"

patterns-established:
  - "Frozen snapshot pattern: snapshot captured at load_from_disk, never mutated; format_for_system_prompt derives output from frozen entries"
  - "Tool response layer: MemoryTool transforms raw store JSON into human-readable strings; store layer stays JSON-first"

requirements-completed: [MEM-01, MEM-02, MEM-03, MEM-04, MEM-05]

# Metrics
duration: 8min
completed: 2026-04-12
---

# Phase 17 Plan 01: Memory Tool UX Summary

**MemoryStore capacity headers in frozen snapshot (D-13) and MemoryTool human-readable success/error responses (D-14/D-15)**

## Performance

- **Duration:** 8 min
- **Started:** 2026-04-12T18:02:55Z
- **Completed:** 2026-04-12T18:06:21Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments
- Refactored MemoryStore snapshot field from pre-formatted string to raw Vec<String> entries, enabling lazy capacity header computation
- format_for_system_prompt now returns "## Memory (67% -- 1,474/2,200 chars)" format per D-13
- MemoryTool.execute now returns human-readable success messages: "Added to memory. Memory: 72% -- 1,584/2,200 chars (3 entries)"
- Error responses normalized to D-15 structured envelopes: capacity_exceeded with current/limit/entry_size/suggestion, content_rejected with injection_pattern_detected reason
- 16 memory_store tests + 11 memory_tool tests passing; 22 prompt_builder tests unaffected

## Task Commits

1. **Task 1: Refactor MemoryStore snapshot to include capacity headers** - `b8f840d` (feat)
2. **Task 2: Update MemoryTool response format with human-readable capacity feedback** - `02dedb3` (feat)

## Files Created/Modified
- `crates/ironhermes-core/src/memory_store.rs` - Changed snapshot type, lazy capacity header, format_with_commas helper, new tests
- `crates/ironhermes-tools/src/memory_tool.rs` - format_success_response/format_error_response/fmt_commas helpers, execute() reformatting, new error envelope tests

## Decisions Made
- Snapshot stores raw `Vec<String>` entries instead of pre-formatted string — allows capacity header to always reflect correct char count at read time, avoids stale pre-computed header
- fmt_commas / format_with_commas implemented inline (no external dep) — thousands-separator needed for D-13 format, avoid adding num-format crate for a trivial function
- Error transformation happens in MemoryTool layer (not MemoryStore) — store stays JSON-first for programmatic use; tool layer is the human-facing boundary

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## Next Phase Readiness
- format_for_system_prompt API unchanged — prompt_builder.load_memory() works without modification
- D-13/D-14/D-15 requirements satisfied; memory tool UX ready for integration testing
- Plans 17-02 through 17-05 can proceed (memory provider backends, etc.)

---
*Phase: 17-memory-tools-external-providers*
*Completed: 2026-04-12*
