---
status: complete
phase: 02-design-system
source: [02-01-SUMMARY.md, 02-02-SUMMARY.md, 02-03-SUMMARY.md, 02-04-SUMMARY.md]
started: 2026-05-03T00:00:00Z
updated: 2026-05-03T00:00:00Z
---

## Current Test

[testing complete]

## Tests

### 1. Cold Start Smoke Test
expected: Run `dx serve --features web` from project root. Server boots without errors, Tailwind compiles, the printed local URL loads in a browser, the page renders on a dark background with no console errors.
result: pass

### 2. Brand Stub Layout
expected: Page shows the IronHermes wordmark (orange/amber, ~32px tall) above the IronHermes shield (~96px tall), both horizontally centered, with ~24px gap between them. The pair sits centered vertically in a full-height (100vh) dark viewport.
result: pass

### 3. Ioskeley Mono Renders (DS-01)
expected: In DevTools, computed `font-family` on `<body>` starts with `"Ioskeley Mono"`. Network tab shows `IoskeleyMono-Regular.woff2` returning HTTP 200. Any visible text on the page renders in the monospace IoskeleyMono face (not a system sans-serif fallback).
result: pass

### 4. CSS Design Tokens Resolve (DS-02)
expected: In DevTools console, `getComputedStyle(document.documentElement).getPropertyValue('--accent-primary').trim()` returns `#4ec9b0`. `--brand` returns `#f0883e`. `--w-radius-block` returns `6px`. `--font-mono` includes `"Ioskeley Mono"`.
result: pass

### 5. Warp Shell CSS Loaded (DS-03)
expected: In DevTools console, `[...document.styleSheets].some(s => [...s.cssRules].some(r => r.selectorText && r.selectorText.includes('.wh-app')))` returns `true`. The `warp-ih.css` stylesheet is reachable from the document.
result: pass

### 6. Brand Assets Render (DS-04)
expected: In DevTools console, `[...document.images].every(i => i.naturalWidth > 0)` returns `true`. Both `/assets/wordmark.svg` and `/assets/ih-shield.png` return HTTP 200 in the Network tab. The shield PNG is not stretched or distorted; the wordmark SVG renders crisply at all zoom levels.
result: pass

## Summary

total: 6
passed: 6
issues: 0
pending: 0
skipped: 0
blocked: 0

## Gaps

[none yet]
