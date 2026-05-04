---
phase: 03-desktop-shell
plan: 04
subsystem: ui
tags: [dioxus, dioxus-0.7, rust, top-level-composer, three-platform-gate, build-restored]

# Dependency graph
requires:
  - phase: 03-desktop-shell
    plan: 01
    provides: "src/state.rs (demo_blocks/demo_messages/demo_palette_items/demo_tabs + Block/Token/Mode/PaletteItem/Tab/Message/TokenBudget); assets/scanner-anim.css; src/components/shell/mod.rs declarations"
  - phase: 03-desktop-shell
    plan: 02
    provides: "5 leaf shell primitives (Sigil, Scanner, CommandLine, ToolCall, StatusBar)"
  - phase: 03-desktop-shell
    plan: 03
    provides: "6 composite shell primitives (TitleBar, Block, BlockStream, InputBox, AgentPanel, CommandPalette) + WORDMARK_SVG/IH_SHIELD_PNG migrated to title_bar.rs"
provides:
  - "src/components/warp_hermes.rs: pure-static `pub fn WarpHermes() -> Element` composer wiring all 6 user-visible primitives under .wh-app with hardcoded data-theme=cyan / data-density=comfy / data-block=framed / data-agent=right"
  - "src/app.rs: root component swapped from Hero {} to WarpHermes {}; new SCANNER_ANIM_CSS Asset const; assets/scanner-anim.css linked into the document::Link cascade AFTER warp-ih.css"
  - "src/components/hero.rs: deleted (Phase 2 brand stub); asset constants previously migrated to title_bar.rs in Plan 03-03"
  - "Three-platform compile gate restored to GREEN: cargo build --features {web|desktop|mobile} all exit 0; cargo clippy --features web -- -D warnings exits 0"
affects: [03-05-mobile-shell, 04-data-layer-and-interactions, 05-tweaks-panel]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Top-level composer pattern: pure-static `#[component] pub fn WarpHermes() -> Element` invokes `demo_*()` fixture functions inline at function entry, then composes 6 primitives under a `.wh-app` wrapper with 4 hardcoded `data-*` attributes — zero use_signal/use_memo/use_resource (CONTEXT D-06)"
    - "Cascade-order discipline: scanner-anim.css `<link>` MUST come after warp-ih.css so `.wh-scanner.is-active span:nth-child(N) { animation-delay: ... }` rules win cascade specificity ties (RESEARCH Pattern 7)"
    - "Public API re-export silencing: `#[allow(unused_imports)]` on each `pub use foo::Foo;` line in `shell/mod.rs` keeps the public API surface intact for Phase 4/5/6 callers while satisfying the -D warnings clippy gate during Phase 3 (only 6 of 11 primitives are used at this layer; the other 5 are composed via super:: paths internally)"
    - "Public API enum-variant silencing: `#[allow(dead_code)]` on individual enum variants (`Token::Str`, `Mode::Agent`) preserves API surface for Phase 4 data layer while keeping clippy happy"
    - "Three-platform compile gate as the canonical phase-level automated verification: `cargo build --features web` + `cargo build --features desktop` + `cargo build --features mobile` + `cargo clippy --features web -- -D warnings`, all four sequential and all four must exit 0 (Phase 1 inheritance per VALIDATION.md)"

key-files:
  created:
    - "src/components/warp_hermes.rs"
    - ".planning/phases/03-desktop-shell/03-04-SUMMARY.md"
  modified:
    - "src/app.rs (Hero → WarpHermes; +SCANNER_ANIM_CSS const + link)"
    - "src/main.rs (drop unused dioxus::prelude::* import)"
    - "src/components/shell/mod.rs (#[allow(unused_imports)] on each pub use line)"
    - "src/state.rs (#[allow(dead_code)] on Token::Str + Mode::Agent + doc-comments)"
  deleted:
    - "src/components/hero.rs"

key-decisions:
  - "Restored the green build by composing the top-level WarpHermes inside `src/components/warp_hermes.rs` exactly as the plan prescribed — zero structural deviation from the prescribed action block. Pure-static composer wires every prototype-default value verbatim per UI-SPEC: tabs=demo_tabs(), active_tab=0, show_traffic_lights=true, blocks=demo_blocks(), focused=true (planner-handoff #4), mode=Mode::Shell, status-pill copy 'Chat'/'claude-sonnet-4'/'anthropic'/12.3K of 128K, scanner_active=true (D-08), hint=verbatim UI-SPEC line 217, messages=demo_messages(), personality='default', items=demo_palette_items(), open=true (D-19)"
  - "Cascade order locked: scanner-anim.css link goes AFTER warp-ih.css. Rationale: scanner-anim.css uses `.wh-scanner.is-active span:nth-child(N) { animation-delay: ... }` rules whose `@keyframes wh-scanner-tick` reference is defined in the same file; warp-ih.css declares the base `.wh-scanner` styling. Putting scanner-anim.css after warp-ih.css guarantees its rules win specificity ties and the @keyframes resolves on the active class toggle."
  - "Phase 3 fixture chrome forced inline warning fixes (Rule 1/3 deviation): cargo clippy --features web -- -D warnings is the canonical Phase 3 gate. The Phase-3-staged primitives carry forward intentional Phase-4-deferred surface (Token::Str, Mode::Agent, the 5 internally-composed primitives' pub use re-exports, and the leftover dioxus::prelude::* import in main.rs from the Hero era). All would have failed the -D warnings clippy gate. Fixed inline rather than escalated as a checkpoint because the plan's Task 4 'Common failure modes' rubric explicitly anticipated #[allow(dead_code)] silencing as the right resolution."

requirements-completed: [SHELL-01, SHELL-02, SHELL-03, SHELL-04, SHELL-05, SHELL-06, SHELL-07, SHELL-08, SHELL-09, SHELL-10]

# Metrics
duration: 4min
completed: 2026-05-03
---

# Phase 3 Plan 04: WarpHermes Composer + Build-Restore Summary

**Lands the top-level `WarpHermes` composer that wires every Phase 3 shell primitive into a fully-rendered desktop shell, swaps `src/app.rs` from the Phase 2 brand stub to the new composer, links the scanner-animation CSS into the document cascade, deletes the now-orphan `src/components/hero.rs`, and restores the three-platform compile gate (web + desktop + mobile + clippy under `-D warnings`) to green — closing the deliberately-broken-build interval that Plans 03-01..03-03 staged.**

## One-liner

Top-level pure-static composer assembled from 6 fixture-fed primitives under a `.wh-app` wrapper with the four prototype-default `data-*` attributes; cascade-correct `<link>` for scanner CSS; canonical Phase 3 automated gate restored to green.

## Performance

- **Duration:** ~4 min (~4m23s wall-clock)
- **Started:** 2026-05-03T10:02:56Z
- **Completed:** 2026-05-03T10:07:19Z
- **Tasks:** 4 / 4
- **Files created:** 1 (warp_hermes.rs)
- **Files modified:** 4 (app.rs, main.rs, shell/mod.rs, state.rs)
- **Files deleted:** 1 (hero.rs)
- **Commits:** 4 atomic per-task commits + 1 SUMMARY commit (this file, made by orchestrator after merge)

### Three-platform compile gate timings

| Gate | Command | Duration | Exit | Warnings |
|------|---------|----------|------|----------|
| 1 | `cargo build --features web` (incremental, after deviation fix) | 0.55s | 0 | 0 |
| 2 | `cargo build --features desktop` (full, first run) | 16.25s | 0 | 0 |
| 3 | `cargo build --features mobile` (incremental) | 2.55s | 0 | 0 |
| 4 | `cargo clippy --features web -- -D warnings` | 6.80s | 0 | 0 |

(The first `cargo build --features web` run before the warning fix took 11.15s and emitted 8 warnings — none were errors; all were Phase-4-deferred-surface warnings that would have failed clippy under `-D warnings`. See "Deviations" below.)

## Accomplishments

### Task 1: `src/components/warp_hermes.rs` (55 lines, new file)

`pub fn WarpHermes() -> Element` — a single `#[component]`-annotated function returning `rsx!`. Imports the 6 user-visible composites (`AgentPanel`, `BlockStream`, `CommandPalette`, `InputBox`, `StatusBar`, `TitleBar`) from `crate::components::shell` and the 4 fixture functions + `Mode` + `TokenBudget` from `crate::state`. Body declares the four owned-Vec values + a `TokenBudget { used: 12_300, max: 128_000 }` literal, then renders:

```
.wh-app[data-theme=cyan][data-density=comfy][data-block=framed][data-agent=right]
├── TitleBar { tabs, active_tab: 0, show_traffic_lights: true }
├── .wh-main
│   ├── .wh-col
│   │   ├── BlockStream { blocks }
│   │   ├── InputBox { mode: Mode::Shell, focused: true }
│   │   └── StatusBar { mode "Chat", model "claude-sonnet-4", provider "anthropic",
│   │                    tokens, scanner_active: true, hint "/help · ⌃C cancel · ⌘K palette" }
│   └── AgentPanel { messages, personality "default" }
└── CommandPalette { items, query "", open: true }
```

Hardcoded prop values per CONTEXT D-19 / D-20 + UI-SPEC tables:

| Primitive | Prop | Value | Rationale |
|-----------|------|-------|-----------|
| TitleBar | `tabs` | `demo_tabs()` | 3-tab fixture from state.rs |
| TitleBar | `active_tab` | `0_usize` | UI-SPEC line 207 ("first tab is is-active") |
| TitleBar | `show_traffic_lights` | `true` | Desktop default; mobile shell flips |
| BlockStream | `blocks` | `demo_blocks()` | 10-block fixture |
| InputBox | `mode` | `Mode::Shell` | UI-SPEC line 461 |
| InputBox | `focused` | `true` | UI-SPEC planner-handoff #4 (focus ring visible during UAT) |
| StatusBar | `mode` | `"Chat"` | UI-SPEC line 217 |
| StatusBar | `model` | `"claude-sonnet-4"` | UI-SPEC line 217 |
| StatusBar | `provider` | `"anthropic"` | UI-SPEC line 217 |
| StatusBar | `tokens` | `TokenBudget { used: 12_300, max: 128_000 }` | UI-SPEC lines 169 / 216 → renders `12.3K/128K (10%)` |
| StatusBar | `scanner_active` | `true` | CONTEXT D-08 (always-on for UAT visibility) |
| StatusBar | `hint` | `"/help · ⌃C cancel · ⌘K palette"` | UI-SPEC line 217 verbatim |
| AgentPanel | `messages` | `demo_messages()` | 5-message fixture |
| AgentPanel | `personality` | `"default"` | UI-SPEC line 243 |
| CommandPalette | `items` | `demo_palette_items()` | 10-item fixture (6 slash + 4 workflow) |
| CommandPalette | `query` | `String::new()` | Empty search query for Phase 3 |
| CommandPalette | `open` | `true` | CONTEXT D-19 (overlay visible by default for UAT) |

Zero `use_signal` / `use_memo` / `use_resource` / `onclick` / `oninput` / `onkeydown` / `onfocus` / `onblur` calls (verified via `grep -c` returning `0`).

### Task 2: `src/app.rs` (mechanical 3-line change + 1-line swap)

Concrete diff:

```diff
 use dioxus::prelude::*;
-use crate::components::Hero;
+use crate::components::WarpHermes;

 const FAVICON: Asset = asset!("/assets/favicon.ico");
 const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
 const MAIN_CSS: Asset = asset!("/assets/main.css");
 const DESIGN_TOKENS_CSS: Asset = asset!("/assets/design-tokens.css");
 const WARP_IH_CSS: Asset = asset!("/assets/warp-ih.css");
+const SCANNER_ANIM_CSS: Asset = asset!("/assets/scanner-anim.css");

 #[component]
 pub fn App() -> Element {
     rsx! {
         document::Link { rel: "icon", href: FAVICON }
         document::Link { rel: "stylesheet", href: TAILWIND_CSS }
         document::Link { rel: "stylesheet", href: MAIN_CSS }
         document::Link { rel: "stylesheet", href: DESIGN_TOKENS_CSS }
         document::Link { rel: "stylesheet", href: WARP_IH_CSS }
+        document::Link { rel: "stylesheet", href: SCANNER_ANIM_CSS }
-        Hero {}
+        WarpHermes {}
     }
 }
```

Cascade order verified: `awk '/href: WARP_IH_CSS/{warp=NR} /href: SCANNER_ANIM_CSS/{scan=NR} END{exit !(scan>warp)}' src/app.rs` → exit 0 (scanner link comes after warp-ih link).

### Task 3: `src/components/hero.rs` deleted

`git rm src/components/hero.rs`. The file was 15 lines; its `WORDMARK_SVG` and `IH_SHIELD_PNG` constants were already migrated to `src/components/shell/title_bar.rs` in Plan 03-03 with `#[allow(dead_code)]`. Asset files (`assets/wordmark.svg`, `assets/ih-shield.png`) remain on disk for Phase 5 TweaksPanel use. No dangling references: `! grep -rq 'mod hero' src/` and `! grep -rq 'use crate::components::Hero' src/` both pass; `Hero {}` invocations have zero hits.

### Task 4: Three-platform compile gate green (canonical Phase 3 automated gate)

Sequential execution per Plan 03-04 Task 4 (shared `target/`, incremental rebuilds per feature):

1. `cargo build --features web` → exit 0 (initial run: 11.15s with 8 warnings — all Phase-4-deferred surface, no errors)
2. **Inline deviation fix** — see "Deviations" below
3. `cargo build --features web` (re-run) → exit 0, 0.55s, 0 warnings
4. `cargo build --features desktop` → exit 0, 16.25s, 0 warnings
5. `cargo build --features mobile` → exit 0, 2.55s, 0 warnings
6. `cargo clippy --features web -- -D warnings` → exit 0, 6.80s, 0 warnings, 0 errors

Plan-level verification:
- ✓ `test -f src/components/warp_hermes.rs`
- ✓ `! test -f src/components/hero.rs`
- ✓ `grep -q 'WarpHermes {}' src/app.rs`
- ✓ `grep -q 'SCANNER_ANIM_CSS' src/app.rs`

## Task Commits

Each task committed atomically on branch `worktree-agent-a2484f4e17255ca36`:

1. **Task 1: Add WarpHermes top-level shell composer** — `baa8385` (feat)
2. **Task 2: Swap App root to WarpHermes; link scanner-anim.css** — `45ab15b` (feat)
3. **Task 3: Delete src/components/hero.rs (replaced by WarpHermes)** — `d4398ef` (chore)
4. **(deviation) Silence warnings to satisfy Phase 3 -D warnings clippy gate** — `cf4487c` (fix) — prerequisite for Task 4 acceptance

_Plan-metadata commit (this SUMMARY.md) is created next; STATE.md / ROADMAP.md updates are deferred to the orchestrator after wave merge per parallel-executor protocol._

## Files Created/Modified

- `src/components/warp_hermes.rs` — **created** (55 lines). Component: `WarpHermes` (no props). Imports 6 composites + 4 fixture functions + 2 types from state.
- `src/app.rs` — **modified** (4 line additions, 2 line swaps). Adds `SCANNER_ANIM_CSS` const + cascade-correct `<link>`; swaps `Hero` import + invocation to `WarpHermes`.
- `src/components/hero.rs` — **deleted** (was 15 lines). All references already removed by Plan 03-01 (mod.rs) and Plan 03-04 Task 2 (app.rs).
- `src/main.rs` — **modified** (deviation fix). Dropped unused `use dioxus::prelude::*;` left over from the Hero brand-stub era.
- `src/components/shell/mod.rs` — **modified** (deviation fix). Added `#[allow(unused_imports)]` to each `pub use foo::Foo;` line + 7-line explanatory doc comment above the block. Public API surface unchanged.
- `src/state.rs` — **modified** (deviation fix). Added `#[allow(dead_code)]` to `Token::Str` and `Mode::Agent` enum variants; doc comments updated to explain Phase 4 deferral.

## Decisions Made

- **Verbatim plan execution for warp_hermes.rs.** The plan's `<action>` block in 03-04-PLAN.md provides a near-complete code listing; I followed it byte-for-byte. The four shell composite invocations (`TitleBar`/`BlockStream`/`InputBox`/`AgentPanel`) match the exact signatures verified by grep against `src/components/shell/*.rs` heads (TitleBar takes `tabs/active_tab/show_traffic_lights`; BlockStream takes `blocks`; InputBox takes `mode/focused`; AgentPanel takes `messages/personality`; CommandPalette takes `items/query/open`; StatusBar takes `mode/model/provider/tokens/scanner_active/hint`). Zero structural deviation.
- **Cascade order: scanner-anim.css AFTER warp-ih.css.** `assets/warp-ih.css` declares `.wh-scanner` base styling; `assets/scanner-anim.css` declares `@keyframes wh-scanner-tick` and the `.wh-scanner.is-active span:nth-child(N)` rules. Putting scanner-anim.css after warp-ih.css ensures (a) the @keyframes is resolved by the time the active class toggles, and (b) any cascade-tie on scanner styling is won by the more-specific (and animation-aware) rules. The pre-existing Phase 2 cascade order (TAILWIND_CSS → MAIN_CSS → DESIGN_TOKENS_CSS → WARP_IH_CSS) is preserved unchanged per Phase 2 STATE.md decision.
- **Inline warning fixes accepted as the right Task 4 resolution.** Plan 03-04 Task 4's "Common failure modes" rubric explicitly listed `#[allow(dead_code)]` silencing as the prescribed fix for `-D warnings` failures; the plan's executor `<action>` block instructs "Fix the offending file in this same task" when builds fail. Three of the 8 warnings (Token::Str, Mode::Agent enum-variant dead-code) are direct matches for that rubric. The remaining 5 (5 unused `pub use` re-exports in shell/mod.rs and 1 unused import in main.rs) are structurally similar — public API surface preserved for Phase 4/5/6 callers, dead code only at the Phase 3 boundary. Silencing with `#[allow(unused_imports)]` is the minimal fix that respects both correctness gates simultaneously.
- **Worktree asset seeding (operational, not source-tracked).** Per the Plan 03-01 / 03-02 / 03-03 SUMMARY notes about the recurring worktree-seeding gap, I copied `assets/favicon.ico` and `assets/tailwind.css` from `/Users/twilson/code/iron_hermes_ui/assets/` into the worktree's `assets/` directory before running the build gate. Both files remain `git status`-untracked (matching main-repo state — favicon.ico is untracked in main; tailwind.css is a `dx serve` build output). NOT committed; this is worktree infrastructure parity, not a plan deliverable.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug / Rule 3 - Blocker] Silenced 8 -D warnings clippy failures from Phase-4-deferred surface**

- **Found during:** Task 4 (initial `cargo build --features web` run)
- **Issue:** 8 warnings emitted from sources outside Plan 03-04's direct deliverables:
  - `src/main.rs:1` — unused `use dioxus::prelude::*;` (left over from the Hero brand-stub era; main.rs only declares modules and launches App)
  - `src/components/shell/mod.rs:14, 16, 17, 18, 22` — 5 unused `pub use` re-exports for `Sigil`, `Block`, `CommandLine`, `ToolCall`, `Scanner` (composed internally via `super::*` paths inside other primitives; the re-exports are public API surface for Phase 4/5/6 callers)
  - `src/state.rs:79` — `Token::Str` enum variant unconstructed (Phase 3 fixtures use Bin/Arg/Flag only; Phase 4 will use Str for quoted shell args)
  - `src/state.rs:121` — `Mode::Agent` enum variant unconstructed (Phase 3 hardcodes `Mode::Shell` per UI-SPEC line 461; Phase 4 will exercise `Agent` via the `⌥+M` keybind)
- **Why this blocks:** `cargo clippy --features web -- -D warnings` (Plan 03-04 Task 4's last gate) treats warnings as errors. Without these fixes the canonical Phase 3 automated gate would fail.
- **Fix:** Per Plan 03-04 Task 4's "Common failure modes" rubric (which explicitly anticipates `#[allow(dead_code)]` silencing of unused public surface):
  - Removed the unused prelude import from main.rs.
  - Added `#[allow(unused_imports)]` to each `pub use` line in shell/mod.rs + a 7-line explanatory doc comment above the block.
  - Added `#[allow(dead_code)]` to `Token::Str` and `Mode::Agent` enum variants + doc comments explaining Phase 4 deferral.
- **Files modified:** `src/main.rs`, `src/components/shell/mod.rs`, `src/state.rs`
- **Commit:** `cf4487c` (fix(03-04): silence warnings to satisfy Phase 3 -D warnings clippy gate)
- **Validation:** Re-run of all 4 gates produced 0 warnings and 0 errors.

No other deviations — all 4 prescribed tasks executed exactly as the plan's `<action>` blocks specified. The 4 task commits and 1 deviation commit map cleanly to the plan's structure.

## Authentication Gates

None — Phase 3 has no external services (PROJECT.md mandate: "no API keys, no network calls in v1"). The only "gate" is the canonical three-platform compile gate, fully automated.

## Issues Encountered

- **Pre-existing worktree-isolation gap (recurring across 03-01..03-04, not in scope of this plan's deliverables):** `assets/favicon.ico` (untracked in main repo) and `assets/tailwind.css` (`dx serve` build output, not committed) were absent from the worktree initially. Copied from the main checkout into the worktree before the build gate. Files remain untracked, matching main-repo state. Surfaced for orchestrator tracking; not auto-fixed as a Rule 1/2/3 deviation because the upstream cause is worktree-seeding behavior outside this plan's responsibility.

## User Setup Required

None.

## Manual UAT Status

Plan 03-05 is the manual UAT plan. To verify visually:
1. Run `dx serve --features web` from the project root
2. Open the browser to the served URL (typically `http://localhost:8080`)
3. Walk through SC-1..SC-N from `.planning/phases/03-desktop-shell/03-VALIDATION.md`

The composer is intentionally pre-configured for UAT: command palette open, focus ring visible on InputBox, scanner running continuously, `cyan` theme, `framed` blocks, agent panel on the right.

## Next Phase Readiness

- **Plan 03-05 (Mobile Shell):** can `use crate::components::shell::*;` and reuse all 11 primitives. Mobile-specific variations (`data-density="compact"`, `data-agent="hidden"`, traffic-lights hidden, bottom tab bar) are toggleable via attributes on the `.wh-app` wrapper or via per-platform branches in a `WarpHermesMobile` composer. The pure-static contract from CONTEXT D-06 carries forward.
- **Phase 4 (Data Layer + Interactions):** the `WarpHermes` composer is the natural insertion point for the runtime state hooks. Phase 4's `WarpHermesAttrs` extraction will hoist the four `data-*` attributes out of literals into `use_context`-backed signals so the TweaksPanel can flip them at runtime. The `demo_*()` fixture calls become the seed values for the runtime stores. Existing primitives accept owned-Vec props; Phase 4 will likely change them to `ReadOnlySignal<Vec<T>>` and update prop sites accordingly.
- **Phase 5 (TweaksPanel):** the four `data-*` attributes on `.wh-app` are the exact attribute set the TweaksPanel will manipulate. CONTEXT D-19 hardcoding `open: true` on CommandPalette will need to flip to a runtime signal at that point.
- **Phase 6 (per-platform polish):** the three-platform compile gate is now the established green baseline. Future plans must keep all four sub-gates passing.
- **UI-SPEC planner-handoff status (post Plan 03-04):**
  - **#1 (scanner animation gap):** RESOLVED in Plan 03-01 (assets/scanner-anim.css with @keyframes wh-scanner-tick). Confirmed wired by Plan 03-04 Task 2 (cascade-correct `<link>` after warp-ih.css).
  - **#2 (wordmark/shield destination):** RESOLVED in Plan 03-03 (migrated to title_bar.rs with `#[allow(dead_code)]`). Confirmed by Plan 03-04 Task 3 (hero.rs deleted; assets unreferenced but on disk).
  - **#3 (Out-block body fidelity):** PROVISIONALLY RESOLVED with Approach A (plain text in .wh-block-body). Phase 3 UAT (Plan 03-05) is the gate for escalation.
  - **#4 (focus ring visibility):** RESOLVED in this plan — InputBox is invoked with `focused: true` at the WarpHermes call site, so the focus ring is visible during UAT.

## Threat Surface

Reviewed Plan 03-04's threat register (T-03-12 Tampering / T-03-13 Information disclosure / T-03-14 Denial of service / T-03-15 Repudiation — all `accept` dispositions). No new surface introduced beyond what was modelled:

- `assets/scanner-anim.css` link uses `asset!()` build-time hashing (T-03-12 accept).
- All fixture data is project-authored prototype-replica copy; no PII / secrets per PROJECT.md (T-03-13 accept).
- @keyframes runs on GPU compositor with no Rust runtime cost (T-03-14 accept).
- hero.rs deletion is preserved in git history; asset migration to title_bar.rs is documented (T-03-15 accept).

No threat flags raised. The deviation fix files (main.rs, shell/mod.rs, state.rs) are pure annotation changes that affect zero runtime behavior.

## Self-Check: PASSED

Verified after writing this summary:

- ✓ `src/components/warp_hermes.rs` exists (55 lines)
- ✓ `! test -f src/components/hero.rs` (deleted)
- ✓ `grep -q 'WarpHermes {}' src/app.rs` (root component swapped)
- ✓ `grep -q 'SCANNER_ANIM_CSS' src/app.rs` (animation CSS linked)
- ✓ `grep -q 'use crate::components::WarpHermes;' src/app.rs` (Hero import removed)
- ✓ `! grep -rq 'mod hero' src/`
- ✓ `! grep -rq 'use crate::components::Hero' src/`
- ✓ `! grep -rq 'Hero {}' src/`
- ✓ `test -f assets/wordmark.svg && test -f assets/ih-shield.png` (asset files preserved)
- ✓ `grep -q 'WORDMARK_SVG' src/components/shell/title_bar.rs` (migration intact)
- ✓ Commit `baa8385` (Task 1 — feat: WarpHermes composer) present in `git log`
- ✓ Commit `45ab15b` (Task 2 — feat: app.rs swap + scanner-anim link) present in `git log`
- ✓ Commit `d4398ef` (Task 3 — chore: hero.rs deletion) present in `git log`
- ✓ Commit `cf4487c` (deviation — fix: warning silencing) present in `git log`
- ✓ `cargo build --features web` exits 0
- ✓ `cargo build --features desktop` exits 0
- ✓ `cargo build --features mobile` exits 0
- ✓ `cargo clippy --features web -- -D warnings` exits 0
- ✓ `grep -c 'use_signal\|use_memo\|use_resource\|onclick\|oninput\|onkeydown' src/components/warp_hermes.rs` returns `0` (pure-static rule honored)
- ✓ STATE.md NOT modified (parallel-executor protocol — orchestrator owns post-wave-merge state updates)
- ✓ ROADMAP.md NOT modified (same reason)

---
*Phase: 03-desktop-shell*
*Plan: 04 — WarpHermes Composer + Build-Restore*
*Completed: 2026-05-03*
