---
phase: 03-desktop-shell
plan: 02
subsystem: ui
tags: [dioxus, dioxus-0.7, rust, shell-primitives, pure-static, leaf-components]

# Dependency graph
requires:
  - phase: 03-desktop-shell
    plan: 01
    provides: "src/state.rs with Token, ToolStatus, TokenBudget types; src/components/shell/mod.rs with pub mod sigil/scanner/command_line/tool_call/status_bar declarations; assets/scanner-anim.css with .wh-scanner.is-active span @keyframes"
provides:
  - "src/components/shell/sigil.rs: pub fn Sigil(size: u16) -> Element rendering the wh-sigil 'IH' stamp with size-derived font-size"
  - "src/components/shell/scanner.rs: pub fn Scanner(active: bool) -> Element rendering 10 sibling spans inside .wh-scanner with conditional is-active class (pure-CSS animation, no setInterval)"
  - "src/components/shell/command_line.rs: pub fn CommandLine(tokens, time, cwd, glyph) -> Element rendering the .wh-cmdline row with per-token .wh-cmd-{kind} classes plus .wh-cmd modifier on bin tokens"
  - "src/components/shell/tool_call.rs: pub fn ToolCall(name, args_summary, status) -> Element rendering the yellow-bordered .wh-toolcall card with status-driven color and 4 status texts"
  - "src/components/shell/status_bar.rs: pub fn StatusBar(mode, model, provider, tokens, scanner_active, hint) -> Element composing 4 .wh-pill spans + Scanner child + .wh-hint"
affects: [03-03-shell-primitives-B, 03-04-warp-hermes-composer, 03-05-mobile-shell]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Pure-static Dioxus 0.7 components — every primitive uses #[component] + plain owned props (size: u16, name: String, status: ToolStatus, active: bool, etc.); zero use_signal/use_memo/use_resource (CONTEXT D-06)"
    - "Name-collision avoidance: state.rs defines `CommandLine` and `ToolCall` data structs; the same-named components in components/shell/ import only the supporting types (Token, ToolStatus) to keep both names in scope without alias gymnastics"
    - "Match-on-reference (`match &status`) pattern in tool_call.rs lets the component dispatch on a non-Copy enum without modifying state.rs's derive list"
    - "Scanner pulls 100% of its animation from CSS (assets/scanner-anim.css from Plan 03-01) — no Rust state, no timers, no async; the component's only job is to render 10 sibling spans plus the is-active toggle class"

key-files:
  created:
    - "src/components/shell/sigil.rs"
    - "src/components/shell/scanner.rs"
    - "src/components/shell/command_line.rs"
    - "src/components/shell/tool_call.rs"
    - "src/components/shell/status_bar.rs"
  modified: []

key-decisions:
  - "Imported only Token (not state::CommandLine) into components/shell/command_line.rs to keep the component name unshadowed by the state-side data struct — same trick for ToolStatus in tool_call.rs"
  - "Used `match &status` in ToolCall instead of adding `Copy` to ToolStatus in state.rs — keeps Plan 03-01's state.rs derive list (`Clone + PartialEq + Debug`) untouched"
  - "Rendered Scanner unconditionally inside StatusBar (not gated on scanner_active) — mirrors the prototype where scanner_active only flips the is-active class, not whether the cells exist"
  - "Used `super::scanner::Scanner` instead of `crate::components::shell::Scanner` in status_bar.rs — keeps imports localized to the shell submodule and avoids the long crate path"

patterns-established:
  - "Pattern (Phase 3): every leaf primitive starts with `use dioxus::prelude::*;` and (when consuming state types) `use crate::state::{TypeName};`. Function annotated `#[component]`, body is one `rsx! { ... }` block. Plain `let` bindings for derived values appear ABOVE the rsx block (e.g. `let font_size = ...` in sigil, `let used_k = ...` in status_bar)."
  - "Pattern (Phase 3): conditional class via second `class:` attribute (e.g. `class: if active { \"is-active\" }`) — Dioxus concatenates multiple class attributes at render time. Used in scanner.rs and command_line.rs."
  - "Pattern (Phase 3): aria/data attributes use quoted-string keys per RESEARCH Pitfall 6 (`\"aria-hidden\": \"true\"`)."

requirements-completed: [SHELL-04, SHELL-05, SHELL-08, SHELL-09]

# Metrics
duration: ~5min
completed: 2026-05-03
---

# Phase 3 Plan 02: Shell Primitives A — Leaf Components Summary

**Lands the 5 leaf shell primitives (Sigil, Scanner, CommandLine, ToolCall, StatusBar) as pure-static Dioxus 0.7 components — every value comes from plain owned props or hardcoded CSS class strings, satisfying CONTEXT D-06's no-reactivity rule and unblocking Plan 03-03's composites which depend on these as child invocations.**

## Performance

- **Duration:** ~5 min
- **Tasks:** 3 / 3
- **Files created:** 5 (all under `src/components/shell/`)
- **Files modified:** 0
- **Commits:** 3 atomic per-task commits

## Accomplishments

- **`src/components/shell/sigil.rs`** — `pub fn Sigil(size: u16) -> Element`. Renders `<span class="wh-sigil" style="width: {size}px; height: {size}px; font-size: {font_size}px;">IH</span>` where `font_size = (size as f32 * 0.46).round() as u16`. Port of `shell.jsx` lines 53-59. Used by title_bar (size 18) and agent_panel (size 20) in Plan 03-03.
- **`src/components/shell/scanner.rs`** — `pub fn Scanner(active: bool) -> Element`. Renders `<span class="wh-scanner [is-active]" aria-hidden="true">` containing exactly 10 sibling `<span>"░"</span>` elements via `for i in 0..10`. The off-state glyph `░` (U+2591) is the same on every cell; the CSS `@keyframes wh-scanner-tick` rule from Plan 03-01's `assets/scanner-anim.css` animates the cell COLOR using staggered `:nth-child` `animation-delay` per UI-SPEC. Zero `use_signal`, zero `setInterval`, zero `gloo_timers` — fully resolves CONTEXT D-08.
- **`src/components/shell/command_line.rs`** — `pub fn CommandLine(tokens: Vec<Token>, time: Option<String>, cwd: Option<String>, glyph: Option<String>) -> Element`. Renders the prototype's `.wh-cmdline` row: dim cwd label (default `~/projects/ironhermes`) + prompt glyph (default `❯`) + flex-1 span containing per-token `<span class="wh-cmd-{kind}">` (with the additional `wh-cmd` modifier class on `Token::Bin` for the stronger fg-strong + 700-weight styling) interspersed with literal-space separators + optional `.wh-cmd-time`. Port of `shell.jsx` lines 115-130.
- **`src/components/shell/tool_call.rs`** — `pub fn ToolCall(name: String, args_summary: String, status: ToolStatus) -> Element`. Renders the `.wh-toolcall` yellow-bordered card with `Tool:` dim label + bold name + status text + status color. Status mapping: `Done → "[OK]" / var(--success)`, `Pending → "pending…" / var(--warn)`, `Running → "running…" / var(--warn)`, `Failed → "failed" / var(--danger)`. Args summary is rendered in a `<pre>` only when non-empty. Port of `shell.jsx` lines 132-147.
- **`src/components/shell/status_bar.rs`** — `pub fn StatusBar(mode: String, model: String, provider: String, tokens: TokenBudget, scanner_active: bool, hint: String) -> Element`. Renders the bottom bar with 4 colored pills (`.wh-pill` × 4 with inline `color: var(--pill-N)` for N=0..3 — mode, model, provider, tokens) separated by 4 `.wh-sep` middots, followed by an unconditional `Scanner { active: scanner_active }` invocation, then the right-aligned `.wh-hint`. Token-budget pill formats as `{used_k:.1}K/{max_k}K ({pct}%)` (e.g. `12.3K/128K (10%)` for `TokenBudget { used: 12300, max: 128000 }`). Port of `shell.jsx` lines 30-50.

## Task Commits

Each task was committed atomically on branch `worktree-agent-a07aa8e9de548f9c9`:

1. **Task 1: Create `sigil.rs` and `scanner.rs` (zero-dependency leaves)** — `9c79d5a` (feat)
2. **Task 2: Create `command_line.rs` and `tool_call.rs` (state-consuming leaves)** — `3caa54f` (feat)
3. **Task 3: Create `status_bar.rs` (composes Scanner)** — `1a7c9f0` (feat)

_Plan-metadata commit (this SUMMARY.md) is created next; STATE.md / ROADMAP.md updates are deferred to the orchestrator after wave merge per parallel-executor protocol._

## Files Created/Modified

- `src/components/shell/sigil.rs` — **created** (18 lines). Component: `Sigil`.
- `src/components/shell/scanner.rs` — **created** (24 lines). Component: `Scanner`.
- `src/components/shell/command_line.rs` — **created** (43 lines). Component: `CommandLine`. Imports `crate::state::Token`.
- `src/components/shell/tool_call.rs` — **created** (47 lines). Component: `ToolCall`. Imports `crate::state::ToolStatus`.
- `src/components/shell/status_bar.rs` — **created** (44 lines). Component: `StatusBar`. Imports `crate::state::TokenBudget` and `super::scanner::Scanner`.

No files modified. The Plan 03-01 `src/components/shell/mod.rs` already declared `pub mod sigil; pub mod scanner; pub mod command_line; pub mod tool_call; pub mod status_bar;` (and the corresponding `pub use` re-exports), so the new files are picked up automatically.

## Decisions Made

- **Name-collision strategy: import only the supporting types, not the same-named structs.**
  - `command_line.rs` does `use crate::state::Token;` — NOT `use crate::state::CommandLine;`. The component's own name `CommandLine` then shadows nothing because the state-side data struct is never brought into the file's namespace.
  - `tool_call.rs` does `use crate::state::ToolStatus;` — NOT `use crate::state::ToolCall;`. Same reason.
  - This is the simplest possible resolution to the name-collision warning flagged in 03-01-SUMMARY.md `key-decisions`. No type aliases needed; consumers in Plan 03-03 / 03-04 that need both will alias one (`use crate::state::Block as BlockData;` etc.).

- **Match-on-reference (`match &status`) instead of adding `Copy` to ToolStatus.** Plan 03-01's state.rs derives only `Clone + PartialEq + Debug` on `ToolStatus`. The component's `match status { ToolStatus::Done => ... }` would consume the prop on the first match, requiring two clones to dispatch on it twice (once for color, once for text). Switching to `match &status { ToolStatus::Done => ... }` borrows the enum and Rust's match-ergonomics handles the unit variants without `ref` bindings or destructuring. State.rs stays untouched — no derive-list churn.

- **Scanner is rendered unconditionally inside StatusBar.** The prototype's React StatusBar always emits the 10 cells; the `scannerActive` prop only controls the `is-active` class on the parent. This implementation mirrors that — `Scanner { active: scanner_active }` is always invoked (never conditionally rendered), and the Scanner component itself emits the cells regardless of `active`. The CSS animation is gated by the `is-active` class only.

- **`super::scanner::Scanner` import in status_bar.rs.** The plan offered both `super::scanner::Scanner` and `crate::components::shell::Scanner` as acceptable. Chose `super::scanner` to keep imports localized within the shell submodule — shorter path and avoids the redundant `components::shell` traversal when the file is already inside that path.

- **Cell glyph is the literal off-state `░` (U+2591) on every Scanner cell.** Per UI-SPEC line 326, the four scanner glyphs are `░ ▒ ▓ █` representing different brightness levels, but `assets/scanner-anim.css` (Plan 03-01) animates the COLOR of each cell, not the glyph. All 10 cells render `░`; the `@keyframes wh-scanner-tick` rule cycles each cell's color through dim → t2 → t1 → lit → t1 → t2 → dim with staggered `:nth-child(N) { animation-delay: -((N-1)*100ms); }` to create the bouncing knight-rider effect. This matches the plan's explicit rsx skeleton.

## Deviations from Plan

None — all 3 tasks executed exactly as written. Each acceptance-criteria grep listed in the plan was verified post-write before commit. Plan-level verification (`find src/components/shell -name '*.rs' -not -name 'mod.rs' | wc -l` = 5; total `#[component]` annotations = 5; total reactivity hits = 0) all pass.

## Issues Encountered

- **Intentional broken-build state continues.** As documented in 03-02-PLAN.md `<verification>` and 03-01-SUMMARY.md `Issues Encountered`, `cargo build --features web` is still expected to fail at the end of this plan because `src/components/shell/mod.rs` declares `pub mod title_bar; pub mod block; pub mod block_stream; pub mod input_box; pub mod agent_panel; pub mod command_palette;` for files that do not yet exist (Plan 03-03's job) and `src/components/mod.rs` declares `pub mod warp_hermes;` for a file that does not yet exist (Plan 03-04's job). Verification of THIS plan's files used `cargo check --features web 2>&1 | grep <file>` to confirm the 5 new files parse without errors — the only diagnostics for them are "unused import" warnings on the corresponding `pub use` lines in `shell/mod.rs`, which are expected because their consumers (TitleBar, Block, BlockStream, AgentPanel, WarpHermes) don't exist yet. The build returns to green at Plan 03-04 Task 4.

## User Setup Required

None — Phase 3 has no external services and these components are pure-static.

## Next Phase Readiness

- **Plan 03-03 (Wave 3 — Shell Primitives B):** can directly invoke each of the 5 new components without alias gymnastics:
  - `TitleBar` will render `Sigil { size: 18_u16 }`
  - `Block` (component) will render `CommandLine { tokens: ..., ... }` (the data type `crate::state::Block` with variant `Block::Cmd` will need `use crate::state::Block as BlockData;` per 03-01-SUMMARY guidance because Block-the-component shares the name)
  - `Block` (component) will also render `ToolCall { name: ..., args_summary: ..., status: ... }` with the same name-collision pattern needed for the Block::Tool variant arm
  - `AgentPanel` will render `Sigil { size: 20_u16 }` and `ToolCall { ... }` for tool-call messages
- **Plan 03-04 (Wave 4 — WarpHermes composer):** will render `StatusBar { mode: "Chat".to_string(), model: "claude-sonnet-4".to_string(), provider: "anthropic".to_string(), tokens: TokenBudget { used: 12300, max: 128000 }, scanner_active: true, hint: "/help · ⌃C cancel · ⌘K palette".to_string() }` per UI-SPEC line 217 verbatim copy.
- **Naming-collision watch (carried from 03-01-SUMMARY):** still applies for `Block` and `ToolCall` (data-struct vs component-fn). `CommandLine` collision is resolved at this layer (the component imports only `Token`, not the data struct of the same name). Plan 03-03's `block.rs` will need `use crate::state::Block as BlockData;`.

## Threat Surface

Reviewed 03-02-PLAN.md threat register (T-03-04 Tampering / T-03-05 Information disclosure / T-03-06 Spoofing — all `accept` dispositions). No new surface introduced beyond what was modelled:

- All component props are plain owned types from project-controlled fixtures (state.rs `demo_blocks`/`demo_messages`/`demo_palette_items`/`demo_tabs`); no user input crosses these props in Phase 3 (T-03-04 accept).
- `args_summary` in ToolCall is rendered as text content inside `<pre>` — Dioxus's rsx text interpolation HTML-escapes by default, so the JSON-ish strings in fixtures cannot inject markup. No information-disclosure risk because the fixtures are hardcoded prototype-replica copy (T-03-05 accept).
- Sigil's `"IH"` is a literal string; no user-controlled identity rendering possible (T-03-06 accept).

No flags raised.

## Self-Check: PASSED

Verified after writing this summary:

- [x] `src/components/shell/sigil.rs` exists; commit `9c79d5a` present in `git log`
- [x] `src/components/shell/scanner.rs` exists; commit `9c79d5a` present in `git log`
- [x] `src/components/shell/command_line.rs` exists; commit `3caa54f` present in `git log`
- [x] `src/components/shell/tool_call.rs` exists; commit `3caa54f` present in `git log`
- [x] `src/components/shell/status_bar.rs` exists; commit `1a7c9f0` present in `git log`
- [x] All 5 files contain `#[component]` annotation
- [x] All 5 files declare `pub fn ComponentName(...) -> Element`
- [x] Zero `use_signal` / `use_memo` / `use_resource` / `use_effect` / `onclick` / `oninput` / `onkeydown` / `onfocus` / `onblur` calls across all 5 files
- [x] `find src/components/shell -name '*.rs' -not -name 'mod.rs' | wc -l` returns 5
- [x] `cargo check --features web` shows no diagnostics on the 5 new files (only pre-existing unused-import warnings on the `pub use` lines in `shell/mod.rs` for primitives whose composites haven't landed yet)
- [x] STATE.md NOT modified (parallel-executor protocol — orchestrator owns post-wave-merge state updates)
- [x] ROADMAP.md NOT modified (same reason)

---
*Phase: 03-desktop-shell*
*Plan: 02 — Shell Primitives A (leaf components)*
*Completed: 2026-05-03*
