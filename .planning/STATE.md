---
gsd_state_version: 1.0
milestone: v2.0
milestone_name: Intelligence & Identity
status: executing
stopped_at: 18-10 shipped + live UAT Test 5&6 pass; 18-11/18-12 planned
last_updated: "2026-04-13T23:50:00.000Z"
last_activity: 2026-04-13 -- Phase 18 live UAT 2/4 new passes; 2 new plans queued
progress:
  total_phases: 13
  completed_phases: 6
  total_plans: 26
  completed_plans: 25
  percent: 96
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-11)

**Core value:** A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.
**Current focus:** Phase 18 — context-compression

## Current Position

Phase: 18 (context-compression) — EXECUTING
Plan: 10/12 shipped; 18-11 & 18-12 queued
Status: Live UAT Tests 5 & 6 pass (with `protect_first_n=2`); default config still deadlocks → 18-11; post-compression agent loop → 18-12
Last activity: 2026-04-13T23:44 -- Live UAT 10 consecutive compressions green, two new gaps filed

Progress: [██████████] 100%

## Performance Metrics

**Velocity:**

- Total plans completed: 5
- Average duration: — min
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 11 | 2 | - | - |
| 15 | 3 | - | - |

**Recent Trend:**

- Last 5 plans: —
- Trend: —

*Updated after each plan completion*
| Phase 12 P02 | 8 | 2 tasks | 3 files |
| Phase 12 P04 | 35 | 2 tasks | 8 files |
| Phase 13 P01 | 3 | 2 tasks | 1 files |
| Phase 13 P02 | 3 | 2 tasks | 3 files |
| Phase 13 P03 | 5 | 2 tasks | 4 files |
| Phase 17 P01 | 8 | 2 tasks | 2 files |
| Phase 17 P02 | 4 | 2 tasks | 3 files |
| Phase 17 P03 | 4 | 2 tasks | 9 files |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- v2.0: Port hermes-agent architecture faithfully — deviate only with documented rationale
- v2.0: Two-tier memory: built-in MEMORY.md/USER.md always active + optional external provider on top
- v2.0: Memory providers scoped to SQLite, Grafeo, DuckDB only (not all 8 Python backends)
- v2.0: Frozen-snapshot pattern — system prompt built once at session start, mid-session writes take effect next session
- [Phase 12]: AnyClient uses enum dispatch (not trait objects) for zero-cost multi-provider abstraction
- [Phase 12]: AgentLoop.client changed from LlmClient to AnyClient; resolve_base_url/resolve_api_key deleted
- [Phase 13]: busy_timeout(5000ms) + deterministic jitter retry (no rand dep) for SQLite write contention
- [Phase 13]: SearchFilter with composable WHERE clauses and FTS5 snippet() using << >> markers
- [Phase 13]: prune_sessions deletes messages explicitly before sessions (no CASCADE); SessionExport with Serialize+Deserialize for JSON export
- [Phase 13]: SessionStore composes Arc<Mutex<StateStore>> + HashMap as write-through cache; every create/message writes to SQLite immediately
- [Phase 17]: Snapshot field changed from HashMap<MemoryTarget, String> to HashMap<MemoryTarget, Vec<String>> - raw entries stored, header computed lazily
- [Phase 17]: Error transformation in MemoryTool: blocked -> content_rejected envelope; capacity_exceeded -> D-15 envelope with suggestion field
- [Phase 17]: Single-pass marker conversion for <<match>> -> >>>match<<< avoids chained String::replace double-substitution
- [Phase 17]: session_search schema only added to LLM tool list when state_store is configured — acts as subagent safety gate
- [Phase 17]: Mutex<Connection> wraps rusqlite::Connection to satisfy Sync bound on MemoryProvider trait
- [Phase 17]: Factory in ironhermes-agent returns Arc<Mutex<dyn MemoryProvider>> vs Box<dyn> in core for MemoryTool compatibility

### Pending Todos

None.

### Blockers/Concerns

- **Default config deadlock (18-11 scope):** With `compression.protect_first_n=3` (documented default) and a [sys, user, asst-tool_use, tool_result] shape, the two-direction guard correctly collapses the prune range to zero — compression cannot fire. UAT only passed after lowering to 2. Fix: auto-extend/auto-shrink `protect_first_n` around tool-pair boundaries.
- **Post-compression retry loop (18-12 scope):** Live UAT saw the agent re-call `web_read` on every turn for 10 consecutive turns (hit MAX_COMPRESSION_PASSES), never returning a summary. `[CONTEXT HISTORY]` summary content does not convey tool-call completion, so the model treats every turn as a fresh request.

## Session Continuity

Last session: 2026-04-13T23:50:00.000Z
Stopped at: Live UAT complete — Tests 5 & 6 pass; 18-11 and 18-12 queued for planning
Resume file: None — run `/gsd-plan-phase 18` to plan 18-11 (protect_first_n tool-pair awareness) next
