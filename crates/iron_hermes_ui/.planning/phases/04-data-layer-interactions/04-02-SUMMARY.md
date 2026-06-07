---
phase: 04-data-layer-interactions
plan: 02
subsystem: state
tags: [dioxus, state, types, enum, refactor, wasm, cfg-gated, signal, context]

# Dependency graph
requires:
  - phase: 04-data-layer-interactions
    plan: 01
    provides: "cfg-gated function-body pattern (template for now_time wasm32/native split); js-sys 0.3 in [dependencies] (consumed by now_time wasm32 branch); three-platform compile gate as canonical Phase 4 verification"
  - phase: 03-desktop-shell
    provides: "Phase 3 type vocabulary in src/state.rs (Block, CommandLine, Token, ToolCall, ToolStatus, Mode, PaletteItem, Tab, Message, TokenBudget) — Wave 1 extends, never replaces"
provides:
  - "src/state.rs Phase 4 type vocabulary: Personality (with label() and ALL: [Personality; 6]), BlockEntry { id, block }, PaletteState (Browse | PersonalityPick), ShellSettings { personality: Signal<Personality> }, now_time() cfg-gated helper"
  - "Destructive rename: pub fn demo_block_entries() -> Vec<BlockEntry> with ids 1..=10 mirroring b1..b10 prototype source-of-truth"
  - "Stable type surface for Wave 2 (mocks/) and Wave 4 (shell prop refactor); Personality::Eq+Hash unblocks Wave 3 personalities.rs::pick_reply lookup; ShellSettings: Copy unblocks Wave 3/4 use_context::<ShellSettings>() consumers"
affects: [04-03, 04-04a, 04-04b, 04-05]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "cfg-gated function body for now_time() (mirrors 04-01 timer::sleep precedent — single public signature, two bodies, gate inside fn body)"
    - "Eq + Hash on small fieldless enum — enables const-table .find() lookups without HashMap allocation (Wave 3 pick_reply consumer)"
    - "Bag-of-related-Signals struct (ShellSettings) — Copy + Clone, individual Signal<T> fields so per-field writes don't invalidate other-field consumers; canonical Dioxus 0.7 use_context_provider pattern"
    - "Identified-wrapper struct (BlockEntry { id, block }) — keeps the inner enum a pure data shape; identity is wrapper concern, RSX iterates with stable key: \"{entry.id}\""
    - "Destructive rename without alias (D-09) — forces all downstream callers to migrate atomically in their own waves; expected single carry-forward breakage documented at Wave-1 compile gate"

key-files:
  created:
    - ".planning/phases/04-data-layer-interactions/04-02-SUMMARY.md"
  modified:
    - "src/state.rs"

key-decisions:
  - "Personality derives include Eq + Hash (D-03 explicit) so Wave 3 personalities.rs::pick_reply can use .find(|(k, _)| *k == p) on a const REPLIES table without HashMap allocation — small fieldless enum makes this a free win"
  - "ShellSettings derives Clone + Copy only (no PartialEq, no Debug) because Signal<T> doesn't impl Debug — Copy is required so use_context::<ShellSettings>() returns cheap clones; consumers don't need PartialEq/Debug on the bundle"
  - "BlockEntry derives Clone + PartialEq + Debug only (NOT Copy) — Block contains String fields which are not Copy; matches the existing Phase 3 derive shape on Block itself"
  - "PaletteState derives Default with #[default] on Browse so use_signal(|| PaletteState::default()) starts in browse mode (D-20)"
  - "now_time() cfg gate at function-body level (not module-level) per RESEARCH Pitfall 4 — js_sys::Date::new_0() is wasm-only; gating at fn body avoids unresolved-import errors on native"
  - "Destructive rename per D-09 — no demo_blocks() deprecated alias kept; warp_hermes.rs:6 import line is the single expected carry-forward breakage that Wave 4 fixes as part of the WarpHermes rewire"

patterns-established:
  - "Pattern: cfg-gated string-returning helper (now_time) — extends the 04-01 timer::sleep cfg pattern to non-async; same single-public-signature shape, function-body gate"
  - "Pattern: Phase 4 use_context bag (ShellSettings) — Phase 5 will add theme/density/block/agent fields additively without refactoring existing consumers (D-02 forward-compatibility contract)"

requirements-completed: []

# Metrics
duration: ~4min
completed: 2026-05-03
---

# Phase 04 Plan 02: state.rs Extensions Summary

**Phase 4 Wave 1: extended `src/state.rs` with the Phase 4 type vocabulary (Personality enum + label/ALL helpers, BlockEntry wrapper, PaletteState substate enum, ShellSettings cross-cutting context bundle, cfg-gated now_time helper) and performed the destructive rename `demo_blocks() -> Vec<Block>` → `demo_block_entries() -> Vec<BlockEntry>` per D-09. state.rs compiles cleanly on web/desktop/mobile — the only carry-forward error is the documented `warp_hermes.rs:6` import line that Wave 4 fixes.**

## Performance

- **Duration:** ~4 min (252s)
- **Started:** 2026-05-03T15:51:05Z
- **Completed:** 2026-05-03T15:55:17Z
- **Tasks:** 3 (2 commit + 1 verify-only)
- **Files modified:** 1 (src/state.rs)
- **Lines added:** ~119 new + ~28 rename-touched

## Accomplishments

- Added `use dioxus::prelude::Signal;` import at the top of `state.rs` (required for the `ShellSettings.personality: Signal<Personality>` field type).
- Added `pub enum Personality { Concise, Technical, Noir, Hype, Catgirl, Default }` deriving `Clone + Copy + PartialEq + Eq + Hash + Debug + Default` with `#[default]` on the `Default` variant (D-03). The `Eq + Hash` derives are explicit so Wave 3's `personalities.rs::pick_reply` can use `.find(|(k, _)| *k == p)` on the const `REPLIES` table without a HashMap allocation.
- Added `Personality::label() -> &'static str` returning lowercase slugs (`concise`/`technical`/`noir`/`hype`/`catgirl`/`default`) for palette `PersonalityPick` rows and the status-bar personality pill (D-22).
- Added `Personality::ALL: [Personality; 6]` const enumerating all variants in declaration order — used by the palette `PersonalityPick` substate to render selectable rows (D-20 + Claude's Discretion: small const helper, no `strum` dep).
- Added `pub struct BlockEntry { pub id: u64, pub block: Block }` deriving `Clone + PartialEq + Debug` — identified wrapper for stable RSX keys across `/clear` and append cycles (D-07). Block enum stays pure data; identity is wrapper concern. Does NOT derive `Copy` because `Block` contains `String` fields.
- Added `pub enum PaletteState { Browse, PersonalityPick }` deriving `Clone + Copy + PartialEq + Debug + Default` with `#[default]` on `Browse` — two-state palette substate machine (D-20). Selecting `/personality` from `Browse` → `PersonalityPick`; selecting a personality writes `ShellSettings.personality` and returns to `Browse`.
- Added `pub struct ShellSettings { pub personality: Signal<Personality> }` deriving `Clone + Copy` — the cross-cutting context bag provided via `use_context_provider` (D-02 + RESEARCH Pattern 5). Forward-compatible: Phase 5 will append `theme`, `density`, `block`, `agent` fields without refactoring existing consumers. Individual `Signal<T>` fields (rather than `Signal<ShellSettings>`) ensure per-field writes don't invalidate consumers of the other fields.
- Added `pub fn now_time() -> String` with cfg-gated body — `js_sys::Date::new_0()` HH:MM:SS on wasm32, hardcoded `"00:00:00"` on native (D-34). Gate is at function-body level (not module-level) per RESEARCH Pitfall 4, mirroring the 04-01 `timer::sleep` precedent. The `d.get_hours() as u32` cast handles the browser Date API's `f64` return type (truncation acceptable; hours ∈ [0, 23]).
- **Destructive rename** `demo_blocks() -> Vec<Block>` → `demo_block_entries() -> Vec<BlockEntry>` (D-09) with all 10 prototype seed blocks wrapped in `BlockEntry { id: <N>, block: Block::X { ... } }` where ids 1..=10 mirror the existing `b1..b10` inline comments. Block literal byte sequence preserved exactly; only the function signature, doc comment, and per-element `BlockEntry` wrapper changed. No deprecation alias — Wave 4 migrates the single downstream caller atomically.
- Three-platform compile gate (Task 3) confirms `src/state.rs` itself compiles cleanly on `cargo check --features {web|desktop|mobile} --no-default-features` — zero state.rs errors on any platform.

## Task Commits

Each task was committed atomically:

1. **Task 1: Add Personality + BlockEntry + PaletteState + ShellSettings + now_time to state.rs** — `ab9451a` (feat)
2. **Task 2: Destructive rename demo_blocks → demo_block_entries returning Vec<BlockEntry>** — `621b0f1` (refactor)
3. **Task 3: Three-platform compile gate (state.rs alone)** — verification-only, no commit (per plan: `<files></files>` empty)

**Plan metadata commit:** pending (final commit after STATE.md / ROADMAP.md updates).

## Files Created/Modified

- `src/state.rs` (MODIFIED, +147 / -23) — Added one `use dioxus::prelude::Signal;` import line; appended a 119-line "Phase 4 type vocabulary" section between `TokenBudget` and the existing `Phase 3 demo fixtures` divider; renamed `demo_blocks() -> Vec<Block>` to `demo_block_entries() -> Vec<BlockEntry>` with each seed wrapped in `BlockEntry { id: 1..=10, block: ... }`. Existing types (`Block`, `CommandLine`, `Token`, `ToolCall`, `ToolStatus`, `Mode`, `PaletteItem`, `Tab`, `Message`, `TokenBudget`) and the other `demo_*` fixtures (`demo_messages`, `demo_palette_items`, `demo_tabs`) untouched.
- `.planning/phases/04-data-layer-interactions/04-02-SUMMARY.md` (NEW) — this file.

## Decisions Made

- **Eq + Hash derives on `Personality`** (D-03 explicit): the plan's must-have `truths` list pinned the full derive set including `Eq + Hash`. This unblocks Wave 3's `personalities.rs::pick_reply` to use `.find(|(k, _)| *k == p)` on a const `REPLIES: [(Personality, &str); 6]` table without a HashMap allocation. For a 6-entry fieldless enum this is the canonical small-cardinality lookup.
- **`ShellSettings: Copy` not `ShellSettings: Clone` only**: `Signal<T>` is `Copy`, so the bag derives `Copy` too (no field is non-Copy). This is required for cheap `use_context::<ShellSettings>()` retrieval — Wave 3/4 consumers (`status_bar`, `agent_panel`, `command_palette`) call `use_context()` and receive a `Copy` of the bundle without any heap traffic. Skipping `PartialEq`/`Debug` on `ShellSettings` is fine because (a) `Signal<T>` doesn't impl `Debug` so the derive would fail, and (b) consumers don't compare or print bundles.
- **`BlockEntry` does NOT derive `Copy`**: `Block` contains `String` fields (not `Copy`), so `BlockEntry` must clone-not-copy. This matches the existing Phase 3 derive shape on `Block` itself (`Clone + PartialEq + Debug`).
- **`PaletteState` `#[default]` on `Browse`**: ensures `use_signal(|| PaletteState::default())` (used in Wave 4b's `command_palette.rs`) starts in browse mode without an explicit initializer.
- **`now_time()` function-body cfg gate**: chose this over splitting into `wasm_now_time` / `native_now_time` modules per RESEARCH Pitfall 4 ("module-level cfg on `use js_sys` causes unresolved-import errors on native"). The fn-body gate keeps `js_sys::Date::new_0()` inside the `#[cfg(target_arch = "wasm32")]` block where the symbol is reachable. Pattern mirrors `timer::sleep` from Plan 04-01.
- **Destructive rename without compatibility shim** (D-09): no `pub fn demo_blocks() -> Vec<Block>` deprecated alias retained. The plan rationale ("forces all Phase 4 work to use BlockEntry-with-id from the start") is honored — Wave 4's `warp_hermes.rs` rewire migrates the single caller atomically. Carry-forward breakage is documented and bounded.

## Deviations from Plan

None — the plan executed exactly as written. All grep predicates passed on first run; all three platforms produced the documented single expected `warp_hermes.rs:6` error and zero state.rs errors. No Rule 1/2/3 auto-fixes triggered.

## Known Wave 1 Carry-Forward Breakage

The plan explicitly anticipates and documents this single breakage (D-09 rationale + Task 3 acceptance criteria):

| File | Line | Symbol | Resolution wave |
|------|------|--------|-----------------|
| `src/components/warp_hermes.rs` | 6 | `use crate::state::demo_blocks;` (now-renamed function) | Wave 4 (Plan 04-04a / 04-04b — WarpHermes prop-shape refactor) |

Compile-gate evidence (post-Task 2, pre-Wave 4):

```
$ cargo check --features web --no-default-features --message-format=short
src/components/warp_hermes.rs:6:5: error[E0432]: unresolved import `crate::state::demo_blocks`: no `demo_blocks` in `state`
error: could not compile `iron_hermes_ui` (bin "iron_hermes_ui") due to 1 previous error

$ cargo check --features desktop --no-default-features --message-format=short
src/components/warp_hermes.rs:6:5: error[E0432]: unresolved import `crate::state::demo_blocks`: no `demo_blocks` in `state`
error: could not compile `iron_hermes_ui` (bin "iron_hermes_ui") due to 1 previous error

$ cargo check --features mobile --no-default-features --message-format=short
src/components/warp_hermes.rs:6:5: error[E0432]: unresolved import `crate::state::demo_blocks`: no `demo_blocks` in `state`
error: could not compile `iron_hermes_ui` (bin "iron_hermes_ui") due to 1 previous error
```

Per-platform error inventory (filtering only source-level `path:line:col: error` lines, not Cargo's aggregate "could not compile" summary):

- **Web:** exactly 1 source error — `src/components/warp_hermes.rs:6:5` E0432.
- **Desktop:** exactly 1 source error — `src/components/warp_hermes.rs:6:5` E0432.
- **Mobile:** exactly 1 source error — `src/components/warp_hermes.rs:6:5` E0432.
- **state.rs source errors:** 0 on every platform.

This is the **single expected** breakage (D-09) and is bounded — the only consumer of the renamed function is `warp_hermes.rs:6`, which Wave 4 (Plan 04-04a/04-04b) rewires as part of the prop-shape refactor that converts `BlockStream` from `Vec<Block>` to `ReadOnlySignal<Vec<BlockEntry>>`.

## Issues Encountered

- **Task 3 verification predicate exit-code interaction**: the plan's `<automated>` predicate runs three sequential `cargo check` invocations and uses pipeline grep filters to assert "no state.rs errors AND only warp_hermes/BlockStream errors remain". Because `cargo check` exits nonzero on compile failure (which is the *expected* state here per D-09), the final invocation in the predicate inherits that nonzero exit. Decomposed the predicate into per-platform source-level error inventories (`grep -E '^[^ ]+\.rs:[0-9]+:[0-9]+:.*error'`) which cleanly separate (a) cargo's "could not compile" aggregate line from (b) per-source-file errors. The semantic acceptance is met: zero state.rs errors and exactly one source error per platform, all from `warp_hermes.rs:6`. No fix required — this is a verification-tooling observation, not a plan deviation.

## Next Phase Readiness

- **Wave 2 (Plan 04-03 — `mocks/` module tree) is unblocked.** All Wave 2 inputs are present: `Personality` enum (with `Eq + Hash` for `pick_reply`'s const-table lookup), `BlockEntry` (for `run_shell` to construct), `now_time()` helper (for `runShell`/`runAgent` block timestamps), `Block` and friends (unchanged from Phase 3), `crate::platform::timer::sleep` (from 04-01).
- **Wave 4a/4b (BlockEntry-id propagation + prop-shape refactor) is unblocked at the type level.** `BlockEntry`, `PaletteState`, and `ShellSettings` are stable; Wave 4 imports them and rewires `BlockStream`, `command_palette`, `status_bar`, `agent_panel` props.
- **Wave 5 (Plan 04-05 — `WarpHermes` rewire) is unblocked at the type level.** `ShellSettings { personality: Signal<Personality> }` is the exact shape `WarpHermes()` will pass to `use_context_provider`. The `demo_block_entries()` returning `Vec<BlockEntry>` is the exact shape the new `Signal<Vec<BlockEntry>>` initializer needs.
- **Carry-forward error is bounded.** Only one file (`warp_hermes.rs`) and one line (6) currently fail to compile. Wave 4 will fix this as part of its planned `WarpHermes` rewire — no extra cleanup work created.
- **Three-platform compile gate cadence preserved.** Subsequent Phase 4 plans will continue running `cargo check --features {web|desktop|mobile} --no-default-features` as their wave-end gate, expecting the residual `warp_hermes.rs` error to disappear at Wave 4.

## Threat Flags

None. This plan is pure type addition + cfg-gated string helper + destructive rename with no external trust boundaries. The `now_time()` wasm32 branch reads from JS `Date` (browser-trusted local clock; no untrusted input). The phase-level threats T-04-01..T-04-04 (clipboard surface, listener cleanup, palette filter, scanner overlap) all attach to consumer waves (Wave 3/4), not to this plan.

## Self-Check

- `src/state.rs` — FOUND (modified; contains `use dioxus::prelude::Signal;`, `pub enum Personality`, `Personality::label()`, `Personality::ALL`, `pub struct BlockEntry`, `pub enum PaletteState`, `pub struct ShellSettings`, `pub fn now_time()`, `pub fn demo_block_entries()`)
- `.planning/phases/04-data-layer-interactions/04-02-SUMMARY.md` — FOUND (this file)
- Commit `ab9451a` (Task 1: feat — Phase 4 type vocabulary) — FOUND
- Commit `621b0f1` (Task 2: refactor — demo_blocks → demo_block_entries) — FOUND

## Self-Check: PASSED

---
*Phase: 04-data-layer-interactions*
*Completed: 2026-05-03*
