---
status: partial
phase: 18-context-compression
source:
  - 18-01-SUMMARY.md
  - 18-02-SUMMARY.md
  - 18-03-SUMMARY.md
  - 18-04-SUMMARY.md
  - 18-05-SUMMARY.md
  - 18-06-SUMMARY.md
  - 18-07-SUMMARY.md
  - 18-08-SUMMARY.md
  - 18-09-SUMMARY.md
started: 2026-04-12T00:00:00Z
updated: 2026-04-13T04:10:00Z
---

## Current Test

[live UAT 2026-04-13T23:44 (post 18-10 post-ship fix): Test 5 passes with `protect_first_n=2`; 10 consecutive successful compressions, zero `pair_atomicity_collapsed_range` warns, stable single pinned `[CONTEXT HISTORY]`. Two new findings surfaced — see Gaps.]

## Tests

### 1. Cold Start Smoke Test
expected: Kill any running ironhermes server/agent. From a clean start, `cargo build --workspace` succeeds and the agent/gateway binaries boot without errors. A primary command (e.g., sending a simple chat turn through the gateway) returns a live response.
result: pass

### 2. Agent Compression Triggers at 50%
expected: With `agent.compression_threshold = 0.50`, run a conversation until estimated tokens cross 50% of the configured context_length. Compression fires: logs show `context:pre_compress` event, message count drops, conversation continues without error.
result: pass
verified: "2026-04-13 live UAT. Config: agent.compression_threshold=0.001, protect_last_tokens=100, aux compression role=claude-haiku-4-5. Logs show `context compression check ratio=... threshold=0.001` followed by `summarizing_engine: compress attempt before_tokens=...` on every turn once tokens exceed the threshold. Wiring gap (18-08/18-09) resolved."

### 3. Gateway Per-Turn Compression at 85%
expected: With `gateway.compression_threshold = 0.85`, send a turn with a prompt whose token estimate exceeds 85% of context_length. Gateway-side compression runs (per-turn hygiene log), request still reaches upstream successfully. Below 85%, no compression runs.
result: deferred
reason: "Live UAT on 2026-04-13 exercised the agent-side path (test 2) which now works. Gateway-side path relies on the same wiring fix (18-08) and is structurally verified in 18-VERIFICATION.md, but a live gateway (Telegram) UAT was not run this session. Reclassify to `pass` after a gateway-mode repro."

### 4. Pressure Warning at 85% of Threshold
expected: As usage climbs past 85% of the engine's compression threshold (but before actual compression), a `tracing::warn!` fires, a `ContextPressure` hook event is emitted, and a one-shot transient system message `[CONTEXT PRESSURE HIGH — earlier history may soon be summarized]` is injected into the next model call. Re-crossing without descent does NOT re-fire; descending and re-crossing does.
result: not_exercised
reason: "Config used for 2026-04-13 UAT (threshold=0.001) skips the pressure band — compression fires before any pre-compression pressure window exists. Needs a separate UAT pass with a realistic threshold (e.g. 0.50) where usage can sit in the 0.425–0.50 band."

### 5. Tool-Pair Atomicity (No Orphaned Calls)
expected: Run a session with tool calls so compression triggers mid-pair. Resulting message list has zero orphaned tool_use/tool_result messages — no API 400 errors from the provider. Adaptive 500-token shift visibly keeps pairs together.
result: pass
verified: "2026-04-13T23:44 live UAT (agent CLI, web_read loop). 10/10 consecutive compressions succeeded (outcome=\"compressed\"), zero pair_atomicity_collapsed_range warns, zero Anthropic 400s. Token reduction stable ~2780/pass (3444→665). Config used: compression.protect_first_n=2, protect_last_tokens=100, threshold=0.001, aux=claude-haiku-4-5."
caveat: "Pass required dropping protect_first_n from the documented default 3 to 2 — see new gap: default-config compression deadlock."

### 6. SummarizingEngine Single Pinned [CONTEXT HISTORY]
expected: With `context_engine = "summarizing"`, drive multiple compression passes. The message list always contains exactly one `[CONTEXT HISTORY]` system message (name=`context_history`), not two. Body reflects updated summary including the newly pruned blocks.
result: pass
verified: "2026-04-13T23:44 live UAT. Post-compression message count was stable at 3 across all 10 passes (sys + user + history). A second pinned history block would have produced messages=4 — not observed. Iterative re-compression (compression_count=1..10) overwrote the single [CONTEXT HISTORY] in place."

### 7. Aux-Model Fallback to LocalPruning
expected: With `context_engine = "summarizing"` but aux-model call forced to fail (misconfigured compression role or network error), compression still succeeds via LocalPruningEngine fallback. Logs show warn about summarization failure; conversation continues; no user-visible error.
result: blocked
blocked_by: 18-10
reason: "The 2026-04-13 UAT with aux model claude-haiku-4-5 configured never reached the fallback path — OrphanedToolPair fires before the aux call returns. Retest after 18-10; then also run with a misconfigured compression role to exercise the actual fallback branch."

### 8. Memory Flush Before Prune
expected: With a MemoryProvider registered, observe log/trace ordering on a compression event: `sync_turn` (memory flush) completes BEFORE destructive prune runs. No memory entries lost across compression boundary.
result: blocked
blocked_by: 18-10
reason: "Destructive prune never reaches the message list — rollback replaces it before any inserts commit. Retest once compression actually succeeds."

### 9. SystemMessage Prompt Slot Wiring
expected: With `config.agent.system_message = "You are a test."`, inspect the assembled prompt — a SystemMessage slot appears between Identity and ToolGuidance, content is scanned (no injection markers) and capped at 20K chars. Empty config omits the slot entirely.
result: skipped
reason: "Deferred; not blocking. Orthogonal to compression wiring fix."

## Summary

total: 9
passed: 4
partial_pass: 0
issues: 0
pending: 0
blocked: 2
not_exercised: 1
deferred: 1
skipped: 1

last_run: 2026-04-13T23:44 (live, agent CLI, post 18-10 post-ship fix)

## Gaps

- truth: "Agent loop compresses when estimated_tokens / context_length >= agent.compression_threshold"
  status: resolved
  resolution: "Wiring gap closed in 18-09 (agent_wiring::attach_context_engine at 3 production sites). Verified live 2026-04-13 — compression fires every turn at the configured threshold."
  test: 2

- truth: "Gateway per-turn hygiene runs when ratio >= gateway.compression_threshold"
  status: resolved_structural
  resolution: "Wiring gap closed in 18-08 (GatewayRunner::start now calls set_gateway_engine). Structurally verified in 18-VERIFICATION.md; live gateway UAT deferred."
  test: 3

- truth: "SummarizingEngine keeps tool_use/tool_result pairs together across the prune boundary"
  status: resolved
  resolution: "18-10 root-cause fix (42eb073 → 4eade57 → a45e511) + post-ship two-direction guard (b123179 → 28caf61). Verified live 2026-04-13T23:44 — 10/10 consecutive compressions succeeded, zero pair_atomicity_collapsed_range warns, token reduction 3444→665 per pass."
  test: 5

- truth: "Default compression.protect_first_n=3 is safe for single-tool-pair conversations"
  status: failed
  severity: high
  reason: "With protect_first_n=3 and a [sys, user, asst(tool_use), tool_result] shape, the asst is pinned in front-protect while the tool_result is the only prunable message. Two-direction guard correctly detects the front-straddle and pushes prune_start past max(result_idx)+1, collapsing prune_start==prune_end. Result: `pair_atomicity_collapsed_range` warn on every turn, zero compression, unbounded token growth until agent context is exhausted."
  impact: "Documented default (protect_first_n=3) is unsafe. UAT only passed after lowering to 2. Must auto-extend protect_first_n to cover dangling tool-pair results OR auto-shrink to exclude a pinned asst tool_use whose result falls outside."
  test: 5 (re-exposed under default config)
  root_cause: "protect_first_n is a pure index count; it does not consider tool-pair atomicity when selecting its boundary."
  artifacts:
    - crates/ironhermes-agent/src/summarizing_engine.rs (compute_protect_start + front-straddle guard at lines 337–372)
    - live UAT log 2026-04-13T23:35 (path-1 reconfig attempt failed with protect_first_n=3)
  owner: 18-11

- truth: "After compression, the model recognizes its prior tool call completed and does not retry"
  status: failed
  severity: medium
  reason: "Live UAT 2026-04-13T23:44: agent re-called web_read on every turn for 10 consecutive turns, never emitting a final summary to the user, until MAX_COMPRESSION_PASSES=10 guard fired. Each compression replaced the prior asst(tool_use)+tool_result pair with a [CONTEXT HISTORY] summary that does not carry enough signal for the model to recognize the task was completed."
  impact: "Compression is technically functional but semantically breaks the agent's sense of progress. Without an explicit 'already executed' signal in the summary, the model treats every turn as a fresh request."
  test: new (discovered during Test 5 re-run)
  root_cause_hypothesis: "Summary prompt / aux-model output does not preserve tool-call outcome markers. Or: summary positioned as system message competes with the original user message which still reads as an open request."
  artifacts:
    - live UAT log 2026-04-13T23:44 (10 consecutive compress-then-re-fetch cycles)
    - crates/ironhermes-agent/src/summarizing_engine.rs (summary prompt construction)
  owner: 18-12

- truth: "Tests 7, 8 require a configurable compression pass end-to-end"
  status: blocked
  blocked_by: 18-11
  reason: "Tests 7 (aux fallback) and 8 (memory flush ordering) require a clean compression pass under default or near-default config. Currently only passes with protect_first_n=2. Retest after 18-11 lands."
  test: [7, 8]
