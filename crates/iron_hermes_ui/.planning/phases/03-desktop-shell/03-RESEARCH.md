# Phase 3: Desktop Shell - Research

**Researched:** 2026-05-03
**Domain:** Dioxus 0.7 RSX porting from React/JSX presentational primitives; CSS @keyframes for knight-rider animation; pixel-perfect verbatim port discipline
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Component Decomposition**
- **D-01:** Mirror `warp2ironhermes/project/app/shell.jsx` 1:1. Create `src/components/shell/` with one file per primitive: `title_bar.rs`, `block.rs`, `command_line.rs`, `tool_call.rs`, `input_box.rs`, `agent_panel.rs`, `status_bar.rs`, `scanner.rs`, `command_palette.rs`. Top-level composer is `src/components/warp_hermes.rs`.
- **D-02:** Add `block_stream.rs` wrapper component (one extension beyond shell.jsx) so the scrollback container styling and `Vec<Block>` ownership live in one file rather than inline in `warp_hermes.rs`.
- **D-03:** Delete `src/components/hero.rs` entirely. `app.rs` swaps `Hero {}` for `WarpHermes {}` in the same `rsx!` slot. `components/mod.rs` changes from `pub mod hero;` to `pub mod warp_hermes; pub mod shell;`.
- **D-04:** Promote `src/state.rs` to hold every shared shell type: `Block`, `BlockKind`, `Mode { Shell, Agent }`, `CommandLine`, `Token`, `ToolCall`, `ToolStatus`, `PaletteItem`, plus `pub fn demo_blocks() -> Vec<Block>`. Single import path: `use crate::state::*` from every shell primitive.
- **D-05:** Asset constants colocate with consuming primitive. `WORDMARK_SVG` migrates from `hero.rs` to `title_bar.rs`. `IH_SHIELD_PNG` lands in whichever primitive uses it. `scanner.svg` only copied if referenced by prototype.

**State Scaffolding Scope**
- **D-06:** Phase 3 is **pure static rendering**. No `use_signal`, no `use_memo`, no `use_resource` calls anywhere in shell components. Every prop value is a hardcoded constant in `rsx!`. **The planner must reject any task that adds reactivity.**
- **D-07:** Component props are **plain owned values**: `Block { data: Block }`, `InputBox { mode: Mode }`, `BlockStream { blocks: Vec<Block> }`. All shared types in `src/state.rs` derive `Clone + PartialEq`. Phase 4 refactors specific props to `Signal<T>` / `ReadOnlySignal<T>`.
- **D-08:** Scanner rendered with `is-active` class **hardcoded** in Phase 3, so CSS `@keyframes` runs continuously on page load.
- **D-09:** Input box is a `<textarea>` with prototype's class names (e.g., `wh-input-textarea`); auto-grow inherited from CSS or browser default.

**Block Data Model**
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
  Six variants. Block enum derives `Clone + PartialEq + Debug`.
- **D-11:** `Block` component matches on enum variant and composes inner content via sibling components. Outer chrome (stripe class, hover-action button row) lives in `block.rs`. For `Cmd`, body renders `<CommandLine>`. For `Tool`, body renders `<ToolCall>`. For `Out`/`Ai`/`Ok`/`Err`, body renders string content directly inside `block.rs`.
- **D-12:** `CommandLine` is `struct CommandLine { tokens: Vec<Token> }` with `enum Token { Bin(String), Arg(String), Flag(String) }`. Renders one `<span>` per token with class `is-bin` / `is-arg` / `is-flag`.
- **D-13:** `ToolCall` is `struct ToolCall { name: String, args_summary: String, status: ToolStatus }` with `enum ToolStatus { Pending, Running, Done, Failed }`.
- **D-14:** `BlockStream` is its own component (`block_stream.rs`) accepting `blocks: Vec<Block>` as owned prop. Iterates with `for block in blocks` in rsx, emits `<Block data={block} />`. Phase 4 swaps prop to `Signal<Vec<Block>>`.
- **D-15:** Hover affordance reveal is **pure CSS `:hover`** — copy/rerun/share buttons render unconditionally; visibility driven by `.wh-block:hover .wh-block-actions { opacity: 1 }`. Zero Rust state.
- **D-16:** `is-ai` content renders as **plain text** in Phase 3. Newlines preserved via `<pre>` wrapper or `white-space: pre-wrap`. No markdown parser in Phase 3.

**Phase 3 Demo Composition**
- **D-17:** Phase 3 fixtures live in `pub fn demo_blocks() -> Vec<Block>` inside `src/state.rs`. `WarpHermes` calls `demo_blocks()` and passes Vec to `BlockStream`.
- **D-18:** `demo_blocks()` returns ~8–10 blocks: one Cmd, one Out, one Ai, one Ok, one Err, plus four Tool blocks one per `ToolStatus` variant.
- **D-19:** Command palette overlay renders **open by default** in Phase 3 to satisfy SC-6 visual UAT.
- **D-20:** Title-bar tabs and status-bar dot-pills use **prototype-default content verbatim** from `app.jsx`.

### Claude's Discretion (resolved by UI-SPEC)

- **`is-tool` stripe color:** RESOLVED — `--w-stripe-tool` (yellow) per ported `warp-ih.css` line 30/186. No planner choice needed.
- **Scanner SVG copy:** RESOLVED — DO NOT COPY. Confirmed unused; scanner cells are unicode glyphs only.
- **Agent panel default content:** RESOLVED — copy `seedMessages()` from `app.jsx` verbatim.
- **Sigil primitive handling:** RESOLVED — own file `src/components/shell/sigil.rs` (used in 3 places: title bar, side panel head, inline-sigil agent variant).
- **Hover-action button glyphs:** RESOLVED — `⎘` `↻` `↗` with `title="copy"` / `"rerun"` / `"share"` per `shell.jsx`.
- **`<pre>` vs `<div>` for `Ai` content:** RESOLVED — use `div` (CSS `white-space: pre-wrap` already inherited from `.wh-block-body`).
- **Wordmark/shield asset destination:** Planner-handoff #1 — recommended: declare in `title_bar.rs` with `#[allow(dead_code)]`; Phase 5 TweaksPanel may consume.
- **`Block::Out` body fidelity:** Planner-handoff #3 — recommended: Approach A (plain `text: String`); escalate to Approach B only if UAT calls out per-line color loss.
- **Focus ring activation:** Planner-handoff #4 — recommended: hardcode `is-focus` on `wh-input-wrap` for Phase 3 UAT visibility.

### Deferred Ideas (OUT OF SCOPE for Phase 3)

- Markdown rendering for `is-ai` blocks (Phase 4: `pulldown-cmark` or `comrak`)
- Reactive signal scaffolding (Phase 4)
- Keyboard handlers (`⌘K`, `Esc`, `⌥M`, `Enter`/`Shift+Enter`, `↑`/`↓` — all Phase 4)
- `runShell` / `runAgent` mocks with prototype timings (Phase 4)
- Personality preset table swap (Phase 4)
- Token counter increments (Phase 4)
- Theme/density/block-style/agent-layout switches (Phase 5)
- Mobile shell (Phase 6)
- Real shell command execution / real LLM API calls / authentication / persistence / SSR / test suite (out of scope per PROJECT.md)
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SHELL-01 | Title bar renders macOS traffic lights, tab strip, and `⌘K` shortcut display | TitleBar component port (shell.jsx lines 62-91) — three `<span>` traffic lights with inline styles, `wh-tabs` with three tabs from `app.jsx`, `wh-titlebar-actions` with `⌘K` text. RSX inline-style attribute `style: "..."` matches React `style={{...}}` verbatim. |
| SHELL-02 | Block stream renders five block types with color-coded 2px left stripe | `Block` component matches on `Block` enum (6 variants per D-10). Class string `"wh-block is-{kind}"` via Dioxus `{var}` interpolation. CSS `.wh-block::before` paints stripe per stripe-token (already in ported `warp-ih.css` lines 177-188). |
| SHELL-03 | Hover reveals copy / rerun / share buttons | Pure CSS `.wh-block:hover .wh-block-actions { opacity: 1 }` — already in `warp-ih.css` line 227. Buttons render unconditionally; zero Rust state per D-15. Use `title: "copy"` / `"rerun"` / `"share"` attributes. |
| SHELL-04 | `CommandLine` sub-component renders bin/arg/flag tokens with distinct colors | `CommandLine { tokens: Vec<Token> }` per D-12. RSX `for token in tokens { span { class: "wh-cmd-{kind}", ... } }`. Color rules already in CSS (`wh-cmd-flag` magenta, `wh-cmd-arg` fg, `wh-cmd` bin fg-strong+bold). |
| SHELL-05 | Tool-call block visualizes tool invocation (name, args, status) | `ToolCall { name, args_summary, status }` per D-13. `Block::Tool` variant per D-10. CSS `.wh-toolcall` already styled (yellow border-left, monospace args pre-block) in `warp-ih.css` lines 391-400. |
| SHELL-06 | Input box: mode glyph, auto-growing textarea, accent focus ring + glow | Real `<textarea rows="1">` per D-09. Mode glyph rendered conditionally (`❯` Shell / `✦` Agent — but Phase 3 hardcodes shell mode per UI-SPEC). Focus ring visible via hardcoded `is-focus` class for UAT (planner-handoff #4). CSS already implements glow (`box-shadow` with `color-mix` 15%). |
| SHELL-07 | Agent side panel renders at right (360px) when `data-agent="right"` | `AgentPanel` component port (shell.jsx lines 186-211). Hardcoded data-attribute on `wh-app` root per UI-SPEC. CSS `.wh-side { width: 360px }` already in place. Seed messages copied verbatim from `seedMessages()`. |
| SHELL-08 | Status bar renders dot-pills for mode · model · provider · tokens · scanner | `StatusBar` component port (shell.jsx lines 30-50). Five `wh-pill` spans with inline `color: var(--pill-N)` style per pill-rotation in `design-tokens.css`. Hardcoded values per D-20: `Chat` / `claude-sonnet-4` / `anthropic` / `12.3K/128K (10%)`. |
| SHELL-09 | Scanner runs knight-rider animation via CSS `@keyframes` (10 cells, 100ms tick, triangle wave) | **Implementation gap identified:** ported CSS has color rules (`lit`/`t1`/`t2` classes) but no `@keyframes` rule. Phase 3 must add a `@keyframes wh-scanner-tick` block. See "Standard Stack: CSS Animation Pattern" below. Per CONTEXT D-08, scanner rendered with `is-active` hardcoded so animation runs on page load. |
| SHELL-10 | Command palette overlay renders slash commands and workflow items | `CommandPalette` component port (shell.jsx lines 214-268). Open by default per D-19. Items hardcoded from `PALETTE_ITEMS` in `app.jsx`. First row (`/help`) gets `is-active` class for highlight visibility. |
</phase_requirements>

## Summary

Phase 3 is a **verbatim React→Dioxus 0.7 port** of `warp2ironhermes/project/app/shell.jsx`. The CONTEXT.md (20 D-XX decisions) and UI-SPEC.md (6/6 dimensions PASS, 4 planner-handoff items) together prescribe nearly everything: component file layout, prop signatures, asset migrations, demo data, hardcoded class strings, hardcoded data-attributes. Research focus is therefore narrow — the "how to translate React patterns to Dioxus 0.7" mechanics.

The five high-leverage findings:

1. **Dynamic class strings use Dioxus `{var}` interpolation directly** — `class: "wh-block is-{kind_class}"` with `kind_class: &str` in scope. No `format!` macro is needed in `rsx!`. Conditional classes use `class: if cond { "..." }`. Both patterns verified in Dioxus 0.7 official docs.
2. **Plain-value props with `Clone + PartialEq + Debug` on enums work natively in `#[component]`** — confirmed via Dioxus 0.7 official docs. The `Block` enum (per CONTEXT D-10) is a textbook fit. No `Props` derive macro needed; the `#[component]` macro auto-generates the props struct.
3. **`for X in Y` iteration in RSX is the project-preferred pattern** (per AGENTS.md) and the official Dioxus 0.7 idiom. Iterating `for block in blocks { Block { data: block.clone() } }` is the canonical port of React's `{blocks.map(b => <Block ... />)}`.
4. **Scanner @keyframes is the only Phase 3 design gap.** The cleanest CSS implementation is **per-cell `:nth-child` rules with staggered `animation-delay`**. A single `@keyframes wh-scanner-tick` block animates each cell's `color` through the triangle wave; each `:nth-child(N)` gets `animation-delay: calc((N-1) * 100ms)` to phase-shift them. The 1800ms period and 10-cell width are already declared as CSS variables in `design-tokens.css`. Pure CSS, GPU-friendly, satisfies CONTEXT D-08 verbatim. **Caveat:** the prototype's React algorithm assigns `lit/t1/t2` *classes* per tick (not just colors). Pure-CSS port animates *colors* through the same color sequence — visually equivalent at the pixel level, structurally different. This is the right tradeoff per the project's "pixel-perfect to prototype" rule.
5. **Validation strategy beyond compile gate:** The three-platform `cargo build` from Phase 1 stays the canonical phase gate. Add `cargo clippy` (signal-borrow-safety rules in clippy.toml are no-ops in Phase 3 since there's no async, but running clippy clean is cheap insurance). The primary validation is the **side-by-side visual UAT against `warp2ironhermes/project/Warp × IronHermes.html`** per project STATE.md todo list. Author a UAT script structured around SC-1..SC-6 acceptance criteria with explicit "look at the prototype, look at our app, do they match?" checks per criterion.

**Primary recommendation:** The planner should split this into 3-4 wave-parallelizable plans organized by component-dependency layer (state.rs first → leaf primitives in parallel → composer last). Avoid the temptation to add `use_signal` for "just one toggle" — the Phase 3/4 boundary is the canonical line.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Static UI rendering (all 12 components) | Browser/Client (WASM) | — | Dioxus web runtime; everything is client-side render |
| Component composition | Browser/Client (Dioxus VDOM) | — | `WarpHermes` composes shell primitives via `rsx!` |
| Data model (Block enum, ToolCall, etc.) | Application/State module (`src/state.rs`) | — | Shared types consumed by every shell primitive |
| Asset loading (CSS, fonts, images) | Build-time (Dioxus `asset!()` macro) | Browser cache | Compile-time-resolved hashed paths; runtime is fetch-from-URL |
| Class-string composition | Component code (string interpolation) | CSS engine (selector match) | Rust formats the class string; browser CSS engine does the styling |
| Scanner animation | CSS engine (@keyframes + animation-delay) | — | Per CONTEXT D-08; no Rust-side animation loop |
| Hover affordance reveal | CSS engine (:hover pseudo-class) | — | Per D-15; zero Rust state |
| Phase 3 demo data | Static fixture (`demo_blocks()` in `state.rs`) | — | Hardcoded constants; no async, no service calls |

**Why this matters:** Every capability in Phase 3 lives in either compile-time Rust (component structure, prop wiring, fixtures) or runtime CSS (animations, hover, focus, layout). **Nothing belongs in JavaScript runtime, server-side, async, or cross-platform-conditional code.** This narrow tier mapping rules out a class of misassignments — the planner should reject any task that proposes signals, server functions, `#[cfg]` gates, or `use_resource` calls in Phase 3.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `dioxus` | 0.7.1 (pinned `=0.7.1`) | UI framework | Already pinned per HYG-01; no new dependencies in Phase 3 |

**Verification:** `Cargo.toml` already pins `dioxus = "=0.7.1"`. Verified via `npm view`-equivalent → Context7 reports Dioxus versions including 0.7.1 and 0.7.2 published; project decision to stay on 0.7.1 is locked. `[VERIFIED: Context7 /dioxuslabs/dioxus]`

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| (none) | — | — | Phase 3 introduces zero new Cargo dependencies |

**Markdown crate (`pulldown-cmark` or `comrak`)** is explicitly Phase 4 territory per CONTEXT "Deferred Ideas". `[CITED: 03-CONTEXT.md]`

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Hand-written `Block` enum | `enum_dispatch` crate for trait-based dispatch | Enum-with-variant-fields + `match` is the Rust-idiomatic pattern for closed sets of variants; `enum_dispatch` adds complexity for no gain at this scale (6 variants). Rejected. |
| `for block in blocks` in RSX | `{blocks.iter().map(...)}` iterator chain | Both work in Dioxus 0.7. **`for` is project-preferred per AGENTS.md** ("Prefer loops over iterators") and is more readable for Vec render. Use `for`. |
| Pure-CSS scanner | Signal-driven scanner (React-style `setInterval`) | CONTEXT D-08 explicitly mandates CSS @keyframes. Pure-CSS is GPU-compositor-friendly and avoids 10Hz VDOM diffs. Rejected signal-driven. |

**Installation:** No `npm install` or `cargo add` needed. `Cargo.toml` is unchanged.

**Version verification:** `Cargo.toml` already pins `dioxus = "=0.7.1"`. Dioxus 0.7.2 exists upstream (newer patch) but project decision is to stay on 0.7.1 per HYG-01. No version change in Phase 3.

## Architecture Patterns

### System Architecture Diagram

```
                ┌─────────────────────────────────┐
                │  src/main.rs (dioxus::launch)   │
                └────────────────┬────────────────┘
                                 │
                ┌────────────────▼────────────────┐
                │  src/app.rs#App                 │
                │  - document::Link cascade       │
                │  - renders WarpHermes {}        │  (Phase 3 swap: Hero → WarpHermes)
                └────────────────┬────────────────┘
                                 │
        ┌────────────────────────▼────────────────────────┐
        │  src/components/warp_hermes.rs#WarpHermes       │
        │  - <div class="wh-app" data-theme=cyan ...>     │
        │  - composes 5 layout regions                    │
        └─┬────────┬────────────┬────────────┬────────────┘
          │        │            │            │
    ┌─────▼──┐ ┌──▼─────────┐ ┌▼─────────┐ ┌▼──────────────┐
    │TitleBar│ │BlockStream │ │InputBox  │ │AgentPanel     │
    │(top)   │ │(scrollable)│ │(bottom)  │ │(right 360px)  │
    └────────┘ └──┬─────────┘ └──────────┘ └───────────────┘
                  │
                  │ for block in blocks {
                  ▼
              ┌────────┐
              │ Block  │  match on Block enum:
              │        │   - Cmd → CommandLine inside body
              └─┬──────┘   - Tool → ToolCall inside body
                │          - Out/Ai/Ok/Err → text inside body
        ┌───────▼─────────┐
        │CommandLine /    │
        │ToolCall         │
        └─────────────────┘

    ┌──────────────────────────────────────┐
    │  StatusBar (bottom of wh-stream)     │
    │  + Scanner (10 unicode cells, CSS    │
    │    @keyframes triggers visual loop)  │
    └──────────────────────────────────────┘

    ┌─────────────────────────────────┐
    │  CommandPalette (open=true,     │
    │  rendered as overlay over .wh-  │
    │  app via position: absolute)    │
    └─────────────────────────────────┘

    ┌──────────────────────────────────────────┐
    │  src/state.rs (data-only module)         │
    │  - Block, BlockKind, Token, ToolCall,    │
    │    ToolStatus, Mode, PaletteItem, ...    │
    │  - demo_blocks(), demo_messages(),       │
    │    demo_palette_items(), demo_tabs()     │
    └──────────────────────────────────────────┘
       ▲ consumed by every shell primitive via `use crate::state::*;`

    ┌──────────────────────────────────────────┐
    │  assets/ (CSS + fonts + images)          │
    │  - design-tokens.css (Phase 2)           │
    │  - warp-ih.css (Phase 2)                 │
    │  - main.css (Phase 2)                    │
    │  - fonts/IoskeleyMono-*.woff2 (Phase 2)  │
    │  - wordmark.svg, ih-shield.png (Phase 2) │
    │  - scanner-anim.css (NEW in Phase 3,     │
    │    OR appended to warp-ih.css)           │
    └──────────────────────────────────────────┘
       ▲ loaded via document::Link in app.rs
```

### Recommended Project Structure
```
src/
├── main.rs                    # entry point — unchanged in Phase 3
├── app.rs                     # swaps Hero {} → WarpHermes {}
├── fonts.rs                   # unchanged
├── state.rs                   # PROMOTED — holds Block enum, ToolCall, demo_blocks(), etc.
├── platform/
│   └── mod.rs                 # unchanged (mobile in Phase 6)
└── components/
    ├── mod.rs                 # MODIFIED — `pub mod warp_hermes; pub mod shell;`
    ├── warp_hermes.rs         # NEW — top-level composer
    └── shell/                 # NEW directory
        ├── mod.rs             # NEW — re-exports every primitive
        ├── title_bar.rs       # NEW — TitleBar + traffic lights + tabs
        ├── sigil.rs           # NEW — Sigil "IH" 26×26 stamp
        ├── block_stream.rs    # NEW — wraps Vec<Block> iteration (D-02 extension)
        ├── block.rs           # NEW — Block component (matches enum)
        ├── command_line.rs    # NEW — CommandLine token spans
        ├── tool_call.rs       # NEW — ToolCall card
        ├── input_box.rs       # NEW — textarea with mode glyph, focus ring
        ├── agent_panel.rs     # NEW — side panel (360px right)
        ├── status_bar.rs      # NEW — dot-pill row with hint
        ├── scanner.rs         # NEW — 10-cell knight-rider span
        └── command_palette.rs # NEW — overlay with slash commands

# To delete:
src/components/hero.rs         # DELETE — Phase 2 brand stub no longer needed
```

**Component count:** 12 component files + state module + 2 mod files = 15 source files touched. **Plus 1 file deletion** (`hero.rs`).

### Pattern 1: Plain-value (non-Signal) component props
**What:** Component functions accept owned values as arguments. The `#[component]` macro auto-generates the props struct from function arguments. Props must be `Clone + PartialEq`. No explicit `#[derive(Props)]` struct is needed.

**When to use:** Phase 3 — every component, no exceptions (per CONTEXT D-07).

**Example:**
```rust
// Source: Context7 /dioxuslabs/dioxus — "Define Dioxus Components with Typed Props"
use dioxus::prelude::*;
use crate::state::Block;

#[component]
pub fn Block(data: Block) -> Element {
    let kind_class = data.kind_class(); // "is-cmd", "is-out", etc.
    rsx! {
        div { class: "wh-block {kind_class}",
            // ...
        }
    }
}
```

**Verification:** Dioxus 0.7 official docs explicitly demonstrate this pattern with `String` props and a custom `bool` prop with `#[props(default)]`. Custom enums and structs work identically as long as they derive `Clone + PartialEq`. `[VERIFIED: Context7 /dioxuslabs/dioxus]`

**Related: when to use `Props` derive instead.** Only when you want fine-grained control over the props struct (e.g., specific generic bounds, custom builders). For Phase 3, the `#[component]` macro handles every case.

### Pattern 2: Dynamic class strings via `{var}` interpolation
**What:** Class strings in RSX use the same `{name}` interpolation syntax as text content. No `format!()` call inside `rsx!`. Multiple modifiers can be conditionally appended via `class: if cond { "extra-class" }`.

**When to use:** Every place where the class string varies based on enum variant or boolean state — `wh-block is-{kind}`, `wh-tab` plus optional `is-active`, `wh-input-wrap` plus optional `is-focus`.

**Example:**
```rust
// Source: Context7 /dioxuslabs/dioxus — "Manage Attributes and Raw Attributes"

let kind_class = "is-cmd"; // computed from enum match
rsx! {
    // Single dynamic class:
    div { class: "wh-block {kind_class}",
        // ...
    }

    // Conditional class composition (multiple class: attributes are concatenated):
    div {
        class: "wh-tab",
        class: if is_active { "is-active" },
        // ...
    }

    // Inline styles use the same interpolation:
    span { style: "background: {color};", "" }
}
```

**Verification:** `[VERIFIED: Context7 /dioxuslabs/dioxus — RSX docs lines on attribute formatting]`

**Anti-pattern:** Do NOT do `class: format!("wh-block is-{}", kind)` inside `rsx!`. Use string interpolation directly: `class: "wh-block is-{kind}"`. The Dioxus macro handles formatting via `IfmtInput`.

### Pattern 3: `for` iteration in RSX (project-preferred)
**What:** Render `Vec<T>` with `for item in items { ... }` directly inside `rsx!`. Each loop body produces zero or more elements. Preferred over `.map()` chains per AGENTS.md project convention and Dioxus 0.7 official docs.

**When to use:** Every Vec-of-blocks, Vec-of-messages, Vec-of-palette-items render.

**Example:**
```rust
// Source: Context7 /dioxuslabs/dioxus — "Conditional Rendering and Iteration"
// + AGENTS.md project preference

#[component]
pub fn BlockStream(blocks: Vec<Block>) -> Element {
    rsx! {
        div { class: "wh-stream",
            div { class: "wh-stream-scroll",
                for (i, block) in blocks.iter().enumerate() {
                    Block { key: "{i}", data: block.clone() }
                }
            }
        }
    }
}
```

**Note on `key`:** Dioxus 0.7 docs show `key: "{i}"` on iterated elements — required for stable diffing across re-renders. Phase 3 has no re-renders (static), but follow the pattern anyway since Phase 4 introduces signals and re-renders.

**Verification:** `[VERIFIED: Context7 /dioxuslabs/dioxus — Conditional Rendering and Iteration docs]`

### Pattern 4: Inline styles via `style: "..."` (verbatim port of React `style={{...}}`)
**What:** React's inline `style={{ width: 12, background: "#ff5f57" }}` ports to Dioxus as `style: "width: 12px; background: #ff5f57;"`. Use string interpolation for dynamic values: `style: "background: {color};"`.

**When to use:** Every inline style in `shell.jsx` — traffic-light dots, brand block borderRight, tab dots, status pills, sigil sizing, etc.

**Example:**
```rust
// Verbatim port of shell.jsx line 67:
//   <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#ff5f57" }} />
rsx! {
    span { style: "width: 12px; height: 12px; border-radius: 50%; background: #ff5f57;" }
}
```

**Why verbatim string and not Dioxus typed CSS attributes:** The pixel-perfect-to-prototype rule outranks code aesthetics (per CONTEXT.md "Specific Ideas"). Inline `style: "..."` strings are 1:1 readable against the React source.

### Pattern 5: Conditional rendering via `if cond { ... }`
**What:** RSX accepts standard Rust `if` directly. No ternary needed. Note: in Phase 3, conditionals are mostly hardcoded (e.g., `show_traffic_lights = true` always).

**Example:**
```rust
// Source: Context7 /dioxuslabs/dioxus — "Conditional Rendering and Iteration"
rsx! {
    if show_traffic_lights {
        div { style: "display: flex; gap: 8px;",
            span { style: "..." } // red
            span { style: "..." } // amber
            span { style: "..." } // green
        }
    }
}
```

### Pattern 6: CSS class on enum variant via `match`
**What:** Map a Rust enum variant to its CSS modifier class via a small helper method on the enum.

**Example:**
```rust
impl Block {
    pub fn kind_class(&self) -> &'static str {
        match self {
            Block::Cmd { .. }  => "is-cmd",
            Block::Out { .. }  => "is-out",
            Block::Ai { .. }   => "is-ai",
            Block::Ok { .. }   => "is-ok",
            Block::Err { .. }  => "is-err",
            Block::Tool { .. } => "is-tool",
        }
    }
}

// Usage in component:
rsx! {
    div { class: "wh-block {data.kind_class()}",
        // ...
    }
}
```

**Why a method instead of a free function:** keeps the source-of-truth (variant→class mapping) next to the type. Phase 4 may add similar helpers for stripe color or author label without scattering them.

### Pattern 7: CSS @keyframes for the scanner animation (the only design gap)
**What:** Pure-CSS knight-rider effect using a single `@keyframes` rule that animates color through the four-state cycle, plus per-cell `:nth-child(N)` `animation-delay` to phase-shift each cell.

**Recommended implementation:**
```css
/* New file: assets/scanner-anim.css
   OR appended to: assets/warp-ih.css (planner picks)
   Source: planner-authored, satisfies CONTEXT D-08 + SHELL-09

   Algorithm: each cell cycles through the same color sequence
   (default → t2 → t1 → lit → t1 → t2 → default → ... → default).
   Cell 0 starts at phase 0; each subsequent cell is delayed by one tick (100ms).
   Period = 1800ms = 18 ticks. 10 cells, triangle wave, returns to start.
*/

@keyframes wh-scanner-tick {
   0%    { color: var(--fg-dim); }                /* off (░) */
   11.1% { color: color-mix(in oklab, var(--accent-primary) 50%, var(--fg-dim)); } /* t2 (▒) */
   22.2% { color: var(--accent-primary); }        /* t1 (▓) */
   33.3% { color: var(--accent-primary-hi); }     /* lit (█) */
   44.4% { color: var(--accent-primary); }        /* t1 (▓) */
   55.5% { color: color-mix(in oklab, var(--accent-primary) 50%, var(--fg-dim)); } /* t2 (▒) */
   66.6% { color: var(--fg-dim); }                /* off */
   100%  { color: var(--fg-dim); }                /* hold */
}

.wh-scanner.is-active span {
   animation: wh-scanner-tick var(--scanner-period, 1800ms) linear infinite;
}
.wh-scanner.is-active span:nth-child(1)  { animation-delay: 0ms; }
.wh-scanner.is-active span:nth-child(2)  { animation-delay: -100ms; }
.wh-scanner.is-active span:nth-child(3)  { animation-delay: -200ms; }
.wh-scanner.is-active span:nth-child(4)  { animation-delay: -300ms; }
.wh-scanner.is-active span:nth-child(5)  { animation-delay: -400ms; }
.wh-scanner.is-active span:nth-child(6)  { animation-delay: -500ms; }
.wh-scanner.is-active span:nth-child(7)  { animation-delay: -600ms; }
.wh-scanner.is-active span:nth-child(8)  { animation-delay: -700ms; }
.wh-scanner.is-active span:nth-child(9)  { animation-delay: -800ms; }
.wh-scanner.is-active span:nth-child(10) { animation-delay: -900ms; }
```

**Important caveat — visual fidelity vs algorithmic fidelity:** The prototype's React algorithm computes a triangle-wave lit-cell index per tick and assigns the `lit/t1/t2` *classes* per cell (lit cell = bright, neighbors = t1/t2). The pure-CSS port animates *colors* through the same color sequence per cell. The visual outcome — a bright spot bouncing back and forth across the row of cells — is identical at the pixel level. The structural difference: in CSS, every cell is always running the animation; in JS, only one cell is "lit" at a time and others have neutral classes. **Side-by-side at 100ms tick rate, the human eye cannot distinguish.** Fidelity to the prototype's *rendered output* is preserved; fidelity to the prototype's *algorithm* is not. Per the project's "pixel-perfect to prototype" rule (PROJECT.md), pixel-equivalence is the contract; algorithm-equivalence is not. **Recommendation: ship pure-CSS.**

**Negative space tradeoff:** A mathematically-identical port would require either (a) JavaScript `setInterval` (rejected — Phase 4 territory and contradicts D-08), (b) Rust `use_signal` + tick state (rejected — same reasoning), or (c) a more elaborate CSS keyframe sequence using nth-child selectors that swap between four discrete keyframes (technically achievable but high complexity for zero visual gain). The recommended approach is the simplest correct one. `[VERIFIED: WebSearch — Multiple sources confirm staggered nth-child animation-delay is the standard knight-rider pattern]`

**File placement decision (planner-handoff #1 from UI-SPEC):** Two options. (A) New file `assets/scanner-anim.css` linked alongside other CSS in `app.rs`. (B) Appended to `assets/warp-ih.css` with a clear comment block. **Recommend (A)** because it preserves the verbatim-port of `warp-ih.css` (no source-of-truth drift) and isolates the Phase-3-authored CSS for clarity. Phase 4 may extend this file when scanner pulse-on-submit lands.

### Anti-Patterns to Avoid

- **Adding `use_signal` for "just one toggle".** Phase 3 is pure static. Reactivity is Phase 4's cohesive boundary. Do not add reactivity even if a single value seems trivial to make reactive.
- **Inventing new CSS class names.** Every class name comes from `warp-ih.css` (already ported). Do not introduce `wh-block-custom` or `wh-shell-foo`.
- **Using `&str` props instead of `String`.** Dioxus props must be owned. `&str` does not satisfy `Clone + PartialEq` requirements for prop types and will fail to compile.
- **Using `format!` macro inside `rsx!` for class strings.** Use `{var}` interpolation directly: `class: "wh-block is-{kind}"`. Dioxus's IfmtInput does the formatting at macro-expansion time.
- **Holding signal borrows across `.await`.** Phase 3 has no async or signals at all, so this is theoretically a non-issue — but if any task accidentally introduces async (e.g., `use_resource`), clippy.toml will catch it. Reject the task.
- **Using `cx`, `Scope`, or `use_state`.** Dioxus 0.6 APIs, removed in 0.7. Won't compile. (Project-wide constraint per PROJECT.md / CLAUDE.md.)
- **Using `<pre>` for `Ai` block bodies.** UI-SPEC explicitly says use `<div>` (CSS `white-space: pre-wrap` already inherited from `.wh-block-body`).
- **Copying `scanner.svg`.** Confirmed unused per UI-SPEC. Do not copy.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Scanner animation tick loop | Rust `setInterval`-equivalent via `use_resource` + `gloo_timers` | CSS `@keyframes` (Pattern 7 above) | CONTEXT D-08 mandates pure-CSS. Avoids 10Hz VDOM diffs; GPU-friendly. |
| Hover affordance reveal | Rust signal toggling on mouseover/mouseout | CSS `.wh-block:hover .wh-block-actions` rule (already in `warp-ih.css`) | Per CONTEXT D-15. Zero Rust state. |
| Class-string concatenation | Manual `String::push_str` or `format!` calls | Dioxus `{var}` interpolation in `rsx!` | Macro handles it; idiomatic Dioxus. |
| Markdown rendering for `Ai` blocks | Hand-parsed `**bold**` / `` `code` `` regex | (defer to Phase 4 — `pulldown-cmark` or `comrak`) | Phase 3 renders plain text per D-16. |
| Auto-grow textarea | Rust `oninput` handler measuring scrollHeight | Browser-default textarea + CSS `min-height: 22px` | Per D-09 / UI-SPEC. Phase 4 escalates if UAT calls it out. |
| Side-panel auto-scroll-to-bottom | Rust `use_effect` with DOM ref | (skip in Phase 3) | UI-SPEC: prototype's behavior is JS-driven; Phase 3 omits, Phase 4 reintroduces if needed. |
| Stripe color logic | Match-statement returning hex per variant | CSS class string + ported CSS rules | `.wh-block.is-cmd::before { background: var(--w-stripe-cmd) }` already exists. Just emit the class. |
| Tool status visual states | Hand-rolled status pill system | (Phase 3: emit class string; CSS handles rendering) | CSS already styles `.wh-toolcall` with yellow `border-left`. Per-status visual differentiation comes from inline status text + CSS. |

**Key insight:** The verbatim CSS port from Phase 2 already implements 95% of the "logic" — Phase 3's job is mostly to emit the right class strings on the right elements with the right structure. Resist any temptation to push styling rules into Rust.

## Common Pitfalls

### Pitfall 1: Dynamic class strings in component prop position
**What goes wrong:** Trying to pass a computed class string as a prop to a child component (e.g., `Block { class: "is-cmd is-active", ... }`) when the child's RSX doesn't accept arbitrary class.

**Why it happens:** React-trained habit of passing `className` as a prop. In Dioxus, each component owns its own root element's class string; child components don't expose `class` as a prop unless explicitly designed to.

**How to avoid:** The `Block` component computes its own class internally based on the `data: Block` prop's variant. The parent passes data; the child decides class.

**Warning signs:** Code like `Block { class_modifier: "is-cmd" }` — should be `Block { data: Block::Cmd { ... } }`.

### Pitfall 2: Forgetting `Clone + PartialEq + Debug` derive on shared state types
**What goes wrong:** `error[E0277]: the trait bound 'Block: PartialEq' is not satisfied` at component-call sites.

**Why it happens:** Dioxus props bound is `Clone + PartialEq`; if Block forgets to derive PartialEq, every component that accepts `data: Block` fails to compile.

**How to avoid:** Every type in `src/state.rs` that crosses a component boundary derives `#[derive(Clone, PartialEq, Debug)]`. The Debug derive is for dev-time `{:?}` introspection during debugging.

**Warning signs:** Trait-bound errors at component-call sites (not at the derive site).

### Pitfall 3: Asset path drift across the move from `hero.rs` to `title_bar.rs`
**What goes wrong:** Asset constants moved but path strings not updated, or vice versa.

**Why it happens:** The `asset!()` macro requires absolute paths starting with `/assets/...`. When constants migrate between files, the path string must stay identical (it's relative to the project root, not the source file).

**How to avoid:** When moving `WORDMARK_SVG` from `hero.rs` to `title_bar.rs`, the line `const WORDMARK_SVG: Asset = asset!("/assets/wordmark.svg");` is identical in the new home. No path adjustment.

**Warning signs:** `cargo build` failure: "asset not found at path".

### Pitfall 4: Iteration `key` collision when blocks have stable IDs
**What goes wrong:** Using `key: "{i}"` (loop index) when block IDs are available — VDOM diffing breaks across re-renders if the order changes.

**Why it happens:** Direct port of React `.map((b, i) => ...)` patterns.

**How to avoid:** Phase 3 has no re-renders so this is harmless, but for forward compat use stable IDs if `Block` carries one. The CONTEXT.md `Block` enum (D-10) doesn't carry an ID field; using loop index is fine for Phase 3. **Phase 4 plan should add an ID field if the prototype's `b1`/`b2`/`bN` IDs become load-bearing.**

**Warning signs:** N/A in Phase 3 (static); flag for Phase 4 review.

### Pitfall 5: CSS cascade ordering when adding `scanner-anim.css`
**What goes wrong:** New CSS rule placed before `warp-ih.css` in the cascade gets overridden by warp-ih's color rules; new rules placed after fail to inherit the variable values from `design-tokens.css`.

**Why it happens:** Phase 2 established a four-link cascade order: `tailwind → main → design-tokens → warp-ih`. Adding scanner-anim has to slot somewhere.

**How to avoid:** Add `SCANNER_ANIM_CSS` link **after** `warp-ih.css` in `app.rs`. The new keyframe rule overrides the static color rules in `warp-ih.css` (which set per-class colors but no animation). The CSS variables (`--accent-primary`, etc.) are already cascaded from `design-tokens.css`.

**Warning signs:** Scanner cells render in static one-frame state with no animation visible; or cells render with broken colors (variables undefined).

### Pitfall 6: `data-attribute` hyphenation in RSX
**What goes wrong:** `data-theme: "cyan"` doesn't compile (Rust identifier rule); `"data-theme": "cyan"` is required (string-literal attribute key).

**Why it happens:** Hyphens are invalid in Rust identifiers. Dioxus 0.7 supports both raw-attribute syntax (string-literal key) and Rust-identifier syntax (snake_case auto-converted to kebab-case for some attributes), but `data-*` attributes require the string-literal form.

**How to avoid:** Always quote `data-*` attribute keys:
```rust
div { class: "wh-app",
    "data-theme": "cyan",
    "data-density": "comfy",
    "data-block": "framed",
    "data-agent": "right",
    // ...
}
```

**Warning signs:** Compile error "expected identifier, found `-`" near `data-`.

### Pitfall 7: Status pill color via `var(--pill-N)` inline style
**What goes wrong:** Five status pills must each render with a different color from the `pill-N` rotation tokens. Using a class per pill (`wh-pill-0`, `wh-pill-1`) requires inventing new classes (forbidden); using inline `style="color: var(--pill-0)"` is the prototype's approach.

**Why it happens:** The prototype's StatusBar (shell.jsx lines 30-50) uses `style={{ color: "var(--pill-0)" }}` per pill — inline-styled, not class-driven.

**How to avoid:** Port the inline style verbatim:
```rust
span { class: "wh-pill", style: "color: var(--pill-0);", "{mode}" }
```

**Warning signs:** All status pills render in the same color (using only `.wh-pill` class with no inline style override).

### Pitfall 8: Sigil font-size derivation from size prop
**What goes wrong:** `Sigil` component takes `size: u16` and must compute `font-size: size * 0.46` per shell.jsx. Dioxus inline style strings are static literals unless interpolated.

**Why it happens:** Direct port of React's `style={{ fontSize: size * 0.46 }}`.

**How to avoid:** Compute in Rust and interpolate:
```rust
#[component]
pub fn Sigil(size: u16) -> Element {
    let font_size = (size as f32 * 0.46) as u16;
    rsx! {
        span {
            class: "wh-sigil",
            style: "width: {size}px; height: {size}px; font-size: {font_size}px;",
            "IH"
        }
    }
}
```

**Warning signs:** Sigil renders at default font size regardless of prop.

## Code Examples

### Example 1: Block component with enum match
```rust
// Source: planner-authored pattern, derived from shell.jsx Block + RenderBlock
// Verified Dioxus 0.7 idiom against Context7 /dioxuslabs/dioxus

use dioxus::prelude::*;
use crate::state::*;

#[component]
pub fn Block(data: Block) -> Element {
    let kind_class = data.kind_class(); // "is-cmd", "is-out", etc.
    rsx! {
        div { class: "wh-block {kind_class}",
            // hover-action button row (always rendered; CSS hides at rest)
            div { class: "wh-block-actions",
                button { class: "wh-icon-btn", title: "copy", "⎘" }
                if !matches!(data, Block::Cmd { .. }) {
                    // cmd block has no rerun/share — only copy
                    button { class: "wh-icon-btn", title: "rerun", "↻" }
                    button { class: "wh-icon-btn", title: "share", "↗" }
                }
            }
            // body via match
            match data {
                Block::Cmd { command } => rsx! {
                    CommandLine { tokens: command.tokens, time: command.time }
                },
                Block::Tool { call } => rsx! {
                    ToolCall { name: call.name, args_summary: call.args_summary, status: call.status }
                },
                Block::Out { text } => rsx! {
                    div { class: "wh-block-body", "{text}" }
                },
                Block::Ai { markdown } => rsx! {
                    div { class: "wh-block-body", "{markdown}" }
                },
                Block::Ok { message } => rsx! {
                    div { class: "wh-block-body", "{message}" }
                },
                Block::Err { message } => rsx! {
                    div { class: "wh-block-body", "{message}" }
                },
            }
        }
    }
}
```

### Example 2: BlockStream iterating Vec<Block>
```rust
// Source: Pattern 3 above + CONTEXT D-14

#[component]
pub fn BlockStream(blocks: Vec<Block>) -> Element {
    rsx! {
        div { class: "wh-stream",
            div { class: "wh-stream-scroll",
                for (i, block) in blocks.iter().enumerate() {
                    Block { key: "{i}", data: block.clone() }
                }
            }
        }
    }
}
```

### Example 3: WarpHermes top-level composer with hardcoded data-attributes
```rust
// Source: app.jsx WarpHermes lines 232-275 + CONTEXT D-19/D-20

use dioxus::prelude::*;
use crate::components::shell::*;
use crate::state::*;

#[component]
pub fn WarpHermes() -> Element {
    let blocks = demo_blocks();
    let messages = demo_messages();
    let palette_items = demo_palette_items();
    let tabs = demo_tabs();

    rsx! {
        div {
            class: "wh-app",
            "data-theme": "cyan",
            "data-density": "comfy",
            "data-block": "framed",
            "data-agent": "right",

            TitleBar {
                tabs: tabs,
                active_tab: 0_usize,
                show_traffic_lights: true,
            }

            div { class: "wh-main",
                div { class: "wh-stream",
                    BlockStream { blocks: blocks }
                    InputBox {
                        mode: Mode::Shell,
                        focused: true, // hardcoded for Phase 3 UAT visibility (planner-handoff #4)
                    }
                    StatusBar {
                        mode: "Chat".to_string(),
                        model: "claude-sonnet-4".to_string(),
                        provider: "anthropic".to_string(),
                        tokens: TokenBudget { used: 12300, max: 128000 },
                        scanner_active: true, // hardcoded per D-08
                        hint: "/help · ⌃C cancel · ⌘K palette".to_string(),
                    }
                }
                AgentPanel { messages: messages, personality: "default".to_string() }
            }

            // Open by default per D-19 for SC-6 UAT
            CommandPalette { items: palette_items, query: String::new(), open: true }
        }
    }
}
```

### Example 4: Sigil with size-derived font-size
```rust
// Source: Pitfall 8 above + shell.jsx lines 53-59

#[component]
pub fn Sigil(#[props(default = 26)] size: u16) -> Element {
    let font_size = (size as f32 * 0.46).round() as u16;
    rsx! {
        span {
            class: "wh-sigil",
            style: "width: {size}px; height: {size}px; font-size: {font_size}px;",
            "IH"
        }
    }
}
```

### Example 5: Three traffic-light dots (verbatim React→RSX inline-style port)
```rust
// Source: shell.jsx TitleBar lines 65-71

if show_traffic_lights {
    div { style: "display: flex; gap: 8px; align-items: center; padding-right: 8px;",
        span { style: "width: 12px; height: 12px; border-radius: 50%; background: #ff5f57;" }
        span { style: "width: 12px; height: 12px; border-radius: 50%; background: #febc2e;" }
        span { style: "width: 12px; height: 12px; border-radius: 50%; background: #28c840;" }
    }
}
```

### Example 6: state.rs Block enum + helper method
```rust
// Source: CONTEXT D-10..D-13 + Pattern 6 above

#[derive(Clone, PartialEq, Debug)]
pub enum Block {
    Cmd  { command: CommandLine },
    Out  { text: String },
    Ai   { markdown: String },
    Ok   { message: String },
    Err  { message: String },
    Tool { call: ToolCall },
}

impl Block {
    pub fn kind_class(&self) -> &'static str {
        match self {
            Block::Cmd  { .. } => "is-cmd",
            Block::Out  { .. } => "is-out",
            Block::Ai   { .. } => "is-ai",
            Block::Ok   { .. } => "is-ok",
            Block::Err  { .. } => "is-err",
            Block::Tool { .. } => "is-tool",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct CommandLine {
    pub tokens: Vec<Token>,
    pub time: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    Bin(String),
    Arg(String),
    Flag(String),
}

#[derive(Clone, PartialEq, Debug)]
pub struct ToolCall {
    pub name: String,
    pub args_summary: String,
    pub status: ToolStatus,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ToolStatus {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Mode {
    Shell,
    Agent,
}

pub fn demo_blocks() -> Vec<Block> {
    vec![
        Block::Cmd { command: CommandLine {
            tokens: vec![
                Token::Bin("ironhermes".into()),
                Token::Arg("doctor".into()),
            ],
            time: Some("0.4s".into()),
        }},
        Block::Out { text: "IronHermes Doctor\n────────────────────────────────────────\n  [OK]      Rust toolchain     1.81.0 stable\n  [OK]      Cargo workspace    7 crates · 360k LOC\n  ...\n  [MISSING] OpenAI key         not set".into() },
        // ... (8-10 total per D-18)
    ]
}
```

## Plan Decomposition Strategy

The 15-file Phase 3 deliverable splits naturally into **3 wave-parallelizable plans** based on file-dependency layers:

### Plan 03-01: Foundation (state + entry-point glue)
**Wave 0 prerequisite — runs alone before any component plans.**

- Promote `src/state.rs`: add `Block`, `BlockKind`, `Mode`, `CommandLine`, `Token`, `ToolCall`, `ToolStatus`, `PaletteItem`, `Tab`, `Message`, `TokenBudget` types with `Clone + PartialEq + Debug` derives.
- Add `demo_blocks()`, `demo_messages()`, `demo_palette_items()`, `demo_tabs()` fixture functions.
- Add `assets/scanner-anim.css` with `@keyframes wh-scanner-tick` per Pattern 7.
- Add `SCANNER_ANIM_CSS` asset constant in `src/app.rs` and `document::Link` for it.
- Update `src/components/mod.rs`: replace `pub mod hero; pub use hero::Hero;` with `pub mod warp_hermes; pub mod shell; pub use warp_hermes::WarpHermes;`.
- Create `src/components/shell/mod.rs` with re-exports of every primitive (placeholder `pub use` lines that resolve once primitives are written).
- Update `src/app.rs` to swap `Hero {}` → `WarpHermes {}`.
- Delete `src/components/hero.rs`.
- Migrate `WORDMARK_SVG` and `IH_SHIELD_PNG` constants → `src/components/shell/title_bar.rs` skeleton (function body left as `todo!()`).

**Verification:** `cargo build --features web` fails (TitleBar etc. not implemented yet) but `state.rs` and `mod.rs` files compile in isolation. Use `cargo check` per file. **Defer compile gate to end of Plan 03-03.**

### Plan 03-02: Leaf primitives (parallel-safe)
**Wave 1 — runs after 03-01 merges; multiple sub-tasks can run in parallel since these primitives don't import each other.**

These are leaves with no dependencies on other shell primitives:

| File | Component | Depends on |
|------|-----------|------------|
| `src/components/shell/sigil.rs` | `Sigil` | nothing (just `rsx!`) |
| `src/components/shell/scanner.rs` | `Scanner` | nothing (renders 10 hardcoded `<span>`s) |
| `src/components/shell/command_line.rs` | `CommandLine` | `state::{Token, CommandLine}` |
| `src/components/shell/tool_call.rs` | `ToolCall` | `state::{ToolCall, ToolStatus}` |

Composite primitives that depend on leaves above:

| File | Component | Depends on |
|------|-----------|------------|
| `src/components/shell/title_bar.rs` | `TitleBar` | `Sigil` |
| `src/components/shell/status_bar.rs` | `StatusBar` | `Scanner` |
| `src/components/shell/input_box.rs` | `InputBox` | nothing |
| `src/components/shell/block.rs` | `Block` | `CommandLine`, `ToolCall` (both leaves) |
| `src/components/shell/agent_panel.rs` | `AgentPanel` | `Sigil`, `ToolCall` |
| `src/components/shell/command_palette.rs` | `CommandPalette` | nothing |

**Wave structure recommendation:**
- **Wave 1a (parallel):** `sigil.rs`, `scanner.rs`, `command_line.rs`, `tool_call.rs`, `input_box.rs`, `command_palette.rs` (6 files, no inter-dependencies)
- **Wave 1b (after 1a):** `title_bar.rs`, `status_bar.rs`, `block.rs`, `agent_panel.rs` (4 files, depend on Wave 1a primitives)

**Verification per primitive:** `cargo check --features web` passes for the file under edit.

### Plan 03-03: Composer + verification
**Wave 2 — runs after 03-02 fully merges.**

- Create `src/components/shell/block_stream.rs` (`BlockStream` consuming `Block`).
- Create `src/components/warp_hermes.rs` (`WarpHermes` top-level composer).
- Three-platform compile gate: `cargo build --features web && cargo build --features desktop && cargo build --features mobile`.
- `cargo clippy --features web -- -D warnings` (signal-borrow rules are no-ops in Phase 3 but verify clean run).
- Author UAT script in plan.

### Plan 03-04: UAT (manual checkpoint)
**Wave 3 — checkpoint:human-verify gate.**

- User runs `dx serve --features web` and walks SC-1..SC-6 against side-by-side `warp2ironhermes/project/Warp × IronHermes.html`.
- UAT script structured per SC.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | none (no test framework configured per CLAUDE.md; project skips test suite per "Out of Scope") |
| Config file | none |
| Quick run command | `cargo check --features web` |
| Full suite command | `cargo build --features web && cargo build --features desktop && cargo build --features mobile && cargo clippy --features web -- -D warnings` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SHELL-01 | Title bar renders traffic lights, tabs, ⌘K | manual-only | `dx serve --features web` + visual UAT | ❌ Wave 0 — UAT script |
| SHELL-02 | Block stream renders 5+1 stripe types | manual-only | `dx serve --features web` + visual UAT | ❌ Wave 0 — UAT script |
| SHELL-03 | Hover reveals copy/rerun/share | manual-only | `dx serve --features web` + hover test | ❌ Wave 0 — UAT script |
| SHELL-04 | CommandLine bin/arg/flag distinct colors | manual-only | `dx serve --features web` + visual UAT | ❌ Wave 0 — UAT script |
| SHELL-05 | Tool-call block (4 states) | manual-only | `dx serve --features web` + visual UAT | ❌ Wave 0 — UAT script |
| SHELL-06 | Input box mode glyph + focus ring | manual-only | `dx serve --features web` + click into textarea | ❌ Wave 0 — UAT script |
| SHELL-07 | Side panel 360px right | manual-only | `dx serve --features web` + DevTools width check | ❌ Wave 0 — UAT script |
| SHELL-08 | Status bar dot-pills | manual-only | `dx serve --features web` + visual UAT | ❌ Wave 0 — UAT script |
| SHELL-09 | Scanner @keyframes (10 cells, knight-rider) | manual-only | `dx serve --features web` + 5-second observation | ❌ Wave 0 — UAT script |
| SHELL-10 | Command palette overlay | manual-only | `dx serve --features web` + visual UAT (open by default) | ❌ Wave 0 — UAT script |
| All | Three-platform compile | automated | `cargo build --features web && cargo build --features desktop && cargo build --features mobile` | ✅ exists from Phase 1 |
| All | Clippy clean | automated | `cargo clippy --features web -- -D warnings` | ✅ existing config |

**Justification for manual-only on SHELL-XX:** Per project's "Out of Scope" table — `Test suite (unit / integration / visual regression)` is explicitly deferred. The project's primary failure mode is "visual drift", which is best caught by human side-by-side review. Visual regression test infrastructure (`TEST-02`) is v2 territory.

### Sampling Rate
- **Per task commit:** `cargo check --features web` (~5-10s, catches type errors fast)
- **Per wave merge:** `cargo build --features web` (full WASM build, ~30s)
- **Phase gate:** Three-platform `cargo build` (web, desktop, mobile) + `cargo clippy` + manual UAT against prototype HTML
- **Phase 3 close:** `/gsd-verify-work` after UAT approval

### Wave 0 Gaps
- [ ] `assets/scanner-anim.css` — keyframe rule for SHELL-09 (planner authors per Pattern 7)
- [ ] UAT script in Plan 03-04 — six SC checks structured around side-by-side prototype HTML comparison
- [ ] No automated test infrastructure — explicitly skipped per project scope; manual UAT only

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Dioxus 0.6 `cx`/`Scope`/`use_state` | Dioxus 0.7 `use_signal`/`use_memo`/`use_resource` | 0.7 release | Project-wide constraint; not a Phase 3 issue (no signals in Phase 3) |
| React `style={{...}}` object syntax | Dioxus `style: "..."` string syntax | always | Verbatim port; trivial translation |
| React `.map(...)` for iteration | Dioxus `for X in Y` directly in `rsx!` | always | Project preference per AGENTS.md; both work |
| JavaScript `setInterval` for animation | Pure-CSS `@keyframes` | Phase 1 decision (CONTEXT D-08) | Avoids 10Hz VDOM diffs |

**Deprecated/outdated:**
- `dioxus = "0.7.0-rc.X"` versions exist on crates.io — project pins `0.7.1` stable. No reason to upgrade to `0.7.2` mid-project.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | The pure-CSS scanner animation (Pattern 7) is visually indistinguishable from the prototype's per-tick class swap at 100ms tick rate | Pattern 7 / Pitfall 5 | If side-by-side UAT calls out the visual difference (e.g., sees "all cells animating" vs "one cell highlighted"), planner falls back to a more elaborate keyframe sequence with discrete `lit/t1/t2` states. Add a Phase 3 UAT-fallback note in Plan 03-01. |
| A2 | The browser's default `<textarea rows="1">` behavior with `min-height: 22px` matches the prototype's textarea height/expansion at idle | Pattern (Input box) | If UAT calls out a height mismatch, Phase 4 plan adds explicit auto-grow via `oninput`. Risk is cosmetic only. |
| A3 | The `Block::Out` plain-text approach (Approach A from UI-SPEC) is acceptable for the doctor-block side-by-side comparison | Don't Hand-Roll table / Pitfall row | If UAT calls out per-line color loss in `b2` doctor block, planner escalates to Approach B (`Vec<DoctorLine>` rich body type). Risk is contained — only affects 1 fixture block. |
| A4 | The wordmark and shield asset constants belong in `title_bar.rs` (with `#[allow(dead_code)]`) for forward use, not in a centralized `state.rs` brand-asset section | Asset Migration | If Phase 5 TweaksPanel work needs them in a different location, a single `mv` of the const block is trivial. Low risk. |

## Open Questions

1. **Should `scanner-anim.css` be its own file or appended to `assets/warp-ih.css`?**
   - What we know: UI-SPEC flags this as planner-handoff item #1 with two options.
   - What's unclear: Project preference for "verbatim port preservation" vs "single CSS file simplicity".
   - **Recommendation:** Separate file (`assets/scanner-anim.css`). Preserves `warp-ih.css` byte-identity to prototype source-of-truth, which is the deciding factor per the project's verbatim-port discipline. Add a fourth `document::Link` entry in `src/app.rs`.

2. **Should `Block::Out` carry a `Vec<OutLine>` rich body or a plain `String`?**
   - What we know: UI-SPEC recommends Approach A (plain `String`); flags Approach B as escalation if UAT fails.
   - What's unclear: How visually-different the doctor block looks without per-line `[OK]` green / `[MISSING]` yellow color.
   - **Recommendation:** Ship Approach A. The seed text can include `[OK]` and `[MISSING]` literal labels in plain text — they're visible as text, just not green/yellow. UAT decides if escalation is needed.

3. **What's the exact `oninput` behavior we expect from a Dioxus textarea with no handler?**
   - What we know: D-09 says ship `<textarea rows="1">` and accept browser default; Phase 4 adds explicit auto-grow if needed.
   - What's unclear: Whether the browser will visually grow the textarea on multi-line paste or just clip it to `min-height: 22px`.
   - **Recommendation:** Defer to UAT. Phase 3 ships D-09's literal interpretation. If UAT shows clipping, Phase 4 plan adds the `oninput` hook.

4. **Are there any unicode glyphs in the spec that won't render in Ioskeley Mono?**
   - What we know: 19 unicode glyphs are used (`❯ ✦ ⎘ ↻ ↗ ▤ ░ ▒ ▓ █ · ▸ / + × ⌘ ↵ ⌥ ⇧`).
   - What's unclear: Whether all are present in Ioskeley Mono's Unicode coverage. Some (`⌘ ⌥ ⇧`) are macOS modifier glyphs that may fall back to system font.
   - **Recommendation:** Assume browser fallback handles missing glyphs (`"Berkeley Mono", ui-monospace, "SF Mono", "Menlo"...`); UAT confirms. If a glyph renders as `[]` tofu, planner adds a Phase 3 hotfix to the plan.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | Compile | ✓ | 1.81.0+ (per Phase 1 deliverables) | — |
| Dioxus CLI (`dx`) | Dev server + WASM build | ✓ (assumed; was working in Phase 2) | per project | — |
| `cargo` | Build, check, clippy | ✓ | bundled with Rust | — |
| Browser (Chrome/Firefox/Safari) | Manual UAT | ✓ | any modern | — |
| `cargo` features `desktop` and `mobile` | Three-platform compile gate | ✓ (verified Phase 1) | per Cargo.toml | — |

**Missing dependencies with no fallback:** none — Phase 3 introduces no external dependencies.

**Missing dependencies with fallback:** none.

## Project Constraints (from CLAUDE.md)

The following project-level directives MUST be honored by all Phase 3 plans:

- **No Dioxus 0.6 APIs:** `cx`, `Scope`, `use_state` are forbidden. Compile errors result. Use `use_signal`, `use_memo`, `use_resource`, `use_context_provider` / `use_context`. (Phase 3 uses NONE of these — pure static.)
- **Single-threaded WASM:** No `std::thread` usage. (No-op for Phase 3 — no concurrency.)
- **Signal borrows must drop before `.await`:** clippy.toml enforces this. (No-op for Phase 3 — no async.)
- **Component naming:** PascalCase function names with `#[component]` attribute. (Required for every primitive.)
- **Multi-platform Cargo features:** web (default), desktop, mobile. Three-platform compile gate is the canonical phase verification. (Carried forward from Phase 1.)
- **No external services:** Zero API keys, zero network calls in v1. (No-op for Phase 3 — only static fixtures.)
- **Design fidelity:** Pixel-perfect to prototype. Visual drift = primary failure mode. (Drives UAT design.)
- **CSS strategy:** Port `warp-ih.css` and `colors_and_type.css` as-is. No Tailwind conversion. (Already done in Phase 2 — Phase 3 consumes.)
- **Reference directory read-only:** `warp2ironhermes/` is consulted, not compiled. Do not import from it. Do not include in build asset path. (Critical — Phase 3 reads `shell.jsx` for porting reference but never imports.)
- **GSD workflow enforcement:** All edits must go through GSD commands.

## Sources

### Primary (HIGH confidence)
- **Context7 `/dioxuslabs/dioxus`** — Dioxus 0.7 API reference (props, RSX, conditional rendering, iteration, attribute formatting). Verified plain-value props, dynamic class strings, `for X in Y` iteration, `class: if cond { ... }` conditional, `key:` attribute on iterated elements.
- **`AGENTS.md`** (project root) — Authoritative Dioxus 0.7 patterns including the project preference for `for` over `.map()` and the rule against `cx`/`Scope`/`use_state`.
- **`.planning/phases/03-desktop-shell/03-CONTEXT.md`** — 20 D-XX locked decisions covering component decomposition, state scope, block data model, demo composition, hover-affordance approach, scanner activation, palette default visibility, tab/pill copy.
- **`.planning/phases/03-desktop-shell/03-UI-SPEC.md`** — 6/6 dimensions PASS UI design contract: spacing scale, typography, color semantics, copywriting verbatim from prototype, scanner animation contract, hover/focus/cursor contracts, component inventory, asset migration map, wrapper data-attributes.
- **`warp2ironhermes/project/app/shell.jsx`** — 273-line presentational primitives source-of-truth for the 1:1 port.
- **`warp2ironhermes/project/app/app.jsx`** — 352-line shell state + layout source for `seedBlocks()`, `seedMessages()`, `PALETTE_ITEMS`, status bar defaults, tab labels.
- **`assets/warp-ih.css`** (already ported) — 511 lines; every `.wh-*` class definition Phase 3 consumes verbatim.
- **`assets/design-tokens.css`** (already ported) — 245 lines; ANSI palette, type scale, spacing, scanner constants.

### Secondary (MEDIUM confidence)
- **WebSearch** — CSS staggered animation pattern with `animation-delay` per `:nth-child` is the standard knight-rider approach. Verified across Josh Comeau's keyframe guide, CSS-Tricks, MDN, and dev.to articles.

### Tertiary (LOW confidence)
- (none — every Phase 3 design question was resolvable from primary sources)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — Cargo.toml is unchanged in Phase 3; only existing pinned `dioxus = "=0.7.1"` is consumed.
- Architecture / component decomposition: HIGH — fully prescribed by CONTEXT D-01..D-04 and UI-SPEC component inventory.
- RSX patterns (props, classes, iteration): HIGH — verified via Context7 + AGENTS.md.
- Scanner @keyframes implementation: MEDIUM — cleanest CSS approach is well-established (verified via WebSearch); visual fidelity vs algorithmic fidelity tradeoff is an A1 assumption pending UAT validation.
- Validation strategy: HIGH — three-platform compile gate inherited from Phase 1; manual UAT against prototype HTML matches project STATE.md todo list.
- Plan decomposition: HIGH — file-dependency graph derived directly from CONTEXT D-01 + UI-SPEC component inventory.
- Pitfalls: HIGH — most are direct ports of React→Dioxus translation gotchas confirmed in official Dioxus docs.

**Research date:** 2026-05-03
**Valid until:** 30 days (stable prescription; primary risk is upstream Dioxus 0.7 patch releases changing prop semantics — low likelihood)

---

*Phase: 03-desktop-shell*
*Research completed: 2026-05-03*
