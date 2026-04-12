---
phase: 15-10-layer-prompt-assembly
plan: 02
subsystem: agent
tags: [personality, registry, prompt-overlay, config, rust, security]

# Dependency graph
requires:
  - phase: 15-01
    provides: PromptBuilder with set_overlay()/clear_overlay() and SessionOverlay slot 8
provides:
  - PersonalityRegistry struct with 14 built-in presets and custom loading
  - AgentConfig.personalities field (HashMap<String, String>) in config.yaml
  - Security scanning + truncation for all custom personality .md files
affects:
  - 15-03 (memory tools integration)
  - 20-personality-command (/personality command integration)

# Tech tracking
tech-stack:
  added: []
  patterns:
    - HashMap<String, String> for personality preset storage
    - entry().or_insert() for built-in-first precedence (home files cannot override built-ins)
    - insert() for config.yaml highest-precedence override
    - scan_context_content() + truncate_content() chain for HERMES_HOME/personalities/*.md security

key-files:
  created:
    - crates/ironhermes-agent/src/personality.rs
  modified:
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-core/src/config.rs

key-decisions:
  - "Built-ins use entry().or_insert() so HERMES_HOME files cannot shadow built-in presets (D-09)"
  - "config.yaml personalities uses insert() to unconditionally overwrite — highest precedence"
  - "PersonalityRegistry::load takes hermes_home as &Path parameter (no env var dependency in tests)"
  - "Security scan applied to HERMES_HOME/personalities/*.md before insertion (T-15-02)"
  - "Truncation at CONTEXT_FILE_MAX_CHARS (20K) applied after scan (T-15-05)"

requirements-completed: [PRMT-06, PRMT-07]

# Metrics
duration: ~2min
completed: 2026-04-12
---

# Phase 15 Plan 02: PersonalityRegistry Summary

**PersonalityRegistry with 14 built-in presets, custom loading from config.yaml and HERMES_HOME/personalities/*.md, security scanning, and config-wins precedence.**

## Performance

- **Duration:** ~2 min
- **Started:** 2026-04-12T13:49:49Z
- **Completed:** 2026-04-12T13:52:00Z
- **Tasks:** 1 of 1
- **Files modified:** 4 (1 created, 3 modified)

## Accomplishments

- Created `crates/ironhermes-agent/src/personality.rs` with `PersonalityRegistry` struct containing 14 built-in presets (helpful, concise, technical, creative, teacher, kawaii, catgirl, pirate, shakespeare, surfer, noir, uwu, philosopher, hype)
- Implemented `PersonalityRegistry::load(config_personalities, hermes_home)` with correct precedence: built-ins < HERMES_HOME files < config.yaml (D-09)
- Security scanning applied via `scan_context_content()` to all custom .md files from HERMES_HOME/personalities/ (T-15-02); truncation at 20K chars (T-15-05)
- Added `AgentConfig.personalities: HashMap<String, String>` to config.rs with `serde(default)` for backward compatibility
- Exported `PersonalityRegistry` from `crates/ironhermes-agent/src/lib.rs`
- Added 3 overlay tests to prompt_builder.rs: `test_personality_overlay`, `test_personality_overlay_in_timestamp`, `test_personality_overlay_absent_by_default`
- All 94 ironhermes-agent tests pass; all 114 ironhermes-core tests pass

## Task Commits

1. **Task 1: PersonalityRegistry with 14 built-ins, custom loading, overlay tests** - `f32941b` (feat)

## Files Created/Modified

- `crates/ironhermes-agent/src/personality.rs` — New: PersonalityRegistry, builtin_presets() (14 entries), load(), get(), list(), comprehensive tests for built-ins/config/hermes_home/precedence/security
- `crates/ironhermes-agent/src/lib.rs` — Added `mod personality` + `pub use personality::PersonalityRegistry`
- `crates/ironhermes-agent/src/prompt_builder.rs` — Added 3 overlay tests to test module
- `crates/ironhermes-core/src/config.rs` — Added `pub personalities: HashMap<String, String>` to AgentConfig struct and Default impl

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — PersonalityRegistry is fully functional. The `/personality` command integration (Phase 20 scope) is out of scope for this plan as documented in the plan's objective.

## Threat Flags

None — security mitigations T-15-02 and T-15-05 implemented as specified in threat model:
- T-15-02: scan_context_content() applied to all HERMES_HOME/personalities/*.md files
- T-15-05: truncate_content() at CONTEXT_FILE_MAX_CHARS applied to all custom personality files
- T-15-04: config.yaml personalities not scanned (same trust level as SOUL.md per plan disposition)

## Self-Check: PASSED

- `crates/ironhermes-agent/src/personality.rs` — exists, contains `pub struct PersonalityRegistry`
- `crates/ironhermes-agent/src/lib.rs` — contains `pub use personality::PersonalityRegistry`
- `crates/ironhermes-core/src/config.rs` — AgentConfig contains `pub personalities: HashMap<String, String>`
- Commit `f32941b` — verified in git log
- 94 tests pass in ironhermes-agent, 114 in ironhermes-core
