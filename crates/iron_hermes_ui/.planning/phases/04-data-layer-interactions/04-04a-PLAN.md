---
phase: 04-data-layer-interactions
plan: 04a
type: execute
wave: 4
depends_on: [01, 02, 03]
files_modified:
  - src/components/shell/markdown.rs
  - src/components/shell/mod.rs
  - src/components/shell/block.rs
  - src/components/shell/block_stream.rs
  - src/components/warp_hermes.rs
autonomous: true
requirements: [KBD-05]
tags: [dioxus, components, signals, refactor, props, event-handlers, markdown, blockentry-id]
must_haves:
  truths:
    - "src/components/shell/markdown.rs declares pub fn render_inline_code(text: &str) -> Element splitting on backticks; even-indexed → <span>, odd-indexed → <code>; wrapped in <div style=\"white-space: pre-wrap;\">; key per child (per D-15 + RESEARCH Pattern 6)"
    - "src/components/shell/block.rs Block prop signature is (entry: BlockEntry, on_copy: EventHandler<()>, on_rerun: EventHandler<()>); is-ai body invokes render_inline_code (per D-16); copy onclick fires on_copy.call(()) (per D-23 + KBD-05); rerun onclick fires on_rerun.call(()) only when is_cmd (per D-24); share button keeps rendering as click-no-op (per D-25); is_cmd boolean drives rerun-button enabled state"
    - "src/components/shell/block_stream.rs BlockStream prop signature is (blocks: ReadOnlySignal<Vec<BlockEntry>>, on_rerun: EventHandler<u64>); iterates blocks.read(); uses key: \"{entry.id}\" (per D-07/D-08); copy onclick on each child resolves text via block_text_for_copy and writes to web_sys clipboard fire-and-forget (per D-23, cfg-gated wasm32); rerun forwards entry.id"
    - "src/components/shell/mod.rs declares pub mod markdown; with #[allow(unused_imports)] pub use markdown::render_inline_code; (mirrors existing re-export pattern)"
    - "src/components/warp_hermes.rs receives a transient shim edit so the new BlockStream prop signature (ReadOnlySignal<Vec<BlockEntry>>, on_rerun: EventHandler<u64>) type-checks; Plan 04-05 will REPLACE the file entirely with the WarpHermes integration body"
  artifacts:
    - path: "src/components/shell/markdown.rs"
      provides: "render_inline_code(text) -> Element backtick-aware inline code helper (D-15)"
      exports: ["render_inline_code"]
    - path: "src/components/shell/block.rs"
      provides: "Block component refactored: entry: BlockEntry + on_copy/on_rerun EventHandler<()> props; is-ai body invokes render_inline_code; clipboard delegated upstream (D-07, D-15, D-16, D-23, D-24, D-25, KBD-05)"
    - path: "src/components/shell/block_stream.rs"
      provides: "BlockStream refactored: ReadOnlySignal<Vec<BlockEntry>> + EventHandler<u64> for rerun forwarding; clipboard write at child-iteration scope (D-01, D-07, D-23)"
    - path: "src/components/shell/mod.rs"
      provides: "Adds pub mod markdown; declaration"
      contains: "pub mod markdown"
  key_links:
    - from: "block.rs is-ai body"
      to: "markdown::render_inline_code"
      via: "{render_inline_code(&markdown)} braces-into-RSX"
      pattern: "render_inline_code"
    - from: "block_stream.rs copy onclick"
      to: "web_sys::Window navigator clipboard write_text"
      via: "fire-and-forget Promise drop, cfg-gated wasm32 (D-23)"
      pattern: "navigator\\(\\)\\.clipboard\\(\\)\\.write_text"
    - from: "block_stream.rs key attribute"
      to: "BlockEntry.id"
      via: "key: \"{entry.id}\" stable across /clear+append (D-07)"
      pattern: 'key: "\{entry\.id\}"'
---

<objective>
Wave 4 first half — BlockEntry-id propagation through the rendering chain.
Create the new markdown.rs (render_inline_code) helper and refactor the three
files that participate in BlockEntry-id propagation: block.rs (entry-typed
prop + render_inline_code routing for is-ai + delegate copy/rerun), and
block_stream.rs (ReadOnlySignal<Vec<BlockEntry>> + key by entry.id +
clipboard write at child scope + on_rerun forwards entry.id).

Plan 04-04b runs IN PARALLEL with this plan and refactors the four
non-BlockEntry-touching primitives (input_box, command_palette, status_bar,
agent_panel) plus the three-platform compile gate.

Purpose: split the 8-task Wave 4 work into two parallel halves so each plan
stays under the 3-task scope-density ceiling. Both 04-04a and 04-04b share
the same Wave 4 dependency (depends_on: [01, 02, 03]) and run in parallel
because they touch disjoint files (no overlap in files_modified). Plan 04-05
depends on BOTH 04-04a and 04-04b.

Output: 1 NEW file (markdown.rs), 2 MODIFIED component files (block.rs +
block_stream.rs), 1 MODIFIED mod.rs, plus a transient shim edit to
warp_hermes.rs to keep the existing call site type-checking until Plan
04-05 fully replaces the file.

Requirements:
- KBD-05: Copy onclick fires clipboard write; rerun onclick fires for is-cmd; share continues to render.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/ROADMAP.md
@.planning/phases/04-data-layer-interactions/04-CONTEXT.md
@.planning/phases/04-data-layer-interactions/04-RESEARCH.md
@.planning/phases/04-data-layer-interactions/04-PATTERNS.md
@.planning/phases/04-data-layer-interactions/04-VALIDATION.md
@CLAUDE.md
@AGENTS.md
@src/state.rs
@src/components/shell/mod.rs
@src/components/shell/block.rs
@src/components/shell/block_stream.rs
@src/components/shell/scanner.rs
@src/components/warp_hermes.rs

<interfaces>
After Wave 1 (Plan 04-02), `src/state.rs` exposes:

```rust
pub enum Block { Cmd { command }, Out { author, time, text }, Ai { author, time, markdown },
                 Ok { author, time, message }, Err { author, time, exit_code, message },
                 Tool { call } }
pub enum Token { Bin(String), Arg(String), Flag(String), Str(String) }
pub struct BlockEntry { pub id: u64, pub block: Block }
pub fn now_time() -> String;
```

Dioxus 0.7 prop conversions verified:
- `Signal<T>` → `ReadOnlySignal<T>` via `.into()` at the call site (Plan 04-05 wires).
- `EventHandler<T>` accepts `move |v| { ... }` from caller; component invokes `on_x.call(value)`.

web_sys clipboard (RESEARCH Common Op 2):
```rust
let _ = web_sys::window().unwrap().navigator().clipboard().write_text(&text);
// Promise dropped — JS still executes; D-23 fire-and-forget.
```

CURRENT prop signatures Plan 04-04a REPLACES:
```rust
pub fn Block(data: BlockData) -> Element                      // block.rs:26
pub fn BlockStream(blocks: Vec<BlockData>) -> Element         // block_stream.rs:17
```
</interfaces>
</context>

<tasks>

<task type="auto">
  <name>Task 1: Create markdown.rs (render_inline_code) + register in shell/mod.rs</name>
  <files>src/components/shell/markdown.rs, src/components/shell/mod.rs</files>
  <read_first>
    - src/components/shell/scanner.rs (lines 13-25 — small Element-returning component analog; markdown is NOT a component)
    - src/components/shell/mod.rs (lines 1-41 — current re-export pattern)
    - .planning/phases/04-data-layer-interactions/04-CONTEXT.md §"Inline-Code Markdown Rendering" (D-15, D-16)
    - .planning/phases/04-data-layer-interactions/04-RESEARCH.md §"Pattern 6: Inline-code markdown renderer" (verbatim implementation)
    - .planning/phases/04-data-layer-interactions/04-PATTERNS.md §"NEW src/components/shell/markdown.rs"
  </read_first>
  <action>
    **1a — CREATE `src/components/shell/markdown.rs`** with EXACTLY:

    ```rust
    //! Inline-code markdown renderer per CONTEXT D-15 + D-16.
    //!
    //! Plain `pub fn` (NOT a `#[component]`) returning Element. Splits text on
    //! backticks; even-indexed segments render as `<span>`, odd-indexed as
    //! `<code>`. Wrapped in `<div style="white-space: pre-wrap;">` so newlines
    //! in personality replies (e.g., Hype) render correctly. ~20 LOC, no new
    //! crate (full markdown deferred to v2).
    //!
    //! Caller pattern:  rsx! { div { class: "wh-block-body", {render_inline_code(&markdown)} } }

    use dioxus::prelude::*;

    /// Render text with inline `<code>` spans for backtick-delimited segments.
    pub fn render_inline_code(text: &str) -> Element {
        let parts: Vec<&str> = text.split('`').collect();
        rsx! {
            div { style: "white-space: pre-wrap;",
                for (i, seg) in parts.iter().enumerate() {
                    if i % 2 == 0 {
                        span { key: "p{i}", "{seg}" }
                    } else {
                        code { key: "c{i}", "{seg}" }
                    }
                }
            }
        }
    }
    ```

    **1b — Edit `src/components/shell/mod.rs`** — add `pub mod markdown;` line after line 11 (`pub mod command_palette;`) and add the matching `#[allow(unused_imports)] pub use markdown::render_inline_code;` re-export at the end of the existing re-export block (after the `pub use command_palette::CommandPalette;` line).

    Critical: markdown.rs is NOT `#[component]`. Caller invokes via `{render_inline_code(&text)}` braces-into-RSX (block.rs Task 2 wires this). Keys on iterated children mandatory.
  </action>
  <verify>
    <automated>test -f src/components/shell/markdown.rs && grep -q 'pub fn render_inline_code(text: &str) -> Element' src/components/shell/markdown.rs && grep -q "white-space: pre-wrap" src/components/shell/markdown.rs && grep -q 'split(' src/components/shell/markdown.rs && grep -q "pre-wrap" src/components/shell/markdown.rs && grep -q '^pub mod markdown;' src/components/shell/mod.rs && grep -q 'pub use markdown::render_inline_code' src/components/shell/mod.rs</automated>
  </verify>
  <done>
    markdown.rs exists with render_inline_code; mod.rs declares pub mod markdown and re-exports render_inline_code.
  </done>
</task>

<task type="auto">
  <name>Task 2: Refactor block.rs (entry + on_copy/on_rerun + render_inline_code routing)</name>
  <files>src/components/shell/block.rs</files>
  <read_first>
    - src/components/shell/block.rs (current — preserve all RSX layout + class strings + Cmd/Tool delegation)
    - src/state.rs (BlockEntry shape; Block enum variants)
    - .planning/phases/04-data-layer-interactions/04-CONTEXT.md §"Hover Action Handlers" (D-23 clipboard, D-24 rerun, D-25 share stub) + §"Inline-Code Markdown Rendering" (D-16 routes is-ai through render_inline_code)
    - .planning/phases/04-data-layer-interactions/04-RESEARCH.md §"Common Operation 2 — clipboard fire-and-forget"
    - .planning/phases/04-data-layer-interactions/04-PATTERNS.md §"MOD src/components/shell/block.rs"
  </read_first>
  <action>
    REPLACE the entirety of `src/components/shell/block.rs` with EXACTLY:

    ```rust
    use dioxus::prelude::*;
    use crate::state::{Block as BlockData, BlockEntry};
    use super::command_line::CommandLine;
    use super::tool_call::ToolCall;
    use super::markdown::render_inline_code;

    /// Block — outer chrome for any of the six block kinds with a variant-
    /// dispatched body and a hover-action button row.
    ///
    /// Phase 4 (per CONTEXT D-07): prop type changed from `data: BlockData`
    /// to `entry: BlockEntry` (BlockEntry wraps Block with stable `id`).
    /// Adds `on_copy` and `on_rerun` EventHandler props (D-23 / D-24 +
    /// KBD-05). is-ai body now invokes `render_inline_code(&markdown)` per
    /// D-15 + D-16. Share button continues to render but is a click-no-op
    /// per D-25 (Phase 3 visual stays intact).
    ///
    /// Hover affordances are pure CSS per Phase 3 D-15: action buttons
    /// render unconditionally; `assets/warp-ih.css` `.wh-block:hover
    /// .wh-block-actions { opacity: 1 }` controls visibility.
    #[component]
    pub fn Block(
        entry: BlockEntry,
        on_copy: EventHandler<()>,
        on_rerun: EventHandler<()>,
    ) -> Element {
        let data = entry.block.clone();
        let kind_class = data.kind_class();

        let (author, time, exit_code) = match &data {
            BlockData::Cmd { command } => (None, command.time.clone(), None),
            BlockData::Out { author, time, .. } => (author.clone(), time.clone(), None),
            BlockData::Ai  { author, time, .. } => (author.clone(), time.clone(), None),
            BlockData::Ok  { author, time, .. } => (author.clone(), time.clone(), None),
            BlockData::Err { author, time, exit_code, .. } => (author.clone(), time.clone(), Some(*exit_code)),
            BlockData::Tool { .. } => (None, None, None),
        };
        let is_ok  = matches!(data, BlockData::Ok  { .. });
        let is_err = matches!(data, BlockData::Err { .. });
        let is_cmd = matches!(data, BlockData::Cmd { .. });

        rsx! {
            div { class: "wh-block {kind_class}",
                div { class: "wh-block-head",
                    if let Some(author) = author { span { class: "wh-author", "{author}" } }
                    if is_ok  { span { style: "color: var(--success); font-size: 10px;", "[OK]" } }
                    if is_err {
                        span {
                            style: "color: var(--danger); font-size: 10px;",
                            if let Some(code) = exit_code { "exit {code}" } else { "exit 1" }
                        }
                    }
                    if let Some(t) = time { span { style: "margin-left: auto; font-size: 11px; color: var(--fg-dim);", "{t}" } }
                }
                // Variant-dispatched body
                match data.clone() {
                    BlockData::Cmd { command } => rsx! {
                        CommandLine {
                            tokens: command.tokens,
                            time: command.time,
                            cwd: command.cwd,
                            glyph: command.glyph,
                        }
                    },
                    BlockData::Tool { call } => rsx! {
                        ToolCall {
                            name: call.name,
                            args_summary: call.args_summary,
                            status: call.status,
                        }
                    },
                    BlockData::Out { text, .. } => rsx! { div { class: "wh-block-body", "{text}" } },
                    BlockData::Ai  { markdown, .. } => rsx! { div { class: "wh-block-body", {render_inline_code(&markdown)} } },
                    BlockData::Ok  { message, .. } => rsx! { div { class: "wh-block-body", "{message}" } },
                    BlockData::Err { message, .. } => rsx! { div { class: "wh-block-body", "{message}" } },
                }
                // Hover-action button row — always rendered.
                // Cmd: copy + rerun. Non-Cmd: copy + rerun (disabled) + share.
                div { class: "wh-block-actions",
                    button {
                        class: "wh-icon-btn",
                        title: "copy",
                        onclick: move |_| on_copy.call(()),
                        "⎘"
                    }
                    button {
                        class: "wh-icon-btn",
                        title: "rerun",
                        style: if !is_cmd { "cursor: not-allowed; opacity: 0.5;" } else { "" },
                        onclick: move |_| { if is_cmd { on_rerun.call(()); } },
                        "↻"
                    }
                    if !is_cmd {
                        button {
                            class: "wh-icon-btn",
                            title: "share",
                            onclick: move |_| { /* D-25: share unwired in Phase 4; v2 needs real share-link backend. */ },
                            "↗"
                        }
                    }
                }
            }
        }
    }
    ```

    Critical (per PATTERNS file risks):
    - `entry.block.clone()` is necessary because the existing `match data.clone()` body chains were written for owned values; preserving them keeps RSX byte-identical.
    - `EventHandler<()>` props are closures with `'static` lifetime; do NOT wrap in Signal<>.
    - `is_cmd` boolean drives the rerun-button disabled state per D-24 — non-Cmd renders rerun greyed-out (cursor: not-allowed; opacity 0.5) and the onclick is a no-op.
    - The clipboard write itself lives in BlockStream (Task 3) — Block only fires `on_copy.call(())` to delegate. This keeps Block free of `web_sys` imports and lets BlockStream hold the entry-id → text mapping.
    - The is-ai body change is `{render_inline_code(&markdown)}` (braces-into-RSX) — replaces the prior `"{markdown}"` plain-text interpolation per D-16.
    - DO NOT remove the share button — D-25 explicit ("plan should NOT remove the button to avoid Phase 3 visual regression").
  </action>
  <verify>
    <automated>grep -q 'pub fn Block(' src/components/shell/block.rs && grep -q 'entry: BlockEntry,' src/components/shell/block.rs && grep -q 'on_copy: EventHandler<()>,' src/components/shell/block.rs && grep -q 'on_rerun: EventHandler<()>,' src/components/shell/block.rs && grep -q 'use super::markdown::render_inline_code;' src/components/shell/block.rs && grep -q 'render_inline_code(&markdown)' src/components/shell/block.rs && grep -q 'on_copy.call(())' src/components/shell/block.rs && grep -q 'on_rerun.call(())' src/components/shell/block.rs && grep -q 'cursor: not-allowed' src/components/shell/block.rs && grep -q 'title: "share"' src/components/shell/block.rs && ! grep -q 'data: BlockData' src/components/shell/block.rs</automated>
  </verify>
  <done>
    block.rs Block() takes (entry, on_copy, on_rerun); is-ai body uses render_inline_code; copy/rerun delegate to event handlers; share button preserved as click-no-op; rerun button greyed-out for non-Cmd.
  </done>
</task>

<task type="auto">
  <name>Task 3: Refactor block_stream.rs (ReadOnlySignal<Vec<BlockEntry>> + EventHandler<u64> forwarding + clipboard mapping) + transient warp_hermes.rs shim</name>
  <files>src/components/shell/block_stream.rs, src/components/warp_hermes.rs</files>
  <read_first>
    - src/components/shell/block_stream.rs (current — preserve wh-stream wrapper structure)
    - src/components/warp_hermes.rs (current Wave 2 shim — single line that calls BlockStream needs adapter to Vec<BlockEntry>)
    - src/state.rs (BlockEntry, Block, Token, ToolCall types)
    - .planning/phases/04-data-layer-interactions/04-CONTEXT.md §"Block Identity for Stable RSX Keys" (D-07/D-08 stable keys) + §"Hover Action Handlers" (D-23 copy text assembly)
    - .planning/phases/04-data-layer-interactions/04-PATTERNS.md §"MOD src/components/shell/block_stream.rs"
    - .planning/phases/04-data-layer-interactions/04-RESEARCH.md §"Common Operation 2 — clipboard fire-and-forget"
  </read_first>
  <action>
    Two file edits — block_stream.rs becomes the canonical Wave 4 prop shape; warp_hermes.rs receives a transient shim so the existing call site type-checks.

    # This task makes a transient shim edit to warp_hermes.rs; Plan 04-05 will REPLACE the file entirely.

    **3a — REPLACE the entirety of `src/components/shell/block_stream.rs`** with EXACTLY:

    ```rust
    use dioxus::prelude::*;
    use crate::state::{Block as BlockData, BlockEntry, Token};
    use super::block::Block;

    /// Block stream — owns the `wh-stream` / `wh-stream-scroll` chrome and
    /// iterates over a reactive `ReadOnlySignal<Vec<BlockEntry>>` per
    /// CONTEXT D-01.
    ///
    /// Per CONTEXT D-07: each iterated child uses `key: "{entry.id}"` for
    /// stable identity across `/clear` (Vec emptied) + append cycles. The
    /// old `key: "{i}"` index-based key would collide on append.
    ///
    /// Copy/rerun handlers live here (not in Block) so Block stays free of
    /// `web_sys` imports. Block fires `on_copy.call(())` and BlockStream
    /// resolves the entry id → text mapping per CONTEXT D-23, then writes
    /// to the browser clipboard fire-and-forget. `on_rerun: EventHandler<u64>`
    /// forwards the entry id up to WarpHermes which dispatches `run_shell`.
    #[component]
    pub fn BlockStream(
        blocks: ReadOnlySignal<Vec<BlockEntry>>,
        on_rerun: EventHandler<u64>,
    ) -> Element {
        rsx! {
            div { class: "wh-stream",
                div { class: "wh-stream-scroll",
                    for entry in blocks.read().iter().cloned() {
                        Block {
                            key: "{entry.id}",
                            entry: entry.clone(),
                            on_copy: {
                                let entry_for_copy = entry.clone();
                                move |_| {
                                    let text = block_text_for_copy(&entry_for_copy);
                                    write_to_clipboard(&text);
                                }
                            },
                            on_rerun: move |_| on_rerun.call(entry.id),
                        }
                    }
                }
            }
        }
    }

    /// Assemble the copy-text for a block per CONTEXT D-23.
    ///
    /// Cmd → tokens joined by space; Out/Ai/Ok/Err → message/text/markdown;
    /// Tool → "{name} {args_summary}".
    fn block_text_for_copy(entry: &BlockEntry) -> String {
        match &entry.block {
            BlockData::Cmd { command } => command
                .tokens
                .iter()
                .map(token_text)
                .collect::<Vec<_>>()
                .join(" "),
            BlockData::Out { text, .. } => text.clone(),
            BlockData::Ai  { markdown, .. } => markdown.clone(),
            BlockData::Ok  { message, .. } => message.clone(),
            BlockData::Err { message, .. } => message.clone(),
            BlockData::Tool { call } => format!("{} {}", call.name, call.args_summary),
        }
    }

    fn token_text(t: &Token) -> String {
        match t {
            Token::Bin(s) | Token::Arg(s) | Token::Flag(s) | Token::Str(s) => s.clone(),
        }
    }

    /// Fire-and-forget clipboard write per CONTEXT D-23.
    ///
    /// Failures (no clipboard, permission denied) are silent — Phase 4 has no
    /// toast/feedback UI per D-23. The returned Promise is dropped; JS engine
    /// still executes per RESEARCH Common Op 2 + Assumptions Log A4.
    fn write_to_clipboard(text: &str) {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(window) = web_sys::window() {
                let _ = window.navigator().clipboard().write_text(text);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Native build: no-op (clipboard wiring is web-only in Phase 4).
            let _ = text;
        }
    }
    ```

    **3b — TRANSIENT SHIM EDIT to `src/components/warp_hermes.rs`** to keep the existing call site type-checking against the new BlockStream prop signature. The Wave 2 shim called `BlockStream { blocks: blocks }` with `blocks: Vec<Block>`; the new signature requires `blocks: ReadOnlySignal<Vec<BlockEntry>>`. Apply the minimal patch:

    1. Replace the `let blocks = ...` line with: `let blocks = use_signal(crate::state::demo_block_entries);`
    2. Update the `BlockStream { ... }` call site to: `BlockStream { blocks: blocks.into(), on_rerun: move |_id: u64| {} }`
    3. Add `crate::state::BlockEntry` to the imports if missing.

    This shim is intentionally minimal — Plan 04-05 fully replaces warp_hermes.rs with the WarpHermes integration body. Do NOT add additional Signal scaffolding here; that work belongs to Plan 04-04b (which adds shims for input_box / status_bar / agent_panel / command_palette) and Plan 04-05 (full integration). Plans 04-04a and 04-04b each touch warp_hermes.rs only enough to keep the file compiling against the partial set of refactored primitive prop signatures from THIS plan; Plan 04-05 then replaces the whole file.

    Note the inter-plan coordination: 04-04a (this plan) and 04-04b BOTH edit warp_hermes.rs. Because they run in parallel, the executor must apply this shim AFTER syncing with whatever shim 04-04b applied (or, if 04-04b runs second, 04-04b absorbs this plan's BlockStream shim into its own combined shim).

    Critical (per RESEARCH + PATTERNS):
    - `blocks.read().iter().cloned()` iterates owned BlockEntries; cloning is necessary because each child closure captures via `move`.
    - `key: "{entry.id}"` MUST use the BlockEntry id (D-07/D-08 stable keys). Old `key: "{i}"` (index-based) would collide on append.
    - The `entry_for_copy` rebind is required because the same `entry` is moved into both `on_copy` and `on_rerun` closures within the same iteration — clone-then-move.
    - The clipboard write is cfg-gated (wasm32 only) so the desktop/mobile compile gate stays green per RESEARCH Pitfall 4. `web_sys::window()` is wasm-only.
    - We do NOT remove the rerun forwarding for non-Cmd entries — the receiving WarpHermes pick handler short-circuits via `is_cmd` check (Plan 04-05). BlockStream forwards every rerun click; semantic gating happens upstream.
  </action>
  <verify>
    <automated>grep -q 'pub fn BlockStream(' src/components/shell/block_stream.rs && grep -q 'blocks: ReadOnlySignal<Vec<BlockEntry>>,' src/components/shell/block_stream.rs && grep -q 'on_rerun: EventHandler<u64>,' src/components/shell/block_stream.rs && grep -q 'key: "{entry.id}"' src/components/shell/block_stream.rs && grep -q 'fn block_text_for_copy' src/components/shell/block_stream.rs && grep -q 'fn write_to_clipboard' src/components/shell/block_stream.rs && grep -q 'navigator().clipboard().write_text' src/components/shell/block_stream.rs && grep -q '#\[cfg(target_arch = "wasm32")\]' src/components/shell/block_stream.rs && ! grep -q 'blocks: Vec<BlockData>' src/components/shell/block_stream.rs && grep -q 'BlockStream' src/components/warp_hermes.rs</automated>
  </verify>
  <done>
    block_stream.rs takes ReadOnlySignal<Vec<BlockEntry>> + on_rerun: EventHandler<u64>; uses entry.id as key; clipboard write is cfg-gated wasm32; copy text assembly fn matches D-23. warp_hermes.rs shim updated minimally to keep BlockStream call site type-checking.
  </done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| Block content → OS clipboard | block_stream.rs Task 3 calls `web_sys` clipboard API with developer-controlled mock content. Phase 4 has no XSS surface (no untrusted input source). |
| Markdown text → DOM render | markdown.rs splits on backtick and emits alternating `<span>`/`<code>`. Dioxus RSX `"{seg}"` interpolation auto-escapes via the framework's text-node renderer. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-04-01 | Information Disclosure | block_stream.rs Task 3 (clipboard write) | accept | Block content is developer-authored mock data in Phase 4 (no XSS surface). v2 hardening: sanitize block text before clipboard write; rely on browser's user-gesture requirement (already enforced). |
| T-04-NA-01 | n/a | markdown.rs Task 1 (inline code render) | n/a | Pure-fn that splits on backtick and renders alternating `<span>`/`<code>`. No HTML escaping needed — Dioxus RSX `"{seg}"` interpolation auto-escapes via the framework's text-node renderer. |

</threat_model>

<verification>
- markdown.rs created and registered in shell/mod.rs.
- block.rs accepts (entry, on_copy, on_rerun); is-ai → render_inline_code; share preserved as no-op.
- block_stream.rs accepts (blocks: ReadOnlySignal<Vec<BlockEntry>>, on_rerun: EventHandler<u64>); key=entry.id; clipboard write cfg-gated wasm32.
- warp_hermes.rs receives the transient BlockStream-shim edit so the file still compiles; Plan 04-05 replaces the file entirely.
- Three-platform compile gate is run by Plan 04-04b Task 5 (the shared exit gate for both halves of Wave 4).
</verification>

<success_criteria>
Plan 04-04a complete when:
- [ ] markdown.rs created with render_inline_code (D-15) and registered in mod.rs.
- [ ] block.rs accepts (entry: BlockEntry, on_copy, on_rerun); is-ai → render_inline_code; is_cmd → rerun enabled, others → rerun greyed; share preserved as no-op (D-23/24/25, KBD-05).
- [ ] block_stream.rs accepts (blocks: ReadOnlySignal<Vec<BlockEntry>>, on_rerun: EventHandler<u64>); key=entry.id; clipboard write cfg-gated wasm32; copy text assembly per D-23.
- [ ] warp_hermes.rs transient shim updated so BlockStream call site type-checks.
- [ ] (Compile gate is shared with 04-04b Task 5 — that plan owns the wave-end gate.)
</success_criteria>

<output>
After completion, create `.planning/phases/04-data-layer-interactions/04-04a-SUMMARY.md`
including a "Wave 4 Coordination Note" section documenting that Plan 04-04b
ran (or runs) in parallel and shares the warp_hermes.rs shim, with the
final compile gate owned by Plan 04-04b Task 5.
</output>
