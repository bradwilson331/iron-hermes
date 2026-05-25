---
phase: 33
plan: 02
subsystem: learning-toolset
tags: [skill_manage, learning-toolset, learn-04, learn-05, tool-registry]
requirements: [LEARN-04, LEARN-05]
dependency-graph:
  requires:
    - "33-01: SkillSource::SelfCreated variant + pub validate_skill_name"
    - "ironhermes_core::context_scanner::scan_skill_content (Phase 19 D-13/D-14)"
    - "ironhermes_core::constants::get_hermes_home (established)"
  provides:
    - "SkillManageTool — 6-action JSON-dispatch tool for self-authored SKILL.md"
    - "ToolRegistry::register_skill_manage_tool() — gated learning-toolset entry"
  affects:
    - "Plan 33-03 will call register_skill_manage_tool from entry points when 'learning' toolset is enabled"
tech-stack:
  added: []
  patterns:
    - "memory_tool.rs action-dispatch JSON pattern (mirror, exact)"
    - "Pre-write scan_skill_content gate (defense-in-depth; D-15 second gate is SkillRegistry on load)"
    - "Two-level skill dir layout HERMES_HOME/skills/<category>/<slug>/ (Pitfall 3)"
    - "JSON error string returned via Ok(_), never Err — Err reserved for missing required params"
key-files:
  created:
    - "crates/ironhermes-tools/src/skill_manage.rs (850 lines)"
    - ".planning/phases/33-autonomous-skill-creation/33-02-SUMMARY.md"
  modified:
    - "crates/ironhermes-tools/src/lib.rs (+1 line: pub mod skill_manage)"
    - "crates/ironhermes-tools/src/registry.rs (+12 lines: register_skill_manage_tool)"
decisions:
  - "TDD compromise: tests + impl committed together. For a greenfield module the canonical RED→GREEN split would either fail to compile (no SkillManageTool type) or write throw-away test stubs; integrated commit preserves the test-first design intent while still producing a single reviewable diff."
  - "Action methods return Ok(JSON-error-string) for domain failures (not_found, already_exists, content_rejected, path_traversal_rejected, invalid_name, invalid_category, path_out_of_scope). Err is reserved for missing required JSON params — same convention as memory_tool.rs."
  - "category validated identically to slug at every action entry (no '/', no '..', no leading '.', non-empty) — prevents category-string traversal attacks (T-33-02-D)."
  - "delete uses canonicalize() on the target then verifies canonical path starts with canonicalize(HERMES_HOME/skills) — covers symlink-through-skills-dir variants of T-33-02-E."
  - "Test isolation: RAII HermesHomeGuard with a process-wide HOME_LOCK mutex; sets IRONHERMES_HOME to a TempDir, restores the prior value on drop. Matches the env-mutation pattern already used in hexapod_tcp.rs but with stronger restore semantics."
metrics:
  duration_minutes: ~12
  completed: 2026-05-16T03:54:23Z
  task_count: 2
  files_created: 2
  files_modified: 2
  tests_added: 7
  tests_passing: 7
  workspace_build: clean (0 errors)
---

# Phase 33 Plan 02: Implement SkillManageTool with Six-Action Dispatch Summary

**One-liner:** SkillManageTool (create/patch/edit/delete/write_file/remove_file) wired into ToolRegistry as a 'learning' toolset, JSON-dispatch mirror of memory_tool with pre-write security scanning, two-level SKILL.md layout, and path-traversal hardening.

## What Was Built

- **`SkillManageTool` struct** (`crates/ironhermes-tools/src/skill_manage.rs`) implementing the `Tool` trait for the `learning` toolset. Six private async action methods cover the full LEARN-05 surface; all writes target `HERMES_HOME/skills/<category>/<slug>/` for SkillRegistry two-level discovery on next session.
- **Action semantics:**
  - `create` — validates slug (`validate_skill_name`) + category, builds frontmatter (LEARN-04 fields: name, description, version 1.0.0, platforms, metadata.hermes.{tags, category, trust_tier: Self-created} + optional fallback_for_toolsets/requires_toolsets), runs `scan_skill_content`, refuses overwrite (`already_exists`), `create_dir_all` + write.
  - `patch` — `content.replacen(old_string, new_string, 1)` per Pattern 2 / Pitfall 4; returns `not_found` JSON when old_string is absent; re-scans patched content before write.
  - `edit` — full-rewrite overwrite of an existing SKILL.md; scan runs first; returns `not_found` JSON when file is absent.
  - `delete` — canonicalize target → verify within canonicalize(HERMES_HOME/skills) → `remove_dir_all`. Idempotent: second delete returns `not_found` JSON.
  - `write_file` / `remove_file` — companion-file management inside the skill dir; path traversal gate (`..` or leading `/` rejected) returns `path_traversal_rejected` JSON; `write_file` runs `scan_skill_content` on body before write.
- **`pub mod skill_manage`** in `crates/ironhermes-tools/src/lib.rs` (alphabetical position between `registry` and `skills_tool`).
- **`ToolRegistry::register_skill_manage_tool()`** in `crates/ironhermes-tools/src/registry.rs:604` — stateless registration mirror of `register_memory_tool`; NOT in `register_defaults_except` (Plan 33-03 will call this from entry points when 'learning' toolset is enabled).

## Tests

7 unit tests, all passing (`cargo test -p ironhermes-tools --lib -- skill_manage`):

| Test                                          | Covers                                                                |
| --------------------------------------------- | --------------------------------------------------------------------- |
| `test_skill_manage_create_frontmatter`        | LEARN-04: trust_tier, platforms, category, version 1.0.0 in frontmatter; two-level path |
| `test_skill_manage_patch`                     | replacen(1) semantics + `not_found` JSON when old_string absent       |
| `test_skill_manage_schema_actions`            | All 6 action strings appear in JSON schema enum                       |
| `test_skill_manage_path_traversal_rejected`   | `..` and leading `/` both return `path_traversal_rejected`; nothing written |
| `test_skill_manage_create_blocked_content`    | `allowed-tools` SKILL_THREAT_PATTERN triggers `content_rejected`; no file written |
| `test_skill_manage_edit_overwrites`           | edit overwrites; scan runs first                                      |
| `test_skill_manage_delete_removes_dir`        | delete + idempotent `not_found` on repeat                             |

Test isolation: `HermesHomeGuard` RAII guard sets `IRONHERMES_HOME` to a `TempDir` and restores the previous value on drop, with a process-wide `HOME_LOCK` mutex serializing env mutation across parallel tests.

## Commits

| Commit     | Subject                                                          |
| ---------- | ---------------------------------------------------------------- |
| `f2795b9f` | feat(33-02): implement SkillManageTool with 6-action dispatch    |
| `e2fa8cd1` | feat(33-02): add register_skill_manage_tool to ToolRegistry      |

## Verification

| Command                                                                   | Result                                |
| ------------------------------------------------------------------------- | ------------------------------------- |
| `cargo test -p ironhermes-tools --lib -- skill_manage`                    | 7 passed; 0 failed                    |
| `cargo build --workspace 2>&1 \| grep '^error' \| wc -l`                  | 0                                     |
| `grep -c "fn action_" crates/ironhermes-tools/src/skill_manage.rs`        | 6                                     |
| `grep -c "scan_skill_content" crates/ironhermes-tools/src/skill_manage.rs`| 6 (≥ 3 required)                      |
| `grep "trust_tier.*Self-created" crates/ironhermes-tools/src/skill_manage.rs` | present in frontmatter builder + test |
| `grep "platforms" crates/ironhermes-tools/src/skill_manage.rs`            | 10 occurrences (LEARN-04 field)       |
| `grep "path traversal" crates/ironhermes-tools/src/skill_manage.rs`       | present                               |
| `grep "pub mod skill_manage" crates/ironhermes-tools/src/lib.rs`          | 1 line                                |
| `grep "pub fn register_skill_manage_tool" crates/ironhermes-tools/src/registry.rs` | 1 line                                |

## Success Criteria (from PLAN)

- [x] 7 unit tests for skill_manage pass — `cargo test -p ironhermes-tools --lib -- skill_manage` → 7 passed
- [x] `pub fn register_skill_manage_tool` exists in registry.rs and compiles
- [x] `pub mod skill_manage` in lib.rs
- [x] `cargo build --workspace` exits 0 (no errors)

## Threat-Register Mitigations Realised

| Threat ID  | Mitigation in code                                                                                                    |
| ---------- | --------------------------------------------------------------------------------------------------------------------- |
| T-33-02-A  | `resolve_skill_file_path` rejects `..` and leading `/` before any `fs` call; covered by `test_skill_manage_path_traversal_rejected` |
| T-33-02-B  | `scan_skill_content` called before every write site (create / patch / edit / write_file); JSON `content_rejected` blocks the write |
| T-33-02-C  | Same as T-33-02-B — `scan_skill_content` covers `allowed-tools` privilege-escalation pattern via existing SKILL_THREAT_PATTERNS (covered by `test_skill_manage_create_blocked_content`) |
| T-33-02-D  | `validate_skill_name` for slug; `validate_category` rejects `/`, `..`, leading `.`, and empty                          |
| T-33-02-E  | delete canonicalizes the target and verifies it lives inside canonicalize(HERMES_HOME/skills) before `remove_dir_all`  |
| T-33-02-SC | No new external packages — `accept` per threat register; no slopcheck required                                         |

## Deviations from Plan

**None** — plan executed exactly as written. All deviations to standard plan execution are documented above as Decisions (TDD-integrated commit, error-string convention, category validation parity, canonical-path delete check, RAII test isolation).

## Out-of-Scope Observations

Pre-existing `DEFAULT_SAFE_TOOLS` dead_code warning in `crates/ironhermes-tools/src/delegate_task.rs:45` — unrelated to this plan, not modified. Pre-existing unused-import warnings in `ironhermes-cli` and `iron_hermes_ui` are unrelated to this plan.

## Self-Check: PASSED

- crates/ironhermes-tools/src/skill_manage.rs: FOUND
- crates/ironhermes-tools/src/lib.rs change: FOUND (pub mod skill_manage line present)
- crates/ironhermes-tools/src/registry.rs change: FOUND (register_skill_manage_tool present at line 604)
- commit f2795b9f: FOUND (feat(33-02): implement SkillManageTool with 6-action dispatch)
- commit e2fa8cd1: FOUND (feat(33-02): add register_skill_manage_tool to ToolRegistry)
- cargo build --workspace: 0 errors
- skill_manage tests: 7 passed
