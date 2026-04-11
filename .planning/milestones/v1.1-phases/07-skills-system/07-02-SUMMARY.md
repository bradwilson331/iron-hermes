---
phase: 07-skills-system
plan: "02"
subsystem: ironhermes-tools
tags: [skills, tool, registry, agent-interface]
dependency_graph:
  requires: [SkillRegistry, SkillRecord, parse_skill_md]
  provides: [SkillsTool, register_skills_tool]
  affects: [ironhermes-agent, ToolRegistry]
tech_stack:
  added: []
  patterns: [action-dispatch pattern (mirror CronjobTool), Arc<SkillRegistry> read-only shared state]
key_files:
  created:
    - crates/ironhermes-tools/src/skills_tool.rs
  modified:
    - crates/ironhermes-tools/src/lib.rs
    - crates/ironhermes-tools/src/registry.rs
decisions:
  - "SkillsTool uses Arc<SkillRegistry> (not Arc<Mutex>) — registry is read-only after construction, no locking needed"
  - "view action reads full SKILL.md from disk via std::fs::read_to_string(record.path) for complete content including frontmatter"
  - "activate action delegates to SkillRegistry.read_content() which returns body-only (no frontmatter) per D-07 design"
  - "worktree shares main repo target dir — CARGO_TARGET_DIR override required to compile worktree changes independently"
metrics:
  duration: "~10 minutes"
  completed: "2026-04-09"
  tasks_completed: 1
  files_created: 1
  files_modified: 2
requirements: [SKILL-04]
---

# Phase 07 Plan 02: SkillsTool Implementation Summary

**One-liner:** SkillsTool exposes list/view/activate actions to the agent via the Tool trait, backed by a shared read-only Arc<SkillRegistry>, registered on ToolRegistry via register_skills_tool().

## What Was Built

`crates/ironhermes-tools/src/skills_tool.rs` — the agent-facing interface to the skills system:

- **SkillsTool** — struct holding `Arc<SkillRegistry>` (read-only, no Mutex needed)
- **handle_list** — iterates `registry.list()`, returns `{"status":"ok","skills":[{"name":...,"description":...}],"count":N}`
- **handle_view** — extracts `name` param, calls `registry.find()`, reads full SKILL.md from disk path, returns `{"status":"ok","name":...,"content":"full file"}`
- **handle_activate** — extracts `name` param, calls `registry.read_content()` for body-only (no frontmatter), returns `{"status":"ok","name":...,"content":"body"}`
- **Unknown action handling** — returns `{"status":"error","message":"Unknown action '...'. Valid: list, view, activate"}`
- **Missing action handling** — returns `anyhow::Error` (caller-level error, not JSON)

`crates/ironhermes-tools/src/lib.rs` — added `pub mod skills_tool;`

`crates/ironhermes-tools/src/registry.rs` — added `register_skills_tool(&mut self, registry: Arc<SkillRegistry>)` following the `register_cronjob_tool` pattern exactly.

## Test Results

10 unit tests, all passing (TDD: RED then GREEN):

| Test | Result |
|------|--------|
| test_name_returns_skills | ok |
| test_toolset_returns_skills | ok |
| test_list_empty_registry | ok |
| test_list_returns_skills_with_name_and_description | ok |
| test_view_existing_skill_returns_full_content | ok |
| test_view_nonexistent_returns_error | ok |
| test_activate_existing_skill_returns_body_only | ok |
| test_activate_nonexistent_returns_error | ok |
| test_unknown_action_returns_error_with_valid_list | ok |
| test_missing_action_returns_error | ok |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking Issue] Worktree shares main repo target directory**
- **Found during:** Task 1 (GREEN phase verification — skills_tool tests not appearing in test list)
- **Issue:** Git worktrees share the parent repo's `target/` directory by default. The cached test binary `ironhermes_tools-dee8f16ff0d2ec3a` was compiled from main repo (without skills_tool), and cargo saw no reason to recompile since the worktree files weren't in the incremental fingerprint path it was using.
- **Fix:** Used `CARGO_TARGET_DIR=/path/to/worktree/target` env var to force an isolated build, which correctly compiled all 10 tests
- **Files modified:** None (env var workaround — no code change needed)
- **Commit:** N/A (test infrastructure only)

## Known Stubs

None — all three actions (list, view, activate) are fully implemented with real SkillRegistry data. No placeholder returns or hardcoded values.

## Threat Flags

None — no new network endpoints, auth paths, or trust boundary changes introduced.

- T-07-03 (Tampering): The `name` parameter is used only as a lookup key in `registry.find()` — no filesystem path is constructed from user input. Paths come from pre-scanned `SkillRecord.path` fields. Mitigated as designed.
- T-07-04 (Information Disclosure): SKILL.md content returned to agent by design — same trust model as SOUL.md/AGENTS.md.

## Self-Check: PASSED

| Item | Status |
|------|--------|
| crates/ironhermes-tools/src/skills_tool.rs | FOUND |
| pub struct SkillsTool in skills_tool.rs | FOUND |
| fn name() returning "skills" | FOUND |
| "list" => and "view" => and "activate" => handlers | FOUND |
| pub mod skills_tool in lib.rs | FOUND |
| pub fn register_skills_tool in registry.rs | FOUND |
| commit b5d5829 | FOUND |
| 10 tests passing | VERIFIED |
| cargo build --workspace | CLEAN |
