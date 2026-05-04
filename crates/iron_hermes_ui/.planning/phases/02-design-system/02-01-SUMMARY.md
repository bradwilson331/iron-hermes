---
phase: 02-design-system
plan: 01
subsystem: ui
tags: [css, fonts, design-tokens, ansi-palette, ioskeley-mono, woff2, dioxus, assets]

# Dependency graph
requires:
  - phase: 01-hygiene
    provides: "assets/ directory established by Phase 1; project module split (src/app.rs, src/components/hero.rs) is the consumer of these tokens in Plan 02-04"
provides:
  - "16 IoskeleyMono woff2 font files at assets/fonts/ (byte-identical to prototype)"
  - "assets/design-tokens.css — 245-line verbatim port of colors_and_type.css with 4-line attribution header"
  - "16 @font-face declarations preserved with relative url(\"fonts/...\") refs (no asset!() wrapping per D-06)"
  - "ANSI palette CSS custom properties (--accent-primary, --brand, --bg, --font-mono, etc.) ready for cascade"
affects: ["02-design-system (plans 02-02, 02-03, 02-04 read these tokens)", "03-desktop-shell", "04-interactions", "05-theming", "06-mobile"]

# Tech tracking
tech-stack:
  added:
    - "IoskeleyMono woff2 family (16 weight/width variants) as static web fonts"
  patterns:
    - "Verbatim CSS port with 4-line attribution header (Pattern 1 from 02-RESEARCH.md)"
    - "Un-declared static-asset font serving (D-06): woff2 files served from /assets/fonts/ without asset!() wrapping; CSS uses relative url(\"fonts/...\") refs"

key-files:
  created:
    - "assets/fonts/IoskeleyMono-Black.woff2"
    - "assets/fonts/IoskeleyMono-Bold.woff2"
    - "assets/fonts/IoskeleyMono-BoldItalic.woff2"
    - "assets/fonts/IoskeleyMono-Condensed.woff2"
    - "assets/fonts/IoskeleyMono-CondensedBold.woff2"
    - "assets/fonts/IoskeleyMono-CondensedMedium.woff2"
    - "assets/fonts/IoskeleyMono-ExtraBold.woff2"
    - "assets/fonts/IoskeleyMono-ExtraLight.woff2"
    - "assets/fonts/IoskeleyMono-Italic.woff2"
    - "assets/fonts/IoskeleyMono-Light.woff2"
    - "assets/fonts/IoskeleyMono-Medium.woff2"
    - "assets/fonts/IoskeleyMono-Regular.woff2"
    - "assets/fonts/IoskeleyMono-SemiBold.woff2"
    - "assets/fonts/IoskeleyMono-SemiCondensed.woff2"
    - "assets/fonts/IoskeleyMono-SemiLight.woff2"
    - "assets/fonts/IoskeleyMono-Thin.woff2"
    - "assets/design-tokens.css"
  modified: []

key-decisions:
  - "Followed plan exactly: verbatim byte-copy + 4-line attribution header (no asset!() wrapping, no @import, no concatenation)"
  - "Attribution header date is 2026-05-03 UTC (executor-day, matches plan's permissive YYYY-MM-DD regex)"

patterns-established:
  - "Verbatim CSS port: source bytes preserved past line N attribution header; cmp(tail -n +N+1) verifies"
  - "Static-asset font serving: woff2 files dropped into assets/fonts/ are served at /assets/fonts/ without Rust asset!() declaration; CSS relative url(\"fonts/...\") resolves correctly"

requirements-completed:
  - DS-01
  - DS-02

# Metrics
duration: ~3min
completed: 2026-05-03
---

# Phase 02 Plan 01: Design Tokens Verbatim Port Summary

**Vendored 16 IoskeleyMono woff2 fonts and byte-copied colors_and_type.css → assets/design-tokens.css (245 lines, 4-line attribution header + 241-line source) — ANSI palette, type scale, brand color, scanner constants, 16 @font-face declarations all preserved verbatim from the warp2ironhermes prototype.**

## Performance

- **Duration:** ~3 min
- **Started:** 2026-05-03T (PLAN_START before Task 1)
- **Completed:** 2026-05-03 (after Task 2 commit `36868d3`)
- **Tasks:** 2 (both `type="auto"`)
- **Files created:** 17 (16 woff2 fonts + 1 CSS file)
- **Files modified:** 0

## Accomplishments

- All 16 IoskeleyMono `.woff2` weight/width variants vendored under `assets/fonts/`, byte-identical to prototype (verified via `cmp` on `IoskeleyMono-Regular.woff2` and the source-still-has-16 invariant).
- `assets/design-tokens.css` produced: 245 lines (4-line attribution header + 241-line verbatim source). Byte-identity past line 4 confirmed via `tail -n +5 ... | cmp - source` (exit 0).
- 16 `@font-face` declarations preserved (`grep -c '@font-face'` → 16).
- 16 relative-form font URL references preserved (`grep -c 'url("fonts/IoskeleyMono-'` → 16); zero `asset!()` wrapping per D-06.
- Zero `@import` directives introduced (`grep -c '^@import'` → 0); D-02 invariant honored.
- No UTF-8 BOM (file opens with `2f2a20` = `/*`); UTF-8 LF preserved by `cat`-into-redirect recipe per RESEARCH.md Pattern 1.
- Source tree `warp2ironhermes/project/ironhermes/colors_and_type.css` untouched (still 241 lines); `warp2ironhermes/project/ironhermes/fonts/` still has 16 woff2 files.

## Task Commits

Each task was committed atomically:

1. **Task 1: Copy 16 IoskeleyMono woff2 files into assets/fonts/** — `d32b96b` (feat)
2. **Task 2: Verbatim port colors_and_type.css → assets/design-tokens.css with 4-line attribution header** — `36868d3` (feat)

(No final metadata commit — orchestrator owns STATE.md/ROADMAP.md updates after wave merge per worktree-mode protocol; this SUMMARY.md is committed alongside the deferred-items log in a closing commit below.)

## Files Created/Modified

- `assets/fonts/IoskeleyMono-Black.woff2` — 750 weight, byte-copy of prototype source
- `assets/fonts/IoskeleyMono-Bold.woff2` — 700 weight, byte-copy
- `assets/fonts/IoskeleyMono-BoldItalic.woff2` — 700 weight italic, byte-copy
- `assets/fonts/IoskeleyMono-Condensed.woff2` — 400 weight 75% width, byte-copy
- `assets/fonts/IoskeleyMono-CondensedBold.woff2` — 700 weight 75% width, byte-copy
- `assets/fonts/IoskeleyMono-CondensedMedium.woff2` — 500 weight 75% width, byte-copy
- `assets/fonts/IoskeleyMono-ExtraBold.woff2` — 800 weight, byte-copy
- `assets/fonts/IoskeleyMono-ExtraLight.woff2` — 200 weight, byte-copy
- `assets/fonts/IoskeleyMono-Italic.woff2` — 400 weight italic, byte-copy
- `assets/fonts/IoskeleyMono-Light.woff2` — 300 weight, byte-copy
- `assets/fonts/IoskeleyMono-Medium.woff2` — 500 weight, byte-copy
- `assets/fonts/IoskeleyMono-Regular.woff2` — 400 weight body default, byte-copy
- `assets/fonts/IoskeleyMono-SemiBold.woff2` — 600 weight, byte-copy
- `assets/fonts/IoskeleyMono-SemiCondensed.woff2` — 400 weight 87.5% width, byte-copy
- `assets/fonts/IoskeleyMono-SemiLight.woff2` — 350 weight (dim/captions), byte-copy
- `assets/fonts/IoskeleyMono-Thin.woff2` — 100 weight, byte-copy
- `assets/design-tokens.css` — 245 lines: 4-line attribution header + 241-line verbatim port of `warp2ironhermes/project/ironhermes/colors_and_type.css` (ANSI 16-color palette, surfaces, text colors, semantic accents, pill rotation, type scale, spacing, radii, scanner constants, z-layering, base body/h1/h2/p/code/pre/hr rules, utility classes, pill classes, status glyphs, layout primitives, 16 @font-face blocks)

## Decisions Made

- None beyond plan: executed Pattern 1 bash recipe verbatim. Attribution-header date string resolved to `2026-05-03` (executor's UTC day), which the plan's acceptance criteria allow via the YYYY-MM-DD regex.

## Deviations from Plan

None — plan executed exactly as written. Both tasks completed within their first attempt; no auto-fixes triggered (no Rule 1 / Rule 2 / Rule 3 deviations).

## Acceptance-Criteria Evidence

### Task 1 (16 woff2 fonts)

| Check | Command | Expected | Actual |
|---|---|---|---|
| Count | `ls -1 assets/fonts/IoskeleyMono-*.woff2 \| wc -l` | 16 | 16 |
| Regular present | `test -f assets/fonts/IoskeleyMono-Regular.woff2` | exit 0 | exit 0 |
| Bold present | `test -f assets/fonts/IoskeleyMono-Bold.woff2` | exit 0 | exit 0 |
| Thin present | `test -f assets/fonts/IoskeleyMono-Thin.woff2` | exit 0 | exit 0 |
| CondensedMedium present | `test -f assets/fonts/IoskeleyMono-CondensedMedium.woff2` | exit 0 | exit 0 |
| Byte identity (Regular) | `cmp assets/fonts/IoskeleyMono-Regular.woff2 warp2ironhermes/.../IoskeleyMono-Regular.woff2` | exit 0 | exit 0 |
| Non-woff2 count | `ls assets/fonts/ \| grep -vc '\.woff2$'` | 0 | 0 |
| Source unchanged | `ls warp2ironhermes/.../IoskeleyMono-*.woff2 \| wc -l` | 16 | 16 |

### Task 2 (design-tokens.css)

| Check | Command | Expected | Actual |
|---|---|---|---|
| File exists | `test -f assets/design-tokens.css` | exit 0 | exit 0 |
| Byte-identity past header | `tail -n +5 assets/design-tokens.css \| cmp - warp2ironhermes/.../colors_and_type.css` | exit 0 | exit 0 |
| @font-face count | `grep -c '@font-face' assets/design-tokens.css` | 16 | 16 |
| Total line count | `wc -l < assets/design-tokens.css` | ≥ 245 | 245 |
| Header line 1 | `sed -n '1p' ...` | `/* Source: warp2ironhermes/project/ironhermes/colors_and_type.css` | matches exactly |
| Header line 2 | `sed -n '2p' ...` matches `[0-9]{4}-[0-9]{2}-[0-9]{2}` | YYYY-MM-DD | ` * Copied: 2026-05-03 (Phase 2)` |
| Header line 3 | `sed -n '3p' ...` | ` * DO NOT EDIT — re-sync from source if upstream changes.` | matches exactly |
| Header line 4 | `sed -n '4p' ...` | ` */` | matches exactly |
| @import count | `grep -c '^@import'` | 0 | 0 |
| Font URL count | `grep -c 'url("fonts/IoskeleyMono-'` | 16 | 16 |
| BOM check | `head -c 3 ... \| od -An -tx1` | NOT `efbbbf` | `2f2a20` (= `/*`) |
| Source unchanged | `wc -l < warp2ironhermes/.../colors_and_type.css` | 241 | 241 |

## Issues Encountered

### Build Verification (Out of Scope, Logged)

The plan's `<verification>` block specifies `cargo build --features web` should still succeed. In this worktree, `cargo build --features web` fails with **pre-existing** "Asset at /assets/X doesn't exist" errors for `favicon.ico`, `main.css`, `tailwind.css`, and `header.svg` — assets that `src/app.rs` and `src/components/hero.rs` reference but that the worktree base commit (`9cd3e80`) does not track in `assets/` (verified via `git ls-tree -r --name-only 9cd3e80 | grep '^assets/'` = empty).

These failures are **not caused by Plan 02-01's changes** — Plan 02-01 only adds new files (16 woff2 + 1 CSS) and does not modify any of the four missing-asset call sites. Per `<deviation_rules>` SCOPE BOUNDARY: only auto-fix issues directly caused by the current task's changes.

Logged to `.planning/phases/02-design-system/deferred-items.md` as `DEFERRED-02-01-A` for orchestrator/Plan 02-04 attention. Plan 02-04 rewrites `src/app.rs` and `src/components/hero.rs` and is the natural place to resolve these references; alternatively the worktree-mode setup may need to include the existing `assets/` files from the main checkout.

## User Setup Required

None — no external service configuration required. All work is local file copies.

## Next Phase Readiness

Plan 02-01 unblocks the design-token cascade for downstream Wave-1 plans:

- **02-02 (brand assets, parallel wave 1):** independent — copies wordmark.svg + ih-shield.png; no overlap.
- **02-03 (warp-ih.css port, parallel wave 1):** independent — copies the second CSS file.
- **02-04 (wave 2 — wires `<link>` tags + Hero rewrite):** consumes `DESIGN_TOKENS_CSS` asset constant from this plan's output. Will also resolve the pre-existing missing-asset cargo errors when it rewrites `src/app.rs` and `src/components/hero.rs`.

The runtime verification (font actually rendering as Ioskeley Mono in the browser) lives in 02-04's manual UAT once the `<link>` tag is wired and `dx serve` is run.

## Threat Surface Scan

No new security-relevant surface introduced. This plan adds same-origin static assets (CSS + woff2). Per the plan's `<threat_model>`:

- **T-02-01 (T): Vendored woff2 files** — disposition: accept. Same-origin only; no third-party CDN; supply-chain covered by repo review.
- **T-02-02 (I): design-tokens.css and font files** — disposition: accept. Public design tokens; no secrets.

No `mitigate` dispositions assigned — Rule 2 not triggered. No new endpoints, auth paths, file-access patterns, or schema changes at trust boundaries. No threat flags raised.

## Self-Check: PASSED

**Files verified to exist:**
- `assets/fonts/IoskeleyMono-Regular.woff2` — FOUND
- `assets/fonts/IoskeleyMono-Bold.woff2` — FOUND
- `assets/fonts/IoskeleyMono-Thin.woff2` — FOUND
- `assets/fonts/IoskeleyMono-CondensedMedium.woff2` — FOUND
- `assets/design-tokens.css` — FOUND (245 lines)

**Commits verified to exist:**
- `d32b96b` (Task 1) — FOUND in `git log --all`
- `36868d3` (Task 2) — FOUND in `git log --all`

(See verification commands above; smoke-check evidence captured in the Acceptance-Criteria Evidence tables.)

---
*Phase: 02-design-system*
*Plan: 01*
*Completed: 2026-05-03*
