# Phase 36.3.7.11 — Deferred Items

Items discovered during execution that are OUT OF SCOPE for the current plan
per the executor's deviation-rule SCOPE BOUNDARY ("Only auto-fix issues
DIRECTLY caused by the current task's changes. Pre-existing warnings, linting
errors, or failures in unrelated files are out of scope.").

## From Plan 04 execution (worktree-agent-a303edb61c7f61835, 2026-06-01)

### Pre-existing clippy warnings in iron_hermes_ui (not caused by Plan 04)

`cargo clippy -p iron_hermes_ui --features server -- -D warnings` fails on
**pre-existing** issues in files NOT modified by Plan 04:

- `crates/iron_hermes_ui/src/components/hermes_app/screens/skills.rs:43,46,48`
  — redundant closures, `useless_*` (pre-existing).
- `crates/iron_hermes_ui/src/server/ws.rs:304,406` — `needless_borrow`
  (pre-existing).
- `crates/iron_hermes_ui/src/server/state.rs:455,479,500,604` — pre-existing.
- `crates/iron_hermes_ui/src/components/hermes_app/screens/agents.rs:75,80,83,320`
  — pre-existing patterns (the toolbar-button insertion at lines 364-374 is
  Plan 04's, and is clippy-clean).
- `crates/iron_hermes_ui/src/state.rs:32,56,93,121,132,140,149,169,176,215,259,279`
  — pre-existing dead-code lints on unrelated demo / placeholder enums and
  structs (`Block` variants, `PaletteState`, `ShellSettings`, etc.). Plan 04's
  changes to `WheelWedge` / `Screen` / their `impl` blocks (state.rs:580-758)
  do NOT produce any clippy warnings.

### Pre-existing clippy errors in ironhermes-core (out of crate scope)

`cargo clippy -p ironhermes-core` fails with 16 errors (collapsible-if, default
enum, etc.) in `crates/ironhermes-core/src/skills.rs` and related files. Plan 04
does NOT touch `ironhermes-core` at all.

### Recommendation

These warnings predate the phase and should be cleaned up by a future
dedicated quality phase, not by Plan 04 of phase 36.3.7.11. The success
criteria's `cargo clippy --workspace --features server -- -D warnings pass`
gate is technically not met because of the pre-existing issues, but Plan 04's
own changes are clippy-clean (no warnings introduced in any modified line).
