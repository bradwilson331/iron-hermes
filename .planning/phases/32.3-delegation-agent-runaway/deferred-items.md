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

## Pre-existing iron_hermes_ui test failure (HEAD as of 649f5007 / Plan 03 merge)

`cargo test -p iron_hermes_ui --features server --test server_runtime_parity`
fails on the `api_sessions_and_tools_are_backed_by_real_state` test:

```text
list_sessions must query StateStore for Platform::Web sessions
```

The test (server_runtime_parity.rs:28) requires `api.rs` to contain the
string `Platform::Web.to_string()`. The current `list_sessions` server fn
in `api.rs` passes `None` instead (per GAP-26.2.1-09 / D-26.2.1-13-A
USER-APPROVED 2026-05-14 — see api.rs:65-69) so the SESSIONS wedge sources
the full cross-platform on-disk catalog. The test assertion was not updated
to match the GAP-26.2.1-09 change.

Verified pre-existing by `git stash`ing Plan 04 edits + re-running the
suite — identical failure on the pre-edited base commit `649f5007`. Plan
04 does NOT introduce this failure.

**Scope:** Out of scope for Plan 04 (Plan 04 is about REST endpoints +
gateway confirm-token wiring; the failing test is testing api.rs `list_sessions`
arg, which Plan 04 does not touch). The new Plan 04 endpoints + tests pass
cleanly (7/7 in `plan_32_3_04_tests`).

**Action:** Update the test assertion to match the GAP-26.2.1-09 contract in
a follow-up doc-polish phase. Tracked here.
