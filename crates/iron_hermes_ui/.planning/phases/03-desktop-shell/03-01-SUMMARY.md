---
phase: 03-desktop-shell
plan: 01
subsystem: ui
tags: [dioxus, dioxus-0.7, rust, css-keyframes, fixtures, shell-types]

# Dependency graph
requires:
  - phase: 02-design-system
    provides: "design-tokens.css (--accent-primary, --accent-primary-hi, --fg-dim, --scanner-period vars), warp-ih.css (.wh-scanner.is-active rules), Phase 2 cascade order verified"
provides:
  - "src/state.rs Phase 3 data model: Block enum (6 variants), CommandLine, Token, ToolCall, ToolStatus, Mode, PaletteItem, Tab, Message, TokenBudget"
  - "Four prototype-verbatim demo fixture functions: demo_blocks() (10), demo_messages() (5), demo_palette_items() (10), demo_tabs() (3)"
  - "assets/scanner-anim.css: @keyframes wh-scanner-tick + 10 staggered :nth-child animation-delay rules — resolves SHELL-09 design gap"
  - "src/components/shell/ module hierarchy staged: 11 pub mod + 11 pub use re-exports (primitive bodies land in 03-02 / 03-03)"
  - "src/components/mod.rs rewritten to expose warp_hermes + shell submodules (hero re-export removed; deletion deferred to 03-04)"
affects: [03-02-shell-primitives-A, 03-03-shell-primitives-B, 03-04-warp-hermes-composer, 03-05-mobile-shell, 04-data-layer-and-interactions]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Pure-data Rust module (src/state.rs): no Dioxus imports, no reactivity primitives, owned String/Vec types — satisfies CONTEXT D-04 + D-06"
    - "Single-file CSS @keyframes for the knight-rider scanner: 10 sibling spans with staggered negative animation-delay phase-shift the wave (CONTEXT D-08, RESEARCH Pattern 7)"
    - "Module-hierarchy staging: declare pub mod / pub use ahead of leaf-file landing — Wave 1 stages contracts so Waves 2/3 build against stable interfaces"
    - "Block enum with kind_class() helper returning the CSS class fragment used by warp-ih.css 2px accent stripe rules (matches prototype shell.jsx classnames verbatim)"

key-files:
  created:
    - "assets/scanner-anim.css"
    - "src/components/shell/mod.rs"
  modified:
    - "src/state.rs (placeholder → 395-line data module)"
    - "src/components/mod.rs (hero re-export → warp_hermes + shell)"

key-decisions:
  - "Authored Block::Tool variant + 4 demo blocks (b7..b10) covering every ToolStatus state — Wave 2's BlockStream then Wave 4's WarpHermes can render the full status enum without further fixture work (CONTEXT D-13 + D-18)"
  - "Token is an owned 4-variant enum (Bin/Arg/Flag/Str) with .text() returning &str via single match arm using the Token::Variant(s) | ... pattern — matches CommandLine token-type CSS class set"
  - "scanner-anim.css uses var(--scanner-period, 1800ms) fallback so the file works even if a future cascade reorder evicts the design-tokens.css var (defensive against Phase 2 cascade reordering)"

patterns-established:
  - "Pattern: Phase 3 fixture functions live in src/state.rs and return owned Vec<T> — every primitive component re-renders against fresh fixture calls (no static mut)"
  - "Pattern: components::shell::* primitives are flat — one file per primitive in src/components/shell/, mod.rs re-exports the PascalCase component name. Naming-collision risk with state types (Block, CommandLine, ToolCall) is documented in 03-01-PLAN line 437; consumer plans alias via `use crate::state::Block as BlockData;` where needed"
  - "Pattern: Module declarations may stage interfaces before bodies exist — Wave 1 commits pub mod / pub use lines that won't compile until Wave 2 lands the leaves. The phase-level green-build gate is Plan 03-04 Task 4, not after every plan"

requirements-completed: [SHELL-02, SHELL-04, SHELL-05, SHELL-08, SHELL-09, SHELL-10]

# Metrics
duration: 4min
completed: 2026-05-03
---

# Phase 3 Plan 01: Foundation — Shared Types, Fixtures, Scanner CSS, Module Hierarchy Summary

**Promotes src/state.rs from a one-line placeholder into the 395-line Phase 3 data module (10 public types + 4 prototype-verbatim demo fixtures), authors assets/scanner-anim.css to resolve the SHELL-09 scanner-animation gap via pure CSS @keyframes, and stages the components::shell::* + components::warp_hermes module hierarchy for Waves 2–4 to populate.**

## Performance

- **Duration:** 4 min
- **Started:** 2026-05-03T09:34:47Z
- **Completed:** 2026-05-03T09:38:50Z
- **Tasks:** 3 / 3
- **Files modified:** 4 (1 created CSS, 1 created shell/mod.rs, 1 rewritten state.rs, 1 rewritten components/mod.rs)

## Accomplishments

- **src/state.rs (395 lines):** Defines `Block` (6 variants: Cmd/Out/Ai/Ok/Err/Tool) with `kind_class()` returning `"is-cmd"/"is-out"/"is-ai"/"is-ok"/"is-err"/"is-tool"`; `CommandLine`, `Token` (Bin/Arg/Flag/Str) with `kind_class()` + `text()` helpers; `ToolCall`; `ToolStatus` (Pending/Running/Done/Failed); `Mode` (Shell/Agent); `PaletteItem`; `Tab`; `Message`; `TokenBudget`. All public types derive `Clone + PartialEq + Debug` (TokenBudget additionally derives `Copy`). No Dioxus imports, no reactivity — pure data module per CONTEXT D-04 + D-06.
- **Fixture functions (verbatim from UI-SPEC tables and app.jsx):**
  - `demo_blocks()` → 10 blocks: b1 `ironhermes doctor` cmd, b2 doctor 10-line output, b3 `git diff --stat` cmd, b4 git stat success, b5 Hermes AI reply, b6 cargo type-error E0282, b7..b10 four `is-tool` blocks covering every `ToolStatus` variant (read_file/Pending, edit_file/Running, search/Done, compile/Failed)
  - `demo_messages()` → 5 side-panel messages (user → hermes tool-call → hermes reply → user → hermes tool-call) — verbatim from app.jsx `seedMessages` lines 109-118 including the `PROMPT_CACHE` nit copy
  - `demo_palette_items()` → 10 palette rows (6 slash + 4 workflow) verbatim from app.jsx `PALETTE_ITEMS` lines 12-48 including kbd shortcut arrays (`["⌘","I"]`, `["⌘","K"]`, `["⌘","Q"]`, `["?"]`)
  - `demo_tabs()` → 3 title-bar tabs (`ironhermes chat` live, `cargo watch` live, `agent · scratch` not-live)
- **assets/scanner-anim.css (31 lines):** New file with `@keyframes wh-scanner-tick` (8 keyframe stops: off → t2 → t1 → lit → t1 → t2 → off → hold; t1/t2 use `color-mix(in oklab, ...)` for the perceptually-uniform fade) + 10 `.wh-scanner.is-active span:nth-child(N)` rules with `animation-delay: -(N-1)*100ms` to phase-shift each cell. Period sourced from `var(--scanner-period, 1800ms)` with hard fallback. Resolves UI-SPEC planner-handoff #1 / SHELL-09 / CONTEXT D-08.
- **src/components/mod.rs rewritten:** Replaced `mod hero;` + `pub use hero::Hero;` with `pub mod warp_hermes;` + `pub mod shell;` + `pub use warp_hermes::WarpHermes;`. The hero re-export is gone but `src/components/hero.rs` is preserved on disk — its deletion is Plan 03-04's responsibility (same commit that swaps `src/app.rs` to render `WarpHermes`).
- **src/components/shell/mod.rs created:** 11 `pub mod` lines (`title_bar`, `sigil`, `block_stream`, `block`, `command_line`, `tool_call`, `input_box`, `agent_panel`, `status_bar`, `scanner`, `command_palette`) + 11 corresponding `pub use foo::Foo;` re-exports. Bodies don't exist yet — Plans 03-02 / 03-03 land them.

## Task Commits

Each task was committed atomically on branch `worktree-agent-a550f20dc6a0bc923`:

1. **Task 1: Write src/state.rs with all shared types and 4 demo fixture functions** — `70c320a` (feat)
2. **Task 2: Author assets/scanner-anim.css resolving the SHELL-09 animation gap** — `9e30c98` (feat)
3. **Task 3: Rewrite src/components/mod.rs and create src/components/shell/mod.rs with module declarations + re-exports** — `03fe6cf` (feat)

_Plan-metadata commit (this SUMMARY.md) is created by the orchestrator after merge._

## Files Created/Modified

- `src/state.rs` — **modified** (placeholder → full Phase 3 data module). Defines 10 public types and 4 fixture functions consumed by every shell primitive in Plans 03-02 / 03-03 / 03-04 via `use crate::state::*;`.
- `assets/scanner-anim.css` — **created**. Standalone CSS file imported by `src/app.rs` in Plan 03-04 (`document::Link { rel: "stylesheet", href: SCANNER_ANIM_CSS }`).
- `src/components/mod.rs` — **modified** (hero re-export → warp_hermes + shell). Stages the new module hierarchy.
- `src/components/shell/mod.rs` — **created**. Declares the 11-primitive shell submodule with PascalCase re-exports. Bodies land in 03-02 / 03-03.

## Decisions Made

- **Owned `String` everywhere in state.rs** — Dioxus 0.7 props require `Clone + PartialEq` and most prop sites need `'static` lifetime; `&str` in struct fields would force lifetimes through every component signature. Owned `String` is the path of least friction (matches RESEARCH "Anti-Patterns" guidance).
- **`Token::text()` returns `&str` via collapsed match arm** — using `Token::Bin(s) | Token::Arg(s) | Token::Flag(s) | Token::Str(s) => s.as_str()` instead of four separate arms keeps the helper small and matches Rust idioms.
- **Doctor block (b2) text uses Rust raw-style string concatenation across `\n`** — multi-line literal preserves the exact 10-line layout (heading + 40-dash rule + 8 status lines, no trailing newline) from UI-SPEC line 235 verbatim.
- **`Block::Tool` blocks (b7..b10) deliberately cover all 4 `ToolStatus` variants** — gives the BlockStream component (Plan 03-02) end-to-end visual coverage of every status icon/color without needing additional fixture data, satisfying CONTEXT D-18's "8-10 fixture blocks" target with usable diversity.
- **scanner-anim.css uses `var(--scanner-period, 1800ms)` fallback** — the design-tokens.css `--scanner-period` variable is loaded via the verbatim Phase 2 port; if a future cascade-reorder change ever evicts it, the animation still runs at 1800 ms instead of breaking. Defensive layering matches the spirit of the Phase 2 "verbatim port" pattern.

## Deviations from Plan

None — plan executed exactly as written. The fixture content matches UI-SPEC tables and app.jsx source-of-truth verbatim (Doctor 10-line block, 5 side-panel messages including the `PROMPT_CACHE` nit copy, 10 palette items in slash-then-workflow order with exact kbd arrays, 3 tabs with `live` flags).

## Issues Encountered

- **Pre-existing worktree-isolation gap (not in plan scope):** `cargo build --features web` initially failed because `assets/favicon.ico` and `assets/tailwind.css` (referenced via `asset!()` in `src/app.rs`) are untracked-but-required runtime artifacts: favicon.ico is `git ls-files`-untracked in the main repo too, and tailwind.css is a `dx serve` build output. Both files exist in the main checkout but were not seeded into the worktree. **Resolution:** Copied both files from the main repo into the worktree to enable Task 1's build verification. The files remain `git status`-untracked (matching main-repo state) and were NOT committed — they're worktree infrastructure parity, not plan deliverables. This is an upstream worktree-seeding gap, surfaced for tracking but not auto-fixed (out of scope per executor `<scope_boundary>` — Rule 3 was not invoked because the failure is not caused by Task 1's changes).
- **Intentional broken-build state after Task 3:** As documented in 03-01-PLAN.md `<verification>` and the orchestrator's `<phase_3_critical_context>`, `cargo build --features web` will fail at the end of this plan because `pub mod warp_hermes;` references `src/components/warp_hermes.rs` (created in Plan 03-04) and `src/components/shell/mod.rs` references 11 primitive files (created in Plans 03-02 / 03-03). This is the expected Wave 1 → Wave 4 hand-off; the canonical green-build gate is Plan 03-04 Task 4. No action needed here.

## User Setup Required

None — Phase 3 has no external services; the IronHermes v1 mandate (no API keys, no network calls) holds.

## Next Phase Readiness

- **Plan 03-02 (Wave 2 — Shell Primitives A):** can `use crate::state::*;` from any new file under `src/components/shell/`. Block enum, Token variants, ToolStatus, ToolCall, CommandLine all carry the documented field shapes. Fixture functions return data ready for visual review against the prototype.
- **Plan 03-03 (Wave 3 — Shell Primitives B):** same — `use crate::state::*;` and `use crate::components::shell::*;` are stable.
- **Plan 03-04 (Wave 4 — WarpHermes composer):** must (a) create `src/components/warp_hermes.rs` defining `WarpHermes`, (b) link `assets/scanner-anim.css` via a new `SCANNER_ANIM_CSS` `Asset` constant in `src/app.rs`, (c) delete `src/components/hero.rs` and swap the `Hero {}` invocation in `src/app.rs` for `WarpHermes {}`. Plan 03-01 leaves all three boundary conditions ready (the const can be added next to FAVICON / MAIN_CSS / etc.).
- **Plan 03-05 (Mobile Shell):** consumes the same `crate::state::*` types — no further data-model work expected.
- **Naming-collision watch:** `Block`, `CommandLine`, `ToolCall` are both data types in `crate::state` and component names in `crate::components::shell`. Plans 03-02 / 03-03 / 03-04 must alias one of the two when both are used in the same scope (typical pattern: `use crate::state::Block as BlockData;` or fully-qualify).

## Threat Surface

Reviewed Plan 03-01's threat model (T-03-01 / T-03-02 / T-03-03 — all `accept` dispositions). No new surface introduced beyond what was modelled:

- assets/scanner-anim.css is checked into git (Tampering already in T-03-01 register, accepted under Phase 2 trust model).
- src/state.rs fixture data is hardcoded prototype-replica copy with no PII / secrets (Information disclosure already in T-03-02, accepted).
- @keyframes runs on the GPU compositor with no Rust runtime cost (DoS already in T-03-03, accepted).

No flags raised.

## Self-Check: PASSED

Verified after writing this summary:

- [x] `src/state.rs` exists (395 lines, ≥ 200 line bound)
- [x] `assets/scanner-anim.css` exists (31 lines, contains `@keyframes wh-scanner-tick` + 10 `animation-delay` rules including `0ms` and `-900ms` boundaries)
- [x] `src/components/mod.rs` exists with `pub mod warp_hermes;` + `pub mod shell;` (no `mod hero`)
- [x] `src/components/shell/mod.rs` exists with 11 `pub mod` + 11 `pub use` lines
- [x] Commit `70c320a` (Task 1 — feat: state.rs) present in `git log`
- [x] Commit `9e30c98` (Task 2 — feat: scanner-anim.css) present in `git log`
- [x] Commit `03fe6cf` (Task 3 — feat: components mod hierarchy) present in `git log`
- [x] `cargo build --features web` exits 0 after Task 1 (Tasks 2 + 3 leave the build intentionally broken — restored by Plan 03-04 Task 4)

---
*Phase: 03-desktop-shell*
*Completed: 2026-05-03*
