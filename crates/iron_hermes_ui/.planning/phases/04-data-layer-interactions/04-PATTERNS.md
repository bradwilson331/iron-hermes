# Phase 4: Data Layer & Interactions - Pattern Map

**Mapped:** 2026-05-03
**Files analyzed:** 14 (6 NEW, 8 MODIFIED)
**Analogs found:** 8 / 14 in-codebase; 6 first-of-kind (covered by RESEARCH.md patterns)

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| **NEW** `src/platform/timer.rs` | utility (cfg-gated platform primitive) | request-response (async) | `src/platform/mod.rs` (empty stub — sets module path only) | NO ANALOG (first cfg-gated impl) |
| **NEW** `src/mocks/mod.rs` | module-root + service entry points | event-driven (async signal mutation) | `src/components/shell/mod.rs` (re-export pattern) | partial (re-exports only; async fns first-of-kind) |
| **NEW** `src/mocks/personalities.rs` | data table (const lookup) | request-response (pure fn) | `src/state.rs` `demo_palette_items()` lines 320-385 | role-match (const-ish data) |
| **NEW** `src/mocks/shell_outputs.rs` | service (keyword-routed factory) | request-response (pure fn) | `src/state.rs` `demo_blocks()` lines 171-270 | role-match (Block constructor pattern) |
| **NEW** `src/mocks/agent_steps.rs` | service (async chain) | event-driven (3-stage pub-sub via Signal writes) | NONE (first async fn in project) | NO ANALOG — RESEARCH Pattern 2 is the template |
| **NEW** `src/components/shell/markdown.rs` | utility (pure-fn render helper) | transform (str → Element) | `src/components/shell/scanner.rs` lines 13-25 (small `#[component]` returning Element) — but NOT a `#[component]`; closest non-component Element-returning fn is NONE in project | partial (similar size, but pattern is non-component pure fn) |
| **MOD** `src/state.rs` | model (type vocabulary + helpers) | n/a (data shapes) | self — extends existing patterns in same file | exact (extends own conventions) |
| **MOD** `src/platform/mod.rs` | module-root | n/a | `src/components/shell/mod.rs` lines 1-11 (`pub mod X;` declarations) | exact |
| **MOD** `src/main.rs` | entry-point | n/a (mod declarations) | self lines 1-5 (`mod X;` chain) | exact |
| **MOD** `src/components/warp_hermes.rs` | composer (state hub) | request-response + event-driven | NONE (first stateful component) | NO ANALOG — RESEARCH Patterns 1, 3, 4 + Common Op 1, 3, 4 |
| **MOD** `src/components/shell/block.rs` | component (variant dispatcher) | event-driven (onclick handlers) | self lines 25-86 (existing prop shape) | exact (signature shape stays; data type changes Block → BlockEntry; +handler closures) |
| **MOD** `src/components/shell/block_stream.rs` | component (iterator chrome) | request-response (read-only signal) | self lines 17-27 | exact (Vec<Block> → ReadOnlySignal<Vec<BlockEntry>>) |
| **MOD** `src/components/shell/input_box.rs` | component (form chrome) | event-driven (oninput/onkeydown/onfocus) | self lines 22-60 | role-match (props refactor + event handlers added) |
| **MOD** `src/components/shell/command_palette.rs` | component (overlay + keyboard) | event-driven (onkeydown ↑/↓/Enter) | self lines 19-72 | role-match (props refactor + nav state + on_pick) |
| **MOD** `src/components/shell/status_bar.rs` | component (read-only display) | request-response (ReadOnlySignal reads) | self lines 19-44 | exact (props become read-only signals; +personality pill via use_context) |
| **MOD** `src/components/shell/agent_panel.rs` | component (read-only display) | request-response (ReadOnlySignal reads) | self lines 19-55 | exact (messages prop → ReadOnlySignal; personality from use_context) |
| **MOD** `Cargo.toml` | config | n/a | self lines 9-10 (`[dependencies]` table) | exact |

---

## Pattern Assignments

### NEW `src/platform/timer.rs` (utility, cfg-gated async)

**Analog:** NONE in this codebase. RESEARCH.md Pattern 1 (lines 215-241) is the canonical spec.

**Module declaration pattern** (from `src/components/shell/mod.rs` line 1):
```rust
pub mod title_bar;
```
Apply same shape: `src/platform/mod.rs` becomes `pub mod timer;` (single line, replaces the 3-line placeholder).

**Public-fn-with-cfg-gated-body pattern** (from RESEARCH.md Pattern 1):
```rust
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

**Doc-comment style** (from `src/state.rs` lines 1-12 module doc + `src/components/shell/scanner.rs` lines 1-12 fn doc):
- Module-level `//!` summary referencing CONTEXT decision (`CONTEXT D-04`).
- Function-level `///` describing platform branches and citing both crates.

**Specific risks:**
- Forgetting `default-features = false` on tokio → bloats native binary by MBs (RESEARCH Pitfall: "tokio = full" anti-pattern).
- `tokio::time::sleep` may panic if Dioxus desktop runtime lacks `time` driver (RESEARCH R1, A1). Wave 0 desktop smoke test mitigates.
- `u32` → `Duration::from_millis(u64::from(ms))` cast required; raw `ms.into()` works but explicit u64 is clearer.

**Wave assignment:** **Wave 0** (foundation; gates the new dep tree).

---

### NEW `src/mocks/mod.rs` (module re-exports + async entry points)

**Analog:** `src/components/shell/mod.rs` lines 1-41 for the re-export shape.

**Re-export pattern** (lines 1-11 + 20-41):
```rust
pub mod title_bar;
pub mod sigil;
// ...
#[allow(unused_imports)]
pub use title_bar::TitleBar;
```
Apply for `src/mocks/mod.rs`:
```rust
pub mod personalities;
pub mod shell_outputs;
pub mod agent_steps;

pub use agent_steps::run_agent_steps;
// run_shell defined inline in this file (per D-10 split).
```

**Async-fn signature** (from RESEARCH.md Pattern 2 lines 265-303):
```rust
pub async fn run_shell(
    text: String,
    mut blocks: Signal<Vec<BlockEntry>>,
    mut next_id: Signal<u64>,
    mut scanner_active: Signal<bool>,
) {
    // body per Pattern 2 — owned-clone-before-await discipline.
}
```

**Specific risks:**
- Forgetting `mut` on `Signal<T>` params used for `.write()` / `.set()` → compile error.
- Importing `dioxus::prelude::*` in non-component file is fine and idiomatic per CLAUDE.md ("`use dioxus::prelude::*` is standard").
- Borrow-across-await landmine (D-06 / clippy.toml): every `.write()` MUST drop at semicolon before any `.await`.

**Wave assignment:** **Wave 2** (after timer + state extensions).

---

### NEW `src/mocks/personalities.rs` (const lookup table)

**Analog:** `src/state.rs` `demo_palette_items()` lines 320-385 — closest existing const-ish data factory.

**Data-vec construction pattern** (state.rs lines 320-385):
```rust
pub fn demo_palette_items() -> Vec<PaletteItem> {
    vec![
        PaletteItem { section: "slash".into(), cmd: "/help".into(), ... },
        // ... 9 more
    ]
}
```

For Phase 4, prefer a true `const` (no allocations) since Personality is `Copy`:
```rust
use crate::state::Personality;

pub const REPLIES: [(Personality, &str); 6] = [
    (Personality::Concise,   "ack."),
    (Personality::Technical, "Looking at the call site, ..."),
    (Personality::Noir,      "The terminal hums..."),
    (Personality::Hype,      "OH MY GOSH ..."),
    (Personality::Catgirl,   "nya~ ..."),
    (Personality::Default,   "Sure — happy to help."),
];

pub fn pick_reply(p: Personality) -> &'static str {
    REPLIES.iter()
        .find(|(k, _)| *k == p)
        .map(|(_, s)| *s)
        .unwrap_or("…")
}
```

**Source-of-truth excerpts:** verbatim from `warp2ironhermes/project/app/app.jsx` lines 339-349 (`fakeAgentReply` table). DO NOT paraphrase — D-21 mandates 1:1 prototype fidelity.

**Specific risks:**
- Forgetting `Eq` on Personality enum → `find` predicate fails to compile. D-03 specifies `Eq + Hash` derives.
- Adding more entries beyond 6 violates D-21 (deferred ideas).

**Wave assignment:** **Wave 2**.

---

### NEW `src/mocks/shell_outputs.rs` (keyword-routed Block factory)

**Analog:** `src/state.rs` `demo_blocks()` lines 171-270 — same Block-constructor idiom.

**Block construction pattern** (state.rs lines 215-223 for Ok variant):
```rust
Block::Ok {
    author: Some("git".into()),
    time: Some("00:14:31".into()),
    message: "src/agent/personality.rs | 36 ++++...".into(),
}
```

For Phase 4, mirror with keyword routing:
```rust
pub fn fake_shell_out(text: &str, time: &str) -> Block {
    let trimmed = text.trim_start();
    if trimmed.starts_with("git status") {
        Block::Ok {
            author: Some("git".into()),
            time: Some(time.into()),
            message: GIT_STATUS_TEXT.into(),
        }
    } else if trimmed.starts_with("cargo") {
        Block::Ok {
            author: Some("cargo".into()),
            time: Some(time.into()),
            message: CARGO_BUILD_TEXT.into(),
        }
    } else if trimmed.starts_with("ls") {
        Block::Out {
            author: Some("ls".into()),
            time: Some(time.into()),
            text: LS_OUTPUT.into(),
        }
    } else {
        Block::Ok {
            author: Some("sh".into()),
            time: Some(time.into()),
            message: format!("(simulated) ran: {text}"),
        }
    }
}

pub const STATUS_TEXT: &str = "..."; // verbatim from app.jsx 25-36
```

**Source-of-truth excerpts:** `warp2ironhermes/project/app/app.jsx` lines 25-36 (STATUS_TEXT) and 309-337 (`fakeShellOut`). READ-ONLY reference.

**Specific risks:**
- `text` may contain leading whitespace from textarea; use `trim_start()` before `.starts_with()`.
- `STATUS_TEXT` must include literal newlines; `&str` const handles this without escape ceremony.
- Author strings must match prototype exactly for visual fidelity (e.g., `"sh"` not `"bash"`).

**Wave assignment:** **Wave 2**.

---

### NEW `src/mocks/agent_steps.rs` (async 3-stage chain)

**Analog:** NONE in this codebase. RESEARCH.md Pattern 2 (lines 244-322) is the canonical template.

**Borrow-then-await pattern** (RESEARCH.md Pattern 2 GOOD example, condensed):
```rust
use crate::platform::timer::sleep;
use crate::state::{Message, Personality, ToolCall, ToolStatus, now_time};
use crate::mocks::personalities::pick_reply;
use dioxus::prelude::*;

pub async fn run_agent_steps(
    prompt: String,
    personality: Personality,
    mut messages: Signal<Vec<Message>>,
) {
    // Stage 1: append user message — write borrow drops at `;`.
    messages.write().push(Message {
        who: "user".into(),
        time: now_time(),
        body: prompt.clone(),
        tool: None,
    });

    sleep(400).await; // NO BORROWS LIVE.

    // Stage 2: append tool-call.
    let summary: String = prompt.chars().take(40).collect();
    messages.write().push(Message {
        who: "hermes".into(),
        time: now_time(),
        body: String::new(),
        tool: Some(ToolCall {
            name: "search".into(),
            args_summary: format!("{{\"q\":\"{summary}\"}}"),
            status: ToolStatus::Done,
        }),
    });

    sleep(1000).await; // NO BORROWS LIVE.

    // Stage 3: append final reply.
    let reply = pick_reply(personality).to_string();
    messages.write().push(Message {
        who: "hermes".into(),
        time: now_time(),
        body: reply,
        tool: None,
    });
}
```

**Specific risks (THE LANDMINE):**
- ANY `let bs = messages.write();` followed by `.await` in same scope = clippy fail (clippy.toml `await-holding-invalid-types` fires on `dioxus_signals::WriteLock`).
- ANY `let m = messages.read();` followed by `.await` in same scope = clippy fail (`generational_box::GenerationalRef`).
- The `prompt.chars().take(40).collect()` MUST happen before await OR be inlined; binding it to a local is safe (it's an owned `String`, no signal borrow).
- `now_time()` is cfg-gated (D-34); calling it after await on wasm32 means each stage gets a real timestamp — desirable.

**Wave assignment:** **Wave 2**.

---

### NEW `src/components/shell/markdown.rs` (pure-fn Element renderer)

**Analog:** `src/components/shell/scanner.rs` lines 13-25 — closest small Element-returning function. BUT scanner is `#[component]`; markdown.rs is intentionally a plain `pub fn`.

**Iteration-in-RSX pattern** (scanner.rs lines 14-25):
```rust
#[component]
pub fn Scanner(active: bool) -> Element {
    rsx! {
        span {
            class: "wh-scanner",
            class: if active { "is-active" },
            for i in 0..10 {
                span { key: "{i}", "░" }
            }
        }
    }
}
```

For markdown.rs (RESEARCH.md Pattern 6 lines 472-485):
```rust
use dioxus::prelude::*;

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

**Specific risks:**
- Plain `pub fn` (NOT `#[component]`) — calling it from inside `block.rs` is `{render_inline_code(&markdown)}` (braces required to escape into Rust expr inside RSX).
- The `key` attribute is required on iterated children even outside `#[component]`.
- Edge case: empty string → `split('`)` returns `[""]`, even-indexed → renders one empty span → blank `<div>`. Acceptable.

**Wave assignment:** **Wave 3**.

---

### MOD `src/state.rs` (extend type vocabulary)

**Analog:** self — extends existing patterns in this file.

**Enum-derive pattern** (state.rs lines 125-130):
```rust
#[derive(Clone, PartialEq, Debug)]
pub enum Mode {
    Shell,
    #[allow(dead_code)]
    Agent,
}
```

For Personality (D-03 requires `Clone + Copy + PartialEq + Debug + Eq + Hash + Default`):
```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Personality {
    Concise,
    Technical,
    Noir,
    Hype,
    Catgirl,
    #[default]
    Default,
}

impl Personality {
    pub fn label(&self) -> &'static str {
        match self {
            Personality::Concise => "concise",
            Personality::Technical => "technical",
            Personality::Noir => "noir",
            Personality::Hype => "hype",
            Personality::Catgirl => "catgirl",
            Personality::Default => "default",
        }
    }

    pub const ALL: [Personality; 6] = [
        Personality::Concise, Personality::Technical, Personality::Noir,
        Personality::Hype, Personality::Catgirl, Personality::Default,
    ];
}
```

**Wrapper-struct pattern** (no in-codebase analog; first wrapper-around-data-enum):
```rust
#[derive(Clone, PartialEq, Debug)]
pub struct BlockEntry {
    pub id: u64,
    pub block: Block,
}
```

**ShellSettings struct** (RESEARCH Pattern 5 lines 431-435):
```rust
use dioxus::prelude::Signal;

#[derive(Clone, Copy)]
pub struct ShellSettings {
    pub personality: Signal<Personality>,
    // Phase 5 will add: theme, density, block, agent (forward-compatible per D-02)
}
```
Note: `Signal<T>` is `Copy` so `ShellSettings` derives `Copy` — required for `use_context::<ShellSettings>()` to clone cheaply.

**`now_time()` cfg-gated helper** (D-34, no in-codebase analog):
```rust
pub fn now_time() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let d = js_sys::Date::new_0();
        format!("{:02}:{:02}:{:02}",
            d.get_hours() as u32,
            d.get_minutes() as u32,
            d.get_seconds() as u32)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "00:00:00".to_string()
    }
}
```

**Destructive rename** (D-09): `demo_blocks() -> Vec<Block>` becomes `demo_block_entries() -> Vec<BlockEntry>` returning entries with `id: 1..=10` (from existing `b1..b10` comments). No deprecation alias.

**Specific risks:**
- Adding `use dioxus::prelude::Signal;` at top of `state.rs` is required for `ShellSettings`. Without it, the struct field type errors.
- `js_sys::Date::get_hours()` returns `u32`-compatible double; cast pattern matters.
- The rename will break `src/components/warp_hermes.rs` line 23 — must be updated in same wave (Wave 1) OR Wave 4 wires the new name.

**Wave assignment:** **Wave 1** (foundation for Waves 2-4).

---

### MOD `src/platform/mod.rs`

**Analog:** `src/components/shell/mod.rs` lines 1-11 (`pub mod X;` declarations).

**Pattern:** replace 3-line placeholder comment with:
```rust
pub mod timer;
```

**Specific risks:** none. Trivial one-liner.

**Wave assignment:** **Wave 0** (alongside timer.rs).

---

### MOD `src/main.rs`

**Analog:** self lines 1-5 — established `mod X;` chain.

**Pattern** (existing):
```rust
mod app;
mod components;
mod fonts;
mod state;
mod platform;
```
Add one line:
```rust
mod mocks;
```

**Specific risks:** ordering doesn't matter for Rust mod declarations; group `mod mocks;` adjacent to `mod state;` for readability.

**Wave assignment:** **Wave 2** (alongside the module being declared).

---

### MOD `src/components/warp_hermes.rs` (the integration)

**Analog:** NONE — this is the FIRST stateful component. The closest reference is `warp2ironhermes/project/app/app.jsx` (READ-ONLY). RESEARCH.md provides the full template via Patterns 1-5 + Common Operations 1-4.

**Current shape** (warp_hermes.rs lines 21-55) — entirely static, hardcoded values, no signals, no handlers.

**Target shape** (synthesized from RESEARCH Patterns 3, 5 + Common Op 1, 3, 4):
```rust
use dioxus::prelude::*;
use dioxus::core::use_drop;             // NOTE: NOT in prelude
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

use crate::components::shell::{...};
use crate::state::{
    demo_block_entries, demo_messages, demo_palette_items, demo_tabs,
    BlockEntry, Mode, Personality, PaletteState, ShellSettings, TokenBudget,
    now_time,
};

#[component]
pub fn WarpHermes() -> Element {
    // 12 signals per D-01.
    let mut input          = use_signal(String::new);
    let mut blocks         = use_signal(demo_block_entries);
    let mut messages       = use_signal(demo_messages);
    let mut mode           = use_signal(|| Mode::Shell);
    let mut pal_open       = use_signal(|| false);
    let mut pal_query      = use_signal(String::new);
    let mut pal_state      = use_signal(|| PaletteState::Browse);
    let mut scanner_active = use_signal(|| false);
    let mut focused        = use_signal(|| false);
    let mut active_tab     = use_signal(|| 0_usize);
    let mut tokens         = use_signal(|| TokenBudget { used: 12_300, max: 128_000 });
    let mut next_id        = use_signal(|| 1000_u64);

    // ShellSettings context — D-02 + RESEARCH Pattern 5.
    let personality = use_signal(|| Personality::Default);
    use_context_provider(|| ShellSettings { personality });

    // Global keydown listener — RESEARCH Pattern 3 (~30 LOC).
    let mut listener_slot: Signal<Option<...>> = use_signal(|| None);
    use_effect(move || { /* install per Pattern 3 */ });
    use_drop(move || { /* remove per Pattern 3 */ });

    // Auto-scroll — RESEARCH Common Op 3.
    use_effect(move || {
        let len = blocks.read().len();
        if len == 0 { return; }
        if let Some(window) = web_sys::window() {
            if let Some(doc) = window.document() {
                if let Ok(Some(el)) = doc.query_selector(".wh-stream-scroll .wh-block:last-child") {
                    el.scroll_into_view_with_bool(false);
                }
            }
        }
    });

    // submit() — RESEARCH Common Op 1.
    let submit = move || { /* per Common Op 1 */ };

    // pick(item) — RESEARCH Common Op 4.
    let pick = move |item: PaletteItem| { /* per Common Op 4 */ };

    rsx! {
        div { class: "wh-app", "data-theme": "cyan", ...,
            TitleBar { tabs: demo_tabs(), active_tab: active_tab(), show_traffic_lights: true }
            div { class: "wh-main",
                div { class: "wh-col",
                    BlockStream { blocks: blocks.into() }     // Signal → ReadOnlySignal
                    InputBox {
                        value: input,
                        mode: mode.into(),
                        focused: focused,
                        on_submit: move |_| submit(),
                    }
                    StatusBar {
                        scanner_active: scanner_active.into(),
                        tokens: tokens.into(),
                        // mode/model/provider/hint either props or context
                    }
                }
                AgentPanel { messages: messages.into() }
            }
            CommandPalette {
                items: demo_palette_items(),
                query: pal_query,
                open: pal_open.into(),
                state: pal_state,
                on_pick: move |item| pick(item),
            }
        }
    }
}
```

**Specific risks (the integration is dense):**
- `use_drop` import is from `dioxus::core`, NOT `dioxus::prelude` (RESEARCH State of the Art row 2; AGENTS.md confirms).
- `Closure::forget()` leaks AND prevents cleanup — must use `Signal<Option<Closure<...>>>` storage (RESEARCH Pattern 3 alternative analysis lines 401-405).
- Auto-scroll naive `query_selector(...).unwrap()` panics on `/clear`; use the `if let Ok(Some(el))` triple-guard (RESEARCH Pitfall 5).
- ⌥M without `focused()` gate intercepts macOS Option-letter combos producing `µ` etc. (RESEARCH Pitfall 6).
- Signal `Copy` semantics: signals declared `let mut x = use_signal(...)` are `Copy`; passing to closures via `move` is the Dioxus idiom — no `.clone()` ceremony needed.
- `Signal<T> → ReadOnlySignal<T>` happens via `.into()` at call site OR coerces automatically depending on Dioxus 0.7 prop-conversion impls. Verify during execution.
- `spawn(async move { ... })` inside `submit` closure — `submit` itself is sync, but the spawned future awaits the mocks. Auto-spawn from event-handler closure is also valid (RESEARCH Summary line 18).

**Wave assignment:** **Wave 4** (the final integration).

---

### MOD `src/components/shell/block.rs`

**Analog:** self lines 25-86.

**Current prop signature** (line 26):
```rust
#[component]
pub fn Block(data: BlockData) -> Element {
```

**Target signature** (D-07: data type changes; D-23/D-24 add handlers):
```rust
#[component]
pub fn Block(
    entry: BlockEntry,
    on_copy: EventHandler<()>,
    on_rerun: EventHandler<()>,
) -> Element {
    let data = &entry.block;
    // ... existing destructuring + match logic stays ...
}
```

**onclick handler pattern for copy** (RESEARCH Common Op 2 lines 581-594):
```rust
button {
    class: "wh-icon-btn",
    title: "copy",
    onclick: move |_| {
        let text = block_text_for_copy(&entry.block);
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(&text);
        }
    },
    "⎘"
}
```

**Specific risks:**
- `EventHandler<()>` props don't need `Signal<>` wrapping — they're closures with `'static` lifetime.
- Existing `match data.clone()` body works unchanged once `data = &entry.block`; the `clone()` is on the inner Block enum (cheap Strings). Could optimize to non-clone match later.
- `is_cmd` boolean must drive rerun-button enabled/disabled state per D-24.

**Wave assignment:** **Wave 3**.

---

### MOD `src/components/shell/block_stream.rs`

**Analog:** self lines 17-27.

**Current** (line 17):
```rust
#[component]
pub fn BlockStream(blocks: Vec<BlockData>) -> Element {
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

**Target** (D-01 + D-07 stable keys):
```rust
#[component]
pub fn BlockStream(
    blocks: ReadOnlySignal<Vec<BlockEntry>>,
    on_copy: EventHandler<u64>,    // u64 = BlockEntry.id, parent dispatches
    on_rerun: EventHandler<u64>,
) -> Element {
    rsx! {
        div { class: "wh-stream",
            div { class: "wh-stream-scroll",
                for entry in blocks.read().iter().cloned() {
                    Block {
                        key: "{entry.id}",
                        entry: entry.clone(),
                        on_copy: move |_| on_copy.call(entry.id),
                        on_rerun: move |_| on_rerun.call(entry.id),
                    }
                }
            }
        }
    }
}
```

**Specific risks:**
- `blocks.read()` returns a `GenerationalRef` — keep its lifetime confined to the for-loop scope (no `.await` here, so safe).
- `key: "{entry.id}"` — D-07 mandates stable keys for `/clear` + append cycles. Old `key: "{i}"` is index-based and would collide on append.
- `for entry in ... .cloned()` iterates owned values; necessary because the parent wraps each in event-handler closures that need `move` ownership.

**Wave assignment:** **Wave 3**.

---

### MOD `src/components/shell/input_box.rs`

**Analog:** self lines 22-60.

**Current** (line 22):
```rust
#[component]
pub fn InputBox(mode: Mode, focused: bool) -> Element {
```

**Target** (D-19):
```rust
#[component]
pub fn InputBox(
    value: Signal<String>,
    mode: ReadOnlySignal<Mode>,
    focused: Signal<bool>,
    on_submit: EventHandler<()>,
) -> Element {
    let is_agent = matches!(mode(), Mode::Agent);
    // ... existing pill/glyph/placeholder logic uses mode() instead of `mode` ...
    rsx! {
        div { class: "wh-input-wrap", class: if focused() { "is-focus" },
            // ...
            textarea {
                class: "wh-textarea",
                rows: "1",
                placeholder: "{placeholder}",
                value: "{value}",
                oninput: move |e| value.set(e.value()),
                onkeydown: move |e| {
                    if e.key() == Key::Enter && !e.modifiers().shift() {
                        e.prevent_default();
                        on_submit.call(());
                    }
                },
                onfocus: move |_| focused.set(true),
                onblur:  move |_| focused.set(false),
            }
            // ...
        }
    }
}
```

**Specific risks:**
- `Signal<String>` for `value` enables controlled-component pattern. The `value: "{value}"` interpolation reads via Display; reactivity is preserved.
- `Key::Enter` import path: `dioxus::events::Key` (verify against AGENTS.md). Alternative: `e.key().to_string() == "Enter"`.
- `e.modifiers().shift()` — Dioxus 0.7 Modifiers API; verify against AGENTS.md.
- `e.prevent_default()` — Dioxus 0.7 event API; some events use `e.stop_propagation()` separately. Verify shape.
- The `focused` signal is read by the global keydown handler in WarpHermes for ⌥M gating (D-17). Both must point to the SAME signal.

**Wave assignment:** **Wave 3**.

---

### MOD `src/components/shell/command_palette.rs`

**Analog:** self lines 19-72.

**Current** (line 19):
```rust
#[component]
pub fn CommandPalette(items: Vec<PaletteItem>, query: String, open: bool) -> Element {
```

**Target** (D-18, D-20, D-32):
```rust
#[component]
pub fn CommandPalette(
    items: Vec<PaletteItem>,
    query: Signal<String>,
    open: ReadOnlySignal<bool>,
    state: Signal<PaletteState>,
    on_pick: EventHandler<PaletteItem>,
) -> Element {
    if !open() { return rsx! {}; }

    let mut selected = use_signal(|| 0_usize);

    // Live filter (D-32).
    let q = query().to_lowercase();
    let filtered: Vec<PaletteItem> = items.iter()
        .filter(|p| p.cmd.to_lowercase().contains(&q) || p.label.to_lowercase().contains(&q))
        .cloned()
        .collect();

    let on_keydown = move |e: KeyboardEvent| {
        let len = filtered.len();
        match e.key().as_str() {
            "ArrowDown" => { let s = selected(); selected.set((s + 1) % len.max(1)); }
            "ArrowUp"   => { let s = selected(); selected.set((s + len.saturating_sub(1)) % len.max(1)); }
            "Enter"     => { if let Some(item) = filtered.get(selected()) { on_pick.call(item.clone()); } }
            _ => {}
        }
    };
    // ... existing slash/workflow split + render ...
}
```

**Specific risks:**
- `selected` index can desync from `filtered.len()` after typing in query — reset to 0 on query change via secondary `use_effect`.
- `KeyboardEvent` here is `dioxus::events::KeyboardEvent`, NOT `web_sys::KeyboardEvent` (those are different types — distinguish carefully).
- `oninput: move |e| query.set(e.value())` on the `<input>` element — current code has `value: "{query}"` static; needs to become controlled.
- D-20 PaletteState transitions: `/personality` selection must NOT close the palette — `on_pick` handler in WarpHermes inspects the cmd before closing.

**Wave assignment:** **Wave 3**.

---

### MOD `src/components/shell/status_bar.rs`

**Analog:** self lines 19-44.

**Current** (line 19):
```rust
#[component]
pub fn StatusBar(
    mode: String,
    model: String,
    provider: String,
    tokens: TokenBudget,
    scanner_active: bool,
    hint: String,
) -> Element {
```

**Target** (D-22, RESEARCH Pattern 5):
```rust
use crate::state::ShellSettings;

#[component]
pub fn StatusBar(
    mode: String,                                 // can stay String if static
    model: String,
    provider: String,
    tokens: ReadOnlySignal<TokenBudget>,
    scanner_active: ReadOnlySignal<bool>,
    hint: String,
) -> Element {
    let settings = use_context::<ShellSettings>();
    let pers_label = settings.personality.read().label();

    let t = tokens();        // call-as-fn deref to get owned TokenBudget (Copy)
    let used_k = t.used as f32 / 1000.0;
    // ... rest unchanged ...

    rsx! {
        div { class: "wh-status",
            // ... existing pills ...
            span { class: "wh-pill", "/{pers_label}" }   // NEW personality pill (D-22)
            span { class: "wh-sep", "·" }
            Scanner { active: scanner_active() }
            span { class: "wh-hint", "{hint}" }
        }
    }
}
```

**Specific risks:**
- `tokens()` (call-as-fn) on `ReadOnlySignal<TokenBudget>` returns owned `TokenBudget` because it's `Copy` (state.rs line 159). For non-Copy types, use `tokens.read().clone()`.
- `use_context::<ShellSettings>()` panics if WarpHermes forgot the provider. Acceptable: development-time error.
- New personality pill placement matters for visual fidelity — verify against prototype HTML side-by-side during Wave 5 UAT.

**Wave assignment:** **Wave 3**.

---

### MOD `src/components/shell/agent_panel.rs`

**Analog:** self lines 19-55.

**Current** (line 19):
```rust
#[component]
pub fn AgentPanel(messages: Vec<Message>, personality: String) -> Element {
```

**Target** (D-02 context, D-22):
```rust
use crate::state::ShellSettings;

#[component]
pub fn AgentPanel(messages: ReadOnlySignal<Vec<Message>>) -> Element {
    let settings = use_context::<ShellSettings>();
    let personality = settings.personality.read().label();

    rsx! {
        aside { class: "wh-side",
            div { class: "wh-side-head",
                Sigil { size: 20_u16 }
                span { class: "wh-side-title", "HERMES" }
                span { class: "wh-personality", style: "cursor: default;", "/{personality}" }
            }
            div { class: "wh-side-scroll",
                for (i, m) in messages.read().iter().enumerate() {
                    // ... existing message rendering ...
                }
            }
        }
    }
}
```

**Specific risks:**
- `messages.read()` borrow lives across the for-loop; that's fine because no await happens here.
- Removing the `personality: String` prop is a breaking call-site change in WarpHermes. Make sure WarpHermes Wave 4 update drops the prop.
- AgentPanel is a read-only consumer; `Signal<Personality>` writes from /personality picks WILL re-render this panel. That's the intended reactivity (KBD-06).

**Wave assignment:** **Wave 3**.

---

### MOD `Cargo.toml`

**Analog:** self lines 9-10.

**Current:**
```toml
[dependencies]
dioxus = { version = "=0.7.1", features = [] }
```

**Target** (D-05, RESEARCH Standard Stack):
```toml
[dependencies]
dioxus = { version = "=0.7.1", features = [] }
gloo-timers = { version = "0.3", features = ["futures"] }
tokio = { version = "1", features = ["time"], default-features = false }
web-sys = { version = "0.3", features = [
    "Window", "Element", "Document",
    "Navigator", "Clipboard",
    "EventTarget", "KeyboardEvent",
] }
wasm-bindgen = "0.2"
js-sys = "0.3"
```

**Specific risks:**
- `tokio = full` is the wrong default — RESEARCH Anti-Patterns explicitly warns. Use `["time"]` only.
- Dioxus may transitively pull web-sys with different features; explicit declaration ensures our list.
- `gloo-timers = "0.4"` exists but is breaking; pin "0.3" per D-05.

**Wave assignment:** **Wave 0**.

---

## Shared Patterns

### Borrow-Then-Await Discipline (THE landmine)

**Source:** `clippy.toml` lines 1-8 + RESEARCH.md Pattern 2 (lines 244-322) + CONTEXT D-06.

**Apply to:** EVERY function in `src/mocks/` and EVERY closure in `src/components/warp_hermes.rs` that contains `.await`.

**The rule (verbatim from clippy.toml line 5):**
> Write should not be held over an await point. This will cause any reads or writes to fail while the await is pending since the write borrow is still active.

**The pattern:**
```rust
// GOOD: borrow drops at semicolon.
blocks.write().push(entry);   // <- WriteLock dropped here
sleep(600).await;             // <- no live borrow

// GOOD: clone before await.
let cur = mode();             // call-as-fn = owned clone (Copy types)
sleep(600).await;
match cur { ... }

// BAD: clippy fails.
let bs = blocks.write();
sleep(600).await;
bs.push(entry);
```

### Module Re-export Convention

**Source:** `src/components/shell/mod.rs` lines 1-41.

**Apply to:** `src/mocks/mod.rs`, `src/platform/mod.rs`.

**Pattern:**
```rust
pub mod submodule_a;
pub mod submodule_b;

pub use submodule_a::PublicItem;
```

### Component Function Convention

**Source:** CLAUDE.md "Naming Patterns" + every file in `src/components/shell/`.

**Apply to:** EVERY component (modified or new) in Phase 4.

**Pattern:**
```rust
use dioxus::prelude::*;

#[component]
pub fn ComponentName(prop: Type) -> Element {
    rsx! { /* ... */ }
}
```
- `PascalCase` function name (Dioxus macro requires).
- `#[component]` attribute mandatory.
- Returns `Element`.
- 4-space RSX indent.

### Signal-as-Prop Conventions

**Source:** RESEARCH.md D-01 + Pattern 5.

**Apply to:** every Phase 3 component being refactored (Wave 3).

**Pattern:**
| Use case | Prop type |
|----------|-----------|
| Child mutates the value (input, palette query) | `Signal<T>` (declare with `mut` if methods like `.set()` called) |
| Child only reads but must re-render on change | `ReadOnlySignal<T>` |
| Child renders a static snapshot once (initial seeds) | `T` directly (e.g., `Vec<PaletteItem>`) |
| Cross-cutting state (personality, future theme) | `use_context::<ShellSettings>()` — no prop |

### Doc-Comment Style

**Source:** every file in `src/components/shell/` and `src/state.rs`.

**Apply to:** every NEW or MODIFIED file in Phase 4.

**Pattern:** module-level `//!` summary; function-level `///` doc citing CONTEXT decisions (e.g., "per CONTEXT D-04") and prototype source-of-truth lines (e.g., "Port of `warp2ironhermes/project/app/shell.jsx` lines 150-183").

### Three-Platform Compile Gate (verification, not a code pattern)

**Source:** CLAUDE.md "Constraints" + CONTEXT D-35 + RESEARCH Validation Architecture.

**Apply to:** end of every wave.

**Commands:**
```bash
cargo build --features web
cargo build --features desktop
cargo build --features mobile
cargo clippy --features web -- -D warnings
```

---

## No Analog Found (use RESEARCH.md patterns)

| File | Role | Data Flow | Reason | RESEARCH ref |
|------|------|-----------|--------|--------------|
| `src/platform/timer.rs` | utility | async | First cfg-gated function; no precedent | Pattern 1 (lines 215-241) |
| `src/mocks/agent_steps.rs` | service | event-driven async | First async fn that mutates signals | Pattern 2 (lines 244-322) |
| `src/components/warp_hermes.rs` (rewire) | composer | event-driven hub | First stateful component; no `use_signal` exists yet in project | Patterns 3, 5 + Common Ops 1, 3, 4 |
| `src/components/shell/markdown.rs` | utility | str → Element transform | First non-`#[component]` Element-returning fn | Pattern 6 (lines 472-485) |

---

## Wave Assignment Summary

| Wave | Files | Gate |
|------|-------|------|
| **Wave 0** | `Cargo.toml`, `src/platform/mod.rs`, `src/platform/timer.rs` | Three-platform `cargo build` green |
| **Wave 1** | `src/state.rs` (extensions + rename) | Three-platform `cargo build` green |
| **Wave 2** | `src/main.rs` (+1 line), `src/mocks/mod.rs`, `src/mocks/personalities.rs`, `src/mocks/shell_outputs.rs`, `src/mocks/agent_steps.rs` | Three-platform `cargo build` + `cargo clippy --features web -- -D warnings` (await-holding rule first applies) |
| **Wave 3** | `src/components/shell/markdown.rs` (NEW), `block.rs`, `block_stream.rs`, `input_box.rs`, `command_palette.rs`, `status_bar.rs`, `agent_panel.rs` | Three-platform `cargo build` green |
| **Wave 4** | `src/components/warp_hermes.rs` (rewire) | Three-platform `cargo build` + clippy green |
| **Wave 5** | UAT only (manual; no code) | 6 ROADMAP success criteria + R1, R2 verification |

---

## Metadata

**Analog search scope:** `src/state.rs`, `src/main.rs`, `src/components/`, `src/components/shell/`, `src/platform/`, `Cargo.toml`, `clippy.toml`.
**Files scanned:** 14 source files + 2 config + 2 planning docs.
**Pattern extraction date:** 2026-05-03.

## PATTERN MAPPING COMPLETE

All 14 Phase 4 files classified with concrete analogs, code excerpts, risks, and 5-wave assignments per RESEARCH.md.
