---
phase: 10-batch-processing
plan: "03"
subsystem: batch-processing
tags: [gap-closure, uat-fix, quality-filters, cancel-resume, secrets-detection]
dependency_graph:
  requires: ["10-01", "10-02"]
  provides: ["cancel-resume-fix", "tightened-no-reasoning", "assistant-secrets-scanning"]
  affects: ["batch-runner", "batch-filters"]
tech_stack:
  added: []
  patterns: ["sentinel-cleanup-on-start", "role-based-message-scanning"]
key_files:
  created: []
  modified:
    - crates/ironhermes-cli/src/batch/runner.rs
    - crates/ironhermes-cli/src/batch/filters.rs
    - crates/ironhermes-cli/src/batch/tests.rs
decisions:
  - "Tool calls required for filter_no_reasoning pass -- text-only responses always rejected in batch mode"
  - "Secrets filter scans both Role::Tool and Role::Assistant messages"
metrics:
  duration: "108s"
  completed: "2026-04-10"
  tasks: 1
  files: 3
---

# Phase 10 Plan 03: UAT Gap Closure Summary

Close 3 UAT gaps: stale cancel sentinel cleared at run start, no_reasoning filter requires tool calls, secrets filter scans assistant messages.

## What Was Done

### Task 1: Fix stale cancel sentinel + tighten filters (TDD)

**RED:** Added 3 failing tests:
- `test_filter_no_reasoning_rejects_text_only` (renamed from passes_with_text, assertion inverted)
- `test_filter_secrets_detects_in_assistant_text` (new -- AWS key in assistant message)
- `test_run_filters_rejects_text_only_no_tools` (new -- integration test for text-only rejection)

**GREEN:** Applied 3 fixes:
1. **runner.rs line 120:** Added `let _ = std::fs::remove_file(&cancel_path);` after cancel_path definition, before dispatch loop. Stale cancel sentinels from prior `batch cancel` calls no longer block resume.
2. **filters.rs filter_no_reasoning:** Removed text-length fallback. Now requires at least one tool call to pass. Text-only responses like "How can I help?" are rejected.
3. **filters.rs filter_secrets_in_output:** Extended role check from `Role::Tool` only to `Role::Tool || Role::Assistant`. Secrets echoed by the model in assistant messages are now caught.

**Result:** All 27 batch tests passing. Zero build errors.

## Commits

| Commit | Type | Description |
|--------|------|-------------|
| d6f4e1e | test | Add failing tests for UAT gap fixes (TDD RED) |
| f44a5a4 | feat | Fix UAT gaps - cancel resume, no_reasoning filter, assistant secrets (TDD GREEN) |

## Deviations from Plan

None -- plan executed exactly as written.

## Verification

```
cargo test -p ironhermes-cli batch: 27 passed, 0 failed
cargo build: zero errors, zero unused warnings
grep remove_file.*cancel_path runner.rs: found at line 120
grep Role::Assistant filters.rs: found at lines 89, 92
```

## Threat Surface

| Threat ID | Category | Disposition | Status |
|-----------|----------|-------------|--------|
| T-10-01 | Information Disclosure | mitigate | RESOLVED - filter_secrets_in_output now scans Role::Assistant |
| T-10-06 | Denial of Service | mitigate | RESOLVED - stale cancel sentinel cleaned at cmd_run start |

## Self-Check: PASSED
