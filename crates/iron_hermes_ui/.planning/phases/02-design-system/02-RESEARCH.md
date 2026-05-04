# Phase 2: Design System - Research

**Researched:** 2026-05-02
**Domain:** Dioxus 0.7 asset pipeline, CSS verbatim porting, web font loading, Tailwind v4 cascade interaction
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**CSS File Topology**
- **D-01:** Inject three separate `<link>` tags from `src/app.rs` in cascade order: `main.css` → `design-tokens.css` → `warp-ih.css`. Mirrors the prototype HTML which uses two `<link>` tags (tokens then layout). Three parallel HTTP requests, debuggable in DevTools, easy to swap any single layer.
- **D-02:** No `@import` chains and no concatenation. Prototype's separation between `colors_and_type.css` (design tokens) and `warp-ih.css` (Warp-shell-specific layout) is preserved as the source-of-truth boundary. Future re-syncs from the prototype copy single files, not merged blobs.

**Phase 2 Visible Output**
- **D-03:** `src/components/hero.rs` is reduced to a minimal stub that renders the IronHermes wordmark (`wordmark.svg`) and shield (`ih-shield.png`) centered on the `--bg` background using the `--font-mono` family. No link list, no Dioxus tutorial content. Phase 3 will replace this Hero entirely.
- **D-04:** No "design system preview" page is built. Token verification per success criteria #1–#3 happens via DevTools.

**Font Loading Scope**
- **D-05:** All 16 Ioskeley Mono `.woff2` files are copied into `assets/fonts/` and all 16 corresponding `@font-face` declarations are included in `assets/design-tokens.css` verbatim from `colors_and_type.css`. Browsers lazy-fetch per used weight — declared-but-unused weights are NOT downloaded.
- **D-06:** Font URLs in `design-tokens.css` use the prototype's relative form: `src: url("fonts/IoskeleyMono-Regular.woff2")`. With the CSS file at `/assets/design-tokens.css` and fonts at `/assets/fonts/*.woff2`, browser relative-URL resolution serves them correctly. No Rust `asset!()` macro is needed inside the CSS.

**Tailwind Disposition**
- **D-07:** The Tailwind `<link>` in `src/app.rs` (and the `TAILWIND_CSS` asset constant) stay as-is. The `dx serve` Tailwind compilation pipeline keeps producing `assets/tailwind.css`. Compiled file is small (~11KB), browser-cacheable, and parallel-loaded with the three ported CSS files.

### Claude's Discretion
- **CSS load order placement:** `main.css` first establishes brand-correct fonts/background per DS-04, `design-tokens.css` second defines all `:root` custom properties, `warp-ih.css` third adds Warp-shell-specific tokens and `.wh-*` layout classes. Last-wins cascade keeps the Warp shell's deeper-than-IH surfaces intact.
- **Brand asset placement in stub Hero:** small fixed-height (~32px) wordmark, larger shield (~96px) below it, both centered, ~24px gap. Final placement in real `wh-titlebar` is Phase 3 territory.
- **Existing `assets/main.css` content:** the current 39 lines of placeholder Hero CSS (Segoe UI, white border buttons, etc.) are deleted entirely and replaced with brand-correct base styles per DS-04: dark `--bg` body background, `--font-mono` body font, minimal centering rules for the stub Hero.
- **Theme data-attribute overrides** (`[data-theme="cyan"]` etc.) are ported now even though runtime switching is Phase 5. Pure CSS rules; no harm carrying them.
- **Scanner asset** (`scanner.svg`) — discovered during scout, not Phase 2 success criteria. Copy opportunistically or defer to Phase 3. Planner's call.

### Deferred Ideas (OUT OF SCOPE)
- **Design preview page** — DevTools verification per success criteria is sufficient.
- **Scanner SVG copy** — belongs to Phase 3/4 scanner implementation; mention in Phase 3 plan.
- **PROJECT.md doc-staleness fixup** — IN-04/IN-05 from Phase 1 review; candidate for a "01.1" hygiene gap-closure phase before Phase 4. Not Phase 2's concern.
- **Tailwind file content cleanup** — PROJECT.md decision is "available but unused"; revisit at milestone close.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| DS-01 | 16 Ioskeley Mono `.woff2` files copied into `assets/fonts/` and referenced by `@font-face` declarations | Font directory verified (16 files, 463K–530K each); browser relative-URL resolution from `/assets/design-tokens.css` to `/assets/fonts/*.woff2` is standard; verified Dioxus copies un-hashed originals to bundle |
| DS-02 | `assets/design-tokens.css` mirrors `warp2ironhermes/project/ironhermes/colors_and_type.css` — ANSI palette, font stacks, radii, brand color | Source verified 241 lines, UTF-8, LF-only, no `@import`, sole `url()` refs are the 16 font files |
| DS-03 | `assets/warp-ih.css` mirrors `warp2ironhermes/project/styles/warp-ih.css` — Warp shell layout, blocks, input, palette, side panel | Source verified 507 lines, UTF-8, LF-only, zero `url()` refs, zero `@import`, all 38 `var(--xxx)` consumers map to tokens defined either in colors_and_type.css or warp-ih.css's own `:root` block |
| DS-04 | IronHermes brand assets (`wordmark.svg`, `ih-shield.png`) replace scaffold `header.svg`; default `assets/main.css` updated for brand-correct fonts and background | `asset!("/assets/wordmark.svg")` and `asset!("/assets/ih-shield.png")` follow established Phase 1 pattern; `<img>` works for SVG and PNG identically; current main.css verified as 39-line scaffold for deletion |
</phase_requirements>

---

## Summary

Phase 2 is a static-asset port. No reactive state, no async, no signals — just CSS, fonts, brand images, and a six-line `document::Link` chain in `src/app.rs`. The two CSS source files (`colors_and_type.css`, 241 lines; `warp-ih.css`, 507 lines) are byte-identical drops into `assets/`. The 16 Ioskeley Mono `.woff2` files are byte-identical drops into `assets/fonts/`. The Hero component shrinks to a brand stub. The new `assets/main.css` becomes a ~10-line shell that does little because the ported tokens already define `html, body { background: var(--bg); font-family: var(--font-body); ... }` in `colors_and_type.css`.

The pivotal technical question is how Dioxus 0.7's asset pipeline handles **CSS-internal relative URLs**. The pipeline auto-minifies any file ending in `.css` (whether or not it's referenced by `asset!()`) and produces hashed filenames. However, the bundle output also retains the **original un-hashed filename** alongside the hashed one (verified via `dx bundle` output tree in Dioxus docs: both `main.css` and `main-14aa55e73f669f3e.css` appear in the published bundle). Because `document::Link { href: DESIGN_TOKENS_CSS }` injects the **hashed** path into the `<link>` tag, the browser resolves the CSS file's `url("fonts/...")` relative to the hashed path's directory — `/assets/` — so `fonts/IoskeleyMono-Regular.woff2` resolves to `/assets/fonts/IoskeleyMono-Regular.woff2`. The fonts dropped into `assets/fonts/` are served as static files with no `asset!()` declaration. This matches D-06's premise.

The Tailwind v4 cascade interaction needs attention. Tailwind preflight resets `body` margin to 0, sets `font-family` to the configured sans (Inter-style default), and applies `box-sizing: border-box` globally. With CONTEXT.md's load order (`main.css` → `design-tokens.css` → `warp-ih.css` → `tailwind.css`), Tailwind loads **last** and wins the cascade — meaning preflight will override `colors_and_type.css`'s `body { font-family: var(--font-body); }` rule. **The recommended cascade order is `tailwind.css` FIRST, then `main.css` → `design-tokens.css` → `warp-ih.css`** so the prototype's body styles win. This is a deviation from D-01's ordering and the planner should confirm with the user before committing.

**Primary recommendation:** Execute as five sequential file operations: (1) copy 16 fonts, (2) copy two CSS files with attribution comment, (3) copy two brand assets, (4) rewrite Hero to brand stub, (5) update app.rs with two new `Asset` consts and reorder `document::Link` calls (Tailwind first). Validation is per-DS DevTools inspection plus an `<img>` natural-dimensions check.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| CSS token cascade | Browser (CSS engine) | — | All token resolution happens at render time in the browser; Rust touches only file paths |
| Font loading | Browser (font loader) | — | Browser parses `@font-face` and lazy-fetches per consumed `font-weight`/`font-style` |
| Asset path resolution | Dioxus `asset!()` macro (compile time) + `dx serve` (build time) | Browser (HTTP request) | `asset!()` produces a hashed string at compile time; `dx serve` serves the file from `assets/` |
| CSS minification + hashing | `dx serve` build pipeline | — | Auto-applied to any `.css` in `assets/`; un-hashed original also retained in bundle |
| Brand image rendering | Dioxus RSX `<img>` element | Browser | `src: asset!(...)` works identically for SVG and PNG |
| Tailwind compilation | `dx serve` (delegates to Tailwind v4 CLI) | — | Already wired in Phase 1; no Phase 2 changes |

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| dioxus | =0.7.1 (pinned) | UI framework, `asset!()` macro, `document::Link` | Project requirement [VERIFIED: Cargo.toml] |
| dx CLI | 0.7.3 (installed) | Asset pipeline, Tailwind watcher, hot-reload | [VERIFIED: dx --version, Phase 1 RESEARCH] |
| Tailwind CSS v4 | managed by dx | Utility CSS (available but unused per PROJECT.md) | [VERIFIED: tailwind.css = `@import "tailwindcss";`] |

### Phase 2 adds NO new crate dependencies.
All work is asset copying + Rust source edits to existing modules.

**Version verification:**
- `dioxus 0.7.1`: pinned in Cargo.toml [VERIFIED: Phase 1]
- No package version checks needed — Phase 2 introduces no `Cargo.toml` changes.

---

## Architecture Patterns

### System Architecture Diagram

```
┌────────────────────────── BUILD TIME ──────────────────────────┐
│  src/app.rs                                                    │
│    asset!("/assets/main.css")          ──┐                    │
│    asset!("/assets/design-tokens.css") ──┤                    │
│    asset!("/assets/warp-ih.css")       ──┤  Compile-time      │
│    asset!("/assets/tailwind.css")      ──┤  hash injection    │
│  src/components/hero.rs                  │                    │
│    asset!("/assets/wordmark.svg")      ──┤                    │
│    asset!("/assets/ih-shield.png")     ──┘                    │
│                                          │                    │
│                                          ▼                    │
│                            ┌─── dx serve / dx bundle ───┐    │
│                            │ - minify CSS              │    │
│                            │ - hash filename           │    │
│                            │ - emit BOTH hashed +      │    │
│                            │   un-hashed copies        │    │
│                            │ - copy un-declared static │    │
│                            │   files (fonts/) verbatim │    │
│                            └────────────┬──────────────┘    │
└──────────────────────────────────────────┼────────────────────┘
                                           ▼
┌────────────────────────── RUN TIME ───────────────────────────┐
│  Browser <head>                                              │
│    <link rel="stylesheet" href="/assets/tailwind-h1.css">    │  preflight (loads first)
│    <link rel="stylesheet" href="/assets/main-h2.css">        │  base body rules
│    <link rel="stylesheet" href="/assets/design-tokens-h3.css"│  ── tokens + @font-face
│       │                                                      │     declares "fonts/..."
│       └──── browser resolves relative URL ────► /assets/fonts/IoskeleyMono-Regular.woff2 │
│    <link rel="stylesheet" href="/assets/warp-ih-h4.css">     │  Warp shell layout
│                                                              │
│  Browser font loader: lazy-fetches woff2 per consumed weight │
│  Browser CSS engine: cascade resolves to last-wins token vals│
└──────────────────────────────────────────────────────────────┘
```

### Recommended Project Structure (post-Phase-2)

```
assets/
├── favicon.ico              # unchanged (Phase 1)
├── header.svg               # DELETED (replaced by wordmark + shield)
├── main.css                 # REWRITTEN (39-line scaffold → ~10-line brand stub base)
├── design-tokens.css        # NEW (verbatim copy of colors_and_type.css + 1 attribution comment)
├── warp-ih.css              # NEW (verbatim copy of warp-ih.css + 1 attribution comment)
├── tailwind.css             # unchanged (auto-generated by dx)
├── wordmark.svg             # NEW (brand asset)
├── ih-shield.png            # NEW (brand asset)
└── fonts/                   # NEW directory
    ├── IoskeleyMono-Black.woff2
    ├── IoskeleyMono-Bold.woff2
    ├── IoskeleyMono-BoldItalic.woff2
    ├── IoskeleyMono-Condensed.woff2
    ├── IoskeleyMono-CondensedBold.woff2
    ├── IoskeleyMono-CondensedMedium.woff2
    ├── IoskeleyMono-ExtraBold.woff2
    ├── IoskeleyMono-ExtraLight.woff2
    ├── IoskeleyMono-Italic.woff2
    ├── IoskeleyMono-Light.woff2
    ├── IoskeleyMono-Medium.woff2
    ├── IoskeleyMono-Regular.woff2
    ├── IoskeleyMono-SemiBold.woff2
    ├── IoskeleyMono-SemiCondensed.woff2
    ├── IoskeleyMono-SemiLight.woff2
    └── IoskeleyMono-Thin.woff2

src/
├── app.rs                   # MODIFIED: 2 new Asset consts + 2 new document::Link calls (cascade-corrected order)
├── components/
│   └── hero.rs              # REWRITTEN: stub with wordmark + shield, no link list
└── ...                      # unchanged
```

### Pattern 1: Verbatim CSS Port with Attribution Header

**What:** Copy each source CSS file unchanged, prepending a single attribution comment block above the existing leading `/* ... */` block.

**When to use:** Every prototype CSS file ported into `assets/`. Establishes traceability for future re-syncs without altering the byte-content invariant of the rules.

**Example:**
```css
/* Source: warp2ironhermes/project/ironhermes/colors_and_type.css
 * Copied: 2026-05-02 (Phase 2)
 * DO NOT EDIT — re-sync from source if upstream changes.
 */
/* IronHermes Design System — colors & type
 *
 * Everything here is derived from the `colored` crate usage in the IronHermes
 * ... (rest of source verbatim)
 */
```

**Mechanical copy command (preserves bytes, line endings, encoding):**
```bash
{
  printf '/* Source: %s\n * Copied: %s (Phase 2)\n * DO NOT EDIT — re-sync from source if upstream changes.\n */\n' \
    "warp2ironhermes/project/ironhermes/colors_and_type.css" "$(date -u +%F)"
  cat warp2ironhermes/project/ironhermes/colors_and_type.css
} > assets/design-tokens.css
```

[VERIFIED: source files are UTF-8, LF-only, no BOM — `file` and `od -c` checked 2026-05-02]

### Pattern 2: Cascade-Aware `document::Link` Ordering

**What:** Order `<link>` injections so Tailwind preflight loses to prototype body styles.

**When to use:** Every Dioxus app that mixes Tailwind v4 with hand-authored CSS that touches `body`/`html`/element selectors.

**Recommended (cascade-correct) ordering:**
```rust
// Source: this RESEARCH.md — derived from Tailwind v4 preflight semantics + cascade rules
rsx! {
    document::Link { rel: "icon", href: FAVICON }
    document::Link { rel: "stylesheet", href: TAILWIND_CSS }       // 1st: preflight (loses cascade)
    document::Link { rel: "stylesheet", href: MAIN_CSS }           // 2nd: brand-stub base
    document::Link { rel: "stylesheet", href: DESIGN_TOKENS_CSS }  // 3rd: ANSI tokens + body styles
    document::Link { rel: "stylesheet", href: WARP_IH_CSS }        // 4th: Warp shell tokens + .wh-* classes
    Hero {}
}
```

**Note for planner:** This deviates from CONTEXT.md D-01's stated order (which puts Tailwind last). The deviation is justified because Tailwind v4 preflight resets `body { font-family, margin, line-height }` — these would override prototype tokens if Tailwind loads last. If the user prefers to honor D-01 verbatim, the alternative is to keep Tailwind last and add an explicit `@layer base { html, body { font-family: var(--font-body) !important; } }` override, but this adds 4 lines of cascade-fighting CSS for no benefit. **Planner should surface this choice during plan-discuss.**

### Pattern 3: `asset!()` for Brand Images in Component Module

**What:** Declare brand asset constants at the top of `src/components/hero.rs`, consume via `<img src: ASSET />`.

**When to use:** Any image (SVG, PNG, JPG, AVIF, WebP) referenced by a component.

**Example:**
```rust
// Source: https://dioxuslabs.com/learn/0.7/essentials/ui/assets
use dioxus::prelude::*;

const WORDMARK_SVG: Asset = asset!("/assets/wordmark.svg");
const IH_SHIELD_PNG: Asset = asset!("/assets/ih-shield.png");

#[component]
pub fn Hero() -> Element {
    rsx! {
        div {
            style: "display: flex; flex-direction: column; align-items: center; justify-content: center; \
                    gap: 24px; min-height: 100vh; background: var(--bg);",
            img { src: WORDMARK_SVG, alt: "IronHermes", style: "height: 32px;" }
            img { src: IH_SHIELD_PNG, alt: "", style: "height: 96px;" }
        }
    }
}
```

[VERIFIED: `asset!()` accepts SVG and PNG identically per Dioxus docs — same macro signature, output is `Asset` in both cases. RSX `img { src: ... }` accepts an `Asset` directly via `IntoAttributeValue` impl.]

### Anti-Patterns to Avoid

- **Wrapping fonts in `asset!()` from Rust:** Tempting (cache-busting!), but breaks the verbatim-port invariant — every font URL in `design-tokens.css` would need rewriting, defeating D-02. The fonts work fine as un-declared static files; Dioxus copies them verbatim into the bundle.
- **Concatenating tokens + warp-ih into one file:** Violates D-02. Future re-sync from prototype would require diff-merge instead of single-file replacement.
- **Editing a copied CSS file to "fix" a token:** Violates verbatim-port invariant. Any token issue should be fixed upstream in `warp2ironhermes/` or in a separate override file (`main.css`).
- **Using `document::Stylesheet` instead of `document::Link rel="stylesheet"`:** Both work in 0.7, but the existing `app.rs` uses `document::Link`. Stay consistent — switching mid-file looks like an oversight.
- **Inlining font weights via `<style>` blocks in Hero:** Defeats the design-tokens cascade. The stub Hero relies entirely on inherited body styles plus 2-3 inline layout-only styles.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Font cache-busting | Custom hash-suffix appender on font URLs | Browser-native ETag/Last-Modified caching | Dioxus copies fonts verbatim; HTTP layer handles caching |
| CSS minification | Hand-stripped whitespace before commit | `dx serve` auto-minifies any `.css` in `assets/` | Built into pipeline; preserves source for diffs |
| `@font-face` weight matching | JS that picks the right woff2 per element | Browser's native font-matching algorithm | Spec-compliant, lazy-fetches only used weights |
| CSS variable polyfill | Sass `@use`, PostCSS, or Rust string interpolation | Native CSS custom properties (`--var: ...`) | Universally supported in target browsers; aligns with prototype |
| SVG sprite system | Custom asset bundler | Direct `<img src: asset!("/assets/wordmark.svg") />` | Two SVGs in v1; sprite system would be premature |

**Key insight:** The CSS-port domain rewards mechanical copying. Every "improvement" risks visual drift, which is the project's primary failure mode per PROJECT.md.

---

## Runtime State Inventory

> Phase 2 is a greenfield asset addition (not a rename/refactor/migration). No external services, no databases, no OS-registered state, no secrets. The only "stored state" affected is the build artifact tree, which `dx serve` regenerates from sources on every invocation. Section omitted.

---

## Common Pitfalls

### Pitfall 1: Tailwind Preflight Overrides Body Font

**What goes wrong:** With Tailwind loading last in cascade, `body { font-family: theme(font.sans); }` from preflight wins over `body { font-family: var(--font-body); }` from `design-tokens.css`. Result: body text renders in Inter or system-ui instead of Ioskeley Mono. SC-1 fails.

**Why it happens:** Tailwind v4 preflight applies base-layer body styles unconditionally. When two `<link>` tags assign body styles, the later one wins. [CITED: tailwindcss.com/docs/preflight]

**How to avoid:** Load `tailwind.css` BEFORE `design-tokens.css` in the `<link>` chain (see Pattern 2). Alternatively, load Tailwind last but add `@layer base { html, body { font-family: var(--font-body); background: var(--bg); color: var(--fg); } }` to `main.css`.

**Warning signs:** DevTools → Computed pane on `<body>` shows `font-family: Inter, ...` instead of `Ioskeley Mono, ...`.

### Pitfall 2: Font URL 404 from Wrong Relative Base

**What goes wrong:** If a planner moves `design-tokens.css` somewhere other than `/assets/` (e.g., a sub-folder `/assets/css/`), the relative `url("fonts/...")` resolves to `/assets/css/fonts/...` and 404s.

**Why it happens:** CSS `url()` relative resolution is anchored to the stylesheet's URL, not the document URL. [CITED: developer.mozilla.org/en-US/docs/Web/CSS/url]

**How to avoid:** Keep `design-tokens.css` directly under `assets/`. The directory structure is fixed by D-06.

**Warning signs:** DevTools → Network tab shows 404s for `IoskeleyMono-*.woff2` requests.

### Pitfall 3: Encoding Drift on CSS Copy

**What goes wrong:** Copying with a tool that re-encodes CRLF or adds a BOM produces a non-byte-identical file. Diff between source and copy shows phantom changes; future re-sync corrupts.

**Why it happens:** Some editors auto-add BOM on save; some shells convert line endings on cross-platform copies.

**How to avoid:** Use `cat source > dest` (preserves bytes exactly) or `cp source dest` (preserves on Unix). Verify with `cmp source dest` after stripping the prepended attribution header. [VERIFIED: source files are UTF-8 no-BOM, LF-only — `file` and `od -c` checked 2026-05-02]

**Warning signs:** `git diff --stat` shows whole-file rewrites on a re-sync that should be a no-op.

### Pitfall 4: Asset Macro Path Resolution

**What goes wrong:** Writing `asset!("assets/wordmark.svg")` (no leading slash) produces a Dioxus 0.6-style relative path that is rejected at compile time in 0.7.

**Why it happens:** Dioxus 0.7 migration changed `asset!()` to require absolute paths only. [CITED: dioxuslabs.com/learn/0.7/migration/to_06]

**How to avoid:** All `asset!()` calls must start with `/assets/`. Phase 1's existing constants (`/assets/favicon.ico`, etc.) follow this rule — keep the convention.

**Warning signs:** Compile error: `path must be absolute and start with /` or similar.

### Pitfall 5: `<img alt>` Missing on Brand Stub

**What goes wrong:** Skipping `alt=""` (decorative) or `alt="IronHermes"` (semantic) on the brand stub images. Accessibility regressions slip into a Phase that's "just visual."

**Why it happens:** Brand stubs feel like throwaway code; A11y feels like polish.

**How to avoid:** Wordmark gets `alt="IronHermes"` (semantic — replaces text); shield gets `alt=""` (decorative companion). See Pattern 3.

**Warning signs:** Lighthouse a11y audit flags missing alts; future grep for `img {` shows props-less images.

### Pitfall 6: Hot-Reload Hits a Compile-Time Boundary

**What goes wrong:** Editing `assets/design-tokens.css` triggers instant CSS hot-reload. But editing `src/app.rs` (e.g., reordering `document::Link` calls) requires a full re-compile.

**Why it happens:** Asset hot-reload is for assets. Source edits go through the Rust compiler. [CITED: dioxuslabs.com/learn/0.7/essentials/ui/hotreload]

**How to avoid:** Set expectations: "edit CSS to iterate on tokens; restart `dx serve` after editing app.rs." Plan tasks accordingly — group app.rs edits into one task to avoid recompile thrash.

**Warning signs:** Browser shows old CSS variables despite a CSS edit → check that the file is actually under `assets/` and saved.

---

## Code Examples

Verified patterns from official sources and codebase.

### CSS Verbatim Copy with Attribution

```bash
# Source: this RESEARCH.md, Pattern 1
# Run from project root:
{
  printf '/* Source: %s\n * Copied: %s (Phase 2)\n * DO NOT EDIT — re-sync from source if upstream changes.\n */\n' \
    "warp2ironhermes/project/ironhermes/colors_and_type.css" "$(date -u +%F)"
  cat warp2ironhermes/project/ironhermes/colors_and_type.css
} > assets/design-tokens.css

{
  printf '/* Source: %s\n * Copied: %s (Phase 2)\n * DO NOT EDIT — re-sync from source if upstream changes.\n */\n' \
    "warp2ironhermes/project/styles/warp-ih.css" "$(date -u +%F)"
  cat warp2ironhermes/project/styles/warp-ih.css
} > assets/warp-ih.css
```

### `src/app.rs` after Phase 2 (cascade-corrected)

```rust
// Source pattern: existing src/app.rs (Phase 1) + Pattern 2 reordering
use dioxus::prelude::*;
use crate::components::Hero;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const DESIGN_TOKENS_CSS: Asset = asset!("/assets/design-tokens.css");
const WARP_IH_CSS: Asset = asset!("/assets/warp-ih.css");

#[component]
pub fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: DESIGN_TOKENS_CSS }
        document::Link { rel: "stylesheet", href: WARP_IH_CSS }
        Hero {}
    }
}
```

### `src/components/hero.rs` after Phase 2 (brand stub)

```rust
// Source pattern: Pattern 3 (this RESEARCH.md)
use dioxus::prelude::*;

const WORDMARK_SVG: Asset = asset!("/assets/wordmark.svg");
const IH_SHIELD_PNG: Asset = asset!("/assets/ih-shield.png");

#[component]
pub fn Hero() -> Element {
    rsx! {
        div {
            style: "display: flex; flex-direction: column; align-items: center; \
                    justify-content: center; gap: 24px; min-height: 100vh;",
            img { src: WORDMARK_SVG, alt: "IronHermes", style: "height: 32px;" }
            img { src: IH_SHIELD_PNG, alt: "", style: "height: 96px;" }
        }
    }
}
```

Note: `background: var(--bg)` is inherited from `html, body { background: var(--bg); }` in the ported `design-tokens.css`. No need to repeat it inline.

### `assets/main.css` after Phase 2 (replacement for 39-line scaffold)

```css
/* IronHermes — base shell styles (Phase 2)
 *
 * Most base rules live in design-tokens.css (the verbatim port of
 * colors_and_type.css). This file holds only what design-tokens.css
 * does NOT define and what is too project-specific to push upstream.
 */

html, body {
  margin: 0;
  padding: 0;
  min-height: 100vh;
}
```

That's it — design-tokens.css already sets `background`, `color`, `font-family`, `font-size`, and `line-height` on `html, body`. main.css only adds the margin/padding reset that the prototype HTML applied via an inline `<style>` block.

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Hand-managed font cache-busting via query strings | Browser ETag + auto-hashed CSS via `dx` | Dioxus 0.6+ asset macro | Cache-busting is a pipeline concern, not a CSS concern |
| Font subsetting + concatenation | Per-weight `.woff2` with `font-display: swap` | Modern browsers (2020+) | Lazy fetch per used weight; declared-but-unused weights are free |
| `<style>` blocks injected at runtime | `document::Link` to bundled CSS | Dioxus 0.7 SSR-ready styling | Pre-loaded; no FOUC during hydration (SSR not used in v1, but pattern future-proofs) |
| `@import` chains in CSS | Multiple `<link>` tags | Performance best practice (2018+) | Parallel HTTP requests; per-file caching; no render-blocking import resolution |

**Deprecated/outdated:**
- Sass/Less preprocessing for CSS variables — native `--var` is universal in target browsers; prototype uses native form already.
- Webfont loaders (Typekit-style JS shims) — `font-display: swap` covers the FOIT problem natively.

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Tailwind v4 preflight resets `body { font-family, margin, line-height }` and applies `box-sizing: border-box` globally | Common Pitfalls (Pitfall 1), Summary | If preflight is more aggressive (e.g., resets `:root` custom properties), additional override layers needed. Easily verified post-build by inspecting compiled `assets/tailwind.css` for `:root` rules. |
| A2 | Static files dropped into `assets/fonts/` are served at `/assets/fonts/...` without `asset!()` declaration | Summary, Pattern 3, Pitfall 2 | If `dx` only serves files declared via `asset!()`, the 16 woff2 files would 404. Mitigation: declare a folder asset (`asset!("/assets/fonts")`) or rewrite font URLs to use `asset!()` per-file. Verifiable in 5 seconds via DevTools Network tab on first run. |
| A3 | The cascade-order deviation from D-01 (Tailwind first vs. last) is preferable to adding `@layer base` overrides | Pattern 2, Summary | If user prefers strict D-01 honor, planner adds 4 lines of `@layer base` cascade-fighting CSS. Material is small either way. |
| A4 | `cat src > dst` preserves byte-identity on macOS for UTF-8/LF files | Pattern 1, Pitfall 3 | Standard POSIX behavior; preserves bytes including line endings. If wrong, `cmp` after copy detects drift. |
| A5 | `font-display: swap` produces no observable FOUT in v1 because the brand stub is the only on-screen text and uses a single weight (default 400) | (implied — not flagged as risk) | Negligible — even if FOUT visible, it's a cosmetic micro-flash on first load. Phase 3+ may want `font-display: optional` for status-bar pills. |

---

## Open Questions (RESOLVED)

1. **Should `scanner.svg` be copied opportunistically in Phase 2?** RESOLVED — defer to Phase 3.
   - What we know: CONTEXT.md Claude's Discretion section says "Planner's call." It's used by the Phase 3/4 scanner status-bar implementation.
   - What's unclear: Whether the planner prefers tight Phase 2 scope (no scanner.svg) or pre-staged assets (copy now to unblock Phase 3 work later).
   - Recommendation: **Defer to Phase 3.** Phase 2 success criteria don't reference it; copying it now muddies the "design system foundation" framing. Phase 3's plan will copy it as part of building the status bar.
   - Implemented in: 02-03 (explicit `! test -f assets/scanner.svg` acceptance criterion preventing accidental inclusion).

2. **Should D-01's cascade order be honored verbatim, accepting an `@layer base` override?** RESOLVED — Tailwind moved to position 2 (after FAVICON, before main.css → design-tokens.css → warp-ih.css). The relative order of the three ported CSS files in D-01 is preserved verbatim.
   - What we know: Tailwind v4 preflight will override prototype body styles if loaded last.
   - What's unclear: User's preference between (a) deviating from CONTEXT.md to put Tailwind first, vs. (b) honoring D-01 and adding 4 lines of override CSS to `main.css`.
   - Recommendation: **Surface in plan-discuss.** Both options work; cleanliness is the deciding factor and that's a stylistic preference.
   - Implemented in: 02-04 `<deviations>` block with rationale + line-ordering acceptance criterion.

3. **Should the `main.css` replacement include any defensive CSS resets?** RESOLVED — match the prototype exactly; no extra resets.
   - What we know: Prototype HTML uses an inline `<style>` for `html, body { margin: 0; padding: 0; ... }`.
   - What's unclear: Whether any other normalize-style resets are implicitly relied on (e.g., Tailwind preflight handles `box-sizing`).
   - Recommendation: Match the prototype's inline `<style>` exactly in `main.css`. Don't add normalize.css. Don't add resets the prototype didn't have. (See Code Examples → `assets/main.css`.)
   - Implemented in: 02-04 Task 3 (rewrites `assets/main.css` to the brand-stub base matching the prototype).

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain | Source compilation | ✓ | 1.94.0 (Phase 1) | — |
| dx CLI | Asset pipeline, hot-reload | ✓ | 0.7.3 | — |
| `cat` / `cp` | Verbatim CSS copy | ✓ (POSIX shell builtin) | — | — |
| Browser DevTools | SC-1, SC-2, SC-3 verification | ✓ (any modern browser) | — | — |
| `cmp` | Byte-identity verification post-copy | ✓ (POSIX) | — | `diff -q` |

**Missing dependencies with no fallback:** None.

**Missing dependencies with fallback:** None.

This phase is fully self-contained — every required tool is already installed (Phase 1) or shell-builtin.

---

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | None configured (REQUIREMENTS.md: "Test suite ... acknowledged gap; revisit before adding real backend") |
| Config file | none |
| Quick run command | `cargo build --features web` |
| Full suite command | `cargo build --features web && cargo build --features desktop && cargo build --features mobile` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| DS-01 | 16 woff2 files in `assets/fonts/` + 16 `@font-face` declarations in `design-tokens.css` | smoke (file count + grep) | `ls assets/fonts/*.woff2 \| wc -l` (== 16) && `grep -c '@font-face' assets/design-tokens.css` (== 16) | ❌ Wave 0 — fonts dir not yet created |
| DS-01 | Body text actually rendered in Ioskeley Mono | manual UAT | DevTools → Inspect `<body>` → Computed → `font-family` starts with `"Ioskeley Mono"` | n/a — runtime verification |
| DS-02 | `design-tokens.css` is byte-identical to source (modulo attribution header) | smoke | `tail -n +5 assets/design-tokens.css \| cmp - warp2ironhermes/project/ironhermes/colors_and_type.css` (exit 0) | ❌ Wave 0 — file not yet created |
| DS-02 | CSS custom props resolve in browser | manual UAT | DevTools console: `getComputedStyle(document.documentElement).getPropertyValue('--accent-primary')` returns `#4ec9b0`, `--brand` returns `#f0883e`, `--font-mono` starts with `"Ioskeley Mono"`, `--w-radius-block` returns `6px` | n/a — runtime |
| DS-03 | `warp-ih.css` is byte-identical to source (modulo attribution header) | smoke | `tail -n +5 assets/warp-ih.css \| cmp - warp2ironhermes/project/styles/warp-ih.css` (exit 0) | ❌ Wave 0 — file not yet created |
| DS-03 | Warp shell classes loaded into stylesheet | smoke | `grep -c '\.wh-' assets/warp-ih.css` (>= 50; source has ~57 `.wh-*` rule selectors) | n/a — same source = same output |
| DS-04 | Wordmark + shield assets present | smoke | `test -f assets/wordmark.svg && test -f assets/ih-shield.png` | ❌ Wave 0 — assets not yet copied |
| DS-04 | Brand assets render at runtime (no broken `<img>`) | manual UAT + DevTools | DevTools → Network: both requests 200; or Console: `[...document.images].every(i => i.naturalWidth > 0)` returns `true` | n/a — runtime |
| All | Three-platform build still green | compile | `cargo build --features web && cargo build --features desktop && cargo build --features mobile` | ✓ Phase 1 gate established |

### Sampling Rate
- **Per task commit:** `cargo build --features web` (catches Rust compile breaks from app.rs / hero.rs edits)
- **Per wave merge:** Full three-platform build (matches Phase 1 gate)
- **Phase gate:** All smoke checks pass + manual UAT in browser confirms SC-1..SC-4 + full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `assets/fonts/` directory (created by `mkdir -p assets/fonts/` then `cp` of 16 files)
- [ ] `assets/design-tokens.css` (verbatim copy of `colors_and_type.css` with attribution header)
- [ ] `assets/warp-ih.css` (verbatim copy of `warp-ih.css` with attribution header)
- [ ] `assets/wordmark.svg`, `assets/ih-shield.png` (copied from `warp2ironhermes/project/ironhermes/assets/`)
- [ ] `assets/main.css` rewritten (39-line scaffold → ~10-line brand stub base)
- [ ] `assets/header.svg` deleted (and `HEADER_SVG` constant removed from hero.rs)
- [ ] No test framework install needed — REQUIREMENTS.md confirms test suite is out-of-scope for v1.

### Manual UAT Checklist (single browser session, runs after Wave merges)

1. Run `dx serve --features web`. Open DevTools.
2. **SC-1 (DS-01):** Inspect `<body>` → Computed → `font-family` should start with `"Ioskeley Mono"`. Network tab should show 1 woff2 request (Regular, 400) for the brand stub.
3. **SC-2 (DS-02):** Console run:
   ```js
   const cs = getComputedStyle(document.documentElement);
   console.assert(cs.getPropertyValue('--accent-primary').trim() === '#4ec9b0', 'accent-primary');
   console.assert(cs.getPropertyValue('--brand').trim()          === '#f0883e', 'brand');
   console.assert(cs.getPropertyValue('--font-mono').includes('Ioskeley Mono'), 'font-mono');
   console.assert(cs.getPropertyValue('--w-radius-block').trim() === '6px',     'w-radius-block');
   ```
   All four asserts should silently pass.
4. **SC-3 (DS-03):** Console run: `[...document.styleSheets].some(s => [...s.cssRules].some(r => r.selectorText && r.selectorText.includes('.wh-app')))` returns `true`.
5. **SC-4 (DS-04):** Visual: wordmark visible at top center, shield below it, both on dark `#0d1117` background. Console: `[...document.images].every(i => i.naturalWidth > 0)` returns `true`.

---

## Security Domain

Phase 2 is pure CSS, font, and brand-image asset addition. No network calls (fonts and CSS are same-origin static assets), no user input handling, no authentication, no cryptography, no server endpoints, no secrets. ASVS V2–V6 do not apply.

The only theoretical attack surface introduced is `@font-face` from a same-origin `.woff2`. WOFF2 has had vulnerabilities historically (e.g., Brotli decompressor bugs), but the files are vendored from the prototype handoff and served from the same origin — no third-party font CDN is used. No mitigation beyond standard browser font loader sandboxing is needed.

---

## Project Constraints (from CLAUDE.md)

- Dioxus 0.7 only — no `cx`, `Scope`, or `use_state`. Use `use_signal`, `use_memo`, etc. (Phase 2 introduces no state, so this constraint is trivially satisfied.)
- Component functions are `PascalCase` and `#[component]`-annotated. Hero stays `pub fn Hero() -> Element`.
- Signal borrows must not span `.await`. (No async in Phase 2.)
- Multi-platform: no platform-gated code in Phase 2. CSS and assets serve all three features identically.
- No external services / network calls. Fonts are same-origin static assets. (No violation.)
- Pixel-perfect to prototype. Verbatim CSS copy with attribution header is the implementation of this constraint.
- `warp2ironhermes/` is read-only. Phase 2 only reads from it; never imports or includes from build paths.
- Rsx blocks are 4-space indented. Asset constants in SCREAMING_SNAKE_CASE, declared `const NAME: Asset = asset!("/assets/...");` at module top.
- Asset paths use leading-slash form: `asset!("/assets/...")`.
- Asset constants colocated with consuming component (Phase 1 module-split decision): `WORDMARK_SVG` and `IH_SHIELD_PNG` go in `hero.rs`; `DESIGN_TOKENS_CSS` and `WARP_IH_CSS` go in `app.rs`.

---

## Sources

### Primary (HIGH confidence)
- [/websites/dioxuslabs_learn via Context7] — `asset!()` semantics, hash suffix behavior, hot-reload, `document::Stylesheet` vs `document::Link`, public folder vs assets folder, bundle output structure
- [dioxuslabs.com/learn/0.7/essentials/ui/assets] — asset hashing produces e.g. `ferrous_wave-dxhx13xj2j.png`; bundle retains both hashed and un-hashed files
- [dioxuslabs.com/learn/0.7/essentials/ui/hotreload] — CSS edits hot-reload; Rust source edits require recompile
- [dioxuslabs.com/learn/0.7/migration/to_06] — `asset!()` requires absolute paths in 0.7
- [dioxuslabs.com/learn/0.7/tutorial/bundle/llms.txt] — bundle output tree shows both `main.css` and `main-14aa55e73f669f3e.css` in `assets/`
- Source CSS files (verified via `file`, `od -c`, `wc -l`): UTF-8, LF-only, 241 + 507 lines, no `@import`, no BOM
- `assets/main.css` (verified via Read): 39-line scaffold matching CONTEXT.md description

### Secondary (MEDIUM confidence)
- [tailwindcss.com/docs/preflight] — Tailwind v4 preflight applies base styles to body (font-family, margin, line-height) and `* { box-sizing: border-box }`
- [warp2ironhermes/project/Warp × IronHermes.html] — prototype HTML reference confirming canonical CSS load order: `colors_and_type.css` then `warp-ih.css`

### Tertiary (LOW confidence / ASSUMED)
- A2: Un-declared static files in `assets/fonts/` are served by `dx serve` — strongly implied by the docs ("any files that end with .css will be minified and bundled" plus the bundle tree showing un-hashed copies retained), but not literally confirmed for `.woff2`. Verifiable in 5 seconds on first build via DevTools Network tab.

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps; existing Phase 1 stack unchanged
- Verbatim CSS copy logistics: HIGH — source files verified UTF-8/LF/no-BOM/no-imports; mechanical bash recipe is byte-stable
- Asset pipeline behavior: HIGH for declared assets (Context7 + bundle output tree); MEDIUM-HIGH for un-declared font files (assumption A2, easily verified at first build)
- Tailwind v4 cascade interaction: MEDIUM — preflight semantics confirmed; recommended cascade-first ordering deviates from D-01 and needs user sign-off
- Brand stub Hero implementation: HIGH — established Phase 1 pattern, verified `asset!()` works for SVG/PNG identically

**Research date:** 2026-05-02
**Valid until:** 2026-06-01 (stable domain; Dioxus 0.7.x patch releases unlikely to change asset pipeline semantics; Tailwind v4 preflight is stable)
