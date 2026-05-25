---
phase: 21
slug: commandline-ui-update-polish-cli-ux-including-graceful-doubl
status: planned
nyquist_compliant: true
wave_0_complete: false
created: 2026-04-16
updated: 2026-04-16
---

# Phase 21 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Updated 2026-04-16 by planner with actual task IDs from plans 21-01, 21-02, 21-03.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml (workspace root) |
| **Quick run command** | `cargo test -p ironhermes-cli --lib tui::` |
| **Full suite command** | `cargo test -p ironhermes-cli` |
| **Invariants suite** | `cargo test -p ironhermes-cli --test run_chat_invariants` |
| **Estimated runtime** | ~30 seconds (small crate, few integration tests) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-cli --lib tui::`
- **After every plan wave:** Run `cargo test -p ironhermes-cli` + `cargo clippy -p ironhermes-cli -- -D warnings`
- **Before `/gsd-verify-work`:** Full workspace suite must be green (`cargo test --workspace`), static-grep invariant tests must pass, manual QA checklist signed off by user
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Decision | Test Type | Automated Command | File Exists | Status |
|---------|------|------|----------|-----------|-------------------|-------------|--------|
| 21-01-T1 | 21-01 | 1 | D-04, D-06, D-07 | unit | `cargo test -p ironhermes-cli --lib tui::pills tui::knight_rider tui::activity` | W0 creates | ⬜ pending |
| 21-01-T2 | 21-01 | 1 | D-10..D-14, D-03, D-05 | unit | `cargo test -p ironhermes-cli --lib tui::double_ctrl_c tui::status_line` | W0 creates | ⬜ pending |
| 21-02-T1 | 21-02 | 2 | D-08, D-09, D-15, D-16, D-17 | unit | `cargo test -p ironhermes-cli --lib tui::render` | W0 creates | ⬜ pending |
| 21-03-T1 | 21-03 | 3 | D-03, D-05, D-08, D-09 | build + existing suite | `cargo build -p ironhermes-cli && cargo test -p ironhermes-cli --lib` | existing | ⬜ pending |
| 21-03-T2 | 21-03 | 3 | D-10, D-11, D-12, D-13, D-14 | build + existing suite | `cargo build -p ironhermes-cli && cargo test -p ironhermes-cli --lib && cargo clippy -p ironhermes-cli -- -D warnings` | existing | ⬜ pending |
| 21-03-T3 | 21-03 | 3 | INV-1..INV-6 | integration (static-grep) | `cargo test -p ironhermes-cli --test run_chat_invariants` | W0 creates tests/run_chat_invariants.rs | ⬜ pending |
| 21-03-T4 | 21-03 | 3 | D-22 | manual | `cargo run -p ironhermes-cli -- chat` + 9-scenario walkthrough | — | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

Populated from RESEARCH.md §Wave 0 Gaps. Each file below is created in one of Plans 21-01 / 21-02 / 21-03.

- [ ] `crates/ironhermes-cli/src/tui/mod.rs` — TUI module root (Plan 21-01 Task 1)
- [ ] `crates/ironhermes-cli/src/tui/activity.rs` — ActivityState enum for watch channel (Plan 21-01 Task 1)
- [ ] `crates/ironhermes-cli/src/tui/pills.rs` — Pill color rotation (Plan 21-01 Task 1)
- [ ] `crates/ironhermes-cli/src/tui/knight_rider.rs` — Scanner frame generator (Plan 21-01 Task 1)
- [ ] `crates/ironhermes-cli/src/tui/double_ctrl_c.rs` — Double-ctrl-c state machine (Plan 21-01 Task 2)
- [ ] `crates/ironhermes-cli/src/tui/status_line.rs` — Status line pure renderer (Plan 21-01 Task 2)
- [ ] `crates/ironhermes-cli/src/tui/render.rs` — TuiHandle + render task (Plan 21-02 Task 1)
- [ ] `crates/ironhermes-cli/tests/run_chat_invariants.rs` — Static-grep invariants INV-1..INV-6 (Plan 21-03 Task 3)

---

## Manual-Only Verifications (D-22)

| # | Behavior | Decision | Why Manual | Test Instructions |
|---|----------|----------|------------|-------------------|
| 1 | Status line pills render with alternating colors | D-03, D-04, D-05 | Visual styling requires human eyes | `cargo run -p ironhermes-cli -- chat`; observe bottom bar (cyan/magenta/green/yellow/dimmed) |
| 2 | Knight rider scanner animates smoothly | D-06, D-07, D-09 | Animation frame rate / smoothness is subjective | Issue a prompt; watch bottom-left during tool calls |
| 3 | Scanner hides when idle | D-08 | Visual state transition | Observe bottom-left after turn completes — row should clear |
| 4 | First ctrl-c cancels mid-stream cleanly | D-11 | Requires real signal + real provider call | Send a long prompt, ctrl-c during streaming → `^C — turn cancelled` + return to prompt |
| 5 | Second ctrl-c exits within 1.5s | D-12 | Requires real signals + timing | Press ctrl-c twice within 1s mid-stream → `Goodbye!` + exit 0 + session marked "interrupted" |
| 6 | Ctrl-c at prompt does NOT exit | D-14 | Requires rustyline context | At empty prompt, ctrl-c → `^C — type /quit to exit` + loop |
| 7 | 3rd ctrl-c emergency exit within 3s | RESEARCH §Pitfall 7 fix | Requires real signal chain | Three rapid ctrl-c presses should exit by 2nd; if shutdown hangs, 3rd triggers `std::process::exit(130)` |
| 8 | Terminal resize doesn't break rendering | Claude's Discretion | SIGWINCH requires live terminal | Start chat, resize window, observe redraw — 1-frame glitch acceptable |
| 9 | Non-tty pipe doesn't render the bar | RESEARCH Open Q5 | Requires piped stderr | `cargo run -p ironhermes-cli -- chat --execute "hi" \| cat` → clean output, no ANSI garbage |

---

## Static-Grep Invariants (Locked via `tests/run_chat_invariants.rs`)

| INV | Grep Target | Plan | Rationale |
|-----|-------------|------|-----------|
| INV-1 | `run_chat` body contains `tokio::signal::ctrl_c` + `tokio::select!` | 21-03 | D-10 — ctrl-c handler around agent future only |
| INV-2 | `run_chat` body contains `child_token()` + `chat_cancel_parent` | 21-03 | D-13 + RESEARCH §Pitfall 2 — fresh token per turn |
| INV-3 | `run_single` body does NOT contain `tokio::signal::ctrl_c` or `DoubleCtrlCState` | 21-03 | D-10 — run_single stays single-shot |
| INV-4 | `tui/render.rs` pairs `SavePosition` and `RestorePosition` | 21-02 | Ensures cursor restoration after every frame |
| INV-5 | Zero `println!`/`print!` in `crates/ironhermes-cli/src/tui/` (outside `#[cfg(test)]`) | 21-01, 21-02 | Render task owns stderr (RESEARCH §Pitfall 6) |
| INV-6 | No new deps (`ratatui`/`reedline`/`ctrlc`/`signal-hook`) in Cargo.toml | 21-01, 21-02, 21-03 | D-18 |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references from RESEARCH.md (7 new files)
- [x] No watch-mode flags in test commands
- [x] Feedback latency < 30s
- [x] Manual-only verifications have explicit human-actionable instructions (9 scenarios in Plan 21-03 Task 4)
- [x] `nyquist_compliant: true` set in frontmatter after planner review

**Approval:** planner (2026-04-16) — ready for `/gsd-execute-phase 21`
