---
phase: 15-10-layer-prompt-assembly
plan: 03
subsystem: agent
tags: [prompt-assembly, context-loader, subdir-discovery, rust, cli, gateway, security]

# Dependency graph
requires:
  - phase: 15-01
    provides: PromptBuilder with BTreeMap slots, with_provider(), load_memory(), load_skills()
  - phase: 15-02
    provides: PersonalityRegistry and AgentConfig.personalities
provides:
  - CONTEXT_CANDIDATES updated to 5 entries including HERMES.md at index 1 (D-18)
  - .cursor/rules/*.mdc fallback in load_project_context_str (D-19, T-15-06)
  - SUBDIR_CONTEXT_MAX_CHARS=8000 in subdir_discovery.rs (D-20, T-15-07)
  - CLI call sites (run_single, run_chat) using full new PromptBuilder API
  - Gateway handler using full new PromptBuilder API with provider, memory, skills
affects:
  - 16-caching (build_split() durable/ephemeral for cache_control breakpoint)
  - 20-personality-command (/personality command integration)

# Tech tracking
tech-stack:
  added: []
  patterns:
    - CONTEXT_CANDIDATES 5-entry priority chain: .hermes.md > HERMES.md > AGENTS.md > CLAUDE.md > .cursorrules
    - .cursor/rules/*.mdc glob fallback with sorted deterministic ordering and security scan
    - SUBDIR_CONTEXT_MAX_CHARS=8000 separate from CONTEXT_FILE_MAX_CHARS=20000 for tool result truncation
    - All PromptBuilder call sites: .with_provider().load_context().load_memory().load_skills() chain

key-files:
  created: []
  modified:
    - crates/ironhermes-agent/src/context_loader.rs
    - crates/ironhermes-agent/src/subdir_discovery.rs
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-gateway/src/handler.rs

key-decisions:
  - "HERMES.md at index 1 in CONTEXT_CANDIDATES (after .hermes.md, before AGENTS.md) — hermes-specific files grouped together (D-18)"
  - ".cursor/rules/*.mdc handled as glob fallback outside CONTEXT_CANDIDATES array — sorted for determinism (D-19)"
  - "SUBDIR_CONTEXT_MAX_CHARS=8000 separate constant — subdirectory tool results need tighter cap than project context (D-20)"
  - "All callers use build_system_message() for now — Phase 16 updates to build_split() when LLM adapter supports multi-block system prompts (D-24)"

patterns-established:
  - "Glob fallback pattern: sort entry_paths for deterministic ordering before processing"
  - "Separate truncation constants for different injection contexts (CONTEXT_FILE_MAX_CHARS vs SUBDIR_CONTEXT_MAX_CHARS)"
  - "PromptBuilder call site pattern: .with_provider(config.model.provider).load_context(&cwd) then .load_memory()/.load_skills() after set_*() calls"

requirements-completed: [PRMT-01, PRMT-02, PRMT-03, PRMT-04, PRMT-05, PRMT-06, PRMT-07, MEM-06]

# Metrics
duration: 25min
completed: 2026-04-12
---

# Phase 15 Plan 03: Context Candidates, Subdir Truncation, and Call Site Migration Summary

**HERMES.md added to 5-entry priority chain, .cursor/rules/*.mdc glob fallback with security scan, 8K subdir truncation cap, and all PromptBuilder call sites migrated to slot-based API with provider/memory/skills.**

## Performance

- **Duration:** ~25 min
- **Started:** 2026-04-12T14:20:00Z
- **Completed:** 2026-04-12T14:45:00Z
- **Tasks:** 2 of 2
- **Files modified:** 5

## Accomplishments

- Updated `CONTEXT_CANDIDATES` from 4 to 5 entries: `.hermes.md` > `HERMES.md` > `AGENTS.md` > `CLAUDE.md` > `.cursorrules` (D-18)
- Added `.cursor/rules/*.mdc` glob fallback in `load_project_context_str()` with `scan_context_content()` security scanning per T-15-06 and deterministic sorted ordering
- Added `SUBDIR_CONTEXT_MAX_CHARS = 8_000` constant in `subdir_discovery.rs`, replacing `CONTEXT_FILE_MAX_CHARS` for subdirectory tool result truncation (D-20, T-15-07)
- Migrated both CLI call sites (`run_single`, `run_chat`) to new API: `.with_provider(&config.model.provider).load_context().load_memory().load_skills()`
- Migrated gateway `handler.rs` call site to new API with `self.config.model.provider`
- Added 3 new tests: `test_hermes_md_in_candidates`, `test_subdir_truncation_cap`; updated `test_context_candidates_case_sensitive` for len==5
- All 96 ironhermes-agent tests pass; full workspace builds clean

## Task Commits

1. **Task 1: Update CONTEXT_CANDIDATES, subdir truncation, .cursor/rules/*.mdc** - `e06c6d9` (feat)
2. **Task 2: Migrate CLI and gateway call sites to new PromptBuilder API** - `63fd4ae` (feat)

## Files Created/Modified

- `crates/ironhermes-agent/src/context_loader.rs` — HERMES.md added at index 1, test updated for len==5, test_hermes_md_in_candidates added
- `crates/ironhermes-agent/src/subdir_discovery.rs` — SUBDIR_CONTEXT_MAX_CHARS=8000 constant, truncate_content call updated, test_subdir_truncation_cap added
- `crates/ironhermes-agent/src/prompt_builder.rs` — .cursor/rules/*.mdc glob fallback block in load_project_context_str()
- `crates/ironhermes-cli/src/main.rs` — Both PromptBuilder call sites updated with .with_provider(), .load_memory(), .load_skills()
- `crates/ironhermes-gateway/src/handler.rs` — PromptBuilder call site updated with .with_provider(), .load_memory(), .load_skills()

## Decisions Made

- `HERMES.md` placed immediately after `.hermes.md` (index 1) — both are hermes-specific files, grouped together before community-standard files (AGENTS.md, CLAUDE.md)
- `.cursor/rules/*.mdc` files sorted alphabetically for deterministic system prompt assembly across runs
- `SUBDIR_CONTEXT_MAX_CHARS` is a separate constant from `CONTEXT_FILE_MAX_CHARS` — makes the different truncation policies explicit and independently adjustable
- All callers continue using `build_system_message()` per D-24 — Phase 16 will update to `build_split()` when the LLM adapter supports multi-block system prompts with `cache_control`

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — all context slots are wired with real data sources. The `.cursor/rules/*.mdc` fallback reads actual files from disk. Memory and skills load from their respective registries.

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: prompt-injection | prompt_builder.rs | .cursor/rules/*.mdc files run through scan_context_content() before injection — T-15-06 mitigated |
| threat_flag: dos-truncation | subdir_discovery.rs | Subdirectory context files truncated at 8,000 chars — T-15-07 mitigated |

## Issues Encountered

Pre-existing test failure: `delegate_task::tests::test_delegate_task_schema_has_required_task` in `ironhermes-tools` — confirmed pre-existing before this plan's changes (verified via git stash). Documented in Plan 01 SUMMARY. Not caused by this plan.

## Next Phase Readiness

- Phase 15 complete: all 3 plans done. PromptBuilder is fully slot-based with BTreeMap, PersonalityRegistry is built, all call sites migrated.
- Phase 16 (caching): `build_split()` returns `(durable, ephemeral)` ready for `cache_control` placement. Call sites need updating from `build_system_message()` to `build_split()`.
- Phase 20 (personality command): `PersonalityRegistry` and `set_overlay()`/`clear_overlay()` are ready for `/personality` command wiring.

---
*Phase: 15-10-layer-prompt-assembly*
*Completed: 2026-04-12*
