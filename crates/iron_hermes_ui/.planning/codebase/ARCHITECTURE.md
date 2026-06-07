<!-- refreshed: 2026-05-02 -->
# Architecture

**Analysis Date:** 2026-05-02

## System Overview

```text
┌─────────────────────────────────────────────────────────────┐
│                    Entry Point                               │
│  `src/main.rs` — dioxus::launch(App)                        │
└──────────────────────────┬──────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                  Root Component: App                         │
│  `src/main.rs` — injects CSS assets, renders Hero           │
└──────────┬────────────────────────────────────────┬─────────┘
           │                                        │
           ▼                                        ▼
┌─────────────────────┐              ┌──────────────────────────┐
│   document::Link    │              │    Hero Component         │
│  (favicon, CSS)     │              │   `src/main.rs`           │
│  `src/main.rs`      │              │   renders #hero + #links  │
└─────────────────────┘              └──────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────┐
│                     Static Assets                            │
│  `assets/favicon.ico` · `assets/main.css`                   │
│  `assets/header.svg`  · `assets/tailwind.css`               │
└─────────────────────────────────────────────────────────────┘
```

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| `App` | Root component; links CSS/favicon, renders Hero | `src/main.rs` |
| `Hero` | Displays header SVG and navigation links | `src/main.rs` |

## Pattern Overview

**Overall:** Minimal Dioxus 0.7 single-page application — scaffold state.

**Key Characteristics:**
- Single `main.rs` containing all Rust source code (no module split yet)
- Dioxus component model: functions annotated with `#[component]`, returning `Element`
- `rsx!` macro for declarative HTML-like UI trees
- Static asset management via Dioxus `asset!()` macro
- Multi-platform via Cargo features: `web` (default), `desktop`, `mobile`

## Layers

**Entry:**
- Purpose: Bootstraps the Dioxus runtime
- Location: `src/main.rs` (line 8–10)
- Contains: `fn main()` calling `dioxus::launch(App)`
- Depends on: `dioxus::prelude::*`
- Used by: Dioxus runtime

**Root Component (`App`):**
- Purpose: Top-level component — injects document head links, composes child components
- Location: `src/main.rs` (line 12–20)
- Contains: `document::Link` calls for favicon and CSS, `Hero {}` component invocation
- Depends on: `FAVICON`, `MAIN_CSS`, `TAILWIND_CSS` asset constants; `Hero` component
- Used by: `dioxus::launch()`

**Presentational Components:**
- Purpose: Pure UI rendering with no state
- Location: `src/main.rs` (line 22–38)
- Contains: `Hero` — renders header image and link list
- Depends on: `HEADER_SVG` asset constant
- Used by: `App`

**Static Assets:**
- Purpose: Visual resources referenced at compile time via `asset!()`
- Location: `assets/`
- Contains: `favicon.ico`, `header.svg`, `main.css`, `tailwind.css`
- Depends on: nothing
- Used by: `App`, `Hero` via `Asset` constants

## Data Flow

### Primary Render Path

1. Binary starts (`src/main.rs:8`) — `fn main()` calls `dioxus::launch(App)`
2. Dioxus runtime mounts `App` (`src/main.rs:12`)
3. `App` emits `document::Link` elements for favicon and both CSS files (`src/main.rs:15–16`)
4. `App` renders `Hero {}` child component (`src/main.rs:17`)
5. `Hero` renders `#hero` div containing `header.svg` image and `#links` anchor list (`src/main.rs:24–37`)

**State Management:**
- No reactive state (`use_signal`, `use_memo`, etc.) exists yet — the app is fully static.
- All displayed content is hardcoded in `rsx!` macros.

## Key Abstractions

**Asset Constants:**
- Purpose: Compile-time-resolved paths to static files, enabling Dioxus asset pipeline (hashing, bundling)
- Examples: `src/main.rs` lines 3–6
- Pattern: `const NAME: Asset = asset!("/assets/filename.ext");`

**Dioxus Components:**
- Purpose: Composable UI units — functions returning `Element`
- Examples: `App` and `Hero` in `src/main.rs`
- Pattern: `#[component]` attribute macro + `fn ComponentName() -> Element { rsx! { ... } }`

## Entry Points

**Web/Desktop/Mobile binary:**
- Location: `src/main.rs`
- Triggers: `cargo run` / `dx serve` / platform-specific build
- Responsibilities: Launch Dioxus runtime with root component

**Feature flags (Cargo):**
- `web` (default): compiles to WASM, served by `dx serve`
- `desktop`: native window via webview
- `mobile`: iOS/Android via webview

## Target Design (warp2ironhermes handoff)

The `warp2ironhermes/` directory contains a React/HTML prototype of the intended production UI. The Rust/Dioxus implementation is expected to recreate this design pixel-perfectly. Key design elements to implement:

**Application Shell (`WarpHermes` in `warp2ironhermes/project/app/app.jsx`):**
- Split layout: scrollable terminal stream (left/main) + agent side panel (right, 360px)
- Title bar with tab strip, macOS-style traffic lights (classic variant), `⌘K` shortcut display
- Bottom status bar with dot-pill rotation: mode · model · provider · token count · scanner
- Command palette overlay (triggered by `⌘K`) with slash commands and workflow items
- Personality preset system: `concise`, `technical`, `noir`, `hype`, `catgirl`, `default`

**Block-based terminal stream:**
- Block types: `is-cmd` (user command), `is-out` (output), `is-ai` (Hermes reply), `is-ok` (success), `is-err` (error)
- Each block has a 2px left accent stripe color-coded by type
- Hover reveals action buttons (copy ⎘, rerun ↻, share ↗)
- `CommandLine` sub-component renders `bin` / `arg` / `flag` token parts with distinct colors

**Input box:**
- Mode toggle: Shell (`❯` glyph) vs Agent (`✦` glyph), switched with `⌥+M`
- Auto-growing textarea, `Enter` submits, `Shift+Enter` for newline
- Focus ring using `var(--accent-primary)` color with glow

**Scanner component (`warp2ironhermes/project/app/shell.jsx`):**
- Knight-rider 10-cell animation, 100ms tick, triangle-wave bouncing
- Characters: `░` (off), `▒` (t2), `▓` (t1), `█` (lit)
- Activates on input submission, auto-deactivates after ~1400ms

**Mobile variant (`WarpHermesMobile`):**
- Compact title bar, no side panel (`data-agent="hidden"`)
- Bottom tab bar: shell (`❯`) / hermes (`✦`) / files (`▤`)
- `data-density="compact"` applied by default

**Design system tokens (`warp2ironhermes/project/ironhermes/colors_and_type.css`):**
- All colors are ANSI palette-derived (16 named colors)
- `--accent-primary`: cyan (`#4ec9b0`)
- `--accent-secondary`: magenta (`#c678dd`)
- `--success`: green (`#3fb950`), `--warn`: yellow (`#d29922`), `--danger`: red (`#f85149`)
- `--brand`: `#f0883e` (IronHermes wordmark)
- Font: `"Ioskeley Mono"` (woff2 files in `warp2ironhermes/project/ironhermes/fonts/`), fallback chain to `"Berkeley Mono"`, `ui-monospace`
- Everything uses monospace — `--font-body: var(--font-mono)`
- Zero border-radius on base elements (`--radius-0: 0px`); Warp blocks use `var(--w-radius-block): 6px`

**Theme/density/layout data attributes:**
- `data-theme`: `cyan` | `magenta` | `green` | `amber` — swaps `--accent-primary`
- `data-density`: `comfy` (default) | `compact`
- `data-block`: `framed` (default) | `flat` | `minimal`
- `data-agent`: `right` (default) | `bottom` | `hidden`

## Architectural Constraints

- **Threading:** Single-threaded WASM event loop in web mode. No `std::thread` usage.
- **Global state:** Only compile-time `Asset` constants at module level (`src/main.rs:3–6`). No runtime global state.
- **Circular imports:** Not applicable — single-file codebase.
- **No routing:** No `Router<Route>` or `Routable` enum defined yet. All content is on one screen.
- **No async:** No `use_resource` or `use_server_future` hooks used yet.
- **Feature gating:** Web/desktop/mobile targets are mutually exclusive via Cargo feature selection.

## Anti-Patterns

### Avoid `cx` / `Scope` / `use_state`

**What happens:** Pre-0.6 Dioxus APIs
**Why it's wrong:** Dioxus 0.7 (the version in use) removed `cx`, `Scope`, and `use_state` entirely
**Do this instead:** Use `use_signal(|| initial_value)` for local state; components are plain functions with `#[component]`

### Avoid module-level mutable statics for state

**What happens:** Using `static mut` or `lazy_static!` for runtime state
**Why it's wrong:** WASM is single-threaded but Rust's safety rules still apply; Dioxus signals are the correct reactive primitive
**Do this instead:** `use_signal` for local state, `use_context_provider` + `use_context` for shared state

## Error Handling

**Strategy:** Not yet defined — scaffold has no fallible operations.

**Patterns:**
- `use_resource` returns `Option<T>` — match on `None` to show loading state (pattern documented in `AGENTS.md`)
- Server functions return `Result<T, ServerFnError>` when fullstack feature is enabled

## Cross-Cutting Concerns

**Logging:** Not configured — no `tracing` or `log` dependency present.
**Validation:** Not applicable — no user input handling yet.
**Authentication:** Not applicable — no backend or auth configured.

---

*Architecture analysis: 2026-05-02*
