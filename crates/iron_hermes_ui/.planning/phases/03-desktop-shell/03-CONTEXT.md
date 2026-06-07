# Phase 3: Desktop Shell - Context

**Gathered:** 2026-05-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Render the full WarpHermes desktop/web shell as a pixel-perfect static port of the prototype layout. Concretely: implement the title bar (macOS traffic lights, tab strip, ⌘K shortcut display), the scrollable block stream rendering all five stripe types (`is-cmd`, `is-out`, `is-ai`, `is-ok`, `is-err`) plus a sixth tool-call block kind, the inner `CommandLine` token spans (bin/arg/flag) and `ToolCall` row (name/args summary/status), the input box with mode glyph and accent focus ring, the agent side panel at right (360px, `data-agent="right"` default), the status bar with rotating dot-pills and the always-active CSS `@keyframes` scanner cells, and the command palette overlay rendered open by default for visual UAT.

What this phase does NOT do: wire keyboard shortcuts, hook up onclick handlers, run mock data through `runShell`/`runAgent`, animate the scanner conditionally on submission, parse markdown for `is-ai` blocks, switch themes/density/block-style/agent-layout via the data-attributes, or render the mobile shell. Those live in Phases 4 (interactions + data layer), 5 (tweaks + theming), and 6 (mobile). Phase 3 is visuals-only — `use_signal` is forbidden in this phase's components; every value is hardcoded in rsx and Phase 4 introduces reactivity as one cohesive change.

</domain>

<decisions>
## Implementation Decisions

### Component Decomposition
- **D-01:** Mirror `warp2ironhermes/project/app/shell.jsx` 1:1. Create `src/components/shell/` with one file per primitive: `title_bar.rs`, `block.rs`, `command_line.rs`, `tool_call.rs`, `input_box.rs`, `agent_panel.rs`, `status_bar.rs`, `scanner.rs`, `command_palette.rs`. Top-level composer is `src/components/warp_hermes.rs`. The shell-jsx-to-rust mapping is the rule for every file path the planner emits.
- **D-02:** Add a `block_stream.rs` wrapper component (one extension beyond shell.jsx) so the scrollback container styling and `Vec<Block>` ownership live in one file rather than inline in `warp_hermes.rs`.
- **D-03:** Delete `src/components/hero.rs` entirely. Phase 2's brand stub was always a temporary placeholder (per Phase 2 D-03). `app.rs` swaps `Hero {}` for `WarpHermes {}` in the same `rsx!` slot. Module declarations in `components/mod.rs` change from `pub mod hero;` to `pub mod warp_hermes; pub mod shell;` (and `shell/mod.rs` re-exports the primitives).
- **D-04:** Promote `src/state.rs` (Phase 1 placeholder) to hold every shared shell type: `Block` (enum, 6 variants), `BlockKind` (if needed for stripe class lookups), `Mode { Shell, Agent }`, `CommandLine`, `Token`, `ToolCall`, `ToolStatus`, `PaletteItem`, plus the Phase 3 fixture function `pub fn demo_blocks() -> Vec<Block>`. Single import path: `use crate::state::*` from every shell primitive.
- **D-05:** Asset constants colocate with the consuming primitive (Phase 2 pattern, D-08 from Phase 2 CONTEXT). The planner moves `WORDMARK_SVG` from `hero.rs` into `title_bar.rs` (the prototype renders the wordmark in the title bar). `IH_SHIELD_PNG` lands in whichever primitive uses it after the planner reads `shell.jsx` (likely `title_bar.rs` or `agent_panel.rs`). If `scanner.svg` (Phase 2 deferred) is referenced by the prototype's status bar, copy it now into `assets/` and declare the const in `status_bar.rs`; if scanner cells are unicode-only (`░ ▒ ▓ █`), skip the copy entirely. **Planner's call after reading prototype.**

### State Scaffolding Scope
- **D-06:** Phase 3 is **pure static rendering**. No `use_signal`, no `use_memo`, no `use_resource` calls anywhere in shell components. Every prop value is a hardcoded constant in `rsx!`. Phase 4 introduces signals, mocks, and event handlers as one cohesive change. This is the canonical Phase 3/4 boundary — the planner must reject any task that adds reactivity.
- **D-07:** Component props are **plain owned/borrowed values**: `Block { data: Block }`, `InputBox { mode: Mode }`, `BlockStream { blocks: Vec<Block> }`, etc. All shared types in `src/state.rs` derive `Clone + PartialEq` (Dioxus prop bound). Phase 4 will refactor specific props to `Signal<T>` / `ReadOnlySignal<T>` where reactivity is needed (input value, palette open, mode, block stream); the prop-signature churn is acceptable because Phase 4 already touches every interactive component.
- **D-08:** Scanner is rendered with the `is-active` class **hardcoded** in Phase 3, so the CSS `@keyframes` runs continuously on page load. This lets SHELL-09 be visually verified during Phase 3 UAT (the 10-cell knight-rider through `░ ▒ ▓ █` glyphs). Phase 4 inverts this: the default class is inactive, and `is-active` is added via signal toggle for ~1400ms post-submission.
- **D-09:** Input box is a `<textarea>` with the prototype's class names (e.g., `wh-input-textarea`); auto-grow behavior is inherited from `assets/warp-ih.css`. If the prototype implements auto-grow with CSS only (e.g., `field-sizing: content` or the grid-row trick), it works in Phase 3 untouched. If the prototype relies on JS scrollHeight measurement, Phase 4 adds the equivalent Rust hook on `oninput`. Planner reads `warp-ih.css` and `shell.jsx` to confirm which.

### Block Data Model
- **D-10:** Blocks are a single enum with kind-specific variant fields:
  ```rust
  enum Block {
      Cmd { command: CommandLine },
      Out { text: String },
      Ai { markdown: String },
      Ok { message: String },
      Err { message: String },
      Tool { call: ToolCall },
  }
  ```
  Six variants. The first five map to the SHELL-02 stripe types (`is-cmd`, `is-out`, `is-ai`, `is-ok`, `is-err`); the sixth is for SHELL-05 tool-call blocks even though they aren't enumerated in the SHELL-02 stripe list. Block enum derives `Clone + PartialEq + Debug`.
- **D-11:** The `Block` component matches on the enum variant and composes inner content via sibling components. Outer chrome (stripe class, hover-action button row, copy/rerun/share affordances) lives in `block.rs`. For `Cmd`, the body renders `<CommandLine tokens={command.tokens} />`. For `Tool`, the body renders `<ToolCall call={call} />`. For `Out`/`Ai`/`Ok`/`Err`, the body renders the string content directly inside `block.rs` (no separate file — these are simple text variants).
- **D-12:** `CommandLine` is `struct CommandLine { tokens: Vec<Token> }` with `enum Token { Bin(String), Arg(String), Flag(String) }`. Preserves prototype ordering (e.g., `git status --porcelain` → `[Bin("git"), Arg("status"), Flag("--porcelain")]`). The component renders one `<span>` per token with class `is-bin` / `is-arg` / `is-flag` driving the prototype's distinct color rules in `warp-ih.css`.
- **D-13:** `ToolCall` is `struct ToolCall { name: String, args_summary: String, status: ToolStatus }` with `enum ToolStatus { Pending, Running, Done, Failed }`. Each status maps to a CSS class so the four states render distinctly. The Phase 3 fixture exercises all four.
- **D-14:** `BlockStream` is its own component (`block_stream.rs`) accepting `blocks: Vec<Block>` as an owned prop. Iterates with `for block in blocks` in rsx, emits `<Block data={block} />` per item. Phase 4 swaps the prop to `Signal<Vec<Block>>`. The wrapper file is the home of the `.wh-stream` scroll container styling glue.
- **D-15:** Hover affordance reveal is **pure CSS `:hover`** — copy/rerun/share buttons render in every block's rsx unconditionally; visibility is driven by `.wh-block:hover .wh-block-actions { opacity: 1 }` (or equivalent) in the ported `warp-ih.css`. Zero Rust state. Phase 4 adds onclick handlers; the visibility approach never changes.
- **D-16:** `is-ai` content renders as **plain text** in Phase 3 (preserve newlines via `<pre>` wrapper or `white-space: pre-wrap` styling). No markdown parser in Phase 3. Phase 4 adds `pulldown-cmark` or `comrak` as a dependency when the personality preset mock-reply tables land — that's the right phase for it because Phase 4 owns the agent-reply rendering pipeline.

### Phase 3 Demo Composition
- **D-17:** Phase 3 fixtures live in `pub fn demo_blocks() -> Vec<Block>` inside `src/state.rs`. `WarpHermes` calls `demo_blocks()` and passes the Vec to `BlockStream`. Phase 4 either replaces the call site with mock-driven blocks or repurposes `demo_blocks()` as a dev/test fixture.
- **D-18:** `demo_blocks()` returns approximately 8–10 blocks: one `Cmd` (with a CommandLine showing bin + args + flags), one `Out` (multiline text), one `Ai` (multi-paragraph plain text), one `Ok`, one `Err`, plus four `Tool` blocks one per `ToolStatus` variant (Pending, Running, Done, Failed). Verifies SC-2/SC-3/SHELL-05 on a single screen scroll.
- **D-19:** Command palette overlay renders **open by default** in Phase 3. This lets SC-6 (palette layout, slash commands list, workflow items) be verified during Phase 3 UAT. Phase 4 inverts the default to closed and wires `⌘K` (open) / `Esc` (close) toggles.
- **D-20:** Title-bar tabs and status-bar dot-pills (mode · model · provider · token count · scanner) use **prototype-default content verbatim**. The planner reads `app.jsx` to extract the prototype's default tab labels and status pill values, then hardcodes them in `title_bar.rs` and `status_bar.rs` rsx. This maximizes visual fidelity during the side-by-side review against the prototype HTML.

### Claude's Discretion
- **`is-tool` stripe color:** Block enum has a Tool variant not mapped to one of the five named stripes. Planner picks an existing token (likely `--accent-secondary` magenta or the same accent as `is-ai`) consistent with the prototype's tool-call visual treatment.
- **Scanner SVG copy:** Per D-05, copy `warp2ironhermes/project/ironhermes/assets/scanner.svg` into `assets/` only if the prototype's status-bar references it. Read shell.jsx and warp-ih.css to decide.
- **Agent panel default content:** The side panel renders at 360px right with `data-agent="right"` per SHELL-07. What it shows in Phase 3 (placeholder text? sample agent thoughts? mock memory list?) — planner reads `shell.jsx <AgentPanel>` and copies whatever default the prototype ships, or a minimal placeholder if the prototype's content is data-driven.
- **Sigil primitive handling:** `shell.jsx` has a `Sigil` primitive (mode glyph). Per D-01 it gets its own file `sigil.rs` if it's a standalone component in the prototype, OR it's inlined into `input_box.rs` if `Sigil` is just a `<span>` with one of two glyphs. Planner reads shell.jsx and decides.
- **Hover-action button glyphs:** Copy (⎘), rerun (↻), share (↗) are unicode glyphs per the prototype. Planner uses the prototype's exact glyphs and aria-labels.
- **`<pre>` vs `<div>` for `Ai` content:** D-16 says preserve newlines; planner picks the structurally-correct element for the markdown-stub variant.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Project-level governance
- `.planning/PROJECT.md` — core value (pixel-perfect to prototype is the primary failure mode), Constraints section (no `cx`/`Scope`/`use_state`; multi-platform features; no API keys), Key Decisions table (separate Desktop/Mobile components; mock data only in v1; scanner via CSS @keyframes)
- `.planning/REQUIREMENTS.md` §"Desktop/Web Shell (`WarpHermes`)" — SHELL-01 through SHELL-10 acceptance criteria
- `.planning/ROADMAP.md` §"Phase 3: Desktop Shell" — goal statement and 6 success criteria (title bar, block types + hover, CommandLine + ToolCall, input + focus, side panel + scanner, palette overlay)
- `.planning/phases/02-design-system/02-CONTEXT.md` — Phase 2 locked decisions (D-08 scanner via CSS @keyframes, asset colocation pattern, brand asset names) the planner inherits unconditionally
- `CLAUDE.md` — Dioxus 0.7 conventions (no `cx`/`Scope`/`use_state`; signal borrows must not span `.await`; component functions PascalCase + `#[component]`)
- `AGENTS.md` — Dioxus 0.7 API reference for `use_signal`, `use_memo`, `#[component]`, `rsx!`, `asset!()`, props, conditional rendering

### Prototype source of truth (READ-ONLY — never compile from here)
- `warp2ironhermes/project/app/app.jsx` — primary `WarpHermes` shell with state model, layout, default tab labels, status-bar dot-pill values, palette items, side panel content. **Planner extracts demo content from here.**
- `warp2ironhermes/project/app/shell.jsx` — presentational primitives: `Scanner`, `StatusBar`, `Sigil`, `TitleBar`, `Block`, `CommandLine`, `ToolCall`, `InputBox`, `AgentPanel`, `CommandPalette`. **The 1:1 source for the `src/components/shell/*.rs` mapping (D-01).**
- `warp2ironhermes/project/app/frames.jsx` — wrapper frames; relevant only for confirming `WarpHermes` ≠ `WarpHermesMobile` (Phase 6 concern, not this phase)
- `warp2ironhermes/project/Warp × IronHermes.html` — reference HTML; the side-by-side visual UAT target for Phase 3
- `warp2ironhermes/project/styles/warp-ih.css` — the layout CSS already ported to `assets/warp-ih.css` in Phase 2 (verbatim). Source of class names for blocks (`wh-block`, `is-cmd`, `is-out`, etc.), input (`wh-input-*`), palette (`wh-palette-*`), side panel (`wh-agent-*`), status bar (`wh-status-*`), scanner (`wh-scanner-*`)
- `warp2ironhermes/project/ironhermes/colors_and_type.css` — design tokens already ported to `assets/design-tokens.css` in Phase 2; source of `--accent-primary`, stripe colors, brand color, font stacks

### Phase 1–2 deliverables (consumed by Phase 3)
- `src/app.rs` — current home of CSS `document::Link` chain (`MAIN_CSS`, `DESIGN_TOKENS_CSS`, `WARP_IH_CSS`, `TAILWIND_CSS`). Phase 3 swaps `Hero {}` → `WarpHermes {}` in this file's rsx; the link chain stays untouched.
- `src/components/hero.rs` — **target for deletion** in Phase 3. `WORDMARK_SVG` and `IH_SHIELD_PNG` consts migrate to `title_bar.rs` (and possibly `agent_panel.rs`) per D-05.
- `src/components/mod.rs` — Phase 3 replaces `pub mod hero;` with `pub mod warp_hermes; pub mod shell;`
- `src/state.rs` — currently a Phase 1 placeholder. Phase 3 promotes it to hold every shared shell type plus `demo_blocks()`.
- `src/main.rs` — slim entry point; no changes needed in Phase 3.
- `assets/design-tokens.css`, `assets/warp-ih.css`, `assets/main.css`, `assets/tailwind.css` — design system already loaded by Phase 2; Phase 3 components consume the class names directly.
- `assets/fonts/IoskeleyMono-*.woff2` — already copied in Phase 2 Plan 02-01; Phase 3 gets correct typography for free.
- `assets/wordmark.svg`, `assets/ih-shield.png` — already present; Phase 3 just changes which component declares the asset constant.
- `clippy.toml` — Dioxus 0.7 signal-borrow safety rules; relevant only when Phase 4 lands signals (Phase 3 has no async, no .await).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src/app.rs` already establishes the `document::Link` cascade (Phase 2 D-01). Phase 3 does not touch this file's CSS chain — it only swaps the rendered child component. The fix for the cascade-order deviation noted in Phase 2's accumulated context (`tailwind.css` between `main.css` and `design-tokens.css`) is locked and the planner inherits it.
- `src/components/hero.rs` is the natural starting point for migrating `WORDMARK_SVG` / `IH_SHIELD_PNG` constants. The existing brand-stub layout (centered wordmark + shield) becomes Phase 3 dead code on deletion; the asset paths and the constant declarations are the only artifacts that survive (they move to `title_bar.rs`).
- `src/components/mod.rs` is currently `pub mod hero;` — single-line module file. Phase 3 expands this to declare the `warp_hermes` and `shell` submodules.
- `src/state.rs` and `src/platform/mod.rs` are placeholder stubs from Phase 1. Phase 3 promotes `state.rs`; `platform/mod.rs` stays empty (mobile-shell territory in Phase 6).

### Established Patterns
- Asset constants live at module top in SCREAMING_SNAKE_CASE with the `asset!("/assets/...")` macro. New assets in Phase 3 follow this exactly: e.g., `const SCANNER_SVG: Asset = asset!("/assets/scanner.svg");` if scanner.svg lands in `status_bar.rs`.
- Component functions are `PascalCase`, annotated `#[component]`, return `Element`. RSX blocks are 4-space-indented inside `rsx! { ... }`.
- Module declarations re-export public components: `components/mod.rs` does `pub use hero::Hero;` style. Phase 3 follows the same pattern: `shell/mod.rs` re-exports every primitive (`pub use title_bar::TitleBar;` etc.) so consumers import from `crate::components::shell::TitleBar` rather than `crate::components::shell::title_bar::TitleBar`.
- CSS class names come straight from the prototype — the ported `warp-ih.css` defines every `wh-*` and `is-*` class. Phase 3 uses these literal class strings; do not invent new class names.

### Integration Points
- `src/app.rs#App` currently renders `Hero {}`. Phase 3's only change here is swapping that to `WarpHermes {}`. The `document::Link` chain, asset constants, and surrounding rsx structure stay identical.
- `src/state.rs` (currently a `pub fn placeholder() {}` stub or empty file) becomes the import target for every shell primitive. Each primitive does `use crate::state::*;` at the top.
- The browser's window viewport is the layout target. Default `data-agent="right"` (per Phase 5 will switch this) means the side panel is the right 360px column always-visible.
- No external integration: no fullstack server, no API, no platform-specific code. Phase 3 ships one binary: web (default Cargo feature) plus desktop and mobile that compile from the same source. The three-platform compile gate (`cargo build --features web` / `--features desktop` / `--features mobile`) from Phase 1 stays the canonical phase gate.

</code_context>

<specifics>
## Specific Ideas

- **Side-by-side visual review against `Warp × IronHermes.html` is the Phase 3 UAT.** Per the project STATE.md todo list ("After Phase 3: side-by-side visual review of desktop shell against `warp2ironhermes/project/Warp × IronHermes.html`"). The planner builds a UAT script that walks the reviewer through every SC-1..SC-6 criterion comparing browser tabs.
- **Pure-static render is the discipline.** Resist the temptation to add `use_signal` for "just one toggle". Phase 4 owns reactivity. If a reviewer says "the input doesn't accept text" — that's expected; Phase 4 wires it.
- **Prototype-fidelity outranks code aesthetics.** If the prototype uses three nested divs where one would do, port three nested divs. Visual drift is the project's primary failure mode (per CLAUDE.md / PROJECT.md core value).
- **Block enum derives Clone + PartialEq + Debug.** Dioxus props need Clone+PartialEq; Debug is for dev-time `{:?}` introspection during the inevitable "why is this block rendering wrong" debugging.
- **The `Sigil` primitive may collapse into `InputBox`.** If shell.jsx defines Sigil as a one-line glyph wrapper, port it inline rather than create a 5-line `sigil.rs` file. D-01's "1:1" rule has discretion for trivial primitives; planner judges.
- **`scanner.svg` may not be needed at all.** The 10 scanner cells are unicode glyphs (`░ ▒ ▓ █`) per SHELL-09 — the SVG might be a separate icon (e.g., the small status-bar pill icon) or unused entirely. Planner confirms by reading shell.jsx + warp-ih.css before copying.

</specifics>

<deferred>
## Deferred Ideas

- **Markdown rendering for `is-ai` blocks** — Phase 3 renders agent reply content as plain text (D-16). Phase 4 adds a markdown crate (`pulldown-cmark` or `comrak`) when the personality preset mock-reply tables land. Phase 4 plan should call out the dependency add explicitly.
- **Reactive signal scaffolding** — Every `Signal<T>` / `ReadOnlySignal<T>` / `use_signal` call in shell components is Phase 4 territory (D-06). Phase 4 plan starts with a "introduce signals" task that refactors props before adding handlers.
- **Keyboard handlers** — `⌘K`, `Esc`, `⌥M`, `Enter`/`Shift+Enter`, `↑`/`↓` are all Phase 4 (KBD-01..KBD-04, KBD-06). Phase 3 must not register any onkeydown / onkeyup / onkeypress.
- **`runShell` and `runAgent` mocks** — MOCK-02/MOCK-03 mocks with prototype-matching timings (600 ms / 400 ms / 1400 ms) are Phase 4. The Phase 3 fixture is hardcoded; no async tasks, no `use_resource`, no `gloo_timers`.
- **Personality preset table swap** — MOCK-01/KBD-06 are Phase 4. Phase 3 has no notion of personality presets in the rendered tree; the AI block content is one fixed sample paragraph.
- **Token counter increments** — MOCK-04 is Phase 4. Phase 3's status-bar token-count pill shows a hardcoded number (whatever the prototype's default is per D-20).
- **Theme/density/block-style/agent-layout switches** — THEME-01..THEME-05 + the `TweaksPanel` are Phase 5. Phase 3 hardcodes `data-theme="cyan"` (or whatever the prototype's default is), `data-density="comfy"`, `data-block="framed"`, `data-agent="right"` on the root.
- **Mobile shell** — MOB-01..MOB-04 + `WarpHermesMobile` are Phase 6. `src/platform/mod.rs` stays empty in Phase 3.
- **Real shell command execution / real LLM API calls / authentication / persistence / SSR / test suite** — All explicitly Out of Scope per PROJECT.md and REQUIREMENTS.md "Out of Scope" tables. Do not introduce.

</deferred>

---

*Phase: 03-desktop-shell*
*Context gathered: 2026-05-03*
