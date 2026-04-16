---
phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - crates/ironhermes-cli/src/tui/mod.rs
  - crates/ironhermes-cli/src/tui/status_line.rs
  - crates/ironhermes-cli/src/tui/knight_rider.rs
  - crates/ironhermes-cli/src/tui/double_ctrl_c.rs
  - crates/ironhermes-cli/src/tui/pills.rs
  - crates/ironhermes-cli/src/tui/activity.rs
  - crates/ironhermes-cli/src/main.rs
autonomous: true
requirements: []
decisions_addressed:
  - D-01
  - D-02
  - D-04
  - D-06
  - D-07
  - D-15
  - D-18
  - D-19
  - D-20
  - D-21

must_haves:
  truths:
    - "tui module compiles as part of ironhermes-cli crate"
    - "rotate_pill_colors produces alternating cyan/magenta/green/yellow/dimmed per D-04"
    - "knight_rider::frame(tick, width) produces fixed-width triangle-wave string per D-06/D-07"
    - "DoubleCtrlCState::on_ctrl_c(now, in_flight) returns the correct CtrlCDecision for all 4 cases (prompt/first-in-flight/second-within-window/second-after-window)"
    - "ActivityState enum defined in tui/activity.rs with Idle / Thinking / Streaming / ToolCall variants"
    - "No new crates added to Cargo.toml"
  artifacts:
    - path: "crates/ironhermes-cli/src/tui/mod.rs"
      provides: "tui module root declaring submodules"
      contains: "pub mod status_line;"
    - path: "crates/ironhermes-cli/src/tui/knight_rider.rs"
      provides: "Pure knight-rider frame generator + tests"
      exports: ["frame", "TRACK_WIDTH"]
    - path: "crates/ironhermes-cli/src/tui/pills.rs"
      provides: "Pure pill color rotation + tests"
      exports: ["rotate_pill_colors"]
    - path: "crates/ironhermes-cli/src/tui/double_ctrl_c.rs"
      provides: "Pure double-ctrl-c state machine + tests"
      exports: ["DoubleCtrlCState", "CtrlCDecision"]
    - path: "crates/ironhermes-cli/src/tui/status_line.rs"
      provides: "Status line pure render function + tests"
      exports: ["StatusLineState", "render_status_line"]
    - path: "crates/ironhermes-cli/src/tui/activity.rs"
      provides: "ActivityState enum for watch channel"
      exports: ["ActivityState"]
  key_links:
    - from: "crates/ironhermes-cli/src/main.rs"
      to: "crates/ironhermes-cli/src/tui/mod.rs"
      via: "mod tui; declaration"
      pattern: "^mod tui;"
---

<objective>
Scaffold the `tui` module under `crates/ironhermes-cli/src/tui/` and implement all pure-function cores with full unit test coverage. This plan produces zero runtime behavior change — main.rs does not yet spawn the render task or wire ctrl-c — but it lays the tested foundation for Plan 21-02 (render task) and Plan 21-03 (integration).

Purpose: Establish tested, deterministic building blocks (pill rotation, knight-rider frame generator, double-ctrl-c state machine, status-line pure renderer) before touching any I/O code. Per RESEARCH.md Pattern 1 + Architecture Patterns, keeping cores pure-function-heavy scales test coverage linearly.

Output: Six new files under `crates/ironhermes-cli/src/tui/`, all compiling green, all test modules passing. `mod tui;` declared in main.rs. No wiring into `run_chat` — that comes in 21-03.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md
@.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-VALIDATION.md
@crates/ironhermes-cli/src/main.rs
@crates/ironhermes-cli/Cargo.toml

<interfaces>
<!-- Already-present dependencies executor uses without adding to Cargo.toml -->

From crates/ironhermes-cli/Cargo.toml (verified 2026-04-16):
```toml
crossterm = { workspace = true }     # 0.28 — for status_line.rs stderr I/O (not used in 21-01; used in 21-02)
colored = { workspace = true }       # 3 — used here for pill rotation + knight rider glyphs
tokio = { workspace = true }         # workspace — for tokio::sync::watch in activity.rs (21-02 uses it)
tokio-util = { workspace = true }    # 0.7 rt — CancellationToken (used in 21-03)
```

From RESEARCH.md Example 4 (knight_rider_frame) — copy verbatim into knight_rider.rs:
```rust
use colored::Colorize;
const TRACK_WIDTH: usize = 10;
pub fn frame(tick: u64) -> String {
    let period = (TRACK_WIDTH as u64 - 1) * 2;
    let phase = tick % period;
    let lit = if phase < TRACK_WIDTH as u64 {
        phase as usize
    } else {
        (period - phase) as usize
    };
    (0..TRACK_WIDTH)
        .map(|i| {
            let distance = (i as i32 - lit as i32).unsigned_abs() as usize;
            match distance {
                0 => "█".bright_cyan().to_string(),
                1 => "▓".cyan().to_string(),
                2 => "▒".cyan().dimmed().to_string(),
                _ => "░".dimmed().to_string(),
            }
        })
        .collect::<String>()
}
```

From RESEARCH.md Example 5 (rotate_pill_colors) — copy verbatim into pills.rs:
```rust
use colored::{ColoredString, Colorize};
pub fn rotate_pill_colors(pills: &[String], hint: Option<&str>) -> Vec<ColoredString> {
    let palette: [fn(&str) -> ColoredString; 5] = [
        |s| s.cyan(),
        |s| s.magenta(),
        |s| s.green(),
        |s| s.yellow(),
        |s| s.dimmed(),
    ];
    let mut out: Vec<ColoredString> = pills
        .iter()
        .enumerate()
        .map(|(i, p)| palette[i % palette.len()](p.as_str()))
        .collect();
    if let Some(h) = hint {
        out.push(h.dimmed());
    }
    out
}
```

From RESEARCH.md Example 6 (DoubleCtrlCState) — copy verbatim into double_ctrl_c.rs:
```rust
use std::time::{Duration, Instant};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CtrlCDecision {
    CancelTurn,
    ExitCleanly,
    ShowPromptHint,
}

pub struct DoubleCtrlCState {
    window: Duration,
    last_cancel_at: Option<Instant>,
}

impl DoubleCtrlCState {
    pub fn new() -> Self {
        Self { window: Duration::from_millis(1500), last_cancel_at: None }
    }
    pub fn on_ctrl_c(&mut self, now: Instant, in_flight: bool) -> CtrlCDecision {
        if !in_flight {
            return CtrlCDecision::ShowPromptHint;
        }
        let within_window = self.last_cancel_at
            .map(|t| now.duration_since(t) < self.window)
            .unwrap_or(false);
        if within_window {
            CtrlCDecision::ExitCleanly
        } else {
            self.last_cancel_at = Some(now);
            CtrlCDecision::CancelTurn
        }
    }
    pub fn reset(&mut self) { self.last_cancel_at = None; }
}
```
</interfaces>
</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Scaffold tui module tree + declare in main.rs + implement pills.rs and knight_rider.rs pure cores with tests</name>
  <files>
    crates/ironhermes-cli/src/tui/mod.rs,
    crates/ironhermes-cli/src/tui/pills.rs,
    crates/ironhermes-cli/src/tui/knight_rider.rs,
    crates/ironhermes-cli/src/tui/activity.rs,
    crates/ironhermes-cli/src/main.rs
  </files>
  <read_first>
    - crates/ironhermes-cli/src/main.rs (see current top-of-file module layout, lines 1-20)
    - crates/ironhermes-cli/Cargo.toml (confirm deps: crossterm, colored, tokio, tokio-util already present)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md (D-01, D-02, D-04, D-06, D-07, D-15, D-18, D-19, D-20)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md (Architecture Patterns §Recommended Module Layout; Code Examples 4, 5)
  </read_first>
  <behavior>
    - Test (pills): rotate_pill_colors(&["a","b","c","d","e","f"].map(String::from), None) returns 6 ColoredString items; colors cycle cyan,magenta,green,yellow,dimmed,cyan (pill[5] wraps to cyan via i%5)
    - Test (pills): rotate_pill_colors(pills, Some("hint")) appends the hint as dimmed and returns N+1 items
    - Test (pills): an empty slice + None hint returns an empty Vec
    - Test (knight_rider): frame(0) contains TRACK_WIDTH (=10) glyphs from the set {█, ▓, ▒, ░} when ANSI is stripped
    - Test (knight_rider): frame widths across ticks 0..30 are constant (10 cells each)
    - Test (knight_rider): sampling tick values 0..18 (one full period) yields lit-cell positions that include both 0 and 9 (proving full-width sweep)
    - Test (activity): ActivityState::Idle, Thinking, Streaming, ToolCall{name:String} round-trip through Clone + PartialEq + Debug
  </behavior>
  <action>
Step 1 — Declare module in main.rs:

At the top of `crates/ironhermes-cli/src/main.rs`, add `mod tui;` alongside existing `mod cron; mod batch; mod memory_setup;` (line ~15). Also expose the same module through `lib.rs` if existing patterns demand — but first inspect `crates/ironhermes-cli/src/lib.rs` and mirror whatever pattern `memory_setup` uses (if `memory_setup` is only `mod` in main.rs, do the same; if it is also re-exported via `lib.rs`, do both).

Step 2 — Create `crates/ironhermes-cli/src/tui/mod.rs` as the module root:

```rust
//! Phase 21: CLI TUI polish — status line, knight-rider scanner, double-ctrl-c state machine.
//!
//! This module is intentionally split into pure-function submodules (`pills`, `knight_rider`,
//! `double_ctrl_c`, `status_line`) with I/O isolated to the (future) render task in `mod.rs`.
//! Per D-15/D-18: crossterm + colored primitives only — no new dependencies.

pub mod activity;
pub mod double_ctrl_c;
pub mod knight_rider;
pub mod pills;
pub mod status_line;

// Re-export the public API consumed by main.rs in Plan 21-03.
pub use activity::ActivityState;
pub use double_ctrl_c::{CtrlCDecision, DoubleCtrlCState};
```

Step 3 — Create `crates/ironhermes-cli/src/tui/activity.rs`:

```rust
//! Shared activity state — published by agent callbacks, consumed by the render task.
//!
//! Per RESEARCH.md Pattern 3: use `tokio::sync::watch::channel::<ActivityState>(Idle)`
//! in Plan 21-02 so the render task always reads latest-wins without a mutex.

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ActivityState {
    #[default]
    Idle,
    Thinking,
    Streaming,
    ToolCall { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_default() {
        assert_eq!(ActivityState::default(), ActivityState::Idle);
    }

    #[test]
    fn tool_call_carries_name() {
        let s = ActivityState::ToolCall { name: "bash".to_string() };
        let cloned = s.clone();
        assert_eq!(s, cloned);
    }

    #[test]
    fn variants_are_distinct() {
        assert_ne!(ActivityState::Idle, ActivityState::Streaming);
        assert_ne!(ActivityState::Thinking, ActivityState::Streaming);
    }
}
```

Step 4 — Create `crates/ironhermes-cli/src/tui/pills.rs` copying RESEARCH.md Example 5 verbatim:

```rust
//! Pill color rotation per D-04: cyan, magenta, green, yellow, dimmed.
//! The hint (if provided) is ALWAYS dimmed regardless of rotation index.

use colored::{ColoredString, Colorize};

/// Rotate pill colors per D-04. Returns a `Vec<ColoredString>` — one entry per
/// input pill, plus one extra dimmed entry if `hint` is `Some`.
pub fn rotate_pill_colors(pills: &[String], hint: Option<&str>) -> Vec<ColoredString> {
    let palette: [fn(&str) -> ColoredString; 5] = [
        |s| s.cyan(),
        |s| s.magenta(),
        |s| s.green(),
        |s| s.yellow(),
        |s| s.dimmed(),
    ];
    let mut out: Vec<ColoredString> = pills
        .iter()
        .enumerate()
        .map(|(i, p)| palette[i % palette.len()](p.as_str()))
        .collect();
    if let Some(h) = hint {
        out.push(h.dimmed()); // hint always dimmed (D-04)
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pills(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("p{}", i)).collect()
    }

    #[test]
    fn empty_input_empty_output() {
        let out = rotate_pill_colors(&[], None);
        assert!(out.is_empty());
    }

    #[test]
    fn five_pills_five_outputs() {
        let out = rotate_pill_colors(&pills(5), None);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn six_pills_wraps_to_cyan_at_index_5() {
        // palette has 5 entries; pill[5] must wrap to palette[0]=cyan.
        let out = rotate_pill_colors(&pills(6), None);
        assert_eq!(out.len(), 6);
        // Compare stripped escape codes to the cyan rendering of "p5".
        let expected_cyan = "p5".to_string().cyan().to_string();
        assert_eq!(out[5].to_string(), expected_cyan);
        let expected_cyan_0 = "p0".to_string().cyan().to_string();
        assert_eq!(out[0].to_string(), expected_cyan_0);
    }

    #[test]
    fn hint_is_appended_and_dimmed() {
        let out = rotate_pill_colors(&pills(3), Some("ctrl+p commands"));
        assert_eq!(out.len(), 4);
        let expected = "ctrl+p commands".dimmed().to_string();
        assert_eq!(out[3].to_string(), expected);
    }

    #[test]
    fn palette_order_is_cyan_magenta_green_yellow_dimmed() {
        let out = rotate_pill_colors(&pills(5), None);
        assert_eq!(out[0].to_string(), "p0".cyan().to_string());
        assert_eq!(out[1].to_string(), "p1".magenta().to_string());
        assert_eq!(out[2].to_string(), "p2".green().to_string());
        assert_eq!(out[3].to_string(), "p3".yellow().to_string());
        assert_eq!(out[4].to_string(), "p4".dimmed().to_string());
    }
}
```

Step 5 — Create `crates/ironhermes-cli/src/tui/knight_rider.rs` copying RESEARCH.md Example 4 (with the function renamed `frame` per CONTEXT hint "knight_rider::frame(tick, width)"; however RESEARCH.md fixes the width as TRACK_WIDTH=10 per D-07, so the public API takes only `tick: u64` and width is a pub const):

```rust
//! Knight Rider scanner frame generator (pure function, no I/O).
//!
//! Per D-07: 10-cell horizontal track, triangle-wave sweep, lit cell bright cyan,
//! trailing cells fade via dimmed. Frame rate is driven externally (21-02's render
//! task ticks every 100ms per D-07).

use colored::Colorize;

pub const TRACK_WIDTH: usize = 10;

/// Given a monotonic tick, produce the 10-cell Knight Rider frame.
/// Triangle wave: lit cell sweeps 0 → 9 → 0 → 9 over (TRACK_WIDTH-1)*2 = 18 ticks.
pub fn frame(tick: u64) -> String {
    let period = (TRACK_WIDTH as u64 - 1) * 2;
    let phase = tick % period;
    let lit = if phase < TRACK_WIDTH as u64 {
        phase as usize
    } else {
        (period - phase) as usize
    };

    (0..TRACK_WIDTH)
        .map(|i| {
            let distance = (i as i32 - lit as i32).unsigned_abs() as usize;
            match distance {
                0 => "█".bright_cyan().to_string(),
                1 => "▓".cyan().to_string(),
                2 => "▒".cyan().dimmed().to_string(),
                _ => "░".dimmed().to_string(),
            }
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count how many glyphs from the knight-rider set appear, ignoring ANSI escapes.
    fn glyph_count(s: &str) -> usize {
        s.chars().filter(|c| ['█', '▓', '▒', '░'].contains(c)).count()
    }

    #[test]
    fn frame_has_track_width_glyphs() {
        for tick in 0..30 {
            assert_eq!(
                glyph_count(&frame(tick)),
                TRACK_WIDTH,
                "tick {} must produce {} glyphs",
                tick,
                TRACK_WIDTH
            );
        }
    }

    #[test]
    fn triangle_wave_reaches_both_endpoints() {
        // Over one full period, the lit-cell index must hit both 0 and TRACK_WIDTH-1.
        let mut positions = std::collections::HashSet::new();
        for tick in 0..18u64 {
            let period = 18u64;
            let phase = tick % period;
            let lit = if phase < TRACK_WIDTH as u64 {
                phase as usize
            } else {
                (period - phase) as usize
            };
            positions.insert(lit);
        }
        assert!(positions.contains(&0), "lit never reaches 0");
        assert!(positions.contains(&(TRACK_WIDTH - 1)), "lit never reaches {}", TRACK_WIDTH - 1);
    }

    #[test]
    fn period_is_stable_frames_18_and_0_match() {
        // Triangle wave period is 18; frame(0) and frame(18) should be identical.
        assert_eq!(frame(0), frame(18));
    }
}
```

Step 6 — Compile and run unit tests.

No wiring into `run_chat`, no spawning any task, no adding any dep. Just scaffold + pure cores.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-cli --lib tui::pills tui::knight_rider tui::activity</automated>
  </verify>
  <done>
    - crates/ironhermes-cli/src/tui/mod.rs exists and declares pub mod {activity, double_ctrl_c, knight_rider, pills, status_line}; (status_line + double_ctrl_c will be added in Task 2 — the mod declarations are added there, NOT here)
    - crates/ironhermes-cli/src/tui/pills.rs exports rotate_pill_colors and 5 tests pass
    - crates/ironhermes-cli/src/tui/knight_rider.rs exports frame + TRACK_WIDTH=10 and 3 tests pass
    - crates/ironhermes-cli/src/tui/activity.rs exports ActivityState enum (Idle|Thinking|Streaming|ToolCall{name}) and 3 tests pass
    - main.rs has `mod tui;` at top alongside mod cron/batch/memory_setup
    - cargo build -p ironhermes-cli exits 0
    - Cargo.toml is unchanged (grep shows no diff vs HEAD for [dependencies] section)
  </done>
  <acceptance_criteria>
    - File exists: crates/ironhermes-cli/src/tui/mod.rs
    - File exists: crates/ironhermes-cli/src/tui/pills.rs
    - File exists: crates/ironhermes-cli/src/tui/knight_rider.rs
    - File exists: crates/ironhermes-cli/src/tui/activity.rs
    - `rg -n "^mod tui;" crates/ironhermes-cli/src/main.rs` returns at least one match
    - `rg -n "pub const TRACK_WIDTH: usize = 10;" crates/ironhermes-cli/src/tui/knight_rider.rs` returns a match
    - `rg -n "pub fn frame\(tick: u64\) -> String" crates/ironhermes-cli/src/tui/knight_rider.rs` returns a match
    - `rg -n "pub fn rotate_pill_colors" crates/ironhermes-cli/src/tui/pills.rs` returns a match
    - `rg -n "pub enum ActivityState" crates/ironhermes-cli/src/tui/activity.rs` returns a match
    - `cargo test -p ironhermes-cli --lib tui::pills` exits 0 and reports 5 tests passing
    - `cargo test -p ironhermes-cli --lib tui::knight_rider` exits 0 and reports 3 tests passing
    - `cargo test -p ironhermes-cli --lib tui::activity` exits 0 and reports 3 tests passing
    - `git diff HEAD -- crates/ironhermes-cli/Cargo.toml` produces empty output (D-18: no new deps)
  </acceptance_criteria>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Implement double_ctrl_c.rs state machine + status_line.rs pure renderer with tests</name>
  <files>
    crates/ironhermes-cli/src/tui/double_ctrl_c.rs,
    crates/ironhermes-cli/src/tui/status_line.rs,
    crates/ironhermes-cli/src/tui/mod.rs
  </files>
  <read_first>
    - crates/ironhermes-cli/src/tui/mod.rs (from Task 1 — confirm pub mod lines to ensure both submodules are declared)
    - crates/ironhermes-cli/src/tui/pills.rs (use its rotate_pill_colors from status_line.rs)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md (D-03, D-04, D-05, D-11, D-12, D-13, D-14, D-21)
    - .planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-RESEARCH.md (Code Example 6; §Pitfall 2 CancellationToken::cancel() permanent)
  </read_first>
  <behavior>
    - Test (double_ctrl_c): ctrl-c when NOT in-flight → ShowPromptHint (D-14)
    - Test (double_ctrl_c): first ctrl-c while in-flight → CancelTurn (D-11)
    - Test (double_ctrl_c): second ctrl-c within 1500ms of first while in-flight → ExitCleanly (D-12)
    - Test (double_ctrl_c): second ctrl-c AFTER 1500ms while in-flight → CancelTurn (D-13)
    - Test (double_ctrl_c): reset() clears last_cancel_at so next ctrl-c is treated as first
    - Test (double_ctrl_c): 3 rapid ctrl-c events within window → CancelTurn, ExitCleanly, ExitCleanly (window persists after ExitCleanly because we do not reset on exit — caller does)
    - Test (status_line): render_status_line with mode="Agent", model="claude-sonnet-4", provider="anthropic", tokens=107700, limit=200000, hint="ctrl+p commands" produces a string containing all pills with " · " (middle-dot space) separator and the hint at the end
    - Test (status_line): percentage formatting — 107700/200000 renders as "54%" (integer percent)
    - Test (status_line): token formatting — 107700 renders as "107.7K"; 1500 renders as "1.5K"; 500 renders as "500"; 1_200_000 renders as "1.2M"
    - Test (status_line): renders consistent output when hint is None (no trailing separator / no empty pill)
  </behavior>
  <action>
Step 1 — Update `crates/ironhermes-cli/src/tui/mod.rs` to ensure the two new submodules are declared (they should already be from Task 1's mod.rs template; verify `pub mod double_ctrl_c;` and `pub mod status_line;` are present).

Step 2 — Create `crates/ironhermes-cli/src/tui/double_ctrl_c.rs` copying RESEARCH.md Example 6 verbatim, adding one extra test for `reset()` and one for the 3-rapid-press scenario:

```rust
//! Double-ctrl-c state machine per D-10..D-14.
//!
//! PURE function state machine — no tokio, no real SIGINT needed for tests (D-21).
//! The 1.5s window is a compile-time constant (D-12 / D-14 §Configuration).

use std::time::{Duration, Instant};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CtrlCDecision {
    /// First ctrl-c while in-flight: cancel in-flight work, return to prompt. (D-11)
    CancelTurn,
    /// Second ctrl-c within window while in-flight: persist + exit 0. (D-12)
    ExitCleanly,
    /// Ctrl-c at prompt (not in-flight): print "^C — type /quit to exit" and loop. (D-14)
    ShowPromptHint,
}

pub struct DoubleCtrlCState {
    window: Duration,
    last_cancel_at: Option<Instant>,
}

impl Default for DoubleCtrlCState {
    fn default() -> Self { Self::new() }
}

impl DoubleCtrlCState {
    pub fn new() -> Self {
        Self { window: Duration::from_millis(1500), last_cancel_at: None }
    }

    /// Returns the decision for THIS ctrl-c event.
    /// Caller tracks `in_flight` externally (derived from whether the agent future is running).
    pub fn on_ctrl_c(&mut self, now: Instant, in_flight: bool) -> CtrlCDecision {
        if !in_flight {
            return CtrlCDecision::ShowPromptHint;
        }
        let within_window = self
            .last_cancel_at
            .map(|t| now.duration_since(t) < self.window)
            .unwrap_or(false);
        if within_window {
            CtrlCDecision::ExitCleanly
        } else {
            self.last_cancel_at = Some(now);
            CtrlCDecision::CancelTurn
        }
    }

    /// Reset on successful turn completion OR on fresh user input (D-13).
    pub fn reset(&mut self) { self.last_cancel_at = None; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrlc_at_prompt_is_hint() {
        let mut s = DoubleCtrlCState::new();
        assert_eq!(
            s.on_ctrl_c(Instant::now(), false),
            CtrlCDecision::ShowPromptHint
        );
    }

    #[test]
    fn first_ctrlc_in_flight_cancels() {
        let mut s = DoubleCtrlCState::new();
        assert_eq!(
            s.on_ctrl_c(Instant::now(), true),
            CtrlCDecision::CancelTurn
        );
    }

    #[test]
    fn second_ctrlc_within_window_exits() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        s.on_ctrl_c(t0, true);
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::ExitCleanly);
    }

    #[test]
    fn second_ctrlc_after_window_cancels_again() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        s.on_ctrl_c(t0, true);
        let t1 = t0 + Duration::from_millis(1600);
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::CancelTurn);
    }

    #[test]
    fn reset_clears_window() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        s.on_ctrl_c(t0, true); // CancelTurn
        s.reset();
        let t1 = t0 + Duration::from_millis(100);
        // After reset the next press should be a fresh first press → CancelTurn.
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::CancelTurn);
    }

    #[test]
    fn three_rapid_presses_within_window() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        assert_eq!(s.on_ctrl_c(t0, true), CtrlCDecision::CancelTurn);
        let t1 = t0 + Duration::from_millis(200);
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::ExitCleanly);
        let t2 = t0 + Duration::from_millis(400);
        // Third press is still within window (≤1500ms since t0) and caller has not
        // reset — state still returns ExitCleanly. The main.rs emergency-exit
        // escape hatch (Plan 21-03) handles the real 3rd-press footgun via a
        // separate 3s window + std::process::exit(130).
        assert_eq!(s.on_ctrl_c(t2, true), CtrlCDecision::ExitCleanly);
    }
}
```

Step 3 — Create `crates/ironhermes-cli/src/tui/status_line.rs`. This is a PURE renderer — it takes state, returns a String. The I/O (write-to-stderr + cursor positioning) happens in Plan 21-02's render task. Per D-03 the pill order is: `{mode} · {model_short} · {provider} · {tokens}/{limit} ({pct}%) · {hint}`. Per D-04 the hint is dimmed and the dots are dimmed.

```rust
//! Status line state + pure render function.
//!
//! Per D-03: `{mode} · {model_short} · {provider} · {tokens}/{limit} ({pct}%) · {hint}`
//! Per D-04: pills rotate cyan/magenta/green/yellow/dimmed; dots are dimmed; hint is dimmed.
//! Per D-05: state carries live token + limit from the agent's PressureTracker snapshot.
//!
//! This module is PURE — it produces a `String` (ANSI-colored) from a state struct.
//! The render task in `mod.rs` (Plan 21-02) calls this and writes to stderr via crossterm.

use crate::tui::pills::rotate_pill_colors;
use colored::Colorize;

/// Snapshot of the values shown in the status line. Updated each tick by the
/// render task (Plan 21-02) from the live `PressureTracker` / budget counter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusLineState {
    pub mode: String,
    pub model_short: String,
    pub provider: String,
    pub tokens_used: u64,
    pub tokens_limit: u64,
    pub hint: String,
}

impl Default for StatusLineState {
    fn default() -> Self {
        Self {
            mode: "Chat".to_string(),
            model_short: "?".to_string(),
            provider: "?".to_string(),
            tokens_used: 0,
            tokens_limit: 128_000,
            hint: "ctrl+c cancel · /help commands".to_string(),
        }
    }
}

/// Format a token count as "107.7K" / "1.2M" / "500".
pub fn format_token_count(n: u64) -> String {
    if n >= 1_000_000 {
        let m = (n as f64) / 1_000_000.0;
        format!("{:.1}M", m)
    } else if n >= 1_000 {
        let k = (n as f64) / 1_000.0;
        format!("{:.1}K", k)
    } else {
        format!("{}", n)
    }
}

/// Produce the dot-separated, color-rotated status line as a single String
/// ready to be written to stderr by the render task.
pub fn render_status_line(state: &StatusLineState) -> String {
    let pct = if state.tokens_limit == 0 {
        0
    } else {
        ((state.tokens_used as f64 / state.tokens_limit as f64) * 100.0).round() as u64
    };
    let tokens_cell = format!(
        "{}/{} ({}%)",
        format_token_count(state.tokens_used),
        format_token_count(state.tokens_limit),
        pct
    );

    let pills: Vec<String> = vec![
        state.mode.clone(),
        state.model_short.clone(),
        state.provider.clone(),
        tokens_cell,
    ];

    let hint = if state.hint.is_empty() { None } else { Some(state.hint.as_str()) };
    let colored_cells = rotate_pill_colors(&pills, hint);

    // Join cells with a dimmed " · " separator (D-04: dots stay dimmed).
    let dot_sep = format!(" {} ", "·".dimmed());
    colored_cells
        .iter()
        .map(|cs| cs.to_string())
        .collect::<Vec<_>>()
        .join(&dot_sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_token_count_under_1000() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(500), "500");
        assert_eq!(format_token_count(999), "999");
    }

    #[test]
    fn format_token_count_kilo() {
        assert_eq!(format_token_count(1_000), "1.0K");
        assert_eq!(format_token_count(1_500), "1.5K");
        assert_eq!(format_token_count(107_700), "107.7K");
        assert_eq!(format_token_count(999_999), "1000.0K");
    }

    #[test]
    fn format_token_count_mega() {
        assert_eq!(format_token_count(1_000_000), "1.0M");
        assert_eq!(format_token_count(1_200_000), "1.2M");
    }

    #[test]
    fn renders_all_pills_and_hint() {
        let state = StatusLineState {
            mode: "Agent".to_string(),
            model_short: "claude-sonnet-4".to_string(),
            provider: "anthropic".to_string(),
            tokens_used: 107_700,
            tokens_limit: 200_000,
            hint: "ctrl+p commands".to_string(),
        };
        let out = render_status_line(&state);
        // All pill values appear in the output.
        assert!(out.contains("Agent"), "missing Agent: {}", out);
        assert!(out.contains("claude-sonnet-4"));
        assert!(out.contains("anthropic"));
        assert!(out.contains("107.7K"));
        assert!(out.contains("200.0K"));
        assert!(out.contains("54%"));
        assert!(out.contains("ctrl+p commands"));
        // Dot separator present.
        assert!(out.contains("·"));
    }

    #[test]
    fn percentage_rounds_to_integer() {
        let state = StatusLineState {
            tokens_used: 107_700,
            tokens_limit: 200_000,
            ..StatusLineState::default()
        };
        let out = render_status_line(&state);
        assert!(out.contains("54%"), "expected 54%, got: {}", out);
    }

    #[test]
    fn handles_zero_limit_without_panic() {
        let state = StatusLineState {
            tokens_limit: 0,
            tokens_used: 5,
            ..StatusLineState::default()
        };
        let out = render_status_line(&state);
        assert!(out.contains("0%"), "expected 0% for zero limit: {}", out);
    }

    #[test]
    fn empty_hint_omits_trailing_hint_pill() {
        let state = StatusLineState {
            mode: "Chat".to_string(),
            model_short: "m".to_string(),
            provider: "p".to_string(),
            tokens_used: 0,
            tokens_limit: 100,
            hint: String::new(),
        };
        let out = render_status_line(&state);
        // With empty hint, rotate_pill_colors gets None; resulting pill count
        // should be 4 (mode, model, provider, tokens) — so 3 separators.
        let sep_count = out.matches('·').count();
        assert_eq!(sep_count, 3, "expected 3 dots for 4 pills, got: {}", out);
    }
}
```

Step 4 — Compile and run unit tests. No integration with main.rs yet.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-cli --lib tui::double_ctrl_c tui::status_line</automated>
  </verify>
  <done>
    - crates/ironhermes-cli/src/tui/double_ctrl_c.rs exports DoubleCtrlCState + CtrlCDecision and 6 tests pass
    - crates/ironhermes-cli/src/tui/status_line.rs exports StatusLineState + render_status_line + format_token_count and 7 tests pass
    - tui/mod.rs declares both `pub mod double_ctrl_c;` and `pub mod status_line;`
    - cargo build -p ironhermes-cli exits 0 with zero warnings related to the new modules
    - cargo clippy -p ironhermes-cli -- -D warnings exits 0 (INV-5 / INV-6 baseline)
  </done>
  <acceptance_criteria>
    - File exists: crates/ironhermes-cli/src/tui/double_ctrl_c.rs
    - File exists: crates/ironhermes-cli/src/tui/status_line.rs
    - `rg -n "pub fn on_ctrl_c\(&mut self, now: Instant, in_flight: bool\) -> CtrlCDecision" crates/ironhermes-cli/src/tui/double_ctrl_c.rs` returns a match
    - `rg -n "pub enum CtrlCDecision" crates/ironhermes-cli/src/tui/double_ctrl_c.rs` returns a match
    - `rg -n "CancelTurn|ExitCleanly|ShowPromptHint" crates/ironhermes-cli/src/tui/double_ctrl_c.rs` returns at least 3 matches
    - `rg -n "pub fn render_status_line\(state: &StatusLineState\) -> String" crates/ironhermes-cli/src/tui/status_line.rs` returns a match
    - `rg -n "pub fn format_token_count\(n: u64\) -> String" crates/ironhermes-cli/src/tui/status_line.rs` returns a match
    - `rg -n "pub mod double_ctrl_c;" crates/ironhermes-cli/src/tui/mod.rs` returns a match
    - `rg -n "pub mod status_line;" crates/ironhermes-cli/src/tui/mod.rs` returns a match
    - `cargo test -p ironhermes-cli --lib tui::double_ctrl_c` exits 0 reporting 6 tests passing
    - `cargo test -p ironhermes-cli --lib tui::status_line` exits 0 reporting 7 tests passing
    - `cargo test -p ironhermes-cli --lib tui::` exits 0 reporting >=24 tests total (5+3+3+6+7)
    - `cargo clippy -p ironhermes-cli -- -D warnings` exits 0
    - `git diff HEAD -- crates/ironhermes-cli/Cargo.toml` produces empty output (INV-6: no new deps)
  </acceptance_criteria>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| stdin (rustyline) → tui logic | User-typed text converted to display state — but only `hint` is ever influenced by this code; pill values come from config (not user input) |
| config file → status line | `model_short` and `provider` derive from Config — static compile-time trust boundary |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-21-01 | Information Disclosure | pills.rs / status_line.rs | accept | No secrets touch this path; pill values are mode/model/provider/tokens — all non-sensitive. Rationale: no PII, no credential surface. |
| T-21-02 | Tampering | knight_rider.rs | mitigate | Pure function with no I/O; cannot be tampered with at runtime. Deterministic output asserted by frame-width tests. |
| T-21-03 | Denial of Service | double_ctrl_c.rs | mitigate | State machine has no unbounded allocations; Instant arithmetic cannot overflow in practical session durations (>500 years of monotonic time). |
| T-21-04 | Repudiation | status_line.rs | accept | No logging surface; observational-only output. |
</threat_model>

<verification>
## Plan-Level Verification

```bash
# All pure-function tests for this plan:
cargo test -p ironhermes-cli --lib tui::

# Structural invariants (locked in by Plan 21-03 but baseline-checkable here):
rg -n "^mod tui;" crates/ironhermes-cli/src/main.rs        # INV: module declared
rg -n "println!" crates/ironhermes-cli/src/tui/             # INV-5: must be ZERO matches
git diff HEAD -- crates/ironhermes-cli/Cargo.toml          # INV-6: empty diff

# Clippy gate:
cargo clippy -p ironhermes-cli -- -D warnings
```

Expected final count: 24+ pure-function tests green. No new crates in Cargo.toml. Zero `println!` inside `crates/ironhermes-cli/src/tui/`.
</verification>

<success_criteria>
- All 6 submodule files exist under `crates/ironhermes-cli/src/tui/`
- `cargo test -p ironhermes-cli --lib tui::` reports 24+ passing tests
- `cargo clippy -p ironhermes-cli -- -D warnings` exits 0
- Cargo.toml dependencies unchanged (enforces D-18)
- Zero println! inside the tui module (enforces INV-5 early)
- `mod tui;` declared in `crates/ironhermes-cli/src/main.rs`
</success_criteria>

<output>
After completion, create `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-01-SUMMARY.md` capturing: files created, test count, clippy baseline, and explicit confirmation that run_chat was NOT yet modified (runtime behavior unchanged).
</output>
