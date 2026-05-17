# Phase 32.3 Deferred Items

## Pre-existing build failure in `ironhermes-cli` (HEAD as of 3dacf68e)

`cargo build -p ironhermes-cli` fails with 11 errors that PRE-DATE Plan 01:

- `error[E0609]: no field 'subagent' on type 'ironhermes_core::Config'` — the
  field was renamed to `delegation` in Phase 32.2 D-07. The CLI still references
  the old name in ~10 call sites.
- `error[E0599]: no method named 'with_max_subagents' found for struct 'CommandContext'`
  — method was removed/renamed but a residual call site remains.

Verified pre-existing by `git stash`ing all Plan 01 edits, building, and
seeing identical errors. Plan 01 does NOT introduce these errors.

**Scope:** Out of scope for Plan 01 (this plan is about RAII registration
guards; the CLI errors are a separate Phase 32.2 follow-up).

**Action:** Track here; surface in Phase 32.3 SUMMARY.md "Deferred Issues".
The CLI test target this plan touched (`agents_list_live_mid_turn.rs`) is a
test-only file; the bin target's compile failure does not block this plan's
test verification because lib + integration-test artifacts compile clean.

## Excluded crate `ironagent-tools-api/tests/delegate_task_timeout_cancel.rs`

Uses outdated `SubagentConfig` field names (`timeout_secs`, `max_subagents`)
that were renamed in Phase 32.2. The crate is `exclude`d from the workspace
(see root `Cargo.toml`), so this file is not compiled. Left untouched.
