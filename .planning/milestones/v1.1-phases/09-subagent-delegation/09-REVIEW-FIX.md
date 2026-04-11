---
phase: 09-subagent-delegation
fixed_at: 2026-04-11T18:30:00Z
review_path: .planning/phases/09-subagent-delegation/09-REVIEW.md
iteration: 2
findings_in_scope: 5
fixed: 5
skipped: 0
status: all_fixed
---

# Phase 9: Code Review Fix Report

**Fixed at:** 2026-04-11T18:30:00Z
**Source review:** .planning/phases/09-subagent-delegation/09-REVIEW.md
**Iteration:** 2

**Summary:**
- Findings in scope: 5
- Fixed: 5
- Skipped: 0

## Fixed Issues

### CR-01: Unsafe string slice at byte offset panics on multi-byte UTF-8

**Files modified:** `crates/ironhermes-agent/src/agent_loop.rs`
**Commit:** 88c5d12
**Applied fix:** Added `is_char_boundary` loop before slicing `args_str` at byte offset 100 for the tool progress preview. The truncation point now backs up to a valid char boundary before slicing, preventing panics on multi-byte UTF-8 content.

### CR-02: Unsafe string slice at byte offset panics on multi-byte UTF-8 (two locations)

**Files modified:** `crates/ironhermes-tools/src/delegate_task.rs`
**Commit:** b6648c7
**Applied fix:** Added `is_char_boundary` loop at both line 228 (`goal[..50]` in batch mode) and line 509 (`task[..50]` in single mode). Both now back up to a valid char boundary before slicing, matching the safe pattern already used in terminal.rs.

### WR-01: Schema declares "task" required but batch mode uses "tasks"

**Files modified:** `crates/ironhermes-tools/src/delegate_task.rs`
**Commit:** bbf48db
**Applied fix:** Changed schema `"required": ["task"]` to `"required": []` and added runtime validation at the top of `execute()` that bails with a clear error if neither "task" nor "tasks" is present. This allows LLM providers with strict schema validation to send either parameter.

### WR-02: Batch error collection uses wrong index for failed/panicked tasks

**Files modified:** `crates/ironhermes-tools/src/delegate_task.rs`
**Commit:** 682fffc
**Applied fix:** Changed `for handle in handles` to `for (expected_idx, handle) in handles.into_iter().enumerate()` and used `expected_idx` instead of `results.len()` in the `Ok(Err(e))` and `Err(e)` arms. This ensures error results get the correct task index regardless of arrival order.

### WR-03: check_guardrails() returns early on Warn, missing later Block

**Files modified:** `crates/ironhermes-tools/src/registry.rs`
**Commit:** b27c4b8
**Applied fix:** Changed `check_guardrails()` to accumulate Warn decisions in a `last_warn` variable and continue iterating instead of returning early. Block decisions still return immediately. After the loop, returns the last Warn if any, otherwise Allow. This matches the behavior of `dispatch_with_hook()` and ensures a Block from a later guardrail is never bypassed.

---

_Fixed: 2026-04-11T18:30:00Z_
_Fixer: Claude (gsd-code-fixer)_
_Iteration: 2_
