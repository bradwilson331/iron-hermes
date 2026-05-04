# Phase 3: Desktop Shell - Pattern Map

**Mapped:** 2026-05-03
**Files analyzed:** 16 (12 new component files + state.rs + 2 mod.rs + 1 new CSS asset; plus 3 modified: app.rs, components/mod.rs, hero.rs deletion)
**Analogs found:** 16 / 16 (every Phase 3 file has a clear in-repo analog or upstream React analog with line-numbered excerpts)

## Strategy Note

The IronHermes repo has only **two** Dioxus 0.7 component files at this point: `src/components/hero.rs` (Phase 2 brand stub — slated for deletion in Phase 3) and `src/app.rs` (root with `document::Link` cascade). All Phase 3 components inherit their Dioxus 0.7 conventions from these two files; their **content** is a verbatim port of `warp2ironhermes/project/app/shell.jsx` (read-only React source).

Therefore "closest analog" for every Phase 3 component file is a pair:
- **Dioxus convention analog** — almost always `src/components/hero.rs` (the only existing presentational component) or `src/app.rs` (for the composer pattern).
- **Behavioral analog (React source-of-truth)** — the corresponding function in `warp2ironhermes/project/app/shell.jsx` that the Rust port mirrors 1:1 per CONTEXT D-01.

Both are listed in every pattern assignment below.

---

## File Classification

| New/Modified File | Role | Data Flow | Dioxus Analog | React Analog | Match Quality |
|-------------------|------|-----------|---------------|--------------|---------------|
| `src/components/warp_hermes.rs` | composer (top-level) | static-render | `src/app.rs` (composer w/ asset consts + child component invocations) | `app.jsx` `WarpHermes` (lines 232-275) | exact (composer-of-composers) |
| `src/components/shell/mod.rs` | module re-export | n/a | `src/components/mod.rs` (single `pub mod` + `pub use`) | n/a | exact |
| `src/components/shell/title_bar.rs` | component (chrome) | static-render | `src/components/hero.rs` (asset const + inline `style:` strings + `rsx!` tree) | `shell.jsx` `TitleBar` (lines 62-91) | exact (asset+style pattern) |
| `src/components/shell/sigil.rs` | component (primitive) | static-render w/ derived prop | `src/components/hero.rs` (single-element `rsx!`) | `shell.jsx` `Sigil` (lines 53-59) | role-match (no existing prop-driven analog) |
| `src/components/shell/block_stream.rs` | component (list iterator) | static-render w/ Vec prop | `src/components/hero.rs` (rsx skeleton; `for` loop NEW pattern) | n/a (D-02 extension beyond shell.jsx) | role-match |
| `src/components/shell/block.rs` | component (variant dispatcher) | static-render w/ enum prop + match | `src/components/hero.rs` (skeleton); RESEARCH Example 1 | `shell.jsx` `Block` (lines 94-113) + `app.jsx` `RenderBlock` | role-match (no existing match-on-enum component) |
| `src/components/shell/command_line.rs` | component (token-row) | static-render w/ Vec<Token> prop | `src/components/hero.rs` (rsx + inline style pattern) | `shell.jsx` `CommandLine` (lines 115-130) | role-match |
| `src/components/shell/tool_call.rs` | component (status-card) | static-render w/ struct + enum | `src/components/hero.rs` (rsx + inline style pattern) | `shell.jsx` `ToolCall` (lines 132-147) | role-match |
| `src/components/shell/input_box.rs` | component (form chrome) | static-render (no signals) | `src/components/hero.rs` (rsx pattern); textarea is NEW | `shell.jsx` `InputBox` (lines 150-183) — strip event handlers | role-match (no textarea analog yet) |
| `src/components/shell/agent_panel.rs` | component (panel + list) | static-render w/ Vec<Message> | `src/components/hero.rs` (rsx); list iteration NEW | `shell.jsx` `AgentPanel` (lines 186-211) — strip ref + useEffect | role-match |
| `src/components/shell/status_bar.rs` | component (chrome) | static-render w/ multiple props | `src/components/hero.rs` (rsx + inline style) | `shell.jsx` `StatusBar` (lines 30-50) | role-match |
| `src/components/shell/scanner.rs` | component (10 cells) | static-render (CSS animates) | `src/components/hero.rs` (single `rsx!`) | `shell.jsx` `Scanner` (lines 5-27) — strip useEffect; emit 10 plain `<span>` cells | role-match |
| `src/components/shell/command_palette.rs` | component (overlay + lists) | static-render w/ Vec<PaletteItem> + flag | `src/components/hero.rs` (rsx); double-list iteration NEW | `shell.jsx` `CommandPalette` (lines 214-268) — strip useState + handlers | role-match |
| `src/state.rs` (modified) | data module (types + fixtures) | n/a | `src/components/hero.rs` (only existing pattern is `const NAME: Asset = asset!(...)`); enum + struct definitions are NEW Rust idiom | `app.jsx` `seedBlocks()` / `seedMessages()` / `PALETTE_ITEMS` | role-match (none in repo; idiom is Rust-standard) |
| `src/components/mod.rs` (modified) | module re-export | n/a | itself (current file: `pub mod hero; pub use hero::Hero;`) | n/a | exact (extends own pattern) |
| `src/app.rs` (modified) | composer (root) | static-render | itself (current file already has the `document::Link` cascade and a child invocation) | n/a | exact (one-line swap: `Hero {}` → `WarpHermes {}`) |
| `assets/scanner-anim.css` (new) | static CSS asset | n/a | `assets/warp-ih.css` (existing CSS files in `assets/`) | n/a (planner-authored; satisfies SHELL-09 + D-08 gap) | role-match |

---

## Pattern Assignments

### `src/components/warp_hermes.rs` (composer, static-render)

**Dioxus analog:** `src/app.rs` lines 1-20 (composer with asset consts + child component invocation pattern).
**React analog:** `warp2ironhermes/project/app/app.jsx` `WarpHermes` (lines ~232-275 per RESEARCH Example 3).

**Imports pattern** (port of `src/app.rs` lines 1-2 — extend `crate::components::*` import to pull shell submodule):
```rust
use dioxus::prelude::*;
use crate::components::shell::*;
use crate::state::*;
```

**Component skeleton** (port of `src/app.rs` lines 10-20 — same `#[component] pub fn ... -> Element { rsx! { ... } }` shape):
```rust
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
            // child component invocations follow ...
        }
    }
}
```

**Data-attribute key escaping** (RESEARCH Pitfall 6 — `data-*` keys must be quoted strings, not Rust identifiers):
```rust
"data-theme": "cyan",
"data-density": "comfy",
"data-block": "framed",
"data-agent": "right",
```

**Child component invocation pattern** (port of `src/app.rs` line 18 `Hero {}` — Phase 3 swap target; pattern extends to all shell primitives):
```rust
TitleBar { tabs: tabs, active_tab: 0_usize, show_traffic_lights: true }
BlockStream { blocks: blocks }
InputBox { mode: Mode::Shell, focused: true }   // focused hardcoded per planner-handoff #4
StatusBar { mode: "Chat".to_string(), model: "claude-sonnet-4".to_string(), /* ... */ }
AgentPanel { messages: messages, personality: "default".to_string() }
CommandPalette { items: palette_items, query: String::new(), open: true }   // open hardcoded per D-19
```

**Anti-pattern guard:** No `use_signal`, no `use_memo`, no `use_resource` (CONTEXT D-06). The `let blocks = demo_blocks()` line is a plain function call, not a hook.

---

### `src/components/shell/mod.rs` (module re-export)

**Analog:** `src/components/mod.rs` lines 1-3 (existing pattern):
```rust
mod hero;

pub use hero::Hero;
```

**Pattern to apply** (extend the same idiom — one `pub mod` per file + one `pub use` per public type, so consumers do `use crate::components::shell::TitleBar` not `crate::components::shell::title_bar::TitleBar`):
```rust
pub mod title_bar;
pub mod sigil;
pub mod block_stream;
pub mod block;
pub mod command_line;
pub mod tool_call;
pub mod input_box;
pub mod agent_panel;
pub mod status_bar;
pub mod scanner;
pub mod command_palette;

pub use title_bar::TitleBar;
pub use sigil::Sigil;
pub use block_stream::BlockStream;
pub use block::Block;
pub use command_line::CommandLine;
pub use tool_call::ToolCall;
pub use input_box::InputBox;
pub use agent_panel::AgentPanel;
pub use status_bar::StatusBar;
pub use scanner::Scanner;
pub use command_palette::CommandPalette;
```

---

### `src/components/shell/title_bar.rs` (component, static-render)

**Dioxus analog:** `src/components/hero.rs` lines 1-15 (asset const declaration + `style:` inline strings + `rsx!` tree).
**React analog:** `shell.jsx` `TitleBar` lines 62-91 (verbatim 1:1 port target).

**Asset constants pattern** (port of `src/components/hero.rs` lines 3-4 — colocate consts with the consuming primitive per CONTEXT D-05):
```rust
// migrated verbatim from src/components/hero.rs (about to be deleted)
const WORDMARK_SVG: Asset = asset!("/assets/wordmark.svg");
const IH_SHIELD_PNG: Asset = asset!("/assets/ih-shield.png");
```

**Inline-style verbatim port pattern** (port of `src/components/hero.rs` line 10 `style: "display: flex; ...";` — directly mirrors React `style={{...}}` with all values in one quoted string):
```rust
// shell.jsx line 67: <span style={{ width: 12, height: 12, borderRadius: "50%", background: "#ff5f57" }} />
span { style: "width: 12px; height: 12px; border-radius: 50%; background: #ff5f57;" }
```

**Conditional rendering pattern** (RESEARCH Pattern 5 — direct `if cond` inside `rsx!`):
```rust
// shell.jsx line 65: {showTrafficLights && (...)}
if show_traffic_lights {
    div { style: "display: flex; gap: 8px; align-items: center; padding-right: 8px;",
        span { style: "width: 12px; height: 12px; border-radius: 50%; background: #ff5f57;" }
        span { style: "width: 12px; height: 12px; border-radius: 50%; background: #febc2e;" }
        span { style: "width: 12px; height: 12px; border-radius: 50%; background: #28c840;" }
    }
}
```

**Conditional class pattern** (RESEARCH Pattern 2 — multiple `class:` attributes concatenate):
```rust
// shell.jsx line 78: className={"wh-tab" + (i === activeTab ? " is-active" : "")}
div {
    class: "wh-tab",
    class: if i == active_tab { "is-active" },
    // ...
}
```

**For-loop iteration pattern** (RESEARCH Pattern 3 + AGENTS.md "Prefer loops over iterators"):
```rust
// shell.jsx line 77: {tabs.map((t, i) => ...)}
for (i, t) in tabs.iter().enumerate() {
    div { key: "{i}",
        class: "wh-tab",
        class: if i == active_tab { "is-active" },
        // ...
    }
}
```

**Sigil child invocation** (line 73 of shell.jsx — Sigil with `size={18}` prop):
```rust
Sigil { size: 18_u16 }
```

---

### `src/components/shell/sigil.rs` (component, static-render w/ derived value)

**Dioxus analog:** `src/components/hero.rs` (single-element `rsx!`).
**React analog:** `shell.jsx` `Sigil` lines 53-59.

**Derived-value-in-style pattern** (RESEARCH Pitfall 8 — compute in Rust, interpolate via `{var}px`):
```rust
// shell.jsx line 55: <span style={{ width: size, height: size, fontSize: size * 0.46 }}>
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

**Default-prop pattern** (Sigil's `size = 26` default in shell.jsx — Dioxus 0.7 uses `#[props(default = 26)]` on the function arg if the planner wants a default; otherwise call sites always pass an explicit value, which is the simpler Phase 3 path).

---

### `src/components/shell/block_stream.rs` (component, static-render w/ Vec prop)

**Dioxus analog:** `src/components/hero.rs` skeleton (no existing list-iteration component).
**React analog:** none — D-02 extension beyond shell.jsx. The wrapper exists to home the `wh-stream-scroll` chrome.

**For-loop with `key` pattern** (RESEARCH Pattern 3 + Example 2 — verbatim):
```rust
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

**Note on `key`:** Phase 3 has no re-renders so `key` is harmless either way; include it because Phase 4 will introduce signals and re-renders (RESEARCH Pitfall 4).

---

### `src/components/shell/block.rs` (component, static-render w/ enum match)

**Dioxus analog:** `src/components/hero.rs` skeleton; full pattern is in RESEARCH Example 1 lines 575-619.
**React analog:** `shell.jsx` `Block` lines 94-113 + `app.jsx` `RenderBlock` (per CONTEXT D-11 — outer chrome here, inner body delegated to `CommandLine` / `ToolCall` / inline text).

**Class-string interpolation pattern** (RESEARCH Pattern 2 + Pattern 6 — call enum helper method directly inside `rsx!` class string):
```rust
// shell.jsx line 96: className={"wh-block is-" + kind}
let kind_class = data.kind_class();
rsx! {
    div { class: "wh-block {kind_class}",
        // ...
    }
}
```

**Enum-variant `match` inside `rsx!` pattern** (RESEARCH Example 1 lines 596-617 — body composition by variant):
```rust
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
        div { class: "wh-block-body", "{markdown}" }   // <div>, not <pre> per UI-SPEC
    },
    Block::Ok { message } => rsx! {
        div { class: "wh-block-body", "{message}" }
    },
    Block::Err { message } => rsx! {
        div { class: "wh-block-body", "{message}" }
    },
}
```

**Hover-action button row pattern** (port of shell.jsx lines 105-109; visibility is pure CSS per CONTEXT D-15):
```rust
div { class: "wh-block-actions",
    button { class: "wh-icon-btn", title: "copy", "⎘" }
    if !matches!(data, Block::Cmd { .. }) {
        // is-cmd shows only copy + rerun per app.jsx RenderBlock; tweak per UI-SPEC if needed
        button { class: "wh-icon-btn", title: "rerun", "↻" }
        button { class: "wh-icon-btn", title: "share", "↗" }
    }
}
```

**Block head pattern** (port of shell.jsx lines 97-103 — author + status chips + time):
```rust
div { class: "wh-block-head",
    if let Some(author) = author { span { class: "wh-author", "{author}" } }
    if matches!(data, Block::Ok { .. }) {
        span { style: "color: var(--success);", "[OK]" }
    }
    if let Block::Err { .. } = &data {
        span { style: "color: var(--danger);", "exit 1" }
    }
    if let Some(time) = time { span { style: "margin-left: auto;", "{time}" } }
}
```

---

### `src/components/shell/command_line.rs` (component, token-row)

**Dioxus analog:** `src/components/hero.rs` (rsx + inline style pattern).
**React analog:** `shell.jsx` `CommandLine` lines 115-130.

**Token-class interpolation pattern** (RESEARCH Pattern 2 — class string built from enum-variant helper):
```rust
// shell.jsx line 122: className={"wh-cmd-" + (p.kind || "arg") + (p.kind === "bin" ? " wh-cmd" : "")}
for (i, t) in tokens.iter().enumerate() {
    let kind = t.kind_class();   // "bin" / "arg" / "flag" / "str"
    span {
        key: "{i}",
        class: "wh-cmd-{kind}",
        class: if matches!(t, Token::Bin(_)) { "wh-cmd" },
        style: if matches!(t, Token::Bin(_)) { "color: var(--fg-strong); font-weight: 700;" },
        if i > 0 { " " }
        "{t.text()}"
    }
}
```

**Outer cmdline structure** (port of shell.jsx lines 116-129):
```rust
div { class: "wh-cmdline",
    span { style: "color: var(--fg-dim); font-size: 11px;", "{cwd}" }   // cwd default "~/projects/ironhermes"
    span { class: "wh-prompt-glyph", "{glyph}" }                         // glyph default "❯"
    span { style: "flex: 1;",
        // for-loop over tokens (above)
    }
    if let Some(time) = time { span { class: "wh-cmd-time", "{time}" } }
}
```

---

### `src/components/shell/tool_call.rs` (component, static-render)

**Dioxus analog:** `src/components/hero.rs` (rsx + inline style pattern).
**React analog:** `shell.jsx` `ToolCall` lines 132-147.

**Status-driven inline color pattern** (port of shell.jsx line 138 — color computed from status enum):
```rust
let status_color = match status {
    ToolStatus::Done => "var(--success)",
    _ => "var(--warn)",
};
let status_text = match status {
    ToolStatus::Done => "[OK]",
    ToolStatus::Pending => "pending…",
    ToolStatus::Running => "running…",
    ToolStatus::Failed => "failed",
};
rsx! {
    div { class: "wh-toolcall",
        div { style: "display: flex; gap: 8px; align-items: baseline;",
            span { style: "color: var(--fg-dim);", "Tool:" }
            b { "{name}" }
            span { style: "margin-left: auto; font-size: 10px; color: {status_color};", "{status_text}" }
        }
        if !args_summary.is_empty() {
            pre { style: "margin: 4px 0 0; color: var(--fg-dim); font-size: 11px; white-space: pre-wrap;", "{args_summary}" }
        }
    }
}
```

---

### `src/components/shell/input_box.rs` (component, form chrome — no signals in Phase 3)

**Dioxus analog:** `src/components/hero.rs` (rsx pattern); textarea is a NEW HTML element for this codebase.
**React analog:** `shell.jsx` `InputBox` lines 150-183 — **strip all event handlers** per CONTEXT D-06 (Phase 4 reintroduces them).

**Conditional class on focus state** (port of shell.jsx line 152):
```rust
// Phase 3 hardcodes focused: true (planner-handoff #4) so the focus ring is visible during UAT
div {
    class: "wh-input-wrap",
    class: if focused { "is-focus" },
    // ...
}
```

**Mode-driven glyph and pill** (port of shell.jsx lines 154-159 — Phase 3 hardcodes Shell mode per UI-SPEC, but the component stays mode-aware for Phase 4):
```rust
let is_agent = matches!(mode, Mode::Agent);
let pill_label = if is_agent { "Agent" } else { "Shell" };
let prompt_glyph = if is_agent { "✦" } else { "❯" };
let placeholder = if is_agent {
    "Ask IronHermes anything…"
} else {
    "Type a command, or `/` for commands"
};
rsx! {
    div { class: "wh-input-mode",
        span {
            class: "wh-mode-pill",
            class: if is_agent { "is-agent" },
            "{pill_label}"
        }
        span { "⌥+M to switch" }
        span { style: "margin-left: auto;", "↵ run · ⇧↵ newline · ⌃C cancel" }
    }
    div { class: "wh-input-row",
        span { class: "wh-prompt-glyph", "{prompt_glyph}" }
        textarea {
            class: "wh-textarea",
            rows: "1",
            placeholder: "{placeholder}",
            // NO oninput, NO onkeydown, NO onfocus, NO onblur — Phase 4 wires these
        }
        div { class: "wh-input-actions",
            button { class: "wh-icon-btn", title: "attach", "@" }
            button { class: "wh-icon-btn", title: "voice", "●" }
            button { class: "wh-icon-btn", title: "run",
                style: "color: var(--accent-primary);",
                "↵"
            }
        }
    }
}
```

**Anti-pattern guard:** **No `oninput`, no `onkeydown`, no `value` signal binding.** The textarea is purely visual in Phase 3 (the user can type into it via browser default behavior, but no Rust state captures the text). Phase 4 (KBD-01..KBD-04) wires this.

---

### `src/components/shell/agent_panel.rs` (component, static-render w/ Vec)

**Dioxus analog:** `src/components/hero.rs` (rsx); list iteration is NEW.
**React analog:** `shell.jsx` `AgentPanel` lines 186-211 — **strip `useRef`/`useEffect`** per UI-SPEC (auto-scroll is JS-driven; Phase 3 omits).

**Sigil + title + personality pill** (port of shell.jsx lines 192-197):
```rust
aside { class: "wh-side",
    div { class: "wh-side-head",
        Sigil { size: 20_u16 }
        span { class: "wh-side-title", "HERMES" }
        span { class: "wh-personality", style: "cursor: default;", "/{personality}" }
    }
    div { class: "wh-side-scroll",
        // for-loop over messages (below)
    }
}
```

**Message list with tool-call branch** (port of shell.jsx lines 199-207):
```rust
for (i, m) in messages.iter().enumerate() {
    div { key: "{i}",
        class: "wh-msg",
        class: if m.who == "user" { "is-user" } else { "is-hermes" },
        div { class: "wh-msg-meta",
            b { if m.who == "user" { "You" } else { "Hermes" } }
            span { "{m.time}" }
        }
        if let Some(tool) = &m.tool {
            ToolCall {
                name: tool.name.clone(),
                args_summary: tool.args_summary.clone(),
                status: tool.status.clone(),
            }
        } else {
            div { class: "wh-msg-body", "{m.body}" }
        }
    }
}
```

---

### `src/components/shell/status_bar.rs` (component, chrome)

**Dioxus analog:** `src/components/hero.rs` (rsx + inline style — pill colors are inline `style: "color: var(--pill-N);"`).
**React analog:** `shell.jsx` `StatusBar` lines 30-50.

**Pill rotation via inline style pattern** (RESEARCH Pitfall 7 — each pill gets its own `style: "color: var(--pill-N);"` rather than a per-pill class):
```rust
let pct = (tokens.used as f32 / tokens.max as f32 * 100.0).round() as u32;
let used_k = tokens.used as f32 / 1000.0;
let max_k = (tokens.max / 1000) as u32;
rsx! {
    div { class: "wh-status",
        span { class: "wh-pill", style: "color: var(--pill-0);", "{mode}" }
        span { class: "wh-sep", "·" }
        span { class: "wh-pill", style: "color: var(--pill-1);", "{model}" }
        span { class: "wh-sep", "·" }
        span { class: "wh-pill", style: "color: var(--pill-2);", "{provider}" }
        span { class: "wh-sep", "·" }
        span { class: "wh-pill", style: "color: var(--pill-3);", "{used_k:.1}K/{max_k}K ({pct}%)" }
        if scanner_active {
            span { class: "wh-sep", "·" }
            Scanner { active: true }
        }
        span { class: "wh-hint", "{hint}" }
    }
}
```

---

### `src/components/shell/scanner.rs` (component, 10 cells; CSS animates)

**Dioxus analog:** `src/components/hero.rs` (single `rsx!`).
**React analog:** `shell.jsx` `Scanner` lines 5-27 — **strip `useState` + `useEffect` + `setInterval`** per CONTEXT D-08. Render exactly 10 plain `<span>` cells with `░` glyphs; CSS `@keyframes` (in `assets/scanner-anim.css`) animates the color.

**Pure-CSS render pattern** (CONTEXT D-08 + RESEARCH Pattern 7):
```rust
#[component]
pub fn Scanner(active: bool) -> Element {
    rsx! {
        span {
            class: "wh-scanner",
            class: if active { "is-active" },
            "aria-hidden": "true",
            // 10 cells, all rendered with default off-glyph "░"
            // CSS @keyframes wh-scanner-tick animates the color via :nth-child animation-delay
            for i in 0..10 {
                span { key: "{i}", "░" }
            }
        }
    }
}
```

**Anti-pattern guard:** No `use_signal`, no `use_effect`, no `gloo_timers`. The animation is 100% CSS.

---

### `src/components/shell/command_palette.rs` (component, overlay + lists)

**Dioxus analog:** `src/components/hero.rs` (rsx); double-list iteration is NEW.
**React analog:** `shell.jsx` `CommandPalette` lines 214-268 — **strip all `useState` + key handlers**; render with `open: true` hardcoded per CONTEXT D-19; first row gets `is-active` per UI-SPEC.

**Overlay open-gate pattern** (port of shell.jsx line 215 `if (!open) return null;`):
```rust
if !open { return rsx! {}; }
```

**Search row pattern** (port of shell.jsx lines 224-235 — strip `value`/`onChange`/`onKeyDown`):
```rust
div { class: "wh-pal-search",
    span { style: "color: var(--accent-primary); font-weight: 700;", "⌘K" }
    input {
        placeholder: "Search commands, files, recent…",
        value: "{query}",
        // NO onchange, NO onkeydown — Phase 4 wires
    }
    span { class: "wh-kbd", "esc" }
}
```

**Sectioned list pattern** (port of shell.jsx lines 237-263 — two `for` loops, one per section, with `is-active` on first item per UI-SPEC):
```rust
let slash_items: Vec<_> = items.iter().filter(|p| p.section == "slash").collect();
let workflow_items: Vec<_> = items.iter().filter(|p| p.section == "workflow").collect();
rsx! {
    div { class: "wh-pal-list",
        div { class: "wh-pal-section", "Slash commands" }
        for (i, it) in slash_items.iter().enumerate() {
            div { key: "{it.cmd}",
                class: "wh-pal-row",
                class: if i == 0 { "is-active" },
                span { class: "wh-pal-glyph", "/" }
                span { style: "color: var(--fg-strong);", "{it.cmd}" }
                span { style: "color: var(--fg-dim);", "— {it.label}" }
                span { class: "wh-pal-hint",
                    span { class: "wh-pal-kbd",
                        for (j, k) in it.kbd.iter().enumerate() {
                            span { key: "{j}", class: "wh-kbd", "{k}" }
                        }
                    }
                }
            }
        }
        div { class: "wh-pal-section", "Workflows" }
        for it in workflow_items.iter() {
            div { key: "{it.cmd}",
                class: "wh-pal-row",
                span { class: "wh-pal-glyph", "▸" }
                span { style: "color: var(--fg-strong);", "{it.label}" }
                span { style: "color: var(--fg-dim);", "{it.cmd}" }
            }
        }
    }
}
```

---

### `src/state.rs` (modified — data module)

**Analog:** none in repo (file is currently `// Phase placeholder — implementation begins in Phase 4`). Idiom is standard Rust enum + struct definitions; closest precedent is `src/components/hero.rs` lines 3-4 for the `Asset` declaration pattern (but consts go in components, not state).

**Standard pattern** (CONTEXT D-04 + D-07 + D-10..D-13):

```rust
// All shared types derive Clone + PartialEq for Dioxus prop bound, plus Debug for {:?} introspection
#[derive(Clone, PartialEq, Debug)]
pub enum Block {
    Cmd { command: CommandLine },
    Out { text: String },
    Ai { markdown: String },
    Ok { message: String },
    Err { message: String },
    Tool { call: ToolCall },
}

impl Block {
    // RESEARCH Pattern 6 — variant→class mapping next to type
    pub fn kind_class(&self) -> &'static str {
        match self {
            Block::Cmd { .. } => "is-cmd",
            Block::Out { .. } => "is-out",
            Block::Ai { .. } => "is-ai",
            Block::Ok { .. } => "is-ok",
            Block::Err { .. } => "is-err",
            Block::Tool { .. } => "is-tool",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct CommandLine {
    pub tokens: Vec<Token>,
    pub time: Option<String>,
    pub cwd: Option<String>,
    pub glyph: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Token {
    Bin(String),
    Arg(String),
    Flag(String),
    Str(String),
}

impl Token {
    pub fn kind_class(&self) -> &'static str { /* "bin" / "arg" / "flag" / "str" */ }
    pub fn text(&self) -> &str { /* unwrap inner String */ }
}

#[derive(Clone, PartialEq, Debug)]
pub struct ToolCall {
    pub name: String,
    pub args_summary: String,
    pub status: ToolStatus,
}

#[derive(Clone, PartialEq, Debug)]
pub enum ToolStatus { Pending, Running, Done, Failed }

#[derive(Clone, PartialEq, Debug)]
pub enum Mode { Shell, Agent }

#[derive(Clone, PartialEq, Debug)]
pub struct PaletteItem {
    pub section: String,   // "slash" | "workflow"
    pub cmd: String,
    pub label: String,
    pub kbd: Vec<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Tab {
    pub label: String,
    pub live: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Message {
    pub who: String,           // "user" | "hermes"
    pub time: String,
    pub body: String,
    pub tool: Option<ToolCall>,
}

#[derive(Clone, PartialEq, Debug, Copy)]
pub struct TokenBudget { pub used: u32, pub max: u32 }

// Fixtures — port verbatim from app.jsx seedBlocks()/seedMessages()/PALETTE_ITEMS per D-17/D-18/D-20
pub fn demo_blocks() -> Vec<Block> { /* ~8-10 blocks per UI-SPEC */ }
pub fn demo_messages() -> Vec<Message> { /* 5 messages from seedMessages() */ }
pub fn demo_palette_items() -> Vec<PaletteItem> { /* 6 slash + 4 workflow */ }
pub fn demo_tabs() -> Vec<Tab> { /* 3 tabs from app.jsx */ }
```

**Critical derives** (RESEARCH Pitfall 2 + AGENTS.md Components rules — Dioxus props bound is `Clone + PartialEq`):
- Every type that crosses a component boundary derives `#[derive(Clone, PartialEq, Debug)]`.
- `TokenBudget` additionally derives `Copy` since it's three `u32` fields and gets passed by value.
- Forgetting `PartialEq` causes `error[E0277]: the trait bound 'Block: PartialEq' is not satisfied` at component-call sites.

---

### `src/components/mod.rs` (modified — module re-export)

**Analog:** itself (current content lines 1-3):
```rust
mod hero;

pub use hero::Hero;
```

**Modification pattern** (extends the same idiom — replace `hero` with the two new submodules):
```rust
pub mod warp_hermes;
pub mod shell;

pub use warp_hermes::WarpHermes;
// Note: shell submodule re-exports its primitives via shell/mod.rs;
// consumers do `use crate::components::shell::TitleBar` etc.
```

---

### `src/app.rs` (modified — root composer)

**Analog:** itself (current content lines 1-20). Phase 3 changes are minimal: one import swap, one rsx-child swap, optionally one new `document::Link` for `scanner-anim.css`.

**Imports** (port of current line 2):
```rust
// Before:
use crate::components::Hero;
// After:
use crate::components::WarpHermes;
```

**Asset const additions** (extend the existing pattern at lines 4-8):
```rust
// Add (only if planner picks Option A in RESEARCH Pattern 7 — separate scanner-anim.css file):
const SCANNER_ANIM_CSS: Asset = asset!("/assets/scanner-anim.css");
```

**document::Link cascade** (extend lines 13-17 — append after `WARP_IH_CSS` per RESEARCH Pitfall 5 cascade-order rule):
```rust
document::Link { rel: "stylesheet", href: WARP_IH_CSS }
document::Link { rel: "stylesheet", href: SCANNER_ANIM_CSS }   // NEW — must come AFTER warp-ih.css
```

**Child swap** (port of line 18):
```rust
// Before:
Hero {}
// After:
WarpHermes {}
```

---

### `assets/scanner-anim.css` (new — CSS asset)

**Analog:** `assets/warp-ih.css` (existing CSS file in `assets/` — no Dioxus pattern, just plain CSS).

**Content** (verbatim from RESEARCH Pattern 7 lines 415-438):
```css
/* Phase 3 design-gap fix: animate the 10-cell scanner via pure CSS.
   Resolves SHELL-09 + CONTEXT D-08 simultaneously. The prototype's
   shell.jsx Scanner (lines 5-27) uses React setInterval; pure-CSS
   port is pixel-equivalent and GPU-friendly.
   Period and width come from --scanner-period / --scanner-width in
   design-tokens.css (already loaded earlier in the cascade). */

@keyframes wh-scanner-tick {
   0%    { color: var(--fg-dim); }
   11.1% { color: color-mix(in oklab, var(--accent-primary) 50%, var(--fg-dim)); }
   22.2% { color: var(--accent-primary); }
   33.3% { color: var(--accent-primary-hi); }
   44.4% { color: var(--accent-primary); }
   55.5% { color: color-mix(in oklab, var(--accent-primary) 50%, var(--fg-dim)); }
   66.6% { color: var(--fg-dim); }
   100%  { color: var(--fg-dim); }
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

---

## Shared Patterns

### Pattern S-1: Asset constants colocate with consuming primitive

**Source:** `src/components/hero.rs` lines 3-4 (current pattern).
**Apply to:** `title_bar.rs` (`WORDMARK_SVG`, `IH_SHIELD_PNG`); `app.rs` (CSS link cascade — additive only); any future primitive that consumes a static asset.

```rust
// Module-top declaration; SCREAMING_SNAKE_CASE; absolute /assets/ path.
const WORDMARK_SVG: Asset = asset!("/assets/wordmark.svg");
```

CONTEXT D-05 + RESEARCH Pitfall 3: the path string is **identical** when migrating between files (it's project-root-relative). No path adjustment needed when moving consts from `hero.rs` to `title_bar.rs`.

---

### Pattern S-2: Component skeleton (Dioxus 0.7 idiom)

**Source:** `src/components/hero.rs` lines 6-15 + `src/app.rs` lines 10-20.
**Apply to:** Every Phase 3 component file.

```rust
use dioxus::prelude::*;
use crate::state::*;          // every shell primitive does this per CONTEXT D-04

#[component]
pub fn ComponentName(/* plain owned props */) -> Element {
    rsx! {
        // ...
    }
}
```

**Rules** (from CLAUDE.md + AGENTS.md):
- Function name is `PascalCase`.
- `#[component]` macro is required.
- Return type is `Element`.
- `use dioxus::prelude::*;` is the only Dioxus import — never enumerate individual items.
- Props are owned values (`String`, `Vec<T>`, `Block`, etc.) — never `&str` or `&[T]`.
- Props must derive `Clone + PartialEq`.

---

### Pattern S-3: Inline `style: "..."` strings (verbatim React port)

**Source:** `src/components/hero.rs` line 10 (`style: "display: flex; flex-direction: column; ..."`); shell.jsx uses `style={{...}}` everywhere.
**Apply to:** `title_bar.rs`, `block.rs`, `command_line.rs`, `tool_call.rs`, `input_box.rs`, `agent_panel.rs`, `status_bar.rs`, `command_palette.rs` — basically every primitive with traffic-light dots, pill colors, sigil sizing, layout flex tweaks, status colors, etc.

Verbatim translation: React `style={{ width: 12, background: "#ff5f57" }}` → Dioxus `style: "width: 12px; background: #ff5f57;"`. Use `{var}` interpolation for dynamic values (RESEARCH Pitfall 8). **Do not** use Dioxus typed CSS attributes — pixel-perfect-to-prototype outranks code aesthetics.

---

### Pattern S-4: Class-string interpolation (no `format!` inside `rsx!`)

**Source:** RESEARCH Pattern 2 + AGENTS.md `class: "container"` example.
**Apply to:** every component with a dynamic class (`Block`, `CommandLine` token spans, `TitleBar` tabs, `InputBox` wrap, `Scanner`, `CommandPalette` rows).

```rust
class: "wh-block {kind_class}",                            // GOOD — direct interpolation
class: if is_active { "is-active" },                        // GOOD — conditional second class
class: format!("wh-block is-{}", kind),                     // BAD — never use format! in rsx!
```

The Dioxus macro's `IfmtInput` handles the formatting at macro-expansion time.

---

### Pattern S-5: For-loop iteration over `Vec<T>`

**Source:** AGENTS.md `for i in 0..5 { div { "{i}" } }` + RESEARCH Pattern 3 + Example 2.
**Apply to:** `BlockStream`, `TitleBar` tabs, `CommandLine` tokens, `AgentPanel` messages, `CommandPalette` rows, `Scanner` cells.

```rust
for (i, item) in items.iter().enumerate() {
    Component { key: "{i}", data: item.clone() }
}
```

`for` is the project-preferred idiom over `.map()` chains (AGENTS.md "Prefer loops over iterators"). Always include `key:` even though Phase 3 has no re-renders — Phase 4 introduces signals (RESEARCH Pitfall 4).

---

### Pattern S-6: Conditional rendering + conditional class

**Source:** AGENTS.md `if condition { div { ... } }` + multi-`class:` attribute concatenation.
**Apply to:** `TitleBar` traffic lights gate, `Block` head + status chips, `InputBox` focus class, `CommandPalette` open gate, `Block` hover-actions branching.

```rust
if show_traffic_lights { /* ... */ }                           // gate-render whole subtree
class: if is_active { "is-active" }                            // append class conditionally
if matches!(data, Block::Cmd { .. }) { /* cmd-only branch */ } // enum-pattern gate
if let Some(time) = time { span { "{time}" } }                 // Option-binding
```

---

### Pattern S-7: Data-attribute keys must be quoted strings

**Source:** RESEARCH Pitfall 6.
**Apply to:** `WarpHermes` root (`data-theme`/`data-density`/`data-block`/`data-agent`); `Scanner` (`aria-hidden`); any future `data-*` or `aria-*` attribute.

```rust
"data-theme": "cyan",          // GOOD — string-literal key
"aria-hidden": "true",         // GOOD
data_theme: "cyan",            // BAD — Rust identifier rule rejects hyphens; auto-conversion only works for some attrs
```

---

### Pattern S-8: No reactivity in Phase 3 (forbidden hooks)

**Source:** CONTEXT D-06 + RESEARCH "Anti-Patterns to Avoid".
**Apply to:** Every Phase 3 component without exception.

**Forbidden in Phase 3:**
- `use_signal(...)`
- `use_memo(...)`
- `use_resource(...)`
- `use_context_provider(...)` / `use_context::<T>()`
- Any `oninput`, `onclick`, `onkeydown`, `onfocus`, `onblur`, etc. event handler
- Any `Signal<T>` / `ReadOnlySignal<T>` prop type

**Forbidden everywhere (project-wide per CLAUDE.md):**
- `cx`, `Scope`, `use_state` (Dioxus 0.6 APIs — won't compile on 0.7).

The planner must reject any task that introduces these in Phase 3.

---

### Pattern S-9: Plain function calls for fixture data (not hooks)

**Source:** `src/components/hero.rs` (no fixture data yet); RESEARCH Example 3 line 650 `let blocks = demo_blocks();`.
**Apply to:** `WarpHermes` (consumes `demo_blocks()`, `demo_messages()`, `demo_palette_items()`, `demo_tabs()`).

```rust
let blocks = demo_blocks();   // plain fn call — not a hook
```

Phase 4 may swap these for `use_signal(|| demo_blocks())` if interactivity demands it; Phase 3 stays plain.

---

### Pattern S-10: Pure-CSS hover affordance (zero Rust state)

**Source:** CONTEXT D-15 + RESEARCH "Don't Hand-Roll" table.
**Apply to:** `Block` hover-action button row.

```rust
// Render unconditionally — visibility driven entirely by CSS:
//   .wh-block:hover .wh-block-actions { opacity: 1 }
// (already in assets/warp-ih.css line 227)
div { class: "wh-block-actions",
    button { class: "wh-icon-btn", title: "copy", "⎘" }
    button { class: "wh-icon-btn", title: "rerun", "↻" }
    button { class: "wh-icon-btn", title: "share", "↗" }
}
```

---

## Cross-File Pattern Index

| Phase 3 Component | Patterns Applied |
|-------------------|------------------|
| `warp_hermes.rs` | S-2, S-3, S-7, S-8, S-9 |
| `shell/mod.rs` | (re-export idiom only) |
| `title_bar.rs` | S-1, S-2, S-3, S-4, S-5, S-6 |
| `sigil.rs` | S-2, S-3 (with Pitfall 8 derived-value) |
| `block_stream.rs` | S-2, S-5 |
| `block.rs` | S-2, S-3, S-4, S-6, S-10 |
| `command_line.rs` | S-2, S-3, S-4, S-5, S-6 |
| `tool_call.rs` | S-2, S-3, S-6 |
| `input_box.rs` | S-2, S-3, S-4, S-6, S-8 |
| `agent_panel.rs` | S-2, S-3, S-5, S-6 |
| `status_bar.rs` | S-2, S-3, S-6 (Pitfall 7 inline pill colors) |
| `scanner.rs` | S-2, S-5, S-6, S-8 |
| `command_palette.rs` | S-2, S-3, S-4, S-5, S-6, S-8 |
| `state.rs` | (Rust idiom — `#[derive(Clone, PartialEq, Debug)]` blanket on every shared type) |
| `components/mod.rs` | (re-export idiom only) |
| `app.rs` | S-1 (additive); minimal swap |
| `scanner-anim.css` | (plain CSS — no Dioxus pattern) |

---

## No Analog Found

Every Phase 3 file has at least one analog in either the existing repo (`src/components/hero.rs`, `src/app.rs`, `src/components/mod.rs`) or the read-only React source-of-truth (`warp2ironhermes/project/app/shell.jsx` and `app.jsx`). **Zero files lack an analog.**

The codebase is small enough (Phase 1 + Phase 2 deliverables only) that "closest analog" frequently resolves to the same two files (`hero.rs`, `app.rs`). This is expected — Phase 3 is the first major component-tree expansion, and `hero.rs` is the established Dioxus 0.7 idiom-template the planner should treat as canonical.

---

## Metadata

**Analog search scope:** `src/`, `assets/`, `warp2ironhermes/project/app/`.
**Files scanned:** `src/main.rs`, `src/app.rs`, `src/components/mod.rs`, `src/components/hero.rs`, `src/state.rs`, `src/fonts.rs` (skimmed), `src/platform/mod.rs` (empty), `assets/warp-ih.css` (referenced via UI-SPEC excerpts), `warp2ironhermes/project/app/shell.jsx` (full), `AGENTS.md` (Dioxus 0.7 reference), `CLAUDE.md` (project conventions).

**Pattern extraction date:** 2026-05-03.

**Key insight for the planner:** Every Phase 3 file is either (a) a structural clone of `src/components/hero.rs` with a richer rsx tree and props, or (b) a verbatim port of one shell.jsx primitive with React-isms stripped (state hooks, event handlers, refs, useEffect). The planner's plan files should reference these two files by name and line range — `hero.rs:1-15` for Dioxus convention, `shell.jsx:NN-MM` for content fidelity — rather than restating the patterns inline.
