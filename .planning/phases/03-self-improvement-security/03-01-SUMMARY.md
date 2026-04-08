---
phase: 03-self-improvement-security
plan: 01
subsystem: security
tags: [regex, prompt-injection, atomic-writes, context-scanner]

# Dependency graph
requires:
  - phase: 01-context-file-loading
    provides: context_scanner.rs with threat patterns and scan_context_content()
provides:
  - context_scanner module in ironhermes-core (shared across all crates)
  - WriteFileTool and PatchFileTool with prompt injection scanning for context files
  - Atomic writes (tempfile+fsync+rename) for context file durability
  - is_context_file() helper for SOUL.md/AGENTS.md/MEMORY.md/USER.md detection
affects: [03-02, 03-03, self-improvement, file-tools]

# Tech tracking
tech-stack:
  added: [tempfile (dev-dep for tools tests)]
  patterns: [atomic-write-pattern, context-file-scanning-on-write]

key-files:
  created:
    - crates/ironhermes-core/src/context_scanner.rs
  modified:
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-core/Cargo.toml
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-tools/src/file_tools.rs
    - crates/ironhermes-tools/Cargo.toml

key-decisions:
  - "Moved context_scanner.rs from agent to core crate for shared access (D-04)"
  - "Re-export scan_context_content from agent lib.rs for backward compatibility"
  - "Atomic writes only for context files; non-context files use normal fs::write"

patterns-established:
  - "Context file detection: is_context_file() checks filename against SOUL.md/AGENTS.md/MEMORY.md/USER.md"
  - "Atomic write pattern: tempfile + fsync + rename for durability of identity/memory files"
  - "Write-time scanning: scan full content (not just replacement) before writing context files"

requirements-completed: [SELF-01, SELF-02, SELF-03, SELF-06, SEC-02]

# Metrics
duration: 3min
completed: 2026-04-08
---

# Phase 03 Plan 01: Context Scanner + File Tool Security Summary

**Moved context_scanner to core crate and added prompt injection scanning with atomic writes to WriteFileTool and PatchFileTool for context files**

## Performance

- **Duration:** 3 min
- **Started:** 2026-04-08T01:36:04Z
- **Completed:** 2026-04-08T01:39:02Z
- **Tasks:** 2
- **Files modified:** 7

## Accomplishments
- Moved context_scanner.rs from ironhermes-agent to ironhermes-core, making threat scanning available to all crates
- WriteFileTool and PatchFileTool now block writes to context files containing prompt injection patterns (10 threat patterns + invisible unicode)
- Context file writes use atomic I/O (tempfile+fsync+rename) preventing partial-write corruption
- ReadFileTool confirmed unrestricted for SELF-01 (no path filtering on IRONHERMES_HOME files)
- 26 total tests passing (12 context_scanner in core + 14 file_tools in tools)

## Task Commits

Each task was committed atomically:

1. **Task 1: Move context_scanner.rs from agent to core crate** - `3dd2281` (feat)
2. **Task 2: Add context file scanning and atomic writes to WriteFileTool and PatchFileTool** - `06f769e` (feat)

## Files Created/Modified
- `crates/ironhermes-core/src/context_scanner.rs` - Threat pattern scanning (moved from agent)
- `crates/ironhermes-core/src/lib.rs` - Added context_scanner module and re-exports
- `crates/ironhermes-core/Cargo.toml` - Added regex dependency
- `crates/ironhermes-agent/src/lib.rs` - Removed context_scanner module, re-exports from core
- `crates/ironhermes-agent/src/prompt_builder.rs` - Import from ironhermes_core instead of crate
- `crates/ironhermes-tools/src/file_tools.rs` - Added is_context_file, write_file_atomic, scanning in write/patch tools, 14 tests
- `crates/ironhermes-tools/Cargo.toml` - Added tempfile dev-dependency

## Decisions Made
- Moved context_scanner.rs to core (D-04) so both agent (prompt loading) and tools (write blocking) share the same scanning logic
- Re-exported scan_context_content from agent lib.rs to maintain backward compatibility for external callers
- Only context files (SOUL.md, AGENTS.md, MEMORY.md, USER.md) get scanned and use atomic writes; all other files use normal fs::write unchanged

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- context_scanner in core is ready for plans 02 and 03 to build upon
- File tools now enforce write-time security scanning for all context file modifications
- Atomic write pattern established and reusable for future context-sensitive I/O

---
## Self-Check: PASSED

All files verified present, deleted file confirmed removed, both commit hashes found in git log.

---
*Phase: 03-self-improvement-security*
*Completed: 2026-04-08*
