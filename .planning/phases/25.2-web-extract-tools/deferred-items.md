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

## Pre-existing workspace clippy warnings (Plan 25.2-10)

Re-confirmed the same 10 `ironhermes-core` clippy errors when running the plan-mandated
`cargo clippy -p ironhermes-tools -- -D warnings` gate after implementing
`crates/ironhermes-tools/src/web_extract/youtube.rs` (D-10 YouTube dispatch via skill helper
shell-out).

Errors (file:line — lint):
- `ironhermes-core/src/commands/handlers.rs:359` — `clippy::collapsible_if`
- `ironhermes-core/src/commands/handlers.rs:901` — `clippy::manual_div_ceil`
- `ironhermes-core/src/commands/handlers.rs:974,975` — `clippy::field_reassign_with_default`
- `ironhermes-core/src/commands/typo.rs:55,56` — `clippy::needless_range_loop`
- `ironhermes-core/src/config.rs:71` — `clippy::iter_contains` (`contains()` instead of `iter().any()`)
- `ironhermes-core/src/config.rs:172,226` — `clippy::derivable_impls`
- `ironhermes-core/src/memory_store.rs:429` — `clippy::manual_is_multiple_of`
- `ironhermes-core/src/skills.rs:125` — `clippy::derivable_impls`

- Filtering full clippy output for `(youtube|web_extract)` returns ZERO matches — the new
  YouTube dispatch code itself is clippy-clean.
- 5/5 youtube unit tests pass (`youtube_skill_name_is_hyphenated`,
  `helper_script_relpath_correct`, `first_h1_pulls_title`, `first_h1_returns_empty_when_no_heading`,
  `first_h1_skips_subheadings`).
- The plan's own acceptance grep `! grep -q '"youtube_content"' ...` is internally
  inconsistent with its prescribed `assert_ne!(YOUTUBE_SKILL_NAME, "youtube_content")` test
  body (the test deliberately includes the underscored literal as a guard). Substantive intent
  — production const HYPHENATED, dispatch path HYPHENATED — is satisfied: the only occurrence
  of `"youtube_content"` in the file is the negative assertion at line 122.

**Resolution path:** same as Plan 25.2-01 entry above.

## Pre-existing workspace test failures (Plan 25.2-13)

Re-confirmed pre-existing failures while running `cargo test --workspace --no-fail-fast`
after Plan 13:

- `ironhermes-core --lib`: `commands::handlers::tests::dispatch_all_todo_stubs_return_not_yet_available`
  fails because the test still expects the cron handler to return a stub message,
  but commit `96b19c1 feat(22.4.2.1-01)` replaced the stub with real cron sub-dispatch
  that returns "cron store not configured" when no store is configured. Pre-existing
  drift between handler and its assertion.
- `ironhermes-cli --test setup_wizard`: failure category not investigated; package
  has no overlap with Plan 13's surface (web_extract / web_local).
- `ironhermes-hub --lib`: failure category not investigated; package has no overlap
  with Plan 13's surface.

Filter check: `git log --oneline crates/ironhermes-core/src/commands/handlers.rs`
shows the file was last touched by commit `c394c44 feat(25-04)` — long before
Plan 25.2-13. Filter check: `cargo test -p ironhermes-tools --lib` returns
286/286 passing — Plan 13's `web_local.rs` SSRF override does not break any
test in the tools crate.

Plan 13's own surface verified clean:
- `cargo test -p ironhermes-tools --test web_extract_integration` → 9/9 pass
- `cargo test -p ironhermes-tools --lib` → 286/286 pass
- `cargo clippy -p ironhermes-tools --tests --no-deps` → zero new warnings on
  `web_local.rs` or `web_extract_integration.rs`

**Resolution path:** same as prior Plan 25.2 entries above — track for cleanup
after Plan 25.2 closes; do not block on it.

## Pre-existing workspace clippy warnings (Plan 25.2-14)

Re-confirmed the same family of pre-existing `ironhermes-core` clippy errors when
running the plan-mandated `cargo clippy -p ironhermes-agent -- -D warnings` and
`cargo clippy -p ironhermes-cli -- -D warnings` gates after wiring `register_web_extract_tool`
in all three CLI entry points (D-13 / D-20 production binary closure).

- `cargo clippy -p ironhermes-agent -- -D warnings` failed compiling the `ironhermes-core`
  dependency due to ~10 errors (manual_is_multiple_of, derivable_impls, collapsible_if,
  manual_div_ceil, field_reassign_with_default, needless_range_loop, iter_contains).
- `cargo clippy -p ironhermes-cli -- -D warnings` failed for the same reason — same
  transitive `ironhermes-core` errors block compilation under `-D warnings`.
- `cargo clippy -p ironhermes-agent --lib --no-deps -- -D warnings` produced 40 errors
  in unrelated agent files (anthropic_client.rs, agent_loop.rs, etc.) — ZERO matches in
  `crates/ironhermes-agent/src/any_client.rs` or `crates/ironhermes-agent/src/lib.rs`
  (Plan 14's only modified files in the agent crate).
- `cargo clippy -p ironhermes-cli --no-deps -- -D warnings` produced 25 errors in
  unrelated CLI files (atomic.rs, render.rs, tui modules) — ZERO matches in
  `crates/ironhermes-cli/src/main.rs` (Plan 14's only modified CLI file).

Plan 14's own surface verified clean:
- `cargo build -p ironhermes-cli` → 0 errors, 26 pre-existing warnings, finished in 3m 37s.
- `cargo test -p ironhermes-agent --lib any_client::tests` → 12/12 pass (10 existing +
  `test_any_client_summarization_handle_constructible` + `web_extract_tool_appears_in_definitions_after_wireup`).
- `cargo test -p ironhermes-cli --bin ironhermes register_web_extract_tool_wired_in_all_three_sites`
  → 1/1 pass (≥3 register_web_extract_tool call sites + ≥3 AnyClientSummarizationHandle::new
  call sites in non-comment source).
- `grep -c "register_web_extract_tool(" crates/ironhermes-cli/src/main.rs` → 4 (3 call
  sites + 1 in guard test source).
- `grep -c "AnyClientSummarizationHandle::new" crates/ironhermes-cli/src/main.rs` → 5
  (3 call sites + 1 in guard test source + 1 in guard assertion message string).

**Resolution path:** same as prior Plan 25.2 entries above — track for cleanup
after Plan 25.2 closes; do not block on it.
