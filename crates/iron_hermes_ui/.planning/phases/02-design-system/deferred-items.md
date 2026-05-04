# Phase 02 — Deferred Items

Issues discovered out-of-scope of plan execution; flagged for orchestrator/future plan attention.

## DEFERRED-02-01-A: Worktree base lacks tracked `assets/` directory

**Discovered during:** Plan 02-01 execution (post-Task-2 cargo build verification)
**Source:** `cargo build --features web` from worktree base commit `9cd3e80`
**Symptom:** `cargo build --features web` fails with 4 "Asset at /assets/X doesn't exist" errors:

- `/assets/favicon.ico` (referenced by `src/app.rs:4`)
- `/assets/main.css` (referenced by `src/app.rs:5`)
- `/assets/tailwind.css` (referenced by `src/app.rs:6`)
- `/assets/header.svg` (referenced by `src/components/hero.rs:3`)

**Root cause (analysis):** The worktree base commit `9cd3e80` contains zero files under `assets/` (`git ls-tree -r --name-only 9cd3e80 | grep '^assets/'` is empty), yet `src/app.rs` and `src/components/hero.rs` already reference these assets via `asset!()`. The Phase 1 deliverables that add these files appear to live outside the committed tree (likely build outputs or untracked workspace files in the main checkout).

**Why deferred (out of scope):**
- Plan 02-01 only adds new files (`assets/fonts/*.woff2` and `assets/design-tokens.css`); it does not modify any of the four missing-asset call sites.
- The cargo build failure is the baseline state of the worktree base commit — it predates this plan's changes.
- Per `<deviation_rules>` SCOPE BOUNDARY: only auto-fix issues directly caused by the current task's changes.
- The plan's `<verification>` cargo build step assumes the baseline is green, which is not true in this worktree. Surfacing here so the orchestrator (or plan 02-04, which rewrites `src/app.rs` and `src/components/hero.rs`) can resolve.

**Plan-level smoke checks (all pass — see SUMMARY):**
- `ls -1 assets/fonts/IoskeleyMono-*.woff2 | wc -l` → 16
- `grep -c '@font-face' assets/design-tokens.css` → 16
- `tail -n +5 assets/design-tokens.css | cmp - warp2ironhermes/project/ironhermes/colors_and_type.css` → exit 0
- `grep -c '^@import' assets/design-tokens.css` → 0
- `grep -c 'url("fonts/IoskeleyMono-' assets/design-tokens.css` → 16

**Suggested resolution:** Either the worktree-mode setup needs to include the existing `assets/` files from the main checkout, or plans 02-02/02-03/02-04 (which add `wordmark.svg`, `ih-shield.png`, `warp-ih.css`, and rewrite `main.css` + the source files referencing these assets) will collectively resolve the missing-asset errors when their work merges back. No action required from plan 02-01.
