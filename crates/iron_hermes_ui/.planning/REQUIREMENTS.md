# Requirements: IronHermes

**Defined:** 2026-05-02
**Core Value:** The Dioxus implementation is visually indistinguishable from the React prototype — every block, panel, palette, scanner tick, and theme variant matches the source design when rendered side by side.

## v1 Requirements

Requirements for the v1 port. Each maps to roadmap phases.

### Project Hygiene

- [x] **HYG-01**: `Cargo.toml` pins `dioxus = { version = "=0.7.1", features = [] }` with the `[features]` indirection table providing `web = ["dioxus/web"]`, `desktop = ["dioxus/desktop"]`, and `mobile = ["dioxus/mobile"]` so the `dx` CLI can activate exactly one platform per build (canonical Dioxus 0.7 pattern, per RESEARCH.md Pattern 1)
- [x] **HYG-02**: `Cargo.lock` is generated and committed
- [x] **HYG-03**: `Dioxus.toml` configures `tailwind_input = "tailwind.css"` and `tailwind_output = "assets/tailwind.css"`; root-level `tailwind.css` is the single source of truth
- [x] **HYG-04**: `src/` is split into a module hierarchy (e.g., `src/main.rs`, `src/app.rs`, `src/components/`, `src/state/`, `src/platform/`) instead of a single `main.rs`
- [x] **HYG-05**: `.gitignore` excludes `**/.DS_Store` recursively and `warp2ironhermes-handoff.zip`

### Design System Port

- [ ] **DS-01**: 16 Ioskeley Mono `.woff2` files are copied into `assets/fonts/` and referenced by `@font-face` declarations
- [ ] **DS-02**: `assets/design-tokens.css` mirrors `warp2ironhermes/project/ironhermes/colors_and_type.css` — ANSI palette CSS variables, font stacks, radii, brand color
- [ ] **DS-03**: `assets/warp-ih.css` mirrors `warp2ironhermes/project/styles/warp-ih.css` — Warp shell layout, blocks, input, palette, side panel
- [ ] **DS-04**: IronHermes brand assets (`wordmark.svg`, `ih-shield.png`) replace the scaffold `header.svg`; default `assets/main.css` is updated to load brand-correct fonts and background

### Desktop/Web Shell (`WarpHermes`)

- [ ] **SHELL-01**: Title bar renders macOS traffic lights, tab strip, and `⌘K` shortcut display
- [ ] **SHELL-02**: Block stream renders five block types (`is-cmd`, `is-out`, `is-ai`, `is-ok`, `is-err`) each with the correct color-coded 2px left accent stripe
- [ ] **SHELL-03**: Hovering a block reveals copy (⎘), rerun (↻), and share (↗) action buttons
- [ ] **SHELL-04**: `CommandLine` sub-component renders `bin` / `arg` / `flag` token parts with distinct colors
- [ ] **SHELL-05**: Tool-call block visualizes a tool invocation (name, args summary, status)
- [ ] **SHELL-06**: Input box has mode glyph (`❯` Shell / `✦` Agent), auto-growing textarea, and accent-color focus ring with glow
- [ ] **SHELL-07**: Agent side panel renders at the right (360px) when `data-agent="right"`
- [ ] **SHELL-08**: Status bar renders rotating dot-pills showing mode · model · provider · token count · scanner
- [ ] **SHELL-09**: Scanner runs the knight-rider animation (10 cells, 100 ms tick, triangle-wave bouncing through `░ ▒ ▓ █` glyphs) using CSS `@keyframes` rather than signal-driven re-renders
- [ ] **SHELL-10**: Command palette overlay renders slash commands and workflow items in a vertical list

### Mobile Shell (`WarpHermesMobile`)

- [ ] **MOB-01**: Compact title bar renders without traffic lights or tab strip
- [ ] **MOB-02**: Bottom tab bar renders shell (`❯`), hermes (`✦`), and files (`▤`) tabs
- [ ] **MOB-03**: Mobile shell applies `data-agent="hidden"` and `data-density="compact"` by default
- [ ] **MOB-04**: Mobile shell reuses block stream and input components with mobile-tuned spacing

### Keyboard & Interactions

- [ ] **KBD-01**: `⌥M` toggles input mode between Shell and Agent
- [ ] **KBD-02**: `⌘K` opens the command palette; `Esc` closes it
- [x] **KBD-03**: `↑` / `↓` navigates palette items; `Enter` selects the active item
- [x] **KBD-04**: `Enter` submits the input box; `Shift+Enter` inserts a newline
- [x] **KBD-05**: Block hover affordances fire copy / rerun / share handlers when clicked
- [ ] **KBD-06**: Switching the personality preset immediately updates the active mock-reply set

### Theming & Tweaks

- [ ] **THEME-01**: `data-theme` switches accent color across `cyan` / `magenta` / `green` / `amber`
- [ ] **THEME-02**: `data-density` switches spacing across `comfy` / `compact`
- [ ] **THEME-03**: `data-block` switches block style across `framed` / `flat` / `minimal`
- [ ] **THEME-04**: `data-agent` switches agent layout across `right` / `bottom` / `hidden`
- [ ] **THEME-05**: `TweaksPanel` UI flips all four data-attributes live

### Mocked Data Layer

- [x] **MOCK-01**: Six personality presets (`concise`, `technical`, `noir`, `hype`, `catgirl`, `default`) each define their own scripted mock-reply table
- [x] **MOCK-02**: `runShell(cmd)` mock emits an `is-cmd` block, then delayed `is-out` / `is-ok` / `is-err` blocks matching the prototype's 600 ms / 400 ms / 1400 ms timings
- [x] **MOCK-03**: `runAgent(prompt)` mock emits agent-thinking → tool-call → `is-ai` reply pulled from the active personality's table
- [ ] **MOCK-04**: Token budget counter increments per submission and renders in the status bar

## v2 Requirements

Deferred to a future milestone. Tracked but not in v1 scope.

### Real Backends

- **REAL-01**: Real shell command execution via a server-function boundary (`dioxus/server` feature)
- **REAL-02**: Real LLM API calls (OpenRouter / Anthropic / OpenAI) with streaming SSE responses
- **REAL-03**: Live token tracking from API response metadata
- **REAL-04**: API-key configuration UI with server-side key storage

### Persistence & Identity

- **PERS-01**: Settings (theme/density/block/agent/personality) persist across reloads
- **PERS-02**: Block stream history persists per session
- **AUTH-01**: User authentication and identity layer

### Testing

- **TEST-01**: Component tests via `dioxus-testing` for every shell component
- **TEST-02**: Visual regression tests against the prototype HTML
- **TEST-03**: Keyboard interaction tests for all shortcuts

## Out of Scope

Explicitly excluded. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| Real shell command execution | v1 is a UI port; real exec is a separate backend concern |
| Real LLM API calls | Same — prototype fakes them and v1 mirrors that |
| Authentication / identity | No UI for it in the prototype; deferred to v2 |
| Settings / history persistence | Reload-fresh is acceptable for the port; persistence is v2 |
| Server-side rendering / hydration | v1 is client-only WASM; `dioxus/fullstack` not needed |
| Tailwind utility-class refactor | CSS is ported as-is; Tailwind stays available but unused |
| Test suite (unit / integration / visual regression) | Acknowledged gap; revisit before adding real backend |
| Renaming `Warp × IronHermes.html` | File stays untouched in `warp2ironhermes/` (reference-only directory) |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| HYG-01 | Phase 1 | Complete (Plan 01-01, commit 7da3841 wave) |
| HYG-02 | Phase 1 | Complete (Plan 01-03, commit 40fc8b8) |
| HYG-03 | Phase 1 | Complete (Plan 01-01, commit 7da3841 wave) |
| HYG-04 | Phase 1 | Complete (Plan 01-02, commits ac86e11/c0b0ad2) |
| HYG-05 | Phase 1 | Complete (Plan 01-01, commit 24a4e5e) |
| DS-01 | Phase 2 | Pending |
| DS-02 | Phase 2 | Pending |
| DS-03 | Phase 2 | Pending |
| DS-04 | Phase 2 | Pending |
| SHELL-01 | Phase 3 | Pending |
| SHELL-02 | Phase 3 | Pending |
| SHELL-03 | Phase 3 | Pending |
| SHELL-04 | Phase 3 | Pending |
| SHELL-05 | Phase 3 | Pending |
| SHELL-06 | Phase 3 | Pending |
| SHELL-07 | Phase 3 | Pending |
| SHELL-08 | Phase 3 | Pending |
| SHELL-09 | Phase 3 | Pending |
| SHELL-10 | Phase 3 | Pending |
| MOB-01 | Phase 6 | Pending |
| MOB-02 | Phase 6 | Pending |
| MOB-03 | Phase 6 | Pending |
| MOB-04 | Phase 6 | Pending |
| KBD-01 | Phase 4 | Pending |
| KBD-02 | Phase 4 | Pending |
| KBD-03 | Phase 4 | Complete |
| KBD-04 | Phase 4 | Complete |
| KBD-05 | Phase 4 | Complete |
| KBD-06 | Phase 4 | Pending |
| THEME-01 | Phase 5 | Pending |
| THEME-02 | Phase 5 | Pending |
| THEME-03 | Phase 5 | Pending |
| THEME-04 | Phase 5 | Pending |
| THEME-05 | Phase 5 | Pending |
| MOCK-01 | Phase 4 | Complete |
| MOCK-02 | Phase 4 | Complete |
| MOCK-03 | Phase 4 | Complete |
| MOCK-04 | Phase 4 | Pending |

**Coverage:**
- v1 requirements: 38 total
- Mapped to phases: 38
- Unmapped: 0

---
*Requirements defined: 2026-05-02*
*Last updated: 2026-05-02 after roadmap creation (traceability populated)*
