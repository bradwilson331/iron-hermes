# Deferred Items — 260602-nd7

Out-of-scope discoveries found while executing the U9 drawer-comments
auto-refresh fix. Per the executor scope-boundary rule, these are NOT
fixed here — they are logged for follow-up because they exist on the
base commit `d2e51d52` (the orchestrator-mandated worktree base) and
are not caused by this plan's changes.

---

## DEFER-1: `ironhermes-kanban` `end_to_end.rs` — 2 tests fail at base

**Repro (run on base d2e51d52, no changes from this plan):**

```bash
cd /Users/twilson/code/ironhermes/.claude/worktrees/agent-a030e685e762f4d4b
git checkout HEAD -- crates/ironhermes-kanban/src/store.rs  # back out the nd7 fix
cargo test -p ironhermes-kanban --test end_to_end -- --test-threads=1
```

**Failing tests:**
- `full_lifecycle_via_tools_layer` — asserts `task.status == "done"` but observes `"ready"` after dispatcher tick.
- `duplicate_completion_is_rejected` — asserts first `KanbanCompleteTool::execute` returns Ok; instead returns `task not found: t_<id>`.

**Verification this is pre-existing:**

Confirmed by checking out store.rs at HEAD (the d2e51d52 base) BEFORE applying the nd7 fix:
the same two failures occur identically. Neither test calls `add_comment` (verified by
`grep -n add_comment crates/ironhermes-kanban/tests/end_to_end.rs` → no matches). The nd7
fix does not modify the dispatcher tick, `KanbanCompleteTool`, `task_runs`, or any
other code path these tests exercise.

**Likely root cause:** env-var race or dispatcher behavioral change unrelated to nd7.
The test file's own docstring (lines 18-25) flags an env-var race with
`HERMES_KANBAN_TASK` / `HERMES_KANBAN_RUN_ID` / `HERMES_KANBAN_CLAIM_LOCK` /
`HERMES_PROFILE` and recommends `--test-threads=1`. Single-threaded still fails, so
the cause may be deeper — possibly related to commit `ed1aee3c fix(36.3.7.10): serialize
HERMES_KANBAN_TASK env access across all 13 tool tests` having an incomplete fix.

**Suggested follow-up:** spawn a `/gsd-debug` quick task on
`ironhermes-kanban/tests/end_to_end.rs` to investigate. Likely targets:
the `claim_lock`/`current_run_id` round-trip, or a regression in
`run_dispatch_tick`'s spawn_fn awaiting.

---

## DEFER-2: Rust 1.94 clippy lint upgrade — 37+ errors across workspace

**Repro:**

```bash
cargo clippy --workspace --features iron_hermes_ui/server -- -D warnings 2>&1 | grep -E "^error" | head -30
```

**Failing lints (selection):**

1. `clippy::collapsible_if` — `crates/ironhermes-core/src/skills.rs:571-578`
   - Author: Brad Wilson, commit `0db139084` (2026-05-09 — 24 days before today)
   - 16 total errors in `ironhermes-core` (lib)
2. `clippy::items_after_test_module` — `crates/ironhermes-kanban/src/tools/mod.rs:89`
   - Author: Brad Wilson, commit `9cc4114d8` (2026-05-29 — 4 days before today)
   - 21 total errors in `ironhermes-kanban` (lib test)

**Verification this is pre-existing:**

`git blame` on each cited line shows the lint targets predate the nd7 base
commit `d2e51d52`. Rust 1.94 promoted these to active lints; with `-D warnings`
they become errors. The nd7 plan does NOT change any of the cited files.

To confirm the nd7 code passes clippy in isolation:
```bash
cargo clippy -p ironhermes-kanban --tests --no-deps -- -D warnings  # nd7 fix + new test
cargo clippy -p iron_hermes_ui --features server --test kanban_drawer --no-deps -- -D warnings  # nd7 consumer test
```
The first passes once the pre-existing `items_after_test_module` lints in
`tools/mod.rs` are silenced (none of those errors point to lines added by
this plan). The second exits clean with no errors on the kanban_drawer test
file (only 4 warnings in transitive dep `ironhermes-gateway`).

**Suggested follow-up:** spawn a `/gsd-quick` to apply the auto-fix suggestions
across the workspace:
```bash
cargo clippy --workspace --fix --allow-dirty --features iron_hermes_ui/server -- -D warnings
```
The `collapsible_if` and `items_after_test_module` lints have automatic
suggestions that should be safe to apply mechanically.

---

## DEFER-3: `ironhermes-cli` `setup.rs` test — env-var race

**Repro (after running other workspace tests):**

```bash
cargo test --workspace 2>&1 | grep -B1 FAILED
# → setup::tests::backfill_uses_process_env_when_dotenv_absent panics:
#   "backfill must write providers.openrouter.api_key_env when env var is in process env"
#   left: None / right: Some("OPENROUTER_API_KEY")
```

**Failing test:** `crates/ironhermes-cli/src/setup.rs:1521::tests::backfill_uses_process_env_when_dotenv_absent`

**Likely root cause:** sibling tests in the same binary mutate `OPENROUTER_API_KEY`
via `std::env::remove_var` and the test runner's parallel execution observes the
unset state. The nd7 plan does NOT touch any env-var handling.

**Suggested follow-up:** add `OPENROUTER_API_KEY` to the same env-lock mutex that
`HERMES_KANBAN_TASK` uses per commit `ed1aee3c`, or annotate the test with
`#[serial]` if the `serial_test` crate is in use elsewhere in the file.

---

## Scope Rationale

Per `executor` workflow's deviation rules (Rule scope boundary):

> Only auto-fix issues DIRECTLY caused by the current task's changes. Pre-existing
> warnings, linting errors, or failures in unrelated files are out of scope.
> - Log out-of-scope discoveries to `deferred-items.md` in the phase directory
> - Do NOT fix them

All three deferrals satisfy: (a) lines blamed to commits older than this plan,
(b) files not in this plan's `files_modified` list, (c) failures reproducible
on the unmodified `d2e51d52` base before any nd7 changes are applied.
