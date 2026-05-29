---
created: 2026-05-29T00:00:00.000Z
title: Remove legacy WarpHermes shell (components/warp_hermes.rs)
area: ui
files:
  - crates/iron_hermes_ui/src/components/warp_hermes.rs
  - crates/iron_hermes_ui/src/main.rs
  - crates/iron_hermes_ui/Cargo.toml
---

## Problem

`components/warp_hermes.rs` is the original WarpHermes design-prototype shell.
`HermesApp` (in `components/hermes_app/`) has been the active root since the
Phase 26.x port, and `WarpHermes` is opt-in only via the `legacy-shell` feature
flag (per project memory `[HermesApp is the active root, not WarpHermes]`).

The file is dead code from a user perspective but still compiles into the
workspace, which has been creating recurring carry-cost across phases:

- Every time a new `ChatStreamEvent` variant is added to `protocol.rs`,
  `warp_hermes.rs`'s exhaustive `match event { ... }` at line 216 also needs
  a no-op arm. Phase 26.7.1 had to add one for `SubagentEvent {}`. Phase
  36.17.4 had to add another for `QueueUpdated { .. }` (fix commit
  `62bfa9c1` — surfaced by the post-merge `cargo check --workspace
  --all-features` gate after Wave 2 merged).
- Each new arm is a 5-line no-op with a "legacy shell does not render X"
  comment — pure friction for plans that legitimately only target the
  active HermesApp shell.
- Plan `files_modified` frontmatter has to remember to declare
  `warp_hermes.rs` whenever a `ChatStreamEvent` variant lands, or the cross-
  plan integration gate trips on `--all-features`. This has been a recurring
  miss because the file is invisible from the active HermesApp tree.

## Solution

Delete `components/warp_hermes.rs` and any wiring that still references it:

1. Audit references — `grep -r "warp_hermes" crates/iron_hermes_ui/` and
   `grep -r "legacy-shell" crates/iron_hermes_ui/`.
2. Remove the `legacy-shell` feature from `Cargo.toml` if it has no other
   purpose.
3. Drop the `mod warp_hermes;` declaration and any `#[cfg(feature =
   "legacy-shell")]` routes / mounting in `main.rs` (or wherever the shell
   is dispatched from).
4. Verify with `cargo check --workspace --all-features` + the full test
   surface (iron_hermes_ui, gateway, CLI).
5. Update project memory note `project_warp_vs_hermes_app_active_root` to
   reflect that the legacy shell has been removed.

Schedule this before the next `ChatStreamEvent` variant lands — it will save
one more 5-line carrying patch and unblock plans from having to declare
`warp_hermes.rs` in their `files_modified` lists.

## Related

- Fix commit: `62bfa9c1` — `fix(36.17.4-02): add QueueUpdated no-op arm to legacy WarpHermes shell`
- Prior precedent: Phase 26.7.1 Plan 02 added the `SubagentEvent {}` no-op arm
- Memory: `project_warp_vs_hermes_app_active_root.md`
