---
phase: 01-context-file-loading
plan: "01"
subsystem: agent
tags: [context-loading, security, prompt-builder, tdd]
dependency-graph:
  requires: []
  provides: [context-scanner, prompt-builder-layered-loading]
  affects: [ironhermes-agent]
tech-stack:
  added: [regex, tempfile, serial_test]
  patterns: [LazyLock-static-regex, head-tail-truncation, first-match-priority-chain, scan-before-inject]
key-files:
  created:
    - crates/ironhermes-agent/src/context_scanner.rs
  modified:
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-agent/Cargo.toml
decisions:
  - "Used std::sync::LazyLock (stable Rust 1.80+) instead of once_cell for THREAT_PATTERNS RegexSet — no extra dependency needed given Rust 2024 edition"
  - "Added serial_test crate for env-var isolation in prompt_builder tests — env var mutation is unsafe in Rust 2024 and requires unsafe blocks"
  - "Pre-existing clippy warnings in client.rs, agent_loop.rs, context_compressor.rs, and ironhermes-core deferred — out of scope for this plan"
metrics:
  duration: "5 minutes"
  completed: "2026-04-02"
  tasks-completed: 2
  files-created: 1
  files-modified: 3
---

# Phase 1 Plan 01: Context File Loading — Security Scanner and PromptBuilder Summary

**One-liner:** Security-scanning context loader with 10 threat patterns, invisible unicode detection, 20K-char head/tail truncation, and layered SOUL.md > project context > AGENTS.md assembly order.

## What Was Built

### Task 1: context_scanner module
New file `crates/ironhermes-agent/src/context_scanner.rs` implementing:
- `scan_context_content(content, filename)` — checks 10 threat patterns (ported from hermes-agent Python) via `RegexSet` plus invisible unicode character detection; returns original content or a `[BLOCKED: ...]` message
- `truncate_content(content, filename, max_chars)` — 20K char limit with 70% head / 20% tail split and a human-readable marker; UTF-8 safe using char iterator
- `THREAT_PATTERNS` static `RegexSet` via `std::sync::LazyLock`
- Exported from `lib.rs` as `pub use context_scanner::{scan_context_content, truncate_content}`
- 12 tests covering all threat types, safe passthrough, and truncation math

### Task 2: PromptBuilder rewrite
Rewrote `crates/ironhermes-agent/src/prompt_builder.rs` to replace the stub `load_context_files` static with proper layered loading:
- `load_context(cwd: &Path)` — single entry point that calls all three load methods
- `load_soul_md()` — reads `$IRONHERMES_HOME/SOUL.md`, scans, truncates, stores as identity
- `load_project_context(cwd)` — priority chain `.hermes.md > HERMES.md > AGENTS.md > agents.md > CLAUDE.md > claude.md > .cursorrules` (first match wins)
- `load_agents_md()` — reads `$IRONHERMES_HOME/AGENTS.md`, wraps as `## AGENTS.md\n\n{content}`
- `build()` — assembly order: identity (SOUL or default) > platform hint > tool guidance > project context > home AGENTS.md
- 6 tests covering default identity, soul override, priority chain, assembly order, empty file skip

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| Task 1 | `d58bada` | feat(01-01): create context_scanner module for security scanning |
| Task 2 | `d19a7a0` | feat(01-01): rewrite PromptBuilder with layered context loading |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Rust 2024 requires unsafe blocks for env var mutation**
- **Found during:** Task 2 compilation
- **Issue:** `std::env::set_var` and `remove_var` are marked unsafe in Rust 2024 edition; calling them without `unsafe {}` block is a compile error
- **Fix:** Wrapped all test `set_var`/`remove_var` calls in `unsafe {}` blocks
- **Files modified:** `crates/ironhermes-agent/src/prompt_builder.rs`
- **Commit:** included in `d19a7a0`

**2. [Rule 2 - Missing critical functionality] `#[allow(dead_code)]` on `model` field**
- **Found during:** Task 2 compilation
- **Issue:** The `model` field is retained per plan spec but triggers `dead_code` warning which would fail `-D warnings`
- **Fix:** Added `#[allow(dead_code)]` attribute to the field
- **Files modified:** `crates/ironhermes-agent/src/prompt_builder.rs`
- **Commit:** included in `d19a7a0`

### Deferred (Out of Scope)

Pre-existing clippy warnings in files not modified by this plan logged to `.planning/phases/01-context-file-loading/deferred-items.md`:
- `client.rs`: type_complexity, collapsible_if
- `agent_loop.rs`: collapsible_if, type_complexity
- `context_compressor.rs`: ptr_arg, needless_range_loop, collapsible_if
- `ironhermes-core/src/config.rs`: derivable_impls
- `ironhermes-core/src/constants.rs`: collapsible_if

## Known Stubs

None. All context loading is wired to real file system reads with full error handling.

## Verification Results

```
cargo test -p ironhermes-agent -- --test-threads=1
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cargo build -p ironhermes-agent
Finished `dev` profile [unoptimized + debuginfo]

cargo clippy -p ironhermes-agent --no-deps -- -D warnings
No errors in context_scanner.rs or prompt_builder.rs (pre-existing issues in other files deferred)
```

## Self-Check: PASSED

- context_scanner.rs: FOUND
- prompt_builder.rs: FOUND
- 01-01-SUMMARY.md: FOUND
- commit d58bada: FOUND
- commit d19a7a0: FOUND
