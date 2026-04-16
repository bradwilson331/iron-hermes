---
phase: 21
slug: commandline-ui-update-polish-cli-ux-including-graceful-doubl
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-16
---

# Phase 21 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Planner fills the Per-Task Verification Map once plans are drafted.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml (workspace root) |
| **Quick run command** | `cargo test -p ironhermes-cli --lib` |
| **Full suite command** | `cargo test -p ironhermes-cli` |
| **Estimated runtime** | ~30 seconds (small crate, few integration tests) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-cli --lib`
- **After every plan wave:** Run `cargo test -p ironhermes-cli` + `cargo clippy -p ironhermes-cli -- -D warnings`
- **Before `/gsd-verify-work`:** Full suite must be green, manual smoke test of chat mode must pass
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

*Planner will fill this table when plans are drafted. Each task must map to either an automated cargo test command or an explicit Wave 0 dependency.*

| Task ID | Plan | Wave | Decision | Test Type | Automated Command | File Exists | Status |
|---------|------|------|----------|-----------|-------------------|-------------|--------|
| _TBD_ | _TBD_ | _TBD_ | D-XX | unit | `cargo test -p ironhermes-cli ...` | W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

*Populated from RESEARCH.md Wave 0 Gaps section. Planner confirms and adds as tasks.*

- [ ] `crates/ironhermes-cli/src/tui/mod.rs` — TUI module root
- [ ] `crates/ironhermes-cli/src/tui/status_line.rs` — status line state + renderer
- [ ] `crates/ironhermes-cli/src/tui/knight_rider.rs` — scanner frame generator
- [ ] `crates/ironhermes-cli/src/tui/double_ctrl_c.rs` — double-ctrl-c state machine
- [ ] `crates/ironhermes-cli/src/tui/activity.rs` — shared activity state (watch channel)
- [ ] `crates/ironhermes-cli/tests/tui_tests.rs` — integration-level tests where needed

---

## Manual-Only Verifications

| Behavior | Decision | Why Manual | Test Instructions |
|----------|----------|------------|-------------------|
| Status line pills render with alternating colors | D-04 | Visual styling requires human eyes | `cargo run -- chat`, observe bottom bar |
| Knight-rider scanner animates smoothly | D-06, D-07 | Animation frame rate / smoothness is subjective | `cargo run -- chat`, issue a prompt, watch bottom-left during tool calls |
| Scanner hides when idle | D-08 | Visual state transition | Observe bottom-left after turn completes |
| First ctrl-c cancels mid-stream cleanly | D-11 | Requires real signal + real provider call | `cargo run -- chat`, send a prompt, ctrl-c during streaming |
| Second ctrl-c exits within 1.5s | D-12 | Requires real signals + timing | Same as above, press ctrl-c twice quickly |
| Ctrl-c at prompt does NOT exit | D-14 | Requires rustyline context | `cargo run -- chat`, press ctrl-c at the prompt |
| Third ctrl-c emergency exit within 3s | RESEARCH footgun fix | Requires real signal chain | Three rapid ctrl-c presses should exit with code 130 |
| Terminal resize doesn't break rendering | D-Claude's-discretion | SIGWINCH requires live terminal | Start chat, resize window, observe redraw |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references from RESEARCH.md
- [ ] No watch-mode flags in test commands
- [ ] Feedback latency < 30s
- [ ] Manual-only verifications have explicit human-actionable instructions
- [ ] `nyquist_compliant: true` set in frontmatter after planner review

**Approval:** pending
