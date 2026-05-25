# Phase 32.3 Deferred Items

## ~~Pre-existing build failure in `ironhermes-cli` (HEAD as of 3dacf68e)~~ — RESOLVED

Originally logged: `cargo build -p ironhermes-cli` failed with 11 errors that
pre-dated Plan 01:

- `error[E0609]: no field 'subagent' on type 'ironhermes_core::Config'` — the
  field was renamed to `delegation` in Phase 32.2 D-07.
- `error[E0599]: no method named 'with_max_subagents' found for struct 'CommandContext'`.

**Status (post-phase-32.3 close):** Both errors are no longer reproducible.
`cargo build --workspace` and `cargo test -p ironhermes-cli --no-run` both
exit 0 at HEAD `d4a36010`. The only residual references to the old `config.subagent`
path were two stale doc comments in `tui/status_line.rs:36` and
`tui_rata/status_line.rs:40` — both were rewritten in the post-phase cleanup
commit to point at the new `config.delegation.max_concurrent_children` path.
The local struct field name `max_subagents` is intentionally preserved for
identifier stability.

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
