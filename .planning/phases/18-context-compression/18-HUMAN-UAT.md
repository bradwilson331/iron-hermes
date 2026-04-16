---
status: complete
phase: 18-context-compression
source: [18-VERIFICATION.md]
started: 2026-04-14
updated: 2026-04-16
---

## Current Test

[testing complete]

## Tests

### 1. Plan 18-14 Task 5 — Live REPL hysteresis (3 consecutive turns)
expected: At `agent.compression_threshold=0.05`, three consecutive CLI prompts in the `[0.0425, 0.05)` pressure band produce:
- Exactly ONE `WARN context pressure warning (85% of compression threshold)` log line across the whole session (not per turn).
- Turn 2's outbound message vector contains a system message whose body starts with `[CONTEXT PRESSURE HIGH — earlier history may soon be summarized]`.
- `compression_count=N` increments monotonically (1, 2, 3…) across turns rather than resetting.

Run command: `cargo run -p ironhermes-cli --features memory-sqlite`
Test payload for turn 1: a tool-heavy prompt that pushes ratio past 0.05; turns 2 and 3 keep ratio in the band without descending below 0.0425.
result: pass

### 2. UAT Test 3 — Gateway per-turn compression (live Telegram)
expected: At `agent.compression_threshold=0.85` via gateway, multi-turn Telegram session exercises compression and pressure warning paths. Structurally verified in 18-VERIFICATION.md but never live-exercised.
result: issue
reported: "Error: Memory provider 'sqlite' requires a feature flag that is not enabled. Available providers: file"
severity: blocker

## Summary

total: 2
passed: 1
issues: 1
pending: 0
skipped: 0
blocked: 0

## Gaps

- truth: "Gateway starts with agent.compression_threshold=0.85 and memory.provider=sqlite to allow live Telegram compression exercise"
  status: failed
  reason: "User reported: Error: Memory provider 'sqlite' requires a feature flag that is not enabled. Available providers: file"
  severity: blocker
  test: 2
  root_cause: "CLI main.rs:610 (run_gateway path) calls the deprecated build_memory_provider from ironhermes-core instead of the feature-gated ironhermes_agent::memory::factory::build_memory_provider. The core version at memory_provider.rs:145-151 hardcodes a non-feature-gated bail for 'sqlite'/'grafeo'/'duckdb' and reports 'Available providers: file', ignoring the --features memory-sqlite flag."
  artifacts:
    - path: "crates/ironhermes-cli/src/main.rs"
      issue: "Line 5 imports build_memory_provider from ironhermes_core; line 610 calls it in run_gateway. Should import/call ironhermes_agent::memory::factory::build_memory_provider."
    - path: "crates/ironhermes-core/src/memory_provider.rs"
      issue: "Lines 135-159 contain deprecated non-feature-gated factory still re-exported from lib.rs:19. Should be removed or made private now that the agent-side factory is the canonical path."
    - path: "crates/ironhermes-core/src/lib.rs"
      issue: "Line 19 re-exports deprecated build_memory_provider — consumers still pick up the wrong symbol."
  missing:
    - "Switch run_gateway to call ironhermes_agent::memory::factory::build_memory_provider (feature-gated)."
    - "Remove the #[deprecated] fallback from ironhermes-core or stop re-exporting it so the compiler surfaces any lingering callers."
    - "Verify REPL path uses the agent factory (test 1 passed, so REPL may already be correct — confirm to narrow scope)."
  debug_session: ""
