# Phase 2: Design System - Context

**Gathered:** 2026-05-02
**Status:** Ready for planning

<domain>
## Phase Boundary

Install the IronHermes visual identity into the Dioxus asset pipeline so all downstream phases render against the correct foundation. Concretely: copy 16 Ioskeley Mono `.woff2` files into `assets/fonts/`, port `colors_and_type.css` (ANSI palette, type scale, brand color, scanner constants) verbatim into `assets/design-tokens.css`, port `warp-ih.css` (Warp shell layout — `.wh-app`, `.wh-titlebar`, `.wh-block` family, status bar, scanner cells, palette overlay, density/theme/block data-attribute overrides) verbatim into `assets/warp-ih.css`, and replace the Dioxus scaffold `header.svg` with the IronHermes brand assets (`wordmark.svg` + `ih-shield.png`) loaded into a minimal Hero stub.

What this phase does NOT do: build the actual shell components, wire interactions, animate the scanner, implement the TweaksPanel, or render any block stream. Those live in Phases 3, 4, and 5. Phase 2 is the design foundation only — the runtime app after this phase is a near-empty page that proves the tokens, fonts, and brand assets load correctly.

</domain>

<decisions>
## Implementation Decisions

### CSS File Topology
- **D-01:** Inject three separate `<link>` tags from `src/app.rs` in cascade order: `main.css` → `design-tokens.css` → `warp-ih.css`. Mirrors the prototype HTML which uses two `<link>` tags (tokens then layout). Three parallel HTTP requests, debuggable in DevTools, easy to swap any single layer when the prototype updates.
- **D-02:** No `@import` chains and no concatenation. The prototype's separation between `colors_and_type.css` (design tokens) and `warp-ih.css` (Warp-shell-specific layout) is preserved as the source-of-truth boundary. Future re-syncs from the prototype copy single files, not merged blobs.

### Phase 2 Visible Output
- **D-03:** `src/components/hero.rs` is reduced to a minimal stub that renders the IronHermes wordmark (`wordmark.svg`) and shield (`ih-shield.png`) centered on the `--bg` background using the `--font-mono` family. No link list, no Dioxus tutorial content. Phase 3 will replace this Hero entirely with the real `WarpHermes` desktop shell.
- **D-04:** No "design system preview" page is built. Token verification per success criteria #1–#3 happens via DevTools (the SCs explicitly specify DevTools). Avoids ~50–100 LOC of throwaway code that Phase 3 would discard anyway.

### Font Loading Scope
- **D-05:** All 16 Ioskeley Mono `.woff2` files are copied into `assets/fonts/` and all 16 corresponding `@font-face` declarations are included in `assets/design-tokens.css` verbatim from `colors_and_type.css`. Browsers lazy-fetch per used weight — declared-but-unused weights are NOT downloaded — so the perceived first-load size cost is zero. Keeps the design system file byte-identical to the prototype source of truth and unblocks any future weight need (italic, condensed-bold, extra-light) without a CSS edit.
- **D-06:** Font URLs in `design-tokens.css` use the prototype's relative form: `src: url("fonts/IoskeleyMono-Regular.woff2")`. With the CSS file at `/assets/design-tokens.css` and fonts at `/assets/fonts/*.woff2`, browser relative-URL resolution serves them correctly. No Rust `asset!()` macro is needed inside the CSS — fonts are referenced as static asset paths only.

### Tailwind Disposition
- **D-07:** The Tailwind `<link>` in `src/app.rs` (and the `TAILWIND_CSS` asset constant) stay as-is. The `dx serve` Tailwind compilation pipeline wired in Phase 1 keeps producing `assets/tailwind.css`. PROJECT.md's "Tailwind stays available but unused" decision holds. The compiled file is small (~11KB), browser-cacheable, and parallel-loaded with the three ported CSS files.

### Claude's Discretion
- **CSS load order placement:** `main.css` first establishes brand-correct fonts/background per DS-04, `design-tokens.css` second defines all `:root` custom properties, `warp-ih.css` third adds Warp-shell-specific tokens (`--w-bg-0..4`, `--w-stripe-*`) and the `.wh-*` layout classes that consume them. Last-wins cascade keeps the warp shell's deeper-than-IH surfaces intact.
- **Brand asset placement in stub Hero:** small fixed-height (~32px) wordmark, slightly larger shield (~96px) below it, both centered, ~24px gap. Final placement in the real `wh-titlebar` is Phase 3 territory; Phase 2 just proves they load and render.
- **Existing `assets/main.css` content:** the current 39 lines of placeholder Hero CSS (Segoe UI, white border buttons, etc.) are deleted entirely and replaced with brand-correct base styles per DS-04: dark `--bg` body background, `--font-mono` body font, minimal centering rules for the stub Hero. The Hero `#hero` / `#links` selectors no longer apply because the stub has different markup.
- **Theme data-attribute overrides** (`[data-theme="cyan"]`, `[data-theme="magenta"]` etc. at `warp-ih.css` lines ~52–55) are ported now even though the runtime switching is Phase 5. Pure CSS rules; no harm carrying them.
- **Scanner asset** (`scanner.svg` at `warp2ironhermes/project/ironhermes/assets/scanner.svg`) — discovered during scout, not part of Phase 2 success criteria; copy it into `assets/` opportunistically to unblock Phase 3, or defer to Phase 3. Planner's call.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Project-level governance
- `.planning/PROJECT.md` — core value (pixel-perfect to prototype), Constraints section (CSS ported as-is, no Tailwind conversion, multi-platform features, brand asset names, base radius zero, font is Ioskeley Mono with Berkeley Mono fallback)
- `.planning/REQUIREMENTS.md` §"Design System Port" — DS-01 through DS-04 with full acceptance criteria
- `.planning/ROADMAP.md` §"Phase 2: Design System" — goal statement and 4 success criteria
- `CLAUDE.md` — Dioxus 0.7 conventions (no `cx`/`Scope`/`use_state`; signal borrows must not span `.await`)
- `AGENTS.md` — Dioxus 0.7 API reference for `#[component]`, `document::Link`, `asset!()` macro semantics

### Prototype source of truth (READ-ONLY — never compile from here)
- `warp2ironhermes/project/ironhermes/colors_and_type.css` — 241 lines; the verbatim source for `assets/design-tokens.css`. Contains 16 `@font-face` blocks, ANSI palette, surfaces, text colors, semantic accents, pill rotation, type scale, spacing, radii, scanner constants, z-layering, base body/h1/h2/p/code/pre/hr rules, utility classes, pill classes, status glyphs, layout primitives.
- `warp2ironhermes/project/styles/warp-ih.css` — 507 lines; the verbatim source for `assets/warp-ih.css`. Contains Warp surface scale (`--w-bg-0..4`), block stripes per type, density/theme/block data-attribute overrides, `.wh-app`, `.wh-titlebar` and downstream `.wh-*` layout classes.
- `warp2ironhermes/project/Warp × IronHermes.html` — reference HTML; `<head>` shows the canonical CSS load order (tokens then warp-ih).
- `warp2ironhermes/project/ironhermes/fonts/IoskeleyMono-*.woff2` — 16 files; verbatim copy targets for `assets/fonts/`. File list confirmed: `Black, Bold, BoldItalic, Condensed, CondensedBold, CondensedMedium, ExtraBold, ExtraLight, Italic, Light, Medium, Regular, SemiBold, SemiCondensed, SemiLight, Thin`.
- `warp2ironhermes/project/ironhermes/assets/wordmark.svg` — IronHermes wordmark, replaces scaffold header.
- `warp2ironhermes/project/ironhermes/assets/ih-shield.png` — IronHermes shield, paired with wordmark.
- `warp2ironhermes/project/ironhermes/assets/scanner.svg` — bonus discovery, Phase 3/4 use (status bar scanner).

### Phase 1 deliverables (consumed by Phase 2)
- `Dioxus.toml` — `[application] tailwind_input/tailwind_output` already wired; `dx serve` regenerates `assets/tailwind.css` automatically. No further config needed.
- `src/app.rs` — current home of `FAVICON`, `MAIN_CSS`, `TAILWIND_CSS` asset constants and the three `document::Link` calls. Phase 2 adds two more `Asset` consts (`DESIGN_TOKENS_CSS`, `WARP_IH_CSS`) and two more `document::Link` calls in cascade order.
- `src/components/hero.rs` — current home of `HEADER_SVG`. Phase 2 swaps `HEADER_SVG` for `WORDMARK_SVG` + `IH_SHIELD_PNG`, deletes the link list, simplifies the `rsx!` to the brand stub layout.
- `src/main.rs` — slim entry; no changes needed for Phase 2 (module declarations already in place).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `src/app.rs` already establishes the asset-const + `document::Link` pattern with three CSS files. Phase 2 follows that pattern for two more CSS files and renames `HEADER_SVG` to brand assets in `hero.rs`.
- `src/components/hero.rs` is the natural mounting point for the brand stub. Strip its current contents (header img + link list) and replace with the wordmark/shield centered layout.
- `src/state.rs` and `src/platform/mod.rs` remain placeholder stubs — Phase 2 does not touch them.

### Established Patterns
- Asset constants colocated with their consuming component (Phase 1 module-split decision). `WORDMARK_SVG` and `IH_SHIELD_PNG` go in `src/components/hero.rs`. `DESIGN_TOKENS_CSS` and `WARP_IH_CSS` go in `src/app.rs` (where the other CSS asset constants live and where `document::Link`s are emitted).
- All asset paths use the leading-slash form: `asset!("/assets/...")`. Maintain this convention for the new constants.
- Component functions are `PascalCase` and `#[component]`-annotated; rsx! blocks are 4-space-indented. Hero's signature stays `pub fn Hero() -> Element`.

### Integration Points
- `src/app.rs#App` renders four `document::Link` tags currently (favicon, main.css, tailwind.css). Phase 2 grows this to six (favicon, main.css, design-tokens.css, warp-ih.css, tailwind.css). Cascade order matters: tokens before warp-ih, both before tailwind (Tailwind preflight should not override anything).
- `src/components/hero.rs` is the only component currently rendering UI. Phase 2's stub swap is fully contained here; no other source files are touched in the component layer.
- Asset pipeline: Dioxus 0.7's `dx serve` reads `Dioxus.toml`, runs the Tailwind watcher, hashes assets per `asset!()` invocation. Static font files referenced from CSS via relative `url(...)` are served from `/assets/fonts/` without needing a Rust `Asset` declaration. This is verified by the prototype's working build pattern.

</code_context>

<specifics>
## Specific Ideas

- **Verbatim port, not transcription.** `colors_and_type.css` and `warp-ih.css` should land byte-identical (modulo a leading comment noting source path and copy date). Any deviation drifts from the source-of-truth and risks visual fidelity per the project's primary failure mode.
- **Brand asset stub aesthetic.** Wordmark on top, shield below, both centered horizontally and vertically, dark `--bg` background, `--font-mono` font already applied via base body styles. Aim for "the design system loaded correctly" feel — not a marketing splash.
- **`@font-face` weights map to actual semantic uses** (per `colors_and_type.css` comments): Regular (400) is body default, Bold (700) is bold/strong, Medium (500) for emphasis, SemiLight (350) for dimmed/captions, Condensed (400) for dense status-bar labels. These will be exercised in Phases 3–5; keeping all 16 declared means none of those phases need to revisit the font CSS.

</specifics>

<deferred>
## Deferred Ideas

- **Design preview page** (token swatches, type scale showcase, sample blocks) — considered but rejected for Phase 2 scope. Could be a useful diagnostic in a later milestone if visual regressions appear; for now DevTools verification per success criteria is sufficient.
- **Scanner SVG copy** — `warp2ironhermes/project/ironhermes/assets/scanner.svg` discovered during scout. Belongs to Phase 3/4 scanner implementation; mention it in the Phase 3 plan rather than copying eagerly here.
- **PROJECT.md doc-staleness fixup** — the Phase 1 code review (IN-05) flagged that `CLAUDE.md` line 32 still says "Cargo.lock is generated but not committed" (now stale post-HYG-02). Same review flagged IN-04 (`[lints.clippy]` table missing in Cargo.toml). Both are candidates for a tiny "01.1" hygiene gap-closure phase before Phase 4 (the first phase that introduces async). Not Phase 2's concern.
- **Tailwind file content cleanup** — third option discussed (`tailwind.css` → `/* unused in v1 */`) deferred. PROJECT.md decision is "available but unused"; if Tailwind stays untouched through v1, revisit at milestone close.

</deferred>

---

*Phase: 02-design-system*
*Context gathered: 2026-05-02*
