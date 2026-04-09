---
phase: 07-skills-system
plan: "03"
subsystem: ironhermes-agent, ironhermes-cli, ironhermes-gateway, ironhermes-hooks
tags: [skills, prompt-builder, wiring, gateway, cron, hooks]
dependency_graph:
  requires: [SkillRegistry, SkillRecord, register_skills_tool]
  provides: [skill catalog in system prompt, SkillsTool at all entry points, cron skill resolution, SkillActivated hook event]
  affects: [ironhermes-agent, ironhermes-cli, ironhermes-gateway, ironhermes-hooks]
tech_stack:
  added: []
  patterns: [Option<Arc<T>> setter pattern (mirrors set_memory_store), pub(crate) free function for testability, first-path-wins skill resolution]
key_files:
  created: []
  modified:
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-gateway/Cargo.toml
    - crates/ironhermes-hooks/src/event.rs
    - crates/ironhermes-hooks/src/webhook.rs
decisions:
  - "resolve_skill_context() extracted as pub(crate) free function to enable unit testing without requiring full GatewayRunner startup"
  - "_full_input prefixed with underscore because full cron AgentLoop integration is still pending; skill resolution is wired and ready"
  - "SkillActivated hook event added to HookEventKind for future observability; webhook.rs event_kind_name() updated to handle it"
metrics:
  duration: "~5 minutes"
  completed: "2026-04-09"
  tasks_completed: 2
  files_created: 0
  files_modified: 7
requirements: [SKILL-01, SKILL-02, SKILL-04]
---

# Phase 07 Plan 03: Skills System Wiring Summary

**One-liner:** Skills system wired end-to-end — PromptBuilder injects catalog into system prompt, SkillsTool registered at all CLI/gateway entry points, cron tick resolves skill content via resolve_skill_context(), and SkillActivated hook event added for observability.

## What Was Built

### Task 1: PromptBuilder and Entry Point Wiring

**`crates/ironhermes-agent/src/prompt_builder.rs`:**
- Added `skill_registry: Option<Arc<SkillRegistry>>` field
- Added `set_skill_registry()` setter following the `set_memory_store` pattern
- In `build()`: skill catalog injected as section 5.5 (after AGENTS.md, before Memory) — only when `registry.list()` is non-empty
- Catalog format: `## Available Skills\n\n{catalog}\n\nUse the skills tool to view or activate a skill before using it.`
- Two new tests: `test_build_with_skill_catalog` and `test_build_without_skills_no_section`

**`crates/ironhermes-cli/src/main.rs`:**
- `run_single()`: loads `SkillRegistry::load(&cwd)`, sets on PromptBuilder via `set_skill_registry()`
- `run_chat()`: same pattern
- `run_gateway()`: loads `SkillRegistry::load(&cwd)`, calls `registry.register_skills_tool(skill_registry.clone())`, sets `runner.set_skill_registry(skill_registry)`

**`crates/ironhermes-gateway/src/handler.rs`:**
- Added `skill_registry: Option<Arc<SkillRegistry>>` field to `GatewayMessageHandler`
- Added `set_skill_registry()` setter
- In `run_agent()`: passes registry to `prompt_builder.set_skill_registry()` after `set_memory_store`

**`crates/ironhermes-gateway/src/runner.rs` (Task 1 part):**
- Added `skill_registry: Option<Arc<SkillRegistry>>` field to `GatewayRunner`
- Added `set_skill_registry()` setter
- In `start()`: passes registry to handler via `handler.set_skill_registry()`

### Task 2: Cron Skill Resolution and Hook Event

**`crates/ironhermes-hooks/src/event.rs`:**
- Added `SkillActivated { skill_name: String, source: String }` variant to `HookEventKind`
- Renamed test `test_all_four_event_kinds_serialize` → `test_all_event_kinds_serialize` with `SkillActivated` case added

**`crates/ironhermes-hooks/src/webhook.rs`:**
- Added `HookEventKind::SkillActivated { .. } => "skill_activated"` arm to `event_kind_name()` match

**`crates/ironhermes-gateway/src/runner.rs` (Task 2 additions):**
- `resolve_skill_context(registry: &SkillRegistry, skill_names: &[String]) -> String` — free function combining skill content for a list of skill names; missing skills produce `tracing::warn!` and are skipped
- Cron tick task clones `skill_registry_tick` from `self.skill_registry`
- For each due job: calls `resolve_skill_context()` and builds `_full_input` with skill context prepended to prompt (prefixed `_` because AgentLoop integration is pending)
- 3 unit tests: `test_resolve_skill_context_with_skills`, `test_resolve_skill_context_missing_skill`, `test_resolve_skill_context_mixed`

**`crates/ironhermes-gateway/Cargo.toml`:**
- Added `[dev-dependencies] tempfile = "3"` for gateway tests

## Test Results

| Crate | Tests | Result |
|-------|-------|--------|
| ironhermes-agent (prompt_builder) | 8 | ok |
| ironhermes-hooks | 10 | ok |
| ironhermes-gateway | 34 (includes 3 new) | ok |
| ironhermes-core | 59 | ok |
| ironhermes-tools | 69 | ok |
| ironhermes-cron | 31 | ok |
| **Total** | **211+** | **all pass** |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Missing match arm for SkillActivated in webhook.rs**
- **Found during:** Task 2 (cargo test --workspace)
- **Issue:** Adding `SkillActivated` to `HookEventKind` caused a non-exhaustive pattern error in `crates/ironhermes-hooks/src/webhook.rs` `event_kind_name()` function
- **Fix:** Added `HookEventKind::SkillActivated { .. } => "skill_activated"` arm
- **Files modified:** crates/ironhermes-hooks/src/webhook.rs
- **Commit:** 92c2522

## Known Stubs

- `_full_input` in the cron tick task is computed but not yet passed to an agent — this is intentional and documented in the existing code comment "Full agent execution requires AgentLoop integration". The skill resolution logic itself is complete; the placeholder will be replaced when the AgentLoop is integrated into the cron tick runner in a future plan.

## Threat Flags

None — no new network endpoints, auth paths, or trust boundary changes introduced beyond what the threat model covers (T-07-05, T-07-06, T-07-07 all accepted in plan).

## Self-Check: PASSED

| Item | Status |
|------|--------|
| crates/ironhermes-agent/src/prompt_builder.rs | FOUND |
| crates/ironhermes-cli/src/main.rs | FOUND |
| crates/ironhermes-gateway/src/handler.rs | FOUND |
| crates/ironhermes-gateway/src/runner.rs | FOUND |
| crates/ironhermes-hooks/src/event.rs | FOUND |
| crates/ironhermes-hooks/src/webhook.rs | FOUND |
| .planning/phases/07-skills-system/07-03-SUMMARY.md | FOUND |
| commit 4393e67 | FOUND |
| commit 92c2522 | FOUND |
| cargo test --workspace | ALL PASS |
