
## Plan 19-03 Deferred Items

### test_delegate_task_schema_has_required_task (pre-existing failure)
- **Location:** crates/ironhermes-tools/src/delegate_task.rs:757
- **Failure:** `schema must have 'task' as required`
- **Scope:** Unrelated to plan 19-03 (delegate_task tool, last touched in phases 09 and 11)
- **Disposition:** Pre-existing failure; out of scope for Plan 19-03 (SCOPE BOUNDARY deviation rule)

## Phase 19 Plan 06 — deferred

**Pre-existing failing test (unrelated to Plan 06):**
- `ironhermes-tools::delegate_task::tests::test_delegate_task_schema_has_required_task`
- Fails at `crates/ironhermes-tools/src/delegate_task.rs:757` — schema must have 'task' as required
- Last modified in Phase 09/11 (git blame: 6d63a69 trait-object migration)
- Out of scope for Plan 06 (does not touch delegate_task.rs). Flag for Phase 20 backlog.
