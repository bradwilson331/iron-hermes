---
phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl
verified: 2026-04-16T22:00:00Z
status: human_needed
score: 10/10
overrides_applied: 0
human_verification:
  - test: "Status line present and colored correctly"
    expected: "Bottom row shows mode/model/provider/tokens/hint with alternating cyan/magenta/green/yellow/dimmed pills"
    why_human: "Visual appearance — color rendering cannot be verified programmatically"
  - test: "Knight rider animates during in-flight turn"
    expected: "10-cell track with bright cyan block sweeping left-right, trailing fade, updating at ~10fps with activity label"
    why_human: "Animation behavior requires real terminal observation"
  - test: "Terminal resize does not corrupt bar"
    expected: "Status bar redraws at new bottom after resize"
    why_human: "Interactive terminal behavior"
---

# Phase 21: Commandline UI Update Verification Report

**Phase Goal:** Polish crates/ironhermes-cli/ REPL UX on existing deps (crossterm/rustyline/colored/tokio -- no new crates per D-18): render a persistent dot-separated pill status line at the bottom (mode/model/provider/tokens/limit/hint, alternating cyan/magenta/green/yellow/dimmed), animate a 10-cell Knight Rider scanner during in-flight turns/tools, and implement graceful double ctrl-c where the first press cancels the in-flight turn (preserving conversation history) and the second press within 1.5s persists the session as "interrupted" and exits cleanly.
**Verified:** 2026-04-16T22:00:00Z
**Status:** human_needed
**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Persistent dot-separated pill status line at bottom with alternating colors | VERIFIED | `render_status_line` at status_line.rs:60 produces colored pill string; `rotate_pill_colors` at pills.rs:8 cycles cyan/magenta/green/yellow/dimmed per D-04; render.rs:233 calls render_status_line every tick; 8 status_line tests + 5 pills tests all pass |
| 2 | 10-cell Knight Rider scanner animates during in-flight turns/tools | VERIFIED | knight_rider.rs:13 `frame(tick)` produces 10-glyph triangle-wave string; TRACK_WIDTH=10 constant at line 11; render.rs:240,245 calls `knight_rider::frame(tick)` when activity != Idle; 3 tests verify frame width consistency and full-sweep coverage |
| 3 | First ctrl-c cancels in-flight turn (preserving conversation history) | VERIFIED | main.rs:594 `tokio::signal::ctrl_c()` in tokio::select!; main.rs:609-616 CtrlCDecision::CancelTurn cancels token + prints "turn cancelled" + continues; main.rs:721 `.with_cancellation_token(cancel_token)` forwards to AgentLoop; child_token pattern at main.rs:428,646 ensures fresh token per turn |
| 4 | Second ctrl-c within 1.5s persists session as "interrupted" and exits cleanly | VERIFIED | double_ctrl_c.rs:22 1500ms window; main.rs:618-626 ExitCleanly path calls cleanup_on_exit, end_session("interrupted"), exit(0); main.rs:657-663 rustyline-Interrupted branch also consults state machine for cross-boundary exit; 7 double_ctrl_c tests cover all cases |
| 5 | No new crates added (D-18) | VERIFIED | `git diff HEAD -- crates/ironhermes-cli/Cargo.toml` produces 0 lines; INV-6 test passes checking for forbidden deps (ratatui, reedline, ctrlc, signal-hook) |
| 6 | Rolled-in todo resolved | VERIFIED | `.planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md` EXISTS with ## Resolution section; pending file MISSING (confirmed deleted) |
| 7 | All pure-function cores tested | VERIFIED | 31 tui unit tests pass: activity(3) + pills(5) + knight_rider(3) + double_ctrl_c(7) + status_line(8) + render(5) |
| 8 | TuiHandle wired into run_chat | VERIFIED | main.rs:407 `TuiHandle::new(initial_status)` spawns handle; main.rs:730 streaming callback publishes ActivityState::Streaming; main.rs:736 tool callback publishes ToolCall; main.rs:763 post-turn reset to Idle; main.rs:685-688 shutdown on clean exit via Arc::try_unwrap |
| 9 | Static-grep invariant tests lock structural guarantees | VERIFIED | tests/run_chat_invariants.rs has 6 tests (inv_1 through inv_6); all 6 pass: INV-1 tokio::select+ctrl_c, INV-2 child_token, INV-3 run_single isolation, INV-4 Save/RestorePosition, INV-5 no stdout prints in tui, INV-6 no forbidden deps |
| 10 | Manual QA passed (9 VALIDATION.md scenarios) | VERIFIED | Per 21-03-SUMMARY.md: all 9 manual QA scenarios PASS (status line, knight rider animation, scanner hides, first ctrl-c cancel, second ctrl-c exit, prompt ctrl-c hint, 3rd ctrl-c emergency, terminal resize, non-tty pipe) |

**Score:** 10/10 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-cli/src/tui/mod.rs` | Module root declaring submodules | VERIFIED | 21 lines; declares 6 pub mod + 4 pub use re-exports |
| `crates/ironhermes-cli/src/tui/activity.rs` | ActivityState enum | VERIFIED | 43 lines; exports ActivityState (Idle/Streaming/ToolCall) + 3 tests |
| `crates/ironhermes-cli/src/tui/pills.rs` | Pill color rotation | VERIFIED | 75 lines; exports rotate_pill_colors + 5 tests |
| `crates/ironhermes-cli/src/tui/knight_rider.rs` | Knight-rider frame generator | VERIFIED | 78 lines; exports frame + TRACK_WIDTH=10 + 3 tests |
| `crates/ironhermes-cli/src/tui/double_ctrl_c.rs` | Double-ctrl-c state machine | VERIFIED | 129 lines; exports DoubleCtrlCState + CtrlCDecision + 7 tests |
| `crates/ironhermes-cli/src/tui/status_line.rs` | Status line renderer | VERIFIED | 182 lines; exports StatusLineState + render_status_line + format_token_count + 8 tests |
| `crates/ironhermes-cli/src/tui/render.rs` | TuiHandle + render task | VERIFIED | 330 lines; exports TuiHandle + FRAME_PERIOD + prepare_prompt + finish_prompt + 5 tests |
| `crates/ironhermes-cli/tests/run_chat_invariants.rs` | Static-grep invariant tests | VERIFIED | 6 invariant tests (INV-1 through INV-6), all passing |
| `crates/ironhermes-cli/src/main.rs` | Integrated TuiHandle + ctrl-c in run_chat | VERIFIED | mod tui at line 20; TuiHandle::new at 407; tokio::select! at 592; DoubleCtrlCState at 431 |
| `.planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md` | Rolled-in todo resolved | VERIFIED | EXISTS with ## Resolution section |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| main.rs | tui/mod.rs | `mod tui;` declaration | WIRED | Line 20 |
| main.rs::run_chat | TuiHandle | `TuiHandle::new(initial_status)` | WIRED | Line 407 |
| main.rs::run_chat | tokio::signal::ctrl_c | `tokio::select!` arm | WIRED | Lines 592-594 |
| main.rs::run_chat | DoubleCtrlCState::on_ctrl_c | state machine drives decisions | WIRED | Lines 609, 658 |
| main.rs::run_agent_turn | AgentLoop | `.with_cancellation_token(cancel_token)` | WIRED | Line 721 |
| render.rs | status_line.rs | `render_status_line()` called every tick | WIRED | Line 233 |
| render.rs | knight_rider.rs | `knight_rider::frame()` called when active | WIRED | Lines 240, 245 |
| render.rs | tokio::sync::watch | Two watch channels (ActivityState + StatusLineState) | WIRED | Line 24 import; channels created in TuiHandle::new |
| main.rs callbacks | ActivityState | streaming publishes Streaming, tool publishes ToolCall | WIRED | Lines 730, 736 |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|--------------|--------|-------------------|--------|
| render.rs | activity_rx | watch channel from main.rs callbacks | Yes -- streaming/tool callbacks publish real ActivityState | FLOWING |
| render.rs | status_rx | watch channel from TuiHandle::new + set_status | Yes -- initial state from config, updated with real token counts post-turn | FLOWING |
| status_line.rs | StatusLineState | Constructed with real client.model(), config.model.provider, token counts | Yes -- real config values, not hardcoded | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Build succeeds | `cargo build -p ironhermes-cli` | Finished dev profile, 2 pre-existing warnings (not in tui/) | PASS |
| 31 tui unit tests pass | `cargo test -p ironhermes-cli --bin ironhermes -- tui::` | 31 passed, 0 failed | PASS |
| 6 invariant tests pass | `cargo test -p ironhermes-cli --test run_chat_invariants` | 6 passed, 0 failed | PASS |
| No forbidden deps | grep for ratatui/reedline/ctrlc/signal-hook in Cargo.toml | 0 matches | PASS |
| No println in tui prod code | grep for println!/print! in tui/ | Only 1 match in doc comment (render.rs:155) | PASS |
| Cargo.toml unchanged | `git diff HEAD -- Cargo.toml` | 0 lines diff | PASS |

### Requirements Coverage

This phase uses D-01..D-22 from 21-CONTEXT.md as requirements (no REQ-IDs mapped in REQUIREMENTS.md). REQUIREMENTS.md maps GW-01..GW-11 to "Phase 21" but those are Gateway Architecture requirements for a different Phase 21 scope -- not applicable to this CLI UX polish phase.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| (none) | -- | -- | -- | Zero TODO/FIXME/PLACEHOLDER/HACK markers in tui module files |
| (none) | -- | -- | -- | Zero empty implementations (return null/{}/ []) |
| (none) | -- | -- | -- | Zero println!/print! in production tui code |

### Human Verification Required

While the SUMMARY claims all 9 manual QA scenarios passed, three items inherently require human observation to confirm:

### 1. Status Line Visual Appearance

**Test:** Run `cargo run -p ironhermes-cli -- chat` and observe bottom row
**Expected:** Pills in alternating cyan/magenta/green/yellow/dimmed with dimmed dot separators and dimmed hint
**Why human:** Color rendering and visual layout cannot be verified programmatically

### 2. Knight Rider Animation

**Test:** Send a prompt that triggers a tool call and observe the animation row
**Expected:** 10-cell track with bright cyan sweep at ~10fps, labeled "Running: {tool}" or "Streaming"
**Why human:** Animation smoothness and timing require real terminal observation

### 3. Terminal Resize Behavior

**Test:** Resize the terminal during an active chat session
**Expected:** Status bar redraws at new bottom position within 1 frame
**Why human:** Interactive terminal behavior cannot be simulated in tests

### Gaps Summary

No code-level gaps found. All 10 observable truths are verified through code inspection, grep evidence, and automated test results. All 37 tests pass (31 unit + 6 invariant). All key links are wired. All artifacts are substantive. No anti-patterns detected.

The 3 human verification items above are inherent to visual terminal behavior and cannot be verified programmatically. The 21-03-SUMMARY.md reports all 9 manual QA scenarios passed, but the verifier cannot independently confirm visual output.

---

_Verified: 2026-04-16T22:00:00Z_
_Verifier: Claude (gsd-verifier)_
