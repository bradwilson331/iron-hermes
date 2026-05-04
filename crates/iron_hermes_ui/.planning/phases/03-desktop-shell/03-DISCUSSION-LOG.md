# Phase 3: Desktop Shell - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-03
**Phase:** 03-desktop-shell
**Areas discussed:** Component decomposition, State scaffolding scope, Block data model, Phase 3 demo composition

---

## Component Decomposition

### Q1 — Module organization

| Option | Description | Selected |
|--------|-------------|----------|
| Mirror shell.jsx 1:1 | `src/components/shell/` directory, one file per prototype primitive (title_bar.rs, block.rs, command_line.rs, tool_call.rs, input_box.rs, agent_panel.rs, status_bar.rs, scanner.rs, command_palette.rs) + top-level warp_hermes.rs | ✓ |
| Group by region | Six files: title_bar, block_stream (contains Block + CommandLine + ToolCall), input, sidepanel, statusbar (contains Scanner inline), palette | |
| Single warp_hermes.rs | Everything in one file under `src/components/warp_hermes.rs` | |
| Region dirs with subfiles | `src/components/shell/{titlebar,stream,input,sidepanel,statusbar,palette}/mod.rs` with sub-primitives as siblings | |

**User's choice:** Mirror shell.jsx 1:1
**Notes:** Closest to source-of-truth, easiest to diff against shell.jsx, easiest to split into parallel plans.

### Q2 — Hero stub disposition + WarpHermes location

| Option | Description | Selected |
|--------|-------------|----------|
| Delete hero.rs, new warp_hermes.rs | Delete `src/components/hero.rs` entirely; create `src/components/warp_hermes.rs` (top-level shell composer) + `src/components/shell/*` (primitives). `app.rs` renders `WarpHermes {}` | ✓ |
| Rename hero.rs → warp_hermes.rs | Rename in place; keep `WORDMARK_SVG` and `IH_SHIELD_PNG` constants | |
| Keep Hero as splash, add WarpHermes alongside | Hero still renders as a temporary loading splash; WarpHermes mounts after | |

**User's choice:** Delete hero.rs, new warp_hermes.rs
**Notes:** Phase 2 CONTEXT.md D-03 explicitly said `Hero` would be replaced — deleting matches that intent.

### Q3 — Shared shell types location

| Option | Description | Selected |
|--------|-------------|----------|
| src/state.rs | Promote Phase 1 placeholder; define BlockKind, Mode { Shell, Agent }, BlockData, PaletteItem, etc. | ✓ |
| src/components/shell/types.rs | Co-locate types with the shell primitives | |
| Inline per primitive file | Each component file declares its own types | |

**User's choice:** src/state.rs
**Notes:** Phase 4 will add `runShell`/`runAgent` mock modules alongside, and shared signals later.

### Q4 — Brand asset constants + scanner.svg disposition

| Option | Description | Selected |
|--------|-------------|----------|
| Move to consuming primitive | WORDMARK_SVG → title_bar.rs; IH_SHIELD_PNG → title_bar.rs or agent_panel.rs; SCANNER_SVG (if needed) → status_bar.rs | ✓ |
| All in src/components/shell/assets.rs | Single file holds every asset constant for the shell | |
| Skip scanner.svg this phase | Confirm by reading shell.jsx; if scanner.svg isn't used in the prototype's status bar either, defer it indefinitely | |

**User's choice:** Move to consuming primitive
**Notes:** Keeps the colocation pattern set in Phase 2; planner reads shell.jsx to confirm scanner.svg need.

---

## State Scaffolding Scope

### Q1 — How much state lands in Phase 3

| Option | Description | Selected |
|--------|-------------|----------|
| Pure static rendering | No `use_signal` calls in Phase 3. All values hardcoded constants in rsx. Phase 4 introduces signals, mocks, and handlers as one cohesive change | ✓ |
| Stub signals, no handlers | Define `use_signal` calls now (mode, palette_open, input_value) initialized to demo values; Phase 4 just adds .onkeydown/.onclick/.oninput handlers | |
| Signals + Phase 4-stub handlers | Signals AND handler closures wired but the closures are no-ops | |

**User's choice:** Pure static rendering
**Notes:** Cleanest Phase 3/4 split; planner has zero state-design ambiguity.

### Q2 — Component prop shape

| Option | Description | Selected |
|--------|-------------|----------|
| Plain values | Components take plain owned/borrowed values; Phase 4 refactors props that need reactivity to `Signal<T>` / `ReadOnlySignal<T>` | ✓ |
| Pre-shape as ReadOnlySignal<T> | Even static values flow as `ReadOnlySignal<T>` from day 1 | |
| Mixed by primitive type | Pure-presentation primitives take plain values; stateful primitives accept Signal<T> from day 1 | |

**User's choice:** Plain values
**Notes:** Simpler Phase 3; small Phase 4 prop-signature churn (acceptable since Phase 4 already touches every interactive component).

### Q3 — Scanner appearance in Phase 3

| Option | Description | Selected |
|--------|-------------|----------|
| Always-active for review | Render scanner with `is-active` class hardcoded; CSS animation runs continuously | ✓ |
| Always-static (off) | Render the 10 cells as `░` glyphs without the active class — scanner sits idle | |
| Phase 3 toggle button placeholder | Add a temporary button or `?scanner=on` query param to toggle | |

**User's choice:** Always-active for review
**Notes:** Lets us verify the 10-cell knight-rider visually during Phase 3 UAT.

### Q4 — Input box auto-grow handling

| Option | Description | Selected |
|--------|-------------|----------|
| Inherit prototype CSS, static textarea | Render `<textarea>` with prototype's class names; if warp-ih.css already implements auto-grow via CSS, it just works | ✓ |
| JS-style auto-grow now | Implement scrollHeight measuring pattern in Rust now | |
| Fixed-size textarea, no autogrow | Hard-code textarea rows=1 or rows=3, ignore auto-grow until Phase 4 | |

**User's choice:** Inherit prototype CSS, static textarea
**Notes:** Phase 3 verifies visual styling and focus-ring glow. Phase 4 adds any JS-style behavior the CSS doesn't cover. Planner reads prototype to confirm.

---

## Block Data Model

### Q1 — Rust type representation

| Option | Description | Selected |
|--------|-------------|----------|
| Single enum variants per kind | `enum Block { Cmd { command: CommandLine }, Out { text: String }, Ai { markdown: String }, Ok { message: String }, Err { message: String } }`. Each variant carries kind-specific fields. | ✓ |
| Struct + kind enum | `enum BlockKind { Cmd, Out, Ai, Ok, Err }` + struct with Option fields | |
| Trait + concrete per-kind structs | `trait BlockData { fn render(&self) -> Element; }` with per-kind structs | |
| No type — direct rsx | Skip data modeling entirely; each block is hand-written rsx in the demo composition | |

**User's choice:** Single enum variants per kind
**Notes:** Strong typing, exhaustive matching, easy fixtures.

### Q2 — Block component dispatch pattern

| Option | Description | Selected |
|--------|-------------|----------|
| Block matches on enum, renders inner via siblings | block.rs matches on variant; CommandLine and ToolCall as own files. Outer chrome (stripe, hover actions) lives in block.rs | ✓ |
| Block renders all inline | block.rs contains all rendering logic for every variant inline | |
| Per-variant Block components | `CmdBlock`, `OutBlock`, etc. as separate components with a thin Block dispatcher | |

**User's choice:** Block matches on enum, renders inner via siblings
**Notes:** Clean separation; matches the "mirror shell.jsx" decomposition decision.

### Q3 — CommandLine internal shape

| Option | Description | Selected |
|--------|-------------|----------|
| Vec<Token> with enum | `enum Token { Bin(String), Arg(String), Flag(String) }` + `struct CommandLine { tokens: Vec<Token> }`. Preserves prototype ordering | ✓ |
| Three Vec fields | `struct CommandLine { bin: String, args: Vec<String>, flags: Vec<String> }` | |
| Pre-rendered string with class spans | `struct CommandLine { html: String }` preformatted with class spans | |

**User's choice:** Vec<Token> with enum
**Notes:** Easy to extend with more token kinds in v2.

### Q4 — ToolCall placement in block model

| Option | Description | Selected |
|--------|-------------|----------|
| Sixth Block variant | `enum Block { Cmd, Out, Ai, Ok, Err, Tool { call: ToolCall } }` | ✓ |
| Nested in Ai variant | `Ai { content: AiContent }` where `enum AiContent { Markdown(String), Tool(ToolCall) }` | |
| Decided by prototype | Defer the decision to the planner | |

**User's choice:** Sixth Block variant
**Notes:** Cleanest typing; matches shell.jsx's separate `<ToolCall>` primitive.

### Q5 — ToolCall internal shape and status enum

| Option | Description | Selected |
|--------|-------------|----------|
| Three-field struct, four-state enum | `struct ToolCall { name, args_summary, status }` with `enum ToolStatus { Pending, Running, Done, Failed }` | ✓ |
| Match prototype verbatim | Defer enum/struct shape to the planner after they read shell.jsx's `<ToolCall>` props | |
| Two-state enum | Just `Running | Done` | |

**User's choice:** Three-field struct, four-state enum
**Notes:** Phase 3 fixture renders one example per status to verify the visual treatment of all four.

### Q6 — Block stream container shape

| Option | Description | Selected |
|--------|-------------|----------|
| Vec<Block> owned prop | `fn BlockStream(blocks: Vec<Block>) -> Element`; Phase 4 swaps to Signal<Vec<Block>> | ✓ |
| ReadOnlySignal<Vec<Block>> | Pre-shape as a signal even in Phase 3 | |
| No container component | warp_hermes.rs maps over Vec<Block> directly with a `for` loop in rsx | |

**User's choice:** Vec<Block> owned prop
**Notes:** Adds a `block_stream.rs` file (slight extension beyond shell.jsx primitives), kept for scroll-container colocation.

### Q7 — Hover affordance reveal strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Pure CSS `:hover` | Action buttons render unconditionally; visibility driven by `.wh-block:hover .wh-block-actions { opacity: 1 }` | ✓ |
| Per-block `Signal<bool> is_hovered` | Rust signal toggled by onmouseenter/onmouseleave | |
| Always visible | Skip the hover reveal in Phase 3; render buttons permanently | |

**User's choice:** Pure CSS `:hover`
**Notes:** Zero Rust state. Phase 4 adds onclick handlers; the reveal stays CSS-only forever.

### Q8 — Ai content rendering in Phase 3

| Option | Description | Selected |
|--------|-------------|----------|
| Plain text now, markdown crate later | Render the string verbatim (preserve newlines via `<pre>` or `white-space: pre-wrap`) | ✓ |
| Add pulldown-cmark now | Pull in `pulldown-cmark` (or `comrak`) in Phase 3 | |
| Pre-rendered HTML strings in fixture | Fixture stores already-rendered HTML; render via `dangerous_inner_html` | |

**User's choice:** Plain text now, markdown crate later
**Notes:** Avoids adding a crate dependency for a feature Phase 3 can't fully exercise. Phase 4 owns the agent-reply rendering pipeline.

---

## Phase 3 Demo Composition

### Q1 — Fixture location

| Option | Description | Selected |
|--------|-------------|----------|
| src/state.rs `pub fn demo_blocks()` | Add a `pub fn demo_blocks() -> Vec<Block>` to the same file holding the type definitions | ✓ |
| src/state/fixtures.rs | Split into a sub-module: state/mod.rs (types) + state/fixtures.rs (sample data) | |
| Inline in warp_hermes.rs | Hardcode the Vec<Block> directly inside the WarpHermes composer | |
| Mirror prototype's demoCommands | If app.jsx has a `demoCommands` array, copy its shape verbatim into a Rust constant | |

**User's choice:** src/state.rs `pub fn demo_blocks()`
**Notes:** Phase 4 replaces the call site with mock-driven blocks but keeps `demo_blocks()` as a useful test/dev fixture.

### Q2 — Fixture content coverage

| Option | Description | Selected |
|--------|-------------|----------|
| One-of-each + variants | ~8–10 blocks: one each of Cmd/Out/Ai/Ok/Err + one Tool block per status (Pending, Running, Done, Failed) | ✓ |
| Minimal 5-block set | Exactly 5 blocks, one per stripe type | |
| Replay a prototype scenario | Replicate a specific demo from app.jsx byte-for-byte | |
| Long realistic scrollback | 20+ blocks simulating a realistic terminal session | |

**User's choice:** One-of-each + variants
**Notes:** Verifies every SC visually in one screen scroll.

### Q3 — Command palette default visibility

| Option | Description | Selected |
|--------|-------------|----------|
| Open by default for review | Palette overlay renders open with sample slash commands and workflow items; SC-6 verifiable in Phase 3 UAT | ✓ |
| Add temporary `?palette=open` query param | Read URL query string in warp_hermes.rs; render palette only when `palette=open` present | |
| Closed by default, accept gap | Palette renders only when Phase 4 wires the keyboard | |
| Two-screen UAT with code toggle | Reviewer flips a hardcoded boolean and rebuilds | |

**User's choice:** Open by default for review
**Notes:** Phase 4 inverts the default to closed and wires `⌘K`/`Esc` to toggle.

### Q4 — Title-bar tabs and status-bar dot-pill content source

| Option | Description | Selected |
|--------|-------------|----------|
| Match prototype defaults | Planner reads `app.jsx` and copies the prototype's default tab labels and dot-pill values verbatim | ✓ |
| Generic placeholders | Tabs: `tab 1`, `tab 2`. Status: `mode · model · provider · 0 · [scanner]` | |
| IronHermes-themed sample | Tabs: `iron_hermes_ui`, `warp2ironhermes`. Status: `shell · hermes · local · 1,337 · [scanner]` | |

**User's choice:** Match prototype defaults
**Notes:** Maximum visual fidelity; downstream agents inherit "as-prototype" without choosing labels.

---

## Claude's Discretion

- `is-tool` stripe color — Block enum has a Tool variant not mapped to one of the five named stripes. Planner picks an existing token consistent with the prototype's tool-call visual treatment.
- Scanner SVG copy — Copy `warp2ironhermes/project/ironhermes/assets/scanner.svg` into `assets/` only if the prototype's status-bar references it.
- Agent panel default content — Planner reads `shell.jsx <AgentPanel>` and copies whatever default the prototype ships, or a minimal placeholder if data-driven.
- Sigil primitive handling — If `Sigil` is just a one-line glyph wrapper in shell.jsx, port inline rather than create a 5-line `sigil.rs` file.
- Hover-action button glyphs — Planner uses the prototype's exact unicode glyphs (⎘ ↻ ↗) and aria-labels.
- `<pre>` vs `<div>` for Ai content — Planner picks the structurally-correct element for the markdown-stub variant.

## Deferred Ideas

- Markdown rendering for `is-ai` blocks — Phase 4 adds `pulldown-cmark` or `comrak` when the personality preset mock-reply tables land.
- Reactive signal scaffolding — Every `Signal<T>` / `ReadOnlySignal<T>` / `use_signal` call in shell components is Phase 4 territory.
- Keyboard handlers — `⌘K`, `Esc`, `⌥M`, `Enter`/`Shift+Enter`, `↑`/`↓` are all Phase 4.
- `runShell` and `runAgent` mocks with prototype-matching timings — Phase 4.
- Personality preset table swap — Phase 4.
- Token counter increments — Phase 4.
- Theme/density/block-style/agent-layout switches + `TweaksPanel` — Phase 5.
- Mobile shell — Phase 6.
