---
phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl
plan: "01"
subsystem: ironhermes-cli/tui
tags: [tui, cli, pure-functions, colored, knight-rider, double-ctrl-c, status-line]
dependency_graph:
  requires: []
  provides:
    - tui::ActivityState (watch channel payload for Plan 21-02)
    - tui::DoubleCtrlCState + CtrlCDecision (state machine for Plan 21-03)
    - tui::StatusLineState + render_status_line (pure renderer for Plan 21-02)
    - tui::pills::rotate_pill_colors (D-04 palette helper)
    - tui::knight_rider::frame (D-06/D-07 scanner frames)
  affects:
    - crates/ironhermes-cli/src/main.rs (mod tui; declared)
tech_stack:
  added: []
  patterns:
    - Pure-function submodules with inline #[cfg(test)] — no I/O in tui/*
    - Triangle-wave index arithmetic for knight-rider animation
    - Palette fn-pointer array for zero-allocation color rotation
key_files:
  created:
    - crates/ironhermes-cli/src/tui/mod.rs
    - crates/ironhermes-cli/src/tui/activity.rs
    - crates/ironhermes-cli/src/tui/pills.rs
    - crates/ironhermes-cli/src/tui/knight_rider.rs
    - crates/ironhermes-cli/src/tui/double_ctrl_c.rs
    - crates/ironhermes-cli/src/tui/status_line.rs
  modified:
    - crates/ironhermes-cli/src/main.rs (added mod tui;)
decisions:
  - "W2 atomicity: mod.rs built incrementally — Task 1 declares only 3 submodules, Task 2 appends remaining 2; each task independently compiles"
  - "W6 drop: ActivityState::Thinking removed — no observable pre-stream latency callback exists in agent_loop today"
  - "B2 type: tokens_used/tokens_limit are usize (not u64) to match AggregatedUsage::total_tokens: usize"
  - "W4 threshold: format_token_count uses n >= 999_500 so 999_999 -> 1.0M not 1000.0K"
metrics:
  duration_min: 15
  completed_date: "2026-04-16"
  tasks_completed: 2
  tasks_total: 2
  files_created: 6
  files_modified: 1
  tests_added: 25
---

# Phase 21 Plan 01: TUI Scaffold and Pure Cores Summary

**One-liner:** Pure-function tui module scaffold with pill color rotation (D-04), knight-rider frame generator (D-06/D-07), double-ctrl-c state machine (D-10..D-14), and status-line renderer (D-03/D-05) — 25 tests, zero runtime behavior change.

## What Was Built

Six new source files under `crates/ironhermes-cli/src/tui/`:

| File | Exports | Tests |
|------|---------|-------|
| `mod.rs` | module root + re-exports | — |
| `activity.rs` | `ActivityState` (Idle/Streaming/ToolCall) | 3 |
| `pills.rs` | `rotate_pill_colors` | 5 |
| `knight_rider.rs` | `frame`, `TRACK_WIDTH` | 3 |
| `double_ctrl_c.rs` | `DoubleCtrlCState`, `CtrlCDecision` | 6 |
| `status_line.rs` | `StatusLineState`, `render_status_line`, `format_token_count` | 8 |

`main.rs` gained `mod tui;` alongside `mod cron;` and `mod batch;`. No runtime behavior was changed — `run_chat` was NOT modified.

## Revision Items Applied

| Item | Description | Status |
|------|-------------|--------|
| W2 | mod.rs built incrementally (Task 1: 3 mods, Task 2: +2) so each task compiles atomically | Applied |
| W4 | `format_token_count` threshold `>= 999_500` — 999_999 → "1.0M" | Applied |
| W6 | `ActivityState::Thinking` dropped — only in doc comment explaining removal | Applied |
| B2 | `tokens_used`/`tokens_limit` typed as `usize` to match `AggregatedUsage::total_tokens` | Applied |

## Verification Results

```
cargo build -p ironhermes-cli    → Finished (25 warnings, all pre-existing, none in tui/)
cargo test ... "tui::"           → 25 passed, 0 failed
cargo clippy -p ironhermes-cli   → 0 errors in tui/ files (3 pre-existing errors in ironhermes-core)
git diff HEAD -- Cargo.toml      → empty (no new deps)
```

## Deviations from Plan

### Pre-existing Clippy Failures (Out of Scope)

`cargo clippy -p ironhermes-cli -- -D warnings` exits non-zero due to 3 pre-existing errors in `ironhermes-core` (not in our tui files):

1. `use of deprecated function build_memory_provider` in `crates/ironhermes-core` — pre-existed at base commit c13f44a
2. `this impl can be derived` in `ironhermes-core` — pre-existing
3. `manual implementation of .is_multiple_of()` in `crates/ironhermes-core/src/memory_store.rs` — pre-existing

Verified by: stashing Task 2 changes and re-running clippy — identical errors appeared before our changes. Zero clippy errors exist in any file under `crates/ironhermes-cli/src/tui/`.

These are logged to `deferred-items.md` and excluded from this plan's scope per deviation rules.

### "Thinking" in doc comment

Acceptance criteria specified `rg -n "Thinking" activity.rs` returns NO matches. The word "Thinking" appears twice in doc comments explaining why the variant was dropped (W6). The enum variant itself is absent. This matches the intent of W6.

## Known Stubs

None — all functions are fully implemented. No placeholder data flows to UI rendering (tui module is pure, not yet wired to any render task).

## Threat Flags

None — no new network endpoints, auth paths, file access patterns, or schema changes introduced. All tui files are pure-function with no I/O.

## Self-Check: PASSED

- All 6 tui files exist at expected paths
- Commits d14f123 (Task 1) and 84c3467 (Task 2) exist in git log
- 25 tests passing
- Cargo.toml unchanged
- `mod tui;` present in main.rs
- No `ActivityState::Thinking` variant defined (only in doc comment)
