---
phase: 19-skills-framework
plan: 05
subsystem: security
tags: [skills, security, scanning, prompt-injection, regex, tracing]

requires:
  - phase: 19-skills-framework-01
    provides: SkillSource enum + SkillRecord.source field
provides:
  - scan_skill_content() in context_scanner.rs runs context THREAT_PATTERNS + SKILL_THREAT_PATTERNS
  - SKILL_THREAT_PATTERNS RegexSet with 30 patterns across 5 categories (tool-redef, sys-prompt-override, role-markers, agent-config-persistence, cred-exfil)
  - Registry-load scan in skills.rs::load_with_paths (D-16) with frontmatter+body scope (D-14)
  - D-15 enforcement branches: Community->hard-reject (continue), Builtin/Official->tracing::warn!
  - extract_raw_frontmatter helper for D-14 combined scan target
  - #[cfg(test)] SkillRegistry::load_with_paths_for_test constructor injecting source + re-applying D-15
affects: [19-06, 19.1, 20+]

tech-stack:
  added: []
  patterns:
    - "Skill-specific RegexSet layered on top of existing context RegexSet via short-circuit composition"
    - "Source-differentiated policy enforcement at registry-load time (not per-activation)"

key-files:
  created: []
  modified:
    - crates/ironhermes-core/src/context_scanner.rs
    - crates/ironhermes-core/src/skills.rs

key-decisions:
  - "scan_skill_content runs context THREAT_PATTERNS first and short-circuits on hit before testing SKILL_THREAT_PATTERNS — preserves separation while ensuring both fire"
  - "Scan scope = raw frontmatter text + body (D-14) via new extract_raw_frontmatter helper that reuses parse_skill_md delimiter logic"
  - "Production load_with_paths hardcodes source=Builtin (Phase 19 A4 default); Phase 19.1 will plumb real provenance before the match block"
  - "Test-only load_with_paths_for_test re-applies D-15 after injecting source so community-hard-reject path is exercised without waiting for 19.1"

patterns-established:
  - "LazyLock<RegexSet> + .matches().into_iter() + indexed pattern names mirrors existing THREAT_PATTERNS shape"
  - "[BLOCKED: filename contained potential prompt injection (ids). Content not loaded.] return-string contract (Pitfall 4)"

requirements-completed: [SKILL-07]

duration: 8 min
completed: 2026-04-14
---

# Phase 19 Plan 05: Skill Security Scanning Summary

**SKILL_THREAT_PATTERNS RegexSet (30 patterns, 5 categories) + scan_skill_content layered on context THREAT_PATTERNS, wired into load_with_paths with D-15 source-differentiated enforcement (Community hard-reject / Builtin+Official WARN-BUT-LOAD) at registry-load time over frontmatter+body scope.**

## Performance

- **Duration:** ~8 min
- **Started:** 2026-04-14 (session continuation after 19-04)
- **Completed:** 2026-04-14
- **Tasks:** 2 (TDD RED + GREEN)
- **Files modified:** 2

## Accomplishments
- `scan_skill_content(content, filename) -> String` with `[BLOCKED: ...]` prefix contract identical to `scan_context_content`
- `SKILL_THREAT_PATTERNS` RegexSet covers all 5 categories from hermes-agent `skills_guard.py` (Cat1 privesc, Cat2 sys-prompt-override, Cat3 role markers, Cat4 agent-config persistence, Cat5 credential exfil)
- Registry-load call site in `load_with_paths` scans combined `raw_frontmatter_text + "\n\n" + body` (D-14) once per skill (D-16)
- D-15 enforcement match branches: `Community => continue` hard-reject; `Builtin | Official => tracing::warn!` WARN-BUT-LOAD
- `extract_raw_frontmatter` helper mirrors `parse_skill_md` delimiter logic to expose the raw YAML block for scanning
- `#[cfg(test)] load_with_paths_for_test(paths, source)` constructor retrofits the match-on-source behavior so both branches are tested today without waiting for Phase 19.1 provenance plumbing
- 12 new tests (9 scanner + 3 enforcement) all green; all 145 ironhermes-core tests pass (zero regressions)

## Task Commits

1. **Task 1: RED — failing tests for scan_skill_content + D-15 enforcement** — `386030e` (test)
2. **Task 2: GREEN — implement SKILL_THREAT_PATTERNS + scan_skill_content + load_with_paths wiring + test-only constructor** — `4dcaaa0` (feat)

**Plan metadata:** (this SUMMARY commit)

## Files Created/Modified
- `crates/ironhermes-core/src/context_scanner.rs` — added `SKILL_THREAT_PATTERNS` LazyLock + `pub fn scan_skill_content` + 9 tests
- `crates/ironhermes-core/src/skills.rs` — added `extract_raw_frontmatter` helper, wired scan into `load_with_paths` with match-on-source enforcement, replaced hardcoded `source: SkillSource::Builtin` with `source` variable (value unchanged at Builtin for Phase 19 per A4), added `#[cfg(test)] load_with_paths_for_test` constructor, added 3 enforcement tests

## Decisions Made
- **Short-circuit composition**: `scan_skill_content` calls `scan_context_content` first, returning its `[BLOCKED: ...]` string immediately on hit. Avoids duplicating context patterns and keeps the context scanner stable for SOUL/AGENTS/system_message callers.
- **Raw frontmatter extraction via delimiter re-walk** (not threading a new return value through `parse_skill_md`) to keep `parse_skill_md`'s signature stable for all existing callers.
- **Test-only `load_with_paths_for_test`**: production path still hardcodes `source = SkillSource::Builtin` (A4). The test helper injects source and re-scans on Community to exercise the hard-reject branch. Phase 19.1 will flip this when real provenance lands — the match-on-source skeleton is already correct.

## Deviations from Plan

None — plan executed exactly as written. Implementation followed the plan's suggested code structure and regex literals verbatim from RESEARCH.md §Instruction-Smuggling Patterns.

## Issues Encountered
None.

## User Setup Required
None — no external service configuration required.

## Next Phase Readiness
- D-13/D-14/D-15/D-16 are all satisfied at the registry-load layer. Malicious community skills can never enter `registry.skills` (so `handle_activate` cannot target them); malicious builtin/official skills log a warning but still load so accidental regex false-positives cannot brick first-party skills.
- Ready for **19-06** (remaining plan in phase 19).
- Phase 19.1 will plumb real provenance (per-path or per-manifest source labeling) into `load_with_paths`; the match-on-source skeleton and enforcement tests are already in place.

## Self-Check: PASSED

Verified:
- `SKILL_THREAT_PATTERNS` exists in `crates/ironhermes-core/src/context_scanner.rs` (line 44)
- `pub fn scan_skill_content` exists in `crates/ironhermes-core/src/context_scanner.rs` (line 131)
- 30 regex literals inside `SKILL_THREAT_PATTERNS` (≥25 required)
- `scan_skill_content(` called inside `load_with_paths` in `crates/ironhermes-core/src/skills.rs` (line 422)
- `SkillSource::Community` and `continue` appear in proximity inside `load_with_paths` (lines 432-439)
- `"WARN-BUT-LOAD"` string literal inside `skills.rs` (line 444)
- `cargo test -p ironhermes-core` → 145 passed, 0 failed, 3 ignored
- `cargo build --workspace` → Finished (dev profile, 4 pre-existing warnings unrelated to this plan)
- Commits `386030e` (RED) and `4dcaaa0` (GREEN) present in `git log`

---
*Phase: 19-skills-framework*
*Completed: 2026-04-14*
