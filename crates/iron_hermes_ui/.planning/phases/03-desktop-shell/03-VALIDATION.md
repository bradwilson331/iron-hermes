---
phase: 3
slug: desktop-shell
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-05-03
---

# Phase 3 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | `cargo build` + `cargo clippy` (no unit/integration test framework — explicitly out of scope per PROJECT.md "Out of Scope" table) |
| **Config file** | `Cargo.toml` (feature flags), `clippy.toml` (signal-borrow rules) |
| **Quick run command** | `cargo build --features web` |
| **Full suite command** | `cargo build --features web && cargo build --features desktop && cargo build --features mobile && cargo clippy --features web -- -D warnings` |
| **Estimated runtime** | ~30s quick / ~90s full (cold) / ~10s incremental |
| **Manual UAT** | `dx serve --features web` + side-by-side comparison against `warp2ironhermes/project/Warp × IronHermes.html` (per project STATE.md todo: "After Phase 3: side-by-side visual review of desktop shell against prototype HTML") |

---

## Sampling Rate

- **After every task commit:** Run `cargo build --features web` (quick gate)
- **After every plan wave:** Run full suite (all three features + clippy)
- **Before `/gsd-verify-work`:** Full suite green AND manual UAT approved
- **Max feedback latency:** 90 seconds (full cold build)

---

## Per-Task Verification Map

> The planner populates this table after task IDs are assigned. Below are the requirement-to-validation slots; each task `<automated>` block must reference one of these checks.

| Requirement | Validation Type | Automated Command | Manual Step | Notes |
|---|---|---|---|---|
| SHELL-01 (title bar: traffic lights, tab strip, ⌘K display) | manual UAT | full suite passes | inspect rendered title bar against prototype `Warp × IronHermes.html` head region | visual fidelity test |
| SHELL-02 (5 stripe types + 6th tool variant render) | manual UAT | full suite passes | scroll demo_blocks() output, confirm each stripe color matches prototype | visual fidelity test |
| SHELL-03 (hover affordances reveal copy/rerun/share) | manual UAT | full suite passes | hover any block; copy/rerun/share buttons appear via CSS `:hover` | pure CSS, zero Rust state |
| SHELL-04 (CommandLine bin/arg/flag tokens with distinct colors) | manual UAT | full suite passes | inspect `is-cmd` block; bin/arg/flag spans have prototype colors | type-driven render |
| SHELL-05 (tool-call block: name + args summary + status) | manual UAT | full suite passes | confirm 4 tool blocks render with Pending/Running/Done/Failed status states | exercises ToolStatus enum |
| SHELL-06 (input box: mode glyph + auto-grow + focus ring) | manual UAT | full suite passes | confirm `❯` mode glyph + cyan focus ring with glow on click | textarea inherits prototype CSS |
| SHELL-07 (agent side panel: right 360px when data-agent="right") | manual UAT | full suite passes | confirm side panel visible at right, 360px wide | hardcoded data-agent="right" |
| SHELL-08 (status bar: rotating dot-pills mode·model·provider·tokens·scanner) | manual UAT | full suite passes | confirm 5 dot-pills with prototype defaults from app.jsx | prototype-default verbatim |
| SHELL-09 (scanner: 10 cells animating via CSS @keyframes through ░ ▒ ▓ █) | manual UAT | full suite passes; new `scanner-anim.css` file exists | observe knight-rider animation running continuously on page load | hardcoded is-active for Phase 3 |
| SHELL-10 (command palette overlay: slash commands + workflow items) | manual UAT | full suite passes | palette renders open by default; slash commands and workflows visible | hardcoded open for Phase 3 UAT |

---

## Wave 0 Requirements

> Wave 0 = pre-execution dependencies that must exist before any task can verify.

- [ ] No new test infrastructure required — `cargo build` and `cargo clippy` are already configured.
- [ ] `assets/scanner-anim.css` (or equivalent location per planner) — new file with `@keyframes wh-scanner-tick` rule. Authored as part of Phase 3 plan (UI-SPEC planner-handoff #1, RESEARCH.md Open Question #1).
- [ ] `src/components/hero.rs` deletion does NOT break `cargo build --features web`. Mitigated by replacing the `Hero {}` call site in `app.rs` with `WarpHermes {}` in the same commit.

*Existing infrastructure covers Phase 3's automated gate. Manual UAT is the dominant verification method.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Pixel-perfect visual fidelity to prototype | All SHELL-01..SHELL-10 | Visual regression testing is Out of Scope per PROJECT.md (v2 milestone); no headless browser automation in v1 | Run `dx serve --features web`, open `localhost:8080` in one tab and `warp2ironhermes/project/Warp × IronHermes.html` in another; compare title bar, block stream, input box, side panel, status bar, and palette overlay side-by-side. Verify font is Ioskeley Mono. Verify accent color is cyan `#4ec9b0`. |
| Scanner animation runs continuously | SHELL-09 | CSS `@keyframes` animation can be inspected visually but not automatically tested without a browser-automation framework (out of scope) | Observe scanner cells in status bar; confirm 10-cell knight-rider triangle-wave through `░ ▒ ▓ █` glyphs. Approximate tick rate is 100ms; the animation should appear smooth and continuous on page load. |
| Hover affordances reveal on hover | SHELL-03 | Pure CSS `:hover` cannot be automated without a browser harness | Hover each of the 8–10 demo blocks; confirm copy (⎘), rerun (↻), share (↗) buttons fade in. |
| Focus ring + glow on input box | SHELL-06 | CSS `:focus` cannot be automated without a browser harness | Click the input textarea; confirm cyan focus ring with glow appears around the input wrap. |
| Three-platform compile gate (inherited from Phase 1) | All requirements | The phase gate is automated but the result must be inspected to confirm clean output | Run `cargo build --features web && cargo build --features desktop && cargo build --features mobile && cargo clippy --features web -- -D warnings`; all four commands must exit 0. |

---

## Validation Sign-Off

- [ ] Three-platform compile gate is the authoritative automated gate
- [ ] Manual UAT covers every SHELL-XX requirement (10/10)
- [ ] No 3 consecutive tasks without compile gate verification (every task triggers `cargo build --features web` post-commit)
- [ ] Wave 0 dependency `assets/scanner-anim.css` is authored before any task that references it
- [ ] No watch-mode flags (CI uses cold build)
- [ ] Feedback latency < 90s
- [ ] `nyquist_compliant: true` set in frontmatter once planner has populated the per-task table

**Approval:** pending — set to `approved YYYY-MM-DD` after planner has wired the per-task verification map and the executor has confirmed the three-platform gate is green.
