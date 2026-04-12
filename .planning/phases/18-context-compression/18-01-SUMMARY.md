---
phase: 18-context-compression
plan: 01
subsystem: api
tags: [rust, context-compression, async-trait, config]

requires:
  - phase: 11-memory-provider-trait
    provides: async_trait + Send + Sync + 'static pattern
provides:
  - ContextEngine trait with async compress()
  - LocalPruningEngine wrapping ContextCompressor
  - CompressionMode / ContextStats / CompressionOutcome / ContextError types
  - CompressionConfig + agent/gateway compression keys
affects: [18-02, 18-03, 18-04, 18-05, 18-06]

tech-stack:
  added: []
  patterns: [async_trait ContextEngine with Arc<dyn T> sharing]

key-files:
  created:
    - crates/ironhermes-agent/src/context_engine.rs
  modified:
    - crates/ironhermes-agent/src/context_compressor.rs
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-core/src/config.rs

key-decisions:
  - "Delegate LocalPruningEngine.compress to existing ContextCompressor (D-08 verbatim wrap)"
  - "protect_last_tokens default = min(20_000, context_length/4)"
  - "New CompressionConfig struct under Config with serde defaults per T-18-06"

patterns-established:
  - "ContextEngine: async_trait trait + Send + Sync + 'static for Arc<dyn> sharing"
  - "Compression config keys live under agent/gateway + top-level compression blocks"

requirements-completed: [PRMT-12, PRMT-14]

duration: ~15min
completed: 2026-04-12
---

# Phase 18 Plan 01: ContextEngine Foundation + Config Keys

**ContextEngine async trait + LocalPruningEngine wrapper + Phase 18 compression config keys with serde defaults**

## Performance

- **Tasks:** 2
- **Files modified:** 4
- **Completed:** 2026-04-12

## Accomplishments
- ContextEngine trait with async compress, threshold, mode methods
- LocalPruningEngine implementing the trait via ContextCompressor delegation
- Phase 18 config keys: agent/gateway compression_threshold + context_engine + system_message + CompressionConfig block
- ContextCompressor::with_protect / protect_first_n / protect_last_tokens setters

## Task Commits

1. **Task 1: Define ContextEngine trait + LocalPruningEngine** — `5458dc0` (feat)
2. **Task 2: Extend Config with Phase 18 keys + ContextCompressor setters** — `a9e7570` (feat)

## Files Created/Modified
- `crates/ironhermes-agent/src/context_engine.rs` — ContextEngine trait, types, LocalPruningEngine
- `crates/ironhermes-agent/src/context_compressor.rs` — with_protect setters + accessors
- `crates/ironhermes-agent/src/lib.rs` — pub mod context_engine
- `crates/ironhermes-core/src/config.rs` — CompressionConfig + agent/gateway keys

## Decisions Made
- Delegation-based LocalPruningEngine (wraps existing ContextCompressor rather than duplicating logic)
- serde(default) on every new config field for T-18-06 forward-compat

## Deviations from Plan
None — plan executed as written.

## Issues Encountered
None.

## Next Phase Readiness
- 18-02 can wire tool_pair primitives into LocalPruningEngine
- 18-03 can implement SummarizingEngine against the trait
- 18-06 can read compression_threshold / context_engine keys from config

---
*Phase: 18-context-compression*
*Completed: 2026-04-12*
