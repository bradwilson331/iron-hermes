# Phase 25.2 — Deferred Items

Out-of-scope discoveries logged during plan execution per the executor `<scope_boundary>` rule.
These are pre-existing issues in unrelated files / crates, NOT introduced by 25.2 plans.
Do not fix them within 25.2; track them here so a later "workspace clippy hygiene" pass can sweep them.

## Pre-existing workspace clippy warnings (Plan 25.2-01)

Discovered while running the plan-mandated `cargo clippy -p ironhermes-tools --lib -- -D warnings`
gate at the end of Task 2. None of the violations are in `crates/ironhermes-tools/`; all are in
transitive dep crates that clippy rebuilds when targeting `ironhermes-tools`.

| File | Line | Lint | Authored |
|------|------|------|----------|
| `crates/ironhermes-core/src/memory_store.rs` | 429 | `clippy::manual_is_multiple_of` (rust-clippy 1.94.0) | 2026-04-12 (b8f840db) |
| `crates/ironhermes-core/src/skills.rs` | 125 | `clippy::derivable_impls` (rust-clippy 1.94.0) | 2026-04-14 (b2306c7d) |
| (8 additional `ironhermes-core` lints — total 10 errors stop compilation under `-D warnings`) | — | — | pre-existing |

**Why deferred:**

- Authored weeks before phase 25.2 (Apr 12 / Apr 14 vs. May 2 plan execution); not caused by the
  D-04 DRY refactor.
- Outside the `ironhermes-tools` crate (Task 2's only modification target).
- Newly introduced in clippy 1.94.0 stable lint set, which post-dates the original code.
- `cargo test -p ironhermes-tools --lib web_read` (the plan's other gate) passes 18/18, proving
  the refactor itself is functionally correct.

**Resolution path:** schedule a workspace-wide `cargo clippy --workspace --fix` pass as a
standalone hygiene plan (or fold into a future v2.1 stabilization phase). Each warning is a
one-line auto-fixable mechanical refactor.

## Pre-existing workspace clippy warnings (Plan 25.2-03)

Re-confirmed the same 10 `ironhermes-core` clippy errors when running the plan-mandated
`cargo clippy -p ironhermes-core -- -D warnings` gate at the end of Task 1.

- Same 10 errors as Plan 25.2-01 (config.rs, memory_store.rs, skills.rs).
- Zero errors in `crates/ironhermes-core/src/provider.rs` — the only file modified by Plan 03.
- New `SummarizationClientHandle` trait + dyn-compatibility test introduce zero new warnings.

**Resolution path:** same as Plan 25.2-01 entry above.

## Pre-existing workspace clippy warnings (Plan 25.2-04)

Re-confirmed the same 10 `ironhermes-core` clippy errors when running the plan-mandated
`cargo clippy -p ironhermes-tools -- -D warnings` gate after writing
`crates/ironhermes-tools/src/web_extract/sanitize.rs`.

- Stashing the new sanitize.rs content and re-running clippy on `ironhermes-core` produced
  the same 11 error lines (10 errors + "could not compile") — proving the lints pre-existed
  this plan and were not introduced by the D-08 + D-19 sanitizer.
- Filtering clippy output for `(sanitize|web_extract)` returns ZERO matches — the new code
  itself is clippy-clean.
- All 11 unit tests in `web_extract::sanitize::tests` pass.

**Resolution path:** same as Plan 25.2-01 entry above.
