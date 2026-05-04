# Codebase Structure

**Analysis Date:** 2026-05-02

## Directory Layout

```
iron_hermes_ui/
├── src/
│   └── main.rs             # Entire Rust source: entry point + all components
├── assets/
│   ├── favicon.ico         # Browser tab icon
│   ├── header.svg          # Dioxus default hero SVG (to be replaced)
│   ├── main.css            # App-wide styles (dark bg, #hero, #links)
│   └── tailwind.css        # Tailwind CSS (currently empty — 1 line)
├── warp2ironhermes/        # Design handoff bundle — READ ONLY, not compiled
│   ├── README.md           # Agent instructions for implementing the design
│   └── project/
│       ├── app/
│       │   ├── app.jsx     # Main WarpHermes shell prototype (state + layout)
│       │   ├── shell.jsx   # Presentational components (Scanner, StatusBar, etc.)
│       │   └── frames.jsx  # Device frame wrappers + mobile variant
│       ├── ironhermes/
│       │   ├── colors_and_type.css   # Design system: ANSI color tokens + Ioskeley Mono
│       │   └── fonts/      # Ioskeley Mono woff2 files (16 variants)
│       ├── styles/
│       │   └── warp-ih.css # Warp-style shell CSS (blocks, input, palette, side panel)
│       ├── design-canvas.jsx         # Figma-like canvas wrapper for the prototype
│       ├── browser-window.jsx        # Browser chrome frame
│       ├── macos-window.jsx          # macOS window frame
│       ├── ios-frame.jsx             # iOS device frame
│       ├── tweaks-panel.jsx          # Live theme/density tweak controls
│       └── Warp × IronHermes.html   # Primary design HTML entry point
├── .planning/
│   └── codebase/           # GSD codebase map documents
├── .omc/                   # oh-my-claudecode agent state
├── Cargo.toml              # Rust package manifest + feature flags
├── Dioxus.toml             # Dioxus CLI config (HTML title, asset includes)
├── tailwind.css            # Tailwind source input file (project root)
├── clippy.toml             # Rust clippy linting config
└── AGENTS.md               # Dioxus 0.7 quick-reference for coding agents
```

## Directory Purposes

**`src/`:**
- Purpose: All Rust source code
- Contains: Single `main.rs` with entry point and all components
- Key files: `src/main.rs`

**`assets/`:**
- Purpose: Static files served by Dioxus asset pipeline
- Contains: CSS stylesheets, favicon, SVG images
- All files referenced via `asset!("/assets/filename")` macro in Rust code
- Key files: `assets/main.css` (current styles), `assets/tailwind.css` (empty placeholder)

**`warp2ironhermes/`:**
- Purpose: Design handoff bundle — HTML/CSS/JS prototypes defining the target UI
- Contains: React components describing the IronHermes terminal UI to be implemented
- **Not compiled into the Rust build** — reference material only
- Key files: `warp2ironhermes/project/app/app.jsx` (primary design), `warp2ironhermes/project/styles/warp-ih.css` (all UI styles), `warp2ironhermes/project/ironhermes/colors_and_type.css` (design tokens)

**`warp2ironhermes/project/ironhermes/fonts/`:**
- Purpose: Ioskeley Mono webfont family (16 woff2 files covering weights 100–900 + condensed widths)
- These fonts need to be copied to `assets/fonts/` when implementing the design
- Generated: No — bundled from design tool export
- Committed: Yes

**`.planning/codebase/`:**
- Purpose: GSD codebase map documents consumed by `/gsd-plan-phase` and `/gsd-execute-phase`
- Contains: ARCHITECTURE.md, STRUCTURE.md (this file)
- Generated: Yes — by `/gsd-map-codebase`
- Committed: Yes

## Key File Locations

**Entry Points:**
- `src/main.rs`: Binary entry point (`fn main()`), root `App` component, `Hero` component

**Configuration:**
- `Cargo.toml`: Package metadata, `dioxus = { version = "0.7.1" }`, feature flags (`web`, `desktop`, `mobile`)
- `Dioxus.toml`: Dioxus CLI settings — HTML title (`"iron_hermes_ui"`), extra CSS/JS includes
- `clippy.toml`: Rust linting rules
- `AGENTS.md`: Dioxus 0.7 API reference (key: `use_signal`, `#[component]`, `rsx!`, `asset!`, routing, fullstack)

**Core Logic:**
- `src/main.rs`: All application logic — currently scaffold only

**Styles:**
- `assets/main.css`: Active app styles (dark `#0f1116` background, `#hero` layout, `#links` button styles)
- `assets/tailwind.css`: Tailwind CSS (currently empty)
- `warp2ironhermes/project/styles/warp-ih.css`: Target CSS to port to `assets/` during implementation
- `warp2ironhermes/project/ironhermes/colors_and_type.css`: Design token source for CSS variables

**Design Reference:**
- `warp2ironhermes/project/app/app.jsx`: Complete WarpHermes shell with all state and sub-components
- `warp2ironhermes/project/app/shell.jsx`: Presentational primitives (Scanner, StatusBar, Sigil, TitleBar, Block, CommandLine, ToolCall, InputBox, AgentPanel, CommandPalette)
- `warp2ironhermes/project/app/frames.jsx`: Device frames + mobile variant (`WarpHermesMobile`)

## Naming Conventions

**Files:**
- Rust source: `snake_case.rs` (e.g., `main.rs`)
- Assets: `kebab-case` or `snake_case` (e.g., `main.css`, `header.svg`, `favicon.ico`)
- Future component modules: `snake_case.rs` matching the primary component name (e.g., `hero.rs`, `status_bar.rs`)

**Directories:**
- Rust modules: `snake_case/`
- Asset subdirectories: `snake_case/` (e.g., `assets/fonts/`)

**Dioxus components:**
- Component function names: `PascalCase` (required by Dioxus — must start with capital letter or contain underscore)
- Component files (when split out): `snake_case.rs`

**CSS classes (from design reference):**
- All Warp shell classes prefixed with `wh-` (e.g., `wh-app`, `wh-block`, `wh-status`)
- Block type modifiers: `is-cmd`, `is-out`, `is-ai`, `is-ok`, `is-err`
- State modifiers: `is-active`, `is-focus`, `is-new`

## Where to Add New Code

**New Dioxus Component:**
- For a small/coupled component: add to `src/main.rs` directly
- For a standalone component: create `src/components/component_name.rs`, declare `mod components;` + `mod component_name;` in `src/main.rs`
- Test file: `src/components/component_name_test.rs` or inline `#[cfg(test)]` mod

**New CSS styles:**
- App-wide styles: `assets/main.css`
- Warp shell styles (when porting): a new `assets/warp-ih.css` mirroring `warp2ironhermes/project/styles/warp-ih.css`
- Design tokens: a new `assets/design-tokens.css` mirroring `warp2ironhermes/project/ironhermes/colors_and_type.css`
- Link new CSS from `App` component using `document::Link { rel: "stylesheet", href: asset!("/assets/filename.css") }`

**New Static Assets:**
- Place in `assets/`
- Reference with `const NAME: Asset = asset!("/assets/filename.ext");` at top of `src/main.rs` (or relevant module)

**Webfonts (Ioskeley Mono):**
- Copy woff2 files from `warp2ironhermes/project/ironhermes/fonts/` → `assets/fonts/`
- Add `@font-face` declarations to a new `assets/fonts.css`
- Link `fonts.css` from `App`

**Routing (when adding multiple pages):**
- Define a `Route` enum deriving `Routable` in `src/main.rs` or a new `src/routes.rs`
- Requires adding `router` feature: `dioxus = { version = "0.7.1", features = ["router"] }`
- Use `Router::<Route> {}` inside `App` and `Outlet<Route> {}` inside layout components

**State (reactive):**
- Local component state: `let mut value = use_signal(|| initial);`
- Shared state: `use_context_provider(|| signal)` in `App`, `use_context::<Signal<T>>()` in consumers
- Async data: `use_resource(move || async move { ... })`

## Special Directories

**`warp2ironhermes/`:**
- Purpose: Design prototype bundle exported from claude.ai/design — defines target UI
- Generated: Yes — exported from AI design tool
- Committed: Yes — source of truth for implementation spec

**`target/`:**
- Purpose: Rust build artifacts
- Generated: Yes — by `cargo build` / `dx build`
- Committed: No (in .gitignore)

**`.planning/`:**
- Purpose: GSD planning documents (phases, codebase maps)
- Generated: Yes — by GSD commands
- Committed: Yes

**`.omc/`:**
- Purpose: oh-my-claudecode agent orchestration state
- Generated: Yes
- Committed: Depends on project config

---

*Structure analysis: 2026-05-02*
