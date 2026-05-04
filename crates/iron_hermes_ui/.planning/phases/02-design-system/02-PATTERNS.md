# Phase 2: Design System - Pattern Map

**Mapped:** 2026-05-02
**Files analyzed:** 11 (5 new, 4 modified, 1 deleted, 1 directory)
**Analogs found:** 9 / 11 (2 verbatim ports have no Rust analog by design — they are byte-stable CSS copies)

---

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `assets/fonts/` (16× `.woff2`) | static-asset (binary) | file-I/O (browser-fetched) | none — Phase 1 had no font directory | no analog (mechanical copy) |
| `assets/design-tokens.css` | static-asset (CSS tokens) | file-I/O (browser-loaded) | `assets/main.css` (Phase 1 scaffold — same `assets/*.css` slot) | role-match (verbatim port from `warp2ironhermes/`) |
| `assets/warp-ih.css` | static-asset (CSS layout) | file-I/O (browser-loaded) | `assets/main.css` (Phase 1 scaffold — same `assets/*.css` slot) | role-match (verbatim port from `warp2ironhermes/`) |
| `assets/wordmark.svg` | static-asset (SVG image) | file-I/O (browser-loaded via `<img>`) | `assets/header.svg` (Phase 1 scaffold image) | exact-role |
| `assets/ih-shield.png` | static-asset (PNG image) | file-I/O (browser-loaded via `<img>`) | `assets/header.svg` (closest existing image asset) | role-match (different format, same role) |
| `assets/main.css` (rewrite) | config (base CSS reset) | file-I/O (browser-loaded) | `assets/main.css` (self, before state — 39-line scaffold) | self-edit (full rewrite) |
| `assets/header.svg` (delete) | static-asset (deletion) | — | n/a | n/a (removal) |
| `src/app.rs` (modify) | component (root + asset registry) | request-response | `src/app.rs` lines 1–16 (self, before state) | self-edit (additive) |
| `src/components/hero.rs` (modify) | component (UI stub) | request-response | `src/components/hero.rs` lines 1–21 (self, before state) | self-edit (full rewrite of body) |
| `src/main.rs` | entrypoint | — | `src/main.rs` (self) | no-change (Phase 2 doesn't touch it) |
| `Dioxus.toml` | config | — | `Dioxus.toml` (self) | no-change (Phase 2 doesn't touch it) |

---

## Pattern Assignments

### `src/app.rs` (component, request-response — DS-01/02/03 wire-up)

**Analog:** `src/app.rs` current state (self-edit; additive)

**Source excerpt — current state** (`/Users/twilson/code/iron_hermes_ui/src/app.rs` lines 1–16):
```rust
use dioxus::prelude::*;
use crate::components::Hero;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        Hero {}
    }
}
```

**Asset constant pattern** (lines 4–6) — copy verbatim for two new constants:
```rust
const NAME: Asset = asset!("/assets/filename.ext");
```
- SCREAMING_SNAKE_CASE
- Type is `Asset` (no path-import — comes via `dioxus::prelude::*`)
- Path always begins `/assets/`
- Declared at module top, after `use` statements, before `#[component]`

**`document::Link` injection pattern** (lines 11–13) — copy verbatim for two new stylesheet links:
```rust
document::Link { rel: "stylesheet", href: ASSET_CONST }
```
- Stylesheet uses `rel: "stylesheet"`
- Icon uses `rel: "icon"` (line 11)
- `href` value is the bare `Asset` constant (no string interpolation, no `.to_string()`)
- Order in `rsx!` block determines `<link>` order in `<head>`, which determines cascade

**After-state target** (per RESEARCH.md Pattern 2 — Tailwind first to defeat preflight; deviates from CONTEXT.md D-01, planner must surface for user sign-off):
```rust
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

**Key deltas:**
1. Two new `Asset` constants (`DESIGN_TOKENS_CSS`, `WARP_IH_CSS`)
2. Cascade-aware reordering: Tailwind moves up to position 2 (was last)
3. Two new `document::Link` calls in positions 4 and 5
4. `Hero {}` invocation unchanged (still last in the rsx tree)

**Cascade reasoning** (from RESEARCH.md Pitfall 1): Tailwind v4 preflight resets `body { font-family, margin, line-height }`. If Tailwind loads last, preflight wins over `colors_and_type.css`'s `body { font-family: var(--font-body); }` rule and SC-1 fails. Loading Tailwind first lets prototype tokens win.

---

### `src/components/hero.rs` (component, request-response — DS-04 brand stub)

**Analog:** `src/components/hero.rs` current state (self-edit; full body rewrite, signature unchanged)

**Source excerpt — current state** (`/Users/twilson/code/iron_hermes_ui/src/components/hero.rs` lines 1–21):
```rust
use dioxus::prelude::*;

const HEADER_SVG: Asset = asset!("/assets/header.svg");

#[component]
pub fn Hero() -> Element {
    rsx! {
        div {
            id: "hero",
            img { src: HEADER_SVG, id: "header" }
            div { id: "links",
                a { href: "https://dioxuslabs.com/learn/0.7/", "📚 Learn Dioxus" }
                a { href: "https://dioxuslabs.com/awesome", "🚀 Awesome Dioxus" }
                a { href: "https://github.com/dioxus-community/", "📡 Community Libraries" }
                a { href: "https://github.com/DioxusLabs/sdk", "⚙️ Dioxus Development Kit" }
                a { href: "https://marketplace.visualstudio.com/items?itemName=DioxusLabs.dioxus", "💫 VSCode Extension" }
                a { href: "https://discord.gg/XgGxMSkvUM", "👋 Community Discord" }
            }
        }
    }
}
```

**Patterns to keep verbatim:**

1. **Imports pattern** (line 1) — unchanged:
   ```rust
   use dioxus::prelude::*;
   ```

2. **Component signature pattern** (lines 5–6) — unchanged:
   ```rust
   #[component]
   pub fn Hero() -> Element {
   ```

3. **`<img>` rendering pattern** (line 10) — copy structure for both new images:
   ```rust
   img { src: HEADER_SVG, id: "header" }
   ```
   - `src: ASSET_CONST` (bare const, no string)
   - Additional attributes: `id`, `style`, `alt`, `class` accepted directly as kwargs
   - Self-closing (no children block)

**Patterns to delete:**

1. The `id: "hero"` outer wrapper and `id: "links"` inner div (lines 9, 11)
2. All six `a { href: ... }` link elements (lines 12–17) — these reference the old Dioxus tutorial scaffolding
3. The `HEADER_SVG` constant (line 3) — replaced by two new constants

**After-state target** (per RESEARCH.md Pattern 3 + CONTEXT.md D-03 + Discretion item "Brand asset placement in stub Hero"):
```rust
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

**Key deltas:**
1. `HEADER_SVG` → `WORDMARK_SVG` + `IH_SHIELD_PNG` (two consts)
2. Outer div: drop `id: "hero"`, add inline `style` for centered column layout (24px gap, full-viewport-height)
3. Inner div + `<a>` link list deleted entirely
4. Two `<img>` tags: wordmark on top (semantic `alt="IronHermes"`, ~32px), shield below (decorative `alt=""`, ~96px)
5. `background: var(--bg)` is **not** repeated inline — inherited from `html, body { background: var(--bg); }` in the ported `design-tokens.css`

**A11y note** (RESEARCH.md Pitfall 5): wordmark gets `alt="IronHermes"` (semantic — replaces brand text); shield gets `alt=""` (decorative companion).

---

### `assets/main.css` (config, file-I/O — base CSS reset, full rewrite)

**Analog:** `assets/main.css` current state (self-edit; full rewrite)

**Source excerpt — current state** (`/Users/twilson/code/iron_hermes_ui/assets/main.css` lines 1–47):
- Lines 1–7: scaffold body styles (Segoe UI font, `#0f1116` bg, 20px margin) — **delete entirely**, replaced by tokens from `design-tokens.css`
- Lines 9–15: `#hero` flexbox centering — **delete**, no longer applies (stub Hero uses inline style, no `id`)
- Lines 17–39: `#links` and `#links a` styling — **delete**, link list is gone
- Lines 41–43: `#header` `max-width` — **delete**, no `#header` element exists

**After-state target** (per RESEARCH.md "`assets/main.css` after Phase 2"):
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

**Rationale (RESEARCH.md Open Question 3):** `design-tokens.css` already sets `background`, `color`, `font-family`, `font-size`, and `line-height` on `html, body`. `main.css` only adds the margin/padding reset that the prototype HTML applied via an inline `<style>` block. No normalize.css. No box-sizing reset (Tailwind preflight handles it).

---

### `assets/design-tokens.css` (static-asset, verbatim port — DS-02)

**Analog:** None in current Rust codebase. Source-of-truth file is `warp2ironhermes/project/ironhermes/colors_and_type.css` (READ-ONLY reference, not part of build).

**Pattern to apply** (RESEARCH.md Pattern 1 — Verbatim CSS Port with Attribution Header):

**Mechanical copy command** (preserves bytes, line endings, encoding):
```bash
{
  printf '/* Source: %s\n * Copied: %s (Phase 2)\n * DO NOT EDIT — re-sync from source if upstream changes.\n */\n' \
    "warp2ironhermes/project/ironhermes/colors_and_type.css" "$(date -u +%F)"
  cat warp2ironhermes/project/ironhermes/colors_and_type.css
} > assets/design-tokens.css
```

**Resulting file structure:**
```css
/* Source: warp2ironhermes/project/ironhermes/colors_and_type.css
 * Copied: 2026-05-02 (Phase 2)
 * DO NOT EDIT — re-sync from source if upstream changes.
 */
/* IronHermes Design System — colors & type
 * ... (rest of source 241 lines verbatim) ...
 */
:root { ... }
@font-face { ... }  /* 16 declarations */
html, body { background: var(--bg); ... }
```

**Verification command** (RESEARCH.md Validation Architecture — DS-02 byte-identity smoke):
```bash
tail -n +5 assets/design-tokens.css | cmp - warp2ironhermes/project/ironhermes/colors_and_type.css
# exit 0 = byte-identical (modulo the 4-line attribution header)
```

**Constraints:**
- UTF-8 encoding, LF line endings, no BOM (verified in RESEARCH.md Sources)
- No `@import` chains (D-02)
- 16 `@font-face` declarations preserved (D-05)
- Font URLs stay as `url("fonts/IoskeleyMono-*.woff2")` — relative resolution from `/assets/design-tokens.css` to `/assets/fonts/*.woff2` works without `asset!()` wrapping (D-06)

---

### `assets/warp-ih.css` (static-asset, verbatim port — DS-03)

**Analog:** None in current Rust codebase. Source-of-truth file is `warp2ironhermes/project/styles/warp-ih.css` (READ-ONLY reference).

**Pattern to apply** — identical to `design-tokens.css` (Pattern 1):
```bash
{
  printf '/* Source: %s\n * Copied: %s (Phase 2)\n * DO NOT EDIT — re-sync from source if upstream changes.\n */\n' \
    "warp2ironhermes/project/styles/warp-ih.css" "$(date -u +%F)"
  cat warp2ironhermes/project/styles/warp-ih.css
} > assets/warp-ih.css
```

**Verification command:**
```bash
tail -n +5 assets/warp-ih.css | cmp - warp2ironhermes/project/styles/warp-ih.css
# exit 0 = byte-identical
```

**Constraints:**
- UTF-8 encoding, LF line endings, no BOM (RESEARCH.md verified)
- 507 lines source
- Zero `url()` refs, zero `@import` (RESEARCH.md DS-03 row)
- `.wh-*` selector count >= 50 (smoke check)
- `[data-theme="cyan"]` / `[data-theme="magenta"]` / etc. data-attribute overrides included (CONTEXT.md Discretion item — pure CSS, no harm in v1)

---

### `assets/fonts/` (16× `.woff2`, static-asset directory — DS-01)

**Analog:** None in current codebase. Source files: `warp2ironhermes/project/ironhermes/fonts/IoskeleyMono-*.woff2` (16 files).

**Pattern to apply** — directory creation + bulk binary copy:
```bash
mkdir -p assets/fonts
cp warp2ironhermes/project/ironhermes/fonts/IoskeleyMono-*.woff2 assets/fonts/
```

**File list** (verified in CONTEXT.md canonical_refs):
```
IoskeleyMono-Black.woff2
IoskeleyMono-Bold.woff2
IoskeleyMono-BoldItalic.woff2
IoskeleyMono-Condensed.woff2
IoskeleyMono-CondensedBold.woff2
IoskeleyMono-CondensedMedium.woff2
IoskeleyMono-ExtraBold.woff2
IoskeleyMono-ExtraLight.woff2
IoskeleyMono-Italic.woff2
IoskeleyMono-Light.woff2
IoskeleyMono-Medium.woff2
IoskeleyMono-Regular.woff2
IoskeleyMono-SemiBold.woff2
IoskeleyMono-SemiCondensed.woff2
IoskeleyMono-SemiLight.woff2
IoskeleyMono-Thin.woff2
```

**Verification command** (RESEARCH.md DS-01 row):
```bash
ls assets/fonts/*.woff2 | wc -l   # == 16
grep -c '@font-face' assets/design-tokens.css   # == 16
```

**Asset pipeline note** (RESEARCH.md Assumption A2): Static files dropped into `assets/fonts/` are served at `/assets/fonts/...` by `dx serve` without any Rust `asset!()` declaration. Browser resolves relative `url("fonts/...")` from the loaded `/assets/design-tokens.css` to `/assets/fonts/...`. Verifiable in DevTools Network tab on first build.

**Anti-pattern (DO NOT DO):** Wrap fonts in `asset!()` from Rust. This would force rewriting every `url(...)` in the verbatim-ported `design-tokens.css`, defeating D-02.

---

### `assets/wordmark.svg` (static-asset, brand image — DS-04)

**Analog:** `assets/header.svg` (Phase 1 scaffold — same role: hero image referenced via `<img src: ASSET_CONST />`)

**Pattern to apply** — single binary copy:
```bash
cp warp2ironhermes/project/ironhermes/assets/wordmark.svg assets/wordmark.svg
```

**Verification command:**
```bash
test -f assets/wordmark.svg
```

**Consumer pattern** (from `src/components/hero.rs` Pattern 3 in this doc):
```rust
const WORDMARK_SVG: Asset = asset!("/assets/wordmark.svg");
// ...
img { src: WORDMARK_SVG, alt: "IronHermes", style: "height: 32px;" }
```

---

### `assets/ih-shield.png` (static-asset, brand image — DS-04)

**Analog:** `assets/header.svg` (closest existing image asset; PNG vs SVG is irrelevant — `asset!()` and `<img>` handle both identically per RESEARCH.md Pattern 3 verification note)

**Pattern to apply** — single binary copy:
```bash
cp warp2ironhermes/project/ironhermes/assets/ih-shield.png assets/ih-shield.png
```

**Verification command:**
```bash
test -f assets/ih-shield.png
```

**Consumer pattern**:
```rust
const IH_SHIELD_PNG: Asset = asset!("/assets/ih-shield.png");
// ...
img { src: IH_SHIELD_PNG, alt: "", style: "height: 96px;" }
```

---

### `assets/header.svg` (deletion)

**Analog:** n/a (removal). Tracked here for completeness — Phase 1 left this file present and `HEADER_SVG`-referenced. Phase 2 removes the const (in `hero.rs` rewrite) and the file.

**Command:**
```bash
rm assets/header.svg
```

**Verification:** `cargo build --features web` must still succeed; `git status` should show the file as deleted.

---

## No Analog Found

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| `assets/design-tokens.css` | static-asset CSS port | file-I/O | The Rust codebase has no precedent for verbatim-CSS-port pattern with attribution header. Source is in the read-only `warp2ironhermes/` reference tree. RESEARCH.md Pattern 1 supplies the byte-stable bash recipe. |
| `assets/warp-ih.css` | static-asset CSS port | file-I/O | Same as above. RESEARCH.md Pattern 1 applies identically. |
| `assets/fonts/*.woff2` | static-asset binary | file-I/O | No font files exist yet in `assets/`. Mechanical `cp` copy with no Rust wrapping (per D-06). |

These three are byte-stable static-asset ports — they have no Rust analog *by design*. The "pattern" is mechanical preservation of source bytes.

---

## Shared Patterns

### Asset Constant Declaration
**Source:** `src/app.rs` lines 4–6 (Phase 1 — three existing constants), `src/components/hero.rs` line 3 (one existing constant)
**Apply to:** `src/app.rs` (`DESIGN_TOKENS_CSS`, `WARP_IH_CSS`), `src/components/hero.rs` (`WORDMARK_SVG`, `IH_SHIELD_PNG`)
```rust
const NAME: Asset = asset!("/assets/filename.ext");
```
- SCREAMING_SNAKE_CASE
- Type is `Asset` (imported via `use dioxus::prelude::*`)
- Path always begins with leading `/assets/` slash (Pitfall 4 — Dioxus 0.7 rejects relative paths)
- Declared at module top, after `use` statements
- File extension matches the actual file — `.css`, `.svg`, `.png`, `.ico` all work identically

### `document::Link` Stylesheet Injection
**Source:** `src/app.rs` lines 11–13 (Phase 1 — three existing link calls)
**Apply to:** `src/app.rs` (two new stylesheet links for `DESIGN_TOKENS_CSS` and `WARP_IH_CSS`)
```rust
document::Link { rel: "stylesheet", href: ASSET_CONST }
```
- `rel: "stylesheet"` for CSS, `rel: "icon"` for favicon
- `href` is the bare `Asset` const (no string interpolation)
- Order in the parent `rsx!` block determines `<head>` injection order, which determines CSS cascade
- Same `document::Link` element type for both icons and stylesheets — distinguished only by `rel`

### Component Function Signature
**Source:** `src/app.rs` lines 8–9, `src/components/hero.rs` lines 5–6
**Apply to:** `src/components/hero.rs` (rewritten body, signature unchanged)
```rust
#[component]
pub fn ComponentName() -> Element {
    rsx! { ... }
}
```
- Always `#[component]` macro (CLAUDE.md constraint)
- Always `PascalCase`
- Always returns `Element`
- No `cx` / `Scope` / `use_state` (Dioxus 0.6 APIs forbidden in 0.7)
- `pub` for components consumed across modules (`Hero` is reached via `crate::components::Hero`)

### Module Import Convention
**Source:** `src/app.rs` line 1, `src/components/hero.rs` line 1
**Apply to:** any `.rs` file Phase 2 touches (only `hero.rs` and `app.rs` in this phase)
```rust
use dioxus::prelude::*;
```
The only import needed for standard component authoring. Brings in `Asset`, `Element`, `asset!`, `rsx!`, `document::Link`, `#[component]`.

### Verbatim CSS Port with Attribution Header (NEW pattern this phase introduces)
**Source:** RESEARCH.md Pattern 1 (no Rust precedent — first verbatim port in the project)
**Apply to:** `assets/design-tokens.css`, `assets/warp-ih.css`
```bash
{
  printf '/* Source: %s\n * Copied: %s (Phase 2)\n * DO NOT EDIT — re-sync from source if upstream changes.\n */\n' \
    "<source path>" "$(date -u +%F)"
  cat <source path>
} > assets/<dest>.css
```
- 4-line attribution header prepended (lines 1–4)
- Source file content concatenated unchanged below the header
- Byte-identity verifiable via `tail -n +5 dest | cmp - source`
- LF-only, UTF-8 no-BOM (Pitfall 3 — encoding drift)
- No edits to source bytes; any token issue gets fixed upstream in `warp2ironhermes/`

### Cascade-Aware `<link>` Ordering (NEW pattern this phase introduces)
**Source:** RESEARCH.md Pattern 2
**Apply to:** `src/app.rs` `App` component
**Order (top to bottom in `rsx!`):**
1. `FAVICON` (icon, not stylesheet)
2. `TAILWIND_CSS` (preflight loses cascade — see Pitfall 1)
3. `MAIN_CSS` (project-specific base reset)
4. `DESIGN_TOKENS_CSS` (ANSI palette, fonts, body styles)
5. `WARP_IH_CSS` (Warp-shell-specific tokens + `.wh-*` classes — last-wins for `--w-bg-*` overrides)

**Why:** Tailwind v4 preflight applies `body { font-family: theme(font.sans); }`. If Tailwind loads after `design-tokens.css`, preflight overrides Ioskeley Mono and SC-1 fails. **Deviation from CONTEXT.md D-01 — planner must surface during plan-discuss for user sign-off** (RESEARCH.md Open Question 2).

---

## Cross-Reference: CONTEXT.md/RESEARCH.md → File → Pattern

| Decision/Req | File | Pattern Applied |
|--------------|------|-----------------|
| D-01 (link order — but cascade-deviated per RESEARCH.md) | `src/app.rs` | Cascade-Aware `<link>` Ordering |
| D-02 (no `@import`, no concat) | `assets/design-tokens.css`, `assets/warp-ih.css` | Verbatim CSS Port |
| D-03 (Hero brand stub) | `src/components/hero.rs` | Component Function Signature + `<img>` rendering |
| D-04 (no preview page) | n/a | (no file created — DevTools verification only) |
| D-05 (16 fonts, 16 `@font-face`) | `assets/fonts/`, `assets/design-tokens.css` | mechanical `cp` + verbatim port |
| D-06 (relative font URLs) | `assets/design-tokens.css` | Verbatim CSS Port (no `asset!()` wrapping) |
| D-07 (Tailwind stays) | `src/app.rs` | existing `TAILWIND_CSS` const + `document::Link` retained |
| DS-01 (fonts) | `assets/fonts/`, `assets/design-tokens.css` | bulk `cp` + verbatim port |
| DS-02 (design tokens) | `assets/design-tokens.css` | Verbatim CSS Port |
| DS-03 (warp-ih layout) | `assets/warp-ih.css` | Verbatim CSS Port |
| DS-04 (brand assets + main.css) | `assets/wordmark.svg`, `assets/ih-shield.png`, `assets/main.css`, `src/components/hero.rs` | `cp` + Asset Constant + `<img>` rendering + main.css rewrite |

---

## Metadata

**Analog search scope:**
- `src/app.rs` — current Phase 1 module (asset consts + `document::Link`)
- `src/components/hero.rs` — current Phase 1 component (asset const + `<img>`)
- `src/main.rs` — entry point (no changes this phase)
- `assets/main.css` — current 39-line scaffold (full rewrite target)
- `Dioxus.toml` — pipeline config (no changes this phase)
- `.planning/phases/01-hygiene/01-PATTERNS.md` — Phase 1 pattern map (style reference for output format)

**Files scanned:** 8 (8 read in parallel — 6 source files + 2 phase-context docs)

**Pattern extraction date:** 2026-05-02

**Key insight for planner:** Phase 2 has only **two source-edit files** (`src/app.rs`, `src/components/hero.rs`) and **five mechanical-copy operations** (2 CSS verbatim ports, 2 brand assets, 1 fonts directory). The Rust patterns are direct extensions of Phase 1 (one new asset const each in two existing files; one stylesheet `document::Link` per new CSS file; full-body rewrite of Hero preserving signature). The non-Rust work is byte-stable copy operations with attribution headers.

**Cascade-order deviation flag:** RESEARCH.md recommends Tailwind-first cascade (deviating from CONTEXT.md D-01). Planner MUST surface this in plan-discuss before locking the `document::Link` order in `src/app.rs`. Both options work; the choice is stylistic (one extra `@layer base` block in `main.css` if D-01 is honored verbatim).
