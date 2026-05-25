---
phase: 34b-context-system-parity
plan: "00"
subsystem: ironhermes-agent
tags: [scaffolding, test-infrastructure, context-refs, wave-0]
dependency_graph:
  requires: []
  provides: [context_refs-module, invariants_34b-integration-target, wave2-placeholder-tests]
  affects: [crates/ironhermes-agent/src/lib.rs]
tech_stack:
  added: []
  patterns: [ignore-placeholder-test, include_str-source-guard]
key_files:
  created:
    - crates/ironhermes-agent/src/context_refs.rs
    - crates/ironhermes-agent/tests/invariants_34b.rs
  modified:
    - crates/ironhermes-agent/src/context_compressor.rs
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-agent/src/lib.rs
decisions:
  - "context_refs.rs is a stub only — no pub fn/struct; Wave 1 (Plan 01) adds all production types"
  - "invariants_34b.rs documents the include_str! source anchors Wave 1/2 will use for handler.rs/state.rs/main.rs"
  - "#[ignore] placeholder tests compile with empty bodies — Wave 2 un-ignores and adds assertions"
metrics:
  duration: "~8 min"
  completed: "2026-05-22T10:50:34Z"
  tasks_completed: 3
  files_created: 2
  files_modified: 3
---

# Phase 34b Plan 00: Wave 0 Test Scaffolding Summary

Wave 0 test scaffolding for Phase 34b context-system parity. Empty module stub, integration-test file, and two `#[ignore]` placeholder tests — all compiling and giving Waves 1/2 concrete red→green targets from the start (Nyquist compliance).

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Create context_refs.rs empty module + lib wiring | e27fc3c1 | context_refs.rs (new), lib.rs |
| 2 | Create invariants_34b.rs integration-test scaffold | 082286f6 | invariants_34b.rs (new) |
| 3 | Add #[ignore] placeholder tests for reset + memory-authority header | 668e94ea | context_compressor.rs, summarizing_engine.rs |

## Verification Results

```
cargo test -p ironhermes-agent --lib context_refs::tests
  → ok. 0 passed; 0 failed; 0 ignored (module compiles, empty test mod)

cargo test -p ironhermes-agent --test invariants_34b
  → ok. 0 passed; 0 failed; 1 ignored (placeholder_34b_wiring)

cargo test -p ironhermes-agent --lib test_context_compressor_reset_zeroes_counter
  → ok. 0 passed; 0 failed; 1 ignored

cargo test -p ironhermes-agent --lib test_memory_authority_header
  → ok. 0 passed; 0 failed; 1 ignored

cargo build -p ironhermes-agent
  → Finished dev profile [unoptimized + debuginfo]

grep -c 'pub mod context_refs' lib.rs → 1
grep -c 'fn test_context_compressor_reset_zeroes_counter' context_compressor.rs → 1
grep -c 'fn test_memory_authority_header' summarizing_engine.rs → 1
```

All must-haves satisfied.

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

- `crates/ironhermes-agent/src/context_refs.rs` — intentional stub; Wave 1 (Plan 01) adds `ContextReference`, `ContextReferenceResult`, `parse_context_references`, `preprocess_context_references_async`
- `test_context_compressor_reset_zeroes_counter` in `context_compressor.rs` — intentional `#[ignore]` placeholder; Wave 2 (Plan 02 Task 1) fills body
- `test_memory_authority_header` in `summarizing_engine.rs` — intentional `#[ignore]` placeholder; Wave 2 (Plan 02 Task 2) fills body

These stubs are intentional scaffolding artifacts. They do not prevent this plan's goal (establishing compiling Wave 0 scaffolds). Plans 01 and 02 resolve them.

## Threat Flags

None — this plan creates only test scaffolds and an empty module; no untrusted input processed, no path/URL/network touched.

## Self-Check: PASSED

- [x] crates/ironhermes-agent/src/context_refs.rs exists
- [x] crates/ironhermes-agent/tests/invariants_34b.rs exists
- [x] Commit e27fc3c1 exists (Task 1)
- [x] Commit 082286f6 exists (Task 2)
- [x] Commit 668e94ea exists (Task 3)
