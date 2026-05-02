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
