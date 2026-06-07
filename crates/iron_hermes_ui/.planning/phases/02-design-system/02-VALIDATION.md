---
phase: 2
slug: design-system
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-05-02
---

# Phase 2 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | None configured (out-of-scope for v1 per REQUIREMENTS.md TEST-01..03) |
| **Config file** | none |
| **Quick run command** | `cargo build --features web` |
| **Full suite command** | `cargo build --features web && cargo build --features desktop && cargo build --features mobile` |
| **Estimated runtime** | ~30–60 seconds (clean), ~3–10 seconds (incremental) |

---

## Sampling Rate

- **After every task commit:** Run `cargo build --features web` (catches Rust compile breaks from `app.rs` / `hero.rs` edits)
- **After every plan wave:** Run full suite (all three platforms)
- **Before `/gsd-verify-work`:** Full suite green AND smoke checks for byte-identical CSS pass AND manual UAT in browser confirms SC-1..SC-4
- **Max feedback latency:** ≤60 seconds (cold), ≤10s (incremental)

---

## Per-Task Verification Map

> Task IDs are placeholders pending the planner. The mapping below is per-requirement; the planner will assign concrete `{N}-{plan}-{task}` IDs.

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 2-XX-01 | XX | 1 | DS-01 | — | N/A | smoke | `ls assets/fonts/*.woff2 \| wc -l` (== 16) | ❌ W0 | ⬜ pending |
| 2-XX-02 | XX | 1 | DS-01 | — | N/A | smoke | `grep -c '@font-face' assets/design-tokens.css` (== 16) | ❌ W0 | ⬜ pending |
| 2-XX-03 | XX | 1 | DS-02 | — | N/A | smoke | `tail -n +5 assets/design-tokens.css \| cmp - warp2ironhermes/project/ironhermes/colors_and_type.css` (exit 0) | ❌ W0 | ⬜ pending |
| 2-XX-04 | XX | 1 | DS-03 | — | N/A | smoke | `tail -n +5 assets/warp-ih.css \| cmp - warp2ironhermes/project/styles/warp-ih.css` (exit 0) | ❌ W0 | ⬜ pending |
| 2-XX-05 | XX | 1 | DS-03 | — | N/A | smoke | `grep -c '\.wh-' assets/warp-ih.css` (>= 50) | ❌ W0 | ⬜ pending |
| 2-XX-06 | XX | 2 | DS-04 | — | N/A | smoke | `test -f assets/wordmark.svg && test -f assets/ih-shield.png` | ❌ W0 | ⬜ pending |
| 2-XX-07 | XX | 2 | DS-04 | — | N/A | smoke | `! test -f assets/header.svg` (scaffold removed) | ❌ W0 | ⬜ pending |
| 2-XX-08 | XX | 2 | DS-01..04 | — | N/A | compile | `cargo build --features web` | ❌ W0 | ⬜ pending |
| 2-PHASE-GATE | gate | 3 | DS-01..04 | — | N/A | compile | `cargo build --features web && cargo build --features desktop && cargo build --features mobile` | gate | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `assets/fonts/` directory created (`mkdir -p assets/fonts/`) and 16 `.woff2` files copied from `warp2ironhermes/project/ironhermes/fonts/` (DS-01)
- [ ] `assets/design-tokens.css` — verbatim copy of `warp2ironhermes/project/ironhermes/colors_and_type.css` with 4-line attribution header (DS-01, DS-02)
- [ ] `assets/warp-ih.css` — verbatim copy of `warp2ironhermes/project/styles/warp-ih.css` with 4-line attribution header (DS-03)
- [ ] `assets/wordmark.svg` — copied from `warp2ironhermes/project/ironhermes/assets/wordmark.svg` (DS-04)
- [ ] `assets/ih-shield.png` — copied from `warp2ironhermes/project/ironhermes/assets/ih-shield.png` (DS-04)
- [ ] `assets/main.css` — rewritten from 39-line scaffold to ~10-line brand-stub base (DS-04)
- [ ] `assets/header.svg` — deleted (replaced by wordmark + shield) (DS-04)

No external test framework install needed — Phase 2 success is verified via `cargo build` + `grep` / `cmp` smoke checks plus a 5-step manual UAT in the browser.

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Body text renders in Ioskeley Mono in the live app | DS-01, SC-1 | Requires running browser; computed-style verification | 1. `dx serve --features web` 2. DevTools → Inspect `<body>` → Computed → `font-family` starts with `"Ioskeley Mono"` 3. Network tab shows woff2 request returns 200 |
| CSS custom properties resolve to expected values | DS-02, SC-2 | Requires browser DOM | DevTools Console: `const cs = getComputedStyle(document.documentElement); console.assert(cs.getPropertyValue('--accent-primary').trim() === '#4ec9b0'); console.assert(cs.getPropertyValue('--brand').trim() === '#f0883e'); console.assert(cs.getPropertyValue('--font-mono').includes('Ioskeley Mono')); console.assert(cs.getPropertyValue('--w-radius-block').trim() === '6px');` — all four asserts pass silently |
| Warp shell classes are loaded into the active stylesheet | DS-03, SC-3 | Requires browser styleSheets API | DevTools Console: `[...document.styleSheets].some(s => [...s.cssRules].some(r => r.selectorText && r.selectorText.includes('.wh-app')))` returns `true` |
| Brand assets render visually | DS-04, SC-4 | Visual confirmation | Visual: wordmark visible at top center, shield below it, both on dark `--bg` background. Console: `[...document.images].every(i => i.naturalWidth > 0)` returns `true` |
| All three platform features compile cleanly from a fresh `cargo clean` | DS-01..04 | Cold build is slow; gate-only check | `cargo clean && cargo build --features web && cargo build --features desktop && cargo build --features mobile` |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references (`assets/fonts/`, `assets/design-tokens.css`, `assets/warp-ih.css`, brand images, scaffold deletion)
- [ ] No watch-mode flags (`dx serve` is manual gate only)
- [ ] Feedback latency < 60s (cold) / 10s (incremental)
- [ ] `nyquist_compliant: true` set in frontmatter once planner has populated all task IDs

**Approval:** pending
