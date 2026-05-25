---
phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl
plan: "02"
subsystem: ironhermes-cli/tui
tags: [tui, cli, tokio, watch-channel, crossterm, render-task, cancellation-token]
dependency_graph:
  requires:
    - tui::ActivityState (from Plan 21-01)
    - tui::StatusLineState + render_status_line (from Plan 21-01)
    - tui::knight_rider::frame (from Plan 21-01)
  provides:
    - tui::TuiHandle (watch publishers + render task handle)
    - tui::render::FRAME_PERIOD
    - tui::render::spawn_render_task (implicit via TuiHandle::new)
  affects:
    - crates/ironhermes-cli/src/tui/mod.rs (added pub mod render + pub use render::TuiHandle)
    - crates/ironhermes-cli/src/skills_cmd.rs (Rule 1 pre-existing clippy fix)
tech_stack:
  added: []
  patterns:
    - tokio::sync::watch for ActivityState + StatusLineState (latest-wins, no backpressure)
    - tokio_util::sync::CancellationToken for cooperative render task shutdown
    - crossterm absolute cursor positioning: SavePosition/Hide/MoveTo/Clear/Print/Show/RestorePosition
    - D-17 non-tty fallback: render_loop exits on first !is_tty() check
    - SIGWINCH tolerance: size() re-queried every tick (no cached rows)
    - Pitfall 3 flicker guard: cursor Hidden for full frame duration
key_files:
  created:
    - crates/ironhermes-cli/src/tui/render.rs
  modified:
    - crates/ironhermes-cli/src/tui/mod.rs
    - crates/ironhermes-cli/src/skills_cmd.rs
decisions:
  - "shutdown(self) consumes self (not &self) — single JoinHandle await, no Arc/Mutex needed at this scope (Wave 3 will wrap in Arc<TuiHandle> per W3)"
  - "ActivityState::Thinking absent per W6 — only Idle/Streaming/ToolCall{name}; redraw match has no Thinking arm"
  - "D-17 implemented by checking stderr().is_tty() as first statement in render_loop; non-tty path awaits shutdown.cancelled() then returns"
  - "collapsible_if in shutdown cleanup fixed with let-else guard pattern"
  - "dead_code/unused_imports lint suppressed at tui module level with #![allow(dead_code, unused_imports)] — all items used in Wave 3"
  - "Pre-existing collapsible match in skills_cmd.rs fixed (Rule 1 auto-fix)"
metrics:
  duration_min: 17
  completed_date: "2026-04-16"
  tasks_completed: 1
  tasks_total: 1
  files_created: 1
  files_modified: 2
  tests_added: 5
---

# Phase 21 Plan 02: Activity Watch and Render Task Summary

**One-liner:** TuiHandle with two tokio::sync::watch channels (ActivityState + StatusLineState), a 100ms crossterm render loop writing to stderr with D-17 non-tty fallback, and 5 lifecycle tests exercising the non-tty code path (cargo test environment).

## What Was Built

One new source file plus two modified files:

| File | Role | Lines |
|------|------|-------|
| `tui/render.rs` | TuiHandle + render_loop + redraw + 5 tests | 252 |
| `tui/mod.rs` | Added `pub mod render` + `pub use render::TuiHandle` + module-level allow | 24 |
| `skills_cmd.rs` | Rule 1: collapsed manual match → `.unwrap_or_default()` | -3 |

### TuiHandle API

```rust
pub struct TuiHandle { /* activity_tx, status_tx, shutdown, task */ }

impl TuiHandle {
    pub fn new(initial_status: StatusLineState) -> Self
    pub fn set_activity(&self, state: ActivityState)   // non-blocking, best-effort
    pub fn set_status(&self, state: StatusLineState)   // non-blocking, best-effort
    pub async fn shutdown(mut self)                    // cancels + awaits render task
    #[cfg(test)] pub fn new_for_tests() -> Self        // forces non-tty path
}
```

### Render Loop

- Ticks every `FRAME_PERIOD` (100ms, D-07)
- D-17: exits immediately if `stderr().is_tty()` is false (pipe / CI / ssh)
- Reads `activity_rx.borrow().clone()` and `status_rx.borrow().clone()` each tick
- Calls `redraw(tick, &activity, &status)` — errors logged via `tracing::debug!`, never panic
- `MissedTickBehavior::Skip` prevents tick backlog (T-21-05)

### Redraw Function

- `size()` queried each tick (SIGWINCH-tolerant, T-21-06)
- Guards `rows < 3` to avoid tiny-terminal collisions
- Frame sequence: `SavePosition → Hide → MoveTo(scanner_row) → Clear → [Print scanner] → MoveTo(bottom) → Clear → Print(status) → Show → RestorePosition → flush`
- Scanner visible iff `ActivityState != Idle` (D-08); includes label: `"Streaming".dimmed()` or `"Running:".dimmed() + name.yellow()`

## Test Coverage

| Test | What it covers |
|------|----------------|
| `construct_and_shutdown_no_panic` | Basic lifecycle — new + shutdown |
| `set_activity_published_to_receiver` | watch channel delivers ActivityState::Streaming |
| `set_status_published_to_receiver` | watch channel delivers StatusLineState snapshot |
| `shutdown_after_set_does_not_panic` | set then shutdown — no panic or hang |
| `double_construct_shutdown_is_safe` | Two sequential TuiHandles — no deadlock |

All 5 tests use the non-tty path (cargo test captures stderr → `!is_tty()`). This is the correct test path — real terminal rendering is D-22 manual verification (Wave 3 scope).

**Test count delta:** 25 (Wave 1) → 30 (Wave 2). All 30 pass.

## Verification Results

```
cargo build -p ironhermes-cli           → Finished (expected dead_code warnings, no errors)
cargo test -p ironhermes-cli --bin ironhermes tui::
                                        → 30 passed, 0 failed
cargo clippy -p ironhermes-cli --no-deps → 0 errors in tui/ files
                                          (pre-existing errors in batch/, memory_setup.rs, main.rs)
git diff HEAD -- crates/ironhermes-cli/Cargo.toml → empty (no new deps, D-18 / INV-6)
rg "println!" crates/ironhermes-cli/src/tui/      → 0 matches (INV-5)
rg "SavePosition" crates/ironhermes-cli/src/tui/render.rs → present (INV-4)
rg "RestorePosition" crates/ironhermes-cli/src/tui/render.rs → present (INV-4)
main.rs NOT modified (per plan constraint)
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Removed collapsible nested if in shutdown cleanup**

- **Found during:** Clippy run after initial implementation
- **Issue:** `if out.is_tty() { if let Ok(...) = size() { ... } }` triggers `clippy::collapsible_if`
- **Fix:** Converted inner `if let` to `let-else` guard statement
- **Files modified:** `crates/ironhermes-cli/src/tui/render.rs`
- **Commit:** 815e8d3

**2. [Rule 1 - Bug] Fixed pre-existing manual_unwrap_or_default in skills_cmd.rs**

- **Found during:** Clippy run scoped to ironhermes-cli
- **Issue:** `match HubManifest::load_or_default() { Ok(m) => m, Err(_) => HubManifest::default() }` flagged by `clippy::manual_unwrap_or_default`
- **Fix:** Replaced with `HubManifest::load_or_default().unwrap_or_default()`
- **Files modified:** `crates/ironhermes-cli/src/skills_cmd.rs`
- **Commit:** 815e8d3

### Pre-existing Clippy Failures (Out of Scope)

`cargo clippy -p ironhermes-cli --no-deps -- -D warnings` exits non-zero due to pre-existing issues in:

- `crates/ironhermes-cli/src/batch/types.rs` — unused field
- `crates/ironhermes-cli/src/batch/runner.rs` — unused function + collapsible_if (×2)
- `crates/ironhermes-cli/src/batch/sharegpt.rs` — collapsible_if
- `crates/ironhermes-cli/src/batch/filters.rs` — collapsible_if (×2) + struct_update_default
- `crates/ironhermes-cli/src/memory_setup.rs` — collapsible_if (×2)
- `crates/ironhermes-cli/src/main.rs` — collapsible_if (×2)

Verified pre-existing: `git stash` before our changes → 1 error (skills_cmd.rs, now fixed); stash pop → these errors were present at base commit. Zero errors exist in any file under `crates/ironhermes-cli/src/tui/`. Logged to deferred-items scope.

### Dead Code Suppression

`#![allow(dead_code, unused_imports)]` added to `tui/mod.rs` to suppress Wave-3-pending items. All tui public items (TuiHandle, ActivityState, StatusLineState, etc.) will be used in Plan 21-03 when `run_chat` is wired. The allow attribute will be removed or narrowed at that time.

### Note on main.rs

`run_chat` was NOT modified in this plan. TuiHandle is introduced but not yet spawned from any caller. Integration is Wave 3 scope (Plan 21-03).

## Known Stubs

None — TuiHandle is fully implemented. The render task writes real crossterm output to real stderr when a TTY is detected. The non-tty path is a deliberate graceful-degradation feature (D-17), not a stub.

## Threat Flags

None — no new network endpoints, auth paths, file access patterns, or schema changes introduced. The render task writes to stderr only (INV-4 confirmed via SavePosition/RestorePosition present).

## Self-Check: PASSED

- `crates/ironhermes-cli/src/tui/render.rs` exists (252 lines)
- Commit 815e8d3 exists in git log
- 30 tui tests pass (25 Wave 1 + 5 Wave 2)
- `pub use render::TuiHandle` present in tui/mod.rs
- No ActivityState::Thinking arm in redraw (W6)
- Cargo.toml unchanged
- main.rs NOT modified
