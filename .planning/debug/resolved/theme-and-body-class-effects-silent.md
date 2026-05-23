---
status: diagnosed
trigger: "Cycling all 5 themes does not work. AccentColors work. Wheel slider scale works. Toggling SCANLINES/BREADCRUMB/FOOTER/RAIL do not work."
created: 2026-05-14
updated: 2026-05-14
---

## Current Focus

reasoning_checkpoint:
  hypothesis: "Effects 1 and 2 ARE firing and writing to the DOM correctly. They appear broken because the CSS rules that would make them visible never made it into the production stylesheets. Effect 1 writes `<html data-theme=...>` and Effect 2 writes body class toggles, but the visible chrome rendered by hermes_app/* consumes `--teal/--bg/--text` from `site.css :root` (which is NEVER reassigned per theme) and the production CSS contains ZERO `body.no-scanlines / body.no-breadcrumb / body.no-footer / body.has-rail / body.density-dense / body.on-chat` selectors. Effect 3 works because it directly mutates the `--teal/--teal-bright` custom properties on `<html>` — the same vars consumed across 30+ rules in site.css, screens.css, wheel.css."
  confirming_evidence:
    - "tokens.css lines 78-249 define `[data-theme=...]` rules — but they only override --accent/--bg-canvas/--fg/--bg-elevated tokens (NEW tokens, only consumed by components.css ih-btn/ih-badge/etc.)."
    - "Grep confirms: components.css ih-btn ih-badge classes are NOT used anywhere under crates/iron_hermes_ui/src/components/hermes_app/. Zero matches for `ih-btn` / `ih-badge` in the new shell. The new shell renders .scanlines / .breadcrumb / .app-footer / .wheel-shell / .tweaks-fab / .tweaks — all styled by site.css / wheel.css / screens.css legacy classes."
    - "Grep confirms: ZERO occurrences of `body.no-scanlines`, `body.no-breadcrumb`, `body.no-footer`, `body.has-rail`, `body.density-dense`, `body.on-chat` in any of {components.css, screens.css, wheel.css, site.css, design-tokens.css, main.css, scanner-anim.css}. These selectors exist ONLY in `crates/iron_hermes_ui/filestoimport/ironhermes-design-system/project/app.html` (the prototype HTML, lines 102-112, 315-316, 320) — they were never ported to the production CSS bundle."
    - "site.css `:root` (lines 7-40) declares `--teal`, `--teal-bright`, `--bg`, `--bg-deep`, `--bg-card`, `--text`, `--gray`, etc. — but NO `[data-theme=...]` selector anywhere in site.css reassigns them. So setting `<html data-theme=iron-dark>` cannot change these vars; only the unused tokens.css tokens flip."
    - "Effect 3 writes `--teal` and `--teal-bright` directly as inline style on `<html>` (inline style has highest cascade priority short of !important). 30+ rules in site.css/screens.css/wheel.css use `var(--teal)`. So accent change cascades immediately, regardless of `[data-theme]`."
    - "Effects 1 and 2 register via `use_effect` and read `theme.read()` / `prefs.read()` / `active_screen.read()` BEFORE the `#[cfg(target_arch = wasm32)]` block — subscription registration is correct. Effect 3 follows the identical pattern. So all three effects DO fire on signal write; the difference is purely whether the DOM mutation has any visible CSS consequence."
  falsification_test: "Open DevTools after clicking a theme button. If `<html>` shows `data-theme=iron-dark` attribute set AND `<body>` shows the right class toggles BUT the screen looks identical, the hypothesis is confirmed (effects fire, CSS missing). If neither attribute nor classes appear, the effects truly are not firing and the hypothesis is wrong."
  fix_rationale: "Two missing CSS porting steps. (a) Port the body-class rules from app.html lines 102-112 (and 315-316, 320) into a production CSS file (components.css or site.css). (b) Either (b1) refactor the visible chrome rules in site.css to use the tokens.css tokens (--accent, --bg-canvas, --fg) so they pick up `[data-theme]` overrides, OR (b2) add per-theme `[data-theme=...]` blocks that override the legacy `--teal/--bg/--text/--border` vars in site.css. Both fix the root cause (missing CSS) rather than touching theme_effects.rs (which is correct as-is)."
  blind_spots: "Have not run the WASM build in a browser to physically confirm the attribute appears on <html> after click. Diagnosis is by source-grep + cascade analysis, not by runtime observation. If somehow Effect 1's set_attribute is silently no-op'ing (e.g. document_element() returns Some(non-HTMLHtmlElement)) the conclusion still holds for Effect 2 (body classes have no targeting CSS regardless)."

hypothesis: SEE reasoning_checkpoint above.
test: Static source-grep over all production CSS for `body.no-*` / `body.has-rail` / `body.density-dense` / `[data-theme]` consumer rules.
expecting: ZERO matches for body-class selectors in production CSS; tokens.css [data-theme] rules touch only tokens that the new shell does not visually consume.
next_action: Return ROOT CAUSE FOUND.

## Symptoms

expected: TweaksPanel theme buttons flip palette via <html data-theme=…>; scanlines/breadcrumb/footer/rail toggles add/remove body classes (body.no-*, body.has-rail, body.density-dense); wheel slider scales SVG (works); AccentColor changes --teal var (works).
actual: Cycling all 5 themes does NOT work. SCANLINES/BREADCRUMB/FOOTER/RAIL toggles do NOT work. AccentColors WORK. Wheel slider scale WORKS.
errors: (none — silent failure; no console errors expected because the DOM writes succeed)
reproduction: Open TweaksPanel; click any theme button → no visual palette change. Toggle SCANLINES/BREADCRUMB/FOOTER/RAIL → corresponding chrome element stays visible (or absent for rail).
started: After Plan 26.2.1-05 (ThemeEffects + TweaksPanel) — symptoms have always been present; never worked end-to-end.

## Eliminated

- hypothesis: "Effects 1 and 2 fail to subscribe because the signal read happens inside the #[cfg(target_arch = wasm32)] block, while Effect 3 reads outside it."
  evidence: "Re-read theme_effects.rs lines 44-58, 70-95, 105-122. All three effects read their signals into a local BEFORE the cfg block. Effect 1: `let theme_name = theme.read().clone();` (line 46). Effect 2: `let p = prefs.read().clone(); let screen = *active_screen.read();` (lines 71-72). Effect 3: `let accent = prefs.read().accent;` (line 106). All three follow the identical structural pattern."
  timestamp: 2026-05-14

- hypothesis: "Effect 2 short-circuits silently because `web_sys::window().and_then(|w| w.document()).and_then(|d| d.body())` returns None during hydration."
  evidence: "Effect 1 uses an identical `web_sys::window().and_then(...).and_then(|d| d.document_element())` chain (line 49-52) and writes to `<html>`. Effect 3 uses the same chain plus `.and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())` (lines 110-113) and works. If the window/document chain failed, Effect 3 would not work either."
  timestamp: 2026-05-14

- hypothesis: "ThemeContext newtype wiring is broken — `use_context::<ThemeContext>().0` returns a different Signal than tweaks_panel writes to."
  evidence: "HermesApp::mod.rs line 59 provides `use_context_provider(|| crate::state::ThemeContext(theme))`. Both consumers (tweaks_panel.rs line 30 and theme_effects.rs line 33) read via `use_context::<ThemeContext>().0` (or `use_context::<crate::state::ThemeContext>().0` — same thing). Signal<T> is Copy and ThemeContext derives Copy (state.rs line 782). Same provider, same consumer pattern, identical access path."
  timestamp: 2026-05-14

- hypothesis: "warp_hermes.rs line 877 `data-theme=cyan` clobbers Effect 1's write."
  evidence: "app.rs `root_shell()` is `#[cfg(not(feature = \"legacy-shell\"))]` → mounts HermesApp; the legacy WarpHermes only mounts when `legacy-shell` feature is on. The hardcoded `data-theme=cyan` sits on a `<div class=wh-app>`, not on `<html>`, and the new shell does not mount that component at all."
  timestamp: 2026-05-14

- hypothesis: "Hydration gate aborts theme_effects.rs — ThemeEffects early-returns on `!hydrated`."
  evidence: "theme_effects.rs does NOT consume the `hydrated` signal at all. Only the persistence effects in hermes_app/mod.rs lines 92-112 gate on `hydrated`. The three ThemeEffects effects have no gate; they always run on signal change."
  timestamp: 2026-05-14

## Evidence

- timestamp: 2026-05-14
  checked: "theme_effects.rs full module"
  found: "Three effects, identical structure: read signal-local before cfg block, then mutate DOM. Effect 1 → html.set_attribute(data-theme, theme_name). Effect 2 → body.class_list().toggle_with_force(...) × 6. Effect 3 → html.style().set_property('--teal'/'--teal-bright')."
  implication: "Code-side, all three effects subscribe and fire correctly on signal write. Difference must be in what their DOM writes are CONSUMED by."

- timestamp: 2026-05-14
  checked: "tweaks_panel.rs theme/scanlines/breadcrumb/footer/rail click handlers"
  found: "Theme buttons (line 108): `onclick: move |_| theme.set(theme_name.to_string())`. Chrome toggles (lines 187, 198, 209, 220): `prefs.with_mut(|p| p.scanlines = !p.scanlines)` etc. All writes target the shared context signals."
  implication: "Writes are reaching the same Signal<UiPrefs> / ThemeContext.0 that ThemeEffects subscribes to. No write path is broken."

- timestamp: 2026-05-14
  checked: "hermes_app/mod.rs context provision and child mount order"
  found: "Line 59 provides ThemeContext(theme) via use_context_provider. Line 304 mounts `theme_effects::ThemeEffects {}` and line 305 mounts `tweaks_panel::TweaksPanel {}` — siblings under HermesApp, both inside the same context provider scope."
  implication: "Context wiring is sound. ThemeEffects consumes the same signal TweaksPanel writes to."

- timestamp: 2026-05-14
  checked: "Grep for body class CSS selectors in production stylesheets"
  found: "Grep `no-scanlines|no-breadcrumb|no-footer|has-rail|density-dense|on-chat` over {components,screens,wheel,site,design-tokens,main,scanner-anim}.css → ZERO matches. The selectors only exist in `crates/iron_hermes_ui/filestoimport/ironhermes-design-system/project/app.html` lines 102-112 (prototype embedded styles) and 315-316, 320 (chat rail offset)."
  implication: "Effect 2 IS toggling body classes correctly, but there is no CSS rule that consumes any of them. The chrome elements (.scanlines, .breadcrumb, .app-footer, #wheel-rail, .screen) stay visible at their base CSS settings. This is missing CSS porting from Plan 05, not an effect bug."

- timestamp: 2026-05-14
  checked: "tokens.css full theme rules + which tokens they override"
  found: "tokens.css declares 5 themes via `[data-theme=slate-dark|slate-light|iron-dark|terminal-dark|parchment-light]` (lines 79, 114, 149, 184, 219). Each block overrides ONLY the new design-system tokens: --bg-canvas, --bg-surface, --bg-elevated, --fg, --fg-muted, --fg-dim, --accent, --accent-hover, --border, --border-strong, etc."
  implication: "If `<html data-theme=iron-dark>` IS set by Effect 1, only the new tokens flip. None of the legacy `--teal`/`--bg`/`--text`/`--gray`/`--border` vars defined on `:root` in site.css are touched."

- timestamp: 2026-05-14
  checked: "site.css :root and which vars get reassigned per theme"
  found: "site.css lines 7-40 declare on `:root` the LEGACY var set: --bg, --bg-deep, --bg-card, --bg-card-hover, --bg-input, --border, --border-faint, --border-strong, --teal, --teal-bright, --teal-dim, --teal-faint, --teal-glow, --text, --text-dim, --gray, --gray-faint, --green, --red, --amber, --font. ZERO of these are reassigned by any `[data-theme=...]` selector anywhere in the codebase. They are static across all themes."
  implication: "No matter what `<html data-theme>` is set to, the visible chrome — which is styled exclusively via site.css legacy vars (corner brackets, .scanlines, .scan-bar, .statbar, .readout, .btn, etc.) and via screens.css / wheel.css (also using --teal / --text / --border / --bg-card) — does not change palette. Effect 1 has NO visible consequence."

- timestamp: 2026-05-14
  checked: "Grep for ih-btn / ih-badge / `var(--accent)` consumers in the new shell render tree"
  found: "components.css ih-btn / ih-badge classes are not used anywhere under crates/iron_hermes_ui/src/components/hermes_app/. The new shell renders site.css/screens.css/wheel.css-styled chrome exclusively (`.scanlines`, `.breadcrumb`, `.app-footer`, `.wheel-shell`, `.tweaks-fab`, `.tweaks`, `.screen`, `.panel`, etc.)."
  implication: "The only production rules that ARE keyed off `[data-theme]` (the tokens.css token overrides flowing into components.css and a couple of screens.css rules using `var(--accent)`) target elements the new shell does not even render. So Effect 1's correct DOM write produces NO visible result."

- timestamp: 2026-05-14
  checked: "Grep for `var(--teal)` consumers"
  found: "30+ rules across site.css (corner brackets, scan-bar, statbar, readout, .btn), wheel.css (HUD text, wheel labels, .stub-close, .ih-mono), screens.css (panel-title, badges, toggles, tabs, soul-preview, plat-card, etc.) all read `var(--teal)`."
  implication: "Why Effect 3 works: it writes `--teal` and `--teal-bright` as inline style on <html>, inline-cascading down to every legacy rule. 30+ visible chrome elements re-color instantly. This is the ONLY effect whose output is consumed by visible rules in the new shell."

## Resolution

root_cause: |
  TWO independent missing-CSS bugs, both rooted in Plan 26.2.1-05's CSS port being incomplete. The Rust theme_effects.rs code is correct — all three effects subscribe and fire properly on signal writes. The bugs are:

  (1) Theme picker silent: `<html data-theme="…">` IS being written by Effect 1, but the visible chrome in the new shell (corner brackets, scanlines, breadcrumb, app-footer, wheel HUD, tweaks panel, screens) is styled exclusively via the LEGACY `--teal/--bg/--text/--border` custom properties declared once on `site.css :root`. NO `[data-theme=...]` selector anywhere in the production CSS bundle reassigns those legacy vars. The new tokens.css `[data-theme=...]` rules only override `--bg-canvas/--fg/--accent/--bg-elevated/--border-strong` (new tokens) — consumed only by the `ih-btn`/`ih-badge` design-system components that the new shell does not render. Net result: Effect 1's DOM write succeeds, but cascades into nothing visible.

  (2) Chrome toggles silent: Effect 2's body class writes (`body.no-scanlines`, `body.no-breadcrumb`, `body.no-footer`, `body.has-rail`, `body.density-dense`, `body.on-chat`) succeed, but there is ZERO matching CSS rule in any production stylesheet (components.css / screens.css / wheel.css / site.css / design-tokens.css / main.css / scanner-anim.css). The corresponding rules (`body.no-scanlines .scanlines { display: none }`, etc.) exist only in the prototype `app.html` lines 102-112, 315-316, 320 — they were never ported. Net result: Effect 2's class toggles succeed but cascade into nothing.

  Why Effect 3 (accent) works and is the only one that does: it writes `--teal`/`--teal-bright` directly as inline style on `<html>` — the same legacy custom properties used by 30+ rules across site.css, screens.css, wheel.css. Inline-style cascade re-colors the entire visible chrome instantly.
fix:
verification:
files_changed: []
