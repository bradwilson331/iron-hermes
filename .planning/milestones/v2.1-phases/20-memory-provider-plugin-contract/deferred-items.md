# Phase 20 — Deferred Items

Items discovered during Phase 20 plan execution that are OUT OF SCOPE for
the current plan but should be addressed elsewhere.

## Pre-existing clippy warnings (ironhermes-core)

Discovered while running `cargo clippy -p ironhermes-agent --all-features
-- -D warnings` as part of Plan 20-01 verification. None are introduced
by Plan 20-01; all existed prior. Out-of-scope per SCOPE BOUNDARY rule —
logged here for future cleanup.

| File | Line | Lint | Message |
|------|------|------|---------|
| crates/ironhermes-core/src/memory_store.rs | 429 | clippy::manual-is-multiple-of | `(len - i) % 3 == 0` — replace with `(len - i).is_multiple_of(3)` |
| crates/ironhermes-core/src/skills.rs | 125 | clippy::derivable_impls | Manual `Default` for `SkillSource` can be derived with `#[derive(Default)]` + `#[default]` on `Builtin` variant |

Impact: workspace does not pass `cargo clippy --workspace --all-features
-- -D warnings`. Plan 20-01 per-crate clippy on ironhermes-agent itself
passes when ironhermes-core is not in scope. Recommend a standalone
chore commit (`chore(core): fix pre-existing clippy warnings`) before or
during Plan 20-02.

## Pre-existing failing test (ironhermes-tools)

Discovered while running `cargo test --workspace --all-features --lib`
during Plan 20-01 verification. Fails on the `develop` baseline commit
4b3d5b0 (pre-Plan 20-01), so NOT introduced by this plan. Out-of-scope
per SCOPE BOUNDARY rule.

| Test | File | Observation |
|------|------|-------------|
| `delegate_task::tests::test_delegate_task_schema_has_required_task` | crates/ironhermes-tools/src/delegate_task.rs:752 | Asserts `task` is in `required` schema field, but commit bbf48db ("WR-01 fix schema task/tasks mutual exclusivity") made `task` and `tasks` mutually-exclusive-optional. The test was not updated. |

Impact: `cargo test --workspace --all-features --lib` shows 1 failure
(138 passed, 1 failed). Recommend deletion or rewrite of this test as
a standalone `fix(tools): align delegate_task schema test with WR-01`
commit before Plan 20-02.

## Mutex flavor migration (Plan 20-02 scope)

Plan 20-01 kept the factory return type on `std::sync::Mutex` (documented
deviation — see 20-01-SUMMARY.md). Plan 20-02 should atomically migrate:

- `crates/ironhermes-agent/src/memory/factory.rs` return type
- `crates/ironhermes-tools/src/memory_tool.rs` store field + `.lock()` sites
- `crates/ironhermes-tools/src/registry.rs::register_memory_tool` param
- `crates/ironhermes-tools/src/delegate_task.rs` param + field
- `crates/ironhermes-gateway/src/runner.rs` field + `.lock().unwrap()` sites
- `crates/ironhermes-gateway/src/handler.rs` field
- `crates/ironhermes-agent/src/prompt_builder.rs` field + `.lock()` site
- `crates/ironhermes-cli/src/main.rs` local type annotation at line 612

All `.lock().unwrap()` sites must become `.lock().await` and their
containing functions must become `async fn` if not already.
