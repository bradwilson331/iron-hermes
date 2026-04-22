# Deferred Items — Phase 21.8

Pre-existing issues discovered during Phase 21.8 execution but out of scope per SCOPE BOUNDARY rule.

## Clippy warnings on `cargo clippy --workspace -- -D warnings`

Pre-existing warnings unrelated to Phase 21.8 changes:

- `crates/ironhermes-core/src/memory_store.rs:429` — manual `.is_multiple_of()` implementation
- `crates/ironhermes-core/src/skills.rs:125` — derivable `impl Default for SkillSource`
- Plus ~14 other pre-existing `ironhermes-hub` warnings (collapsible_if, needless_borrow, etc.)

**Verification:** `cargo clippy -p ironhermes-hub --lib 2>&1 | grep sanitize.rs` returns zero lines — the 9 new `sanitize::*` functions are clippy-clean. The workspace-level `-D warnings` requirement listed in the plan's `<verification>` is not satisfiable without touching pre-existing code outside the 21.8 scope. Recommend a cleanup phase for existing clippy debt or scope adjustment.

