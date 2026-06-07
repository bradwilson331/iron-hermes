# Phase 2: Design System - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-02
**Phase:** 02-design-system
**Areas discussed:** CSS file topology, Phase 2 visible output, Font loading scope, Tailwind link disposition

---

## CSS File Topology

| Option | Description | Selected |
|--------|-------------|----------|
| Three separate `<link>` tags | Inject main.css → design-tokens.css → warp-ih.css as three separate `document::Link` tags from app.rs in cascade order. Mirrors prototype HTML exactly. Fewest gotchas, easiest to debug. Three parallel HTTP requests. | ✓ |
| main.css `@imports` the others | Single `<link>` to main.css; main.css does `@import url(...)` for the others. One link in app.rs but `@import` is render-blocking and serializes the requests. Harder to inspect cascade in DevTools. | |
| Concatenate into single main.css | Manually merge all three into one file, link once. Single request but loses prototype separation; every prototype change requires re-merging by hand. | |

**User's choice:** Three separate `<link>` tags
**Notes:** Recommended option chosen — matches the prototype HTML's two-link pattern (tokens, then layout) extended with the existing main.css for brand-correct base styles per DS-04.

---

## Phase 2 Visible Output

| Option | Description | Selected |
|--------|-------------|----------|
| Minimal wordmark + shield stub | Replace Hero contents with a small centered layout: wordmark.svg + ih-shield.png on dark `--bg`, font set to Ioskeley Mono. Satisfies SC #4 with minimal code that Phase 3 will fully replace. Token verification via DevTools. | ✓ |
| Design preview page | Build a throwaway 'design system preview' showing color swatches, type scale, sample wh-block elements with each accent stripe, scanner-cell glyphs, brand assets. Phase 3 scraps it. ~50–100 LOC of disposable code. | |
| Keep Hero, swap assets only | Leave existing Hero structure (header image + link buttons) but swap HEADER_SVG → wordmark, add ih-shield somewhere. Least intrusive but result is incoherent (terminal-themed CSS behind a Dioxus tutorial layout). | |

**User's choice:** Minimal wordmark + shield stub
**Notes:** Recommended option chosen. Avoids ~50–100 LOC of throwaway preview code; DevTools verification is what success criteria #1–#3 explicitly specify.

---

## Font Loading Scope

| Option | Description | Selected |
|--------|-------------|----------|
| All 16 weights, verbatim port | Copy all 16 woff2 files into assets/fonts/ and port all 16 `@font-face` declarations from colors_and_type.css verbatim. Browsers lazy-fetch per used weight (declared-but-unused weights are NOT downloaded). Keeps design system file byte-identical to source of truth. | ✓ |
| Only weights used by the shell | Subset to ~5 weights (Regular, Bold, Medium, SemiLight, Condensed Regular). Smaller assets/fonts/ and CSS, but drift from prototype — future phases that want italic/extra-bold/condensed-bold would need piecemeal additions. | |
| All 16 woff2 copied, only used weights `@font-face`-d | Copy all 16 woff2 (so they're available) but only emit `@font-face` for the ~5 used weights. Hybrid: full asset library, lean CSS. Wastes disk on unused fonts that never get served. | |

**User's choice:** All 16 weights, verbatim port
**Notes:** Recommended option chosen. The size concern is moot because woff2 files declared-but-unused in `@font-face` are not downloaded by browsers — the cost is zero, the fidelity is full.

---

## Tailwind Link Disposition

| Option | Description | Selected |
|--------|-------------|----------|
| Keep the link as-is | Phase 1 wired the dx-CLI Tailwind pipeline. PROJECT.md says 'Tailwind stays available but unused.' Link costs ~11KB (cacheable), and being available means a future utility class is a one-line change. | ✓ |
| Drop the link from app.rs | Remove the `document::Link` and asset constant. Lower request count, no risk of Tailwind preflight resets fighting ported CSS. Conflicts with PROJECT.md's 'available but unused' wording. | |
| Keep the link, strip tailwind.css to a comment | Leave the link, replace tailwind.css input with a comment. Avoids the ~11KB compiled output while preserving the wiring. Signals intent in code. | |

**User's choice:** Keep the link as-is
**Notes:** Recommended option chosen — matches the documented PROJECT.md decision verbatim.

---

## Claude's Discretion

The following implementation details were not asked but were explicitly noted in CONTEXT.md as Claude's discretion (planner may adjust):

- CSS load order placement (main → design-tokens → warp-ih → tailwind)
- Brand asset placement in stub Hero (~32px wordmark over ~96px shield, 24px gap, centered)
- Existing `assets/main.css` content — replaced wholesale (current 39 lines of Segoe-UI placeholder don't match brand)
- Theme data-attribute overrides ported now even though Phase 5 owns the runtime switching
- `scanner.svg` opportunistic copy (Phase 3 use) — planner decides whether to include here

## Deferred Ideas

- **Design preview page** — considered, rejected for Phase 2 scope. Possible future diagnostic if visual regressions appear.
- **Scanner SVG copy** — discovered during scout; belongs to Phase 3/4 scanner implementation, not Phase 2.
- **PROJECT.md / CLAUDE.md doc-staleness fixup** — Phase 1 code review IN-05 flagged stale Cargo.lock comment; IN-04 flagged missing `[lints.clippy]` table. Candidates for a small "01.1" hygiene gap-closure before Phase 4 (first async).
- **Tailwind file content cleanup** — third option (strip to comment) deferred; revisit at milestone close if Tailwind stays unused throughout v1.
