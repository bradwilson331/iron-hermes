---
phase: 02
slug: design-system
status: verified
threats_open: 0
asvs_level: 1
created: 2026-05-03
---

# Phase 02 — Security

> Per-phase security contract: threat register, accepted risks, and audit trail.

---

## Trust Boundaries

| Boundary | Description | Data Crossing |
|----------|-------------|---------------|
| Same-origin static asset fetch | Browser fetches `/assets/fonts/*.woff2`, `/assets/design-tokens.css`, `/assets/warp-ih.css`, `/assets/main.css`, `/assets/wordmark.svg`, `/assets/ih-shield.png` from the same origin as the app HTML. No third-party CDN. | Public design tokens, fonts, brand images |
| Wired CSS/image references via `asset!()` macro | Compile-time path resolution — build fails if a referenced asset is missing. | Compile-time string paths |
| `dx serve` dev origin | Local HTTP server (typically `localhost:8080`); not exposed to public internet during Phase 2 verification. | Dev artifacts (`target/`, `assets/`) |

---

## Threat Register

| Threat ID | Category | Component | Disposition | Mitigation | Status |
|-----------|----------|-----------|-------------|------------|--------|
| T-02-01 | T (Tampering) | Vendored woff2 files in `assets/fonts/` | accept | Same-origin only; no third-party font CDN. Supply-chain covered by repo review. ASVS V2-V6 N/A (static-asset-only phase). | closed |
| T-02-02 | I (Information disclosure) | `assets/design-tokens.css` and font files | accept | Public design tokens; no secrets. CSS is intentionally world-readable in any deployed web app. | closed |
| T-02-03 | T (Tampering) | Vendored layout CSS (`assets/warp-ih.css`) | accept | Same-origin only; no remote stylesheet links. Pure layout/typography rules — no selectors target `iframe`/`script`. | closed |
| T-02-04 | T (Tampering) | Vendored brand images | accept | Same-origin only; no remote image links. Repo-review supply-chain. | closed |
| T-02-05 | I (SVG `<script>` injection) | `assets/wordmark.svg` | accept | Rendered via `<img src=...>` (Pattern 3), not inline-SVG injection. Browser sandboxes `<script>` tags inside `<img>`-loaded SVGs. SVG sourced from trusted prototype handoff. | closed |
| T-02-06 | T (Tampering) | Wired CSS + image references via `asset!()` macro in `src/app.rs` and `src/components/hero.rs` | accept | Compile-time `asset!()` paths fail the build if a referenced file is missing. Same-origin only. ASVS V2-V6 N/A (no auth, no input handling, no secrets). | closed |
| T-02-07 | I (Information disclosure via DevTools UAT) | Local `dx serve` exposes `target/` and `assets/` over HTTP | accept | Dev-only, localhost-bound; not deployed during this phase. PROJECT.md "No external services" constraint precludes any production exposure in v1. | closed |

*Status: open · closed*
*Disposition: mitigate (implementation required) · accept (documented risk) · transfer (third-party)*

---

## Accepted Risks Log

| Risk ID | Threat Ref | Rationale | Accepted By | Date |
|---------|------------|-----------|-------------|------|
| AR-02-01 | T-02-01 | Same-origin static woff2 vendoring; no third-party CDN; supply-chain covered by repo/Cargo review. | Phase-2 planner (PLAN 02-01 `<threat_model>`) | 2026-05-03 |
| AR-02-02 | T-02-02 | Public design tokens (ANSI palette, type scale) are intentionally world-readable in CSS. No secrets present. | Phase-2 planner (PLAN 02-01 `<threat_model>`) | 2026-05-03 |
| AR-02-03 | T-02-03 | Layout CSS contains no `iframe`/`script` selectors; same-origin static fetch only. | Phase-2 planner (PLAN 02-02 `<threat_model>`) | 2026-05-03 |
| AR-02-04 | T-02-04 | Brand images (wordmark.svg, ih-shield.png) vendored same-origin from prototype handoff. | Phase-2 planner (PLAN 02-03 `<threat_model>`) | 2026-05-03 |
| AR-02-05 | T-02-05 | SVG renders via `<img src=...>` only — browser sandboxing prevents `<script>` execution. | Phase-2 planner (PLAN 02-03 `<threat_model>`) | 2026-05-03 |
| AR-02-06 | T-02-06 | Asset path validity is enforced at compile time by Dioxus `asset!()` macro. No remote/third-party references. | Phase-2 planner (PLAN 02-04 `<threat_model>`) | 2026-05-03 |
| AR-02-07 | T-02-07 | `dx serve` is a dev-only localhost server. PROJECT.md v1 "no external services" constraint precludes production exposure. | Phase-2 planner (PLAN 02-04 `<threat_model>`) | 2026-05-03 |

*Accepted risks do not resurface in future audit runs.*

---

## Security Audit Trail

| Audit Date | Threats Total | Closed | Open | Run By |
|------------|---------------|--------|------|--------|
| 2026-05-03 | 7 | 7 | 0 | /gsd-secure-phase 2 (orchestrator) |

---

## Sign-Off

- [x] All threats have a disposition (mitigate / accept / transfer) — all 7 are `accept`
- [x] Accepted risks documented in Accepted Risks Log — AR-02-01..AR-02-07
- [x] `threats_open: 0` confirmed
- [x] `status: verified` set in frontmatter

**Approval:** verified 2026-05-03
