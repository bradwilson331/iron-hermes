---
status: partial
phase: 34b-context-system-parity
source: [34b-VERIFICATION.md]
started: 2026-05-22T12:30:00Z
updated: 2026-05-22T13:55:00Z
---

## Current Test

[awaiting human testing]

## Tests

### 1. CLI scroll region visual separation
expected: The `--- Context Warnings ---` block appears visually separate from the model's response text in the TUI scroll region (not embedded in model output). Reproduce by sending `@file:~/.ssh/id_rsa` (a blocklisted path) in a REPL session.
result: [pending]

### 2. Gateway distinct message
expected: Warnings arrive as a separate adapter message (not appended to the streamed response). Test via Telegram or another gateway adapter by sending a message containing a blocked/budget-exceeding `@`-reference.
result: [pending]

### 3. Web stream_callback annotation
expected: The `Arc<StreamCallback>` post-turn invocation delivers the warnings block as a distinct streamed annotation after the model response in the web UI.
result: [pending]

## Summary

total: 3
passed: 0
issues: 0
pending: 3
skipped: 0
blocked: 0

## Gaps

### WR-01: context_warnings not consumed by any surface
status: resolved
source: 34b-VERIFICATION.md, 34b-REVIEW.md
detail: `AgentResult.context_warnings` was populated in `run_turn` but no production surface read it; warnings reached users only via in-message embedding by `preprocess_context_references_async`.
resolution: Closed by plan 34b-03 — removed the in-message `--- Context Warnings ---` embedding; CLI (run_single + run_chat_turn), gateway (run_agent), and web (run_web_turn) now read `result.context_warnings` and render the block out-of-band, guarded by `is_empty()`. Doc comments corrected; two invariants_34b source-guard tests pin the contract.
