---
status: partial
phase: 34b-context-system-parity
source: [34b-VERIFICATION.md, 34b-REVIEW.md]
started: 2026-05-22T12:30:00Z
updated: 2026-05-22T12:30:00Z
---

## Current Test

[awaiting human testing]

## Tests

### 1. context_warnings surface rendering (WR-01)
expected: When a user sends a message containing a blocked or budget-exceeding `@`-reference (e.g. `@file:~/.ssh/id_rsa`), the resulting `--- Context Warnings ---` block is visible to the user in the response.
result: [pending]
note: Functionally the warning DOES reach the user — `preprocess_context_references_async` embeds the `--- Context Warnings ---` block directly into the message text. However, `AgentResult.context_warnings` is populated by `run_turn` but read by NO production surface (CLI / gateway / web). The doc comments promise out-of-band rendering from that field, which does not exist. Decision needed: (a) accept in-message delivery and update the doc comments to match, or (b) wire each surface to consume `AgentResult.context_warnings` for out-of-band rendering. Does not block functionality.

## Summary

total: 1
passed: 0
issues: 0
pending: 1
skipped: 0
blocked: 0

## Gaps
