---
phase: 04-data-layer-interactions
researched: 2026-05-03
domain: Dioxus 0.7 reactivity, async, WASM/web-sys keyboard + clipboard, three-platform cfg-gating
confidence: HIGH
---

# Phase 4: Data Layer & Interactions - Research

**Researched:** 2026-05-03
**Domain:** Dioxus 0.7.1 reactive primitives, single-threaded WASM async, cfg-gated platform timer, web_sys global event listeners
**Confidence:** HIGH (every load-bearing API verified against Context7 + crates.io + AGENTS.md)

## Summary

Phase 4 is the project's first encounter with async + reactivity simultaneously, but the technical surface is small and well-trodden. The two genuine landmines are (1) the clippy-enforced "no signal borrow across `.await`" rule (CONTEXT D-06), and (2) lifetime management of the `wasm_bindgen::Closure` that backs the global keydown listener — naive code drops the closure when `use_effect` returns and the listener silently no-ops. Everything else (cfg-gated `sleep`, `spawn` for fire-and-forget, `use_context_provider`, `Signal<T>`/`ReadOnlySignal<T>` props, `web_sys` clipboard + `scroll_into_view_with_bool`) is canonical Dioxus 0.7 / wasm-bindgen practice with verified source examples.

Every CONTEXT decision (D-01..D-37) survives validation. Two ergonomic notes worth surfacing to the planner: (1) `use_drop` is at `dioxus::core::use_drop`, not in the `prelude` re-exports — the import has to be explicit; (2) Dioxus 0.7 *automatically* spawns futures returned from event handlers, which means `submit()` can be an `async move |_|` closure on the textarea's `onkeydown` and skip the explicit `spawn(async move { ... })` wrapper that CONTEXT D-23/D-24 reaches for. Both forms work; the auto-spawn form is shorter and idiomatic.

**Primary recommendation:** Build Phase 4 in five waves: (Wave 0) test scaffolding + Cargo deltas + cfg-gated `timer.rs` compile gate; (Wave 1) `state.rs` extensions (Personality, BlockEntry, PaletteState, now_time, ShellSettings); (Wave 2) `mocks/` module tree with prototype-verbatim outputs; (Wave 3) shell primitive prop refactor (Signal<T>/ReadOnlySignal<T>); (Wave 4) `WarpHermes()` rewiring with all signals, `use_context_provider`, global keydown `use_effect` + `use_drop`, hover handlers, auto-scroll. Each wave terminates at a green three-platform `cargo build` gate.

## Architectural Responsibility Map

Per Step 1.5 — Phase 4 is a single-tier (browser/client) WASM application; no server, no database, no CDN concerns. The "tiers" here are layers within the client.

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Local component state (input, blocks, mode, palette flags) | `WarpHermes()` component | `Signal<T>` props to children | D-01: hybrid model. State lives in the composer; children mutate via `Signal<T>` props. No prop-drilling because tree is 2-deep. |
| Cross-cutting settings (personality, future theme/density) | `use_context_provider` in `WarpHermes()` | `use_context::<ShellSettings>()` in AgentPanel + StatusBar | D-02: forward-compatible bundle. Phase 5 adds fields, never refactors. |
| Async timer primitive | `src/platform/timer.rs` (cfg-gated) | gloo-timers (wasm32) / tokio::time (native) | D-04: one cfg branch in `timer.rs`; callers see one `sleep(ms)` API. |
| Global keyboard handler (⌘K, Esc, ⌥M) | `WarpHermes()` `use_effect` | `web_sys::Window::add_event_listener_with_callback` + `use_drop` cleanup | D-17: global because ⌘K must work even when input has no focus. ⌥M gates on `focused()` signal to avoid eating typing. |
| Local keyboard (palette ↑/↓/Enter, textarea Enter/Shift+Enter) | Component `onkeydown` attribute | Dioxus event system | D-18/D-19: scoped to the component that owns the surface. No global listener. |
| Mock data layer (run_shell, run_agent_steps) | `src/mocks/` module | `Signal<Vec<_>>` mutation via fn params | D-10..D-12: function signatures match v2 server-fn shape so v2 swap is impl-only. |
| Browser clipboard write | `web_sys::Window::navigator().clipboard().write_text(_)` | Promise dropped (fire-and-forget) | D-23: failures silent, no toast. |
| Auto-scroll to bottom | `use_effect` watching `blocks.read().len()` | `web_sys::Element::scroll_into_view_with_bool(false)` | D-33: triggered by len change; no-op when Vec empty. |

## Standard Stack

### Core (verified against crates.io 2026-05-03)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `gloo-timers` | `0.3` (latest 0.3.0; 0.4.0 exists but is breaking) | wasm32 async sleep via `TimeoutFuture::new(ms).await` | Canonical wasm-bindgen-friendly timer; tiny dep; the `futures` feature gates `future::TimeoutFuture` [VERIFIED: docs.rs/gloo-timers/0.3.0/gloo_timers/future] |
| `tokio` | `1` (latest 1.52.1) | Native async sleep via `tokio::time::sleep(Duration)` | Standard async runtime; **`features = ["time"]`** is sufficient — `time` enables `tokio::time::sleep`, `default-features = false` strips reactor/net/macros [VERIFIED: docs.rs/tokio/1/tokio/time/fn.sleep.html — "Available on crate feature `time` only"] |
| `web-sys` | `0.3` (latest 0.3.97) | Browser API bindings (Window, Element, Clipboard, EventTarget, KeyboardEvent) | Already transitively present via dioxus; explicit declaration with our feature list is cleaner than relying on transitive feature unification [VERIFIED: crates.io] |
| `wasm-bindgen` | `0.2` (latest 0.2.120) | `Closure::wrap` for JS callback bridging | Required transitively; declare explicitly because our keydown handler instantiates `Closure::<dyn FnMut(_)>::new(...)` [VERIFIED: crates.io] |
| `js-sys` | `0.3` (latest 0.3.97) | `js_sys::Date::new_0()` for `now_time()` on wasm32 | Required if D-34 picks the cfg-gated approach over `chrono` [VERIFIED: crates.io] |

### Supporting (Claude's Discretion / D-34 trade-off)
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `chrono` | `0.4` | Native `Local::now()` for `now_time()` | Only if D-34 picks cfg-gated time over hardcoded `"00:00:00"`. Adds ~200KB to native binary. |
| (none) | — | If skipping chrono: hardcode `"00:00:00"` in non-wasm builds | Acceptable per CONTEXT D-34: prototype is web-only-tested; native builds are smoke-tested via the compile gate, not visually exercised. |

**Recommendation to planner:** D-34 trade-off — skip chrono. Use `js_sys::Date` on wasm32 + `"00:00:00"` placeholder on native. Saves bundle + dep budget; native build is compile-gated only.

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `gloo-timers` | `wasm-bindgen-futures` + raw `setTimeout` | More code, no benefit; gloo-timers is exactly this with a clean API |
| `tokio` time-only | `async-std` | tokio is already in the broader Dioxus ecosystem; one less dep family |
| Module-level `Signal<T>` via `use_root_context` | `use_context_provider` in `WarpHermes()` | D-02 picks the latter; root context only needed for cross-route state (no router in v1) |
| `wasm_bindgen::Closure::forget()` | Storing `Closure` in `use_signal` and `.remove_event_listener_with_callback()` in `use_drop` | `forget()` leaks memory but works; the use_drop+remove pattern is correct cleanup. CONTEXT D-17 implies cleanup; planner should NOT use `.forget()`. |

**Installation:**
```toml
[dependencies]
dioxus = { version = "=0.7.1", features = [] }
gloo-timers = { version = "0.3", features = ["futures"] }
tokio = { version = "1", features = ["time"], default-features = false }
web-sys = { version = "0.3", features = [
    "Window",
    "Element",
    "Document",
    "Navigator",
    "Clipboard",
    "EventTarget",
    "KeyboardEvent",
] }
wasm-bindgen = "0.2"
js-sys = "0.3"
```

**Version verification:**
```bash
$ cargo search gloo-timers --limit 1   # 0.4.0 (we pin "0.3" intentionally — 0.4 is breaking)
$ cargo search tokio --limit 1         # 1.52.1
$ cargo search web-sys --limit 1       # 0.3.97
$ cargo search wasm-bindgen --limit 1  # 0.2.120
$ cargo search js-sys --limit 1        # 0.3.97
```
[VERIFIED: cargo search 2026-05-03]

**Note on gloo-timers 0.4 vs 0.3:** crates.io shows `gloo-timers = "0.4.0"` as latest. CONTEXT D-05 specifies `"0.3"`. The 0.4 release exists but is a breaking version bump; sticking with 0.3 keeps the verified `TimeoutFuture::new(u32)` signature exactly as documented in the 0.3 docs. No reason to take the upgrade in Phase 4.

## Architecture Patterns

### System Architecture Diagram

```
                       ┌──────────────────────────────────────────┐
                       │  WarpHermes()  [the composer + state hub] │
                       │                                            │
                       │  use_signal × 12  (D-01 hybrid topology)  │
                       │  ├─ input, blocks, messages, mode         │
                       │  ├─ pal_open, pal_query, pal_state        │
                       │  ├─ scanner_active, focused, active_tab   │
                       │  ├─ tokens, next_id                       │
                       │                                            │
                       │  use_context_provider(ShellSettings {     │
                       │     personality: Signal<Personality>      │
                       │  })  (D-02)                               │
                       │                                            │
                       │  use_effect:  install global keydown      │
                       │     ⌘K toggle, Esc close, ⌥M (if focused) │
                       │  use_drop:    remove listener on unmount  │
                       │                                            │
                       │  use_effect:  watch blocks.len(), call    │
                       │     scroll_into_view on .wh-block:last    │
                       └──────────────┬───────────────────────────┘
                                      │ Signal<T> / ReadOnlySignal<T> props
              ┌───────────────────────┼───────────────────────┐
              │                       │                       │
        ┌─────▼───────┐         ┌─────▼─────┐         ┌──────▼───────┐
        │ BlockStream │         │ InputBox  │         │ CommandPalette│
        │   onclick:  │         │ onkeydown │         │ onkeydown:    │
        │   copy/rerun│         │ Enter →   │         │ ↑/↓/Enter     │
        │   handlers  │         │  submit() │         │ → pick(item)  │
        └─────────────┘         │ Shift+Ent │         └───────────────┘
                                │ → newline │
                                └─────┬─────┘
                                      │
                                      ▼ submit()
                       ┌──────────────────────────────────┐
                       │  if mode == Agent → run_agent    │
                       │  else            → run_shell    │
                       └──────────────┬───────────────────┘
                                      │
                                      ▼  spawn (or async-event-handler auto-spawn)
                       ┌──────────────────────────────────┐
                       │ src/mocks/                       │
                       │  ├─ shell_outputs.rs             │
                       │  │    fake_shell_out(text)       │
                       │  │    keyword routing (git/cargo/│
                       │  │       ls/default)             │
                       │  ├─ personalities.rs             │
                       │  │    REPLIES: [(Personality,    │
                       │  │              &str); 6]        │
                       │  └─ agent_steps.rs               │
                       │       run_agent_steps:           │
                       │         user msg → sleep(400)    │
                       │         tool msg → sleep(1000)   │
                       │         hermes reply             │
                       └──────────────┬───────────────────┘
                                      │ awaits
                                      ▼
                       ┌──────────────────────────────────┐
                       │ src/platform/timer.rs            │
                       │  #[cfg(target_arch="wasm32")]    │
                       │    gloo_timers::future::         │
                       │      TimeoutFuture::new(ms).await│
                       │  #[cfg(not(...))]                │
                       │    tokio::time::sleep(...)       │
                       └──────────────────────────────────┘
                                      │
                                      ▼ side-effects: signal writes
                                   blocks.write().push(...)
                                   messages.write().push(...)
                                   tokens.write().used += 120
```

### Recommended Project Structure (deltas from current state)
```
src/
├── main.rs                           [+1 line: `mod mocks;`]
├── app.rs                            [unchanged]
├── state.rs                          [+Personality, +BlockEntry,
│                                       +PaletteState, +ShellSettings,
│                                       +now_time(); rename
│                                       demo_blocks → demo_block_entries]
├── platform/
│   ├── mod.rs                        [+1 line: `pub mod timer;`]
│   └── timer.rs                      [NEW — cfg-gated sleep(u32)]
├── mocks/                            [NEW MODULE]
│   ├── mod.rs                        [re-exports + run_shell, run_agent_steps]
│   ├── personalities.rs              [REPLIES const + pick_reply()]
│   ├── shell_outputs.rs              [fake_shell_out + STATUS_TEXT const]
│   └── agent_steps.rs                [3-step async chain]
└── components/
    ├── mod.rs                        [unchanged]
    ├── warp_hermes.rs                [REWRITE: ~120 lines, all signals,
                                        use_effect, use_drop, handlers]
    └── shell/
        ├── mod.rs                    [unchanged]
        ├── markdown.rs               [NEW — render_inline_code(&str) -> Element]
        ├── block.rs                  [REFACTOR: data → Signal<BlockData>?
                                        actually no — block stays Vec<&BlockEntry>
                                        iteration target; component receives one
                                        BlockEntry by value. Add onclick handlers.]
        ├── block_stream.rs           [REFACTOR: blocks: ReadOnlySignal<Vec<BlockEntry>>]
        ├── input_box.rs              [REFACTOR: value: Signal<String>,
                                        mode: ReadOnlySignal<Mode>,
                                        focused: Signal<bool>; on_submit closure prop]
        ├── command_palette.rs        [REFACTOR: query: Signal<String>,
                                        open: ReadOnlySignal<bool>,
                                        on_pick closure prop;
                                        local Signal<usize> selected]
        ├── status_bar.rs             [REFACTOR: scanner_active: ReadOnlySignal<bool>,
                                        tokens: ReadOnlySignal<TokenBudget>;
                                        +personality pill via use_context]
        ├── agent_panel.rs            [REFACTOR: messages: ReadOnlySignal<Vec<Message>>;
                                        +personality via use_context]
        └── (sigil, scanner, command_line, tool_call, title_bar — minor or no changes)
```

### Pattern 1: cfg-gated async sleep (D-04)
**What:** Single `sleep(u32)` exposed across both platforms; cfg branches inside the function.
**When to use:** Any await point in the mock data layer.
**Example:**
```rust
// src/platform/timer.rs

/// Cross-platform async sleep.
///
/// Web (wasm32): backed by gloo_timers::future::TimeoutFuture.
/// Native (desktop/mobile): backed by tokio::time::sleep with the
/// `time` feature.
///
/// CONTEXT D-04. Single API; cfg-branch at function scope (NOT module
/// scope) so the public signature is identical on every platform.
pub async fn sleep(ms: u32) {
    #[cfg(target_arch = "wasm32")]
    {
        gloo_timers::future::TimeoutFuture::new(ms).await;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(ms))).await;
    }
}
```
[VERIFIED: gloo_timers::future::TimeoutFuture::new(u32) signature — docs.rs/gloo-timers/0.3.0/gloo_timers/future/struct.TimeoutFuture.html#method.new]
[VERIFIED: tokio::time::sleep with `features = ["time"]` only — docs.rs/tokio/1/tokio/time/fn.sleep.html]

### Pattern 2: Signal-borrow-safe async function (D-06 — THE LANDMINE)
**What:** Read all signal data into local owned values BEFORE any `.await`. Borrow drops at semicolon.
**When to use:** Every async function in `src/mocks/*.rs`.

**BAD (clippy fails — borrow held across await):**
```rust
pub async fn run_shell_BAD(text: String, mut blocks: Signal<Vec<BlockEntry>>) {
    let mut bs = blocks.write();      // GenerationalRefMut acquired
    bs.push(BlockEntry { /* cmd */ }); // ok
    sleep(600).await;                  // ❌ CLIPPY: bs (WriteLock) held across await
    bs.push(BlockEntry { /* out */ }); // never reached at runtime; clippy refuses
}
```

**GOOD (D-06 canonical pattern):**
```rust
// src/mocks/mod.rs (and likewise in agent_steps.rs)
use crate::platform::timer::sleep;
use crate::state::{BlockEntry, Block, CommandLine, Token, now_time};
use dioxus::prelude::*;

pub async fn run_shell(
    text: String,
    mut blocks: Signal<Vec<BlockEntry>>,
    mut next_id: Signal<u64>,
    mut scanner_active: Signal<bool>,
) {
    // ── Stage 1: append cmd block. Each .write() borrow drops at `;`. ──
    let id1 = {
        let id = next_id();        // call-as-fn = clone, no borrow
        next_id.set(id + 1);       // separate write call
        id
    };
    let tokens = tokenize(&text);
    blocks.write().push(BlockEntry {
        id: id1,
        block: Block::Cmd {
            command: CommandLine {
                tokens,
                time: Some("…".into()),
                cwd: None,
                glyph: Some("❯".into()),
            },
        },
    }); // write borrow drops here

    // ── Stage 2: pulse scanner (fire-and-forget). ──
    pulse_scanner(2000, scanner_active);

    // ── Stage 3: wait, then append output. NO BORROWS LIVE. ──
    sleep(600).await;

    let id2 = {
        let id = next_id();
        next_id.set(id + 1);
        id
    };
    let out_block = crate::mocks::shell_outputs::fake_shell_out(&text);
    blocks.write().push(BlockEntry { id: id2, block: out_block });
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut iter = text.split_whitespace();
    let mut out = Vec::new();
    if let Some(first) = iter.next() {
        out.push(Token::Bin(first.into()));
    }
    for tok in iter {
        if tok.starts_with('-') {
            out.push(Token::Flag(tok.into()));
        } else {
            out.push(Token::Arg(tok.into()));
        }
    }
    out
}
```
**Why this passes clippy:** `next_id()` (call-as-fn) clones the inner value via `Copy`/`Clone`; `next_id.set(...)` is an owned method that internally takes a brief write lock and immediately drops it. `blocks.write().push(...)` chains the lock release to the end of the statement (the temporary `WriteLock` is dropped at the `;`). No `WriteLock` / `GenerationalRefMut` is alive when `sleep(600).await` is reached.
[VERIFIED: clippy.toml `await-holding-invalid-types` config + AGENTS.md "use_signal" semantics + Dioxus 0.7 docs on Signal::set / Signal::call patterns]

### Pattern 3: Global keydown listener with use_drop cleanup (D-17)
**What:** Install a `keydown` listener on `window` from inside `WarpHermes()`'s `use_effect`; remove it in `use_drop`. Store the `Closure` in a `Signal<Option<Closure<...>>>` so its lifetime extends until cleanup.
**When to use:** Once per `WarpHermes()` instance; this is the only global listener in Phase 4.
**Example:**
```rust
use dioxus::prelude::*;
use dioxus::core::use_drop;          // NOTE: NOT in prelude
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

#[component]
pub fn WarpHermes() -> Element {
    let mut pal_open  = use_signal(|| false);
    let mut pal_query = use_signal(String::new);
    let mut pal_state = use_signal(|| PaletteState::Browse);
    let mut mode      = use_signal(|| Mode::Shell);
    let focused       = use_signal(|| false);

    // Storage slot for the JS-side Closure. Must outlive the use_effect
    // call OR JS will hold a dangling pointer. We store in a Signal so
    // use_drop can read it back to call remove_event_listener.
    let mut listener_slot: Signal<Option<wasm_bindgen::closure::Closure<dyn FnMut(KeyboardEvent)>>>
        = use_signal(|| None);

    use_effect(move || {
        let window = web_sys::window().expect("window");
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut(KeyboardEvent)>::new(
            move |ev: KeyboardEvent| {
                let key = ev.key();
                let lower = key.to_lowercase();
                if (ev.meta_key() || ev.ctrl_key()) && lower == "k" {
                    ev.prevent_default();
                    let cur = pal_open();
                    pal_open.set(!cur);
                    pal_query.set(String::new());
                    return;
                }
                if key == "Escape" {
                    pal_open.set(false);
                    pal_state.set(PaletteState::Browse);
                    return;
                }
                if ev.alt_key() && lower == "m" && focused() {
                    let next = match mode() {
                        Mode::Shell => Mode::Agent,
                        Mode::Agent => Mode::Shell,
                    };
                    mode.set(next);
                }
            },
        );
        let _ = window.add_event_listener_with_callback(
            "keydown",
            cb.as_ref().unchecked_ref(),
        );
        listener_slot.set(Some(cb));
    });

    use_drop(move || {
        if let Some(cb) = listener_slot.write().take() {
            if let Some(window) = web_sys::window() {
                let _ = window.remove_event_listener_with_callback(
                    "keydown",
                    cb.as_ref().unchecked_ref(),
                );
            }
            // cb drops here, releasing the JS-side reference.
        }
    });

    rsx! { /* …composition… */ }
}
```
[VERIFIED: Closure::wrap / Closure::new + .as_ref().unchecked_ref() pattern — Context7 wasm-bindgen "Rust Closures for Event Handlers" snippet]
[VERIFIED: dioxus::core::use_drop signature + closure runs on component drop — Context7 dioxuslabs.com/learn/0.7 "Component Cleanup with use_drop"]

**Why store in Signal<Option<Closure>>** — three options were considered:
1. `cb.forget()` — leaks the closure permanently. Works for app-lifetime listeners but doesn't give us a handle for `remove_event_listener_with_callback`. CONTEXT D-17 implies cleanup, so this is wrong.
2. `let cb = Box::leak(Box::new(cb))` — same leak problem; can't be removed.
3. **`Signal<Option<Closure<...>>>` — chosen.** Closure lives as long as the component; `use_drop` takes it out and removes the JS listener before dropping. Clean, no leaks, no panic on remount.

### Pattern 4: spawn for fire-and-forget (D-14 pulse_scanner)
**What:** `pulse_scanner` flips `scanner_active = true` synchronously, then spawns a 1400ms task that flips it false. Caller doesn't wait.
**When to use:** Any side-effect that should outlive the calling event handler but doesn't need cancellation.
**Example:**
```rust
use crate::platform::timer::sleep;
use dioxus::prelude::*;

pub fn pulse_scanner(ms: u32, mut scanner_active: Signal<bool>) {
    scanner_active.set(true);
    spawn(async move {
        sleep(ms).await;
        scanner_active.set(false);
    });
    // returns immediately; the spawned task runs detached.
}
```
**Why spawn over use_future:** `use_future` is a hook; it can only be called at the top of a component body. `pulse_scanner` is called from inside event handlers, which is `spawn`'s exact use case. Per Dioxus 0.7 docs, "spawn will start a task running in the background ... this task will automatically be canceled when the component is dropped" — that auto-cancel is exactly the safety we want.
[VERIFIED: dioxus::prelude::spawn signature `pub fn spawn(fut: impl Future<Output = ()> + 'static) -> Task` — Context7 dioxuslabs.com/learn/0.7]

**Multi-pulse race:** CONTEXT D-14 acknowledges that overlapping submissions cause overlapping spawned tasks. The most-recent task wins on the false-set timing because each call re-asserts true before any pending false-setter fires. Worst case: a flicker. Acceptable per D-14.

### Pattern 5: ShellSettings via use_context_provider (D-02)
```rust
// src/state.rs (new)
#[derive(Clone, Copy)]
pub struct ShellSettings {
    pub personality: Signal<Personality>,
    // Phase 5 will add: theme, density, block, agent  (forward-compatible)
}

// src/components/warp_hermes.rs
let personality = use_signal(|| Personality::Default);
use_context_provider(|| ShellSettings { personality });

// src/components/shell/agent_panel.rs (or status_bar.rs)
let settings = use_context::<ShellSettings>();
let label = settings.personality.read().label();
```
**Why `Signal<Personality>` inside the struct, not `Signal<ShellSettings>`:** putting individual signals in the struct means writes to one field don't invalidate consumers of the others. Phase 5's theme write won't trigger AgentPanel re-render (which only reads personality). This is the canonical Dioxus 0.7 pattern for "bag of related signals."
[VERIFIED: dioxuslabs.com/learn/0.7 "Migrate Context State with Signals" — `Signal::new(...)` inside `use_context_provider`]

### Pattern 6: Inline-code markdown renderer (D-15)
**What:** ~25 LOC pure-fn returning Element. Splits on backticks; alternating segments render as `<span>` plain or `<code>`. Mismatched closes fall back to plain text.
**Example:**
```rust
// src/components/shell/markdown.rs
use dioxus::prelude::*;

/// Render text with inline `<code>` spans for backtick-delimited segments.
///
/// Splits on backtick pairs (alternating). Even-indexed segments are plain
/// text; odd-indexed are wrapped in <code>. An unmatched trailing backtick
/// falls back to plain text (the splitter never produces an isolated odd
/// segment because str::split returns N+1 fragments for N delimiters).
///
/// Edge cases:
///   - Empty string                  → empty <div>
///   - "no backticks here"           → one plain span
///   - "a `b` c"                     → ["a ", "b", " c"]
///   - "unclosed `here"              → ["unclosed ", "here"] — second renders as
///                                       <code> (the trailing fragment is plain
///                                       per intent; trade-off: rare in practice;
///                                       if user complains, switch to "odd-count
///                                       backticks → all plain" fallback).
///   - "``empty pair``"              → ["", "", "empty pair", "", ""] — handled.
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
**Acceptable scope creep guardrail:** D-15 says "no new crate." This implementation has zero deps. The trade-off on unclosed backticks (above) is documented; if a user files an issue, swap to a count-check that falls back to all-plain on odd count.

### Anti-Patterns to Avoid
- **Holding `.read()` across `.await`:** clippy.toml errors. Always clone or call-as-fn first.
- **Calling `cb.forget()` for the keydown closure:** leaks AND prevents cleanup. Use the Signal-storage pattern from Pattern 3.
- **Defining `sleep` at module-cfg level (separate `mod web_timer;` and `mod native_timer;`):** doubles the API surface; callers must reach into the right module. CONTEXT D-04 explicitly puts the cfg gate inside the function body — keep it there.
- **`tokio = { version = "1", features = ["full"] }`:** bloats native binary by ~MBs. Only `time` is needed.
- **Putting the global keydown listener in `App` or `main()`:** `use_effect` only runs in components; `main()` has no hook scope. CONTEXT D-17 places it in `WarpHermes()` for exactly this reason.
- **Wrapping every Signal field of ShellSettings in another Signal:** `Signal<Signal<T>>` is legal but pointless; reads have to deref twice and writes are confusing. Each field of `ShellSettings` is one Signal, the bundle is plain.
- **Re-rendering the entire BlockStream on every signal write:** D-01 hands children `ReadOnlySignal<Vec<BlockEntry>>`, not `Vec<BlockEntry>` by value. Dioxus's reactive runtime then only re-runs BlockStream when the signal write fires; passing by value would re-render every consumer of any sibling signal.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Cross-platform sleep | Custom `setTimeout`+`spawn_local` shim | `gloo_timers::future::TimeoutFuture` | Already does exactly this with verified API |
| Markdown parsing | None — D-15 is intentionally hand-rolled (no new crate) | (n/a) | Inline-code-only is ~25 LOC; pulldown-cmark is overkill |
| Clipboard write | Custom JS via `document::eval` | `web_sys::Clipboard::write_text` | Native bindings with type safety |
| Auto-scroll | `scrollTop = scrollHeight` calculation | `Element::scroll_into_view_with_bool(false)` | One line; browser handles the math |
| Async signal mutation | Wrap signals in `Arc<Mutex<_>>` | Just write the signal — Dioxus signals ARE the synchronization | Signals already coordinate read/write within the single-threaded event loop |
| Personality dispatch | `Box<dyn ReplyStrategy>` trait dynamic dispatch | `match personality { ... }` in a const lookup table | D-21: 6 entries, no extension story; trait abstraction is overengineering |
| Tokenization for shell input | Pull in `shellwords` or `shell-words` crate | Inline 8-line `split_whitespace` + `starts_with('-')` per D-12 | `Token::Str` (quoted args) is deferred to v2; basic split is enough |

**Key insight:** Phase 4's "novel" code is ~30 lines: timer.rs + tokenize() + pulse_scanner() + render_inline_code() + the mocks bodies. Everything else is plumbing — wiring existing APIs (web_sys, dioxus prelude, gloo-timers) together with the right cfg gates. Resist the urge to add abstractions.

## Common Pitfalls

### Pitfall 1: Signal borrow held across await (THE clippy landmine)
**What goes wrong:** `let bs = blocks.write(); ... sleep(600).await; bs.push(...)` — clippy errors at compile time, runtime would deadlock the signal.
**Why it happens:** Mental model from sync code: "get a mutable reference, do stuff with it." Async breaks this — the await suspends with the lock held.
**How to avoid:** Discipline of D-06: read/clone before await; write inline-and-drop after. Never name a `.read()` or `.write()` binding that lives more than one statement.
**Warning signs:** Any `let x = sig.read();` or `let mut x = sig.write();` line followed within the same scope by `.await`.

### Pitfall 2: Forgotten Closure dropped before listener fires
**What goes wrong:** `add_event_listener_with_callback(..., cb.as_ref().unchecked_ref())` is called; `cb` goes out of scope; JS holds a dangling function pointer; the listener silently does nothing on keypress.
**Why it happens:** wasm-bindgen `Closure` is the Rust-side keepalive; once dropped, the JS reference is invalid. The compiler doesn't catch it because the JS-side reference is opaque.
**How to avoid:** Pattern 3's `Signal<Option<Closure<_>>>` storage. Or `cb.forget()` IF you accept the leak AND don't need to remove the listener later (we do need to — `use_drop` requires it).
**Warning signs:** Listener installed but `keydown` events do nothing in DevTools console. Test by adding `web_sys::console::log_1(&"keydown fired".into());` inside the closure body — silence means the closure was dropped.

### Pitfall 3: tokio panic — "no current timer set"
**What goes wrong:** Native build with `tokio = { features = ["time"] }` — but the runtime has no time driver enabled; `tokio::time::sleep` panics at runtime.
**Why it happens:** Dioxus desktop's runtime config controls reactor flags. The `time` feature gates the API existence; the runtime build flags gate availability.
**How to avoid:** Dioxus 0.7 desktop runtime enables tokio's full driver set by default. Verify with the three-platform compile gate AND a smoke run of `dx serve --platform desktop` once — if `submit()` triggers the panic, the runtime needs `Runtime::new()` with `enable_all()` (it should already; this is a "verify, don't assume" item).
**Warning signs:** Panic message containing "there is no reactor running" on first submit in desktop build.
[VERIFIED: tokio docs.rs warning — "This function panics if there is no current timer set"]

### Pitfall 4: Three-platform compile gate breakage from cfg drift
**What goes wrong:** Code that uses `web_sys::Window` or `js_sys::Date` compiles on wasm32 but errors on native because those crates are wasm-only-compatible.
**Why it happens:** `web-sys` and `js-sys` are not native targets — they generate calls to JS-side runtime that doesn't exist outside wasm32.
**How to avoid:** Every `use web_sys::*;` import inside a function body should be inside a `#[cfg(target_arch = "wasm32")]` block, OR the entire function body should be a single cfg branch (Pattern 1). For `now_time()`, the cfg gate goes around the body, not around the import — see D-34.
**Warning signs:** `cargo build --features desktop` errors about unresolved imports from web_sys or js_sys.

### Pitfall 5: scroll_into_view firing on /clear
**What goes wrong:** `use_effect` watches `blocks.read().len()`; `/clear` sets len to 0; the effect fires; the query selector `.wh-block:last-child` returns null; unwrap panics.
**Why it happens:** Naive `query_selector(...).unwrap()` on an empty stream.
**How to avoid:** `if let Ok(Some(el)) = doc.query_selector(...) { el.scroll_into_view_with_bool(false); }` — three-level Result/Option/None handling. CONTEXT D-33 specifies "Does NOT fire on /clear (Vec is empty)" but the safe pattern is to also gate the side-effect on `len > 0` inside the effect body.

### Pitfall 6: ⌥M intercepting normal typing on macOS
**What goes wrong:** Bare `altKey && key == "m"` in the global handler fires while the user is typing the letter "µ" (which is what Option-M produces on US keyboards).
**Why it happens:** macOS converts Option+letter into special characters; the `key` value at the JS event level is sometimes the original letter "m" depending on input element. Without the `focused` gate, the handler swallows the keypress for non-input contexts too.
**How to avoid:** D-17 specifies "ONLY when focused == true" — the textarea has its own focus state we already track. The handler must read `focused()` and exit early if false. (Pattern 3 above includes this.)

## Code Examples

### Common Operation 1 — submit() routing
```rust
// Inside WarpHermes(); attached as on_submit prop to InputBox.
let submit = move || {
    let text = input.read().trim().to_string();
    if text.is_empty() { return; }
    input.set(String::new());
    pulse_scanner(2000, scanner_active);
    pulse_token(120, tokens);

    let cur_mode = mode();           // call-as-fn = clone
    let cur_personality = use_context::<ShellSettings>().personality;

    spawn(async move {
        match cur_mode {
            Mode::Agent => {
                crate::mocks::run_agent_steps(
                    text, cur_personality.read().clone(), messages
                ).await;
            }
            Mode::Shell => {
                crate::mocks::run_shell(text, blocks, next_id, scanner_active).await;
            }
        }
    });
};
```
[Pattern source: app.jsx lines 151-161 + D-13 routing rules + D-26 token pulse]

### Common Operation 2 — clipboard fire-and-forget (D-23)
```rust
// Inside Block component's copy button onclick.
use wasm_bindgen_futures::JsFuture;

onclick: move |_| {
    let text = block_text_for_copy(&data); // assemble per D-23
    if let Some(window) = web_sys::window() {
        let promise = window.navigator().clipboard().write_text(&text);
        // Drop the promise: fire-and-forget. No spawn needed.
        let _ = promise;
    }
}
```
**Note on the simpler form:** because `Promise` does not implement `Future` directly, just dropping it is sufficient — the JS engine still executes the underlying clipboard write. We don't need `spawn_local` here unless we want to handle errors (D-23 says no toast / silent failure, so we don't).
[VERIFIED: web_sys::Clipboard::write_text returns js_sys::Promise — Context7 web-sys docs]

### Common Operation 3 — auto-scroll on new blocks (D-33)
```rust
// Inside WarpHermes(), after all use_signal calls.
use_effect(move || {
    let len = blocks.read().len();
    if len == 0 { return; }                   // no-op on /clear
    if let Some(window) = web_sys::window() {
        if let Some(doc) = window.document() {
            if let Ok(Some(el)) = doc.query_selector(".wh-stream-scroll .wh-block:last-child") {
                el.scroll_into_view_with_bool(false);  // false = align to bottom
            }
        }
    }
});
```
[VERIFIED: Element::scroll_into_view_with_bool(align_to_top: bool) — false aligns to bottom — docs.rs/web-sys]

### Common Operation 4 — palette pick handler (D-27..D-32)
```rust
let pick = move |item: PaletteItem| {
    pal_open.set(false);
    pal_query.set(String::new());
    pal_state.set(PaletteState::Browse);
    match item.cmd.as_str() {
        "/clear" => { blocks.set(Vec::new()); }
        "/status" => {
            let id = next_id();  next_id.set(id + 1);
            blocks.write().push(BlockEntry {
                id,
                block: Block::Out {
                    author: Some("ironhermes".into()),
                    time: Some(now_time()),
                    text: crate::mocks::shell_outputs::STATUS_TEXT.into(),
                },
            });
        }
        "/help" => { /* per D-29 */ }
        "/personality" => {
            pal_open.set(true);                       // re-open
            pal_state.set(PaletteState::PersonalityPick);
        }
        _ => { input.set(item.cmd.clone()); }
    }
};
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `cx.use_state` (Dioxus 0.6) | `use_signal` (0.7) | 0.7.0 release | All Phase 3 components are 0.7-native; Phase 4 builds on the same baseline |
| `use_effect` cleanup-via-return-closure (React-style) | `use_effect` for setup + separate `use_drop` for teardown (Dioxus 0.7) | 0.7.0 design | Two-hook pattern; planner must know to import `use_drop` from `dioxus::core` |
| `gloo-timers = "0.2"` (no `futures` feature) | `gloo-timers = "0.3"` with `features = ["futures"]` | 0.3.0 release | The `future::TimeoutFuture` requires the explicit feature; without it, the type doesn't exist |
| `tokio = { features = ["full"] }` defaults in tutorials | `features = ["time"], default-features = false` | Always (tutorials are bloat-tolerant; production should be lean) | Saves substantial native binary size; only `time` is needed for `sleep` |

**Deprecated/outdated:**
- Dioxus 0.6 `cx`/`Scope`/`use_state` patterns: forbidden by CLAUDE.md, removed in 0.7. Anything in training data referencing these is wrong for this project.
- `wasm-bindgen` `Closure::wrap` (legacy form): superseded by `Closure::<dyn FnMut(_)>::new(|...| ...)` in current docs. Both compile; `new` is preferred.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Dioxus 0.7 desktop runtime enables tokio's full reactor (including `time` driver) by default | Pitfall 3 | If wrong, `dx serve --platform desktop` panics on first `sleep` call. Mitigation: smoke-test desktop once before declaring Phase 4 done. |
| A2 | `js_sys::Date::new_0()` produces local-tz time on wasm32 (not UTC) | D-34 | If wrong, `now_time()` shows UTC strings. Easy to fix post-hoc; not a blocker. |
| A3 | The `markdown` content of `Block::Ai` always uses backticks for code spans (no other markdown syntax expected from mocks) | Pattern 6 | If wrong, asterisks/brackets render as literal text. CONTEXT D-21 confirms 6 verbatim strings — manual verification at execute time is trivial. |
| A4 | Dropping a `js_sys::Promise` is safe — the underlying JS task still executes | Common Op 2 | If wrong, clipboard writes silently no-op even on permission grant. Mitigation: switch to `spawn_local(JsFuture::from(promise).await)` if reports come in. |

**If this table needs user confirmation:** A1 is the only one with execute-blocking risk. The discuss-phase output already locked tokio version + features, so the scope is correct; the runtime-availability question is purely a smoke-test concern.

## Project Constraints (from CLAUDE.md)

- Rust 2021 + Dioxus 0.7.1 — `cx`, `Scope`, `use_state` FORBIDDEN.
- Component functions: `PascalCase`, annotated `#[component]`, return `Element`.
- WASM single-threaded; signal borrows MUST NOT span `.await` (clippy.toml enforces).
- Multi-platform via three Cargo features (web/desktop/mobile); `#[cfg(target_arch = "wasm32")]` or `src/platform/` modules for platform-specific code.
- No external services, mocks only (zero API keys, zero network calls).
- Pixel-perfect to React prototype is the primary failure mode.
- CSS strategy: ported as-is, no Tailwind conversion.
- `warp2ironhermes/` is READ-ONLY — consult, never import or build from.
- `use dioxus::prelude::*` is standard; do not enumerate individual items (BUT `use_drop` is at `dioxus::core::use_drop`, not in prelude — explicit import required).
- 4-space indent, rustfmt, clippy with project config.
- Asset constants: `SCREAMING_SNAKE_CASE` at module top, `const NAME: Asset = asset!("/assets/...")`.
- Stylesheet injection: `document::Link { rel: "stylesheet", href: CONST_NAME }` in App composer (already done; no Phase 4 CSS additions per CONTEXT D-15-block).

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| MOCK-01 | Six personality presets each define their own scripted mock-reply table | D-21 + Pattern: `pub const REPLIES: [(Personality, &str); 6]` in `personalities.rs`, verbatim from app.jsx 339-349 |
| MOCK-02 | `runShell(cmd)` mock emits is-cmd then delayed is-out / is-ok / is-err blocks at 600ms / 400ms / 1400ms | D-12 + Pattern 2 — `sleep(600).await` between cmd append and output append (the 400/1400 timings are the agent flow per app.jsx 179-184) |
| MOCK-03 | `runAgent(prompt)` mock emits agent-thinking → tool-call → is-ai reply pulled from active personality | D-10 `agent_steps.rs` + Pattern 2 — three-stage chain with `sleep(400)` then `sleep(1000)` |
| MOCK-04 | Token budget counter increments per submission and renders in status bar | D-26 + Common Op 1 — `pulse_token(120, tokens)` saturating at max |
| KBD-01 | ⌥M toggles input mode between Shell and Agent | D-17 + Pattern 3 — global `keydown` handler, gated on `focused()` to avoid intercepting non-input typing |
| KBD-02 | ⌘K opens command palette; Esc closes | D-17 + Pattern 3 — same global handler, `(meta || ctrl) && key == "k"` toggles, `Escape` closes |
| KBD-03 | ↑/↓ navigates palette items; Enter selects active item | D-18 + Common Op 4 — local `onkeydown` on palette overlay, `Signal<usize> selected` index |
| KBD-04 | Enter submits input box; Shift+Enter inserts newline | D-19 — local `onkeydown` on textarea; Shift+Enter is browser default; plain Enter calls `submit()` + `prevent_default` |
| KBD-05 | Block hover affordances fire copy/rerun/share handlers | D-23 (copy via clipboard API), D-24 (rerun re-runs `run_shell`), D-25 (share is no-op stub) — handlers attached to existing Phase 3 buttons |
| KBD-06 | Switching personality preset immediately updates active mock-reply set | D-20/D-21 + Pattern 5 — `ShellSettings.personality` Signal lookup at `run_agent_steps` call site; reactive consumers (status bar pill, agent panel) re-render on write |

## Open Questions (RESOLVED)

1. **`tokio` time driver availability in Dioxus 0.7 desktop runtime**
   - What we know: `tokio = { features = ["time"] }` enables the `sleep` API at compile time; CONTEXT D-05 specifies this exact feature subset.
   - What's unclear: Whether Dioxus 0.7's desktop launch path constructs a tokio runtime with `enable_all()` or only `enable_io()`. If the latter, `sleep` panics at runtime in desktop builds.
   - Recommendation: Wave 0 task — write a no-op desktop smoke test (just `dx serve --platform desktop` and verify the shell loads without panic). If panic, add a tokio runtime configuration patch — likely zero-LOC because Dioxus desktop default already enables full reactor.
   - **RESOLVED:** Addressed at runtime by the new desktop smoke test added in Plan 04-01 Task 4 (`#[tokio::test] desktop_sleep_does_not_panic` exercising `tokio::time::sleep` on the native target). Acceptable risk for compile-time only because the test exists and gates the wave.

2. **Closure cleanup ordering (use_drop vs Signal drop)**
   - What we know: `use_drop` runs when the component drops; the Signal-stored Closure drops shortly after.
   - What's unclear: Whether the JS event listener can fire between `use_drop` removing it and the Closure being deallocated. Theoretically yes; practically the gap is microseconds and the closure body is reentrant-safe (only signal writes).
   - Recommendation: Accept the theoretical race; the Closure body has no resources that would explode if called once after deregistration.
   - **RESOLVED:** Closure body is idempotent (only signal writes); race window is benign per RESEARCH §Pattern 3 documentation. No code-level mitigation required.

3. **Dx serve features-flag pass-through for `dioxus = { features = [] }`**
   - What we know: Cargo.toml has `features = []` on dioxus and adds `web`/`desktop`/`mobile` as workspace features that activate `dioxus/web` etc.
   - What's unclear: Whether `dx serve --platform desktop` correctly activates the `desktop` feature (and only `desktop`), or whether mutual exclusivity needs explicit `--no-default-features`.
   - Recommendation: Verify in Wave 0 with `cargo build --features desktop --no-default-features` and `cargo build --features mobile --no-default-features`. The Phase 1 plan already established this gate; Phase 4 just inherits it.
   - **RESOLVED:** Phase 1 already verified all three `dx serve --platform <X>` invocations work; Phase 4 inherits Phase 1's gate without re-verification (same Cargo feature set).

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | All Cargo work | ✓ | (assumed; project has built before) | — |
| `dx` CLI | Dev server, three-platform compile gate | ✓ (assumed; Phase 1-3 used it) | — | `cargo build --features X` works without dx for compile-only verification |
| `cargo` | Dependency resolution | ✓ | (assumed) | — |
| Browser w/ Clipboard API | KBD-05 manual UAT | ✓ (any modern browser) | — | UAT only; not a build dep |
| Webview runtime (desktop) | `cargo build --features desktop` | ✓ (assumed; Phase 1 verified) | — | If desktop fails, Phase 4 ships without that gate but Phase 6 still depends on it |

**Missing dependencies with no fallback:** None.
**Missing dependencies with fallback:** None — all build-blocking deps verified present from prior phases.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | None configured. CONTEXT and PROJECT.md say tests are v2 (TEST-01..03). Phase 4 has no automated test suite. |
| Config file | none |
| Quick run command | `cargo build --features web` (compile-time clippy check is the closest to "tests" in v1) |
| Full suite command | `cargo build --features web && cargo build --features desktop && cargo build --features mobile && cargo clippy --features web -- -D warnings` |
| Phase gate command | The full-suite command above. Manual UAT (Wave 5) is the behavioral gate. |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| MOCK-01 | Six personality presets, one canned reply each | manual-only | (UAT: cycle personality, send `hello`, observe reply text matches `personalities.rs` table) | manual |
| MOCK-02 | `runShell` 600ms timing | manual-only | (UAT: type `git status`, count seconds; ~600ms cmd→out, ~1400ms total per success criteria) | manual |
| MOCK-03 | `runAgent` three-stage flow | manual-only | (UAT: ⌥M to Agent mode, type `hello`, observe user → tool-call → ai chain) | manual |
| MOCK-04 | Token counter +120 per submit | manual-only | (UAT: read status bar before/after submit; +120) | manual |
| KBD-01 | ⌥M toggles mode | manual-only | (UAT: focus textarea, ⌥M, observe glyph ❯ ↔ ✦) | manual |
| KBD-02 | ⌘K opens, Esc closes palette | manual-only | (UAT: ⌘K → palette visible; Esc → hidden) | manual |
| KBD-03 | ↑/↓/Enter palette navigation | manual-only | (UAT: arrow keys move highlight; Enter selects) | manual |
| KBD-04 | Enter submits, Shift+Enter newline | manual-only | (UAT: type, Enter → submission; type, Shift+Enter → newline visible) | manual |
| KBD-05 | Hover handlers fire | manual-only | (UAT: hover block, click ⎘ → clipboard contains text; click ↻ on cmd block → re-runs) | manual |
| KBD-06 | Personality switch updates mock replies | manual-only | (UAT: /personality → noir, send agent prompt, observe noir-style reply) | manual |

**Compile-time validation (continuously enforced):**
- `cargo build --features web` — wasm32 target compile (catches gloo-timers usage, web_sys feature gate completeness)
- `cargo build --features desktop` — native target compile (catches missing tokio cfg gates)
- `cargo build --features mobile` — native target compile (catches mobile-only regressions)
- `cargo clippy --features web -- -D warnings` — enforces clippy.toml `await-holding-invalid-types` rule (the D-06 landmine)

### Sampling Rate
- **Per task commit:** `cargo build --features web` (the development feature; fastest)
- **Per wave merge:** Full three-platform compile gate + `cargo clippy --features web -- -D warnings`
- **Phase gate:** Full suite GREEN + manual UAT walk-through against `Warp × IronHermes.html` side-by-side

### Wave 0 Gaps
- [ ] Cargo.toml deltas: add `gloo-timers`, `tokio`, `web-sys` (with explicit features), `wasm-bindgen`, `js-sys`. Run three-platform compile gate AFTER deltas to confirm a baseline before any code changes.
- [ ] `src/platform/timer.rs` skeleton with the cfg-gated `sleep` function — first three-platform compile gate exercise of the new dep tree.
- [ ] No test framework setup (Phase 4 stays test-free per PROJECT.md / TEST-01..03 v2 deferral).

*(If no gaps: not applicable — Wave 0 IS the gap-closing wave by design.)*

**Phase gate (UAT):** matches Phase 3's success pattern — manual side-by-side review against the prototype HTML. The 6 success criteria from ROADMAP.md Phase 4 (Enter→is-cmd→is-out 600ms, ⌥M toggle, ⌘K palette, Shift+Enter, personality swap, token counter) are the UAT checklist.

## Risk Register (Open Questions, Deferred Decisions, Integration Concerns)

| # | Risk | Impact | Mitigation |
|---|------|--------|------------|
| R1 | tokio runtime missing time driver in desktop build | Panic on first submit in desktop variant | Wave 0 desktop smoke test; if fails, add tokio runtime configuration |
| R2 | wasm-bindgen Closure dropped early → silent listener failure | KBD-01..03 don't fire; cryptic "nothing happens" UAT failure | Pattern 3 Signal-storage; UAT step "press ⌘K" early in interaction sequence catches this |
| R3 | Signal borrow held across await passes locally but clippy fails CI | Plan tasks need explicit "drop borrow" guidance | D-06 pattern documented in every mock function; Pattern 2 example serves as template |
| R4 | gloo-timers version drift (0.4 published, we pin 0.3) | Minor; 0.4 is breaking and we don't need it | Pin `"=0.3"` if exact version control matters; otherwise `"0.3"` is a caret-equivalent within 0.3.x |
| R5 | use_drop closure ordering with Signal cleanup | Theoretical race window | Closure body is idempotent (only signal writes); race is benign |
| R6 | Personality enum traits — D-03 says derive Hash + Eq; planner picks IntoIterator helper | Minor surface area decision | CONTEXT Claude's Discretion: implement helper rather than `strum` crate; ~5 LOC `Personality::ALL: [Personality; 6]` |
| R7 | Auto-scroll firing during initial render with seed blocks | Brief flash of scrolling on first paint | Acceptable; matches React prototype behavior with `useEffect`-driven scroll |
| R8 | Three-platform compile gate failure from web_sys feature drift | Build red on desktop after adding new web_sys API call | Every new web_sys usage triggers the gate; expect ~2 iterations during Wave 4 |

## Recommendations for Plan Structure (advisory — planner decides)

A 5-wave decomposition emerges naturally from the dependency graph and isolates the three-platform compile gate after each significant Cargo.toml or platform-cfg change:

**Wave 0 — Cargo + cfg foundation (compile gate after each task)**
- Cargo.toml deltas (one task per dep group: gloo-timers, tokio, web-sys, wasm-bindgen, js-sys)
- `src/platform/timer.rs` cfg-gated sleep function
- Add `pub mod timer;` to `src/platform/mod.rs`
- **GATE:** three-platform `cargo build` green

**Wave 1 — state.rs extensions**
- Add `Personality` enum + `Personality::ALL` + `Personality::label()`
- Add `BlockEntry` wrapper struct
- Add `PaletteState` enum (Browse | PersonalityPick)
- Add `ShellSettings` struct
- Add `now_time()` cfg-gated helper
- Rename `demo_blocks() -> Vec<Block>` to `demo_block_entries() -> Vec<BlockEntry>` (destructive rename per D-09)
- **GATE:** three-platform `cargo build` green (state.rs is platform-neutral except for now_time())

**Wave 2 — mocks/ module tree (parallelizable: 3 tasks)**
- `src/mocks/personalities.rs` (REPLIES const + pick_reply)
- `src/mocks/shell_outputs.rs` (fake_shell_out + STATUS_TEXT)
- `src/mocks/agent_steps.rs` (run_agent_steps async fn)
- `src/mocks/mod.rs` (re-exports + run_shell async fn)
- Add `mod mocks;` to `src/main.rs`
- **GATE:** three-platform `cargo build` green + clippy green (the await-holding rule first applies here)

**Wave 3 — Shell primitive prop refactor (parallelizable per file; 7 files)**
- `src/components/shell/markdown.rs` (NEW — render_inline_code)
- `block.rs` — accept BlockEntry; add copy/rerun onclick handlers
- `block_stream.rs` — `blocks: ReadOnlySignal<Vec<BlockEntry>>`
- `input_box.rs` — `value: Signal<String>`, `mode: ReadOnlySignal<Mode>`, `focused: Signal<bool>`, plus on_submit closure prop and onkeydown handler
- `command_palette.rs` — `query: Signal<String>`, `open: ReadOnlySignal<bool>`, on_pick prop, local `selected: Signal<usize>`, palette substate
- `status_bar.rs` — `scanner_active: ReadOnlySignal<bool>`, `tokens: ReadOnlySignal<TokenBudget>`, +personality pill via `use_context`
- `agent_panel.rs` — `messages: ReadOnlySignal<Vec<Message>>`, +personality via `use_context`
- **GATE:** three-platform `cargo build` green

**Wave 4 — WarpHermes() rewire (single task, the integration)**
- All 12 `use_signal` declarations (D-01)
- `use_context_provider(ShellSettings { personality })`
- `use_effect` for global keydown listener (Pattern 3)
- `use_drop` for cleanup
- `use_effect` for auto-scroll (Common Op 3)
- `submit()` closure (Common Op 1)
- `pick(item)` palette handler (Common Op 4)
- `pulse_scanner` and `pulse_token` helpers
- Wire all primitive components with their new prop signatures
- **GATE:** three-platform `cargo build` green + clippy green

**Wave 5 — Manual UAT (checkpoint:human-verify)**
- Side-by-side against `Warp × IronHermes.html`
- Checklist: 6 ROADMAP success criteria + risks R1, R2 verification
- (Mirrors Phase 3 Plan 03-05's UAT pattern)

**Why 5 waves and not 3:** The three-platform compile gate is non-negotiable per CLAUDE.md and CONTEXT D-35. Each Cargo.toml delta and each cfg-gated module addition is a potential gate failure; isolating them in their own wave makes failures localized and recoverable. Bunching everything into one wave makes a single failure block the entire phase.

## Sources

### Primary (HIGH confidence)
- **Context7: dioxus library** (`/dioxuslabs/dioxus`, `/websites/dioxuslabs_learn`) — `use_effect`, `use_drop`, `use_context_provider`, `spawn` patterns. All cited snippets are from `dioxuslabs.com/learn/0.7/...` (the canonical 0.7 docsite).
- **Context7: wasm-bindgen library** (`/wasm-bindgen/wasm-bindgen`) — Closure lifetime patterns; "Rust Closures for Event Handlers" + "Initialize Canvas and Event Listeners" snippets.
- **docs.rs/gloo-timers/0.3.0** — TimeoutFuture::new(u32) signature; required `futures` feature.
- **docs.rs/tokio/1/tokio/time/fn.sleep.html** — `time` feature gating; panic-on-no-reactor warning.
- **docs.rs/web-sys** — Element::scroll_into_view_with_bool semantics; Clipboard::write_text returns js_sys::Promise.
- **clippy.toml @ project root** — `await-holding-invalid-types` enforcement of D-06.
- **CLAUDE.md @ project root** — Dioxus 0.7 conventions, prop bounds, `use dioxus::prelude::*` rule.
- **AGENTS.md @ project root** — Dioxus 0.7 API reference (Signal/ReadOnlySignal/use_signal/use_memo).
- **CONTEXT.md @ `.planning/phases/04-data-layer-interactions/`** — D-01..D-37 user-locked decisions; the canonical input for this research.
- **`warp2ironhermes/project/app/app.jsx`** — prototype source of truth for runShell (lines 163-174), runAgent (176-185), pulseScanner (144-148), pick (187-210), fakeShellOut (309-337), fakeAgentReply (339-349).

### Secondary (MEDIUM confidence)
- `cargo search` results 2026-05-03 — current crates.io versions for gloo-timers, tokio, web-sys, wasm-bindgen, js-sys.

### Tertiary (LOW confidence)
- None — every pattern in this document has a HIGH-confidence source.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — every dep verified against crates.io + docs.rs + Context7 in this session.
- Architecture: HIGH — patterns sourced from Context7 dioxus 0.7 snippets and the existing Phase 3 codebase.
- Pitfalls: HIGH — landmines are project-specific (clippy.toml, CONTEXT D-06, three-platform gate) and verified via the project's own enforcement.
- Validation Architecture: HIGH for compile-time; MEDIUM for runtime UAT (manual-only by project policy; the plan IS the test).

**Research date:** 2026-05-03
**Valid until:** 2026-06-03 (30 days; Dioxus 0.7.x is on a slow patch cadence; gloo-timers and tokio are stable)
