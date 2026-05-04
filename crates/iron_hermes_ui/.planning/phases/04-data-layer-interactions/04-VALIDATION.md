---
phase: 4
slug: data-layer-interactions
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-05-03
---

# Phase 4 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> Phase 4 has no test framework (TEST-01..03 are v2 per PROJECT.md).
> The compile-time gate (`cargo build` × 3 platforms + clippy) is the proxy for "tests";
> manual UAT against `Warp × IronHermes.html` is the behavioral phase gate.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | None (TEST-01..03 deferred to v2 per PROJECT.md / REQUIREMENTS.md) |
| **Config file** | none — `Cargo.toml` features (`web` / `desktop` / `mobile`) and `clippy.toml` are the entire enforcement surface |
| **Quick run command** | `cargo build --features web` |
| **Full suite command** | `cargo build --features web && cargo build --features desktop && cargo build --features mobile && cargo clippy --features web -- -D warnings` |
| **Estimated runtime** | ~30–90s for full three-platform compile + clippy on warm cache; 3–5 min cold |

---

## Sampling Rate

- **After every task commit:** `cargo build --features web` (fastest feedback; catches most regressions)
- **After every wave merge:** Full three-platform compile gate + `cargo clippy --features web -- -D warnings`
- **Before `/gsd-verify-work`:** Full suite GREEN + manual UAT walk-through against `warp2ironhermes/project/Warp × IronHermes.html`
- **Max feedback latency:** 90 seconds (compile only); manual UAT is the irreducible gate per CONTEXT D-35

---

## Per-Task Verification Map

> Per-task IDs assigned by the planner. This map is **requirement-keyed** because Phase 4
> has no automated test framework — every requirement maps to a manual UAT probe plus
> compile-time gates.

| Req ID | Behavior | Test Type | Automated Command | Manual UAT | Status |
|--------|----------|-----------|-------------------|------------|--------|
| MOCK-01 | Six personality presets, one canned reply each (verbatim from app.jsx 342–348) | manual + compile | `cargo build --features web` | Cycle personality via `/personality`, send `hello` in Agent mode, observe reply text matches `personalities.rs` table for each variant | ⬜ pending |
| MOCK-02 | `runShell` ~600ms cmd→out timing | manual-only | (compile only — timing not unit-testable in v1) | Type `git status`, count seconds; ~600ms cmd→out, ~1400ms total | ⬜ pending |
| MOCK-03 | `runAgent` three-stage flow (user → tool-call → ai) with 400ms / 1000ms gaps | manual-only | (compile only) | ⌥M to Agent, type `hello`, observe user-message → tool-call message → ai-reply chain | ⬜ pending |
| MOCK-04 | Token counter +120 per submission, saturating at 128_000 | manual-only | (compile only) | Read status bar `tokens.used` before/after submit; +120 each time; never exceeds max | ⬜ pending |
| KBD-01 | ⌥M toggles mode glyph ❯ ↔ ✦ when input focused | manual + compile | `cargo build --features web` | Focus textarea, press ⌥M, observe glyph swap; press without focus → no swap | ⬜ pending |
| KBD-02 | ⌘K (or Ctrl-K) opens palette; Esc closes | manual + compile | `cargo build --features web` | ⌘K → palette overlay visible; Esc → hidden; both work from any focus | ⬜ pending |
| KBD-03 | ↑/↓ move palette selection; Enter selects active item | manual-only | (compile only) | Open palette, arrow keys move highlight (wraps at boundaries); Enter dispatches `pick(item)` | ⬜ pending |
| KBD-04 | Enter submits, Shift+Enter inserts newline | manual + compile | `cargo build --features web` | In textarea: Enter clears input + appends block; Shift+Enter inserts `\n` visibly without submitting | ⬜ pending |
| KBD-05 | Hover affordance handlers fire (copy ⎘, rerun ↻; share ↗ stub) | manual + compile | `cargo build --features web` (clipboard API requires `Clipboard` web-sys feature; compile validates) | Hover block, click ⎘ → text in OS clipboard; click ↻ on Cmd block → re-runs same command; click ↗ → no-op (deferred per D-25) | ⬜ pending |
| KBD-06 | `/personality` palette substate cycles personality and updates next mock reply | manual-only | (compile only) | Open palette, pick `/personality`, pick `noir`, send agent prompt → noir-style reply | ⬜ pending |

**Compile-time validation (continuously enforced — proxies for unit tests):**

- `cargo build --features web` — wasm32 compile (catches gloo-timers usage, web_sys feature gate completeness, wasm-bindgen Closure issues)
- `cargo build --features desktop` — native compile (catches missing `tokio::time` cfg gates and tokio runtime configuration)
- `cargo build --features mobile` — native compile (catches mobile-only regressions in webview wrapper)
- `cargo clippy --features web -- -D warnings` — enforces `clippy.toml` `await-holding-invalid-types` rule (the D-06 signal-borrow-across-await landmine)

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `Cargo.toml` deltas applied (one task per dep group): `gloo-timers = { version = "0.3", features = ["futures"] }`, `tokio = { version = "1", features = ["time"], default-features = false }`, `web-sys = { version = "0.3", features = [...] }`, `wasm-bindgen = "0.2"` and `js-sys` if surfaced as direct deps
- [ ] `src/platform/timer.rs` skeleton with cfg-gated `pub async fn sleep(ms: u32)` (gloo_timers on wasm32, tokio::time::sleep on native)
- [ ] `src/platform/mod.rs` exports `pub mod timer;`
- [ ] Three-platform compile gate green (baseline before any non-skeleton code lands)

*No test framework setup — Phase 4 stays test-free per PROJECT.md "tests are v2" deferral. The compile gate IS the verification surface.*

---

## Manual-Only Verifications

Per the test-deferral policy, **every behavioral acceptance criterion in Phase 4 is manual UAT.**
Compile + clippy catch type/borrow regressions; UAT against the prototype HTML catches behavioral drift.

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Submit timing 600ms / 1400ms | MOCK-02, MOCK-03 | No test framework; timing assertions need `wasm-bindgen-test` (v2) | Side-by-side: open `Warp × IronHermes.html` and `dx serve --features web`. Type `git status` in both. Visual timing should match within ±100ms. |
| Personality reply selection | MOCK-01, KBD-06 | UI-driven flow that traverses palette substate | Open palette, pick `/personality` → `noir`. In Agent mode, type `hello`. Reply should match `noir` row in `src/mocks/personalities.rs` REPLIES const. Repeat for all 6 variants. |
| Token counter increment | MOCK-04 | Status bar pixel readout; no programmatic assertion | Note `tokens.used` in status bar. Submit any command. Confirm value increased by exactly 120. Submit until you'd exceed 128_000; confirm saturation (no overflow). |
| ⌥M mode toggle behavior | KBD-01 | OS-level keyboard event chain | Focus textarea (cursor visible). Press ⌥M (macOS) or Alt+M (other). Glyph in input prefix should toggle ❯ ↔ ✦. Without focus, ⌥M should NOT toggle (don't intercept generic typing). |
| ⌘K palette open + Esc close | KBD-02 | Browser keyboard event preventDefault | From any focus state: press ⌘K (or Ctrl-K). Palette overlay should appear. Press Esc. Overlay should disappear AND palette substate reset to `Browse`. |
| Palette ↑/↓/Enter navigation | KBD-03 | Local Signal index walking | Open palette. Press ↓ four times → highlight on row 5. Press ↑ once → row 4. Press ↑ from row 0 → wraps to last row. Enter dispatches `pick(items[selected])`. |
| Shift+Enter newline | KBD-04 | Textarea native behavior preservation | In textarea, type `line one`, then Shift+Enter, then `line two`. Two visible lines should appear in the input. Pressing Enter (no shift) submits both lines as one command. |
| Copy ⎘ button | KBD-05 | Browser clipboard permission gate | Hover any block, click ⎘. Open OS clipboard (Cmd+V into another app). Should match block content. Failure (permission denied) should be silent — no toast in v1. |
| Rerun ↻ button | KBD-05 | Cmd-only behavior; visual disabled state on non-Cmd | Hover an `is-cmd` block, click ↻. Same command should re-execute (new is-cmd + is-out blocks appended). Hover an `is-out` block; ↻ button should be greyed out (cursor: not-allowed). |
| Three-platform parity | D-35 | No cross-compilation test in v1 | After every wave: run all three `cargo build --features <X>` commands. All MUST succeed. The cfg-gated `timer::sleep` is the only platform-conditional code; everything else compiles uniformly. |

---

## Validation Sign-Off

- [ ] All Phase 4 tasks have `<automated>` verify (the compile gate command) OR Wave 0 dependency on Cargo.toml deltas + timer.rs
- [ ] Sampling continuity: every task ends with at least `cargo build --features web` (no 3 consecutive tasks without compile feedback)
- [ ] Wave 0 covers all MISSING references (Cargo deps, timer.rs, mocks/ module declaration)
- [ ] No watch-mode flags (no `cargo watch`; one-shot builds only)
- [ ] Feedback latency < 90s on warm cache
- [ ] All 6 ROADMAP.md success criteria mapped to UAT probes above
- [ ] `nyquist_compliant: true` set in frontmatter once planner confirms every task has automated verification command

**Approval:** pending (planner reviews + sets `nyquist_compliant: true` after task IDs assigned)
