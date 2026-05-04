---
phase: 01-hygiene
plan: 01
subsystem: infra
tags: [cargo, dioxus, dioxus-cli, tailwind, gitignore, build-config]

# Dependency graph
requires: []
provides:
  - Exact-version pin on the dioxus facade crate (dioxus = "=0.7.1") preventing silent 0.7.x patch upgrades
  - Tailwind watcher wiring in Dioxus.toml so `dx serve` auto-compiles tailwind.css → assets/tailwind.css
  - Recursive .DS_Store ignore (**/.DS_Store) and exclusion of warp2ironhermes-handoff.zip from VCS
affects: [01-02, 01-03, 02-design-tokens, all subsequent phases needing reproducible builds]

# Tech tracking
tech-stack:
  added: []  # No new crates; all changes are configuration
  patterns:
    - "Cargo feature indirection (web/desktop/mobile in [features], not on dep line)"
    - "Tailwind keys live under [application] in Dioxus.toml, not [web.app] or [web.resource]"
    - "Recursive gitignore globs use the **/ prefix"

key-files:
  created:
    - "Cargo.toml — first commit on this branch (was untracked previously); pins dioxus to =0.7.1"
    - "Dioxus.toml — first commit on this branch (was untracked previously); adds tailwind_input/output keys"
    - ".gitignore — first commit on this branch (was untracked previously); recursive DS_Store + handoff zip exclusion"
  modified: []

key-decisions:
  - "Pin dioxus to exact 0.7.1 (`=0.7.1`) to prevent silent 0.7.x patch upgrades — Dioxus 0.7.x is in active development with potential breaking patches"
  - "Keep features = [] on the dioxus dep line; rely on the [features] indirection table — dx CLI selects exactly one platform feature at build time, and listing all three on the dep line would cause the linker to reject simultaneous platform renderers"
  - "Place tailwind_input / tailwind_output under [application] (not [web.app] or [web.resource]) per Dioxus CLI schema.json — the dx CLI reads them from ApplicationConfig"
  - "Use **/.DS_Store recursive glob instead of root-only .DS_Store — macOS Finder writes DS_Store files into every folder it visits, including assets/, src/, warp2ironhermes/"

patterns-established:
  - "Cargo feature indirection: dependency declares `features = []`, [features] table maps web/desktop/mobile to dioxus sub-features"
  - "Dioxus.toml [application]: tailwind_input + tailwind_output configure the dx-managed Tailwind watcher"
  - "Recursive gitignore: use `**/.DS_Store` pattern for macOS noise files in any subdirectory"

requirements-completed: [HYG-01, HYG-03, HYG-05]

# Metrics
duration: 2m 24s
completed: 2026-05-02
---

# Phase 01-hygiene Plan 01: Pin dioxus, wire Tailwind, tighten gitignore — Summary

**Cargo.toml pins dioxus to exact 0.7.1, Dioxus.toml wires the dx Tailwind watcher under [application], and .gitignore now blocks .DS_Store recursively and excludes the 7.8 MB handoff zip.**

## Performance

- **Duration:** 2m 24s
- **Started:** 2026-05-03T01:47:06Z
- **Completed:** 2026-05-03T01:49:30Z
- **Tasks:** 3
- **Files modified:** 3 (Cargo.toml, Dioxus.toml, .gitignore — all first-time commits on this branch)

## Accomplishments

- **Reproducible builds:** Cargo.toml now pins the dioxus facade crate to exactly `=0.7.1`, preventing automatic upgrade to 0.7.2+ on `cargo update`. The build verification confirms `dioxus v0.7.1` is the version actually compiled.
- **Tailwind watcher live:** Dioxus.toml's `[application]` section now contains `tailwind_input = "tailwind.css"` and `tailwind_output = "assets/tailwind.css"`, enabling `dx serve` to auto-recompile Tailwind CSS without a separate watcher process.
- **VCS hygiene:** `.gitignore` blocks `.DS_Store` recursively (catches the file in every subdirectory, not only at the project root) and explicitly excludes `warp2ironhermes-handoff.zip` (7.8 MB design archive that is already extracted to `warp2ironhermes/`).
- **Build gate passed:** `cargo build --features web` exits 0; Cargo's resolver compiled `dioxus v0.7.1` cleanly along with the latest 0.7.x sub-crates (dioxus-web 0.7.7, etc., which are pinned by the facade crate's own dependency declarations).

## Task Commits

Each task was committed atomically on branch `worktree-agent-a680e5b1ef9d0f63d`:

1. **Task 1-01-01: Pin dioxus version in Cargo.toml (HYG-01)** — `ff82e58` (chore)
2. **Task 1-03-01: Wire Tailwind keys in Dioxus.toml (HYG-03)** — `7da3841` (chore)
3. **Task 1-05-01: Fix .gitignore — recursive DS_Store and handoff zip (HYG-05)** — `24a4e5e` (chore)

_Note: each file appears as `create mode 100644` in `git log` because these source files were previously untracked in the main repo's working tree. This worktree's branch brings them under version control for the first time. The orchestrator-merged result will replace the untracked files in main with these tracked, edited versions._

## Files Created/Modified

- `Cargo.toml` — Pinned dioxus dependency from `version = "0.7.1"` → `version = "=0.7.1"`. The `[features]` indirection table (`web = ["dioxus/web"]`, `desktop = ["dioxus/desktop"]`, `mobile = ["dioxus/mobile"]`) is intact and unchanged.
- `Dioxus.toml` — Added two keys under the existing `[application]` section: `tailwind_input = "tailwind.css"` and `tailwind_output = "assets/tailwind.css"`. The `[web.app]`, `[web.resource]`, and `[web.resource.dev]` sections are untouched.
- `.gitignore` — Replaced root-only `.DS_Store` with recursive `**/.DS_Store`; appended `warp2ironhermes-handoff.zip` as a final entry. The `/target` and `**/*.rs.bk` rules and the surrounding comments are unchanged.

## Decisions Made

- **Pin the facade crate, not the sub-crates.** The `=0.7.1` pin only constrains the top-level `dioxus` crate. Cargo resolved `dioxus-web`, `dioxus-core`, and the rest of the 0.7.x sub-crates to the latest patch (0.7.7) because the facade crate's own dependency declarations allow that range. This is the standard Dioxus pinning strategy and is the behaviour the plan's HYG-01 requirement targets — broader-graph pinning is a Cargo.lock concern, owned by Plan 01-03.
- **Keep `features = []` on the dioxus dep line.** Adding `features = ["web", "desktop", "mobile"]` directly to the dep would force all three platform renderers into a single compilation unit, which the linker rejects. The `[features]` indirection table already routes the correct sub-feature when `dx` selects a platform at build time.
- **Place Tailwind keys under `[application]`.** The Dioxus CLI reads `tailwind_input` / `tailwind_output` from `ApplicationConfig`, not from the web-specific tables. Placing them under `[web.app]` or `[web.resource]` would silently disable the watcher (Pitfall 2 in 01-RESEARCH.md).
- **No source-tree edits in this plan.** `src/main.rs`, `assets/`, `tailwind.css`, and `clippy.toml` were intentionally excluded from this plan's commits, even though the worktree had to materialize them transiently to run `cargo build --features web`. Those files belong to plans 01-02 (HYG-04 module split) and 01-03 (HYG-02 Cargo.lock).

## Deviations from Plan

None — plan executed exactly as written. All three task edits matched the plan's `<action>` blocks character-for-character; all acceptance criteria pass on the first attempt; the build verification succeeded on the first attempt.

## Issues Encountered

- **Worktree did not contain source files at startup.** Phase 1's working files (Cargo.toml, Dioxus.toml, .gitignore, src/, assets/, etc.) were untracked in the main repo and therefore absent from the worktree's checkout. The plan was authored against absolute paths in the main repo where those untracked files exist. Resolution: copied the three plan-scoped files into the worktree before editing (Cargo.toml, Dioxus.toml, .gitignore) so the edits could be committed on this branch. For the `cargo build --features web` verification step (which requires `src/main.rs`, `assets/`, `tailwind.css`, `clippy.toml` to also exist), materialized a temporary directory under `$TMPDIR` that combined this worktree's edited config files with the main repo's untracked source tree, ran the build there, and removed the temp dir afterward. None of the source files were committed to this branch — they remain owned by plans 01-02 and 01-03.

## Verification Evidence

- `grep 'version = "=0.7.1"' Cargo.toml` → `dioxus = { version = "=0.7.1", features = [] }` ✓
- `grep '^version = "0.7.1"' Cargo.toml` → no match (root-anchored unpinned form gone) ✓
- `[features]` table: `default = ["web"]`, `web = ["dioxus/web"]`, `desktop = ["dioxus/desktop"]`, `mobile = ["dioxus/mobile"]` — all four lines present and unchanged ✓
- `grep -c 'tailwind_input = "tailwind.css"' Dioxus.toml` → 1 ✓
- `grep -c 'tailwind_output = "assets/tailwind.css"' Dioxus.toml` → 1 ✓
- Both Tailwind keys placed before `[web.app]` (lines 2 and 3, immediately under `[application]`) ✓
- `grep -c '^\*\*/\.DS_Store$' .gitignore` → 1 ✓
- `grep -c '^\.DS_Store$' .gitignore` → 0 (root-only pattern gone) ✓
- `grep -c '^warp2ironhermes-handoff\.zip$' .gitignore` → 1 ✓
- `cargo build --features web` → `Finished dev profile [unoptimized + debuginfo] target(s) in 25.74s`, exit code 0; resolver compiled `dioxus v0.7.1` (confirming the pin is honoured) ✓

## User Setup Required

None — no external service configuration introduced. All changes are local build configuration and VCS metadata.

## Next Phase Readiness

- **Plan 01-02 (HYG-04, module split)** can proceed against the pinned dioxus version. The `=0.7.1` pin guarantees the API surface seen by 01-02's new modules will not shift between waves.
- **Plan 01-03 (HYG-02, Cargo.lock)** can now generate `Cargo.lock` from the corrected `Cargo.toml` — running `cargo build` produces a lockfile whose `dioxus` entry pins exactly `0.7.1` (verified above as a side-effect of the temp-dir build, though that lockfile lived only in the temp dir and was deleted).
- **No blockers** for Wave 2 / Wave 3 of phase 01.

## Self-Check: PASSED

Verified before final metadata commit:

- **Files claimed exist:**
  - `Cargo.toml` ✓
  - `Dioxus.toml` ✓
  - `.gitignore` ✓
  - `.planning/phases/01-hygiene/01-01-SUMMARY.md` ✓
- **Commits claimed exist in git log:**
  - `ff82e58` (Task 1-01-01) ✓
  - `7da3841` (Task 1-03-01) ✓
  - `24a4e5e` (Task 1-05-01) ✓

---
*Phase: 01-hygiene*
*Completed: 2026-05-02*
