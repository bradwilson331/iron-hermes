---
phase: 14-context-files-soul-md
plan: 02
subsystem: agent
tags: [subdir-discovery, agent-loop, context-injection, file-tools]
dependency_graph:
  requires: [context_loader module]
  provides: [SubdirDiscovery, AgentLoop progressive context injection]
  affects: [ironhermes-agent, agent_loop]
tech_stack:
  added: []
  patterns: [visited-set dedup, depth-limited parent walk, post-tool-call injection]
key_files:
  created:
    - crates/ironhermes-agent/src/subdir_discovery.rs
  modified:
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/lib.rs
decisions:
  - "Depth limit is 5 parent directories per CTX-04/D-06"
  - "Canonicalize paths for visited-set to prevent symlink/traversal dedup failures"
  - "Save tool path arg before args ownership moves to execute_tool to avoid borrow-after-move"
  - "Context appended only to Ok results — errors do not get discovery injection"
metrics:
  duration: 10m
  completed: "2026-04-12T07:45:00Z"
  tasks_completed: 2
  files_changed: 3
---

# Phase 14 Plan 02: SubdirDiscovery and AgentLoop Wiring Summary

SubdirDiscovery module created with progressive context discovery; AgentLoop wired to inject context from subdirectories on file-access tool calls.

## What Was Built

### Task 1: subdir_discovery.rs (commit c43995a)

New module `crates/ironhermes-agent/src/subdir_discovery.rs`:

- `SubdirDiscovery` struct with `HashSet<PathBuf>` for visited-dir tracking
- `check_path(&mut self, file_path: &Path) -> Option<String>`: walks upward from file's directory, checking up to 5 parent directories. Uses CONTEXT_CANDIDATES priority chain. Strips .hermes.md frontmatter, scans all content for injection, truncates to 20K cap.
- Each directory checked at most once per session (canonicalized paths in visited set)
- Empty context files are skipped, walking continues to parent

7 unit tests covering: discovery, visited-once dedup, depth limit, priority chain, frontmatter stripping, injection scanning, empty file handling.

### Task 2: AgentLoop wiring (commit 6e232bb)

Modified `crates/ironhermes-agent/src/agent_loop.rs`:

- Added `subdir_discovery: Option<Arc<std::sync::Mutex<SubdirDiscovery>>>` field
- Added `with_subdir_discovery()` builder method
- In `execute_tool_call()` Ok arm: saves `args["path"]` before ownership moves to `execute_tool`, then checks `FILE_ACCESS_TOOLS` allowlist (`read_file`, `write_file`, `patch`, `search_files`). On match, calls `discovery.check_path()` and appends result to tool output.
- Err arm unchanged — no context injection on errors.

## Deviations

- Fixed borrow-after-move: `args` is consumed by `execute_tool()`, so path is extracted beforehand.
- Adjusted scan test to use actual threat pattern (`ignore previous instructions`) instead of `<|im_start|>` which the scanner doesn't detect.

## Self-Check: PASSED

All acceptance criteria met:
- subdir_discovery.rs exists with SubdirDiscovery, check_path, HashSet, depth limit
- Uses CONTEXT_CANDIDATES from context_loader (not hardcoded)
- agent_loop.rs has optional SubdirDiscovery, FILE_ACCESS_TOOLS allowlist, check_path in Ok arm only
- 78 agent tests pass, workspace compiles clean
