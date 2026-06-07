---
phase: 04-data-layer-interactions
plan: 04a
subsystem: shell

# Dependency graph
requires:
  - phase: 04-data-layer-interactions
    plan: 02
    provides: "BlockEntry wrapper + demo_block_entries() + Personality/ShellSettings/PaletteState types"
  - phase: 04-data-layer-interactions
    plan: 03
    provides: "mocks/ module tree (run_shell, run_agent_steps) + warp_hermes.rs Wave 3 shim"
provides:
  - "src/components/shell/markdown.rs: render_inline_code(text) -> Element backtick-aware inline code helper (D-15)"
  - "src/components/shell/block.rs: Block component refactored with entry: BlockEntry + on_copy/on_rerun EventHandler<()> props; is-ai body uses render_inline_code (D-16, D-23, D-24, KBD-05)"
  - "src/components/shell/block_stream.rs: BlockStream refactored with ReadOnlySignal<Vec<BlockEntry>> + on_rerun EventHandler<u64>; key by entry.id; clipboard write at child scope cfg-gated wasm32 (D-01, D-07, D-23)"
affects: [04-04b, 04-05]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Non-component Element-returning pure fn (markdown::render_inline_code) — caller uses {render_inline_code(&text)} braces-into-RSX"
    - "EventHandler<()> props for delegated hover actions — Block fires on_copy.call(()) / on_rerun.call(()), parent resolves semantics"
    - "ReadOnlySignal<Vec<BlockEntry>> as reactive prop — child calls blocks.read().iter().cloned(), each child captures entry via move closure"
    - "Stable RSX keys via entry.id — replaces index-based key: \"{i}\" with key: \"{entry.id}\" for /clear+append correctness (D-07/D-08)"
    - "Clipboard fire-and-forget via web_sys::Window navigator clipboard write_text, cfg-gated wasm32 only (D-23 + RESEARCH Common Op 2)"

key-files:
  created:
    - "src/components/shell/markdown.rs"
  modified:
    - "src/components/shell/mod.rs"
    - "src/components/shell/block.rs"
    - "src/components/shell/block_stream.rs"
    - "src/components/warp_hermes.rs"
    - "src/state.rs"

key-decisions:
  - "Dead-code suppression removal from state.rs: removed #[allow(dead_code)] on Personality::label/ALL, PaletteState, ShellSettings as Wave 4 begins wiring them (per execution directive). Temporary warnings expected until 04-04b/04-05 complete the wiring."

requirements-completed: [KBD-05]

# Metrics
duration: ~4min
completed: 2026-05-03
---

# Phase 04 Plan 04a: BlockEntry-id Propagation Summary

**Wave 4 first half — BlockEntry-id propagation through the rendering chain: markdown.rs (render_inline_code), block.rs (entry-typed props + render_inline_code routing), block_stream.rs (ReadOnlySignal<Vec<BlockEntry>> + stable id keys + clipboard write), and transient warp_hermes.rs shim to keep the call site type-checking.**

## Performance

- **Duration:** ~4 min
- **Completed:** 2026-05-03
- **Tasks:** 3 (all auto)
- **Files modified:** 5 (1 created, 4 modified)

## Task Commits

Each task committed atomically:

1. **Task 1: Create markdown.rs render_inline_code + register in shell/mod.rs** — `3561e38` (feat)
2. **Task 2: Refactor block.rs with entry + EventHandler props + render_inline_code routing** — `9b8f72d` (feat)
3. **Task 3: Refactor block_stream.rs to ReadOnlySignal<Vec<BlockEntry>> + clipboard wasm32 + warp_hermes.rs shim** — `6b0306d` (feat)
4. **Dead-code suppression cleanup** — `a2f1a5d` (feat)

**Plan metadata commit:** pending (final commit after STATE.md / ROADMAP.md updates).

## Files Created/Modified

- `src/components/shell/markdown.rs` (NEW, 27 lines) — `render_inline_code(text: &str) -> Element` pure fn; splits on backtick, alternating span/code segments wrapped in `<div style="white-space: pre-wrap;">` (D-15).
- `src/components/shell/mod.rs` (MODIFIED, +3 lines) — added `pub mod markdown;` and re-export `pub use markdown::render_inline_code;`.
- `src/components/shell/block.rs` (MODIFIED, 40 insertions, 24 deletions) — prop refactored from `data: BlockData` to `entry: BlockEntry` with `on_copy`/`on_rerun: EventHandler<()>`; is-ai body uses `{render_inline_code(&markdown)}`; copy/rerun delegate upstream; share button preserved as no-op; rerun greyed-out for non-Cmd (D-16, D-23, D-24, D-25, KBD-05).
- `src/components/shell/block_stream.rs` (MODIFIED, 77 insertions, 16 deletions) — prop refactored from `blocks: Vec<BlockData>` to `blocks: ReadOnlySignal<Vec<BlockEntry>>` + `on_rerun: EventHandler<u64>`; iterates `blocks.read().iter().cloned()`; stable keys via `key: "{entry.id}"`; clipboard write cfg-gated wasm32 with `block_text_for_copy` per D-23.
- `src/components/warp_hermes.rs` (MODIFIED, 8 insertions, 4 deletions) — transient shim: `let blocks = use_signal(crate::state::demo_block_entries);` and `BlockStream { blocks: blocks, on_rerun: move |_id: u64| {} }` call site (Plan 04-05 replaces the file entirely).
- `src/state.rs` (MODIFIED, 3 deletions) — removed `#[allow(dead_code)]` suppressions on `Personality::label`/`ALL`, `PaletteState`, `ShellSettings` (execution directive from important_notes).

## Decisions Made

- **Dead-code suppression removal ahead of full wiring:** Removed the scoped `#[allow(dead_code)]` annotations from Wave 1-introduced symbols (Personality::label, Personality::ALL, PaletteState, ShellSettings) as a proactive cleanup. The three `unused` warnings on these symbols are expected and bounded — 04-04b (command_palette, status_bar, agent_panel) and 04-05 (WarpHermes full rewire) will consume them. `cargo build` (without `-D warnings`) remains green on all platforms.

## Deviations from Plan

### Auto-fixed Issues

**None.** The plan executed exactly as written for Tasks 1-3.

### User-directed Deviation

**Dead-code suppression removal from state.rs**
- **Instruction:** User's important_notes directive: "Dead-code suppressions on `Personality::label`, `Personality::ALL`, `PaletteState`, `ShellSettings` should be REMOVED as this plan wires those symbols."
- **Action:** Removed three `#[allow(dead_code)]` attribute lines from state.rs.
- **Impact:** Introduces three `warn(dead_code)` on web/desktop/mobile `cargo check` output. These are warnings, not errors — `cargo build` exits 0. The warnings will vanish once 04-04b and 04-05 wire the remaining consumers.
- **Committed in:** `a2f1a5d`

## Known Stubs

| File | Line | Description | Resolution |
|------|------|-------------|------------|
| `src/components/shell/block.rs` | ~289 | Share button onclick is click-no-op (`/* D-25: share unwired in Phase 4 */`) | v2 — needs real share-link backend |
| `src/components/warp_hermes.rs` | ~39 | `on_rerun: move \|_id: u64\| {}` is a no-op shim | Plan 04-05 — full WarpHermes rewire replaces the entire file |

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: clipboard-write | block_stream.rs (write_to_clipboard) | New trust boundary: browser clipboard API with developer-controlled mock content. No XSS surface in Phase 4 (no untrusted input). Matches plan T-04-01 disposition `accept`. |

## Wave 4 Coordination Note

**Plan 04-04b runs in parallel (conceptually) and shares the warp_hermes.rs shim.** This plan (04-04a) touched the BlockEntry-propagation half (markdown/block/block_stream). 04-04b touches the non-BlockEntry primitives (input_box, command_palette, status_bar, agent_panel). Both started from the same Wave 3 checkpoint and both edit warp_hermes.rs minimally to keep their respective refactored primitives type-checking. The final compile gate (`cargo build --features {web,desktop,mobile}` + `cargo clippy --features web -- -D warnings`) is owned by 04-04b Task 5 and must pass with ZERO warnings after both halves land and the combined dead-code symbols are wired.

## Three-Platform Compile Gate

- **Web:** `cargo check --no-default-features --features web` — PASSED (0 errors, 3 expected dead_code warnings)
- **Desktop:** `cargo check --no-default-features --features desktop` — PASSED (0 errors, 3 expected dead_code warnings)
- **Mobile:** `cargo check --no-default-features --features mobile` — PASSED (0 errors, 3 expected dead_code warnings)

## Self-Check: PASSED

- `src/components/shell/markdown.rs` — FOUND
- `src/components/shell/mod.rs` — `pub mod markdown;` and `pub use markdown::render_inline_code;` present
- `src/components/shell/block.rs` — `entry: BlockEntry`, `on_copy: EventHandler<()>`, `on_rerun: EventHandler<()>`, `render_inline_code(&markdown)` present
- `src/components/shell/block_stream.rs` — `blocks: ReadOnlySignal<Vec<BlockEntry>>`, `on_rerun: EventHandler<u64>`, `key: "{entry.id}"`, `write_to_clipboard` with `#[cfg(target_arch = "wasm32")]` present
- `src/components/warp_hermes.rs` — `use_signal(crate::state::demo_block_entries)`, `BlockStream { blocks: blocks, on_rerun: ... }` present
- Commit `3561e38` (Task 1) — FOUND
- Commit `9b8f72d` (Task 2) — FOUND
- Commit `6b0306d` (Task 3) — FOUND
- Commit `a2f1a5d` (dead_code cleanup) — FOUND

---
*Phase: 04-data-layer-interactions*
*Plan: 04a — Wave 4 first half (BlockEntry-id propagation)*
*Completed: 2026-05-03*
