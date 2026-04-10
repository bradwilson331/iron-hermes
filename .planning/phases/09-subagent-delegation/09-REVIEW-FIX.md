---
phase: 09-subagent-delegation
fixed_at: 2026-04-10T18:30:00Z
review_path: .planning/phases/09-subagent-delegation/09-REVIEW.md
iteration: 1
findings_in_scope: 5
fixed: 4
skipped: 1
status: partial
---

# Phase 9: Code Review Fix Report

**Fixed at:** 2026-04-10T18:30:00Z
**Source review:** .planning/phases/09-subagent-delegation/09-REVIEW.md
**Iteration:** 1

**Summary:**
- Findings in scope: 5
- Fixed: 4
- Skipped: 1

## Fixed Issues

### CR-01: String truncation at non-char-boundary causes panic

**Files modified:** `crates/ironhermes-tools/src/terminal.rs`
**Commit:** 6ce21f8
**Applied fix:** Added `is_char_boundary()` loop to find the nearest valid UTF-8 character boundary at or before `MAX_OUTPUT_LEN` before slicing the output string. This prevents a panic when terminal output contains multi-byte UTF-8 characters (emoji, international text, binary-like data).

### WR-01: Semaphore "waited" flag is racy and may produce incorrect log messages

**Files modified:** `crates/ironhermes-tools/src/delegate_task.rs`
**Commit:** e2ead34
**Applied fix:** Replaced the racy `available_permits() == 0` check with actual wait duration measurement using `Instant::now()` before acquire and `elapsed()` after. Only logs "waited" message when actual wait exceeds 50ms, eliminating both false positives and false negatives.

### WR-02: Memory tool schema advertises write actions in read-only mode

**Files modified:** `crates/ironhermes-tools/src/memory_tool.rs`
**Commit:** fb58a68
**Applied fix:** Split `description()` and `schema()` to return read-only variants when `self.read_only` is true. The read-only schema only advertises `["get"]` as a valid action (removing `add`, `replace`, `remove` from the enum), and the description clearly states read-only constraints. Also updated the runtime error message to accurately describe how memory works in subagent context.

### WR-04: Missing `memory` tool silently ignored when no MemoryStore provided

**Files modified:** `crates/ironhermes-tools/src/delegate_task.rs`
**Commit:** 6b615e1
**Applied fix:** Added `tracing::warn!` log message in the `else` branch when `memory_store` is `None` but `"memory"` is in the allowed_tools list, so operators can diagnose why the memory tool is unavailable in child agents.

## Skipped Issues

### WR-03: Child subagent has no hook registry -- tool events are silently lost

**File:** `crates/ironhermes-agent/src/subagent_runner.rs:39`
**Reason:** Accepted by plan as a known limitation. The review itself notes this was "accepted in the plan but worth tracking." The fix requires changing the `SubagentRunner` trait signature and threading `Option<Arc<HookRegistry>>` through the trait boundary, which is an architectural change beyond the scope of a code review fix. Documented as won't-fix per plan acceptance.
**Original issue:** `AgentSubagentRunner::run_child()` creates a child `AgentLoop` without calling `.with_hook_registry()`, so child agent tool calls do not emit hook events.

---

_Fixed: 2026-04-10T18:30:00Z_
_Fixer: Claude (gsd-code-fixer)_
_Iteration: 1_
