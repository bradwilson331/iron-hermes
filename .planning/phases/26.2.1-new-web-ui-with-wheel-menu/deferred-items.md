# Phase 26.2.1 — Deferred Items

Out-of-scope discoveries logged during plan execution (per executor SCOPE BOUNDARY).


## Plan 05 — pre-existing clippy errors in `ironhermes-core` block `-D warnings`

- Source: `crates/ironhermes-core/src/skills.rs` (derivable_impls, collapsible_if, manual_is_multiple_of, etc — 14 errors)
- Verified pre-existing on base commit `e26cf21b` by stashing Plan 05 changes and re-running clippy.
- Impact: `cargo clippy -p iron_hermes_ui -- -D warnings` cannot run cleanly because `iron_hermes_ui` depends on `ironhermes-core`.
- Workaround applied: Plan 05 verified via `cargo check -p iron_hermes_ui` (host + wasm) instead. New code added by this plan contains zero clippy warnings (visually audited; `cargo check` reports only pre-existing dead-code warnings in unrelated files).
- Out of scope per executor SCOPE BOUNDARY (these errors are not caused by Plan 05 changes).
