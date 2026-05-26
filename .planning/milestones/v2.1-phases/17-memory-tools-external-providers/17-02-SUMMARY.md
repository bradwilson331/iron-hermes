---
phase: 17-memory-tools-external-providers
plan: "02"
subsystem: agent
tags: [session_search, fts5, tool-interception, agent-loop]
dependency_graph:
  requires: [ironhermes-state::StateStore, ironhermes-state::SearchFilter, ironhermes-state::sanitize_fts_query]
  provides: [session_search tool schema, session_search handler, agent loop state_store integration]
  affects: [ironhermes-agent::AgentLoop, ironhermes-agent::session_search]
tech_stack:
  added: []
  patterns: [tool interception before registry dispatch, spawn_blocking for sync StateStore in async context, single-pass marker conversion]
key_files:
  created:
    - crates/ironhermes-agent/src/session_search.rs
  modified:
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/lib.rs
decisions:
  - Single-pass marker conversion (scan byte-by-byte) avoids double-substitution from chained String::replace calls
  - session_search schema only added to LLM tool list when state_store is configured — absent state_store acts as subagent safety gate
  - spawn_blocking used around handle_session_search since rusqlite is sync and must not block the tokio executor
metrics:
  duration_min: 4
  completed: "2026-04-12T18:11:18Z"
  tasks: 2
  files_changed: 3
---

# Phase 17 Plan 02: Session Search Tool Summary

session_search tool wrapping StateStore FTS5 search with >>>match<<< markers, intercepted in agent loop via state_store field before registry dispatch.

## What Was Built

### Task 1: session_search module (TDD)

Created `crates/ironhermes-agent/src/session_search.rs` with:

- `session_search_schema()` — D-05 compliant ToolSchema with query (required), role_filter, source_filter, limit parameters
- `handle_session_search(args, store)` — Extracts args, builds SearchFilter, calls StateStore::search_messages, post-processes results
- Single-pass marker conversion: `<<match>>` → `>>>match<<<` (D-06) using byte-by-byte scan to avoid chained replace double-substitution
- Context truncation: context_before/context_after clamped to 200 chars with `...` suffix
- Error envelopes: `missing_query` for absent/empty query, `search_failed` for DB errors, `unavailable` for missing store
- `pub mod session_search` added to lib.rs

5 unit tests covering: missing query, empty query, marker conversion, context truncation, empty results.

### Task 2: Agent loop wiring

Modified `crates/ironhermes-agent/src/agent_loop.rs`:

- Added `state_store: Option<Arc<std::sync::Mutex<StateStore>>>` field
- Added `with_state_store(store)` builder method
- Intercepted `session_search` tool calls before `registry.execute_tool()` in `execute_tool_call()` using `spawn_blocking` wrapper (D-07)
- Added `session_search_schema()` to tool list in `run()` when state_store is Some — absent when not configured (subagent safety per plan spec)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Marker conversion double-substitution**
- **Found during:** Task 1 RED phase test run
- **Issue:** `s.replace("<<", ">>>").replace(">>", "<<<")` chained String::replace caused double-substitution. `<<<` contains `<<` which gets re-replaced. Result was `>>>match>>><` instead of `>>>match<<<`
- **Fix:** Single-pass byte scanner that replaces `<<` and `>>` in one pass without re-scanning substituted output
- **Files modified:** crates/ironhermes-agent/src/session_search.rs
- **Commit:** 5cd9f75

## Verification

- `cargo test -p ironhermes-agent session_search` — 5/5 passed
- `cargo test -p ironhermes-agent` — 101/101 passed (no regressions)
- `cargo check --workspace` — clean (only pre-existing warnings)

## Known Stubs

None.

## Threat Flags

None — session_search uses StateStore::search_messages which calls sanitize_fts_query per D-08 (T-17-03 mitigated). No new trust boundaries introduced beyond what was planned.

## Self-Check: PASSED

- [x] crates/ironhermes-agent/src/session_search.rs exists
- [x] crates/ironhermes-agent/src/agent_loop.rs contains `state_store`
- [x] crates/ironhermes-agent/src/agent_loop.rs contains `session_search`
- [x] crates/ironhermes-agent/src/agent_loop.rs contains `spawn_blocking`
- [x] crates/ironhermes-agent/src/lib.rs contains `pub mod session_search`
- [x] Commits 5cd9f75 and 152e25c exist
