---
phase: 01-context-file-loading
plan: 02
subsystem: agent
tags: [rust, prompt-builder, context-loading, security-scanning, cli]

# Dependency graph
requires:
  - phase: 01-context-file-loading/01-01
    provides: context_scanner module and PromptBuilder.load_context() method (implemented inline as Rule 3 deviation)
provides:
  - context_scanner.rs with 10 threat patterns, invisible unicode detection, and 70/20 truncation
  - PromptBuilder rewritten with layered loading: SOUL.md > project context > AGENTS.md
  - CLI binary wired to load context at session start (frozen-snapshot)
affects: [02-telegram-gateway, 03-security-hardening, all phases using PromptBuilder]

# Tech tracking
tech-stack:
  added: [regex (workspace), tempfile (dev-dep)]
  patterns:
    - Frozen-snapshot context loading — cwd captured once, system_msg built once, never rebuilt
    - Priority chain for project context — first match wins (.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules)
    - Security scanning before any context injection — scan_context_content wraps all file reads

key-files:
  created:
    - crates/ironhermes-agent/src/context_scanner.rs
  modified:
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-agent/Cargo.toml
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/src/constants.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/client.rs
    - crates/ironhermes-agent/src/context_compressor.rs
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-gateway/src/telegram.rs
    - crates/ironhermes-state/src/lib.rs
    - crates/ironhermes-tools/src/file_tools.rs

key-decisions:
  - "SOUL.md loaded from IRONHERMES_HOME (not cwd) — home directory is the personality store"
  - "Project context uses first-match-wins priority chain — only ONE file loads per session"
  - "All context content scanned before injection — 10 threat patterns + invisible unicode"
  - "Frozen-snapshot: cwd captured and context loaded once at session start, never reloaded"
  - "Pre-existing clippy warnings fixed across workspace to satisfy -D warnings requirement"

patterns-established:
  - "PromptBuilder.load_context(&cwd): call once before build_system_message(), never call again"
  - "scan_context_content() wraps every std::fs::read_to_string() for context files"
  - "truncate_content() applies after scan, before storing to prevent prompt bloat"

requirements-completed: [CTX-01, CTX-03, CTX-04, CTX-05]

# Metrics
duration: 45min
completed: 2026-04-01
---

# Phase 1 Plan 2: Context File Loading — CLI Wiring Summary

**PromptBuilder rewritten with SOUL.md/project-context/AGENTS.md layered loading wired into both CLI entry points, with regex-based prompt injection scanning on all context files**

## Performance

- **Duration:** ~45 min
- **Started:** 2026-04-01T00:00:00Z
- **Completed:** 2026-04-01T00:45:00Z
- **Tasks:** 2 (+ Plan 01-01 prerequisite work as Rule 3 deviation)
- **Files modified:** 14

## Accomplishments
- Created `context_scanner.rs` with 10 threat pattern regex set, invisible unicode detection, and 70%/20% head/tail truncation at 20K chars — all ported from hermes-agent
- Rewrote `PromptBuilder` to load SOUL.md from `IRONHERMES_HOME`, project context via first-match-wins priority chain, and AGENTS.md from `IRONHERMES_HOME`; all scanned and truncated before injection
- Wired `load_context(&cwd)` into both `run_single()` and `run_chat()` in the CLI — context frozen at session start, never reloaded mid-session
- 29 workspace tests passing, `cargo clippy --workspace -- -D warnings` clean

## Task Commits

Each task was committed atomically:

1. **Deviation: context_scanner + PromptBuilder rewrite** - `3649996` (feat)
2. **Task 1: Wire load_context() into CLI entry points** - `b8bbd66` (feat)
3. **Task 2: Full build/test verification + clippy fixes** - `5666b21` (fix)

## Files Created/Modified
- `crates/ironhermes-agent/src/context_scanner.rs` - Security scanner with 10 threat patterns and truncation
- `crates/ironhermes-agent/src/prompt_builder.rs` - Rewritten with layered context loading
- `crates/ironhermes-agent/src/lib.rs` - Added context_scanner module and re-exports
- `crates/ironhermes-agent/Cargo.toml` - Added regex and tempfile dev-dep
- `crates/ironhermes-cli/src/main.rs` - load_context(&cwd) wired at both entry points
- `crates/ironhermes-core/src/config.rs` - Fixed derivable Default impls (clippy)
- `crates/ironhermes-core/src/constants.rs` - Collapsed nested if-let (clippy)
- `crates/ironhermes-agent/src/agent_loop.rs` - Added ToolCallDelta type alias, collapsed if-let (clippy)
- `crates/ironhermes-agent/src/client.rs` - Published ToolCallDelta type alias, collapsed if-let (clippy)
- `crates/ironhermes-agent/src/context_compressor.rs` - &mut Vec->slice, needless_range_loop fix (clippy)
- `crates/ironhermes-gateway/src/runner.rs` - Collapsed nested if-let (clippy)
- `crates/ironhermes-gateway/src/telegram.rs` - #[allow(dead_code)] on date field (clippy)
- `crates/ironhermes-state/src/lib.rs` - Redundant closure fix (clippy)
- `crates/ironhermes-tools/src/file_tools.rs` - Collapsed nested if-let (clippy)

## Decisions Made
- Used `LazyLock` (stable Rust 1.80+) instead of `once_cell` for `THREAT_PATTERNS` — project uses Rust 2024 edition, `LazyLock` is in std
- Made `ToolCallDelta` a `pub type` alias in `client.rs` to share between `client.rs` and `agent_loop.rs` without duplicating the complex tuple type
- Fixed pre-existing clippy warnings across the entire workspace to satisfy the `-D warnings` success criterion — documented as deviation but scoped to what was blocking the build

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Implemented Plan 01-01 prerequisite work inline**
- **Found during:** Task 1 setup
- **Issue:** Plan 01-01 was not executed. `context_scanner.rs` did not exist. `PromptBuilder` had no `load_context()` instance method. Both are required by Plan 01-02's CLI wiring task.
- **Fix:** Implemented the full Plan 01-01 scope: created `context_scanner.rs` with all 10 threat patterns, truncation, and tests; rewrote `PromptBuilder` with SOUL.md/project-context/AGENTS.md loading. Added `tempfile` dev-dep for tests.
- **Files modified:** context_scanner.rs (new), prompt_builder.rs, lib.rs, Cargo.toml
- **Verification:** `cargo test -p ironhermes-agent -- --test-threads=1` — 20 tests pass
- **Committed in:** `3649996`

**2. [Rule 3 - Blocking] Fixed pre-existing clippy -D warnings failures across workspace**
- **Found during:** Task 2 (full build and test verification)
- **Issue:** `cargo clippy --workspace -- -D warnings` was a stated success criterion. Multiple pre-existing violations existed in `ironhermes-core`, `ironhermes-agent`, `ironhermes-state`, `ironhermes-tools`, and `ironhermes-gateway`.
- **Fix:** Fixed 9 files: derivable Default impls, collapsible-if patterns, redundant closure, complex type alias, &mut Vec->slice, needless_range_loop, dead_code field.
- **Files modified:** config.rs, constants.rs, agent_loop.rs, client.rs, context_compressor.rs, runner.rs, telegram.rs, lib.rs (state), file_tools.rs
- **Verification:** `cargo clippy --workspace -- -D warnings` exits 0
- **Committed in:** `5666b21`

---

**Total deviations:** 2 auto-fixed (2 blocking)
**Impact on plan:** Both auto-fixes were required to complete the plan's success criteria. No scope creep beyond what was strictly necessary.

## Issues Encountered
- `CONTEXT_FILE_MAX_CHARS` const re-export attempted via `pub use` before making it `pub` — compilation error caught immediately, fixed by making the const `pub` and importing it in `prompt_builder.rs`
- Truncation test assertion used `find("[...truncated")` but the marker starts with `"\n\n["` — the position was off by 2; fixed test to use `find("\n\n[...truncated")`
- `build()` method initially used `Box::leak` for platform_hint (memory leak) — immediately caught and replaced with `Vec<String>` approach

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 1 context file loading is fully complete: scanner, builder, and CLI wiring all implemented and tested
- Ready to proceed to Phase 2 (Telegram Gateway) — the PromptBuilder is now ready to inject agent personality into any platform adapter
- IRONHERMES_HOME must be set (or defaults to `~/.ironhermes`) for SOUL.md and AGENTS.md to load; the agent runs fine without them using default identity

---
*Phase: 01-context-file-loading*
*Completed: 2026-04-01*

## Self-Check: PASSED

- context_scanner.rs: FOUND
- prompt_builder.rs: FOUND
- main.rs: FOUND
- Commit 3649996: FOUND
- Commit b8bbd66: FOUND
- Commit 5666b21: FOUND
