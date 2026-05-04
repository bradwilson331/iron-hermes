---
phase: 02-design-system
plan: 03
subsystem: ui
tags: [dioxus, brand-assets, svg, png, asset-pipeline]

# Dependency graph
requires:
  - phase: 01-hygiene
    provides: clean asset/ directory pipeline (Dioxus.toml tailwind wiring, asset!() pattern established)
provides:
  - assets/wordmark.svg (IronHermes wordmark, replaces scaffold header.svg in Hero stub)
  - assets/ih-shield.png (IronHermes shield, paired with wordmark in Hero stub)
  - File half of DS-04 (Rust-side asset!() declarations + Hero rsx! wiring + main.css rewrite live in 02-04)
affects: [02-design-system/02-04, 03-desktop-shell, 06-mobile-shell]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Brand assets in assets/ are byte-identical copies from warp2ironhermes/project/ironhermes/assets/ (single-binary-copy pattern, no transcoding)"

key-files:
  created:
    - "assets/wordmark.svg (321 B; SVG; byte-identical to prototype source)"
    - "assets/ih-shield.png (92,340 B; PNG, magic bytes 89504e470d0a1a0a; byte-identical to prototype source)"
  modified: []

key-decisions:
  - "Brand asset files copied byte-identical from prototype (no rename, no transcoding) so 02-04 asset!() constants resolve to the exact lowercase filenames wordmark.svg and ih-shield.png"
  - "scanner.svg intentionally NOT copied (deferred to Phase 3 per CONTEXT.md <deferred> and RESEARCH.md Open Question 1) — keeps Phase 2 scope tight to the four success criteria"
  - "assets/header.svg deletion deferred to merge time: file is untracked in git (never reached the repo) and does not exist in this worktree, so a `rm` here would be a no-op for git history. Plan 02-04 will rewrite hero.rs to remove HEADER_SVG, after which any working-directory copy of header.svg in the parent project becomes orphaned and trivially removable."

patterns-established:
  - "Single-binary-copy: brand assets land in assets/ as byte-identical clones of the prototype handoff sources; no per-file transcoding, no SVG minification, no PNG re-encoding"

requirements-completed: [DS-04]

# Metrics
duration: 1m 23s
completed: 2026-05-03
---

# Phase 02 Plan 03: Brand Assets Summary

**IronHermes wordmark SVG and shield PNG land byte-identical in `assets/`, ready for Plan 02-04's Rust-side `asset!()` wiring in the Hero stub.**

## Performance

- **Duration:** 1m 23s
- **Started:** 2026-05-03T06:03:32Z
- **Completed:** 2026-05-03T06:04:55Z
- **Tasks:** 1
- **Files modified:** 2 (both created; 0 modified-in-place)

## Accomplishments
- Vendored `wordmark.svg` (321 B) into `assets/` byte-identical to `warp2ironhermes/project/ironhermes/assets/wordmark.svg`
- Vendored `ih-shield.png` (92,340 B, valid PNG magic `89504e470d0a1a0a`) into `assets/` byte-identical to the prototype source
- Successfully excluded `scanner.svg` from the copy set (correctly deferred to Phase 3)
- All 12 plan acceptance criteria pass (existence × 2, byte identity × 2, non-empty × 2, format magic × 2, source preservation × 2, scanner-not-copied × 1, header-absent × 1)

## Task Commits

Each task was committed atomically:

1. **Task 1: Copy wordmark.svg and ih-shield.png; (no-op) delete header.svg** — `2f7cafa` (feat)

_Note: This plan has a single task per the plan file._

## Files Created/Modified
- `assets/wordmark.svg` — IronHermes wordmark, byte-identical to prototype source (321 B). Will be referenced by `WORDMARK_SVG: Asset = asset!("/assets/wordmark.svg")` in `src/components/hero.rs` in Plan 02-04.
- `assets/ih-shield.png` — IronHermes shield, byte-identical to prototype source (92,340 B). Will be referenced by `IH_SHIELD_PNG: Asset = asset!("/assets/ih-shield.png")` in `src/components/hero.rs` in Plan 02-04.

## Decisions Made
- **scanner.svg deferred to Phase 3** — followed CONTEXT.md `<deferred>` and RESEARCH.md Open Question 1 explicitly; the prototype directory contains three files (`ih-shield.png`, `scanner.svg`, `wordmark.svg`) but only the first and third are in scope.
- **No edits to `src/components/hero.rs`** — the `HEADER_SVG` constant there references a file that this plan would have removed in a single working tree, but in this worktree the file is absent (see Deviations). Plan 02-04 (Wave 2) is responsible for the Rust edit; Wave 1 plans never invoke `cargo build`, so the broken reference is benign until 02-04 lands.
- **`warp2ironhermes/` source tree untouched** — verified via post-copy `cmp` and `test -f` against both source files.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Plan source files don't materialize inside the git worktree**
- **Found during:** Task 1 (pre-execution sanity check)
- **Issue:** The plan instructs `cp warp2ironhermes/project/ironhermes/assets/wordmark.svg assets/wordmark.svg` from the project root. In this Wave 1 git worktree (`worktree-agent-a9c9fbb027d75153d`), neither `warp2ironhermes/` nor `assets/` materialize because both directories are untracked in git (`git ls-files` returns 0 matches for `^warp2ironhermes/` and 0 for `^assets/`). Worktrees only contain tracked files, so the byte-copy source path didn't exist in the worktree filesystem.
- **Fix:** Resolved the source path against the parent project's working directory (`/Users/twilson/code/iron_hermes_ui/warp2ironhermes/project/ironhermes/assets/...`) where the prototype handoff lives untracked, and created `assets/` in the worktree with `mkdir -p assets` before the copy. Byte identity verified post-copy with `cmp` (exit 0 on both files).
- **Files modified:** assets/wordmark.svg (created), assets/ih-shield.png (created)
- **Verification:** All 12 plan acceptance criteria pass — `cmp` returns exit 0 against both source files, PNG magic bytes match (`89504e470d0a1a0a`), SVG starts with `<?xml`, both files non-empty, sources unchanged, `scanner.svg` not present.
- **Committed in:** `2f7cafa` (Task 1 commit)

**2. [Rule 3 - Blocking] `assets/header.svg` deletion is a no-op in git for this worktree**
- **Found during:** Task 1
- **Issue:** Plan instructs `rm assets/header.svg`. In this worktree, `assets/header.svg` does not exist on disk and was never tracked by git (`git log --all -- 'assets/header.svg'` returns empty; `.gitignore` does not exclude `assets/`). The file exists only in the parent project's untracked working directory. `rm` would be a working-directory operation outside this worktree's commit, and there is nothing for git to record as a deletion.
- **Fix:** Skipped the `rm` step inside this worktree. The acceptance criterion `! test -f assets/header.svg` is naturally satisfied (file doesn't exist here). When this branch merges to main, the new `assets/wordmark.svg` and `assets/ih-shield.png` files will land alongside the parent's untracked `assets/header.svg`; that orphaned working-tree copy can be removed at merge time or by Plan 02-04 (which removes the `HEADER_SVG` Rust constant, making the file orphaned regardless).
- **Files modified:** none
- **Verification:** `! test -f assets/header.svg` exits 0; `git log --all -- 'assets/header.svg'` empty; `git ls-files | grep '^assets/'` empty (assets/ never tracked); commit 2f7cafa contains zero deletions.
- **Committed in:** N/A (no operation needed in this worktree's commit)

---

**Total deviations:** 2 auto-fixed (both Rule 3 — blocking issues from worktree/parent-tree mismatch)
**Impact on plan:** All four plan success criteria are met inside the worktree:
1. ✓ `assets/wordmark.svg` and `assets/ih-shield.png` exist and are byte-identical to their `warp2ironhermes/` sources.
2. ✓ `assets/header.svg` does not exist in this worktree (never reached git history; remains untracked in parent working directory).
3. ✓ `assets/scanner.svg` is NOT present (deferred to Phase 3).
4. ✓ `warp2ironhermes/` source tree is untouched.
Git diff shows two file additions and zero deletions in this worktree's commit (`2f7cafa`). The orchestrator should be aware that the parent working directory's `assets/header.svg` is an orphaned untracked file post-merge — it can be removed at merge time or as part of Plan 02-04 cleanup. No scope creep; no architectural deviation.

## Issues Encountered
- **Worktree/parent-tree mismatch:** initial `cp` from the plan's relative path failed because the worktree's filesystem is sparse (only git-tracked files materialize). Resolved by using absolute paths to the parent project's working directory for the source side of the copy. Documented as Rule 3 deviations above.

## User Setup Required

None — no external service configuration required.

## Verification Notes for Orchestrator

- **`cargo build` was intentionally NOT run** — would fail with `path must point to a file: /assets/header.svg` because `src/components/hero.rs` still references the now-deleted (or never-existed-in-worktree) `HEADER_SVG`. This is expected per the plan's `<verification>` section. The compile gate runs only inside Plan 02-04 after `hero.rs` is rewritten.
- **Post-merge note:** The parent project's working directory has an untracked `assets/header.svg` (23 KB) and an untracked `assets/main.css` that this plan does not touch. Plan 02-04 (Wave 2) is responsible for the Rust-side `HEADER_SVG` removal and the `main.css` rewrite; either of those lines of work, or merge-time cleanup, can dispose of the orphaned `header.svg` file in the working tree.
- **Other untracked items in this worktree:** `.omc/` (orchestrator scratch space) — out of scope, not committed.

## Next Phase Readiness

- Brand assets ready for Plan 02-04's `asset!()` declarations and Hero stub `<img>` wiring.
- Wave 1 (this plan + 02-01 fonts + 02-02 CSS tokens) provides all the static files Plan 02-04 needs to consume.
- No blockers for Plan 02-04.

## Self-Check: PASSED

**Files claimed to exist (verified via `test -f`):**
- ✓ FOUND: assets/wordmark.svg
- ✓ FOUND: assets/ih-shield.png
- ✓ FOUND: .planning/phases/02-design-system/02-03-SUMMARY.md (this file)

**Commits claimed to exist (verified via `git log --all | grep`):**
- ✓ FOUND: 2f7cafa (feat(02-03): land IronHermes brand assets)

**Acceptance criteria (all 12 from plan):**
- ✓ test -f assets/wordmark.svg
- ✓ test -f assets/ih-shield.png
- ✓ ! test -f assets/header.svg
- ✓ cmp assets/wordmark.svg <source> exit 0
- ✓ cmp assets/ih-shield.png <source> exit 0
- ✓ test -s assets/wordmark.svg (non-empty)
- ✓ test -s assets/ih-shield.png (non-empty)
- ✓ PNG magic bytes = 89504e470d0a1a0a
- ✓ SVG starts with `<?xml` or `<svg`
- ✓ source wordmark.svg still exists at warp2ironhermes/...
- ✓ source ih-shield.png still exists at warp2ironhermes/...
- ✓ ! test -f assets/scanner.svg

---
*Phase: 02-design-system*
*Completed: 2026-05-03*
