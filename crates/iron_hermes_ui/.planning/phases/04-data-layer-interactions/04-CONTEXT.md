# Phase 4: Data Layer & Interactions - Context

**Gathered:** 2026-05-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Wire reactivity, mock data flows, and keyboard/click interactions into the Phase 3 desktop shell. Concretely: introduce `Signal<T>`/`ReadOnlySignal<T>` props across every shell primitive that mutates (input value, blocks, messages, mode, palette open/query, scanner active, focused, tabs.active, tokens); install one `use_context_provider` bundle (`ShellSettings { personality }`) read by `AgentPanel` and `StatusBar`; ship the mock data layer (six personalities × one canned reply each per MOCK-01, prototype-matching `runShell`/`runAgent` timings 600 ms / 400 ms / 1400 ms per MOCK-02/MOCK-03, +120-per-submission saturating token counter per MOCK-04); register all keyboard handlers (KBD-01 ⌥M mode toggle when input focused, KBD-02 ⌘K/Ctrl-K palette open + Esc close, KBD-03 ↑/↓/Enter palette navigation, KBD-04 Enter/Shift-Enter input submission, KBD-05 hover-affordance copy/rerun handlers, KBD-06 personality switch via `/personality` palette-substate); add a new `src/platform/timer.rs` cfg-gated `sleep(ms)` (gloo-timers on wasm32, tokio::time::sleep on native) so the three-platform compile gate stays green.

What this phase does NOT do: add markdown rendering beyond inline backtick→`<code>`, ship `data-theme`/`data-density`/`data-block`/`data-agent` runtime switching (Phase 5), build the TweaksPanel (Phase 5), render the mobile shell variant (Phase 6), call any real backend (REAL-01..REAL-04 are v2), persist any state across reloads (PERS-01/02 are v2), or extend the `ShellSettings` context bundle beyond `personality` (Phase 5 owns that extension). The scanner inverts default from Phase 3: `is-active` is now class-toggled by a `Signal<bool>` for ~1400 ms post-submission rather than always-on.

</domain>

<decisions>
## Implementation Decisions

### State Ownership Topology
- **D-01:** Hybrid state model. Local `Signal<T>` declared in `WarpHermes()` for: `input: Signal<String>`, `blocks: Signal<Vec<BlockEntry>>`, `messages: Signal<Vec<Message>>`, `mode: Signal<Mode>`, `pal_open: Signal<bool>`, `pal_query: Signal<String>`, `pal_state: Signal<PaletteState>` (Browse | PersonalityPick), `scanner_active: Signal<bool>`, `focused: Signal<bool>`, `active_tab: Signal<usize>`, `tokens: Signal<TokenBudget>`, `next_id: Signal<u64>`. Each is passed to children as `Signal<T>` (mutating consumer) or `ReadOnlySignal<T>` (read-only consumer) props. No prop-drilling layers because the shell is two levels deep at most.
- **D-02:** ONE `use_context_provider` bundle: `ShellSettings { personality: Signal<Personality> }` — provided in `WarpHermes()`. `AgentPanel` and `StatusBar` consume it via `use_context::<ShellSettings>()`. Phase 5 extends the same struct with `theme: Signal<Theme>`, `density: Signal<Density>`, `block: Signal<BlockStyle>`, `agent: Signal<AgentLayout>` — the struct shape is FORWARD-COMPATIBLE so Phase 5 only adds fields, never refactors existing ones.
- **D-03:** New enum `Personality { Concise, Technical, Noir, Hype, Catgirl, Default }` lives in `src/state.rs`, derives `Clone + Copy + PartialEq + Debug + Eq + Hash`. `Default` derives via `#[default]` annotation on the `Default` variant. The string lookup `Personality::label() -> &'static str` returns the lowercase slug used in palette items and status hints.

### Async Timer Primitive + Cargo Deltas
- **D-04:** New file `src/platform/timer.rs` exposing `pub async fn sleep(ms: u32)`. Implementation:
  - `#[cfg(target_arch = "wasm32")]` → `gloo_timers::future::TimeoutFuture::new(ms).await;`
  - `#[cfg(not(target_arch = "wasm32"))]` → `tokio::time::sleep(std::time::Duration::from_millis(ms.into())).await;`
  - Single import for callers: `use crate::platform::timer::sleep;`. Cgs are at the function level inside the file — one function, two bodies.
- **D-05:** `Cargo.toml` deltas — add to `[dependencies]`:
  - `gloo-timers = { version = "0.3", features = ["futures"] }` (wasm32 only — no need to feature-gate; the cfg-gated impl is the gate)
  - `tokio = { version = "1", features = ["time"], default-features = false }` (NOT `full` — we only need the timer driver; saves substantial native binary size)
  - `web-sys = { version = "0.3", features = ["Window", "Element", "Navigator", "Clipboard", "EventTarget", "KeyboardEvent"] }` — for keyboard handler, clipboard, scroll-into-view (transitively present through dioxus but adding it explicitly with the features we need is cleaner than leaning on transitive feature unification)
  - `wasm-bindgen = "0.2"` — already transitively present; declared explicitly only if `web-sys` features require it (planner verifies during research)
  - `js-sys` — same; declared only if needed
  - The three-platform compile gate (`cargo build --features web/desktop/mobile`) MUST stay green. This is the canonical Phase 4 verification gate.
- **D-06:** No signal `.read()` / `.write()` borrows held across `.await` points (clippy.toml enforces). Pattern for `runShell`/`runAgent`:
  ```rust
  let cmd_text = input.read().clone();          // borrow scope: this line only
  input.set(String::new());                     // separate write call
  blocks.write().push(BlockEntry { ... });      // write borrow drops at semicolon
  sleep(600).await;                             // no borrows live here
  blocks.write().push(BlockEntry { ... });      // fresh borrow
  ```
  Every async function in Phase 4 follows this pattern — read or clone before the await, never hold `.read()` / `.write()` across one.

### Block Identity for Stable RSX Keys
- **D-07:** New wrapper struct `pub struct BlockEntry { pub id: u64, pub block: Block }` in `src/state.rs`. The `Block` enum stays as a pure data shape; identity is wrapper concern. RSX uses `key: "{entry.id}"` per iteration so `/clear` (which empties the Vec) and append (which grows it) both maintain stable keys.
- **D-08:** `next_id: Signal<u64>` initialized at `1000` (above the demo seeds' 1..N). Every new `BlockEntry` reads `next_id`, clones it, then writes `next_id + 1`. Never held across await.
- **D-09:** `demo_blocks() -> Vec<Block>` becomes `demo_block_entries() -> Vec<BlockEntry>` returning entries with ids 1..10 (matching the existing `b1..b10` strings from app.jsx — but as numbers). Old function signature deleted, not deprecated; this is a Phase 4 cohesive refactor.

### Mock Data Layer Module Layout
- **D-10:** New module hierarchy `src/mocks/`:
  - `src/mocks/mod.rs` — re-exports `personalities`, `shell_outputs`, `agent_steps`, plus public `pub async fn run_shell(...)` and `pub async fn run_agent(...)` entry points.
  - `src/mocks/personalities.rs` — `pub const REPLIES: [(Personality, &str); 6]` mapping each variant to its single canned `fakeAgentReply` string (verbatim from app.jsx lines 342-348).
  - `src/mocks/shell_outputs.rs` — `pub fn fake_shell_out(text: &str, time: &str) -> Block` matching prototype's keyword routing: starts-with `git status` → Ok block with the prototype's hardcoded git-status output, `cargo` → Ok block with cargo build output, `ls` → Out block with directory listing, else Ok block with `(simulated) ran: <text>`.
  - `src/mocks/agent_steps.rs` — `pub async fn run_agent_steps(prompt: &str, personality: Personality, messages: Signal<Vec<Message>>) -> ()` performs the three-step chain: append user message → sleep(400) → append hermes-with-tool-call message (search tool, query=first 40 chars of prompt) → sleep(1000) → append hermes reply pulled from `personalities::REPLIES`.
- **D-11:** `state.rs` keeps `demo_block_entries()`, `demo_messages()`, `demo_palette_items()`, `demo_tabs()` — these stay as INITIAL seeds shown on first render. They are NOT re-emitted by mocks; they're the starting state. After the user's first submission they coexist with mock-emitted blocks/messages.
- **D-12:** `runShell(text)` flow:
  1. Tokenize: split on whitespace; first token = `Token::Bin(s)`; tokens starting with `-` (any prefix) → `Token::Flag(s)`; else → `Token::Arg(s)`. (`Token::Str` deferred to v2.)
  2. Append `BlockEntry { id, block: Block::Cmd { command: CommandLine { tokens, time: Some("…".into()), cwd: None, glyph: Some("❯".into()) } } }` to blocks.
  3. `pulse_scanner(2000).await` (kicks scanner_active true; spawns a task to flip it false after 2000 ms).
  4. `sleep(600).await`.
  5. Append `BlockEntry { id, block: fake_shell_out(text, now_time()) }` to blocks.

### Submit Routing & Tokenization
- **D-13:** `submit()` reads `input` clone, trims, returns early if empty. Reads `mode` clone. Sets input to empty. Calls `pulse_scanner(2000)`. If `mode == Mode::Agent` → `run_agent_steps(text, personality, messages).await`. Else → `run_shell(text).await`. Tokenization strictly follows D-12 step 1.
- **D-14:** `pulse_scanner(ms)` flow: write `scanner_active = true`; spawn an async task: `sleep(ms).await; scanner_active.set(false)`. Multiple submissions overlap by re-setting true (the most-recent task wins on the false-set timing — deviation from prototype's `clearTimeout(_t)` JS-ism, but functionally equivalent because each spawn re-affirms `true` before the previous task fires `false`). Worst case: one extra "true→true" reactive cycle.

### Inline-Code Markdown Rendering
- **D-15:** New file `src/components/shell/markdown.rs` exposing `pub fn render_inline_code(text: &str) -> Element` (Element fragment). Logic: split `text` on backtick (`\``) pairs alternating; even-indexed segments render as plain `<span>{seg}</span>` with `white-space: pre-wrap`; odd-indexed segments render as `<code>{seg}</code>`. Mismatched/unclosed backticks fall back to plain text. ~25 LOC including the splitter helper. No new crate.
- **D-16:** `Block` component's `is-ai` body invokes `render_inline_code(markdown)` instead of plain text rendering. Other variants stay plain. The `<pre>`-vs-`<div>` discretion from Phase 3 D-16 is now resolved: the function returns an `<div style="white-space: pre-wrap">` containing the alternating spans.

### Keyboard Handler Placement
- **D-17:** Global keyboard handler installed in `WarpHermes()` via `use_effect` + `web_sys::window().add_event_listener_with_callback("keydown", ...)`. Cleanup via `use_drop`. Listener fires on:
  - `(metaKey || ctrlKey) && key.toLowerCase() == "k"` → toggle `pal_open` and `pal_query` reset; preventDefault.
  - `key == "Escape"` → close palette (set `pal_open = false`, `pal_state = Browse`).
  - `altKey && key.toLowerCase() == "m"` → toggle `mode` ONLY when `focused == true`. (Bare ⌥M without focus would intercept generic typing.)
- **D-18:** Palette navigation (↑/↓/Enter) lives on the palette overlay's `onkeydown` (not global) so it only fires when the palette is open. ↑/↓ updates a local `Signal<usize> selected` index; Enter dispatches `pick(items[selected])`. Wraps at boundaries (idx=0 + ↑ → idx=last).
- **D-19:** Input submission (Enter / Shift+Enter) lives on the textarea's `onkeydown`. Shift+Enter inserts a newline naturally (default textarea behavior); plain Enter calls `submit()` and preventDefaults. The textarea is the only input surface in Phase 4 — palette query input has its own onkeyup handler for live filtering.

### Personality Switching UX
- **D-20:** New `Signal<PaletteState>` enum with variants `Browse` (default — full PALETTE_ITEMS list) and `PersonalityPick` (shows the 6 personalities as palette rows). Selecting `/personality` while in `Browse` transitions to `PersonalityPick` (palette stays open). Selecting a personality writes `ShellSettings.personality` and transitions back to `Browse` then closes palette. Esc from `PersonalityPick` returns to `Browse` without closing.
- **D-21:** Personality reply table is 1:1 with `fakeAgentReply` — six strings, one per variant, prompt-agnostic. Stored in `src/mocks/personalities.rs` as a `[(Personality, &str); 6]` const array. `pick_reply(personality) -> &'static str` returns the matching string. Phase 4 does NOT add round-robin or keyword matching — visual fidelity to the prototype outranks demo richness.
- **D-22:** Status bar gets a NEW "personality" pill displaying the current `personality.label()` value. Read-only in Phase 4 (no click handler — that lands with TweaksPanel in Phase 5). Visible affordance for "the personality switched" feedback after `/personality` selection.

### Hover Action Handlers
- **D-23:** `copy ⎘` button onclick: assemble block content (Cmd → tokens joined by space; Out/Ai/Ok/Err → message/text/markdown; Tool → `format!("{name} {args_summary}")`) and call `web_sys::window().navigator().clipboard().write_text(&text)`. The promise is `.then`-chained to a no-op (fire-and-forget); failures (permission denied, no clipboard) are silent — no toast, no error block. Phase 4 is pre-feedback-UI scope.
- **D-24:** `rerun ↻` button onclick: enabled ONLY for `Block::Cmd` variants (other variants render the rerun button greyed-out via CSS). On click, joins the original `command.tokens` back into a string and calls `run_shell(text).await` via `spawn`. Other block kinds: button is rendered (per Phase 3 D-15 unconditional rendering) but click is no-op + `cursor: not-allowed` styling.
- **D-25:** `share ↗` button NOT wired in Phase 4. The button continues to render (Phase 3 stub) but click is a no-op. KBD-05's "fire copy/rerun/share handlers" is satisfied by copy + rerun firing; share is a v2 concern (real share-link backend). Documented in deferred ideas; Phase 4 plan should NOT remove the button to avoid Phase 3 visual regression.

### Token Counter
- **D-26:** `pulse_token(amount: u32)` increments `tokens.write().used` saturating at `tokens.read().max` (128_000). Default amount per submission: `120` (compromise between 100/150 prototype-feel; not message-length-derived). Single call site: `submit()` after the input clear, before the runShell/runAgent dispatch.

### Palette Pick Handlers (match prototype `pick(item)`)
- **D-27:** `/clear` → `blocks.set(Vec::new())`; close palette.
- **D-28:** `/status` → append one `BlockEntry { Out { author: Some("ironhermes"), time: now_time(), text: STATUS_TEXT } }`; close palette. `STATUS_TEXT` const lives in `src/mocks/shell_outputs.rs`, verbatim from app.jsx lines 25-36.
- **D-29:** `/help` → append one `BlockEntry { Out { author: Some("help"), time: now_time(), text: <generated list of slash commands from PALETTE_ITEMS> } }`; close palette.
- **D-30:** `/personality` → transition `pal_state = PersonalityPick` (per D-20); palette stays open.
- **D-31:** `/doctor`, `/quit`, and all `workflow` items → fill input box (`input.set(item.cmd)`); close palette. Submission is user-driven (Enter); Phase 4 does not auto-fire.
- **D-32:** Live palette filtering on `pal_query` change: substring match against `cmd` and `label` fields, case-insensitive. No fuzzy match (10 items doesn't justify the dependency).

### Auto-Scroll
- **D-33:** Stream auto-scroll on new blocks: `use_effect` watching `blocks.read().len()` calls `scroll_into_view_with_bool(false)` on the last `.wh-block` element via `web_sys::Element` query (`.wh-stream-scroll .wh-block:last-child`). Fires on submission, `/status`, `/help`, mock outputs. Does NOT fire on `/clear` (Vec is empty). Without it, new blocks land below the visible viewport.

### `now_time()` Helper
- **D-34:** Single helper `pub fn now_time() -> String` in `src/state.rs` returning `HH:MM:SS` from `js_sys::Date::new_0()` on wasm32 and `chrono::Local::now()` on native. Cfg-gated like `timer::sleep`. Used by `runShell`/`runAgent` outputs and palette `/status` and `/help` blocks to give realistic timestamps. (Optional: declare `chrono` and `js_sys` as new deps; alternative is to hardcode `"00:00:00"` in mock outputs — planner picks based on bundle-size budget.)

### Three-Platform Compile Gate
- **D-35:** Phase 4 verification gate (matches Phase 1 Plan 01-03 pattern): `cargo build --features web` + `cargo build --features desktop` + `cargo build --features mobile` MUST all succeed. The cfg-gated timer is the only platform-conditional code; everything else compiles uniformly. Web feature is the runtime UAT target (where the actual mock interactions can be tested side-by-side with `Warp × IronHermes.html`).

### ironhermes Integration Boundary (forward-looking)
- **D-36:** Phase 4 stays mocks-only per PROJECT.md "No external services" and REQUIREMENTS.md REAL-01..REAL-04 v2. BUT mock function signatures should mirror the v2 server-fn shapes:
  - `pub async fn run_shell(cmd: String, blocks: Signal<Vec<BlockEntry>>) -> ()` — Vec append via signal write
  - `pub async fn run_agent_steps(prompt: String, personality: Personality, messages: Signal<Vec<Message>>) -> ()` — same pattern
  - v2 swap: replace function bodies with `dioxus_fullstack` server-function calls into `ironhermes-agent`. Type vocabulary (`Block`, `ToolCall`, `ToolStatus { Pending|Running|Done|Failed }`) already matches `ironhermes-tools` (verified). Result: v2 integration is impl-only; no type churn, no signature churn.
- **D-37:** No `trait ShellRunner` / `trait AgentRunner` abstractions in Phase 4. Phase 4 ships concrete mock impls; v2 introduces traits IF the swap reveals they're useful (likely they aren't — server-fn injection happens at the function boundary, not at trait dispatch).

### Claude's Discretion
- **Personality enum traits beyond D-03:** if the planner finds a use for `IntoIterator` over personalities (palette substate enumeration), implement it via a small helper rather than `strum`. No new crate.
- **`spawn` vs `use_future` for `pulse_scanner`:** Dioxus 0.7 has both — planner picks based on whether the task should be cancellable on unmount. `pulse_scanner` doesn't need cancellation (the timeout fires once); `spawn` is the natural fit.
- **Substring filter case-folding:** simple `to_lowercase()` on both sides; no Unicode-aware case folding. 10-item palette doesn't warrant it.
- **`/help` rendered output style:** plain-text formatted list (one slash command per line, `cmd` left-aligned, `label` right-padded). Planner reads app.jsx line 196-208 `pick("/help")` for exact prototype layout.

### Folded Todos
None — `cross_reference_todos` returned no matches for Phase 4.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Project-level governance
- `.planning/PROJECT.md` — core value (pixel-perfect to prototype is the primary failure mode); Constraints (no `cx`/`Scope`/`use_state`; multi-platform features; **no API keys, no network calls in v1 — mocks only**); Out of Scope (real shell/LLM, auth, persistence, SSR, test suite — all v2)
- `.planning/REQUIREMENTS.md` §"Keyboard & Interactions" (KBD-01..KBD-06) and §"Mocked Data Layer" (MOCK-01..MOCK-04) — the 10 acceptance criteria this phase delivers
- `.planning/REQUIREMENTS.md` §"v2 Requirements" — REAL-01..REAL-04 (real backends), PERS-01/02 (persistence), TEST-01..03 (tests) — explicit guardrails for what Phase 4 does NOT do
- `.planning/ROADMAP.md` §"Phase 4: Data Layer & Interactions" — goal statement and 6 success criteria (Enter→is-cmd→is-out timing; ⌥M mode toggle; ⌘K/Esc/↑/↓/Enter palette; Shift+Enter newline; personality preset switching; token counter increment)
- `.planning/phases/02-design-system/02-CONTEXT.md` — Phase 2 locked decisions (CSS load order; brand assets; scanner-svg disposition)
- `.planning/phases/03-desktop-shell/03-CONTEXT.md` — **Phase 3 D-06 (no `use_signal` in Phase 3) is now inverted in Phase 4**; D-08 (scanner is now class-toggled, not always-on); D-15 (hover affordances unconditionally rendered, click handlers added now); D-16 (markdown deferral resolved here via D-15 inline-code spans); component prop signatures from Phase 3 are TO BE REFACTORED to `Signal<T>` / `ReadOnlySignal<T>` for mutating consumers
- `CLAUDE.md` — Dioxus 0.7 conventions; `clippy.toml` enforces NO signal `.read()`/`.write()` borrows held across `.await` (CRITICAL for Phase 4's first async code in the project)
- `AGENTS.md` — Dioxus 0.7 API reference for `use_signal`, `use_memo`, `use_effect`, `use_context_provider`/`use_context`, `spawn`, signal lifetimes, `ReadOnlySignal<T>`, prop bounds (`PartialEq + Clone`)

### Prototype source of truth (READ-ONLY — never compile from here)
- `warp2ironhermes/project/app/app.jsx` — primary `WarpHermes` shell with state model, submit routing, palette pick handlers (lines 187-210), `pulseScanner` (lines 144-148), `runShell` (lines 163-174), `runAgent` (lines 176-185), `fakeShellOut` keyword routing (lines 309-337), `fakeAgentReply` (lines 339-349). **Verbatim source of truth for D-10..D-32.**
- `warp2ironhermes/project/app/shell.jsx` — input box, palette, scanner primitives — Phase 3 already mapped these to `src/components/shell/`. Phase 4 adds onclick / onkeydown / oninput handlers, no new components.
- `warp2ironhermes/project/Warp × IronHermes.html` — interactive UAT target. Phase 4 success means clicking through the prototype HTML in one browser tab and `dx serve --features web` in another reveals matching behavior on every interaction listed in success criteria.
- `warp2ironhermes/project/styles/warp-ih.css` (already ported to `assets/warp-ih.css`) — class names: `.wh-block-actions`, `.wh-icon-btn`, `.wh-input-textarea`, palette `.wh-palette-*`, scanner `.wh-scanner-*`. Phase 4 adds NO new CSS — all interaction states are already styled.
- `warp2ironhermes/project/ironhermes/colors_and_type.css` (already ported to `assets/design-tokens.css`) — focus ring color (`--accent-primary`), pill colors, no edits in Phase 4.

### Phase 1–3 deliverables (consumed by Phase 4)
- `src/state.rs` — current `Block` enum, `CommandLine`, `Token`, `ToolCall`, `ToolStatus`, `Mode`, `PaletteItem`, `Tab`, `Message`, `TokenBudget`, plus `demo_blocks()`, `demo_messages()`, `demo_palette_items()`, `demo_tabs()`. Phase 4 ADDS `Personality` enum, `BlockEntry` wrapper, `PaletteState` enum, `now_time()` helper. Phase 4 RENAMES `demo_blocks` → `demo_block_entries` returning `Vec<BlockEntry>`.
- `src/components/warp_hermes.rs` — currently passes hardcoded values. Phase 4 introduces every signal here, wires submit/keydown handlers, replaces hardcoded `tokens`/`scanner_active`/`active_tab` fields with signal reads, swaps `Hero`-pattern to fully-reactive composition.
- `src/components/shell/*.rs` (10 files) — Phase 3 primitives. Each gets prop signature changes per D-01: mutating props become `Signal<T>` (or `mut Signal<T>` for components that write), read-only-but-reactive props become `ReadOnlySignal<T>`.
- `src/platform/mod.rs` — currently empty (Phase 1 stub). Phase 4 fills with `pub mod timer;` and the new `timer.rs` file. Sets the precedent for Phase 6's mobile-shell platform module.
- `src/app.rs` — `document::Link` chain unchanged. No CSS additions.
- `Cargo.toml` — Phase 4 adds `gloo-timers`, `tokio` (time-only), `web-sys` (with explicit features). First Cargo.toml dependency growth since Phase 1.
- `clippy.toml` — already enforces signal-borrow-across-await rule. Phase 4 is the FIRST phase with async code; expect to hit clippy errors during execution and learn the patterns. Document any false-positive workarounds inline.

### v2 Integration Target (forward reference, NOT a Phase 4 dep)
- `/Users/twilson/code/ironhermes/` — Cargo workspace, edition 2024, tokio + reqwest + rusqlite. Crates: `ironhermes-agent` (LLM client + agent loop + personality.rs + tool_pair.rs), `ironhermes-tools` (terminal, file_tools, web_search, browser_*), `ironhermes-core` (types.rs, error.rs, provider.rs, model_metadata.rs), `ironhermes-state` (SQLite session store). v2 swap-in: introduce `dioxus/fullstack` server functions that call `ironhermes-agent` from `crates/iron_hermes_ui_server` (new). Phase 4 mock signatures (`run_shell`/`run_agent_steps`) deliberately match what those server-fns will return. **DO NOT** import or compile-link from `/Users/twilson/code/ironhermes/` in Phase 4 — it's WASM-incompatible (tokio + rusqlite + reqwest).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src/state.rs` — already has the EXACT type vocabulary that mocks need: `Block` enum with 6 variants, `Token` enum with `Bin/Arg/Flag/Str`, `ToolCall { name, args_summary, status }`, `ToolStatus { Pending|Running|Done|Failed }`, `Mode { Shell, Agent }`, `Message { who, time, body, tool }`, `TokenBudget { used, max }`. Phase 4 adds `Personality`, `BlockEntry`, `PaletteState` — net new types are minimal because Phase 3 anticipated this.
- `src/components/warp_hermes.rs` — currently 56 lines of hardcoded composition. Phase 4 expands to ~120 lines (signals + handlers + use_effect + use_context_provider). All shell primitives are already imported and composed correctly — Phase 4 adds reactivity around the same composition tree.
- `src/components/shell/command_palette.rs`, `input_box.rs`, `block.rs`, `status_bar.rs`, `block_stream.rs` — Phase 3 components that will receive the bulk of prop-signature changes. The CSS class strings, RSX layout, and component decomposition stay; only props change shape.
- `src/platform/mod.rs` — empty stub created in Phase 1. Phase 4 is the first phase to use it (timer.rs). Sets the modular pattern that Phase 6's mobile platform code will follow.
- `assets/warp-ih.css` and `assets/design-tokens.css` — every interaction state is already styled. `:hover`, `:focus`, `.wh-block-actions { opacity }`, `is-active` scanner cell color shifts — all in place. Phase 4 adds no CSS.

### Established Patterns
- Asset constants colocated with consuming primitive (Phase 2/3 D-05). Phase 4 adds none.
- Component functions `PascalCase`, annotated `#[component]`, return `Element`. RSX 4-space-indented inside `rsx! { ... }`. Phase 4 follows.
- Module re-export pattern: `mod.rs` does `pub use submod::Comp;`. Phase 4 follows for `mocks/mod.rs` and `platform/mod.rs`.
- `use crate::state::*;` import pattern from every shell primitive (Phase 3 D-04). Phase 4 extends `state.rs` so existing imports continue working unchanged.
- Three-platform compile gate is the canonical Phase verification (Phase 1 Plan 01-03 pattern). Phase 4 follows.

### Integration Points
- `src/app.rs#App` renders `WarpHermes {}`. No changes — Phase 4's reactivity lives strictly inside `WarpHermes()`.
- `src/main.rs` — slim entry point with `mod app; mod components; mod state; mod platform;` declarations. Phase 4 adds `mod mocks;` (sibling of state/components/platform).
- The new `mocks::run_shell` / `mocks::run_agent_steps` are the only async functions in `WarpHermes`'s callback closures. They're invoked via `spawn(async move { ... })` to detach from the synchronous click/keypress handler.
- `web_sys::window().add_event_listener_with_callback` for global keydown listener — installed in `use_effect`, removed in `use_drop`. The closure must be `Box::leak`'d or stored in a `Signal<Closure<dyn FnMut(_)>>` to outlive the use_effect call. (Standard Dioxus 0.7 pattern; planner reads AGENTS.md for the canonical example.)
- v2 forward-link: `mocks::run_*` functions become server-function call sites; `BlockEntry`/`Message`/`ToolCall` types travel across the server-fn boundary unchanged because they already match `ironhermes-tools` vocabulary.

</code_context>

<specifics>
## Specific Ideas

- **The integration target is `/Users/twilson/code/ironhermes/`.** This is a real, sizable Rust workspace (Tokio + reqwest + rusqlite, edition 2024, 11 crates) that already has a ratatui CLI. iron_hermes_ui is the future GUI face. Phase 4 mocks should NOT couple to it (it's WASM-incompatible) but mock signatures and types should anticipate what server-functions will return so v2 swap-in is impl-only. CONTEXT D-36/D-37 capture this; downstream agents should treat the integration as "v2 design pressure," NOT as a Phase 4 dependency.
- **Three-platform compile gate is non-negotiable.** Phase 1 set it up; Phase 4 (first phase with async) will be tempted to skip it because gloo-timers is wasm-only. The cfg-gated timer pattern (D-04) is the canonical solution; the planner must NOT fall back to `#[cfg(feature = "web")]` gates because that breaks runtime parity for desktop/mobile webview targets.
- **Visual fidelity over demo richness.** PROJECT.md core value: "visually indistinguishable from the React prototype when rendered side by side." This is why D-21 ships ONE reply per personality (matching `fakeAgentReply` 1:1) instead of round-robin variants. Demo richness is a v2 concern when real Hermes is wired.
- **Signal borrows must not span `.await`.** clippy.toml enforces this. Every async function in Phase 4 follows D-06's clone-before-await pattern. Expect clippy to flag mistakes during execution; the fix is always "read into a local, drop the borrow, then await."
- **Phase 3 components get prop-signature churn, not new components.** D-01 changes nearly every shell primitive's prop list. This is acceptable because Phase 4 already touches every interactive component — the churn is contained in one cohesive change rather than spread over multiple phases.

</specifics>

<deferred>
## Deferred Ideas

- **Round-robin or keyword-matched personality replies** — Phase 4 ships 1:1 prototype-fidelity (D-21). v2 (when real Hermes returns), the reply variation comes from the model itself; mocks become irrelevant. Do NOT expand the table in v1.
- **Full markdown rendering for `is-ai`** — Phase 4 ships only inline backtick→`<code>` (D-15). v2 adds `pulldown-cmark` or `comrak` when real Hermes returns markdown-formatted responses. Replacement is one-file isolated (D-15 anticipates this).
- **Share button (`↗`) handler** — D-25 leaves it unwired; v2 needs a real share-link backend. The button's visual stays so Phase 3 visual UAT doesn't regress.
- **Click-to-cycle status-bar personality pill** — Phase 4 renders the pill as read-only feedback (D-22). Phase 5's TweaksPanel adds the click handler.
- **Theme/density/block/agent runtime switching + TweaksPanel** — Phase 5 (THEME-01..THEME-05). Phase 4's `ShellSettings` struct is forward-compatible (D-02) so Phase 5 adds fields without refactoring.
- **Mobile shell variant** — Phase 6 (MOB-01..MOB-04). `src/platform/` directory is now populated by `timer.rs`; Phase 6 adds mobile-specific helpers there.
- **Trait-abstracted data layer (`ShellRunner` / `AgentRunner`)** — D-37 rejects this for Phase 4. v2 may introduce them IF the server-function refactor reveals dispatch needs; Phase 4 ships concrete impls.
- **Persistence (block stream history, settings)** — PERS-01/02 are v2.
- **Tests** — TEST-01/02/03 are v2 (REQUIREMENTS.md "v2 Requirements"). Phase 4 ships interaction code without unit/integration tests; manual UAT against `Warp × IronHermes.html` is the verification.
- **Real shell command execution / real LLM API calls / authentication** — REAL-01..REAL-04 + AUTH-01 are v2. PROJECT.md "No external services" is the active constraint.
- **`Token::Str` (quoted-string args) tokenization** — D-12 leaves it unhandled in Phase 4. Real shells need it; v2 (real `runShell`) will. The variant exists in `Token` already.
- **Live token estimation from prompt size** — D-26 ships +120 fixed. v2 token tracking comes from real API response metadata (REAL-03).
- **Fuzzy palette filtering** — D-32 ships substring case-fold. 10-item palette doesn't warrant a fuzzy crate.

</deferred>

---

*Phase: 04-data-layer-interactions*
*Context gathered: 2026-05-03*
