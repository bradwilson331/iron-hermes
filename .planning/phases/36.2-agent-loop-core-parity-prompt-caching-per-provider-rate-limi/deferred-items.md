# Phase 36.2 Deferred Items

## Pre-existing clippy lints in ironhermes-core (out of scope for Plan 36.2-04)

Plan 36.2-04 acceptance ran `cargo clippy -p ironhermes-core --no-deps -- -D warnings`
against the new `pricing.rs` and `pricing_cache.rs`. Both files are 100% clean.
The 14 clippy warnings the lib emits crate-wide all live in pre-existing files
that the plan does not modify:

| File | Lint | Count |
|------|------|-------|
| crates/ironhermes-core/src/browser_profile.rs | collapsible_if (3), manually_reimplement_div_ceil | 4 |
| crates/ironhermes-core/src/commands/handlers.rs | field_reassign_with_default, needless_range_loop ×2 | 3 |
| crates/ironhermes-core/src/commands/typo.rs | iter_any_over_contains ×2 | 2 |
| crates/ironhermes-core/src/config.rs | derivable_impls ×2, missing_default ×1 | 3 |
| crates/ironhermes-core/src/memory_store.rs | manual_is_multiple_of | 1 |
| crates/ironhermes-core/src/skills.rs | derivable_impls, collapsible_if | 2 |

Scope boundary applied (Rule SCOPE BOUNDARY in execute-plan.md): only auto-fix
issues DIRECTLY caused by current task changes. These lints pre-date Plan 36.2-04
and are unchanged by it. Suggested follow-up: a dedicated clean-up plan running
`cargo clippy --fix --lib -p ironhermes-core` against the 11 auto-applicable
suggestions, then a manual pass for the 3 remaining (needless_range_loop in
handlers.rs is intentional pairwise indexing).

Verification:
```bash
cargo clippy -p ironhermes-core --no-deps 2>&1 | grep -E '(pricing\.rs|pricing_cache\.rs)'
# (empty output — new files are lint-clean)
```
