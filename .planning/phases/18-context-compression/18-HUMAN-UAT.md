---
status: partial
phase: 18-context-compression
source: [18-VERIFICATION.md]
started: 2026-04-14
updated: 2026-04-14
---

## Current Test

[awaiting human testing]

## Tests

### 1. Plan 18-14 Task 5 — Live REPL hysteresis (3 consecutive turns)
expected: At `agent.compression_threshold=0.05`, three consecutive CLI prompts in the `[0.0425, 0.05)` pressure band produce:
- Exactly ONE `WARN context pressure warning (85% of compression threshold)` log line across the whole session (not per turn).
- Turn 2's outbound message vector contains a system message whose body starts with `[CONTEXT PRESSURE HIGH — earlier history may soon be summarized]`.
- `compression_count=N` increments monotonically (1, 2, 3…) across turns rather than resetting.

Run command: `cargo run -p ironhermes-cli --features memory-sqlite`
Test payload for turn 1: a tool-heavy prompt that pushes ratio past 0.05; turns 2 and 3 keep ratio in the band without descending below 0.0425.
result: [pending]

### 2. UAT Test 3 — Gateway per-turn compression (live Telegram)
expected: At `agent.compression_threshold=0.85` via gateway, multi-turn Telegram session exercises compression and pressure warning paths. Structurally verified in 18-VERIFICATION.md but never live-exercised.
result: [pending]

## Summary

total: 2
passed: 0
issues: 0
pending: 2
skipped: 0
blocked: 0

## Gaps
