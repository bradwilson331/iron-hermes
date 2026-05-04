# IronHermes

## What This Is

A pixel-perfect Dioxus 0.7 port of the Warp × IronHermes design prototype — a terminal-style application shell with a block-based command stream, agent side panel, command palette, and a comprehensive ANSI-derived design system. The Rust/Dioxus implementation targets web (WASM), desktop (webview), and mobile from one codebase, recreating the React/JSX prototype in `warp2ironhermes/` exactly.

## Core Value

The Dioxus implementation is visually indistinguishable from the React prototype — every block, panel, palette, scanner tick, and theme variant matches the source design when rendered side by side.

## Requirements

### Validated

<!-- Shipped and confirmed valuable. -->

- ✓ Dioxus 0.7 scaffold initialized — existing
- ✓ Multi-platform feature flags wired (web / desktop / mobile) — existing, needs feature-list fix
- ✓ Tailwind v4 input present at project root — existing
- ✓ Clippy safety rules for signal borrows across `.await` — existing
- ✓ Codebase mapped (`.planning/codebase/`) — existing
- ✓ Dioxus 0.7 API reference in `AGENTS.md` — existing

### Active

<!-- Current scope — v1 of the port. Hypotheses until shipped. -->

**Project hygiene:**
- [ ] Pin `dioxus` dependency to `=0.7.1` and add explicit `web`/`desktop`/`mobile` features to the dependency line
- [ ] Generate and commit `Cargo.lock`
- [ ] Wire `tailwind_input` / `tailwind_output` in `Dioxus.toml`; remove duplicate `tailwind.css` confusion
- [ ] Establish module structure under `src/` (replace single `main.rs` with components/state/platform layout)
- [ ] Add `.DS_Store` recursive ignore and exclude `warp2ironhermes-handoff.zip` from git

**Design system port:**
- [ ] Copy Ioskeley Mono woff2 family (16 variants) from prototype to `assets/fonts/`
- [ ] Port `colors_and_type.css` design tokens to `assets/design-tokens.css` (ANSI palette, monospace fonts, radii, spacing)
- [ ] Port `warp-ih.css` Warp shell styles to `assets/warp-ih.css` (blocks, input, palette, side panel)
- [ ] Replace placeholder `assets/main.css` and `assets/header.svg` with IronHermes brand assets (`wordmark.svg`, `ih-shield.png`)

**Desktop/web shell (`WarpHermes`):**
- [ ] Title bar with macOS traffic lights, tab strip, and `⌘K` shortcut display
- [ ] Block-based command stream with `is-cmd` / `is-out` / `is-ai` / `is-ok` / `is-err` block types and color-coded 2px accent stripes
- [ ] Per-block hover actions (copy ⎘, rerun ↻, share ↗)
- [ ] `CommandLine` sub-component rendering `bin` / `arg` / `flag` token parts with distinct colors
- [ ] Tool-call visualization block
- [ ] Auto-growing input box with mode glyph (`❯` Shell / `✦` Agent), focus ring with accent glow
- [ ] Agent side panel (right, 360px, `data-agent="right"` default)
- [ ] Status bar with rotating dot-pills: mode · model · provider · token count · scanner
- [ ] Knight-rider scanner animation: 10 cells, 100 ms tick, triangle-wave bouncing through `░ ▒ ▓ █` glyphs (CSS `@keyframes`, not signal-driven)
- [ ] Command palette overlay with slash commands and workflow items, list keyboard navigation

**Mobile shell (`WarpHermesMobile`):**
- [ ] Compact title bar (no traffic lights / tab strip)
- [ ] Bottom tab bar: shell (`❯`) · hermes (`✦`) · files (`▤`)
- [ ] `data-agent="hidden"`, `data-density="compact"` defaults
- [ ] Same block stream and input components, mobile-tuned spacing

**Interactions:**
- [ ] Mode toggle Shell ↔ Agent (`⌥M`)
- [ ] Command palette open/close (`⌘K` / `Esc`)
- [ ] Palette item navigation (`↑` / `↓` / `Enter`)
- [ ] Input submit (`Enter`) / newline (`Shift+Enter`)
- [ ] Block hover affordances trigger copy/rerun/share handlers
- [ ] Switching personality preset updates the mock-reply set live

**Theming and tweaks:**
- [ ] Theme switch via `data-theme`: `cyan` / `magenta` / `green` / `amber` (swaps `--accent-primary`)
- [ ] Density switch via `data-density`: `comfy` / `compact`
- [ ] Block style switch via `data-block`: `framed` / `flat` / `minimal`
- [ ] Agent layout switch via `data-agent`: `right` / `bottom` / `hidden`
- [ ] `TweaksPanel` component flips all of the above live

**Mocked data layer:**
- [ ] Personality presets — `concise` / `technical` / `noir` / `hype` / `catgirl` / `default` — each with its own scripted mock-reply table
- [ ] `runShell(cmd)` mock — produces `is-cmd` block then delayed `is-out` / `is-ok` / `is-err` blocks (replicating prototype 600 ms / 400 ms / 1400 ms timings)
- [ ] `runAgent(prompt)` mock — emits agent-thinking → tool-call → `is-ai` reply pulled from the active personality's table
- [ ] Token budget display reads from a mock counter that increments per submission

**Verification:**
- [ ] Side-by-side visual review against the prototype HTML at every variant combination

### Out of Scope

<!-- Explicit boundaries with reasoning. -->

- Real shell command execution — v1 is a UI port; real exec lives behind a future server-function boundary
- Real LLM API calls (OpenRouter / Anthropic / OpenAI) — same reason; the prototype already fakes these and v1 mirrors that
- Authentication and identity — no user accounts in v1; the design has no UI for them
- Persistence of settings / history — every reload starts fresh; localStorage / file-backed persistence comes later
- Server-side rendering / hydration — v1 is client-only WASM; `dioxus/fullstack` is not needed yet
- Test suite — no unit, integration, or visual-regression tests in this milestone (acknowledged gap; revisit before adding real backend in a later milestone)
- Tailwind utility-class refactor — CSS is ported as-is from the prototype; Tailwind stays available but unused
- Renaming the unicode-special design file (`Warp × IronHermes.html`) — it stays untouched in `warp2ironhermes/` since that directory is reference-only

## Context

**Source material.** `warp2ironhermes/` is a complete handoff bundle exported from claude.ai/design — React/JSX prototype plus all CSS, fonts, and brand assets. Its `README.md` instructs the implementing agent to read every prototype file and recreate the design pixel-perfectly in whatever target tech fits. For this project, that target is the existing `iron_hermes_ui` Rust/Dioxus codebase.

**Current code state.** `src/main.rs` is the default `dx new` scaffold — a `Hero` component with placeholder Dioxus documentation links. Nothing about the IronHermes product has been implemented yet. Single-file architecture; will be split into a module hierarchy as part of v1.

**Codebase audit findings (relevant).** `assets/tailwind.css` is empty, `Dioxus.toml` is missing `tailwind_input`/`tailwind_output`, `dioxus` is declared with `features = []` (relying on package-level `default = ["web"]` indirection that breaks under `--no-default-features` and silently drops desktop/mobile), `Cargo.lock` isn't committed, and `warp2ironhermes-handoff.zip` (7.8 MB) is tracked alongside the already-extracted `warp2ironhermes/` directory. All addressed in the hygiene block of Active.

**Design tokens.** All colors derive from a 16-color ANSI palette. Brand accent is `--brand: #f0883e`. `--accent-primary` defaults to cyan `#4ec9b0`. Font is Ioskeley Mono (custom Berkeley Mono look-alike, 16 woff2 weights/widths). Body and mono are unified — `--font-body: var(--font-mono)`. Base radius is `0px`; Warp blocks use `6px`. Everything monospace.

**Pre-existing constraints already encoded in repo.** `clippy.toml` enforces a critical Dioxus 0.7 rule: signal `read()` / `write()` borrows must not be held across `.await` points. Any async code in the implementation must respect this.

## Constraints

- **Tech stack**: Rust 2021 + Dioxus 0.7.1 — `cx`, `Scope`, and `use_state` are forbidden (Dioxus 0.6 APIs, removed in 0.7). Use `use_signal`, `use_memo`, `use_resource`, `use_context_provider` / `use_context`. Component functions are `PascalCase` and annotated `#[component]`.
- **Threading**: WASM single-threaded; signal borrows must not span `.await` (clippy-enforced).
- **Multi-platform**: One codebase, three Cargo features (`web` / `desktop` / `mobile`). Platform-specific behavior gated via `#[cfg(feature = "...")]` or per-platform modules under `src/platform/`.
- **No external services**: Zero API keys, zero network calls in v1. Mocks only.
- **Design fidelity**: Pixel-perfect to the prototype. Visual drift is the primary failure mode.
- **CSS strategy**: Port `warp-ih.css` and `colors_and_type.css` as-is — no Tailwind conversion in v1.
- **Reference directory is read-only**: `warp2ironhermes/` is consulted, not compiled. Do not import from it. Do not include it in any build asset path.

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| v1 scope is visuals + interactions with mocked data | Real shell / LLM backends are a separate concern; UI port is the bottleneck | — Pending |
| Ship web + desktop + mobile together | Prototype defines all three frames; one Dioxus codebase covers all | — Pending |
| Port prototype CSS as-is (no Tailwind conversion) | Fastest path to pixel-perfect; Tailwind translation risks visual drift | — Pending |
| Separate `DesktopShell` and `MobileShell` components | Mirrors prototype's structural split; simpler than one adaptive shell with conditional layout | — Pending |
| Ship `TweaksPanel` and full data-attribute switching in v1 | Showcases the design system end-to-end; cheap to implement once tokens are wired | — Pending |
| Personality presets switchable with distinct mock-reply tables | Demonstrates the design system; trivial mock-data work | — Pending |
| All keyboard shortcuts (`⌘K`, `⌥M`, `Enter`/`Shift+Enter`, `↑`/`↓`, `Esc`) wired in v1 | Design depends on them; partial coverage feels broken | — Pending |
| Pin `dioxus = "=0.7.1"` and commit `Cargo.lock` early | Reproducibility; Dioxus 0.7 is in active development with breaking patches possible | — Pending |
| Scanner animation via CSS `@keyframes` rather than signal-driven re-renders | Avoids 10 Hz VDOM diffs; offloads to GPU compositor | — Pending |
| Brand name is "IronHermes" (crate stays `iron_hermes_ui`) | Matches design wordmark; crate name is a Rust identifier, separate concern | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-05-02 after initialization*
