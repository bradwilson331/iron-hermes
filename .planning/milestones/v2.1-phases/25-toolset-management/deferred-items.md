
## Plan 04 — pre-existing test failure (not caused by 25-04)

- **Test:** `commands::handlers::tests::dispatch_all_todo_stubs_return_not_yet_available`
- **Failure:** `Command 'cron' should return stub message, got: /cron: cron store not configured.`
- **Root cause:** `cron` was given a real handler (`cmd_cron`) by Phase 22.4.2.1-01, but its name was never removed from the `todo_commands` array in this test. Verified via `git stash` round-trip — same failure on the base commit `62be4f0`.
- **Disposition:** OUT OF SCOPE for Plan 04 (executor scope-boundary rule). Should be cleaned up in a future maintenance pass — either move `cron` out of the todo list, or change `cmd_cron` None-branch to emit "is not yet available".
