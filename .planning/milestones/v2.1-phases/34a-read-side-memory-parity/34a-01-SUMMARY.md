---
phase: 34a-read-side-memory-parity
plan: "01"
subsystem: memory
tags: [memory, trait, rust, regex, tdd]
dependency_graph:
  requires: []
  provides: [MEM-READ-01, MEM-READ-02]
  affects: [ironhermes-core, ironhermes-agent]
tech_stack:
  added: []
  patterns: [OnceLock<Regex> static singletons, defaulted async trait no-op, primary-only read proxy]
key_files:
  created:
    - crates/ironhermes-agent/src/memory_context.rs
  modified:
    - crates/ironhermes-core/src/memory_provider.rs
    - crates/ironhermes-agent/src/memory/manager.rs
    - crates/ironhermes-agent/src/lib.rs
decisions:
  - "Regex order in sanitize_context is block -> note -> fence (Pitfall 6: reversing leaves system-note content after tag strip)"
  - "Test 4 (double_wrap_idempotency) asserts NO fence tags and NO [System note:] after sanitize — inner content is also stripped (matches Test 5 strip_full_block semantics and Python reference)"
  - "MemoryStore inherits prefetch_with_query default no-op — impl block untouched (0 lines changed in MemoryStore)"
  - "pub mod memory_context placed alphabetically between memory_flush_handler and nudge in lib.rs"
metrics:
  duration: "~30 min"
  completed: "2026-05-21T00:39:52Z"
  tasks_completed: 2
  files_modified: 4
---

# Phase 34a Plan 01: prefetch_with_query trait + memory_context.rs (MEM-READ-01/02) Summary

**One-liner:** Read-side recall query primitive (defaulted no-op trait method + primary-only proxy) and sanitize/build context-block helpers ported byte-exact from Python, secured with sanitize-before-wrap for T-34a-01/T-34a-02.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add prefetch_with_query to MemoryProvider trait + MemoryManager proxy (MEM-READ-01) | aaadd5c7 | memory_provider.rs, manager.rs |
| 2 | Create memory_context.rs with sanitize_context + build_memory_context_block + 8 tests (MEM-READ-02) | ddc1b709 | memory_context.rs, lib.rs |

## What Was Built

### Task 1: prefetch_with_query (MEM-READ-01)

Added `async fn prefetch_with_query(&self, _query: &str, _session_id: &str) -> anyhow::Result<String>` to the `MemoryProvider` trait in `ironhermes-core/src/memory_provider.rs`, placed immediately after `queue_prefetch`. Default returns `Ok(String::new())` — the file provider (`MemoryStore`) inherits this no-op with zero changes to its impl block.

Added `pub async fn prefetch_with_query(&self, query: &str, session_id: &str) -> anyhow::Result<String>` to `MemoryManager` in the "Read paths" section, copying the primary-only proxy pattern of `prefetch()` exactly (D-26/D-28: mirror is write-only, no fan-out).

Extended `default_hook_methods_return_defaults` test to assert `p.prefetch_with_query("q", "sid").await.unwrap() == ""`.

Extended `read_paths_hit_primary_only` test to assert `prefetch_with_query` returns `Ok("")` on the file provider and that the mock recorder records zero reads on the mirror.

### Task 2: memory_context.rs (MEM-READ-02)

New module `crates/ironhermes-agent/src/memory_context.rs` porting `sanitize_context` and `build_memory_context_block` from `/Users/twilson/code/hermes-agent/agent/memory_manager.py` (lines 43–187).

Three `static OnceLock<Regex>` singletons (no `lazy_static`):
- `internal_context_re`: `(?is)<\s*memory-context\s*>[\s\S]*?</\s*memory-context\s*>` — strips complete blocks including content
- `internal_note_re`: matches both system-note phrasings (`informational background data` and `authoritative reference data[^\]]*`)
- `fence_tag_re`: `(?i)</?\s*memory-context\s*>` — strips bare open/close tags

`sanitize_context` applies in order: block → note → fence (order is load-bearing per Pitfall 6).

`build_memory_context_block` returns `None` on empty/whitespace input; otherwise calls `sanitize_context(raw)` before wrapping in the byte-exact Python format with `\u{2014}` em dash (T-34a-01/T-34a-02 mitigation: provider cannot forge fence boundary or system note).

`pub mod memory_context` added to `lib.rs` alphabetically.

8 unit tests all passing.

## Test Results

```
cargo test -p ironhermes-core --lib memory_provider
  2 passed; 0 failed

cargo test -p ironhermes-agent --lib memory::manager
  7 passed; 0 failed

cargo test -p ironhermes-agent --lib memory_context::tests
  8 passed; 0 failed

cargo test -p ironhermes-agent --lib nudge::tests
  6 passed; 0 failed (regression gate)

cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load
  1 passed; 0 failed (D-12 gate)
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Test 4 (double_wrap_idempotency) plan spec contradicts Test 5 semantics**

- **Found during:** Task 2 GREEN phase
- **Issue:** Plan's Test 4 behavior block stated `sanitize_context(build_memory_context_block("fact A").unwrap())` should "contain 'fact A'" but `internal_context_re` strips the full block including inner content — the same behavior that Test 5 (strip_full_block) explicitly asserts. The two assertions are mutually exclusive with a single regex implementation.
- **Fix:** Updated Test 4 to assert absence of fence tags and `[System note:]` lines (matching the true idempotency semantics: a wrapped block fed through sanitize is fully stripped, no leakage). This matches Python reference behavior and is consistent with Test 5.
- **Files modified:** `crates/ironhermes-agent/src/memory_context.rs`
- **Commit:** ddc1b709

## Known Stubs

None. Both `sanitize_context` and `build_memory_context_block` are fully implemented. The `prefetch_with_query` no-op default is intentional (semantic providers override it; file provider inherits it as a correct no-op).

## Success Criteria Verification

- [x] `MemoryProvider::prefetch_with_query` exists with default no-op; file provider unchanged
- [x] `MemoryManager::prefetch_with_query` is a primary-only proxy
- [x] `memory_context.rs` exists; `sanitize_context` + `build_memory_context_block` ported byte-exact; 8 tests pass
- [x] `pub mod memory_context;` declared in lib.rs
- [x] Regex order in sanitize_context is block -> note -> fence (idempotency holds)
- [x] D-12 gate (`test_snapshot_frozen_after_load`) stays green
- [x] Phase 32 `nudge::tests` stays green (6/6)

## Self-Check: PASSED

All created files exist on disk. Both task commits (aaadd5c7, ddc1b709) confirmed in git log.
