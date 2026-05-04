---
phase: 01-hygiene
plan: 02
subsystem: infra
tags: [rust, dioxus, modules, file-organization, scaffold]

# Dependency graph
requires:
  - phase: 01-hygiene
    provides: "Pinned dioxus = =0.7.1 (Plan 01-01) — guarantees the Dioxus 0.7 API surface seen by these new modules will not shift mid-phase"
provides:
  - "Rust module hierarchy under src/: app.rs (App root), components/ (mod.rs + hero.rs), state.rs (Phase 4 stub), platform/mod.rs (Phase 6 stub)"
  - "Slim src/main.rs containing only mod declarations + use app::App + fn main() — entry point is now <15 lines"
  - "Standard Rust re-export pattern (mod hero; pub use hero::Hero) for clean cross-module imports as crate::components::Hero"
  - "Asset-constant placement convention: each Asset is declared in the module that uses it (FAVICON/MAIN_CSS/TAILWIND_CSS in app.rs, HEADER_SVG in hero.rs) — not in main.rs"
affects: [01-03, 02-design-tokens, 03-desktop-shell, 04-data-layer, 05-tweaks-panel, 06-mobile-shell]

# Tech tracking
tech-stack:
  added: []  # Pure source reorganization; zero new dependencies
  patterns:
    - "Module hierarchy: src/main.rs is the thin entry point; component definitions live in src/<module>.rs or src/<module>/mod.rs"
    - "Component module split: a directory module (src/components/) uses mod.rs to declare private submodules and pub use to re-export selected items"
    - "Asset constant locality: const NAME: Asset = asset!(...) is declared in the module that consumes it, not centralized in main.rs"
    - "Stub modules carry a single comment naming the future phase that will populate them — no empty file confusion"

key-files:
  created:
    - "src/main.rs — slim entry point: mod app; mod components; mod state; mod platform; use app::App; fn main()"
    - "src/app.rs — pub fn App component + FAVICON/MAIN_CSS/TAILWIND_CSS asset constants"
    - "src/components/mod.rs — mod hero + pub use hero::Hero re-export"
    - "src/components/hero.rs — pub fn Hero (the Dioxus scaffold's link list, untouched semantically) + HEADER_SVG asset constant"
    - "src/state.rs — Phase 4 placeholder stub"
    - "src/platform/mod.rs — Phase 6 placeholder stub"
  modified: []

key-decisions:
  - "Split asset constants by usage site, not by type — FAVICON/MAIN_CSS/TAILWIND_CSS go to app.rs because App's rsx! references them; HEADER_SVG goes to hero.rs because Hero's rsx! references it. This keeps each module self-contained and avoids a globals module."
  - "Use the directory-module form for src/components/ (src/components/mod.rs) instead of the flat form (src/components.rs). The directory form scales: Phases 2–6 will add many more components alongside Hero."
  - "src/state.rs is a flat module file (not a directory) because Phase 4's data layer is currently scoped to a single module; promote to src/state/mod.rs only if it grows."
  - "src/platform/ uses the directory form (src/platform/mod.rs) from day one because Phase 6 will add per-platform submodules (web.rs, desktop.rs, mobile.rs) gated by #[cfg(feature = ...)]."
  - "Keep `use dioxus::prelude::*;` in src/main.rs verbatim per the plan's prescribed content — the plan and PATTERNS.md both specify it. This produces a benign unused-import warning until a future plan adds rsx! or other prelude items to main.rs (see Known Stubs below)."

patterns-established:
  - "Entry-point thinness: src/main.rs holds only mod declarations + the launch call. New components/state/platform code goes into the corresponding module files."
  - "Component re-export: components are private submodules of src/components/ and re-exported via pub use. Callers write use crate::components::ComponentName."
  - "Asset locality: const NAME: Asset = asset!(\"/assets/file.ext\"); is declared at the top of the module that uses it."
  - "Phase-stub annotation: empty placeholder modules carry a single comment line identifying the future phase that will populate them."

requirements-completed: [HYG-04]

# Metrics
duration: 11m 22s
completed: 2026-05-03
---

# Phase 01-hygiene Plan 02: Module hierarchy split — Summary

**Split the monolithic 38-line src/main.rs into a 6-file Rust module hierarchy (entry point + App module + components/ subtree + state and platform stubs), with `cargo build --features web` exiting 0 on the first compile attempt.**

## Performance

- **Duration:** 11m 22s
- **Started:** 2026-05-03T01:54:35Z
- **Completed:** 2026-05-03T02:05:57Z
- **Tasks:** 2
- **Files modified:** 6 (5 new module files + slimmed src/main.rs)

## Accomplishments

- **Module hierarchy in place.** `src/` now has 6 source files in 3 directories (`src/`, `src/components/`, `src/platform/`) instead of a single 38-line `main.rs`. Phases 2–6 can now add components, state primitives, and platform-specific code into the right module without growing `main.rs`.
- **Component split executed cleanly.** `App` moved from `main.rs` lines 12–20 into `src/app.rs` with `pub` visibility added; `Hero` moved from `main.rs` lines 22–38 into `src/components/hero.rs` (already `pub`); the `crate::components::Hero` import path is wired via the standard `pub use` re-export in `components/mod.rs`.
- **Asset constants relocated by usage site.** `FAVICON`, `MAIN_CSS`, `TAILWIND_CSS` now live next to `App` in `src/app.rs`. `HEADER_SVG` lives next to `Hero` in `src/components/hero.rs`. `src/main.rs` no longer carries any `Asset` declarations.
- **Build gate green on first attempt.** `cargo build --features web` exits 0 with one benign warning (unused `use dioxus::prelude::*;` in `main.rs`, see Known Stubs). No compiler errors. Dioxus 0.7's `#[component]` macro, asset pipeline, and `rsx!` all resolve correctly across the module split.
- **Layout-bug carry-over fixed.** The original `src/main.rs` had two `document::Link` elements crammed onto a single line in the `App` rsx! body (line 16: `document::Link { ... } document::Link { ... }`). The new `src/app.rs` puts each on its own line per the plan's prescribed content, matching standard Dioxus rsx! formatting.

## Task Commits

Each task was committed atomically on `main`:

1. **Task 1-04-01: Create new module files (5 files)** — `ac86e11` (chore)
2. **Task 1-04-02: Slim src/main.rs + verify compile** — `c0b0ad2` (refactor)

_Plan metadata commit will follow this SUMMARY._

## Files Created/Modified

- `src/main.rs` — Replaced the 38-line monolith with a 12-line entry point: `use dioxus::prelude::*;` + four `mod` declarations (`app`, `components`, `state`, `platform`) + `use app::App;` + `fn main()`. Both `#[component]` definitions and all four asset constants are gone — they moved to their new modules.
- `src/app.rs` — New file. Contains the `App` root component (now `pub`), the three asset constants `FAVICON` / `MAIN_CSS` / `TAILWIND_CSS`, and the `use crate::components::Hero;` import. The `rsx!` body matches `main.rs` lines 14–20 with the two-Link-on-one-line formatting bug fixed (each `document::Link` on its own line).
- `src/components/mod.rs` — New file. Two lines of substance: `mod hero;` (private submodule declaration) + `pub use hero::Hero;` (re-export so callers can `use crate::components::Hero;`).
- `src/components/hero.rs` — New file. Contains the `Hero` component (already `pub` in source) verbatim, plus the `HEADER_SVG` asset constant. The link list with the six Dioxus documentation URLs is preserved exactly — Phase 2 will replace this entirely with the IronHermes brand hero.
- `src/state.rs` — New file. One comment line: `// Phase placeholder — implementation begins in Phase 4 (Data Layer & Interactions).`
- `src/platform/mod.rs` — New file. Three comment lines naming Phase 6 as the populator and noting the `#[cfg(feature = ...)]` gating pattern that platform-specific submodules will use.

## Decisions Made

- **Asset constants split by usage site** (not centralized in a `constants` module). The `App` rsx! body references `FAVICON`/`MAIN_CSS`/`TAILWIND_CSS` so those declarations live in `app.rs`; the `Hero` rsx! body references `HEADER_SVG` so that declaration lives in `hero.rs`. Each module is self-contained — adding/removing assets touches only the module that uses them. The plan's `<action>` blocks specified this layout explicitly.
- **Directory module for `components/`, flat module for `state.rs`.** `src/components/mod.rs` (directory form) is correct because Phases 2–6 will add many sibling component files (status bar, command palette, agent panel, blocks, etc.). `src/state.rs` (flat form) is currently sufficient because Phase 4's data layer is one module's worth of code; if it grows it can be promoted to `src/state/mod.rs` non-breakingly.
- **Directory module for `platform/` from day one.** Even though Phase 6 hasn't started, `src/platform/mod.rs` (directory form) is in place because the multi-platform constraint guarantees per-platform submodules (`web.rs`, `desktop.rs`, `mobile.rs`) gated by `#[cfg(feature = ...)]`. Starting with the directory form avoids a later move.
- **`mod hero;` is private; `pub use hero::Hero;` is the only public surface.** Standard Rust re-export pattern: callers don't depend on `components::hero::Hero` (the file path) — they depend on `components::Hero` (the re-exported name). The `hero` submodule could be renamed without breaking callers.
- **Followed the plan's prescribed content verbatim, including the `use dioxus::prelude::*;` in slim `src/main.rs`.** The plan and PATTERNS.md both specify that import. It produces a benign unused-import warning today because the slim `main.rs` only calls fully-qualified `dioxus::launch(App)`. Removing it would be a deviation from the plan-prescribed content; logging it as a Known Stub is the correct path (see below).

## Deviations from Plan

None — plan executed exactly as written. Both task `<action>` blocks were applied character-for-character; all 14 acceptance criteria across the two tasks pass; the `cargo build --features web` verification succeeded on the first attempt with no compiler errors.

## Issues Encountered

- **`src/` was untracked at executor start.** The entire `src/` tree (including `main.rs`) was untracked in the working tree at this plan's start (it became tracked when this plan committed it). The plan anticipated this — Task 1's `<read_first>` instructed reading the current `src/main.rs` first, which contained the original prototype `App` + `Hero` components verbatim. Resolution: read the untracked source, confirmed it matched the plan's `<interfaces>` block exactly, then proceeded with the split. After Task 1's commit, the 5 new module files became tracked; after Task 2's commit, `src/main.rs` became tracked. All other untracked working-tree items (assets/, AGENTS.md, README.md, clippy.toml, tailwind.css, warp2ironhermes/, .omc/, Cargo.lock) remained out of scope and were not staged or committed.

## Known Stubs

| Stub | File | Line | Reason | Resolved by |
|------|------|------|--------|-------------|
| `use dioxus::prelude::*;` is unused in slim main.rs | `src/main.rs` | 1 | Plan and PATTERNS.md both specify this import verbatim, but the slim `main.rs` only calls fully-qualified `dioxus::launch(App)` and references no `dioxus::prelude` items, so rustc emits one `unused_imports` warning. Build still exits 0. | Phase 2 or later: when `main.rs` adds an `rsx!` macro call, router declaration, or any other `dioxus::prelude` item, the import becomes used and the warning disappears. Alternative: a follow-up plan can drop the unused import explicitly. |
| `state.rs` body is a single comment | `src/state.rs` | 1 | Phase 4 placeholder — Data Layer & Interactions has not started. | Phase 4 (Data Layer & Interactions) populates this module with signal types, mock-data tables, and the personality-preset registry. |
| `platform/mod.rs` body is three comments | `src/platform/mod.rs` | 1–3 | Phase 6 placeholder — Mobile Shell has not started. | Phase 6 (Mobile Shell) populates this module with `#[cfg(feature = "web")]` / `#[cfg(feature = "desktop")]` / `#[cfg(feature = "mobile")]`-gated submodules for platform-specific behavior. |
| `Hero` body is the dx-new scaffold link list | `src/components/hero.rs` | 8–18 | The original `src/main.rs` shipped with the default Dioxus scaffold's six documentation links. This plan preserved the body verbatim — visual replacement is out of scope for hygiene work. | Phase 2 (Design Tokens) replaces this body with the IronHermes brand hero (wordmark.svg, IH shield, monospace title bar) per `warp2ironhermes/project/Warp × IronHermes.html`. |

These stubs are intentional — they implement the plan's `<must_haves>` "exists as stubs for later phases" contract. None block Phase 1 completion.

## Verification Evidence

- **All 5 new files exist:**
  - `test -f src/app.rs` → exit 0 ✓
  - `test -f src/components/mod.rs` → exit 0 ✓
  - `test -f src/components/hero.rs` → exit 0 ✓
  - `test -f src/state.rs` → exit 0 ✓
  - `test -f src/platform/mod.rs` → exit 0 ✓
- **Content checks (Task 1):**
  - `grep -q 'pub fn App' src/app.rs` → exit 0 ✓
  - `grep -q 'use crate::components::Hero' src/app.rs` → exit 0 ✓
  - `grep -q 'pub use hero::Hero' src/components/mod.rs` → exit 0 ✓
  - `grep -q 'pub fn Hero' src/components/hero.rs` → exit 0 ✓
  - `grep -q 'HEADER_SVG' src/components/hero.rs` → exit 0 ✓
  - `grep -q 'Phase placeholder' src/state.rs` → exit 0 ✓
  - `grep -q 'Phase placeholder' src/platform/mod.rs` → exit 0 ✓
- **Content checks (Task 2 — slim main.rs):**
  - `grep -q 'mod app' src/main.rs` → exit 0 ✓
  - `grep -q 'mod components' src/main.rs` → exit 0 ✓
  - `grep -q 'mod state' src/main.rs` → exit 0 ✓
  - `grep -q 'mod platform' src/main.rs` → exit 0 ✓
  - `grep -q 'use app::App' src/main.rs` → exit 0 ✓
  - `grep -q 'fn main' src/main.rs` → exit 0 ✓
  - `grep -q 'fn App' src/main.rs` → exit 1 (App removed from main.rs) ✓
  - `grep -q 'fn Hero' src/main.rs` → exit 1 (Hero removed from main.rs) ✓
- **Compile gate:**
  - `cargo build --features web` → `Finished dev profile [unoptimized + debuginfo] target(s) in 0.57s`, exit 0 ✓
  - One benign warning: `unused import: dioxus::prelude::* in src/main.rs:1` (logged in Known Stubs)
  - Zero errors

## User Setup Required

None — no external service configuration introduced. All changes are local source reorganization.

## Next Phase Readiness

- **Plan 01-03 (HYG-02, Cargo.lock)** can proceed immediately on `main`. The module split is committed; running `cargo build --features web` on the new tree produces a `Cargo.lock` whose root dependency tree reflects the slimmed source. (Note: `Cargo.lock` is currently untracked in the working tree as a side-effect of the build run during this plan; Plan 01-03 will commit it.)
- **Phase 2 (Design Tokens)** can begin populating `src/components/` with brand-aligned components. The `pub use` re-export pattern in `components/mod.rs` is the established convention — new components add `mod x; pub use x::X;` lines.
- **Phase 4 (Data Layer)** has its `src/state.rs` placeholder ready; the file form can be promoted to `src/state/mod.rs` if the data layer outgrows a single module.
- **Phase 6 (Mobile Shell)** has its `src/platform/` directory ready for `#[cfg(feature = ...)]`-gated submodules.
- **No blockers** for Wave 3 of phase 01 (Plan 01-03).

## Self-Check: PASSED

Verified before final metadata commit:

- **Files claimed exist:**
  - `/Users/twilson/code/iron_hermes_ui/src/main.rs` ✓
  - `/Users/twilson/code/iron_hermes_ui/src/app.rs` ✓
  - `/Users/twilson/code/iron_hermes_ui/src/components/mod.rs` ✓
  - `/Users/twilson/code/iron_hermes_ui/src/components/hero.rs` ✓
  - `/Users/twilson/code/iron_hermes_ui/src/state.rs` ✓
  - `/Users/twilson/code/iron_hermes_ui/src/platform/mod.rs` ✓
  - `/Users/twilson/code/iron_hermes_ui/.planning/phases/01-hygiene/01-02-SUMMARY.md` ✓ (this file)
- **Commits claimed exist in git log:**
  - `ac86e11` (Task 1-04-01: chore — create module file scaffolding) ✓
  - `c0b0ad2` (Task 1-04-02: refactor — slim src/main.rs to entry point and module declarations) ✓

---
*Phase: 01-hygiene*
*Completed: 2026-05-03*
