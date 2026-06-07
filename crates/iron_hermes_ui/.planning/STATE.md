---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: unknown
stopped_at: Completed 04-04b-PLAN.md
last_updated: "2026-05-03T19:50:45.252Z"
progress:
  total_phases: 6
  completed_phases: 4
  total_plans: 18
  completed_plans: 18
  percent: 100
---

# State: IronHermes

**Project:** IronHermes — Dioxus 0.7 port of the Warp × IronHermes prototype
**Milestone:** v1 (UI port)
**Last updated:** 2026-05-03

---

## Project Reference

**Core value:** The Dioxus implementation is visually indistinguishable from the React prototype — every block, panel, palette, scanner tick, and theme variant matches the source design when rendered side by side.

**Current focus:** Phase 04 — data-layer-interactions

---

## Current Position

Phase: 5
Plan: Not started
| Field | Value |
|-------|-------|
| Current phase | 04 |
| Phase name | Data Layer & Interactions |
| Current plan | 6 of 6 (Plans 04-01 through 04-04b complete; 04-05 is the final integration + UAT plan) |
| Phase status | Wave 4 complete (04-04a + 04-04b both done; three-platform compile gate green + clippy green); Wave 5 (Plan 04-05) is next — WarpHermes full rewire with all signals wired |
| Overall progress | Phases 1/3 verified; Phase 2 implementation-complete pending UAT; Phase 4 Waves 0–4 complete (5/6 plans done) |

```
Phase 1 progress: [██████████] 100% (3/3 plans complete)
Phase 2 progress: [██████████]  Auto tasks 4/4 done — UAT pending
Phase 4 progress: [█████████░]  Wave 4 complete (5/6 plans done)
Phase 1 [✓] Phase 2 [⊙] Phase 3 [✓] Phase 4 [⊙] Phase 5 [ ] Phase 6 [ ]
```

---

## Performance Metrics

| Metric | Value |
|--------|-------|
| Phases defined | 6 |
| Phases complete | 1 verified + Phase 2 implementation-complete pending UAT |
| Requirements mapped | 38/38 |
| Plans written | 13 (Phase 1: 01-01..01-03; Phase 2: 02-01..02-04; Phase 4: 04-01..04-05 + 04-04a/04-04b split) |
| Plans complete | 12 (Phase 1: 3/3; Phase 2: 4/4 auto-task — UAT pending; Phase 4: 5/6 — Wave 4 complete) |
| Phase 04-data-layer-interactions P04a | 240 | 4 tasks | 5 files |
| Phase 04 P04b | ~7min | 5 tasks | 6 files |

### Per-Plan Execution Metrics

| Phase | Plan | Duration | Tasks | Files |
|-------|------|----------|-------|-------|
| 01-hygiene | 03 | 2min | 2 | 1 |
| 02-design-system | 01 | ~3min | 2 | 17 (16 woff2 + 1 CSS) |
| 02-design-system | 02 | ~2min | 1 | 1 |
| 02-design-system | 03 | ~1min | 1 | 2 |
| 02-design-system | 04 | ~2min auto + UAT pending | 5 (4 auto + 1 checkpoint) | 3 (modified) |
| 04-data-layer-interactions | 01 | ~3min | 4 (3 commit + 1 verify) | 3 (1 created + 2 modified) |
| 04-data-layer-interactions | 02 | ~4min | 3 (2 commit + 1 verify) | 1 (modified) |
| 04-data-layer-interactions | 03 | ~6min | 5 (4 mock-file commits + 1 shim/clippy commit) | 8 (4 created + 3 modified + 1 SUMMARY) |
| 04-data-layer-interactions | 04a | ~15min | 3 (all auto) | 5 (1 created + 4 modified) |
| 04-data-layer-interactions | 04b | ~7min | 5 (all auto) | 6 (0 created + 6 modified) |

---

## Accumulated Context

### Key Decisions (Carry Forward)

- Pin `dioxus = "=0.7.1"` — prevents silent breakage from 0.7.x patch releases
- CSS ported as-is from prototype — no Tailwind conversion; visual fidelity is the constraint
- Scanner animation via CSS `@keyframes` — avoids 10 Hz VDOM diffs; GPU compositor handles it
- Separate `DesktopShell` and `MobileShell` components — mirrors prototype structural split
- Mock data only in v1 — all AI and shell responses are faked with prototype-matching timings
- Never use `cx`, `Scope`, or `use_state` — Dioxus 0.6 APIs, removed in 0.7
- Signal borrows must not span `.await` points — clippy.toml enforces this
- Cargo.lock committed for binary crate — standard Rust supply-chain hygiene; ensures reproducible builds across machines and CI (Plan 01-03)
- Three-platform phase gate (web && desktop && mobile) is the canonical Phase 1 verification — catches feature-flag drift before it compounds (Plan 01-03)
- CSS files ported verbatim with 4-line attribution header (Pattern 1) — preserves byte identity vs prototype source-of-truth, enables single-file re-sync from upstream (Plans 02-01, 02-02)
- Static woff2 fonts served from `/assets/fonts/` without `asset!()` wrapping — relative `url("fonts/...")` in CSS resolves correctly without rewriting the verbatim port (D-06, Plan 02-01)
- Cascade-order deviation from CONTEXT.md D-01: Tailwind `<link>` moves to position 2 so v4 preflight loses cascade vs prototype body styles; relative order of three ported CSS files (main → design-tokens → warp-ih) preserved verbatim (Plan 02-04)
- Phase 2 Hero is an intentional brand-stub (wordmark + shield centered) — Phase 3 replaces it entirely with the WarpHermes shell; no investment in named selectors here (D-03, Plan 02-04)
- Phase 4 Wave 0: cfg-gated `timer::sleep(u32)` primitive established — single public signature on every platform; gloo-timers (wasm32) + tokio time-only (native); cfg branches inside fn body (D-04, RESEARCH Pattern 1) (Plan 04-01)
- Phase 4 dev-dependency split: production `[dependencies] tokio` stays at `time` only (D-05); test ergonomics (`#[tokio::test]` macros + rt) live in `[dev-dependencies]` so native binary footprint is unaffected (Plan 04-01)
- Phase 4 desktop tokio time-driver runtime availability proven via `#[tokio::test] desktop_sleep_does_not_panic`; resolves RESEARCH Q1 — no panic-on-first-submit risk in Plan 04-05 desktop UAT (Plan 04-01)
- Phase 4 Wave 1: `Personality` enum derives `Eq + Hash` so Wave 3's `personalities.rs::pick_reply` can use `.find(|(k, _)| *k == p)` on a const `REPLIES` table without HashMap allocation — small fieldless enum makes this the canonical small-cardinality lookup (D-03, Plan 04-02)
- Phase 4 Wave 1: `ShellSettings` derives `Clone + Copy` only (no `PartialEq`/`Debug` — `Signal<T>` doesn't impl `Debug`); `Copy` is required for cheap `use_context::<ShellSettings>()` retrieval in Wave 3/4 consumers (D-02 + RESEARCH Pattern 5, Plan 04-02)
- Phase 4 Wave 1: destructive rename `demo_blocks() -> Vec<Block>` → `demo_block_entries() -> Vec<BlockEntry>` per D-09 with NO compatibility shim; `warp_hermes.rs:6` is the single expected carry-forward breakage that Wave 4 (Plan 04-04a/04-04b) fixes as part of the WarpHermes prop-shape refactor (Plan 04-02)
- Phase 4 Wave 1: `now_time()` cfg gate at function-body level (NOT module level) — `js_sys::Date::new_0()` HH:MM:SS on wasm32, hardcoded `"00:00:00"` on native; mirrors 04-01 `timer::sleep` pattern per RESEARCH Pitfall 4 (D-34, Plan 04-02)
- Phase 4 Wave 3: borrow-then-await discipline (D-06 + clippy.toml `await-holding-invalid-types`) verified at compile time across `run_shell` + `run_agent_steps`; FIRST wave with `.await` — `cargo clippy --features web -- -D warnings` gate now binding for all subsequent Phase 4 work; every `Signal<T>::write()` drops at the `;` before any `.await`, reads cloned into owned locals, `next_id()` (call-as-fn) clones the Copy u64 value (Plan 04-03)
- Phase 4 Wave 3: mock async fn signatures (`run_shell(text, blocks, next_id, _scanner_active)` and `run_agent_steps(prompt, personality, messages)`) match v2 server-fn shapes byte-for-byte per D-36 — v2 swap is impl-only (replace function bodies, not call sites); `_scanner_active` param kept for forward-compat ballast even though Phase 4 calls `pulse_scanner` from `WarpHermes::submit()`, not from `run_shell` (Plan 04-03)
- Phase 4 Wave 3: `REPLIES` strings (six personalities) + `STATUS_TEXT` + `git status` / `cargo` / `ls` body templates are byte-for-byte verbatim from `app.jsx` 25-36 / 309-337 / 339-349; design fidelity is the constraint per CLAUDE.md — strings preserved including the literal `⚡` Unicode in the Hype reply, the `(=^.^=)` ASCII emoticon in the Catgirl reply, and the backticks-around-`crates/...` in the Technical reply (Plan 04-03)
- Phase 4 Wave 3: `warp_hermes.rs` shim (`demo_block_entries().into_iter().map(|e| e.block).collect()` into `Vec<Block>`) bridges Wave 1's destructive rename and Wave 4's prop-shape refactor — TEMPORARY, replaced entirely by Plan 04-04a (Plan 04-03)
- Phase 4 Wave 3: scoped `#[allow(dead_code)]` on Wave-1-introduced-but-Wave-4-consumed symbols (`Personality::label`, `Personality::ALL`, `PaletteState`, `ShellSettings`) — required for the Wave 3 clippy gate to pass since `-D warnings` promotes Wave-1 dead-code warnings to errors; named-consumer comments document the wave each will be wired in (Plan 04-03)
- Phase 4 Wave 4a: `ReadOnlySignal` deprecated in Dioxus 0.7.1 — replaced with `ReadSignal` at compile time because `clippy -- -D warnings` promotes deprecation warnings to errors (Plan 04-04a)
- Phase 4 Wave 4a: `Signal<T>` to `ReadSignal<T>` prop conversion uses direct pass (not `.into()`) because `.into()` is ambiguous across multiple `SuperInto` impls in `dioxus_core` vs `dioxus_stores`; the `#[component]` macro's generated props builder handles the conversion internally (Plan 04-04a)
- Phase 4 Wave 4b: `PaletteState::PersonalityPick` gets targeted `#[allow(dead_code)]` — used in command_palette.rs match arm but never constructed as a value until Plan 04-05 wires the `/personality` transition; narrower than the previous blanket suppression on the whole enum removed by 04-04a (Plan 04-04b)
- Phase 4 Wave 4b: `ReadSignal<T>` (not `ReadOnlySignal<T>`) confirmed as the correct read-only-reactive prop type across all four refactored primitives (InputBox mode, CommandPalette open, StatusBar tokens/scanner_active, AgentPanel messages) — consistent with 04-04a's finding that Dioxus 0.7.1 deprecated ReadOnlySignal (Plan 04-04b)
- Phase 4 Wave 4b: `use_context::<ShellSettings>()` pattern established for cross-cutting personality — StatusBar and AgentPanel both consume personality via use_context instead of prop-drilling; KBD-06 reactivity achieved through Signal<Personality> inside the ShellSettings bundle (Plan 04-04b)

### Critical Constraints (Always Active)

- `warp2ironhermes/` is READ ONLY — consult, never import or build from it
- No API keys, no network calls in v1 — mocks only
- WASM is single-threaded — no `std::thread`; signal borrows must drop before `.await`
- Component functions must be `PascalCase` and annotated `#[component]`

### Prototype Bug: CommandPalette Hook Order

`warp2ironhermes/project/app/shell.jsx` calls `React.useState(0)` after an early `return null` guard on line 215. This violates React's Rules of Hooks. When porting to Dioxus, move all `use_signal` calls ABOVE any early-return guards.

### Prototype Bug: pulseScanner._t Timer

`app.jsx` stores a `setTimeout` ID as a property on the function object — a JavaScript-ism with no Rust equivalent. In Dioxus, hold the task handle in a `use_signal` and cancel it explicitly.

### Todos

- [x] After Phase 1: verify `cargo build --features web`, `cargo build --features desktop`, `cargo build --features mobile` all compile cleanly (Plan 01-03 phase gate, 2026-05-03)
- [ ] **(BLOCKING — open now)** Phase 2 Plan 04 manual UAT: run `dx serve --features web`, walk through SC-1..SC-4 checklist in `.planning/phases/02-design-system/02-04-PLAN.md` Task 5 `<how-to-verify>`. Reply "approved" or paste failing console output.
- [ ] After Phase 2 UAT approval: run `/gsd-verify-work` to close Phase 2.
- [ ] After Phase 3: side-by-side visual review of desktop shell against `warp2ironhermes/project/Warp × IronHermes.html`
- [ ] After Phase 4: exercise every keyboard shortcut and verify timings against the prototype
- [ ] After Phase 5: cycle through all theme/density/block/agent combinations in TweaksPanel
- [ ] After Phase 6: view mobile shell on a narrow viewport and compare to the prototype's iOS frame

### Blockers

- **Phase 2 manual UAT** (Plan 02-04 Task 5, checkpoint:human-verify, blocking gate) — implementation is complete and three-platform `cargo build` is green; user must verify SC-1..SC-4 in a live browser before Phase 2 can close. See `.planning/phases/02-design-system/02-04-SUMMARY.md` "Manual UAT Status" section for instructions.

---

## Session Continuity

**Last session:** 2026-05-03T17:39:55.189Z

**Stopped at:** Completed 04-04b-PLAN.md

**To resume work:** Wave 5 — Plan 04-05 (WarpHermes integration: replaces the expanded shim with full reactive body — 12 use_signals + use_context_provider + global keydown listener + auto-scroll + pulse_scanner/pulse_token + submit/on_rerun/pick handlers + checkpoint:human-verify UAT). Three-platform `cargo build` GREEN and `cargo clippy --features web -- -D warnings` GREEN as of Wave 4 (Plan 04-04b Task 5). All four refactored primitives (InputBox, CommandPalette, StatusBar, AgentPanel) plus the three from 04-04a (markdown, block, block_stream) type-check against the expanded shim. Phase 2 manual UAT remains an open blocker (independent track) — see Blockers section.

**Source of truth for design:** `warp2ironhermes/project/app/app.jsx` (shell state + layout), `warp2ironhermes/project/app/shell.jsx` (presentational primitives), `warp2ironhermes/project/styles/warp-ih.css` (all UI styles), `warp2ironhermes/project/ironhermes/colors_and_type.css` (design tokens).

**Dioxus 0.7 API reference:** `AGENTS.md` at project root.

---

*State initialized: 2026-05-02*
