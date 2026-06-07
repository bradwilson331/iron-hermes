---
plan: 04-05
phase: 04-data-layer-interactions
status: complete
tasks: 2/2
started: 2026-05-03
completed: 2026-05-03
---

# Plan 04-05 — WarpHermes Integration

## Objective
Wave 4 — Replace the Wave 3 expanded shim with the full WarpHermes integration: 12 signals, use_context_provider, global keydown listener, auto-scroll, all handler closures, plus checkpoint UAT.

## Tasks Completed

### Task 1: Replace shim with full integration body
- 12 Phase 4 signals (`use_signal`): input, blocks, messages, mode, pal_open, pal_query, pal_state, scanner_active, focused, active_tab, tokens, next_id, personality
- `use_context_provider` for `ShellSettings { personality }` (D-02)
- Global keydown listener via `wasm_bindgen::Closure` + `use_effect` + `use_drop` (D-17, KBD-01..06)
- Auto-scroll via `use_effect` on `blocks.len()` → scroll `.wh-block-stream` into view (D-33)
- `submit()` handler: Common Op 1 + D-13 — reads input, fires `pulse_scanner` + `pulse_token`, `spawn(async)` → `run_shell` (Shell) or `run_agent_steps` (Agent)
- `on_rerun` handler: clones `BlockEntry` by id, re-executes via `run_shell`
- `pick` handler: `/personality` palette → updates personality via `ShellSettings` context
- `pulse_scanner` / `pulse_token` closures

**Commits:** `18230a5`

### Bug Fixes (discovered during UAT)
- `mode` signal needs `mut` + `#[allow(unused_mut)]` for `.set()` via Callback closure (`c032346`)
- `KeyboardEvent.code()` used instead of `.key()` for ⌘K/⌥M to avoid Option-key character mappings (`459e919`)
- `ev.prevent_default()` on ⌥M to stop `µ` insertion into textarea (`5e9b5ee`)

### Task 2: Manual UAT — side-by-side against prototype
**Status:** ✅ APPROVED (all 11 probes pass)

| Probe | Requirement | Status |
|-------|-------------|--------|
| SC-1: Submit + timing | MOCK-02 | pass |
| SC-2: Mode toggle (⌥M) | KBD-01 | pass |
| SC-3: Palette open/close | KBD-02 | pass |
| SC-3 cont: Navigation | KBD-03 | pass |
| SC-4: Shift+Enter | KBD-04 | pass |
| SC-5: Personality switch | MOCK-01 + KBD-06 | pass |
| SC-6: Token counter | MOCK-04 | pass |
| KBD-05: Hover affordances | — | pass |
| MOCK-03: Agent flow | — | pass |
| Auto-scroll | D-33 | pass |
| Three-platform compile | — | pass |

## What This Unlocks
- Shell is fully interactive — all keyboard shortcuts and mock flows work
- Phase 4 is now complete (all 6 plans done)
- Enables Phase 5 (TweaksPanel) which will add `theme`, `density`, `block`, `agent` fields to `ShellSettings`

## Verification
- `cargo build --features {web,desktop,mobile}`: ALL PASS (0 errors)
- `cargo clippy --features web -- -D warnings`: PASS (0 errors, 0 warnings)

## Files Modified
- `src/components/warp_hermes.rs` (replaced)
- `src/state.rs` (dead-code suppression removal)
- `src/components/shell/input_box.rs` (minor: ref to `focused` signal)
