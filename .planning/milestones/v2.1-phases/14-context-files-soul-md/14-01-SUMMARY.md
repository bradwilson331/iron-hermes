---
phase: 14-context-files-soul-md
plan: 01
subsystem: agent
tags: [context-loading, prompt-builder, security, git-root-walk, frontmatter]
dependency_graph:
  requires: []
  provides: [context_loader module, skip_context_files flag, git-root walk, frontmatter stripping]
  affects: [ironhermes-agent, prompt_builder, subagent_runner]
tech_stack:
  added: []
  patterns: [git-root upward walk, YAML frontmatter stripping, case-sensitive filename matching]
key_files:
  created:
    - crates/ironhermes-agent/src/context_loader.rs
  modified:
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-agent/src/prompt_builder.rs
decisions:
  - "CONTEXT_CANDIDATES uses exact case-sensitive names only — no lowercase variants per D-08"
  - "find_git_root stops at $HOME not above it — prevents loading system-level context files per D-03"
  - "strip_yaml_frontmatter returns input unchanged on malformed frontmatter (no-op, not error)"
  - "skip_context_files flag is on PromptBuilder itself so build() also enforces it (not just load_context)"
metrics:
  duration: 8m
  completed: "2026-04-12T07:08:28Z"
  tasks_completed: 2
  files_changed: 3
---

# Phase 14 Plan 01: ContextLoader Module and PromptBuilder Priority Chain Summary

ContextLoader module created with git-root walk, YAML frontmatter stripping, and case-sensitive priority chain; PromptBuilder updated with skip_context_files flag, .hermes.md upward walk, and frontmatter stripping before injection scanning.

## What Was Built

### Task 1: context_loader.rs (commit f588688)

New module `crates/ironhermes-agent/src/context_loader.rs` with three exports:

- `CONTEXT_CANDIDATES: &[&str]` — case-sensitive priority chain: `.hermes.md`, `AGENTS.md`, `CLAUDE.md`, `.cursorrules`. No lowercase variants, no `HERMES.md`. Per D-08 and T-14-05.
- `find_git_root(start: &Path) -> Option<PathBuf>` — walks upward from `start` checking for `.git` (file or directory, supporting worktrees). Stops at `$HOME`. Returns the first directory containing `.git` or `None`. Per D-01 and D-03. T-14-03 mitigated.
- `strip_yaml_frontmatter(content: &str) -> &str` — strips YAML frontmatter (content between `---` delimiters). Returns input unchanged if no closing `---` found (malformed). Per D-02 and CTX-07. T-14-02 mitigated.

Module added to `lib.rs` as `pub mod context_loader`.

9 unit tests covering all behaviors.

### Task 2: PromptBuilder updates (commit 3f5ddb4)

Updated `crates/ironhermes-agent/src/prompt_builder.rs`:

- Added `skip_context_files: bool` field initialized to `false` in `new()`
- Added `pub fn skip_context_files(mut self) -> Self` builder method
- `load_context()` returns early when `skip_context_files` is true — no SOUL.md, no project context, no AGENTS.md loaded (D-10)
- `build()` always uses `DEFAULT_AGENT_IDENTITY` when `skip_context_files` is true
- `load_project_context()` rewritten:
  - Step 1: Walk upward from CWD looking for `.hermes.md`. Uses `find_git_root()` to determine stop (falls back to `$HOME`). On match, calls `strip_yaml_frontmatter()` FIRST, then `scan_context_content()`, then `truncate_content()`. Per D-01, D-02, D-03, T-14-01, T-14-02, T-14-04.
  - Step 2: If no `.hermes.md` found, checks CWD only for `AGENTS.md`, `CLAUDE.md`, `.cursorrules` (CONTEXT_CANDIDATES indices 1-3). First match wins. No frontmatter stripping for these files.
- All lowercase candidate names (`agents.md`, `claude.md`, `HERMES.md`) removed. Per T-14-05.

13 tests pass (8 existing + 5 new).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed panic in strip_yaml_frontmatter on malformed input**
- **Found during:** Task 1 TDD RED phase (test_strip_frontmatter_malformed)
- **Issue:** Initial implementation used byte offset arithmetic that panicked with `byte index 11 is out of bounds of 'no closing'` when content had `---` but no closing marker
- **Fix:** Rewrote the line-walking loop to use `lines()` iterator with cumulative offset tracking, bounds-checking before slicing
- **Files modified:** crates/ironhermes-agent/src/context_loader.rs
- **Commit:** f588688 (fixed inline in same commit as GREEN phase)

## Threat Surface Scan

All threat mitigations from the plan's threat model are implemented:

| Threat ID | Mitigation | Status |
|-----------|-----------|--------|
| T-14-01 | `scan_context_content()` called on all context files | Implemented |
| T-14-02 | `strip_yaml_frontmatter()` called before `scan_context_content` on .hermes.md | Implemented |
| T-14-03 | `find_git_root()` stops at `$HOME` | Implemented |
| T-14-04 | `truncate_content()` enforces 20K cap on every load path | Implemented |
| T-14-05 | `CONTEXT_CANDIDATES` has exact case-sensitive names only | Implemented |

No new threat surface introduced beyond what was planned.

## Known Stubs

None.

## Self-Check: PASSED

- `crates/ironhermes-agent/src/context_loader.rs` exists: FOUND
- `crates/ironhermes-agent/src/lib.rs` contains `pub mod context_loader`: FOUND
- `crates/ironhermes-agent/src/prompt_builder.rs` contains `skip_context_files`: FOUND
- Commit f588688 exists: FOUND
- Commit 3f5ddb4 exists: FOUND
- All 9 context_loader tests pass
- All 13 prompt_builder tests pass
- `cargo check --workspace` clean
