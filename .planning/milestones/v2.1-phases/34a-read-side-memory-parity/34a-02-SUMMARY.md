---
phase: 34a-read-side-memory-parity
plan: "02"
subsystem: memory
tags: [memory, streaming, agent-loop, recall-injection, scrubber]
dependency_graph:
  requires: ["34a-01"]
  provides: ["MEM-READ-03", "MEM-READ-04", "MEM-READ-05"]
  affects: ["agent_loop", "context_compressor", "streaming_scrubber", "cli", "gateway", "ws"]
tech_stack:
  added: ["streaming_scrubber.rs (new module)"]
  patterns:
    - "Arc<std::sync::Mutex<StreamingContextScrubber>> for Fn closure + post-stream flush"
    - "Scoped tokio MutexGuard drop before Vec::insert (borrow checker safety)"
    - "Vec::retain for ephemeral message eviction (pre-turn + compressor step 0)"
    - "#[serde(skip)] wire-transparent bool field on ChatMessage"
key_files:
  created:
    - crates/ironhermes-agent/src/streaming_scrubber.rs
  modified:
    - crates/ironhermes-core/src/types.rs
    - crates/ironhermes-agent/src/context_compressor.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/iron_hermes_ui/src/server/ws.rs
    - crates/ironhermes-agent/src/anthropic_client.rs
    - crates/ironhermes-agent/src/any_client.rs
    - crates/ironhermes-agent/src/summarizing_engine.rs
    - crates/ironhermes-state/tests/tool_pair_round_trip.rs
    - crates/ironhermes-cli/tests/provider_integration.rs
decisions:
  - "D-01: is_recall_context = #[serde(skip)] bool — wire-transparent flag, never serialized"
  - "D-02: pre-turn recall injection order — retain BEFORE rposition BEFORE insert (Pitfall 3)"
  - "D-03: compressor step 0 — retain strips recall before token estimation"
  - "D-08: skip inject when build_memory_context_block returns None (file-provider parity)"
metrics:
  duration: "~35 min"
  completed: "2026-05-21"
  tasks: 3
  files: 13
---

# Phase 34a Plan 02: End-to-End Recall Injection + Streaming Scrubber Summary

Wire the per-turn semantic recall path end-to-end: `is_recall_context` flag on `ChatMessage`, pre-turn recall injection in the agent loop, compressor step-0 eviction, `StreamingContextScrubber` module with 6 tests, and scrubber wired into all three streaming surfaces (CLI, gateway, web UI).

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | ChatMessage.is_recall_context + recall_system constructor; compressor step 0 | e5665142 | types.rs, context_compressor.rs + 8 call-site fixes |
| 2 | Create streaming_scrubber.rs (StreamingContextScrubber feed/flush/reset) with 6 tests | e795c65e | streaming_scrubber.rs, lib.rs |
| 3 | Pre-turn recall injection in agent_loop + scrubber wiring into all 3 streaming surfaces | 4a4a3cec | agent_loop.rs, main.rs, handler.rs, ws.rs |

## What Was Built

### Task 1 — ChatMessage schema + compressor step 0

Added `#[serde(skip)] pub is_recall_context: bool` as the last field of `ChatMessage`. The `#[serde(skip)]` attribute omits the field from both serialization and deserialization — no sentinel string, no wire leakage (D-01). Added `ChatMessage::recall_system()` constructor immediately after `system()`. Updated all 5 existing constructors and all 13 direct struct initializations across the workspace (agent_loop, any_client, anthropic_client, summarizing_engine, handler, state.rs, tool_pair_round_trip, provider_integration tests) with `is_recall_context: false`.

Prepended step 0 to `ContextCompressor::compress()`: `messages.retain(|m| !m.is_recall_context)` runs unconditionally before the `should_compress` check, freeing recall tokens first when context is tight (D-03). Added `compress_step0_evicts_recall_messages` test.

### Task 2 — StreamingContextScrubber

Created `crates/ironhermes-agent/src/streaming_scrubber.rs` ported from the Python `StreamingContextScrubber` state machine. Struct holds `in_span: bool` and `buf: String`. The `feed()` method prepends any held partial-tag tail to each delta, then uses a state machine loop to strip `<memory-context>...</memory-context>` spans. Partial tags at chunk boundaries are held in `buf` until the next delta confirms or disproves them. `flush()` discards an unterminated span rather than leaking it (Test 5 / T-34a-04). Case-insensitive via `.to_lowercase()` throughout (Pitfall 7). `max_partial_suffix()` finds the longest buf-suffix that is a tag-prefix to determine how much to hold back.

6 tests all pass: full_block_in_one_delta, split_open_tag, split_close_tag, partial_tail_held, span_never_closes_flush_returns_empty, two_complete_blocks_back_to_back.

### Task 3 — Agent loop injection + 3-surface wiring

**Agent loop:** Pre-turn recall block inserted after compression, before `turns_used += 1`. Order follows D-02: (1) `messages.retain(|m| !m.is_recall_context)` evicts prior recall unconditionally (so index scans are correct — Pitfall 3). (2) If memory_manager wired, rev-find last user message text. (3) Scoped block acquires and drops the tokio MutexGuard before `messages.insert()` (Pitfall 1). (4) `prefetch_with_query` fetches recall. (5) `build_memory_context_block` wraps and sanitizes; returns `None` on empty, skipping inject (D-08). (6) `rposition` finds last user message index; `messages.insert(idx, ChatMessage::recall_system(block))` injects immediately before it.

**CLI:** `run_single` and `run_chat` both wired. `Arc::new(std::sync::Mutex::new(StreamingContextScrubber::new()))` created before the `AgentLoop` builder; `Arc::clone` moves into `with_streaming` closure; `flush()` called on the outer `Arc` after `agent.run().await` returns.

**Gateway:** Same pattern in `handle_with_multimodal`. `stream_callback` closure feeds through the scrubber; `flush()` emits any tail via `stream_tx.try_send()` before `drop(agent)`.

**Web UI:** `scrubber_ws` created inside the `tokio::spawn` async block; `Arc::clone` moved into `stream_callback`; `flush()` via `tx.send(ChatStreamEvent::Delta { text: tail })` after `run_web_turn` awaits.

## Verification Results

| Test Suite | Result |
|------------|--------|
| streaming_scrubber::tests | 6/6 PASSED |
| context_compressor tests | 4/4 PASSED (includes new recall-eviction test) |
| agent_loop tests | 66/66 PASSED |
| memory_context::tests (from 34a-01) | 8/8 PASSED |
| nudge::tests (Phase 32 gate) | 6/6 PASSED |
| invariants_33 (Phase 33 gate) | 8/8 PASSED |
| test_snapshot_frozen_after_load (D-12 gate) | PASSED |
| cargo build --workspace | CLEAN (no errors) |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical functionality] Wired scrubber into run_chat (in addition to run_single)**
- **Found during:** Task 3
- **Issue:** Plan specified "CLI run_chat" as a surface but only described the `run_single` path in detail. The `run_chat` interactive REPL path (~line 2317) also has its own `with_streaming` closure and `agent.run()` call and would have leaked `<memory-context>` tags in interactive chat sessions.
- **Fix:** Added the same `Arc<Mutex<StreamingContextScrubber>>` pattern to both `run_single` (lines 817-883) and the per-turn `run_chat` helper function (lines 2311-2402).
- **Files modified:** `crates/ironhermes-cli/src/main.rs`
- **Commit:** 4a4a3cec

**2. [Rule 1 - Bug fix] Fixed all direct ChatMessage struct initializations across workspace**
- **Found during:** Task 1
- **Issue:** Adding `is_recall_context` to `ChatMessage` caused compile errors on every direct struct initialization that didn't list all fields. There were 13 such sites across 7 files beyond the 5 constructors listed in the plan.
- **Fix:** Added `is_recall_context: false` to each site.
- **Files modified:** anthropic_client.rs, any_client.rs, summarizing_engine.rs, handler.rs, state.rs, tool_pair_round_trip.rs, provider_integration.rs
- **Commit:** e5665142

## Known Stubs

None — all wiring is complete. The `prefetch_with_query` trait method has a default no-op `Ok(String::new())` implementation (from 34a-01), which correctly causes `build_memory_context_block` to return `None` on file-only providers — no inject happens, session buffer is byte-identical to pre-34a (D-08).

## Threat Flags

No new trust boundaries introduced beyond what is documented in the plan's threat model. All T-34a-04 (streaming delta scrubbing), T-34a-05 (recall message stacking), T-34a-06 (D-12 frozen snapshot), and T-34a-07 (empty-recall DoS) mitigations are implemented as planned.

## Self-Check: PASSED

| Item | Status |
|------|--------|
| streaming_scrubber.rs | FOUND |
| types.rs | FOUND |
| agent_loop.rs | FOUND |
| Commit e5665142 (Task 1) | FOUND |
| Commit e795c65e (Task 2) | FOUND |
| Commit 4a4a3cec (Task 3) | FOUND |
| SUMMARY.md | FOUND |
