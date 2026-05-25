# Deferred Items — Phase 21.8

Pre-existing issues discovered during Phase 21.8 execution but out of scope per SCOPE BOUNDARY rule.

## Clippy warnings on `cargo clippy --workspace -- -D warnings`

Pre-existing warnings unrelated to Phase 21.8 changes:

- `crates/ironhermes-core/src/memory_store.rs:429` — manual `.is_multiple_of()` implementation
- `crates/ironhermes-core/src/skills.rs:125` — derivable `impl Default for SkillSource`
- Plus ~14 other pre-existing `ironhermes-hub` warnings (collapsible_if, needless_borrow, etc.)

**Verification:** `cargo clippy -p ironhermes-hub --lib 2>&1 | grep sanitize.rs` returns zero lines — the 9 new `sanitize::*` functions are clippy-clean. The workspace-level `-D warnings` requirement listed in the plan's `<verification>` is not satisfiable without touching pre-existing code outside the 21.8 scope. Recommend a cleanup phase for existing clippy debt or scope adjustment.

## Pre-existing `ironhermes-core` test failures (plan 21.8-04, Wave 3)

Two `ironhermes-core` tests fail on `cargo test --workspace` on `develop` and reproduce on a clean `git stash` of the Wave 3 working tree — pre-dating any 21.8-04 change:

- `ironhermes-core::commands::handlers::tests::cmd_models_info_known_model`
  - Panic: `missing context length: claude-sonnet-4`
  - Root cause (not investigated here): `commands/handlers.rs:584` unwraps a missing context-length entry for `claude-sonnet-4` from the static model table; model metadata fetcher or static table doesn't populate `max_context` for that model id.
- `ironhermes-core::provider::tests::test_provider_resolver_populates_model_metadata`
  - Assertion: `left: 1000000, right: 200000` — the resolved context length doesn't match the fixture.

**Verification that these are pre-existing:** `git stash` the Wave 3 changes and run the two tests in isolation — both still fail. They are not reachable from any file Wave 3 modified (`skills_cmd.rs`, `skills_tool.rs`, `lib.rs` in hub). Logged here per SCOPE BOUNDARY; recommend a follow-up in a Phase 21.3 touch-up or a dedicated model-registry cleanup plan.

