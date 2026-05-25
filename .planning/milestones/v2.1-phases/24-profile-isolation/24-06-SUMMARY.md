---
phase: 24
plan: "06"
subsystem: profile-isolation
tags: [phase-24, profile-isolation, first-use, auto-scaffold, wizard-launch, d-06]
dependency_graph:
  requires: ["24-03"]
  provides: ["D-06 first-use loop closed", "Phase 24 preflight gate comment lock"]
  affects: ["crates/ironhermes-cli/src/main.rs", "crates/ironhermes-cli/tests/profile_first_use.rs"]
tech_stack:
  added: []
  patterns: ["Phase 23 apply_minimum_viable_answers seam reuse", "OnceLock<Mutex<()>> env_lock pattern"]
key_files:
  created: ["crates/ironhermes-cli/tests/profile_first_use.rs"]
  modified: ["crates/ironhermes-cli/src/main.rs"]
decisions:
  - "D-06: first-use trigger is implicit (missing config.yaml) — no new gate, no new flag"
  - "Phase 23 preflight gate condition byte-for-byte preserved; || cli.profile.is_some() not introduced"
  - "apply_minimum_viable_answers seam reused (Phase 23 testability pattern) — no binary spawn needed"
metrics:
  duration: "~10 min"
  completed: "2026-04-29"
  tasks: 2
  files: 2
---

# Phase 24 Plan 06: First-Use Profile Scaffold + Wizard Launch Summary

**One-liner:** Phase 24 D-06 first-use loop closed via implicit config.yaml trigger — Phase 23 preflight gate condition byte-for-byte preserved, no new gate widening introduced.

## What Was Built

Plan 06 closes the D-06 first-use loop: when `hermes --profile NEW chat` runs and `<profile_path>/config.yaml` does not yet exist, the existing Phase 23 preflight machinery automatically launches the setup wizard for that profile. Plan 24 adds NO new logic — the trigger is entirely implicit:

1. Plan 03 (already done) pivots `IRONHERMES_HOME` to the profile-scoped path
2. `ensure_home_dirs()` (already done) scaffolds the 8-subdir tree at that path
3. `config.yaml` does not exist for a brand-new profile
4. `preflight::run_preflight_check` sees missing `config.yaml` and fires the wizard
5. After wizard completes, dispatch proceeds to the originally-requested subcommand

**Task 1:** Added an explanatory comment block immediately above the Phase 23 preflight gate in `main.rs` that locks the Phase 24 D-06 contract for future refactor reviewers. No functional changes — comment only.

**Task 2:** Created `tests/profile_first_use.rs` with two integration tests:
- `first_use_scaffolds_and_runs_wizard`: simulates first `hermes --profile testfoo chat` against a brand-new tempdir profile path, scaffolds 8-subdir tree, drives `apply_minimum_viable_answers` seam, asserts `config.yaml` is written with seeded provider + model
- `bare_hermes_first_use_works_unchanged`: asserts bare `hermes` (no `--profile`) does NOT auto-create a `profiles/` subdirectory — locks D-05 zero-migration contract

## Preflight Gate Verification

- `grep -c 'matches!(cli.command, Some(Commands::Chat'` → **2** (gate condition preserved)
- `grep -c '&& cli.execute.is_none()'` → **3** (gate clause 2 preserved)
- `grep -c '|| cli.profile.is_some()'` → **0** (no widening introduced)
- `grep -c 'Phase 24 D-06 first-use contract'` → **1** (explanatory comment present)
- `git diff` on `main.rs` for Task 1: comment-only addition, zero functional lines changed
- Gate location confirmed at lines 252-256 (post-Plan-05 line numbers), matching 23-VERIFICATION.md lock

## Ordering Invariant Verification

`awk '/^async fn main/,/^}$/' main.rs | grep -nE '(resolve_and_set_profile|ensure_home_dirs|run_preflight_check)'` output:
- `resolve_and_set_profile` at relative line 14
- `ensure_home_dirs` at relative line 36
- `run_preflight_check` at relative line 64

Strictly ascending — pivot < scaffold < preflight gate. Contract intact.

## apply_minimum_viable_answers Visibility

`ironhermes_cli::setup::apply_minimum_viable_answers` is `pub fn` in `pub mod setup` (lib.rs:12). Accessible from integration test crate without any visibility adjustment.

## Test Results

```
running 2 tests
test bare_hermes_first_use_works_unchanged ... ok
test first_use_scaffolds_and_runs_wizard ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## Deviations from Plan

None — plan executed exactly as written.

The plan's test template was used verbatim. The `ensure_home_dirs()` function lives in `main.rs` (binary, not lib), so the test creates the 8-subdir tree directly — this is explicitly documented in the plan's template comment and matches the "no-binary-spawn" testability pattern.

## Known Stubs

None. Both tests exercise real library surfaces (not mocks), write real files to tempdir, and assert real file presence and content.

## Threat Flags

No new network endpoints, auth paths, file access patterns, or schema changes introduced. The comment addition is documentation-only. The test file accesses only tempdir paths under env_lock protection. No new threat surface.

## Self-Check: PASSED

- FOUND: crates/ironhermes-cli/src/main.rs
- FOUND: crates/ironhermes-cli/tests/profile_first_use.rs
- FOUND: commit af14dc5 (docs(24-06): add Phase 24 D-06 first-use contract comment)
- FOUND: commit 2097f43 (feat(24-06): add profile first-use scaffold + wizard integration tests)
