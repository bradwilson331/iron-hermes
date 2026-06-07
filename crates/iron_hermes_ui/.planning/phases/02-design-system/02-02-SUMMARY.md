---
phase: 02-design-system
plan: 02
subsystem: ui
tags: [css, design-system, warp-shell, vendored, verbatim-port]

# Dependency graph
requires:
  - phase: 01-hygiene
    provides: assets/ directory exists; Dioxus 0.7.1 asset pipeline wired
provides:
  - assets/warp-ih.css — Warp shell layout CSS (511 lines = 4 attribution + 507 verbatim source)
  - .wh-* selector family (101 selector occurrences) for desktop shell, blocks, status bar, palette, scanner cells
  - [data-theme=cyan|magenta|green|amber] override blocks (carried forward for Phase 5 theme switcher)
  - [data-density=compact] override block (carried forward for Phase 5 density switcher)
  - [data-block=framed|flat|minimal] override blocks (carried forward for Phase 5 block-style switcher)
  - [data-agent=right|bottom|hidden] override blocks (carried forward for Phase 5 agent layout switcher)
affects: [03-desktop-shell, 04-interactions, 05-tweaks-panel, 06-mobile-shell]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - Verbatim CSS port with attribution header (RESEARCH.md Pattern 1)
    - Source-of-truth boundary preserved (no @import, no concatenation per D-02)

key-files:
  created:
    - assets/warp-ih.css
  modified: []

key-decisions:
  - "Verbatim mechanical port via printf+cat shell recipe — preserves bytes, line endings, encoding (UTF-8 LF, no BOM)"
  - "Theme/density/block/agent data-attribute override blocks ported now even though runtime switching is Phase 5 — pure CSS, no runtime cost (per CONTEXT.md Discretion)"
  - "Single attribution header at top (4 lines) is the only deviation from byte-identity; verified via tail -n +5 | cmp"

patterns-established:
  - "Pattern 1 — Verbatim CSS port: prepend 4-line attribution comment, then cat source. Verify byte-identity via tail -n +5 | cmp."

requirements-completed: [DS-03]

# Metrics
duration: 2min
completed: 2026-05-03
---

# Phase 2 Plan 02: Warp shell layout CSS port Summary

**Verbatim byte-identical port of `warp2ironhermes/project/styles/warp-ih.css` (507 lines) to `assets/warp-ih.css` with 4-line attribution header — DS-03 file-complete.**

## Performance

- **Duration:** ~2 min
- **Started:** 2026-05-03T06:03:20Z
- **Completed:** 2026-05-03T06:05:00Z (approximate)
- **Tasks:** 1 (single mechanical port task)
- **Files modified:** 1 (`assets/warp-ih.css` created)

## Accomplishments

- Ported the full Warp shell layout CSS verbatim — 511 lines = 4 attribution + 507 source.
- All `.wh-*` selectors landed: 101 occurrences (RESEARCH.md DS-03 row noted ~57; the 101 figure counts every `.wh-*` token across selector lists, exceeding the ≥ 50 threshold).
- All four data-attribute override families present and ride along for Phase 5: `[data-theme=` (4), `[data-density=` (1), `[data-block=` (4), `[data-agent=` (3).
- Zero `@import`, zero `url(...)` introduced (matches source); UTF-8, LF-only, no BOM (`head -c 3 | od` = `2f2a20`, the literal `/* ` opener — not `efbbbf`).
- Source tree (`warp2ironhermes/project/styles/warp-ih.css`) verified untouched at 507 lines post-port.

## Task Commits

Each task was committed atomically:

1. **Task 1: Verbatim port warp-ih.css → assets/warp-ih.css with 4-line attribution header** — `e84dd8e` (feat)

_(No separate plan-metadata commit — orchestrator owns the metadata commit after the wave merges, per parallel-executor instructions.)_

## Files Created/Modified

- `assets/warp-ih.css` — 511 lines: 4-line attribution header + verbatim 507-line source. Provides Warp surface scale (`--w-bg-0..4`), block stripes (`--w-stripe-*`), `.wh-app`, `.wh-titlebar`, `.wh-block` family, status-bar pill rules, scanner cell rules, palette overlay, and density/theme/block/agent data-attribute override blocks. Consumed downstream by Phase 3 (desktop shell render), Phase 4 (interactions), Phase 5 (tweaks panel runtime data-attribute switching), Phase 6 (mobile shell — uses `[data-density="compact"]`, `[data-agent="hidden"|"bottom"]` rules already present here).

## Decisions Made

None beyond what was already locked in CONTEXT.md / RESEARCH.md. The plan was executed exactly as specified (Pattern 1 verbatim copy recipe, no edits to source bytes, no `@import` introduced, no concatenation with `design-tokens.css`).

## Deviations from Plan

None - plan executed exactly as written.

The plan body explicitly stated `<deviations>None for this plan.</deviations>` and `<deferred>Nothing deferred from this plan's scope.</deferred>` — both held true at execution time. No Rule 1 (bug), Rule 2 (missing critical), Rule 3 (blocking), or Rule 4 (architectural) deviations were triggered.

## Acceptance Criteria — Verification Results

All 17 acceptance criteria from the plan PASS:

| # | Criterion | Expected | Actual | Result |
|---|-----------|----------|--------|--------|
| 1 | `test -f assets/warp-ih.css` | exit 0 | exit 0 | PASS |
| 2 | Byte-identity past line 4: `tail -n +5 \| cmp -` | exit 0 | exit 0 | **PASS (cmp exit 0)** |
| 3 | `grep -c '\.wh-'` | ≥ 50 | 101 | PASS |
| 4 | `grep -c '\.wh-app'` | ≥ 1 | 1 | PASS |
| 5 | `grep -c '\[data-theme='` | ≥ 1 | 4 | PASS |
| 6 | `grep -c '\[data-density='` | ≥ 1 | 1 | PASS |
| 7 | `grep -c '\[data-block='` | ≥ 1 | 4 | PASS |
| 8 | `grep -c '\[data-agent='` | ≥ 1 | 3 | PASS |
| 9 | `wc -l < assets/warp-ih.css` | ≥ 510 | 511 | PASS |
| 10 | Header line 1 exact | `/* Source: warp2ironhermes/project/styles/warp-ih.css` | exact match | PASS |
| 11 | Header line 2 regex `Copied: YYYY-MM-DD (Phase 2)` | matches | ` * Copied: 2026-05-03 (Phase 2)` | PASS |
| 12 | Header line 3 exact | ` * DO NOT EDIT — re-sync from source if upstream changes.` | exact match (em-dash preserved) | PASS |
| 13 | Header line 4 exact | ` */` | ` */` | PASS |
| 14 | `grep -c '^@import'` | 0 | 0 | PASS |
| 15 | `grep -c 'url('` | 0 | 0 | PASS |
| 16 | No UTF-8 BOM (first 3 bytes ≠ `efbbbf`) | first 3 bytes are `2f2a20` (`/* `) | not `efbbbf` | PASS |
| 17 | Source untouched (`wc -l < warp2ironhermes/...`) | 507 | 507 | PASS |

**UTC date in attribution header line 2:** `2026-05-03`

## Issues Encountered

**Worktree setup gap (resolved during execution, not committed):** The `warp2ironhermes/` reference tree and `assets/` directory are untracked in the parent repo (per the session-start `git status`). The Claude Code worktree therefore did not contain them at agent start. Both were physically copied from the parent repo into the worktree to make the verbatim-port shell recipe runnable; neither was staged or committed (only `assets/warp-ih.css` was committed, per plan scope). This is a pre-existing project-state issue (the `warp2ironhermes/` reference handoff has not yet been added to git in the parent repo) — not a deviation from this plan, which assumed both directories were present. The orchestrator / future plans (likely 02-04) will need to commit `assets/` (existing files: favicon.ico, header.svg, main.css, tailwind.css) and decide on `warp2ironhermes/` tracking policy.

## Known Stubs

None. Single `::placeholder` match in the file (line 303: `.wh-textarea::placeholder { color: var(--fg-dim); }`) is a CSS pseudo-element selector, not a stub.

## Next Phase Readiness

- **DS-03 file-complete.** Runtime verification (`.wh-app` selector reachable from `document.styleSheets`, `[data-theme="..."]` overrides applied when attribute set on `<html>`) lives in 02-04's manual UAT after the Wave 1 plans merge and the `<link>` chain is wired in `src/app.rs`.
- **No build wiring done here** (deferred to 02-04 per plan `<objective>`); `assets/warp-ih.css` is on disk but not yet referenced from `src/app.rs`.
- **Cascade ordering against Tailwind preflight** is handled in 02-04 (RESEARCH.md Pitfall 1 — Tailwind first vs. last is 02-04's call).
- **Wave 1 sibling plans (02-01 design-tokens.css, 02-03 fonts + brand assets)** run independently and do not depend on this file. Wave 2 (02-04 build wiring) consumes all three Wave 1 outputs.

## Self-Check: PASSED

**Files claimed created — verification:**
- `assets/warp-ih.css` → FOUND (511 lines, 14881-byte source past header confirmed via `cmp` exit 0)
- `.planning/phases/02-design-system/02-02-SUMMARY.md` → FOUND (this file, written before this self-check section)

**Commits claimed — verification:**
- `e84dd8e` (Task 1 commit) → FOUND in `git log --oneline -1`: `e84dd8e feat(02-02): port warp-ih.css verbatim to assets/warp-ih.css (DS-03)`

**Acceptance criteria results table above all PASS.** Source unchanged (`wc -l < warp2ironhermes/project/styles/warp-ih.css` = 507).

---
*Phase: 02-design-system*
*Plan: 02*
*Completed: 2026-05-03*
