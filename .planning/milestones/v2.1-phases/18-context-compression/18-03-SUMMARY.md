---
phase: 18-context-compression
plan: 03
subsystem: api
tags: [rust, context-compression, summarization, aux-model]

requires:
  - phase: 18-context-compression
    provides: ContextEngine trait + LocalPruningEngine (18-01) + tool_pair primitives (18-02)
  - phase: 12-provider-resolution
    provides: AnyClient + build_role_client("compression") aux-model routing
provides:
  - SummarizingEngine (agent-side ContextEngine, Soft mode)
  - SummarizationClient trait + AnyClientSummarizer production impl
  - HISTORY_SENTINEL / HISTORY_NAME constants + locate_history_segment helper
  - Iterative re-compression (single pinned segment, replace-in-place)
  - LocalPruningEngine fallback on SummarizationFailed
affects: [18-06]

tech-stack:
  added: []
  patterns: [async_trait SummarizationClient for mock-friendly aux-model calls]

key-files:
  created:
    - crates/ironhermes-agent/src/summarizing_engine.rs
  modified:
    - crates/ironhermes-agent/src/lib.rs

key-decisions:
  - "SummarizationClient trait abstracts aux-model — production wraps AnyClient::chat_completion, tests use in-memory mock"
  - "Iterative re-compression mutates the single pinned segment in place; never produces two [CONTEXT HISTORY] messages"
  - "Fallback to LocalPruningEngine on SummarizationFailed — compression never fails upward (T-18-03)"
  - "8K char cap on summary body pre-pin (T-18-01 mitigation), 4K char cap per pruned block in prompt assembly"
  - "D-24 MAX_COMPRESSION_PASSES=10 runaway-loop guard returns default outcome when exceeded"

patterns-established:
  - "SummarizingEngine::compress: threshold check → adaptive shift → locate prior pin → serialize pruned blocks → aux-model call (with fallback) → replace pin in place → invariant check"
  - "Pinned system-role history message located by stable name field, not content string matching"

requirements-completed: [PRMT-15, PRMT-12]

duration: ~20min
completed: 2026-04-12
---

# Phase 18 Plan 03: SummarizingEngine

**Agent-side ContextEngine using an auxiliary LLM to maintain a single pinned `[CONTEXT HISTORY]` segment with iterative re-compression.**

## Performance

- **Tasks:** 1
- **Files modified:** 2 (1 created + 1 edited)
- **Tests:** 5 new unit tests green (125 total `ironhermes-agent --lib` tests pass)
- **Completed:** 2026-04-12

## Accomplishments

- `SummarizingEngine` implementing `ContextEngine` with `CompressionMode::Soft`
- Single pinned `[CONTEXT HISTORY]` system message with `name="context_history"` at `protect_first_n`
- Iterative formula `NewSummary = Summarize(OldSummary + NewPrunedBlocks)` replaces the pin in place
- `SummarizationClient` trait + `AnyClientSummarizer` production impl over `AnyClient::chat_completion`
- `LocalPruningEngine` fallback on summarization failure (T-18-03 mitigation)
- Adaptive tool-pair shift (D-15) and post-compression orphan invariant (D-16) reused from Plan 02
- T-18-01 mitigation via 8K char summary cap and 4K char per-block truncation before prompt assembly
- D-24 runaway-loop guard at 10 passes

## Task Commits

1. **Task 1: SummarizingEngine + pinned history segment** — `6138982` (feat)

## Files Created/Modified

- `crates/ironhermes-agent/src/summarizing_engine.rs` — new module (~500 LOC with tests)
- `crates/ironhermes-agent/src/lib.rs` — `pub mod summarizing_engine;`

## Decisions Made

- **Abstracted summarizer via trait**: `SummarizationClient` lets tests mock without touching the network or needing a configured provider. `AnyClientSummarizer` is the production impl.
- **In-place pin replacement**: the pinned history segment is detected by stable `name` field (not content substring) and replaced rather than appended, ensuring exactly one segment always.
- **Fallback on failure**: any `ContextError::SummarizationFailed` from the aux model delegates to `LocalPruningEngine::compress` — compression is never a failure mode for the caller (T-18-03).
- **Runaway-loop guard at engine level** (D-24): returns default outcome with a warn log when `stats.compression_count >= 10`.

## Deviations from Plan

None structural. The plan's `m.role == "system"` string check was mapped to `Role::System` enum match to fit the actual `ChatMessage` type (Role is an enum in this codebase). This is the only idiomatic adaptation.

## Issues Encountered

- Worktree was reset-soft at start (per `worktree_branch_check`). Phase 18 planning files (`18-03-PLAN.md`, `18-CONTEXT.md`, `18-RESEARCH.md`) had to be copied in from the main repo into the worktree; they remain untracked in this worktree and are owned by the orchestrator.
- `cargo clippy -- -D warnings` at workspace level fails on three pre-existing lints in `ironhermes-core` (manual `Default`, manual `is_multiple_of`). Out-of-scope per scope-boundary rule — not fixed here, flagged for a separate cleanup.

## Next Phase Readiness

- 18-06 (engine wiring) can now construct `SummarizingEngine::new(ctx_len, threshold, Arc::new(AnyClientSummarizer::new(aux_client, model)))` and mount it on the agent loop.
- 18-04/18-05 (hook events + pressure warning) can consume the `CompressionOutcome { new_summary, tokens_freed }` from successful summarization passes.

## Self-Check: PASSED

- `crates/ironhermes-agent/src/summarizing_engine.rs` — exists
- `crates/ironhermes-agent/src/lib.rs` — updated with `pub mod summarizing_engine;`
- Commit `6138982` — present in git log
- `cargo test -p ironhermes-agent --lib summarizing_engine` — 5/5 pass
- `cargo test -p ironhermes-agent --lib` — 125/125 pass (no regressions)

---
*Phase: 18-context-compression*
*Completed: 2026-04-12*
