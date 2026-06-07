---
phase: 04-data-layer-interactions
plan: 01
subsystem: infra
tags: [dioxus, wasm, async, cargo, cfg-gated, platform, gloo-timers, tokio, web-sys]

# Dependency graph
requires:
  - phase: 01-hygiene
    provides: "Three-platform Cargo feature gate (web/desktop/mobile mutually exclusive); pinned dioxus =0.7.1; src/platform/ module stub"
  - phase: 03-desktop-shell
    provides: "Phase 3 component vocabulary (Block, BlockData, Mode, etc.) — consumed by later Phase 4 waves, not Wave 0 itself"
provides:
  - "Cargo.toml Phase 4 dep tree: gloo-timers 0.3 (futures), tokio 1 (time-only), web-sys 0.3 (7 explicit features), wasm-bindgen 0.2, js-sys 0.3"
  - "src/platform/timer.rs cfg-gated `pub async fn sleep(u32)` — single API on every platform; gloo_timers on wasm32, tokio::time on native"
  - "src/platform/mod.rs `pub mod timer;` registration (no re-export shortcut per D-04)"
  - "Three-platform compile gate established for Phase 4 (web/desktop/mobile all green)"
  - "Desktop tokio time-driver runtime availability proven via #[tokio::test] (resolves RESEARCH Q1)"
  - "[dev-dependencies] tokio with macros + rt + time — test-only; production tokio stays at time-only per D-05"
affects: [04-02, 04-03, 04-04a, 04-04b, 04-05, 06-mobile-shell]

# Tech tracking
tech-stack:
  added:
    - "gloo-timers 0.3 (futures feature) — wasm32 async sleep via TimeoutFuture::new(u32)"
    - "tokio 1 (time, default-features=false) — native async sleep; no full-runtime bloat"
    - "web-sys 0.3 with 7 explicit features (Window, Element, Document, Navigator, Clipboard, EventTarget, KeyboardEvent)"
    - "wasm-bindgen 0.2 — Closure bridging (consumed in later waves)"
    - "js-sys 0.3 — Date::new_0() for now_time() helper (Wave 1)"
    - "[dev-dependencies] tokio with macros + rt + time — #[tokio::test] support"
  patterns:
    - "cfg-gated function body (NOT module-level cfg) — single public signature, two bodies"
    - "Test-only feature isolation via [dev-dependencies] — production binary unaffected"

key-files:
  created:
    - "src/platform/timer.rs"
    - ".planning/phases/04-data-layer-interactions/04-01-SUMMARY.md"
  modified:
    - "Cargo.toml"
    - "src/platform/mod.rs"

key-decisions:
  - "tokio dev-dependency added with macros + rt features for #[tokio::test] support; production [dependencies] tokio stays at time-only (D-05 honored, no native binary impact)"
  - "Cfg gate placed inside function body rather than splitting into web_timer/native_timer modules (per D-04, RESEARCH Anti-Pattern row 3) — single import path crate::platform::timer::sleep on every platform"
  - "u64::from(ms) cast (NOT ms as u64 or .into()) for explicit clarity per PATTERNS risk note"
  - "No re-export shortcut in platform/mod.rs — callers import via full path crate::platform::timer::sleep (single import path; D-04)"

patterns-established:
  - "Pattern: cfg-gated async primitive with single public signature (timer::sleep) — template for any future cross-platform async/IO helper in src/platform/"
  - "Pattern: dev-dependency feature isolation for #[tokio::test] when production tokio is feature-minimized — keeps native binary lean while allowing test ergonomics"
  - "Pattern: per-Cargo-change three-platform compile gate (cargo build --features {web|desktop|mobile} --no-default-features) — Phase 4 inherits Phase 1 Plan 01-03's canonical verification"

requirements-completed: []

# Metrics
duration: ~3min
completed: 2026-05-03
---

# Phase 04 Plan 01: Cargo + cfg foundation Summary

**Phase 4 Wave 0 foundation: cfg-gated `timer::sleep(u32)` primitive with gloo-timers/tokio dual backing, five new deps wired into Cargo.toml with exact D-05 feature lists, three-platform compile gate green, and desktop tokio time-driver runtime availability proven via #[tokio::test].**

## Performance

- **Duration:** ~3 min (164s)
- **Started:** 2026-05-03T15:42:55Z
- **Completed:** 2026-05-03T15:45:39Z
- **Tasks:** 4 (all `type=auto`)
- **Files modified:** 3 (1 created, 2 modified)

## Accomplishments

- Phase 4 dependency tree wired into Cargo.toml with exact versions and feature flags from CONTEXT D-05 (gloo-timers 0.3 with futures, tokio 1 time-only with default-features=false, web-sys 0.3 with 7 explicit features, wasm-bindgen 0.2, js-sys 0.3); no `tokio = full`, no `chrono` per D-34.
- New `src/platform/timer.rs` exposing `pub async fn sleep(ms: u32)` with cfg-gated body — `gloo_timers::future::TimeoutFuture::new(ms).await` on wasm32, `tokio::time::sleep(Duration::from_millis(u64::from(ms))).await` on native. Single public signature on every platform; cfg branches inside the function body per D-04 / RESEARCH Pattern 1.
- `src/platform/mod.rs` upgraded from 3-line placeholder comment to `pub mod timer;` declaration (no re-export shortcut — single import path via `crate::platform::timer::sleep`).
- Three-platform compile gate green: `cargo build --features {web|desktop|mobile} --no-default-features` all exit 0 (D-35).
- Desktop tokio time-driver runtime availability proven by `#[tokio::test] desktop_sleep_does_not_panic` calling `sleep(10).await`; `cargo test --features desktop` passes (1 passed, 0 failed). Resolves RESEARCH Open Question Q1 (compile-time API existence vs runtime driver availability gap).

## Task Commits

Each task was committed atomically:

1. **Task 1: Add Phase 4 dependencies to Cargo.toml** — `166f538` (chore)
2. **Task 2: Create src/platform/timer.rs with cfg-gated sleep, register module** — `1cb06e1` (feat)
3. **Task 3: Three-platform compile gate (Phase 4 baseline)** — verification-only, no commit (per plan: `<files></files>` empty)
4. **Task 4: Desktop runtime smoke test for tokio::time::sleep** — `0da48ef` (test)

**Plan metadata commit:** pending (final commit after STATE.md / ROADMAP.md updates).

## Files Created/Modified

- `src/platform/timer.rs` (NEW, 39 lines) — Cross-platform async `sleep(u32)`; cfg-gated body branching between gloo_timers (wasm32) and tokio::time (native); appended `#[tokio::test]` smoke test for desktop runtime availability proof.
- `src/platform/mod.rs` (MODIFIED, replaced 3-line placeholder with 5-line module declaration) — `pub mod timer;` registration; doc comment references CONTEXT D-04 and Phase 6 forward link.
- `Cargo.toml` (MODIFIED, +18 lines) — five new entries in `[dependencies]` plus a new `[dev-dependencies]` block with tokio (macros + rt + time) for `#[tokio::test]` ergonomics; production tokio stays at time-only.

## Decisions Made

- **Test framework via dev-dependencies, not production deps:** The plan's failure-mode remediation suggested adding `tokio = { features = ["macros", "rt", "time"] }` to `[dev-dependencies]` if test compilation requires it. Honored that path: production `[dependencies] tokio` stays at `features = ["time"]` only (D-05 unchanged, no native binary impact); test ergonomics live in `[dev-dependencies]`. Rationale: keeps the native binary lean per RESEARCH Anti-Pattern guidance ("avoid `features = ["full"]`"), while enabling `#[tokio::test]` for the wave-end smoke gate.
- **Cfg gate inside function body** (per D-04, RESEARCH Pattern 1) — chose this over splitting into `mod web_timer; mod native_timer;` because (a) single import path for callers (`use crate::platform::timer::sleep;`), (b) public signature identical on every platform, (c) RESEARCH Anti-Pattern row 3 explicitly forbids the module-split form.
- **No re-export shortcut in `platform/mod.rs`** — `pub use timer::sleep;` was deliberately omitted per D-04 ("Single import for callers"); callers will import via the full path. Avoids two-import-paths anti-pattern.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added [dev-dependencies] tokio for #[tokio::test] support**
- **Found during:** Task 4 (Desktop runtime smoke test)
- **Issue:** The plan instructs to use `#[tokio::test]` and run `cargo test --features desktop -- desktop_sleep_does_not_panic`. The `#[tokio::test]` attribute requires tokio's `macros` AND `rt` features at compile time, but production `[dependencies] tokio` stays at `features = ["time"]` per D-05 to keep the native binary lean. Without dev-dependency tokio expansion, the test would fail to compile.
- **Fix:** Added a new `[dev-dependencies]` block with `tokio = { version = "1", features = ["macros", "rt", "time"], default-features = false }`. Production `[dependencies] tokio` remains at `time` only (D-05 honored).
- **Files modified:** Cargo.toml
- **Verification:** `cargo test --features desktop -- desktop_sleep_does_not_panic` exits 0 (1 passed; 0 failed); subsequent three-platform `cargo build --features {web|mobile} --no-default-features` re-runs both green (no native-binary impact from dev-deps).
- **Plan compliance:** This deviation is explicitly anticipated in the plan's Task 4 `<action>` block under "Failure modes and remediation": *"If the only failure is "test framework not found"... move the tokio macros feature into a [dev-dependencies] tokio = { features = ["macros", "rt"] } line (keep the production [dependencies] tokio line as features = ["time"] per D-05). This split is acceptable because dev-dependencies don't affect bundle size."* The remediation was applied preemptively rather than reactively because the requirement was documented and the failure was certain.
- **Committed in:** 0da48ef (Task 4 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Deviation was explicitly anticipated by the plan's failure-mode remediation; applied as written. No scope creep, no architectural change. D-05 production tokio feature set is unchanged.

## Issues Encountered

None. The dead_code warning on `pub async fn sleep` is expected and explicitly called out in the plan ("Warnings about `dead_code` for `sleep` (unused in Phase 4 Wave 1) are acceptable and expected — Wave 2 will introduce the first call site.").

## Next Phase Readiness

- **Wave 1 (Plan 04-02 — state.rs extensions) is unblocked.** The `crate::platform::timer::sleep` symbol is reachable; Wave 1's `now_time()` cfg-gated helper has the precedent pattern to follow.
- **Wave 2 (Plan 04-03 — mocks/) is unblocked.** Wave 2's `run_shell` and `run_agent_steps` async functions can call `sleep(600).await`, `sleep(400).await`, `sleep(1000).await` etc. without further Cargo.toml or platform/ changes.
- **Three-platform gate established as the canonical Phase 4 verification.** Subsequent plans can re-run `cargo build --features {web|desktop|mobile} --no-default-features` as their wave-end gate.
- **Tokio runtime time-driver availability proven for desktop builds.** RESEARCH Q1 closed; no blocker for Plan 04-05's desktop UAT (no panic-on-first-submit risk).

## Self-Check: PASSED

- `src/platform/timer.rs` — FOUND
- `src/platform/mod.rs` — FOUND (modified, contains `pub mod timer;`)
- `Cargo.toml` — FOUND (modified, contains gloo-timers/tokio/web-sys/wasm-bindgen/js-sys deps + dev-dependencies)
- `.planning/phases/04-data-layer-interactions/04-01-SUMMARY.md` — FOUND
- Commit `166f538` (Task 1: chore — Phase 4 deps) — FOUND
- Commit `1cb06e1` (Task 2: feat — cfg-gated sleep + module declaration) — FOUND
- Commit `0da48ef` (Task 4: test — desktop runtime smoke test) — FOUND

---
*Phase: 04-data-layer-interactions*
*Completed: 2026-05-03*
