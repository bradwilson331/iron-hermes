---
phase: 34b-context-system-parity
plan: "03"
subsystem: context-warnings-wiring
tags: [context-refs, warnings, out-of-band, cli, gateway, web, tdd]
dependency_graph:
  requires: [34b-02]
  provides: [WR-01-closed, context-warnings-out-of-band]
  affects:
    - crates/ironhermes-agent/src/context_refs.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/agent_runtime.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/iron_hermes_ui/src/server/state.rs
    - crates/ironhermes-agent/tests/invariants_34b.rs
tech_stack:
  added: []
  patterns:
    - Arc<StreamCallback> wrapping to retain callback reference after move into TurnRequest
    - include_str! source-guard pattern extended to cover context_refs.rs
    - TDD RED/GREEN for warnings-channel contract
key_files:
  created: []
  modified:
    - crates/ironhermes-agent/src/context_refs.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/agent_runtime.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/iron_hermes_ui/src/server/state.rs
    - crates/ironhermes-agent/tests/invariants_34b.rs
decisions:
  - "Removed in-message warnings embedding from preprocess_context_references_async (lines 880-884): the push_str block for --- Context Warnings --- was deleted; warnings flow exclusively via ContextReferenceResult.warnings Vec"
  - "Arc<StreamCallback> wrapping in run_web_turn: StreamCallback is Box<dyn Fn>, not Clone; wrapped in Arc before move into TurnRequest, retained Arc clone for post-turn warnings emission"
  - "Gateway out-of-band rendering mirrors Err arm pattern: a distinct adapter.send_message call (not appended to streamed response)"
  - "CLI run_single uses plain print!/stdout; run_chat_turn uses write_into_scroll_region to stay within scroll region conventions"
metrics:
  duration: "9 min"
  completed: "2026-05-22"
  tasks_completed: 3
  files_modified: 7
---

# Phase 34b Plan 03: WR-01 Gap Closure — Out-of-Band context_warnings Rendering Summary

**One-liner:** Removed in-message `--- Context Warnings ---` embedding from `preprocess_context_references_async` and wired all three production surfaces (CLI, gateway, web) to render `context_warnings` out-of-band after `run_turn` returns, closing WR-01 with source-guard and unit tests.

## Tasks Completed

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| 1 (RED) | Failing test — warnings must not appear in message text | d61c211e | context_refs.rs |
| 1 (GREEN) | Remove in-message warnings embedding; correct doc comments | 39e152a7 | context_refs.rs, agent_loop.rs, agent_runtime.rs |
| 2 | Render context_warnings out-of-band at all three surfaces | 3819395d | main.rs, handler.rs, state.rs |
| 3 | Source-guard tests proving WR-01 closure in invariants_34b | f0c4b303 | invariants_34b.rs |

## What Was Built

### Task 1: Stop in-message warnings embedding + correct doc comments (TDD)

**RED phase (d61c211e):** Added `test_warnings_not_in_message_text_but_on_warnings_vec` asserting:
- `result.message` does NOT contain `--- Context Warnings ---`
- `result.warnings` is non-empty when soft-limit is triggered
- `result.message` still contains `--- Attached Context ---`

Test failed as expected (pre-fix code embedded warnings in message text).

**GREEN phase (39e152a7):** Removed the `if !warnings.is_empty() { ... }` block from `preprocess_context_references_async` that pushed the warnings header into `final_msg`. Specifically removed lines 880-884:
```rust
// REMOVED:
if !warnings.is_empty() {
    let warning_lines: Vec<String> = warnings.iter().map(|w| format!("- {}", w)).collect();
    final_msg.push_str("\n\n--- Context Warnings ---\n");
    final_msg.push_str(&warning_lines.join("\n"));
}
```
The `--- Attached Context ---` block (lines 886-889) was retained intact. The `warnings` Vec continues to be populated and returned on `ContextReferenceResult.warnings`.

Doc comments corrected:
- `agent_loop.rs:71-76`: `context_warnings` doc now states it is the out-of-band carrier rendered by each surface after `run_turn` returns; removed "without per-surface preprocessing" (which implied automatic surfacing that didn't exist)
- `agent_runtime.rs:369-370`: comment updated to describe out-of-band path, not automatic surfacing

All 18 context_refs tests pass.

### Task 2: Render context_warnings out-of-band at all three surfaces (3819395d)

- **CLI `run_single`** (`main.rs` ~line 858): after scrubber tail flush, guards `!result.context_warnings.is_empty()`, builds `--- Context Warnings ---` block, prints to stdout with `print!` / `io::stdout().flush()`
- **CLI `run_chat_turn`** (`main.rs` ~line 2325): after scrubber tail flush, guards `is_empty()`, renders via `write_into_scroll_region` so it lands in the scroll region (visually out-of-band from model response)
- **Gateway `run_agent`** (`handler.rs` Ok arm ~line 1129): guards `is_empty()`, sends as a separate `adapter.send_message` call — mirrors the Err arm `error_suffix` pattern (distinct message, not appended to stream)
- **Web `run_web_turn`** (`state.rs` ~line 248): wraps `stream_callback` in `Arc<StreamCallback>` before moving into TurnRequest, retains Arc clone, calls it post-turn with the warnings block; also `tracing::warn!` for server-side visibility

### Task 3: Source-guard tests (f0c4b303)

Two new tests added to `invariants_34b.rs` (total: 7 tests, was 5):

- **`surfaces_consume_context_warnings`**: asserts MAIN_SOURCE, HANDLER_SOURCE, STATE_SOURCE each `.contains("context_warnings")` — WR-01 closure guard
- **`warnings_not_embedded_in_message_text`**: asserts CONTEXT_REFS_SOURCE does NOT contain the in-message embedding marker `final_msg.push_str("\n\n--- Context Warnings ---"` AND does still contain `--- Attached Context ---` — no-double-render contract

## Verification Results

```
cargo test -p ironhermes-agent --lib context_refs::tests  → 18 passed
cargo test -p ironhermes-agent --test invariants_34b       → 7 passed
cargo test -p ironhermes-agent --lib nudge                 → 6 passed
cargo test -p ironhermes-agent --lib streaming_scrubber    → 8 passed
cargo test -p ironhermes-agent --test invariants_33        → 8 passed
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load → 1 passed
cargo build --workspace                                     → Finished (warnings only)

grep -c 'final_msg.push_str("\n\n--- Context Warnings ---' context_refs.rs → 0 ✓
grep -c '--- Attached Context ---' context_refs.rs → 7 ✓
grep -c 'without per-surface preprocessing' agent_loop.rs → 0 ✓
grep -c context_warnings main.rs → 6 ✓ (>= 2)
grep -c context_warnings handler.rs → 3 ✓ (>= 1)
grep -c context_warnings state.rs → 6 ✓ (>= 1)
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Accidental edit to main repo instead of worktree**
- **Found during:** Task 1 RED phase
- **Issue:** Initial Edit call targeted the main repo's `context_refs.rs` (the test ran against the main repo binary) instead of the worktree copy
- **Fix:** Reverted main repo with `git checkout --` and re-applied the edit to the correct worktree path `/Users/twilson/code/ironhermes/.claude/worktrees/agent-acbc8ec03bad47c8c/`
- **Files modified:** None (corrected process)
- **Commit:** d61c211e (correctly targeted)

**2. [Rule 2 - Missing critical functionality] Arc wrapping for StreamCallback in web surface**
- **Found during:** Task 2 web surface implementation
- **Issue:** `StreamCallback` is `Box<dyn Fn(&str) + Send + Sync>` — not Clone. Once moved into `TurnRequest.stream`, it's consumed. The plan said to invoke `stream_callback` after `run_turn` for the web surface, but that requires retaining a reference.
- **Fix:** Wrapped `stream_callback` in `Arc<StreamCallback>` before creating the forwarding `Box` moved into TurnRequest; retained the `Arc` clone for post-turn warnings emission. This is the minimal change that preserves ownership without altering TurnRequest's API.
- **Files modified:** `crates/iron_hermes_ui/src/server/state.rs`
- **Commit:** 3819395d

## Known Stubs

None — all three surfaces fully wired to render warnings when present.

## Threat Flags

None — no new network endpoints, auth paths, file access patterns, or schema changes introduced. The warnings text is agent-generated operational metadata (blocklist hits, budget violations), not untrusted external input.

## Self-Check: PASSED

- [x] `crates/ironhermes-agent/src/context_refs.rs` — modified, exists
- [x] `crates/ironhermes-agent/src/agent_loop.rs` — modified, exists
- [x] `crates/ironhermes-agent/src/agent_runtime.rs` — modified, exists
- [x] `crates/ironhermes-cli/src/main.rs` — modified, exists
- [x] `crates/ironhermes-gateway/src/handler.rs` — modified, exists
- [x] `crates/iron_hermes_ui/src/server/state.rs` — modified, exists
- [x] `crates/ironhermes-agent/tests/invariants_34b.rs` — modified, exists
- [x] Commit d61c211e exists (RED test)
- [x] Commit 39e152a7 exists (GREEN implementation)
- [x] Commit 3819395d exists (surface wiring)
- [x] Commit f0c4b303 exists (source-guard tests)
