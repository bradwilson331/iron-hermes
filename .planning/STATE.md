---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: executing
stopped_at: Phase 22 UI-SPEC approved
last_updated: "2026-04-17T17:20:48.128Z"
last_activity: 2026-04-17 -- Phase 22 planning complete
progress:
  total_phases: 5
  completed_phases: 2
  total_plans: 9
  completed_plans: 7
  percent: 78
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-11)

**Core value:** A working conversational AI agent with personality (context files) that operates reliably over Telegram — the core loop of receive message, think with tools, respond must work flawlessly.
**Current focus:** Phase 21 — commandline-ui-update-polish-cli-ux-including-graceful-doubl

## Current Position

Phase: 21
Plan: Not started
Status: Ready to execute
Last activity: 2026-04-17 -- Phase 22 planning complete

Progress: [██████████] 100%

## Performance Metrics

**Velocity:**

- Total plans completed: 32
- Average duration: — min
- Total execution time: 0 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 11 | 2 | - | - |
| 15 | 3 | - | - |
| 19.1 | 5 | - | - |
| 18 | 15 | - | - |
| 20 | 4 | - | - |
| 21 | 3 | - | - |

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
| Phase 19 P03 | 6min | 2 tasks | 6 files |
| Phase 19 P04 | ~3 min | 2 tasks | 3 files |
| Phase 19 P05 | 8 min | 2 tasks | 2 files |
| Phase 19 P06 | 7min | 2 tasks | 7 files |
| Phase 18 P15 | 3 | 3 tasks | 4 files |
| Phase 20-memory-provider-plugin-contract P01 | 19 | 3 tasks | 10 files |
| Phase 20 P02 | 42 min | 3 tasks | 13 files |
| Phase 20 P04 | 8 min | 3 tasks | 9 files |
| Phase 20 P03 | 5 min | 2 tasks | 4 files |
| Phase 21 P21-02 | 17 | 1 tasks | 3 files |

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
- [Phase 19]: 19-03: setup_needed envelope shape aligns with Phase 17 D-15 structured errors; setup_note is a verbatim-quotable relay string
- [Phase 19]: 19-03: credential_dir precedence = SkillsConfig.credential_dir → HERMES_HOME/credentials → ~/.ironhermes/credentials (per D-10)
- [Phase 19]: Plan 04: SkillsConfig.config stored as HashMap<String, HashMap<String, serde_yaml::Value>> with serde(default) for backward compat
- [Phase 19]: Plan 04: [Skill config: ...] header keys lex-sorted for deterministic prompt output and cache safety
- [Phase 19]: Plan 04: declared_config_schema returns None for unknown skill / no hermes meta / empty config — single sentinel for 'no schema'
- [Phase 19]: Plan 05: scan_skill_content layers SKILL_THREAT_PATTERNS over existing context THREAT_PATTERNS via short-circuit composition; scope=frontmatter+body (D-14), enforcement=Community-hard-reject + Builtin/Official-WARN-BUT-LOAD at registry-load (D-15/D-16)
- [Phase 18]: Disk-load responsibility moved into agent factory file branch so gateway needs only a single factory call
- [Phase 18]: Used .err().unwrap() instead of .unwrap_err() to extract errors from Result<Arc<Mutex<dyn MemoryProvider>>, _> which lacks Debug on T
- [Phase 20-memory-provider-plugin-contract]: Plan 20-01: kept std::sync::Mutex in build_memory_provider return type; deferred tokio::sync::Mutex workspace migration to Plan 20-02 atomic wave
- [Phase 20-memory-provider-plugin-contract]: Plan 20-01: grafeo DB path must use .grafeo file extension (memory_graph.grafeo) — required for grafeo persistence flush
- [Phase 20-memory-provider-plugin-contract]: Plan 20-01: MemoryProviderConfig deleted entirely (no compat shim per D-10/D-20); all providers migrated in lockstep
- [Phase 20-memory-provider-plugin-contract]: Plan 20-01: env-mutating tests use OnceLock<Mutex<()>> + double-set idiom (re-assert IRONHERMES_HOME before each build_memory_provider call) to tolerate racing prompt_builder tests
- [Phase 20]: Plan 20-02: MemoryManagerHandle trait in ironhermes-tools resolves tools→agent circular dep; impl lives in ironhermes-agent so MemoryTool can delegate to handle_tool_call via dyn dispatch
- [Phase 20]: Plan 20-02: full workspace migration from std::sync::Mutex to tokio::sync::Mutex executed atomically; load_memory promoted to async fn; queue_prefetch fires as detached tokio::spawn on natural-end break with last user message as query
- [Phase 20]: Plan 20-02: on_pre_compress fire site placed inside ContextEngine.compress_messages (not at caller boundary) to structurally guarantee D-23 ordering; trait-level contract test in ironhermes-core locks the ordering into a regression test reusable by any future provider crate
- [Phase 20]: Plan 20-04: file-provider get_config_schema written in memory_provider.rs (actual impl site from 20-01), not memory_store.rs; tests placed in memory_store.rs tests mod with qualified trait syntax
- [Phase 20]: Plan 20-04: ConfigField.description is Option<String> — all 4 providers use Some("...".to_string()); assertion helper uses is_some_and non-empty
- [Phase 20]: Plan 20-04: sqlite_mirror_fixture uses Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>> SharedProvider (per 20-02), not Box<dyn>+parking_lot as plan samples showed; no new dep
- [Phase 20]: Plan 20-04: DuckDB threads field declarative only — wizard prompts+persists; PRAGMA threads=N runtime wiring deferred
- [Phase 20]: Plan 20-03: scripted-stdin D-23 integration test uses always-present file provider (3 defaulted fields) instead of cfg-gated TestProvider — zero new code surface, full wizard round-trip still covered
- [Phase 20]: Plan 20-03: run_memory_setup_with_io<R: BufRead, W: Write> is the pure testable core; public run_memory_setup(&Cli) is a thin wrapper that locks real stdin/stdout
- [Phase 20]: Plan 20-03: Fix 2 closure — run_chat and run_single now build MemoryManager + register_memory_tool + set_memory_manager + delegate_task memory slot; CLI reaches gateway parity for cross-invocation memory persistence
- [Phase 20]: Plan 20-03: static-grep regression test (run_chat_and_run_single_both_wire_memory_manager) locks the three wiring calls in main.rs against future refactor regressions
- [Phase 21]: TuiHandle uses shutdown(self) consuming self — Wave 3 wraps in Arc<TuiHandle> per W3
- [Phase 21]: ActivityState::Thinking absent (W6) — only Idle/Streaming/ToolCall{name}
- [Phase 21]: dead_code suppressed in tui/mod.rs with module-level allow — removed in Wave 3 on wiring

### Roadmap Evolution

- Phase 22 added: CLI feature parity

### Pending Todos

6 pending. Latest:

- [skills] Slash command integration SKILL-13 (2026-04-17)
- [tools] Tool registry improvements (2026-04-17)
- [cli] CLI feature parity (2026-04-17)
- [cli] Configuration and setup wizard improvements (2026-04-17)

### Blockers/Concerns

- **Default config deadlock (18-11 scope):** With `compression.protect_first_n=3` (documented default) and a [sys, user, asst-tool_use, tool_result] shape, the two-direction guard correctly collapses the prune range to zero — compression cannot fire. UAT only passed after lowering to 2. Fix: auto-extend/auto-shrink `protect_first_n` around tool-pair boundaries.
- **Post-compression retry loop (18-12 scope):** Live UAT saw the agent re-call `web_read` on every turn for 10 consecutive turns (hit MAX_COMPRESSION_PASSES), never returning a summary. `[CONTEXT HISTORY]` summary content does not convey tool-call completion, so the model treats every turn as a fresh request.

## Session Continuity

Last session: 2026-04-17T17:05:04.329Z
Stopped at: Phase 22 UI-SPEC approved
Resume file: .planning/phases/22-cli-feature-parity/22-UI-SPEC.md
