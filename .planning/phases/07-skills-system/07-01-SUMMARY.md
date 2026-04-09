---
phase: 07-skills-system
plan: "01"
subsystem: ironhermes-core
tags: [skills, registry, parsing, discovery]
dependency_graph:
  requires: []
  provides: [SkillRegistry, SkillRecord, parse_skill_md]
  affects: [ironhermes-tools, ironhermes-agent]
tech_stack:
  added: []
  patterns: [serde_yaml frontmatter parsing, first-path-wins deduplication, HashSet dedup]
key_files:
  created:
    - crates/ironhermes-core/src/skills.rs
  modified:
    - crates/ironhermes-core/src/lib.rs
decisions:
  - "Added load_with_paths() as a test-isolation constructor to prevent real ~/.agents/skills from contaminating unit tests"
  - "First closing \\n--- delimiter logic (not last) correctly handles --- appearing in body content"
metrics:
  duration: "~12 minutes"
  completed: "2026-04-09"
  tasks_completed: 1
  files_created: 1
  files_modified: 1
requirements: [SKILL-01, SKILL-03]
---

# Phase 07 Plan 01: SkillRegistry Implementation Summary

**One-liner:** SkillRegistry discovers and parses SKILL.md files from three priority-ordered paths using serde_yaml frontmatter deserialization with first-path-wins deduplication.

## What Was Built

`crates/ironhermes-core/src/skills.rs` — the foundation data layer for the skills system:

- **SkillFrontmatter** — serde Deserialize struct for YAML frontmatter (name, description required; version, author, license optional)
- **SkillRecord** — Clone+Debug struct holding name, description, and absolute path to SKILL.md
- **parse_skill_md** — parses `---\nYAML\n---\nbody` format; uses first occurrence of `\n---` as closing delimiter so body can safely contain `---`
- **SkillRegistry** — discovers skills from three paths in priority order, deduplicates by lowercase name (first-path-wins), exposes catalog_text/find/read_content/list/load_with_paths methods
- **lib.rs** — wired `pub mod skills` and `pub use skills::{SkillRegistry, SkillRecord}`

## Test Results

14 unit tests, all passing:

| Test | Result |
|------|--------|
| test_parse_skill_md_valid_frontmatter | ok |
| test_parse_skill_md_missing_frontmatter | ok |
| test_parse_skill_md_invalid_yaml | ok |
| test_parse_skill_md_dash_in_body | ok |
| test_parse_skill_md_optional_fields | ok |
| test_registry_load_discovers_skills | ok |
| test_registry_load_nonexistent_paths_no_panic | ok |
| test_registry_load_first_path_wins_dedup | ok |
| test_registry_load_skips_invalid_skill_md | ok |
| test_catalog_text_format | ok |
| test_catalog_text_empty_when_no_skills | ok |
| test_find_returns_some_case_insensitive | ok |
| test_find_returns_none_for_nonexistent | ok |
| test_read_content_returns_body_without_frontmatter | ok |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed double-borrow in load_with_paths iterator**
- **Found during:** Task 1 (GREEN phase compile)
- **Issue:** `for search_path in &search_paths` where `search_paths: &[PathBuf]` created `&&[PathBuf]` which isn't an iterator
- **Fix:** Changed to `for search_path in search_paths`
- **Files modified:** crates/ironhermes-core/src/skills.rs
- **Commit:** ca2d3ee

**2. [Rule 2 - Missing Critical Functionality] Added load_with_paths() for test isolation**
- **Found during:** Task 1 (RED phase — tests were picking up real ~/.agents/skills on developer machine)
- **Issue:** Tests using `SkillRegistry::load(cwd)` always scanned real ~/.agents/skills, causing 4 test failures
- **Fix:** Added `pub fn load_with_paths(search_paths: &[PathBuf]) -> Self` constructor; `load()` delegates to it; tests use `load_with_paths` with explicit temp paths only
- **Files modified:** crates/ironhermes-core/src/skills.rs
- **Commit:** ca2d3ee

## Known Stubs

None — all methods are fully implemented and tested.

## Threat Flags

None — no new network endpoints, auth paths, or trust boundary changes introduced. Scan paths are hardcoded as per threat model T-07-02.

## Self-Check: PASSED

| Item | Status |
|------|--------|
| crates/ironhermes-core/src/skills.rs | FOUND |
| crates/ironhermes-core/src/lib.rs | FOUND |
| commit ca2d3ee | FOUND |
| 14 tests passing | VERIFIED |
| cargo build -p ironhermes-core | CLEAN |
