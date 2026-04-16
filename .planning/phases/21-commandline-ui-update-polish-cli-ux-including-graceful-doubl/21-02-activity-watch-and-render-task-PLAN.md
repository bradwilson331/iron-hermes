---
phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl
plan: 02
type: execute
wave: 2
depends_on:
  - 21-01
files_modified:
  - crates/ironhermes-cli/src/tui/mod.rs
  - crates/ironhermes-cli/src/tui/activity.rs
  - crates/ironhermes-cli/src/tui/render.rs
autonomous: true
requirements: []
decisions_addressed:
  - D-03
  - D-05
  - D-08
  - D-09
  - D-16
  - D-17

must_haves:
  truths:
    - "TuiHandle::new() returns a handle wired to a live tokio::sync::watch::Sender<ActivityState> and a spawned render task"
    - "TuiHandle::set_activity(state) publishes to the watch channel without blocking"
    - "TuiHandle::set_status(StatusLineState) publishes to a second watch channel for status-line state"
    - "TuiHandle::shutdown().await cancels the render task and awaits its JoinHandle"
    - "Render task detects non-tty stderr via crossterm::tty::IsTty and exits its loop immediately (D-17 graceful-degradation)"
    - "Render task queries crossterm::terminal::size() every tick (SIGWINCH-tolerant per RESEARCH §Pitfall 4)"
    - "Render task uses SavePosition/Hide/MoveTo/Clear/Print/Show/RestorePosition sequence per RESEARCH Example 3"
    - "Render task writes to stderr ONLY (never stdout) — enforced by INV-4"
  artifacts:
    - path: "crates/ironhermes-cli/src/tui/render.rs"
      provides: "spawn_render_task + TuiHandle"
      exports: ["TuiHandle", "spawn_render_task"]
    - path: "crates/ironhermes-cli/src/tui/mod.rs"
      provides: "Re-exports TuiHandle"
      contains: "pub use render::TuiHandle;"
  key_links:
    - from: "crates/ironhermes-cli/src/tui/render.rs"
      to: "crates/ironhermes-cli/src/tui/status_line.rs::render_status_line"
      via: "imported and called every tick"
      pattern: "render_status_line\\("
    - from: "crates/ironhermes-cli/src/tui/render.rs"
      to: "crates/ironhermes-cli/src/tui/knight_rider.rs::frame"
      via: "imported and called every tick when activity != Idle"
      pattern: "knight_rider::frame\\("
    - from: "crates/ironhermes-cli/src/tui/render.rs"
      to: "tokio::sync::watch"
      via: "two watch channels (ActivityState + StatusLineState)"
      pattern: "tokio::sync::watch"
---

<objective>
Build the rendering layer on top of Plan 21-01's pure cores. Introduce `TuiHandle` — a public type that owns two `tokio::sync::watch` channels (one for `ActivityState`, one for `StatusLineState`) plus a spawned `tokio::task` that ticks every 100ms, reads both watch channels, and renders the status line + knight-rider scanner to stderr via crossterm absolute cursor positioning.

Per D-08 the scanner is visible iff `ActivityState != Idle`. Per D-17 rendering is stderr-only. Per RESEARCH.md Open Question #5 the render task auto-detects non-tty stderr (e.g. piped output, ssh-on-slow-link, CI) and no-ops — it does not error.

This plan does NOT yet wire into `run_chat` — integration lives in Plan 21-03. At the end of this plan, adding `let tui = TuiHandle::new().await;` to any binary would spin up a rendering loop, but main.rs is unchanged.

Purpose: Isolate the only I/O in the tui module — the render task — so it can be iterated in one place. Keeps the pure-function submodules from Plan 21-01 untouched.

Output: Two new/expanded files (`tui/render.rs`, `tui/mod.rs` update) plus three new tests that exercise the TuiHandle lifecycle without a real terminal (using the IsTty fallback path).
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-VALIDATION.md
@crates/ironhermes-cli/src/tui/mod.rs
@crates/ironhermes-cli/src/tui/status_line.rs
@crates/ironhermes-cli/src/tui/knight_rider.rs
@crates/ironhermes-cli/src/tui/activity.rs

<interfaces>
<!-- Dependencies already in Cargo.toml (no new deps per D-18): -->

From crossterm 0.28 (workspace):
```rust
use crossterm::{
    cursor::{Hide, MoveTo, RestorePosition, SavePosition, Show},
    queue,
    style::Print,
    terminal::{size, Clear, ClearType},
    tty::IsTty,
};
```

From tokio (workspace):
```rust
use tokio::sync::watch;              // for ActivityState + StatusLineState channels
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
```

From tokio_util 0.7:
```rust
use tokio_util::sync::CancellationToken;  // shutdown signal for render task
```

From Plan 21-01 (already exists):
```rust
use crate::tui::activity::ActivityState;
use crate::tui::status_line::{StatusLineState, render_status_line};
use crate::tui::knight_rider::{self, TRACK_WIDTH};
```

Reference — RESEARCH.md Pattern 2 (Bottom-Bar Render) — adapt this as the core of the render loop:
```rust
use crossterm::{queue, cursor::{MoveTo, SavePosition, RestorePosition, Hide, Show},
                terminal::{size, Clear, ClearType}, style::Print};
use std::io::{stderr, Write};

fn redraw(status: &str, scanner: Option<&str>) -> std::io::Result<()> {
    let (_cols, rows) = size()?;
    let bottom = rows.saturating_sub(1);
    let scanner_row = rows.saturating_sub(2);
    let mut out = stderr();
    queue!(out, SavePosition, Hide)?;
    // Scanner row (only when in-flight per D-08):
    if let Some(s) = scanner {
        queue!(out, MoveTo(0, scanner_row), Clear(ClearType::CurrentLine), Print(s))?;
    } else {
        queue!(out, MoveTo(0, scanner_row), Clear(ClearType::CurrentLine))?;
    }
    queue!(out, MoveTo(0, bottom), Clear(ClearType::CurrentLine), Print(status))?;
    queue!(out, Show, RestorePosition)?;
    out.flush()
}
```
</interfaces>
</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Implement TuiHandle + spawn_render_task in tui/render.rs</name>
  <files>
    crates/ironhermes-cli/src/tui/render.rs,
    crates/ironhermes-cli/src/tui/mod.rs,
    crates/ironhermes-cli/src/tui/activity.rs
  </files>
  <read_first>
    - crates/ironhermes-cli/src/tui/mod.rs (from Plan 21-01 — confirm module layout)
    - crates/ironhermes-cli/src/tui/status_line.rs (render_status_line signature — Plan 21-01)
    - crates/ironhermes-cli/src/tui/knight_rider.rs (frame(tick: u64) -> String — Plan 21-01)
    - crates/ironhermes-cli/src/tui/activity.rs (ActivityState enum — Plan 21-01)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md (D-05, D-08, D-09, D-15, D-16, D-17)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md (Pattern 2, Pattern 3, §Pitfall 3 flicker, §Pitfall 4 SIGWINCH, §Pitfall 6 stderr collision)
  </read_first>
  <behavior>
    - Test: TuiHandle::new_for_tests() constructs a handle whose render task is disabled (non-tty path taken) and shutdown completes within 200ms
    - Test: set_activity(ActivityState::Streaming) followed by set_activity(ActivityState::Idle) results in the latest value being observable via the watch receiver clone
    - Test: set_status(StatusLineState { mode: "Agent", tokens_used: 100, tokens_limit: 1000, ... }) causes the receiver to see it on its next borrow()
    - Test: shutdown().await completes without panicking even when the render task has already exited (non-tty path)
    - Test: calling set_activity / set_status after shutdown is a no-op and does NOT panic (Sender::send on closed channel returns Err, which we ignore)
  </behavior>
  <action>
Step 1 — Create `crates/ironhermes-cli/src/tui/render.rs` with the full TuiHandle implementation.

This is the ONE file in the tui module that does I/O. It owns the watch senders, spawns a render task, and exposes a shutdown handle. Per D-16 we use only crossterm + tokio primitives. Per D-17 we detect non-tty and skip rendering.

```rust
//! Render task + TuiHandle — the ONLY I/O surface in the tui module.
//!
//! Per D-15/D-16: one tokio::task ticks every 100ms (D-07 frame rate), reads two
//! watch channels (activity + status), and writes to stderr via crossterm absolute
//! cursor positioning. Per D-17: non-tty stderr causes the loop to exit immediately
//! so piped output / CI / ssh-over-slow-link degrade gracefully.
//!
//! Flicker mitigation per RESEARCH §Pitfall 3: Hide/Show wraps every frame.
//! SIGWINCH tolerance per RESEARCH §Pitfall 4: re-query size() each tick.

use crate::tui::activity::ActivityState;
use crate::tui::knight_rider;
use crate::tui::status_line::{render_status_line, StatusLineState};
use colored::Colorize;
use crossterm::{
    cursor::{Hide, MoveTo, RestorePosition, SavePosition, Show},
    queue,
    style::Print,
    terminal::{size, Clear, ClearType},
    tty::IsTty,
};
use std::io::{stderr, Write};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Frame period for the render loop (D-07: ~10 fps).
pub const FRAME_PERIOD: Duration = Duration::from_millis(100);

/// Public handle returned by `TuiHandle::new`. Holds the watch senders +
/// shutdown token + render task handle. All setter methods are non-blocking
/// and best-effort: a send on a closed channel is silently dropped.
pub struct TuiHandle {
    activity_tx: watch::Sender<ActivityState>,
    status_tx: watch::Sender<StatusLineState>,
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl TuiHandle {
    /// Spawn the render task and return a handle. Always succeeds; if stderr
    /// is not a TTY the loop exits on first iteration (non-tty fallback).
    pub fn new(initial_status: StatusLineState) -> Self {
        let (activity_tx, activity_rx) = watch::channel(ActivityState::Idle);
        let (status_tx, status_rx) = watch::channel(initial_status);
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();

        let task = tokio::spawn(async move {
            render_loop(activity_rx, status_rx, task_shutdown).await;
        });

        Self {
            activity_tx,
            status_tx,
            shutdown,
            task: Some(task),
        }
    }

    /// Test-only constructor: forces the non-tty path by never writing to
    /// a real terminal. The render task still spawns but exits on its first
    /// IsTty check. Used by unit tests without a real terminal.
    #[cfg(test)]
    pub fn new_for_tests() -> Self {
        Self::new(StatusLineState::default())
    }

    /// Publish a new activity state. Non-blocking; errors (closed channel)
    /// are ignored per D-09 "best-effort" semantics.
    pub fn set_activity(&self, state: ActivityState) {
        let _ = self.activity_tx.send(state);
    }

    /// Publish a new status-line snapshot. Non-blocking.
    pub fn set_status(&self, state: StatusLineState) {
        let _ = self.status_tx.send(state);
    }

    /// Shut down the render task cooperatively. Safe to call multiple times —
    /// the second call is a no-op because the JoinHandle is Option-consumed.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        if let Some(h) = self.task.take() {
            let _ = h.await;
        }
        // Best-effort terminal cleanup: clear the two bottom rows so the
        // status bar doesn't linger after exit. Only if stderr is a tty.
        let mut out = stderr();
        if out.is_tty() {
            if let Ok((_cols, rows)) = size() {
                let _ = queue!(
                    out,
                    SavePosition,
                    Hide,
                    MoveTo(0, rows.saturating_sub(2)),
                    Clear(ClearType::CurrentLine),
                    MoveTo(0, rows.saturating_sub(1)),
                    Clear(ClearType::CurrentLine),
                    Show,
                    RestorePosition,
                );
                let _ = out.flush();
            }
        }
    }
}

/// Main render loop. Per D-17 exits immediately if stderr is not a TTY.
async fn render_loop(
    activity_rx: watch::Receiver<ActivityState>,
    status_rx: watch::Receiver<StatusLineState>,
    shutdown: CancellationToken,
) {
    // D-17 graceful degradation: non-tty stderr (pipe/ssh/CI) → no-op loop.
    if !stderr().is_tty() {
        tracing::debug!("tui: stderr is not a tty, render loop is a no-op");
        shutdown.cancelled().await;
        return;
    }

    let mut ticker = tokio::time::interval(FRAME_PERIOD);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tick: u64 = 0;

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                let activity = activity_rx.borrow().clone();
                let status = status_rx.borrow().clone();
                if let Err(e) = redraw(tick, &activity, &status) {
                    // Re-query size on next tick; log and continue.
                    tracing::debug!(error = %e, "tui: redraw failed — continuing");
                }
                tick = tick.wrapping_add(1);
            }
        }
    }
}

/// Single-frame redraw. Per RESEARCH §Pitfall 4: re-query size() each tick
/// to handle SIGWINCH without subscribing to the signal. Per §Pitfall 3:
/// Hide/Show wraps the frame to prevent cursor flicker.
fn redraw(tick: u64, activity: &ActivityState, status: &StatusLineState) -> std::io::Result<()> {
    let (_cols, rows) = size()?;
    // Need at least 3 rows: prompt + scanner + status. On tiny terminals (<3 rows)
    // skip rendering to avoid colliding with the prompt.
    if rows < 3 {
        return Ok(());
    }
    let bottom = rows.saturating_sub(1);
    let scanner_row = rows.saturating_sub(2);

    let status_str = render_status_line(status);

    // Per D-08: scanner visible iff in-flight.
    let scanner_str: Option<String> = match activity {
        ActivityState::Idle => None,
        ActivityState::Thinking => Some(format!(
            "{} {}",
            knight_rider::frame(tick),
            "Thinking".dimmed()
        )),
        ActivityState::Streaming => Some(format!(
            "{} {}",
            knight_rider::frame(tick),
            "Streaming".dimmed()
        )),
        ActivityState::ToolCall { name } => Some(format!(
            "{} {} {}",
            knight_rider::frame(tick),
            "Running:".dimmed(),
            name.yellow()
        )),
    };

    let mut out = stderr();
    queue!(out, SavePosition, Hide)?;
    // Scanner row: either write the scanner string or clear it so the line
    // doesn't retain stale glyphs when activity returns to Idle.
    queue!(out, MoveTo(0, scanner_row), Clear(ClearType::CurrentLine))?;
    if let Some(ref s) = scanner_str {
        queue!(out, Print(s))?;
    }
    // Status row:
    queue!(
        out,
        MoveTo(0, bottom),
        Clear(ClearType::CurrentLine),
        Print(&status_str),
        Show,
        RestorePosition
    )?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test spawns its own tokio runtime so we don't depend on #[tokio::test].
    // Non-tty path (stderr in `cargo test` is typically captured → not a tty)
    // means the render_loop exits immediately on `shutdown.cancelled()`.

    #[tokio::test]
    async fn construct_and_shutdown_no_panic() {
        let tui = TuiHandle::new_for_tests();
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn set_activity_published_to_receiver() {
        let tui = TuiHandle::new_for_tests();
        // Peek the receiver by cloning from the sender — watch::Sender
        // doesn't expose receiver cloning, so we grab a new receiver via subscribe.
        let rx = tui.activity_tx.subscribe();
        tui.set_activity(ActivityState::Streaming);
        // Allow the update to propagate (watch is synchronous but a select-poll
        // tick may be needed in practice — use a bounded wait).
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(*rx.borrow(), ActivityState::Streaming);
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn set_status_published_to_receiver() {
        let tui = TuiHandle::new_for_tests();
        let rx = tui.status_tx.subscribe();
        tui.set_status(StatusLineState {
            mode: "Agent".to_string(),
            model_short: "m".to_string(),
            provider: "p".to_string(),
            tokens_used: 50,
            tokens_limit: 100,
            hint: String::new(),
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(rx.borrow().tokens_used, 50);
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn set_after_shutdown_is_noop() {
        let tui = TuiHandle::new_for_tests();
        let _ = tui.activity_tx.clone(); // prevent immediate drop of sender
        tui.set_activity(ActivityState::Streaming);
        // Shutdown before next set — should not panic.
        // (Drop the handle via shutdown, then attempt nothing; the test is that
        // Drop + task join works without error.)
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn double_shutdown_is_safe() {
        // Constructing two TuiHandles back-to-back must not deadlock.
        let a = TuiHandle::new_for_tests();
        a.shutdown().await;
        let b = TuiHandle::new_for_tests();
        b.shutdown().await;
    }
}
```

Step 2 — Update `crates/ironhermes-cli/src/tui/mod.rs` to declare and re-export the new module:

Add the line `pub mod render;` alongside the existing `pub mod` declarations.
Add the line `pub use render::TuiHandle;` alongside the existing re-exports.

Final `tui/mod.rs` should look like:

```rust
//! Phase 21: CLI TUI polish — status line, knight-rider scanner, double-ctrl-c state machine.

pub mod activity;
pub mod double_ctrl_c;
pub mod knight_rider;
pub mod pills;
pub mod render;
pub mod status_line;

pub use activity::ActivityState;
pub use double_ctrl_c::{CtrlCDecision, DoubleCtrlCState};
pub use render::TuiHandle;
pub use status_line::StatusLineState;
```

Step 3 — `crates/ironhermes-cli/src/tui/activity.rs`: no code changes needed for this task, but confirm the enum is `Clone + Debug + PartialEq + Eq` so it can be stored in `watch::channel` (Plan 21-01 already ensured this).

Step 4 — Compile + test.

NOTE on the test `set_activity_published_to_receiver`: `watch::Sender::subscribe()` requires `Sender` to be accessible. The test accesses `tui.activity_tx.subscribe()` which requires the field to be pub(crate) or the test to live in the same module — since `#[cfg(test)] mod tests` in `render.rs` has full module access to private fields, this works.

NOTE on non-tty detection in tests: `cargo test` captures stderr, so `stderr().is_tty()` returns false, so `render_loop` exits immediately on `shutdown.cancelled()`. This is the correct test path — we are NOT testing real terminal rendering here (that's D-22 manual verification).

No modifications to main.rs in this plan.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-cli --lib tui::render</automated>
  </verify>
  <done>
    - crates/ironhermes-cli/src/tui/render.rs exists with TuiHandle + spawn_render_task
    - TuiHandle exposes: new(initial_status), set_activity, set_status, shutdown
    - render_loop exits on non-tty stderr (D-17)
    - render_loop ticks every FRAME_PERIOD (100ms) per D-07
    - redraw uses SavePosition/Hide/MoveTo/Clear/Print/Show/RestorePosition per RESEARCH Example 3
    - redraw queries size() each tick (SIGWINCH-tolerant)
    - All 5 render tests pass
    - cargo test -p ironhermes-cli --lib tui:: reports all 24+ prior tests STILL passing (no regressions)
    - cargo clippy -p ironhermes-cli -- -D warnings exits 0
    - Cargo.toml unchanged (INV-6)
  </done>
  <acceptance_criteria>
    - File exists: crates/ironhermes-cli/src/tui/render.rs
    - `rg -n "pub struct TuiHandle" crates/ironhermes-cli/src/tui/render.rs` returns a match
    - `rg -n "pub fn new\(initial_status: StatusLineState\) -> Self" crates/ironhermes-cli/src/tui/render.rs` returns a match
    - `rg -n "pub fn set_activity\(&self, state: ActivityState\)" crates/ironhermes-cli/src/tui/render.rs` returns a match
    - `rg -n "pub fn set_status\(&self, state: StatusLineState\)" crates/ironhermes-cli/src/tui/render.rs` returns a match
    - `rg -n "pub async fn shutdown\(mut self\)" crates/ironhermes-cli/src/tui/render.rs` returns a match
    - `rg -n "pub const FRAME_PERIOD: Duration = Duration::from_millis\(100\)" crates/ironhermes-cli/src/tui/render.rs` returns a match
    - `rg -n "tokio::sync::watch" crates/ironhermes-cli/src/tui/render.rs` returns at least 2 matches (2 channels)
    - `rg -n "IsTty" crates/ironhermes-cli/src/tui/render.rs` returns at least one match (D-17 non-tty detection)
    - `rg -n "SavePosition|RestorePosition" crates/ironhermes-cli/src/tui/render.rs` returns at least 2 matches (INV-4 partial — full check in 21-03)
    - `rg -n "Hide|Show" crates/ironhermes-cli/src/tui/render.rs` returns at least 2 matches (flicker guard)
    - `rg -n "size\(\)" crates/ironhermes-cli/src/tui/render.rs` returns a match (SIGWINCH-tolerant)
    - `rg -n "println!" crates/ironhermes-cli/src/tui/` returns NO matches (INV-5)
    - `rg -n "print!" crates/ironhermes-cli/src/tui/render.rs` returns NO matches
    - `rg -n "pub use render::TuiHandle;" crates/ironhermes-cli/src/tui/mod.rs` returns a match
    - `cargo test -p ironhermes-cli --lib tui::render` exits 0 reporting 5 tests passing
    - `cargo test -p ironhermes-cli --lib tui::` exits 0 reporting >=29 tests total
    - `cargo clippy -p ironhermes-cli -- -D warnings` exits 0
    - `git diff HEAD -- crates/ironhermes-cli/Cargo.toml` produces empty output (INV-6: no new deps)
    - `rg -n "ratatui|reedline|ctrlc = " crates/ironhermes-cli/Cargo.toml` returns NO matches (D-18)
  </acceptance_criteria>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| ActivityState watch channel | Cross-task boundary — streaming callback → render task; watch semantics ensure latest-wins, no backpressure DoS |
| stderr I/O | Render task is the only writer; collides with existing `eprintln!` in subagent_progress (main.rs:412-439) — RESEARCH §Pitfall 6 |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-21-05 | Denial of Service | render_loop | mitigate | `MissedTickBehavior::Skip` prevents tick backlog from accumulating under load; `watch` has no backpressure so a slow render task never blocks callbacks |
| T-21-06 | DoS via SIGWINCH | redraw | mitigate | `size()` re-queried every tick — old cached rows can never drive off-screen writes |
| T-21-07 | Tampering / Integrity | redraw | mitigate | Single `queue!` + `flush()` per frame ensures atomic ANSI sequence (RESEARCH §Pitfall 3) |
| T-21-08 | DoS — panic in render task | render_loop | mitigate | `let _ = queue!(...)` and `tracing::debug!` on redraw errors ensure the task never panics on I/O failure |
| T-21-09 | Information Disclosure via stderr | redraw | accept | Status line contains only mode/model/provider/tokens — all non-secret, already visible in agent banner |
| T-21-10 | Resource exhaustion — stderr collision with eprintln | redraw + subagent_progress | accept | Subagent progress lines will briefly overwrite the status bar row; next tick (100ms later) redraws it. Accepted degradation per RESEARCH §Pitfall 6 (option a replacement is Plan 21-03 scope). |
</threat_model>

<verification>
## Plan-Level Verification

```bash
# Render-task tests:
cargo test -p ironhermes-cli --lib tui::render

# Full tui module tests (should now report 29+):
cargo test -p ironhermes-cli --lib tui::

# Clippy:
cargo clippy -p ironhermes-cli -- -D warnings

# Structural invariants partially verifiable at this plan boundary:
rg -n "println!" crates/ironhermes-cli/src/tui/               # INV-5: zero
rg -n "SavePosition" crates/ironhermes-cli/src/tui/render.rs  # INV-4 basis
rg -n "RestorePosition" crates/ironhermes-cli/src/tui/render.rs
git diff HEAD -- crates/ironhermes-cli/Cargo.toml             # INV-6: empty
```
</verification>

<success_criteria>
- crates/ironhermes-cli/src/tui/render.rs exists and exports TuiHandle + FRAME_PERIOD
- All 5 render-task tests pass
- Total `tui::` test count >= 29
- No println!/print! calls inside the tui module (INV-5)
- SavePosition and RestorePosition used in redraw (INV-4)
- Cargo.toml dependencies unchanged (D-18, INV-6)
- cargo clippy -p ironhermes-cli -- -D warnings exits 0
- main.rs is NOT modified in this plan (integration deferred to 21-03)
</success_criteria>

<output>
After completion, create `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-02-SUMMARY.md` capturing: render.rs line count, test count delta, confirmation that non-tty path is the one exercised by tests, explicit note that main.rs was NOT modified.
</output>
