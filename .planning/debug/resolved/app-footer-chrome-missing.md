---
status: diagnosed
trigger: "AppFooter chrome (bottom-left NODE/SCREEN/AGENT and bottom-right MEM/SKILLS/P50/UTC-clock) missing on page load. Test 6 reports footer toggle in Tweaks panel also doesn't fire."
created: 2026-05-14T00:00:00Z
updated: 2026-05-14T00:00:00Z
---

## Current Focus

reasoning_checkpoint:
  hypothesis: "The `.app-footer` CSS class is rendered into the DOM but the CSS rule that positions it (position: fixed; left: 0; right: 0; bottom: 0; height: 30px; with background/border) is COMPLETELY ABSENT from every CSS file loaded by the app. Plan 03 SUMMARY claims `site.css` was 'copied verbatim from the design bundle by Plan 01' — that copy was incomplete. The footer renders as an unstyled inline block somewhere in document flow (likely behind the wheel column or screen content), invisible to the user."
  confirming_evidence:
    - "site.css ends at line 223 with .btn--ghost rules. grep for 'app-footer|no-footer' returns ZERO matches across ALL 10 CSS files in assets/ (site.css, tokens.css, wheel.css, screens.css, components.css, design-tokens.css, main.css, scanner-anim.css, warp-ih.css, tailwind.css)."
    - "Canonical design source `crates/iron_hermes_ui/filestoimport/ironhermes-design-system/project/app.html` lines 78-97 contain the FULL `.app-footer { position: fixed; left: 0; right: 0; bottom: 0; height: 30px; ... }` rule + .app-footer-right + .app-footer .v + .app-footer .sep — and lines 102-108 contain the body.no-* toggle rules (`body.no-footer .app-footer { display: none }` etc). None of these CSS rules made it into site.css."
    - "AppFooter component (`app_footer.rs` lines 43-60) DOES render the correct DOM: `<div class='app-footer'>...<div class='app-footer-right'>...</div></div>` — markup is correct."
    - "AppFooter IS mounted in hermes_app/mod.rs line 301 inside HermesApp's rsx body, and `pub mod app_footer;` is declared on line 26 — mount is correct."
    - "UiPrefs default has `footer: true` (ui_prefs.rs line 72, asserted by unit test at line 246) — so theme_effects sets `body.no-footer` to false on load. Even if it were inverted, with NO CSS rule for `body.no-footer .app-footer` the toggle would produce no visible effect — confirming the cross-reference: 'Test 6 reports FOOTER toggle also doesn't fire even when scanlines/breadcrumb/rail toggles silently fail too' has a CSS-layer explanation: NONE of `body.no-scanlines`, `body.no-breadcrumb`, `body.no-footer`, `body.has-rail` rules exist in any CSS file."
  falsification_test: "If I `grep -n '\\.app-footer' crates/iron_hermes_ui/assets/site.css` and it returns ANY match, this hypothesis is wrong. (Verified: zero matches.)"
  fix_rationale: "Adding `.app-footer`, `.app-footer .v`, `.app-footer .sep`, `.app-footer-right` and `.breadcrumb`, `.breadcrumb .dot`, `.breadcrumb .v`, `.breadcrumb .sep`, `.sys-meta`, `.sys-meta .v`, plus the body-class hide/show rules from app.html lines 42-112 to site.css addresses the root cause directly. The fix is purely additive CSS — no Rust/Dioxus changes needed."
  blind_spots: "I have not actually visually inspected the rendered DOM in a browser to confirm the `<div class='app-footer'>` is present-but-invisible vs absent. The footer could theoretically be in document flow somewhere and the user may scroll to find it. But since `body { overflow: hidden }` is set (site.css line 56), and the canonical positioning was `position: fixed; bottom: 0`, the unstyled flow position is almost certainly off-screen, behind the wheel/screen z-index layers, or zero-sized due to empty children + no flex."

## Symptoms

expected: After page load, NODE/SCREEN/AGENT visible bottom-left and MEM/SKILLS/P50/UTC-clock visible bottom-right; UTC clock ticks every second.
actual: User: "I don't see the bottom left or bottom right bread crumbs, but everything else is there"
errors: (none reported; silent failure — DOM renders, no CSS rules to position/style)
reproduction: Load web UI in browser; observe bottom edge of viewport.
started: After Phase 26.2.1 deployment. Was never working — Plan 01 CSS copy was incomplete.

## Eliminated

- hypothesis: "AppFooter not mounted in mod.rs"
  evidence: "mod.rs line 301: `app_footer::AppFooter {}` is present inside rsx! body; line 26: `pub mod app_footer;` declared."
  timestamp: 2026-05-14T00:00:00Z

- hypothesis: "UiPrefs.footer defaults to false"
  evidence: "ui_prefs.rs line 72: `footer: true,` in `impl Default`. Unit test at line 246: `assert!(p.footer)` passes."
  timestamp: 2026-05-14T00:00:00Z

- hypothesis: "theme_effects.rs has inverted body.no-footer toggle"
  evidence: "theme_effects.rs line 82: `let _ = list.toggle_with_force(\"no-footer\", !p.footer);` — when p.footer=true (default), this sets no-footer to false (NOT hidden). Logic is correct."
  timestamp: 2026-05-14T00:00:00Z

- hypothesis: "AppFooter component returns empty rsx"
  evidence: "app_footer.rs lines 43-60 render full markup unconditionally with no conditional branching. No `is_active` prop, no signal-read failure path."
  timestamp: 2026-05-14T00:00:00Z

- hypothesis: "AppFooter rendering but behind another z-index layer"
  evidence: "Partially true — but the deeper root cause is the missing CSS. With no `position: fixed; z-index: 50` rule, the div flows inline in document order at the bottom of HermesApp's rsx body, after the .app div with screens. Screens have `padding: 60px` bottom (screens.css line 19) and could cover it; the wheel SVG could overlap. But this is downstream from the missing CSS — fixing the CSS makes the z-index ordering correct."
  timestamp: 2026-05-14T00:00:00Z

## Evidence

- timestamp: 2026-05-14T00:00:00Z
  checked: "ls /assets/*.css — all CSS files in crate"
  found: "10 CSS files: site.css (223 lines), tokens.css (321), wheel.css (249), screens.css (845), components.css (453), design-tokens.css (245), main.css (69), scanner-anim.css (40), warp-ih.css (822), tailwind.css (516). Only first 5 are loaded for the default (non-legacy) shell per app.rs lines 12-17, 55-59. Legacy-shell loads 5 more (tailwind/main/design-tokens/warp-ih/scanner-anim) — gated behind `#[cfg(feature = \"legacy-shell\")]`."
  implication: "The default build only sees: tokens.css, site.css, wheel.css, screens.css, components.css."

- timestamp: 2026-05-14T00:00:00Z
  checked: "grep -rn 'app-footer|no-footer' across crates/iron_hermes_ui/assets/"
  found: "ZERO matches in any CSS file. Not in site.css, not in tokens.css, wheel.css, screens.css, components.css. Also not in legacy-only main.css/warp-ih.css/tailwind.css/scanner-anim.css/design-tokens.css."
  implication: "The `.app-footer` CSS class has no styling rules anywhere. The element renders unstyled."

- timestamp: 2026-05-14T00:00:00Z
  checked: "grep for '\\.breadcrumb|\\.sys-meta|no-scanlines|no-breadcrumb|has-rail' in assets/"
  found: "ZERO matches for `.breadcrumb`, `.sys-meta`, `body.no-scanlines`, `body.no-breadcrumb`, `body.no-footer`, `body.has-rail`, `body.density-dense .screen` — these CSS rules are entirely absent. Only one stray match in screens.css line 19 (a comment referencing 'top breadcrumb/sys-meta')."
  implication: "MULTIPLE chrome rules are missing — but breadcrumb/sys-meta happen to render acceptably without CSS (they flow inline as block divs at the top of HermesApp's rsx output, looking 'mostly OK' due to body's font-family + the spans inheriting text. AppFooter flows similarly but at a position where it gets covered by screen content. This ALSO explains why Test 6 reports SCANLINES/BREADCRUMB/FOOTER/RAIL toggles 'do not work' — there are NO CSS rules for the toggle classes to drive."

- timestamp: 2026-05-14T00:00:00Z
  checked: "crates/iron_hermes_ui/filestoimport/ironhermes-design-system/project/app.html (canonical design source)"
  found: "Lines 42-112 contain the FULL CSS for `.breadcrumb`, `.sys-meta`, `.app-footer`, `.app-footer-right`, and the body-class hide/show toggles (`body.no-scanlines .scanlines { display: none }`, `body.no-breadcrumb .breadcrumb, body.no-breadcrumb .sys-meta { display: none }`, `body.no-footer .app-footer { display: none }`, `body.has-rail #wheel-rail { display: flex !important }`, `body.density-dense .screen { padding-top: 70px; gap: 16px }`)."
  implication: "Plan 03 SUMMARY claim (line 114) that site.css was 'copied verbatim from the design bundle by Plan 01' is FALSE. Plan 01 dropped ~70 lines of critical CSS during the copy. The Plan 03 SUMMARY (lines 113-127) discusses HudChrome and notes 'Plan 05 will add body-class toggle effects (...) that hide individual chrome elements via CSS rules in site.css — no DOM churn, the elements stay mounted.' — but the CSS rules those toggles depend on were never copied in."

- timestamp: 2026-05-14T00:00:00Z
  checked: "AppFooter mount + component implementation"
  found: "mod.rs:26 declares pub mod app_footer. mod.rs:301 mounts `app_footer::AppFooter {}` in HermesApp rsx body. app_footer.rs:43-60 emits the correct DOM (div.app-footer wrapping NODE/SCREEN/AGENT spans + div.app-footer-right wrapping MEM/SKILLS/P50/clock). Clock signal is wired via use_future + gloo_timers."
  implication: "DOM is correct. Mount is correct. The failure is purely at the CSS layer."

- timestamp: 2026-05-14T00:00:00Z
  checked: "UiPrefs::default + theme_effects body-class toggle direction"
  found: "ui_prefs.rs:72 `footer: true`. theme_effects.rs:82 `list.toggle_with_force(\"no-footer\", !p.footer)` → with footer=true, sets no-footer=false. Symmetric correct logic for all four toggles."
  implication: "On page load body class list does NOT include no-footer. So even if `.no-footer .app-footer { display:none }` rule existed (it doesn't), the footer would NOT be hidden by it. This rules out the 'hydration writes wrong default' theory."

## Resolution

root_cause: "Phase 26.2.1 Plan 01 incompletely copied `site.css` from the canonical design source (`filestoimport/ironhermes-design-system/project/app.html` lines 42-112). Approximately 70 lines of critical CSS — including the entire `.app-footer { position: fixed; bottom: 0; ... }` rule, the `.breadcrumb` / `.sys-meta` positioning rules, and ALL six `body.no-*` / `body.has-rail` / `body.density-dense` toggle rules — were dropped during the copy. The AppFooter Rust component (`app_footer.rs`) renders the correct DOM and is correctly mounted in HermesApp's rsx body, but the `<div class='app-footer'>` element has no CSS rules to position it (`position: fixed; left:0; right:0; bottom:0; height:30px;`), background, border, padding, or flex layout — so it renders as an invisible/zero-impact inline div in document flow at the bottom of HermesApp's rsx output. Combined with `body { overflow: hidden }` (site.css:56), the wheel SVG and screen containers cover the area, making the footer effectively unviewable. This same missing-CSS bug explains the cross-reference in Test 6: the FOOTER / SCANLINES / BREADCRUMB / RAIL Tweaks-panel toggles appear silent because the CSS rules they toggle (`body.no-footer .app-footer { display: none }`, etc.) do not exist in any loaded CSS file."
fix: "Append the missing CSS rules to `crates/iron_hermes_ui/assets/site.css` — copy lines 42-112 from `crates/iron_hermes_ui/filestoimport/ironhermes-design-system/project/app.html` (the `.breadcrumb`, `.sys-meta`, `.app-footer`, `.app-footer-right`, `.scanlines opacity`, `.hud-grid opacity` overrides, and the 6 `body.no-*` / `body.has-rail` / `body.density-dense` toggle rules). No Rust changes required. After the CSS additions, the AppFooter chrome will appear pinned to the viewport bottom, and the cross-reference symptom in Test 6 (FOOTER / SCANLINES / BREADCRUMB / RAIL toggles silent) will also resolve because the body-class effects in theme_effects.rs already toggle the classes correctly — they just had no CSS rules to drive visible changes."
verification: "(pending — diagnose-only mode; fix and verify deferred to plan-phase)"
files_changed: []
